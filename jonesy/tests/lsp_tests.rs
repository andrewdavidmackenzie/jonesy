//! LSP integration tests for jonesy.
//!
//! These tests spawn the jonesy LSP server as a subprocess and communicate
//! with it via JSON-RPC over stdin/stdout.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Helper to find the workspace root
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

/// Ensure jonesy is built
fn ensure_jonesy_built(workspace_root: &PathBuf) {
    let status = Command::new("cargo")
        .args(["build", "--package", "jonesy"])
        .current_dir(workspace_root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("Failed to build jonesy");
    assert!(status.success(), "Failed to build jonesy");
}

/// LSP client for testing with timeout support
struct TestLspClient {
    child: Child,
    request_id: i32,
    response_rx: mpsc::Receiver<Value>,
    _reader_thread: thread::JoinHandle<()>,
    /// Buffer for notifications received during wait_for_response
    notification_buffer: Vec<Value>,
}

impl TestLspClient {
    /// Start the LSP server subprocess with a background reader thread
    fn start(working_dir: &PathBuf) -> Self {
        // Always use jonesy from the main workspace target/debug
        let main_workspace = find_workspace_root();
        let jonesy_bin = main_workspace.join("target/debug/jonesy");

        let mut child = Command::new(&jonesy_bin)
            .arg("lsp")
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("Failed to spawn jonesy lsp");

        // Take stdout and spawn a reader thread
        let stdout = child.stdout.take().expect("Failed to get stdout");
        let (tx, rx) = mpsc::channel();

        let reader_thread = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(message) = read_lsp_message(&mut reader) {
                if tx.send(message).is_err() {
                    break; // Receiver dropped
                }
            }
        });

        Self {
            child,
            request_id: 0,
            response_rx: rx,
            _reader_thread: reader_thread,
            notification_buffer: Vec::new(),
        }
    }

    /// Send a JSON-RPC request and return the response with timeout
    fn send_request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.send_request_timeout(method, params, Duration::from_secs(10))
    }

    /// Send a JSON-RPC request with custom timeout
    fn send_request_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.request_id += 1;
        let id = self.request_id;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        self.send_message(&request)?;
        self.wait_for_response(id, timeout)
    }

    /// Send a JSON-RPC notification (no response expected)
    fn send_notification(&mut self, method: &str, params: Value) -> Result<(), String> {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });

        self.send_message(&notification)
    }

    /// Send a message with LSP content-length header
    fn send_message(&mut self, message: &Value) -> Result<(), String> {
        let content = serde_json::to_string(message).map_err(|e| e.to_string())?;
        let header = format!("Content-Length: {}\r\n\r\n", content.len());

        let stdin = self.child.stdin.as_mut().ok_or("Failed to get stdin")?;
        stdin
            .write_all(header.as_bytes())
            .map_err(|e| e.to_string())?;
        stdin
            .write_all(content.as_bytes())
            .map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Wait for a response with the given ID, buffering notifications for later retrieval
    fn wait_for_response(&mut self, expected_id: i32, timeout: Duration) -> Result<Value, String> {
        let deadline = std::time::Instant::now() + timeout;

        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .unwrap_or(Duration::ZERO);

            if remaining.is_zero() {
                return Err("Timeout waiting for response".to_string());
            }

            match self.response_rx.recv_timeout(remaining) {
                Ok(message) => {
                    // Check if this is our response (has id, no method = response)
                    if let Some(id) = message.get("id") {
                        // Server-initiated request (has both id and method) - respond to it
                        if message.get("method").is_some() {
                            self.respond_to_server_request(&message);
                            continue;
                        }
                        // Our response
                        if id.as_i64() == Some(expected_id as i64) {
                            return Ok(message);
                        }
                    }
                    // Notification (no id) - buffer it
                    if message.get("id").is_none() {
                        self.notification_buffer.push(message);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err("Timeout waiting for response".to_string());
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Reader thread disconnected".to_string());
                }
            }
        }
    }

    /// Respond to server-initiated requests (like workDoneProgress/create)
    fn respond_to_server_request(&mut self, request: &Value) {
        let id = request.get("id").cloned().unwrap_or(json!(null));
        let response = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": null
        });
        let _ = self.send_message(&response);
    }

    /// Collect notifications for a duration (also drains buffered notifications)
    /// Also responds to any server requests received during collection
    #[allow(dead_code)]
    fn collect_notifications(&mut self, duration: Duration) -> Vec<Value> {
        // First, drain any notifications that were buffered during wait_for_response
        let mut notifications: Vec<Value> = self.notification_buffer.drain(..).collect();

        let deadline = std::time::Instant::now() + duration;

        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .unwrap_or(Duration::ZERO);

            if remaining.is_zero() {
                break;
            }

            match self.response_rx.recv_timeout(remaining) {
                Ok(message) => {
                    // Server request (has both id and method) - respond to it
                    if message.get("id").is_some() && message.get("method").is_some() {
                        self.respond_to_server_request(&message);
                        continue;
                    }
                    // Notification (no id)
                    if message.get("id").is_none() {
                        notifications.push(message);
                    }
                    // Response to our request - ignore during notification collection
                }
                Err(_) => break,
            }
        }

        notifications
    }

    /// Shutdown the server gracefully
    fn shutdown(mut self) {
        // Send shutdown request (ignore errors)
        let _ = self.send_request("shutdown", json!({}));

        // Send exit notification
        let _ = self.send_notification("exit", json!(null));

        // Wait briefly for process to exit
        thread::sleep(Duration::from_millis(100));

        // Force kill if still running
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for TestLspClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Read a single LSP message from a buffered reader
fn read_lsp_message<R: BufRead>(reader: &mut R) -> Result<Value, String> {
    // Read headers
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let line = line.trim();

        if line.is_empty() {
            break;
        }

        if line.to_lowercase().starts_with("content-length:") {
            let len_str = line.split(':').nth(1).ok_or("Invalid header")?.trim();
            content_length = Some(
                len_str
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?,
            );
        }
    }

    let content_length = content_length.ok_or("No Content-Length header")?;

    // Read content
    let mut content = vec![0u8; content_length];
    reader.read_exact(&mut content).map_err(|e| e.to_string())?;

    serde_json::from_slice(&content).map_err(|e| e.to_string())
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn test_lsp_initialize() {
    let workspace_root = find_workspace_root();
    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&workspace_root);

    // Send initialize request
    let response = client
        .send_request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": format!("file://{}", workspace_root.display()),
                "capabilities": {}
            }),
        )
        .expect("Initialize request failed");

    // Verify response
    assert!(
        response.get("error").is_none(),
        "Initialize failed: {:?}",
        response
    );

    let result = response.get("result").expect("No result in response");

    // Check server capabilities
    let capabilities = result.get("capabilities").expect("No capabilities");
    assert!(
        capabilities.get("textDocumentSync").is_some(),
        "Missing textDocumentSync capability"
    );
    assert!(
        capabilities.get("codeActionProvider").is_some(),
        "Missing codeActionProvider capability"
    );
    assert!(
        capabilities.get("executeCommandProvider").is_some(),
        "Missing executeCommandProvider capability"
    );

    // Check server info
    let server_info = result.get("serverInfo").expect("No serverInfo");
    assert_eq!(server_info.get("name"), Some(&json!("jonesy")));

    client.shutdown();
}

