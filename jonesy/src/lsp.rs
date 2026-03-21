//! LSP server for jonesy panic point diagnostics.
//!
//! This module implements a Language Server Protocol server that publishes
//! panic point diagnostics to IDEs and code editors. It runs alongside
//! rust-analyzer, publishing its own diagnostics.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::{Mutex, RwLock};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::notification::Progress;
use tower_lsp::lsp_types::request::WorkDoneProgressCreate;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::call_tree::CrateCodePoint;
use crate::cargo::{find_binary, find_library};

/// Counter for generating unique progress tokens
static PROGRESS_TOKEN_COUNTER: AtomicU32 = AtomicU32::new(0);

/// State shared across the LSP server
struct ServerState {
    /// Workspace root path
    workspace_root: Option<PathBuf>,
    /// Cached panic points by file URI
    panic_points: HashMap<Url, Vec<CrateCodePoint>>,
    /// Files that have been opened (for re-publishing after analysis)
    opened_files: HashSet<Url>,
}

impl ServerState {
    fn new() -> Self {
        Self {
            workspace_root: None,
            panic_points: HashMap::new(),
            opened_files: HashSet::new(),
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
        // Get all causes sorted by error code for determinism
        let sorted_causes: Vec<_> = {
            let mut causes: Vec<_> = point.causes.iter().collect();
            causes.sort_by_key(|c| c.error_code());
            causes
        };

        // Build message showing all causes
        let (message, suggestion, error_code, docs_url) = if sorted_causes.is_empty() {
            ("potential panic point".to_string(), None, None, None)
        } else if sorted_causes.len() == 1 {
            let cause = sorted_causes[0];
            (
                format!("panic point: {}", cause.description()),
                Some(
                    cause
                        .format_suggestion(point.is_direct_panic, point.called_function.as_deref()),
                ),
                Some(cause.error_code().to_string()),
                Url::parse(&cause.docs_url()).ok(),
            )
        } else {
            // Multiple causes - show all in message
            let descriptions: Vec<_> = sorted_causes
                .iter()
                .map(|c| format!("{}: {}", c.error_code(), c.description()))
                .collect();
            let primary = sorted_causes[0];
            (
                format!("panic point: {}", descriptions.join(", ")),
                Some(
                    primary
                        .format_suggestion(point.is_direct_panic, point.called_function.as_deref()),
                ),
                // Show first error code (most specific/important)
                Some(primary.error_code().to_string()),
                Url::parse(&primary.docs_url()).ok(),
            )
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

        // Store cause info in data field for use by code actions
        // Use sorted_causes for consistency with the displayed message
        let cause_ids: Vec<String> = sorted_causes.iter().map(|c| c.id().to_string()).collect();
        let data = serde_json::json!({
            "causes": cause_ids,
            "function": &point.name,
            "file": &point.file,
        });

        // Create code_description with documentation URL if available
        let code_description = docs_url.map(|href| CodeDescription { href });

        let mut diagnostic = Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::WARNING),
            code: error_code.map(NumberOrString::String),
            code_description,
            source: Some("jonesy".to_string()),
            message,
            related_information: None,
            tags: None,
            data: Some(data),
        };

        // Add suggestion as related information if available
        if let Some(help) = suggestion {
            if !help.is_empty() {
                diagnostic.message = format!("{}\nhelp: {}", diagnostic.message, help);
            }
        }

        diagnostic
    }

