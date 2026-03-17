//! LSP server for jonesy panic point diagnostics.
//!
//! This module implements a Language Server Protocol server that publishes
//! panic point diagnostics to IDEs and code editors. It runs alongside
//! rust-analyzer, publishing its own diagnostics.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::call_tree::CrateCodePoint;

/// State shared across the LSP server
struct ServerState {
    /// Workspace root path
    workspace_root: Option<PathBuf>,
    /// Cached panic points by file URI
    panic_points: HashMap<Url, Vec<CrateCodePoint>>,
}

impl ServerState {
    fn new() -> Self {
        Self {
            workspace_root: None,
            panic_points: HashMap::new(),
        }
    }
}

/// The jonesy LSP server backend
pub struct JonesyLspServer {
    client: Client,
    state: Arc<RwLock<ServerState>>,
    /// Lock to serialize analysis runs and prevent out-of-order diagnostics
    analysis_lock: Arc<Mutex<()>>,
}

impl JonesyLspServer {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            state: Arc::new(RwLock::new(ServerState::new())),
            analysis_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Convert a CrateCodePoint to an LSP Diagnostic
    fn code_point_to_diagnostic(point: &CrateCodePoint) -> Diagnostic {
        // Get primary cause for the message
        let (message, suggestion) = if let Some(cause) = point.causes.iter().next() {
            (
                format!("panic point: {}", cause.description()),
                Some(cause.suggestion().to_string()),
            )
        } else {
            ("potential panic point".to_string(), None)
        };

        let range = Range {
            start: Position {
                line: point.line.saturating_sub(1), // LSP uses 0-based lines
                character: point.column.unwrap_or(1).saturating_sub(1),
            },
            end: Position {
                line: point.line.saturating_sub(1),
                character: point.column.unwrap_or(1).saturating_sub(1) + 10, // Approximate width
            },
        };

        let mut diagnostic = Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::WARNING),
            code: None,
            code_description: None,
            source: Some("jonesy".to_string()),
            message,
            related_information: None,
            tags: None,
            data: None,
        };

        // Add suggestion as related information if available
        if let Some(help) = suggestion {
            if !help.is_empty() {
                diagnostic.message = format!("{}\nhelp: {}", diagnostic.message, help);
            }
        }

        diagnostic
    }

    /// Analyze the workspace and publish diagnostics
    /// Returns true if analysis succeeded, false otherwise
    async fn analyze_and_publish(&self) -> bool {
        // Serialize analysis runs to prevent out-of-order diagnostics
        let _guard = self.analysis_lock.lock().await;

        let state = self.state.read().await;
        let Some(workspace_root) = &state.workspace_root else {
            self.client
                .log_message(MessageType::WARNING, "No workspace root set")
                .await;
            return false;
        };

        self.client
            .log_message(
                MessageType::INFO,
                format!("Analyzing workspace: {}", workspace_root.display()),
            )
            .await;

        // First, discover workspace structure
        let workspace_info = {
            let workspace_root = workspace_root.clone();
            tokio::task::spawn_blocking(move || discover_workspace(&workspace_root))
                .await
                .ok()
                .flatten()
        };

        if let Some(info) = &workspace_info {
            self.client
                .log_message(
                    MessageType::INFO,
                    format!("Workspace members: {}", info.members.join(", ")),
                )
                .await;
            self.client
                .log_message(
                    MessageType::INFO,
                    format!(
                        "Found {} targets: {}",
                        info.targets.len(),
                        info.targets.join(", ")
                    ),
                )
                .await;
        }

        // Get list of targets to analyze
        let targets = {
            let workspace_root = workspace_root.clone();
            tokio::task::spawn_blocking(move || find_workspace_binaries(&workspace_root))
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default()
        };

        if targets.is_empty() {
            self.client
                .log_message(MessageType::WARNING, "No targets found to analyze")
                .await;
            return false;
        }

        self.client
            .log_message(
                MessageType::INFO,
                format!("Starting analysis of {} targets...", targets.len()),
            )
            .await;

        // Get src_filter for filtering to workspace source files
        let src_filter = workspace_info
            .as_ref()
            .map(|info| info.src_filter.clone())
            .unwrap_or_else(|| {
                // Fallback: use workspace root name + /src/
                workspace_root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|name| format!("{}/src/", name))
                    .unwrap_or_else(|| "src/".to_string())
            });

        self.client
            .log_message(MessageType::LOG, format!("Source filter: {}", src_filter))
            .await;

        // Analyze each target and log progress
        let mut all_code_points: Vec<CrateCodePoint> = Vec::new();
        let mut seen: std::collections::HashSet<(String, u32, u32)> =
            std::collections::HashSet::new();

        for target in &targets {
            let target_name = target
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());

            let analysis_result = {
                let target = target.clone();
                let workspace_root = workspace_root.clone();
                let src_filter = src_filter.clone();
                tokio::task::spawn_blocking(move || {
                    analyze_single_target(&target, &workspace_root, &src_filter)
                })
                .await
            };

            match analysis_result {
                Ok(Ok(points)) => {
                    let new_points: Vec<_> = points
                        .into_iter()
                        .filter(|p| {
                            let key = (p.file.clone(), p.line, p.column.unwrap_or(0));
                            seen.insert(key)
                        })
                        .collect();
                    self.client
                        .log_message(
                            MessageType::INFO,
                            format!(
                                "  {} - found {} panic points",
                                target_name,
                                new_points.len()
                            ),
                        )
                        .await;
                    all_code_points.extend(new_points);
                }
                Ok(Err(e)) => {
                    self.client
                        .log_message(
                            MessageType::LOG,
                            format!("  {} - skipped: {}", target_name, e),
                        )
                        .await;
                }
                Err(_) => {
                    self.client
                        .log_message(
                            MessageType::WARNING,
                            format!("  {} - analysis failed", target_name),
                        )
                        .await;
                }
            }
        }

        self.client
            .log_message(
                MessageType::INFO,
                format!("Total: {} panic points", all_code_points.len()),
            )
            .await;

        let code_points = all_code_points;

        // Group code points by file, excluding build artifacts in target/
        let mut points_by_file: HashMap<Url, Vec<CrateCodePoint>> = HashMap::new();
        let target_dir = state
            .workspace_root
            .as_ref()
            .map(|r| r.join("target").to_string_lossy().to_string());

        for point in code_points {
            // Skip files in target/ directory (build artifacts, generated code)
            if let Some(ref target) = target_dir {
                if point.file.starts_with(target) {
                    continue;
                }
            }

            let raw_path = PathBuf::from(&point.file);
            let file_path = if raw_path.is_absolute() {
                raw_path
            } else if let Some(root) = &state.workspace_root {
                root.join(raw_path)
            } else {
                raw_path
            };

            if let Ok(uri) = Url::from_file_path(&file_path) {
                points_by_file.entry(uri).or_default().push(point);
            }
        }
        drop(state);

        // Update state and publish diagnostics
        let mut state = self.state.write().await;
        let old_files: std::collections::HashSet<_> = state.panic_points.keys().cloned().collect();
        state.panic_points = points_by_file.clone();
        drop(state);

        // Publish diagnostics for each file
        let new_files: std::collections::HashSet<_> = points_by_file.keys().cloned().collect();
        for (uri, points) in &points_by_file {
            let diagnostics: Vec<Diagnostic> =
                points.iter().map(Self::code_point_to_diagnostic).collect();

            // Log diagnostic details for debugging
            for (point, diag) in points.iter().zip(diagnostics.iter()) {
                self.client
                    .log_message(
                        MessageType::LOG,
                        format!(
                            "  Diag: {}:{} col:{:?} -> LSP line:{} '{}'",
                            uri.path().rsplit('/').next().unwrap_or("?"),
                            point.line,
                            point.column,
                            diag.range.start.line,
                            diag.message.lines().next().unwrap_or("")
                        ),
                    )
                    .await;
            }

            self.client
                .log_message(
                    MessageType::LOG,
                    format!(
                        "Publishing {} diagnostics to {}",
                        diagnostics.len(),
                        uri.path()
                    ),
                )
                .await;

            self.client
                .publish_diagnostics(uri.clone(), diagnostics, None)
                .await;
        }

        // Clear diagnostics for files that no longer have panic points
        for uri in old_files.difference(&new_files) {
            self.client
                .publish_diagnostics(uri.clone(), vec![], None)
                .await;
        }

        self.client
            .log_message(
                MessageType::INFO,
                format!(
                    "Jonesy analysis complete: published diagnostics for {} files",
                    new_files.len()
                ),
            )
            .await;
        true
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for JonesyLspServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Store workspace root - try root_uri first, then fallback to workspace_folders
        let workspace_path = if let Some(root_uri) = params.root_uri {
            root_uri.to_file_path().ok()
        } else if let Some(folders) = params.workspace_folders {
            folders.first().and_then(|f| f.uri.to_file_path().ok())
        } else {
            None
        };

        if let Some(path) = workspace_path {
            let mut state = self.state.write().await;
            state.workspace_root = Some(path);
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        will_save: None,
                        will_save_wait_until: None,
                        save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                    },
                )),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["jonesy.analyze".to_string()],
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "jonesy".to_string(),
                version: Some(crate::args::VERSION.to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Jonesy LSP server initialized")
            .await;

        // Run initial analysis
        self.analyze_and_publish().await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, _params: DidOpenTextDocumentParams) {
        // No-op: we analyze binaries, not source text
    }

    async fn did_change(&self, _params: DidChangeTextDocumentParams) {
        // No-op: we analyze binaries, not source text
    }

    async fn did_close(&self, _params: DidCloseTextDocumentParams) {
        // No-op: we don't track open documents
    }

    async fn did_save(&self, _params: DidSaveTextDocumentParams) {
        // Re-analyze on file save
        // Note: This triggers on any file save, which may be slow
        // A better approach would be to watch target/debug/ for binary changes
        self.analyze_and_publish().await;
    }

    async fn did_change_watched_files(&self, _params: DidChangeWatchedFilesParams) {
        // Re-analyze when watched files change (e.g., binaries in target/)
        self.analyze_and_publish().await;
    }

    async fn code_action(&self, _params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        // Provide a code action to manually trigger analysis
        let action = CodeAction {
            title: "Run Jonesy Panic Analysis".to_string(),
            kind: Some(CodeActionKind::SOURCE),
            diagnostics: None,
            edit: None,
            command: Some(Command {
                title: "Run Jonesy Panic Analysis".to_string(),
                command: "jonesy.analyze".to_string(),
                arguments: None,
            }),
            is_preferred: Some(false),
            disabled: None,
            data: None,
        };
        Ok(Some(vec![CodeActionOrCommand::CodeAction(action)]))
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        if params.command == "jonesy.analyze" {
            let success = self.analyze_and_publish().await;
            Ok(Some(serde_json::json!({"success": success})))
        } else {
            Ok(None)
        }
    }
}