#[test]
fn test_lsp_initialized_notification() {
    let workspace_root = find_workspace_root();
    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&workspace_root);

    // Initialize
    let _ = client.send_request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", workspace_root.display()),
            "capabilities": {}
        }),
    );

    // Send initialized notification (should not error)
    client
        .send_notification("initialized", json!({}))
        .expect("Failed to send initialized notification");

    // Give server time to process
    thread::sleep(Duration::from_millis(100));

    client.shutdown();
}

#[test]
fn test_lsp_shutdown() {
    let workspace_root = find_workspace_root();
    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&workspace_root);

    // Initialize
    let _ = client.send_request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", workspace_root.display()),
            "capabilities": {}
        }),
    );

    // Use the shutdown helper which handles the proper protocol
    client.shutdown();

    // If we get here without panicking, shutdown worked
}

#[test]
fn test_lsp_server_info_version() {
    let workspace_root = find_workspace_root();
    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&workspace_root);

    // Initialize
    let response = client
        .send_request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": format!("file://{}", workspace_root.display()),
                "capabilities": {}
            }),
        )
        .expect("Initialize failed");

    let result = response.get("result").expect("No result");
    let server_info = result.get("serverInfo").expect("No serverInfo");

    // Version should be present and non-empty
    let version = server_info.get("version").expect("No version");
    assert!(version.is_string(), "Version should be a string");
    let version_str = version.as_str().unwrap();
    assert!(!version_str.is_empty(), "Version should not be empty");
    assert!(
        version_str.contains('.'),
        "Version should be semver-like: {}",
        version_str
    );

    client.shutdown();
}