    /// Create a progress token and request the client to show progress UI.
    /// Returns the token if successful, None if the client doesn't support it.
    async fn create_progress(&self) -> Option<ProgressToken> {
        let token_id = PROGRESS_TOKEN_COUNTER.fetch_add(1, Ordering::SeqCst);
        let token = ProgressToken::Number(token_id as i32);

        // Request the client to create a progress indicator
        match self
            .client
            .send_request::<WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
                token: token.clone(),
            })
            .await
        {
            Ok(()) => Some(token),
            Err(e) => {
                // Client may not support progress - log and continue without it
                self.client
                    .log_message(MessageType::LOG, format!("Progress not supported: {}", e))
                    .await;
                None
            }
        }
    }

    /// Send a progress begin notification
    async fn progress_begin(&self, token: &ProgressToken, title: &str, message: Option<&str>) {
        self.client
            .send_notification::<Progress>(ProgressParams {
                token: token.clone(),
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                    WorkDoneProgressBegin {
                        title: title.to_string(),
                        cancellable: Some(false),
                        message: message.map(String::from),
                        percentage: Some(0),
                    },
                )),
            })
            .await;
    }

    /// Send a progress report notification
    async fn progress_report(&self, token: &ProgressToken, message: &str, percentage: u32) {
        self.client
            .send_notification::<Progress>(ProgressParams {
                token: token.clone(),
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::Report(
                    WorkDoneProgressReport {
                        cancellable: Some(false),
                        message: Some(message.to_string()),
                        percentage: Some(percentage),
                    },
                )),
            })
            .await;
    }

    /// Send a progress end notification
    async fn progress_end(&self, token: &ProgressToken, message: &str) {
        self.client
            .send_notification::<Progress>(ProgressParams {
                token: token.clone(),
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(WorkDoneProgressEnd {
                    message: Some(message.to_string()),
                })),
            })
            .await;
    }

    /// Register file watchers for binaries and config files.
    /// Watches target/debug/ for binary changes and config files (jonesy.toml, Cargo.toml).
    async fn register_file_watchers(&self) {
        // Clone workspace_root and release lock before async operations
        let workspace_root = {
            let state = self.state.read().await;
            state.workspace_root.clone()
        };
        let Some(workspace_root) = workspace_root else {
            return;
        };

        // Build glob patterns for target/debug/ binaries
        let target_debug = workspace_root.join("target/debug");
        let target_debug_str = target_debug.to_string_lossy();

        // Watch for all files in target/debug/ (binaries, rlibs, dylibs)
        let mut watchers = vec![
            FileSystemWatcher {
                glob_pattern: GlobPattern::String(format!("{}/*", target_debug_str)),
                kind: Some(WatchKind::Create | WatchKind::Change),
            },
            // Also watch for dSYM bundles on macOS
            FileSystemWatcher {
                glob_pattern: GlobPattern::String(format!("{}/*.dSYM/**", target_debug_str)),
                kind: Some(WatchKind::Create | WatchKind::Change),
            },
        ];

        // Watch config files (jonesy.toml and Cargo.toml files)
        let config_files = find_config_files(&workspace_root);
        for config_path in &config_files {
            watchers.push(FileSystemWatcher {
                glob_pattern: GlobPattern::String(config_path.to_string_lossy().to_string()),
                kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
            });
        }

        let registration_options = DidChangeWatchedFilesRegistrationOptions { watchers };

        let registration = Registration {
            id: "jonesy-file-watcher".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: Some(serde_json::to_value(registration_options).unwrap()),
        };

        match self.client.register_capability(vec![registration]).await {
            Ok(()) => {
                self.client
                    .log_message(
                        MessageType::INFO,
                        format!(
                            "Watching {} for binary changes and {} config file(s)",
                            target_debug_str,
                            config_files.len()
                        ),
                    )
                    .await;
            }
            Err(e) => {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("Failed to register file watchers: {}", e),
                    )
                    .await;
            }
        }
    }

    /// Analyze the workspace and publish diagnostics
    /// Returns true if analysis succeeded, false otherwise
    async fn analyze_and_publish(&self) -> bool {
        // Serialize analysis runs to prevent out-of-order diagnostics
        let _guard = self.analysis_lock.lock().await;

        // Extract workspace root early and release state lock
        let (workspace_root, target_dir) = {
            let state = self.state.read().await;
            let Some(root) = state.workspace_root.clone() else {
                self.client
                    .log_message(MessageType::WARNING, "No workspace root set")
                    .await;
                return false;
            };
            let target_dir = root.join("target").to_string_lossy().to_string();
            (root, target_dir)
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

        // Create progress indicator for IDE status bar
        let progress_token = self.create_progress().await;
        if let Some(ref token) = progress_token {
            self.progress_begin(token, "Panic Analysis", Some("Analyzing targets..."))
                .await;
        }

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

        // Track all diagnostics by file URI (accumulates across targets)
        let mut points_by_file: HashMap<Url, Vec<CrateCodePoint>> = HashMap::new();
        let mut seen: std::collections::HashSet<(String, u32, u32)> =
            std::collections::HashSet::new();
        let mut total_points = 0usize;

        // Analyze each target and publish diagnostics incrementally
        let total_targets = targets.len();
        for (target_idx, target) in targets.iter().enumerate() {
            let target_name = target
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());

            // Update progress indicator
            if let Some(ref token) = progress_token {
                let percentage = (((target_idx + 1) * 100) / total_targets) as u32;
                self.progress_report(
                    token,
                    &format!(
                        "Analyzing {} ({}/{})",
                        target_name,
                        target_idx + 1,
                        total_targets
                    ),
                    percentage,
                )
                .await;
            }

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
                    // Filter to new points only (dedup across targets)
                    let new_points: Vec<_> = points
                        .into_iter()
                        .filter(|p| {
                            let key = (p.file.clone(), p.line, p.column.unwrap_or(0));
                            seen.insert(key)
                        })
                        .collect();

                    let point_count = new_points.len();
                    total_points += point_count;

                    self.client
                        .log_message(
                            MessageType::INFO,
                            format!("  {} - {} new panic points", target_name, point_count),
                        )
                        .await;

                    // Group new points by file and publish incrementally
                    let mut files_updated: std::collections::HashSet<Url> =
                        std::collections::HashSet::new();

                    for point in new_points {
                        // Skip files in target/ directory
                        if point.file.starts_with(&target_dir) {
                            continue;
                        }

                        let raw_path = PathBuf::from(&point.file);
                        let file_path = if raw_path.is_absolute() {
                            raw_path
                        } else {
                            workspace_root.join(raw_path)
                        };

                        if let Ok(uri) = Url::from_file_path(&file_path) {
                            files_updated.insert(uri.clone());
                            points_by_file.entry(uri).or_default().push(point);
                        }
                    }

                    // Publish updated diagnostics for files that changed
                    for uri in files_updated {
                        if let Some(points) = points_by_file.get(&uri) {
                            let diagnostics: Vec<Diagnostic> =
                                points.iter().map(Self::code_point_to_diagnostic).collect();

                            self.client
                                .publish_diagnostics(uri.clone(), diagnostics, None)
                                .await;
                        }
                    }
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

        // Update state with final results
        let mut state = self.state.write().await;
        let old_files: std::collections::HashSet<_> = state.panic_points.keys().cloned().collect();
        let new_files: std::collections::HashSet<_> = points_by_file.keys().cloned().collect();
        state.panic_points = points_by_file;
        drop(state);

        // Clear diagnostics for files that no longer have panic points
        for uri in old_files.difference(&new_files) {
            self.client
                .publish_diagnostics(uri.clone(), vec![], None)
                .await;
        }

        // Complete progress indicator
        if let Some(ref token) = progress_token {
            self.progress_end(
                token,
                &format!(
                    "Found {} panic points in {} files",
                    total_points,
                    new_files.len()
                ),
            )
            .await;
        }

        self.client
            .log_message(
                MessageType::INFO,
                format!(
                    "Analysis complete: {} panic points in {} files",
                    total_points,
                    new_files.len()
                ),
            )
            .await;

        // Re-publish diagnostics to files that were opened before/during analysis
        // Snapshot all data while holding the lock once, then publish outside the lock
        let republish: Vec<(Url, Vec<Diagnostic>)> = {
            let state = self.state.read().await;
            state
                .opened_files
                .iter()
                .filter_map(|uri| {
                    state.panic_points.get(uri).map(|points| {
                        let diagnostics =
                            points.iter().map(Self::code_point_to_diagnostic).collect();
                        (uri.clone(), diagnostics)
                    })
                })
                .collect()
        };

        for (uri, diagnostics) in republish {
            if !diagnostics.is_empty() {
                self.client
                    .publish_diagnostics(uri, diagnostics, None)
                    .await;
            }
        }
        true
    }

    /// Create a code action that inserts an inline allow comment at end of line.
    fn create_inline_allow_action(
        uri: &Url,
        range: Range,
        cause: &str,
        diagnostic: &Diagnostic,
    ) -> Option<CodeAction> {
        let title = if cause == "*" {
            "Allow all panics on this line".to_string()
        } else {
            format!("Allow '{}' on this line", cause)
        };

        // Insert comment at end of the line (using a large character value)
        // LSP clients will clamp this to the actual line length
        let edit = TextEdit {
            range: Range {
                start: Position {
                    line: range.start.line,
                    character: 10000, // Will be clamped to line end
                },
                end: Position {
                    line: range.start.line,
                    character: 10000,
                },
            },
            new_text: format!(" // jonesy:allow({})", cause),
        };

        let mut changes = HashMap::new();
        changes.insert(uri.clone(), vec![edit]);

        Some(CodeAction {
            title,
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(cause != "*"), // Prefer specific cause over wildcard
            disabled: None,
            data: None,
        })
    }

    /// Create a code action that adds a file-scoped rule to jonesy.toml.
    fn create_file_allow_action(
        uri: &Url,
        cause: &str,
        workspace_root: &Path,
        diagnostic: &Diagnostic,
    ) -> Option<CodeAction> {
        // Extract filename from URI
        let filename = uri.path().rsplit('/').next()?;

        let title = format!("Allow '{}' in {}", cause, filename);
        let rule_text = format!(
            "\n[[rules]]\npath = \"**/{}\"\nallow = [\"{}\"]\n",
            filename, cause
        );

        let jonesy_toml_path = workspace_root.join("jonesy.toml");
        let jonesy_toml_uri = Url::from_file_path(&jonesy_toml_path).ok()?;

        // Check if jonesy.toml exists and get its length
        let (file_exists, file_length) = if jonesy_toml_path.exists() {
            let content = std::fs::read_to_string(&jonesy_toml_path).unwrap_or_default();
            let lines = content.lines().count() as u32;
            (true, lines)
        } else {
            (false, 0)
        };

        let mut document_changes = Vec::new();

        // If file doesn't exist, create it first
        if !file_exists {
            document_changes.push(DocumentChangeOperation::Op(ResourceOp::Create(
                CreateFile {
                    uri: jonesy_toml_uri.clone(),
                    options: Some(CreateFileOptions {
                        overwrite: Some(false),
                        ignore_if_exists: Some(true),
                    }),
                    annotation_id: None,
                },
            )));
        }

        // Add the rule to the file
        let edit = TextEdit {
            range: Range {
                start: Position {
                    line: file_length,
                    character: 0,
                },
                end: Position {
                    line: file_length,
                    character: 0,
                },
            },
            new_text: rule_text,
        };

        document_changes.push(DocumentChangeOperation::Edit(TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: jonesy_toml_uri,
                version: None,
            },
            edits: vec![OneOf::Left(edit)],
        }));

        Some(CodeAction {
            title,
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(WorkspaceEdit {
                changes: None,
                document_changes: Some(DocumentChanges::Operations(document_changes)),
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(false),
            disabled: None,
            data: None,
        })
    }

    /// Create a code action that adds a function-scoped rule to jonesy.toml.
    fn create_function_allow_action(
        function: &str,
        cause: &str,
        workspace_root: &Path,
        diagnostic: &Diagnostic,
    ) -> Option<CodeAction> {
        // Extract just the function name (last segment)
        let func_name = function.rsplit("::").next()?;

        let title = format!("Allow '{}' in all functions named '{}'", cause, func_name);
        let rule_text = format!(
            "\n[[rules]]\nfunction = \"*::{}\"\nallow = [\"{}\"]\n",
            func_name, cause
        );

        let jonesy_toml_path = workspace_root.join("jonesy.toml");
        let jonesy_toml_uri = Url::from_file_path(&jonesy_toml_path).ok()?;

        // Check if jonesy.toml exists and get its length
        let (file_exists, file_length) = if jonesy_toml_path.exists() {
            let content = std::fs::read_to_string(&jonesy_toml_path).unwrap_or_default();
            let lines = content.lines().count() as u32;
            (true, lines)
        } else {
            (false, 0)
        };

        let mut document_changes = Vec::new();

        // If file doesn't exist, create it first
        if !file_exists {
            document_changes.push(DocumentChangeOperation::Op(ResourceOp::Create(
                CreateFile {
                    uri: jonesy_toml_uri.clone(),
                    options: Some(CreateFileOptions {
                        overwrite: Some(false),
                        ignore_if_exists: Some(true),
                    }),
                    annotation_id: None,
                },
            )));
        }

        // Add the rule to the file
        let edit = TextEdit {
            range: Range {
                start: Position {
                    line: file_length,
                    character: 0,
                },
                end: Position {
                    line: file_length,
                    character: 0,
                },
            },
            new_text: rule_text,
        };

        document_changes.push(DocumentChangeOperation::Edit(TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: jonesy_toml_uri,
                version: None,
            },
            edits: vec![OneOf::Left(edit)],
        }));

        Some(CodeAction {
            title,
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(WorkspaceEdit {
                changes: None,
                document_changes: Some(DocumentChanges::Operations(document_changes)),
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(false),
            disabled: None,
            data: None,
        })
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

        // Register file watchers for binaries and config files
        // Re-analyze when binaries change or config is modified
        self.register_file_watchers().await;

        // Run initial analysis
        self.analyze_and_publish().await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;

        // Track opened file for re-publishing diagnostics
        self.state.write().await.opened_files.insert(uri.clone());

        // If we have cached diagnostics for this file, publish them now
        let state = self.state.read().await;
        if let Some(points) = state.panic_points.get(&uri) {
            let diagnostics: Vec<Diagnostic> =
                points.iter().map(Self::code_point_to_diagnostic).collect();
            drop(state); // Release lock before async call

            if !diagnostics.is_empty() {
                self.client
                    .publish_diagnostics(uri, diagnostics, None)
                    .await;
            }
        }
    }

    async fn did_change(&self, _params: DidChangeTextDocumentParams) {
        // No-op: we analyze binaries, not source text
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        // Remove from opened files tracking
        self.state
            .write()
            .await
            .opened_files
            .remove(&params.text_document.uri);
    }

    async fn did_save(&self, _params: DidSaveTextDocumentParams) {
        // No-op: we watch target/debug/ for binary changes instead of
        // re-analyzing on every file save. This avoids redundant analysis
        // when the user saves files without building.
        //
        // Analysis is triggered by:
        // 1. did_change_watched_files (when binaries or config files change)
        // 2. Manual "jonesy.analyze" command
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        // Re-analyze when watched files change (binaries or config files)
        let changed_paths: Vec<_> = params
            .changes
            .iter()
            .filter_map(|c| c.uri.to_file_path().ok())
            .collect();

        if changed_paths.is_empty() {
            return;
        }

        // Categorize changes for logging
        let config_changes: Vec<_> = changed_paths
            .iter()
            .filter(|p| {
                p.file_name()
                    .map(|n| n == "jonesy.toml" || n == "Cargo.toml")
                    .unwrap_or(false)
            })
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();

        let binary_changes: Vec<_> = changed_paths
            .iter()
            .filter(|p| {
                p.file_name()
                    .map(|n| n != "jonesy.toml" && n != "Cargo.toml")
                    .unwrap_or(true)
            })
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();

        // Log what changed
        if !config_changes.is_empty() {
            self.client
                .log_message(
                    MessageType::INFO,
                    format!("Config changes detected: {}", config_changes.join(", ")),
                )
                .await;

            // Re-register watchers in case workspace membership changed
            // (e.g., Cargo.toml added/removed workspace members)
            self.register_file_watchers().await;
        }
        if !binary_changes.is_empty() {
            self.client
                .log_message(
                    MessageType::INFO,
                    format!("Binary changes detected: {}", binary_changes.join(", ")),
                )
                .await;
        }

        self.analyze_and_publish().await;
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let mut actions: Vec<CodeActionOrCommand> = Vec::new();

        // Get workspace root for jonesy.toml path
        let workspace_root = {
            let state = self.state.read().await;
            state.workspace_root.clone()
        };

        // Filter to jonesy diagnostics only
        let jonesy_diagnostics: Vec<_> = params
            .context
            .diagnostics
            .iter()
            .filter(|d| d.source.as_deref() == Some("jonesy"))
            .collect();

        for diag in jonesy_diagnostics {
            // Extract cause info from diagnostic data
            if let Some(data) = &diag.data {
                let causes: Vec<String> = data
                    .get("causes")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let function: String = data
                    .get("function")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                // Track which causes we've added actions for (avoid duplicates)
                let mut seen_causes = std::collections::HashSet::new();

                for cause in &causes {
                    if !seen_causes.insert(cause.clone()) {
                        continue;
                    }

                    // Action 1: Allow on this line (inline comment)
                    if let Some(action) = Self::create_inline_allow_action(
                        &params.text_document.uri,
                        diag.range,
                        cause,
                        diag,
                    ) {
                        actions.push(CodeActionOrCommand::CodeAction(action));
                    }

                    // Action 2: Allow in this file (scoped rule in jonesy.toml)
                    if let Some(ref root) = workspace_root {
                        if let Some(action) = Self::create_file_allow_action(
                            &params.text_document.uri,
                            cause,
                            root,
                            diag,
                        ) {
                            actions.push(CodeActionOrCommand::CodeAction(action));
                        }

                        // Action 3: Allow in this function (scoped rule in jonesy.toml)
                        if !function.is_empty() {
                            if let Some(action) =
                                Self::create_function_allow_action(&function, cause, root, diag)
                            {
                                actions.push(CodeActionOrCommand::CodeAction(action));
                            }
                        }
                    }
                }

                // Action 4: Allow all panics on this line (wildcard)
                if causes.len() > 1 {
                    if let Some(action) = Self::create_inline_allow_action(
                        &params.text_document.uri,
                        diag.range,
                        "*",
                        diag,
                    ) {
                        actions.push(CodeActionOrCommand::CodeAction(action));
                    }
                }
            }
        }

        // Always add the manual analyze action
        let analyze_action = CodeAction {
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
        actions.push(CodeActionOrCommand::CodeAction(analyze_action));

        Ok(Some(actions))
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

/// Analyze a single target (binary or library) and return panic points.
/// This reuses the same analysis functions as the CLI for consistency.
fn analyze_single_target(
    target_path: &Path,
    workspace_root: &Path,
    src_filter: &str,
) -> std::result::Result<Vec<CrateCodePoint>, String> {
    use crate::args::OutputFormat;
    use crate::config::Config;
    use crate::sym::{SymbolTable, read_symbols};
    use crate::{analyze_archive, analyze_macho};
    use goblin::mach::Mach::{Binary, Fat};
    use goblin::mach::SingleArch;
    use goblin::mach::constants::cputype::{CPU_TYPE_ARM64, CPU_TYPE_X86_64};

    let binary_buffer =
        std::fs::read(target_path).map_err(|e| format!("Failed to read target: {}", e))?;

    let symbols =
        read_symbols(&binary_buffer).map_err(|e| format!("Failed to read symbols: {}", e))?;

    let config =
        Config::load_for_project(workspace_root, None).unwrap_or_else(|_| Config::with_defaults());

    // Use quiet output format (no progress display in LSP)
    let output = OutputFormat::quiet();

    match symbols {
        SymbolTable::MachO(Binary(macho)) => {
            let result = analyze_macho(
                &macho,
                &binary_buffer,
                target_path,
                Some(src_filter),
                false, // show_timings
                &config,
                &output,
            );
            Ok(result.code_points)
        }
        SymbolTable::MachO(Fat(fat)) => {
            // Fat binary - find native architecture slice and analyze it
            // Prefer slice matching the current host architecture
            let preferred_cputype = match std::env::consts::ARCH {
                "aarch64" => Some(CPU_TYPE_ARM64),
                "x86_64" => Some(CPU_TYPE_X86_64),
                _ => None,
            };

            // Find host-native slice, or fall back to first available
            let mut selected_macho = None;
            for entry in fat.into_iter() {
                match entry {
                    Ok(SingleArch::MachO(macho)) => {
                        if preferred_cputype
                            .map(|cpu| macho.header.cputype == cpu)
                            .unwrap_or(false)
                        {
                            selected_macho = Some(macho);
                            break;
                        }
                        // Keep first as fallback
                        if selected_macho.is_none() {
                            selected_macho = Some(macho);
                        }
                    }
                    Ok(SingleArch::Archive(_)) => continue, // Skip archive slices
                    Err(_) => continue,
                }
            }

            match selected_macho {
                Some(macho) => {
                    let result = analyze_macho(
                        &macho,
                        &binary_buffer,
                        target_path,
                        Some(src_filter),
                        false, // show_timings
                        &config,
                        &output,
                    );
                    Ok(result.code_points)
                }
                None => Err("Fat binary contains no analyzable MachO slices".to_string()),
            }
        }
        SymbolTable::Archive(archive) => {
            let result = analyze_archive(
                &archive,
                &binary_buffer,
                Some(src_filter),
                false, // show_timings
                &config,
                &output,
            );
            Ok(result.code_points)
        }
    }
}

/// Find all config files that affect jonesy analysis.
/// Returns paths to jonesy.toml and all Cargo.toml files (workspace + members).
fn find_config_files(workspace_root: &Path) -> Vec<PathBuf> {
    let mut config_files = Vec::new();

    // Always watch jonesy.toml if it exists (or might be created)
    config_files.push(workspace_root.join("jonesy.toml"));

    // Watch workspace Cargo.toml
    let cargo_toml = workspace_root.join("Cargo.toml");
    if !cargo_toml.exists() {
        return config_files;
    }
    config_files.push(cargo_toml.clone());

    // Parse manifest to find workspace members
    let Ok(content) = std::fs::read_to_string(&cargo_toml) else {
        return config_files;
    };
    let Ok(manifest) = cargo_toml::Manifest::from_slice(content.as_bytes()) else {
        return config_files;
    };

    // Add Cargo.toml for each workspace member
    if let Some(workspace) = &manifest.workspace {
        for member in &workspace.members {
            let member_paths: Vec<PathBuf> = if member.contains('*') {
                expand_workspace_glob(workspace_root, member)
            } else {
                vec![workspace_root.join(member)]
            };

            for member_path in member_paths {
                let member_cargo = member_path.join("Cargo.toml");
                if member_cargo.exists() {
                    config_files.push(member_cargo);
                }
            }
        }
    }

    // Deduplicate paths (can occur with overlapping glob patterns)
    config_files.sort();
    config_files.dedup();
    config_files
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

    // Check for package binaries and libraries (non-virtual workspace or single crate)
    if manifest.package.is_some() {
        // Complete the manifest to discover implicit targets (src/main.rs, src/lib.rs, etc.)
        let mut completed_manifest = manifest.clone();
        completed_manifest
            .complete_from_path_and_workspace::<toml::Value>(
                &cargo_toml,
                None::<(&cargo_toml::Manifest<toml::Value>, &std::path::Path)>,
            )
            .map_err(|e| {
                format!(
                    "Failed to complete manifest {}: {}",
                    cargo_toml.display(),
                    e
                )
            })?;
        collect_binaries_from_manifest(&completed_manifest, &target_debug, &mut targets);
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
                let member_content = match std::fs::read_to_string(&member_cargo) {
                    Ok(content) => content,
                    Err(e) => {
                        eprintln!("Warning: Failed to read {}: {}", member_cargo.display(), e);
                        continue;
                    }
                };
                let mut member_manifest =
                    match cargo_toml::Manifest::from_slice(member_content.as_bytes()) {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("Warning: Failed to parse {}: {}", member_cargo.display(), e);
                            continue;
                        }
                    };
                // Complete the manifest to discover implicit targets
                // Continue on error - don't let one bad member break the whole workspace
                if let Err(e) = member_manifest.complete_from_path_and_workspace(
                    &member_cargo,
                    Some((&manifest, cargo_toml.as_path())),
                ) {
                    eprintln!(
                        "Warning: Failed to complete {}: {}",
                        member_cargo.display(),
                        e
                    );
                    continue;
                }
                collect_binaries_from_manifest(&member_manifest, &target_debug, &mut targets);
            }
        }
    }

    Ok(targets)
}