/// Info about workspace structure (for logging before analysis)
struct WorkspaceInfo {
    members: Vec<String>,
    targets: Vec<String>,
    /// Filter path for source files like "flowc/src/|flowr/src/|..."
    src_filter: String,
}

/// Quickly discover workspace structure without running full analysis
fn discover_workspace(workspace_root: &Path) -> Option<WorkspaceInfo> {
    let cargo_toml = workspace_root.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml).ok()?;
    let manifest = cargo_toml::Manifest::from_slice(content.as_bytes()).ok()?;

    let mut members = Vec::new();
    let mut member_src_paths = Vec::new();
    let mut targets = Vec::new();

    let workspace_root_name = workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    // Get workspace members
    if let Some(workspace) = &manifest.workspace {
        for member in &workspace.members {
            if member.contains('*') {
                // Expand glob
                for path in expand_workspace_glob(workspace_root, member) {
                    if let Some(name) = path.file_name() {
                        let name_str = name.to_string_lossy().to_string();
                        member_src_paths.push(format!("{}/src/", name_str));
                        members.push(name_str);
                    }
                }
            } else {
                // Use directory basename for src path
                let path = std::path::Path::new(member);
                if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                    member_src_paths.push(format!("{}/src/", dir_name));
                }
                members.push(member.clone());
            }
        }
    } else if let Some(pkg) = &manifest.package {
        // Single crate, not a workspace
        if workspace_root_name.is_empty() {
            member_src_paths.push("src/".to_string());
        } else {
            member_src_paths.push(format!("{}/src/", workspace_root_name));
        }
        members.push(pkg.name.clone());
    }

    // Get targets
    if let Ok(found_targets) = find_workspace_binaries(workspace_root) {
        for target in found_targets {
            if let Some(name) = target.file_name() {
                targets.push(name.to_string_lossy().to_string());
            }
        }
    }

    // Build filter path like "flowc/src/|flowr/src/|..."
    let src_filter = member_src_paths.join("|");

    Some(WorkspaceInfo {
        members,
        targets,
        src_filter,
    })
}