#[test]
fn test_lsp_workspace_folders() {
    let workspace_root = find_workspace_root();
    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&workspace_root);

    // Initialize with workspace folders instead of rootUri
    let response = client
        .send_request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": null,
                "workspaceFolders": [{
                    "uri": format!("file://{}", workspace_root.display()),
                    "name": "jonesy"
                }],
                "capabilities": {}
            }),
        )
        .expect("Initialize failed");

    assert!(
        response.get("error").is_none(),
        "Initialize with workspace folders failed: {:?}",
        response
    );

    client.shutdown();
}

#[test]
fn test_lsp_did_open_close() {
    let workspace_root = find_workspace_root();
    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&workspace_root);

    // Initialize
    let _ = client.send_request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", workspace_root.display()),
            "capabilities": {}
        }),
    );

    client.send_notification("initialized", json!({})).unwrap();

    // Open a file
    let test_file = workspace_root.join("jonesy/src/main.rs");
    client
        .send_notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": format!("file://{}", test_file.display()),
                    "languageId": "rust",
                    "version": 1,
                    "text": "fn main() {}"
                }
            }),
        )
        .expect("didOpen failed");

    // Close the file
    client
        .send_notification(
            "textDocument/didClose",
            json!({
                "textDocument": {
                    "uri": format!("file://{}", test_file.display())
                }
            }),
        )
        .expect("didClose failed");

    thread::sleep(Duration::from_millis(100));

    client.shutdown();
}

#[test]
fn test_lsp_code_action_returns_analyze() {
    let workspace_root = find_workspace_root();
    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&workspace_root);

    // Initialize
    let _ = client.send_request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", workspace_root.display()),
            "capabilities": {}
        }),
    );

    client.send_notification("initialized", json!({})).unwrap();

    // Request code actions (without diagnostics, should still return analyze action)
    let main_rs = workspace_root.join("jonesy/src/main.rs");
    let response = client
        .send_request(
            "textDocument/codeAction",
            json!({
                "textDocument": {
                    "uri": format!("file://{}", main_rs.display())
                },
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 0}
                },
                "context": {
                    "diagnostics": []
                }
            }),
        )
        .expect("Code action request failed");

    assert!(
        response.get("error").is_none(),
        "Code action failed: {:?}",
        response
    );

    let result = response.get("result").expect("No result");
    let actions = result.as_array().expect("Result is not an array");

    // Should have the "Run Jonesy Panic Analysis" action
    let has_analyze_action = actions.iter().any(|action| {
        action
            .get("title")
            .and_then(|t| t.as_str())
            .map(|t| t.contains("Jonesy"))
            .unwrap_or(false)
    });

    assert!(
        has_analyze_action,
        "Should have 'Run Jonesy Panic Analysis' action. Actions: {:?}",
        actions
    );

    client.shutdown();
}

#[test]
fn test_lsp_code_action_with_diagnostic() {
    let workspace_root = find_workspace_root();
    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&workspace_root);

    // Initialize
    let _ = client.send_request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", workspace_root.display()),
            "capabilities": {}
        }),
    );

    client.send_notification("initialized", json!({})).unwrap();

    // Create a mock jonesy diagnostic
    let main_rs = workspace_root.join("jonesy/src/main.rs");
    let mock_diagnostic = json!({
        "range": {
            "start": {"line": 5, "character": 0},
            "end": {"line": 5, "character": 10}
        },
        "severity": 2,
        "source": "jonesy",
        "message": "panic point: unwrap() on None",
        "data": {
            "causes": ["unwrap"],
            "function": "main",
            "file": "src/main.rs"
        }
    });

    // Request code actions with the diagnostic
    let response = client
        .send_request(
            "textDocument/codeAction",
            json!({
                "textDocument": {
                    "uri": format!("file://{}", main_rs.display())
                },
                "range": {
                    "start": {"line": 5, "character": 0},
                    "end": {"line": 5, "character": 10}
                },
                "context": {
                    "diagnostics": [mock_diagnostic]
                }
            }),
        )
        .expect("Code action request failed");

    assert!(
        response.get("error").is_none(),
        "Code action failed: {:?}",
        response
    );

    let result = response.get("result").expect("No result");
    let actions = result.as_array().expect("Result is not an array");

    // Should have quick fix actions for the diagnostic
    let has_allow_action = actions.iter().any(|action| {
        action
            .get("title")
            .and_then(|t| t.as_str())
            .map(|t| t.contains("Allow"))
            .unwrap_or(false)
    });

    assert!(
        has_allow_action,
        "Should have 'Allow' quick fix actions. Actions: {:?}",
        actions
    );

    client.shutdown();
}