/// Collect binaries from a completed manifest into the targets vector
fn collect_binaries_from_manifest(
    manifest: &cargo_toml::Manifest,
    target_debug: &Path,
    targets: &mut Vec<PathBuf>,
) {
    let Some(pkg) = &manifest.package else {
        return;
    };
    let pkg_name = &pkg.name;

    // Check for [[bin]] targets (populated by complete_from_path_and_workspace)
    // No fallback probe needed - complete_from_path_and_workspace populates bin if there's a binary
    for bin in &manifest.bin {
        let bin_name = bin.name.as_deref().unwrap_or(pkg_name);
        if let Some(bin_path) = find_binary(target_debug, bin_name) {
            if !targets.contains(&bin_path) {
                targets.push(bin_path);
            }
        }
    }

    // Check for library target
    if manifest.lib.is_some() {
        let lib_name = manifest
            .lib
            .as_ref()
            .and_then(|lib| lib.name.as_deref())
            .unwrap_or(pkg_name);
        if let Some(lib_path) = find_library(target_debug, lib_name) {
            if !targets.contains(&lib_path) {
                targets.push(lib_path);
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Find the workspace root by looking for Cargo.toml with [workspace]
    fn find_workspace_root() -> PathBuf {
        let mut current = std::env::current_dir().unwrap();
        loop {
            let cargo_toml = current.join("Cargo.toml");
            if cargo_toml.exists() {
                let content = std::fs::read_to_string(&cargo_toml).unwrap_or_default();
                if content.contains("[workspace]") {
                    return current;
                }
            }
            if !current.pop() {
                panic!("Could not find workspace root");
            }
        }
    }

    #[test]
    fn test_find_workspace_binaries_with_custom_lib_name() {
        // Use workspace_test example which has a crate with custom [lib] name
        let workspace_root = find_workspace_root();
        let workspace_test_dir = workspace_root.join("examples").join("workspace_test");

        // Build if needed
        let status = std::process::Command::new("cargo")
            .arg("build")
            .current_dir(&workspace_test_dir)
            .status()
            .expect("Failed to build workspace_test");
        assert!(status.success(), "Failed to build workspace_test");

        // Find targets
        let targets =
            find_workspace_binaries(&workspace_test_dir).expect("Should find workspace binaries");

        // Extract just the file names for easier comparison
        let target_names: Vec<String> = targets
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .collect();

        // Expected targets:
        // - crate_a (binary from crate_a)
        // - crate_b_bin (binary from crate_b, explicit [[bin]] name)
        // - libcrate_b_lib.rlib (library from crate_b, [lib] name = "crate_b_lib")
        // - libcrate_c.rlib (library-only crate)

        assert!(
            target_names.iter().any(|n| n == "crate_a"),
            "Should find crate_a binary. Found: {:?}",
            target_names
        );
        assert!(
            target_names.iter().any(|n| n == "crate_b_bin"),
            "Should find crate_b_bin binary. Found: {:?}",
            target_names
        );
        assert!(
            target_names.iter().any(|n| n == "libcrate_b_lib.rlib"),
            "Should find libcrate_b_lib.rlib (custom [lib] name). Found: {:?}",
            target_names
        );
        assert!(
            target_names.iter().any(|n| n == "libcrate_c.rlib"),
            "Should find libcrate_c.rlib (library-only crate). Found: {:?}",
            target_names
        );
    }

    #[test]
    fn test_lsp_analysis_matches_cli() {
        use std::collections::HashSet;

        // Use workspace_test example
        let workspace_root = find_workspace_root();
        let workspace_test_dir = workspace_root.join("examples").join("workspace_test");

        // Build if needed
        let status = std::process::Command::new("cargo")
            .arg("build")
            .current_dir(&workspace_test_dir)
            .status()
            .expect("Failed to build workspace_test");
        assert!(status.success(), "Failed to build workspace_test");

        // Run CLI and capture panic points
        let cli_output = std::process::Command::new(workspace_root.join("target/debug/jonesy"))
            .arg("--quiet")
            .current_dir(&workspace_test_dir)
            .output()
            .expect("Failed to run jonesy CLI");

        let cli_stdout = String::from_utf8_lossy(&cli_output.stdout);

        // Parse CLI output for panic points (top-level lines starting with " --> ")
        // Skip nested points (lines in the call tree that are indented)
        let cli_points: HashSet<(String, u32)> = cli_stdout
            .lines()
            .filter(|line| line.starts_with(" --> ")) // Only top-level points
            .filter_map(|line| {
                // Parse " --> path/to/file.rs:123:45"
                let arrow_pos = line.find(" --> ")?;
                let location = &line[arrow_pos + 5..];
                let parts: Vec<&str> = location.split(':').collect();
                if parts.len() >= 2 {
                    let file = parts[0].trim();
                    let line_num: u32 = parts[1].parse().ok()?;
                    // Normalize path - extract just the relative part
                    let file = file
                        .rsplit("workspace_test/")
                        .next()
                        .unwrap_or(file)
                        .to_string();
                    Some((file, line_num))
                } else {
                    None
                }
            })
            .collect();

        // Run LSP-style analysis using the same functions
        let targets =
            find_workspace_binaries(&workspace_test_dir).expect("Should find workspace binaries");

        let workspace_info =
            discover_workspace(&workspace_test_dir).expect("Should discover workspace");

        let mut lsp_points: HashSet<(String, u32)> = HashSet::new();

        for target in &targets {
            let result =
                analyze_single_target(target, &workspace_test_dir, &workspace_info.src_filter);
            if let Ok(points) = result {
                for point in points {
                    // Normalize path
                    let file = point
                        .file
                        .rsplit("workspace_test/")
                        .next()
                        .unwrap_or(&point.file)
                        .to_string();
                    lsp_points.insert((file, point.line));
                }
            }
        }

        // Compare: LSP should find at least as many points as CLI
        let missing_in_lsp: Vec<_> = cli_points.difference(&lsp_points).collect();
        let extra_in_lsp: Vec<_> = lsp_points.difference(&cli_points).collect();

        if !missing_in_lsp.is_empty() {
            eprintln!("CLI found but LSP missed:");
            for (file, line) in &missing_in_lsp {
                eprintln!("  {}:{}", file, line);
            }
        }

        if !extra_in_lsp.is_empty() {
            eprintln!("LSP found but CLI missed:");
            for (file, line) in &extra_in_lsp {
                eprintln!("  {}:{}", file, line);
            }
        }

        // The LSP should find all points the CLI finds
        assert!(
            missing_in_lsp.is_empty(),
            "LSP analysis should find all panic points that CLI finds. \
             Missing {} points, extra {} points. CLI found {}, LSP found {}",
            missing_in_lsp.len(),
            extra_in_lsp.len(),
            cli_points.len(),
            lsp_points.len()
        );
    }

    #[test]
    fn test_create_inline_allow_action() {
        let uri = Url::parse("file:///tmp/test.rs").unwrap();
        let range = Range {
            start: Position {
                line: 10,
                character: 5,
            },
            end: Position {
                line: 10,
                character: 15,
            },
        };
        let diagnostic = Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("jonesy".to_string()),
            message: "test".to_string(),
            ..Default::default()
        };

        // Test specific cause
        let action =
            JonesyLspServer::create_inline_allow_action(&uri, range, "unwrap", &diagnostic)
                .unwrap();
        assert_eq!(action.title, "Allow 'unwrap' on this line");
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
        assert!(action.is_preferred.unwrap_or(false)); // Specific cause is preferred

        // Verify the edit inserts the comment
        let edit = action.edit.unwrap();
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, " // jonesy:allow(unwrap)");

        // Test wildcard
        let action =
            JonesyLspServer::create_inline_allow_action(&uri, range, "*", &diagnostic).unwrap();
        assert_eq!(action.title, "Allow all panics on this line");
        assert!(!action.is_preferred.unwrap_or(true)); // Wildcard is not preferred
    }

    #[test]
    fn test_create_file_allow_action() {
        let uri = Url::parse("file:///workspace/src/main.rs").unwrap();
        let range = Range::default();
        let diagnostic = Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("jonesy".to_string()),
            message: "test".to_string(),
            ..Default::default()
        };
        let workspace_root = PathBuf::from("/workspace");

        let action =
            JonesyLspServer::create_file_allow_action(&uri, "unwrap", &workspace_root, &diagnostic)
                .unwrap();

        assert_eq!(action.title, "Allow 'unwrap' in main.rs");
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));

        // Verify the edit targets jonesy.toml
        let edit = action.edit.unwrap();
        let doc_changes = edit.document_changes.unwrap();
        match doc_changes {
            DocumentChanges::Operations(ops) => {
                // Should have at least a text edit (possibly a create file too)
                assert!(!ops.is_empty());
            }
            _ => panic!("Expected Operations"),
        }
    }

    #[test]
    fn test_create_function_allow_action() {
        let range = Range::default();
        let diagnostic = Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("jonesy".to_string()),
            message: "test".to_string(),
            ..Default::default()
        };
        let workspace_root = PathBuf::from("/workspace");

        let action = JonesyLspServer::create_function_allow_action(
            "my_crate::module::parse_config",
            "unwrap",
            &workspace_root,
            &diagnostic,
        )
        .unwrap();

        // Should extract just the function name, but clarify it matches all functions with that name
        assert_eq!(
            action.title,
            "Allow 'unwrap' in all functions named 'parse_config'"
        );
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
    }

    #[test]
    fn test_find_config_files() {
        use std::collections::HashSet;

        // Use workspace_test example which has multiple workspace members
        let workspace_root = find_workspace_root();
        let workspace_test_dir = workspace_root.join("examples").join("workspace_test");

        let config_files = find_config_files(&workspace_test_dir);

        // Collect unique full paths to verify no duplicates
        let unique_paths: HashSet<String> = config_files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        // Verify no duplicates (set size should equal vec size)
        assert_eq!(
            config_files.len(),
            unique_paths.len(),
            "Config files should not contain duplicates. Found {} paths but {} unique.",
            config_files.len(),
            unique_paths.len()
        );

        // Should always include jonesy.toml (even if it doesn't exist)
        assert!(
            config_files.iter().any(|p| p.ends_with("jonesy.toml")),
            "Should include jonesy.toml path. Found: {:?}",
            config_files
        );

        // Should include workspace Cargo.toml
        assert!(
            config_files.iter().any(|p| {
                p.parent()
                    .map(|parent| parent.ends_with("workspace_test"))
                    .unwrap_or(false)
                    && p.ends_with("Cargo.toml")
            }),
            "Should include workspace Cargo.toml. Found: {:?}",
            config_files
        );

        // Should include each member's Cargo.toml
        for member in ["crate_a", "crate_b", "crate_c"] {
            assert!(
                config_files.iter().any(|p| {
                    p.parent()
                        .map(|parent| parent.ends_with(member))
                        .unwrap_or(false)
                        && p.ends_with("Cargo.toml")
                }),
                "Should include {}/Cargo.toml. Found: {:?}",
                member,
                config_files
            );
        }

        // Total should be exactly 5: jonesy.toml + workspace Cargo.toml + 3 members
        assert_eq!(
            config_files.len(),
            5,
            "Should find exactly 5 config files. Found: {:?}",
            config_files
        );
    }
}