/// Analyze a single target (binary or library) and return panic points
fn analyze_single_target(
    target_path: &Path,
    workspace_root: &Path,
    src_filter: &str,
) -> std::result::Result<Vec<CrateCodePoint>, String> {
    use crate::call_tree::{
        CallTreeNode, build_call_tree_parallel, collect_crate_code_points, prune_call_tree,
    };
    use crate::cargo::derive_crate_src_path;
    use crate::config::Config;
    use crate::sym::{
        CallGraph, DebugInfo, SymbolTable, find_symbol_address, find_symbol_containing,
        load_debug_info, read_symbols,
    };
    use dashmap::DashSet;
    use goblin::mach::Mach::Binary;
    use std::sync::Arc;

    let binary_buffer =
        std::fs::read(target_path).map_err(|e| format!("Failed to read target: {}", e))?;

    let symbols =
        read_symbols(&binary_buffer).map_err(|e| format!("Failed to read symbols: {}", e))?;

    let SymbolTable::MachO(Binary(macho)) = symbols else {
        return Err("Not a Mach-O binary (may be archive/rlib)".to_string());
    };

    let config =
        Config::load_for_project(workspace_root, None).unwrap_or_else(|_| Config::with_defaults());

    // derive_crate_src_path used for debug info loading
    let crate_src_path = derive_crate_src_path(target_path);

    // Find panic symbol
    const PANIC_PATTERNS: &[&str] = &["rust_panic$", "panic_fmt$", "panic_display"];
    let mut target_addr = 0u64;
    let mut demangled = String::new();

    for pattern in PANIC_PATTERNS {
        if let Ok(Some((sym, dem))) = find_symbol_containing(&macho, pattern) {
            if let Some((_, addr)) = find_symbol_address(&macho, &sym) {
                target_addr = addr;
                demangled = dem;
                break;
            }
        }
    }

    if target_addr == 0 {
        return Err("No panic symbol found".to_string());
    }

    // Load debug info and build call graph
    let debug_info = load_debug_info(&macho, target_path, true);
    let call_graph = match &debug_info {
        DebugInfo::Embedded => CallGraph::build_with_debug_info(
            &macho,
            &binary_buffer,
            &macho,
            &binary_buffer,
            crate_src_path.as_deref(),
            false,
        )
        .unwrap_or_else(|_| {
            CallGraph::build(&macho, &binary_buffer).unwrap_or_else(|_| CallGraph::empty())
        }),
        DebugInfo::DSym(dsym_info) => dsym_info.with_debug_macho(|debug_macho| {
            if let Binary(debug_mach) = debug_macho {
                CallGraph::build_with_debug_info(
                    &macho,
                    &binary_buffer,
                    debug_mach,
                    dsym_info.borrow_debug_buffer(),
                    crate_src_path.as_deref(),
                    false,
                )
                .unwrap_or_else(|_| {
                    CallGraph::build(&macho, &binary_buffer).unwrap_or_else(|_| CallGraph::empty())
                })
            } else {
                CallGraph::build(&macho, &binary_buffer).unwrap_or_else(|_| CallGraph::empty())
            }
        }),
        _ => CallGraph::build(&macho, &binary_buffer).unwrap_or_else(|_| CallGraph::empty()),
    };

    // Build call tree
    let mut root = CallTreeNode::new_root(demangled);
    let visited = Arc::new(DashSet::new());
    visited.insert(target_addr);
    root.callers = build_call_tree_parallel(&call_graph, target_addr, &visited);

    // Prune and collect code points using src_filter to capture all workspace crates
    // The src_filter contains relative paths like "flowc/src/|flowr/src/|..." which match
    // the relative paths in debug info
    prune_call_tree(&mut root, src_filter);
    let (code_points, _) = collect_crate_code_points(&root, src_filter, &config);

    Ok(code_points)
}