#[test]
fn test_lsp_code_action_called_function_allow() {
    let workspace_root = find_workspace_root();
    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&workspace_root);

    // Initialize
    let _ = client.send_request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", workspace_root.display()),
            "capabilities": {}
        }),
    );

    client.send_notification("initialized", json!({})).unwrap();

    // Create a mock diagnostic for an indirect panic (called_function present)
    let main_rs = workspace_root.join("jonesy/src/main.rs");
    let mock_diagnostic = json!({
        "range": {
            "start": {"line": 10, "character": 0},
            "end": {"line": 10, "character": 20}
        },
        "severity": 2,
        "source": "jonesy",
        "message": "panic point: unwrap() on None",
        "data": {
            "causes": ["unwrap"],
            "function": "run",
            "file": "src/main.rs",
            "called_function": "parse_config",
            "is_direct_panic": false
        }
    });

    // Request code actions with the diagnostic
    let response = client
        .send_request(
            "textDocument/codeAction",
            json!({
                "textDocument": {
                    "uri": format!("file://{}", main_rs.display())
                },
                "range": {
                    "start": {"line": 10, "character": 0},
                    "end": {"line": 10, "character": 20}
                },
                "context": {
                    "diagnostics": [mock_diagnostic]
                }
            }),
        )
        .expect("Code action request failed");

    assert!(
        response.get("error").is_none(),
        "Code action failed: {:?}",
        response
    );

    let result = response.get("result").expect("No result");
    let actions = result.as_array().expect("Result is not an array");

    // Should have the called-function allow action
    let has_called_fn_action = actions.iter().any(|action| {
        action
            .get("title")
            .and_then(|t| t.as_str())
            .map(|t| t.contains("Allow panics on calls to 'parse_config()'"))
            .unwrap_or(false)
    });

    assert!(
        has_called_fn_action,
        "Should have 'Allow panics on calls to parse_config()' action. Actions: {actions:?}",
    );

    // Verify there's exactly one such action (not duplicated per cause)
    let called_fn_count = actions
        .iter()
        .filter(|action| {
            action
                .get("title")
                .and_then(|t| t.as_str())
                .map(|t| t.contains("parse_config"))
                .unwrap_or(false)
        })
        .count();

    assert_eq!(
        called_fn_count, 1,
        "Called-function action should appear exactly once, found {called_fn_count}"
    );

    client.shutdown();
}

#[test]
fn test_lsp_execute_command_analyze() {
    let workspace_root = find_workspace_root();
    let panic_example = workspace_root.join("examples/panic");

    // Build the panic example with local target directory
    let status = Command::new("cargo")
        .args(["build", "--target-dir", "./target"])
        .current_dir(&panic_example)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("Failed to build panic example");
    assert!(status.success());

    // Generate dSYM for macOS debug info
    let binary_path = panic_example.join("target/debug/panic");
    let _ = Command::new("dsymutil")
        .arg(&binary_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&panic_example);

    // Initialize with panic example as workspace
    let _ = client.send_request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", panic_example.display()),
            "capabilities": {}
        }),
    );

    client.send_notification("initialized", json!({})).unwrap();

    // Wait for initial analysis to complete
    thread::sleep(Duration::from_secs(2));

    // Execute jonesy.analyze command
    let response = client
        .send_request(
            "workspace/executeCommand",
            json!({
                "command": "jonesy.analyze",
                "arguments": []
            }),
        )
        .expect("Execute command failed");

    assert!(
        response.get("error").is_none(),
        "Execute command failed: {:?}",
        response
    );

    // Verify the command returned a result (success field is optional)
    assert!(
        response.get("result").is_some(),
        "Command should return a result"
    );

    client.shutdown();
}

