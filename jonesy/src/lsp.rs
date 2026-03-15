//! LSP server for jonesy panic point diagnostics.
//!
//! This module implements a Language Server Protocol server that publishes
//! panic point diagnostics to IDEs and code editors. It runs alongside
//! rust-analyzer, publishing its own diagnostics.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
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
}

impl JonesyLspServer {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            state: Arc::new(RwLock::new(ServerState::new())),
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
    async fn analyze_and_publish(&self) {
        let state = self.state.read().await;
        let Some(workspace_root) = &state.workspace_root else {
            return;
        };

        // Run jonesy analysis on the workspace
        let analysis_result = tokio::task::spawn_blocking({
            let workspace_root = workspace_root.clone();
            move || run_analysis(&workspace_root)
        })
        .await;

        let Ok(Ok(code_points)) = analysis_result else {
            self.client
                .log_message(MessageType::ERROR, "Failed to run jonesy analysis")
                .await;
            return;
        };

        // Group code points by file
        let mut points_by_file: HashMap<Url, Vec<CrateCodePoint>> = HashMap::new();
        for point in code_points {
            let file_path = if point.file.starts_with('/') {
                PathBuf::from(&point.file)
            } else if let Some(root) = &state.workspace_root {
                root.join(&point.file)
            } else {
                PathBuf::from(&point.file)
            };

            if let Ok(uri) = Url::from_file_path(&file_path) {
                points_by_file.entry(uri).or_default().push(point);
            }
        }
        drop(state);

        // Update state and publish diagnostics
        let mut state = self.state.write().await;
        state.panic_points = points_by_file.clone();
        drop(state);

        // Publish diagnostics for each file
        for (uri, points) in points_by_file {
            let diagnostics: Vec<Diagnostic> =
                points.iter().map(Self::code_point_to_diagnostic).collect();

            self.client
                .publish_diagnostics(uri, diagnostics, None)
                .await;
        }

        self.client
            .log_message(MessageType::INFO, "Jonesy analysis complete")
            .await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for JonesyLspServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Store workspace root
        if let Some(root_uri) = params.root_uri {
            if let Ok(path) = root_uri.to_file_path() {
                let mut state = self.state.write().await;
                state.workspace_root = Some(path);
            }
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["jonesy.analyze".to_string()],
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
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

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        if params.command == "jonesy.analyze" {
            self.analyze_and_publish().await;
            Ok(Some(serde_json::json!({"success": true})))
        } else {
            Ok(None)
        }
    }
}

/// Run jonesy analysis on the given workspace root
fn run_analysis(workspace_root: &Path) -> std::result::Result<Vec<CrateCodePoint>, String> {
    use crate::args::OutputFormat;
    use crate::call_tree::{
        CallTreeNode, build_call_tree_parallel, collect_crate_code_points, prune_call_tree,
    };
    use crate::cargo::{derive_crate_src_path, find_project_root};
    use crate::config::Config;
    use crate::sym::{
        CallGraph, DebugInfo, SymbolTable, find_symbol_address, find_symbol_containing,
        load_debug_info, read_symbols,
    };
    use dashmap::DashSet;
    use goblin::mach::Mach::Binary;
    use std::sync::Arc;

    // Find binaries to analyze
    let binaries = find_workspace_binaries(workspace_root)?;
    if binaries.is_empty() {
        return Ok(Vec::new());
    }

    let mut all_code_points: Vec<CrateCodePoint> = Vec::new();
    let mut seen: std::collections::HashSet<(String, u32)> = std::collections::HashSet::new();

    let config =
        Config::load_for_project(workspace_root, None).unwrap_or_else(|_| Config::with_defaults());
    let _output = OutputFormat::text(false, false, true, false); // Quiet mode (for future use)

    for binary_path in binaries {
        let Ok(binary_buffer) = std::fs::read(&binary_path) else {
            continue;
        };

        let Ok(symbols) = read_symbols(&binary_buffer) else {
            continue;
        };

        let SymbolTable::MachO(Binary(macho)) = symbols else {
            continue;
        };

        let crate_src_path = derive_crate_src_path(&binary_path);
        let _project_root = find_project_root(&binary_path);

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
            continue;
        }

        // Load debug info and build call graph
        let debug_info = load_debug_info(&macho, &binary_path, true);
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
                        CallGraph::build(&macho, &binary_buffer)
                            .unwrap_or_else(|_| CallGraph::empty())
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

        // Prune and collect code points
        if let Some(crate_path) = crate_src_path.as_deref() {
            prune_call_tree(&mut root, crate_path);
            let (code_points, _) = collect_crate_code_points(&root, crate_path, &config);

            for point in code_points {
                let key = (point.file.clone(), point.line);
                if seen.insert(key) {
                    all_code_points.push(point);
                }
            }
        }
    }

    Ok(all_code_points)
}

/// Find binary files in the workspace
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

    let mut binaries = Vec::new();

    // Check for package binaries
    if let Some(pkg) = &manifest.package {
        let pkg_name = &pkg.name;

        // Default binary
        let default_bin = target_debug.join(pkg_name);
        if default_bin.exists() {
            binaries.push(default_bin);
        }

        // Explicit [[bin]] targets
        for bin in &manifest.bin {
            let bin_name = bin.name.as_deref().unwrap_or(pkg_name);
            let bin_path = target_debug.join(bin_name);
            if bin_path.exists() && !binaries.contains(&bin_path) {
                binaries.push(bin_path);
            }
        }
    }

    // Check for workspace members
    if let Some(workspace) = &manifest.workspace {
        for member in &workspace.members {
            if member.contains('*') {
                continue; // Skip glob patterns for simplicity
            }
            let member_path = workspace_root.join(member);
            let member_cargo = member_path.join("Cargo.toml");
            if let Ok(content) = std::fs::read_to_string(&member_cargo) {
                if let Ok(member_manifest) = cargo_toml::Manifest::from_slice(content.as_bytes()) {
                    if let Some(pkg) = &member_manifest.package {
                        let bin_path = target_debug.join(&pkg.name);
                        if bin_path.exists() {
                            binaries.push(bin_path);
                        }
                    }
                }
            }
        }
    }

    Ok(binaries)
}

/// Run the LSP server
pub async fn run_lsp_server() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(JonesyLspServer::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