/// Find binary and library files in the workspace
fn find_workspace_binaries(workspace_root: &Path) -> std::result::Result<Vec<PathBuf>, String> {
    let target_debug = workspace_root.join("target/debug");
    if !target_debug.exists() {
        return Ok(Vec::new());
    }

    // Look for Cargo.toml to find binary names
    let cargo_toml = workspace_root.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&cargo_toml)
        .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;

    let manifest = cargo_toml::Manifest::from_slice(content.as_bytes())
        .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

    let mut targets = Vec::new();

    // Helper to find binary with optional .exe extension on Windows
    let find_binary = |dir: &Path, name: &str| -> Option<PathBuf> {
        let path = dir.join(name);
        if path.exists() {
            return Some(path);
        }
        #[cfg(windows)]
        {
            let exe_path = path.with_extension("exe");
            if exe_path.exists() {
                return Some(exe_path);
            }
        }
        None
    };

    // Helper to find library (.dylib on macOS, .so on Linux, .dll on Windows)
    let find_library = |dir: &Path, name: &str| -> Option<PathBuf> {
        // Convert crate name to lib name (replace - with _)
        let lib_name = name.replace('-', "_");

        // Try platform-specific extensions
        #[cfg(target_os = "macos")]
        {
            let dylib = dir.join(format!("lib{}.dylib", lib_name));
            if dylib.exists() {
                return Some(dylib);
            }
        }
        #[cfg(target_os = "linux")]
        {
            let so = dir.join(format!("lib{}.so", lib_name));
            if so.exists() {
                return Some(so);
            }
        }
        #[cfg(target_os = "windows")]
        {
            let dll = dir.join(format!("{}.dll", lib_name));
            if dll.exists() {
                return Some(dll);
            }
        }
        // Also try .rlib (Rust static library)
        let rlib = dir.join(format!("lib{}.rlib", lib_name));
        if rlib.exists() {
            return Some(rlib);
        }
        None
    };

    // Check for package binaries and libraries
    if let Some(pkg) = &manifest.package {
        let pkg_name = &pkg.name;

        // Default binary
        if let Some(default_bin) = find_binary(&target_debug, pkg_name) {
            targets.push(default_bin);
        }

        // Explicit [[bin]] targets
        for bin in &manifest.bin {
            let bin_name = bin.name.as_deref().unwrap_or(pkg_name);
            if let Some(bin_path) = find_binary(&target_debug, bin_name) {
                if !targets.contains(&bin_path) {
                    targets.push(bin_path);
                }
            }
        }

        // Library target
        if let Some(lib_path) = find_library(&target_debug, pkg_name) {
            if !targets.contains(&lib_path) {
                targets.push(lib_path);
            }
        }
    }

    // Check for workspace members
    if let Some(workspace) = &manifest.workspace {
        for member in &workspace.members {
            let member_paths: Vec<PathBuf> = if member.contains('*') {
                // Expand glob patterns like "crates/*"
                expand_workspace_glob(workspace_root, member)
            } else {
                vec![workspace_root.join(member)]
            };

            for member_path in member_paths {
                let member_cargo = member_path.join("Cargo.toml");
                if let Ok(content) = std::fs::read_to_string(&member_cargo) {
                    if let Ok(member_manifest) =
                        cargo_toml::Manifest::from_slice(content.as_bytes())
                    {
                        if let Some(pkg) = &member_manifest.package {
                            // Default binary
                            if let Some(default_bin) = find_binary(&target_debug, &pkg.name) {
                                if !targets.contains(&default_bin) {
                                    targets.push(default_bin);
                                }
                            }

                            // Explicit [[bin]] targets
                            for bin in &member_manifest.bin {
                                let bin_name = bin.name.as_deref().unwrap_or(&pkg.name);
                                if let Some(bin_path) = find_binary(&target_debug, bin_name) {
                                    if !targets.contains(&bin_path) {
                                        targets.push(bin_path);
                                    }
                                }
                            }

                            // Library target
                            if let Some(lib_path) = find_library(&target_debug, &pkg.name) {
                                if !targets.contains(&lib_path) {
                                    targets.push(lib_path);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(targets)
}

/// Expand a workspace glob pattern like "crates/*" or "crates/**" to actual paths
fn expand_workspace_glob(workspace_root: &Path, pattern: &str) -> Vec<PathBuf> {
    // Build full glob pattern rooted at workspace
    let full_pattern = workspace_root.join(pattern);
    let pattern_str = full_pattern.to_string_lossy();

    // Use glob to expand the pattern
    match glob::glob(&pattern_str) {
        Ok(paths) => paths
            .filter_map(|p| p.ok())
            .filter(|p| p.is_dir() && p.join("Cargo.toml").exists())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Run the LSP server
pub async fn run_lsp_server() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(JonesyLspServer::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