#[test]
fn test_lsp_diagnostics_published() {
    let workspace_root = find_workspace_root();
    let panic_example = workspace_root.join("examples/panic");

    // Build the panic example with local target directory
    let status = Command::new("cargo")
        .args(["build", "--target-dir", "./target"])
        .current_dir(&panic_example)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("Failed to build panic example");
    assert!(status.success());

    // Generate dSYM for macOS debug info
    let binary_path = panic_example.join("target/debug/panic");
    let _ = Command::new("dsymutil")
        .arg(&binary_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&panic_example);

    // Initialize
    let _ = client.send_request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", panic_example.display()),
            "capabilities": {}
        }),
    );

    client.send_notification("initialized", json!({})).unwrap();

    // Wait briefly for server to start processing
    thread::sleep(Duration::from_millis(500));

    // Drain initial log messages
    let _ = client.collect_notifications(Duration::from_secs(2));

    // Manually trigger analysis and wait for completion (60s timeout for analysis)
    let response = client
        .send_request_timeout(
            "workspace/executeCommand",
            json!({
                "command": "jonesy.analyze",
                "arguments": []
            }),
            Duration::from_secs(60),
        )
        .expect("Execute command failed");

    assert!(
        response.get("error").is_none(),
        "Analysis command failed: {:?}",
        response
    );

    // Now collect the diagnostics that were published during analysis
    let notifications = client.collect_notifications(Duration::from_secs(5));

    // Filter for publishDiagnostics notifications
    let diagnostic_notifications: Vec<_> = notifications
        .iter()
        .filter(|n| n.get("method") == Some(&json!("textDocument/publishDiagnostics")))
        .collect();

    // Debug: print what notifications we received
    let notification_methods: Vec<_> = notifications
        .iter()
        .filter_map(|n| n.get("method").and_then(|m| m.as_str()))
        .collect();

    // Should have published diagnostics for at least one file
    assert!(
        !diagnostic_notifications.is_empty(),
        "Should have published diagnostics. Got {} notifications total. Methods: {:?}",
        notifications.len(),
        notification_methods
    );

    // Verify diagnostic structure
    for notif in &diagnostic_notifications {
        let params = notif
            .get("params")
            .expect("publishDiagnostics should have params");

        // Should have uri field
        assert!(
            params.get("uri").is_some(),
            "Diagnostic should have uri: {:?}",
            params
        );

        // Should have diagnostics array
        let diagnostics = params
            .get("diagnostics")
            .expect("Should have diagnostics array");
        assert!(diagnostics.is_array(), "diagnostics should be an array");

        // If there are diagnostics, verify their structure
        if let Some(arr) = diagnostics.as_array() {
            for diag in arr {
                // Should have range
                assert!(diag.get("range").is_some(), "Diagnostic should have range");

                // Should have message
                assert!(
                    diag.get("message").is_some(),
                    "Diagnostic should have message"
                );

                // Should have source = "jonesy"
                assert_eq!(
                    diag.get("source"),
                    Some(&json!("jonesy")),
                    "Diagnostic source should be 'jonesy'"
                );

                // Should have severity (warning = 2)
                assert_eq!(
                    diag.get("severity"),
                    Some(&json!(2)),
                    "Diagnostic severity should be warning (2)"
                );
            }
        }
    }

    // At least one file should have actual diagnostics (panic example has many panic points)
    let has_diagnostics = diagnostic_notifications.iter().any(|n| {
        n.get("params")
            .and_then(|p| p.get("diagnostics"))
            .and_then(|d| d.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false)
    });

    assert!(
        has_diagnostics,
        "At least one file should have diagnostics in panic example"
    );

    client.shutdown();
}

#[test]
fn test_lsp_diagnostics_contain_error_codes() {
    let workspace_root = find_workspace_root();
    let panic_example = workspace_root.join("examples/panic");

    // Build the panic example with local target directory
    let status = Command::new("cargo")
        .args(["build", "--target-dir", "./target"])
        .current_dir(&panic_example)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("Failed to build panic example");
    assert!(status.success());

    // Generate dSYM for macOS debug info
    let binary_path = panic_example.join("target/debug/panic");
    let _ = Command::new("dsymutil")
        .arg(&binary_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&panic_example);

    // Initialize
    let _ = client.send_request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", panic_example.display()),
            "capabilities": {}
        }),
    );

    client.send_notification("initialized", json!({})).unwrap();

    // Wait briefly then drain initial messages
    thread::sleep(Duration::from_millis(500));
    let _ = client.collect_notifications(Duration::from_secs(2));

    // Trigger analysis manually (use longer timeout as analysis can take time)
    let _ = client.send_request_timeout(
        "workspace/executeCommand",
        json!({
            "command": "jonesy.analyze",
            "arguments": []
        }),
        Duration::from_secs(60),
    );

    // Collect notifications from analysis
    let notifications = client.collect_notifications(Duration::from_secs(10));

    // Find diagnostics with error codes
    let mut found_error_code = false;
    let mut found_docs_url = false;

    for notif in &notifications {
        if notif.get("method") != Some(&json!("textDocument/publishDiagnostics")) {
            continue;
        }

        if let Some(params) = notif.get("params") {
            if let Some(diagnostics) = params.get("diagnostics").and_then(|d| d.as_array()) {
                for diag in diagnostics {
                    // Check for error code (JP001, JP002, etc.)
                    if let Some(code) = diag.get("code") {
                        if let Some(code_str) = code.as_str() {
                            if code_str.starts_with("JP") {
                                found_error_code = true;
                            }
                        }
                    }

                    // Check for documentation URL in codeDescription
                    if let Some(code_desc) = diag.get("codeDescription") {
                        if code_desc.get("href").is_some() {
                            found_docs_url = true;
                        }
                    }
                }
            }
        }
    }

    assert!(
        found_error_code,
        "Diagnostics should contain JP error codes"
    );

    assert!(
        found_docs_url,
        "Diagnostics should contain documentation URLs in codeDescription"
    );

    client.shutdown();
}

#[test]
fn test_lsp_binary_change_triggers_reanalysis() {
    use std::fs;

    let workspace_root = find_workspace_root();
    let panic_example = workspace_root.join("examples/panic");

    // Build the panic example with local target directory
    let status = Command::new("cargo")
        .args(["build", "--target-dir", "./target"])
        .current_dir(&panic_example)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("Failed to build panic example");
    assert!(status.success());

    // Generate dSYM for macOS debug info
    let binary_path = panic_example.join("target/debug/panic");
    let _ = Command::new("dsymutil")
        .arg(&binary_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&panic_example);

    // Initialize
    let _ = client.send_request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", panic_example.display()),
            "capabilities": {}
        }),
    );

    client.send_notification("initialized", json!({})).unwrap();

    // Wait for initial analysis to complete
    let _ = client.collect_notifications(Duration::from_secs(3));

    // Touch the binary to simulate a rebuild
    let binary_path = panic_example.join("target/debug/panic");
    assert!(
        binary_path.exists(),
        "Binary should exist after build: {}",
        binary_path.display()
    );

    // Read and rewrite to update mtime
    let content = fs::read(&binary_path).unwrap();
    fs::write(&binary_path, content).unwrap();

    // Send didChangeWatchedFiles notification (simulating what the client would do)
    client
        .send_notification(
            "workspace/didChangeWatchedFiles",
            json!({
                "changes": [{
                    "uri": format!("file://{}", binary_path.display()),
                    "type": 2  // Changed
                }]
            }),
        )
        .expect("Failed to send didChangeWatchedFiles");

    // Collect notifications - should see new diagnostics being published
    let notifications = client.collect_notifications(Duration::from_secs(5));

    // Should have triggered re-analysis (look for publishDiagnostics)
    let has_diagnostics = notifications
        .iter()
        .any(|n| n.get("method") == Some(&json!("textDocument/publishDiagnostics")));

    assert!(
        has_diagnostics,
        "Binary change should trigger re-analysis and publish diagnostics"
    );

    client.shutdown();
}

#[test]
fn test_lsp_progress_notifications() {
    let workspace_root = find_workspace_root();
    let panic_example = workspace_root.join("examples/panic");

    // Build the panic example with local target directory
    let status = Command::new("cargo")
        .args(["build", "--target-dir", "./target"])
        .current_dir(&panic_example)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("Failed to build panic example");
    assert!(status.success());

    // Generate dSYM for macOS debug info
    let binary_path = panic_example.join("target/debug/panic");
    let _ = Command::new("dsymutil")
        .arg(&binary_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&panic_example);

    // Initialize
    let _ = client.send_request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", panic_example.display()),
            "capabilities": {}
        }),
    );

    client.send_notification("initialized", json!({})).unwrap();

    // Wait briefly then drain initial messages
    thread::sleep(Duration::from_millis(500));
    let _ = client.collect_notifications(Duration::from_secs(2));

    // Trigger analysis and wait for completion
    let _ = client.send_request_timeout(
        "workspace/executeCommand",
        json!({
            "command": "jonesy.analyze",
            "arguments": []
        }),
        Duration::from_secs(60),
    );

    // Collect notifications from analysis
    let notifications = client.collect_notifications(Duration::from_secs(5));

    // Filter for progress notifications ($/progress)
    let progress_notifications: Vec<_> = notifications
        .iter()
        .filter(|n| {
            n.get("method")
                .and_then(|m| m.as_str())
                .map(|m| m == "$/progress")
                .unwrap_or(false)
        })
        .collect();

    // Should have at least some progress notifications (begin + end at minimum)
    assert!(
        !progress_notifications.is_empty(),
        "Should have received $/progress notifications during analysis. \
         Got {} total notifications.",
        notifications.len()
    );

    // Verify we have begin and end kinds
    let has_begin = progress_notifications.iter().any(|n| {
        n.get("params")
            .and_then(|p| p.get("value"))
            .and_then(|v| v.get("kind"))
            .and_then(|k| k.as_str())
            == Some("begin")
    });

    let has_end = progress_notifications.iter().any(|n| {
        n.get("params")
            .and_then(|p| p.get("value"))
            .and_then(|v| v.get("kind"))
            .and_then(|k| k.as_str())
            == Some("end")
    });

    assert!(
        has_begin,
        "Should have a progress 'begin' notification. Progress notifications: {:?}",
        progress_notifications
    );

    assert!(
        has_end,
        "Should have a progress 'end' notification. Progress notifications: {:?}",
        progress_notifications
    );

    client.shutdown();
}

#[test]
fn test_lsp_file_change_watching() {
    use std::fs;

    let workspace_root = find_workspace_root();
    let panic_example = workspace_root.join("examples/panic");

    // Build the panic example with local target directory
    let status = Command::new("cargo")
        .args(["build", "--target-dir", "./target"])
        .current_dir(&panic_example)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("Failed to build panic example");
    assert!(status.success());

    // Generate dSYM for macOS debug info
    let binary_path = panic_example.join("target/debug/panic");
    let _ = Command::new("dsymutil")
        .arg(&binary_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    ensure_jonesy_built(&workspace_root);

    let mut client = TestLspClient::start(&panic_example);

    // Initialize
    let _ = client.send_request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": format!("file://{}", panic_example.display()),
            "capabilities": {}
        }),
    );

    client.send_notification("initialized", json!({})).unwrap();

    // Wait for initial analysis to complete and drain notifications
    let _ = client.collect_notifications(Duration::from_secs(5));

    // Trigger a config file change (jonesy.toml) via didChangeWatchedFiles
    let config_path = panic_example.join("jonesy.toml");
    // Create a minimal config file to trigger the watcher
    fs::write(&config_path, "# jonesy config\n").expect("Failed to create test config file");

    client
        .send_notification(
            "workspace/didChangeWatchedFiles",
            json!({
                "changes": [{
                    "uri": format!("file://{}", config_path.display()),
                    "type": 2  // Changed
                }]
            }),
        )
        .expect("Failed to send didChangeWatchedFiles for config");

    // Collect notifications - should see re-analysis triggered
    let notifications = client.collect_notifications(Duration::from_secs(10));

    // Should have triggered re-analysis (look for publishDiagnostics or progress)
    let has_response = notifications.iter().any(|n| {
        let method = n.get("method").and_then(|m| m.as_str()).unwrap_or("");
        method == "textDocument/publishDiagnostics" || method == "$/progress"
    });

    assert!(
        has_response,
        "Config file change should trigger re-analysis. \
         Got {} notifications: {:?}",
        notifications.len(),
        notifications
            .iter()
            .filter_map(|n| n.get("method").and_then(|m| m.as_str()))
            .collect::<Vec<_>>()
    );

    // Clean up the test config file
    let _ = fs::remove_file(&config_path);

    client.shutdown();
}
