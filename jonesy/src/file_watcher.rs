//! Native file watching for LSP binary change detection.
//!
//! This module provides platform-agnostic file watching to detect when
//! binaries or config files change, triggering LSP re-analysis.
//!
//! # File Changes That Trigger Re-Analysis
//!
//! ## Binary/Library Changes (in `target/debug/`)
//!
//! | File Type | Pattern | Trigger Reason |
//! |-----------|---------|----------------|
//! | Binary executable | `target/debug/<name>` (no extension) | Main crate binary rebuilt |
//! | Rust library | `target/debug/lib<name>.rlib` | Library crate rebuilt |
//! | Dynamic library | `target/debug/lib<name>.dylib` (macOS) | cdylib/dylib rebuilt |
//! | Dynamic library | `target/debug/lib<name>.so` (Linux) | cdylib/dylib rebuilt |
//! | Static library | `target/debug/lib<name>.a` | staticlib rebuilt |
//!
//! ## Config File Changes
//!
//! | File | Location | Trigger Reason |
//! |------|----------|----------------|
//! | `jonesy.toml` | Workspace root | Allow/deny rules changed |
//! | `Cargo.toml` | Workspace root | Workspace members changed |
//! | `Cargo.toml` | Each member crate | Dependencies or features changed |
//!
//! # Platform Support
//!
//! The `notify` crate provides cross-platform file watching:
//! - **macOS**: FSEvents (efficient, low overhead)
//! - **Linux**: inotify (may require increasing watch limits for large projects)
//! - **Windows**: ReadDirectoryChangesW
//!
//! The design allows for custom platform-specific implementations if needed.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

/// Events emitted by the file watcher.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// A binary or library file changed (created, modified, or removed).
    BinaryChanged(PathBuf),
    /// A config file changed (jonesy.toml, Cargo.toml).
    ConfigChanged(PathBuf),
    /// The watcher encountered an error.
    Error(String),
}

/// Configuration for the file watcher.
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Directory to watch for binary changes (e.g., target/debug/).
    pub target_dir: PathBuf,
    /// Config files to watch (e.g., jonesy.toml, Cargo.toml).
    pub config_files: Vec<PathBuf>,
    /// Debounce duration - events within this window are coalesced.
    pub debounce: Duration,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            target_dir: PathBuf::from("target/debug"),
            config_files: Vec::new(),
            debounce: Duration::from_millis(500),
        }
    }
}

/// A handle to control the file watcher.
///
/// IMPORTANT: This handle must be kept alive for the watcher to function.
/// Dropping this handle stops file watching.
pub struct WatcherHandle {
    /// Receiver for watch events.
    pub events: mpsc::Receiver<WatchEvent>,
    /// Keep the watcher alive - dropping this stops watching.
    pub watcher: RecommendedWatcher,
}

/// Start watching for file changes.
///
/// Returns a handle with an event receiver. The watcher runs until the handle
/// is dropped.
///
/// # Platform Notes
/// - macOS: Uses FSEvents (efficient, low overhead)
/// - Linux: Uses inotify (requires inotify watches, may hit limits)
/// - Windows: Uses ReadDirectoryChangesW
pub fn start_watching(config: WatcherConfig) -> Result<WatcherHandle, String> {
    let (tx, rx) = mpsc::channel(100);

    // Clone paths for the closure
    let target_dir = config.target_dir.clone();
    let config_files: Vec<PathBuf> = config.config_files.clone();

    // Create the notify watcher
    let tx_clone = tx.clone();

    let mut watcher = notify::recommended_watcher(move |result: Result<Event, notify::Error>| {
        match result {
            Ok(event) => {
                // Filter to relevant event kinds
                let dominated_events = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                );

                if !dominated_events {
                    return;
                }

                for path in event.paths {
                    let watch_event = categorize_path(&path, &target_dir, &config_files);
                    if let Some(evt) = watch_event {
                        // Non-blocking send - drop events if channel is full
                        let _ = tx_clone.try_send(evt);
                    }
                }
            }
            Err(e) => {
                let _ = tx_clone.try_send(WatchEvent::Error(e.to_string()));
            }
        }
    })
    .map_err(|e| format!("Failed to create file watcher: {}", e))?;

    // Watch target directory (non-recursive - we only care about direct children)
    // If target/debug doesn't exist yet, watch target/ to catch when it's created
    if config.target_dir.exists() {
        watcher
            .watch(&config.target_dir, RecursiveMode::NonRecursive)
            .map_err(|e| format!("Failed to watch {}: {}", config.target_dir.display(), e))?;
    } else if let Some(parent) = config.target_dir.parent() {
        if parent.exists() {
            // Watch parent (target/) to detect when target/debug is created
            let _ = watcher.watch(parent, RecursiveMode::NonRecursive);
        }
    }

    // Watch config files (or their parent directories if files don't exist yet)
    for config_file in &config.config_files {
        // Watch the parent directory since the file might be deleted and recreated
        if let Some(parent) = config_file.parent() {
            if parent.exists() {
                // Ignore errors for config files - they're optional
                let _ = watcher.watch(parent, RecursiveMode::NonRecursive);
            }
        }
    }

    Ok(WatcherHandle {
        events: rx,
        watcher,
    })
}

/// Categorize a changed path into a WatchEvent.
fn categorize_path(path: &Path, target_dir: &Path, config_files: &[PathBuf]) -> Option<WatchEvent> {
    // Check if it's a config file
    for config_file in config_files {
        if path == config_file {
            return Some(WatchEvent::ConfigChanged(path.to_path_buf()));
        }
    }

    // Check if it's in the target directory
    if path.starts_with(target_dir) {
        // Filter to binary-like files (no extension or known library extensions)
        if is_binary_or_library(path) {
            return Some(WatchEvent::BinaryChanged(path.to_path_buf()));
        }
    }

    None
}

/// Check if a path looks like a binary or library file.
fn is_binary_or_library(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    match extension.as_deref() {
        // Binary extensions (Windows)
        Some("exe") => true,
        // Library extensions
        Some("rlib") | Some("dylib") | Some("so") | Some("dll") | Some("a") => true,
        // dSYM debug symbols on macOS
        Some("dsym") => false, // Skip dSYM directories
        // No extension - likely a binary on Unix
        None => {
            // But skip common non-binary files
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            !name.starts_with('.') && !name.ends_with(".d") && name != "build" && name != "deps"
        }
        // Skip other extensions (like .d dependency files, .rmeta, etc.)
        _ => false,
    }
}

/// Spawn a debounced event processor.
///
/// This takes raw events and emits debounced analysis triggers.
/// Multiple changes within the debounce window result in a single trigger.
pub async fn debounced_events(
    mut events: mpsc::Receiver<WatchEvent>,
    debounce: Duration,
) -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel(10);

    tokio::spawn(async move {
        let mut pending = false;
        let mut debounce_timer: Option<tokio::time::Instant> = None;

        loop {
            let timeout = debounce_timer
                .map(|t| t.saturating_duration_since(tokio::time::Instant::now()))
                .unwrap_or(Duration::from_secs(3600)); // Long timeout when no pending

            tokio::select! {
                event = events.recv() => {
                    match event {
                        Some(WatchEvent::BinaryChanged(_)) | Some(WatchEvent::ConfigChanged(_)) => {
                            pending = true;
                            debounce_timer = Some(tokio::time::Instant::now() + debounce);
                        }
                        Some(WatchEvent::Error(e)) => {
                            eprintln!("File watcher error: {}", e);
                        }
                        None => break, // Channel closed
                    }
                }
                _ = tokio::time::sleep(timeout), if pending => {
                    pending = false;
                    debounce_timer = None;
                    // Emit analysis trigger
                    if tx.send(()).await.is_err() {
                        break; // Receiver dropped
                    }
                }
            }
        }
    });

    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test binary detection for all supported file types.
    /// See module docs for the full list of monitored file types.
    #[test]
    fn test_is_binary_or_library() {
        // === BINARY EXECUTABLES (no extension on Unix) ===
        assert!(
            is_binary_or_library(Path::new("target/debug/myapp")),
            "Binary without extension should trigger"
        );
        assert!(
            is_binary_or_library(Path::new("target/debug/jonesy")),
            "Binary without extension should trigger"
        );
        assert!(
            is_binary_or_library(Path::new("target/debug/my-app-name")),
            "Binary with hyphens should trigger"
        );

        // === RUST LIBRARIES (.rlib) ===
        assert!(
            is_binary_or_library(Path::new("target/debug/libfoo.rlib")),
            "Rust library (.rlib) should trigger"
        );
        assert!(
            is_binary_or_library(Path::new("target/debug/libjonesy.rlib")),
            "Rust library (.rlib) should trigger"
        );

        // === DYNAMIC LIBRARIES (macOS: .dylib) ===
        assert!(
            is_binary_or_library(Path::new("target/debug/libfoo.dylib")),
            "Dynamic library (.dylib) should trigger"
        );

        // === DYNAMIC LIBRARIES (Linux: .so) ===
        assert!(
            is_binary_or_library(Path::new("target/debug/libfoo.so")),
            "Shared object (.so) should trigger"
        );

        // === DYNAMIC LIBRARIES (Windows: .dll) ===
        assert!(
            is_binary_or_library(Path::new("target/debug/foo.dll")),
            "DLL (.dll) should trigger"
        );

        // === STATIC LIBRARIES (.a) ===
        assert!(
            is_binary_or_library(Path::new("target/debug/libfoo.a")),
            "Static library (.a) should trigger"
        );

        // === FILES TO SKIP ===

        // Hidden files/directories
        assert!(
            !is_binary_or_library(Path::new("target/debug/.fingerprint")),
            "Hidden directories should be skipped"
        );
        assert!(
            !is_binary_or_library(Path::new("target/debug/.cargo-lock")),
            "Hidden files should be skipped"
        );

        // Cargo build directories
        assert!(
            !is_binary_or_library(Path::new("target/debug/deps")),
            "deps directory should be skipped"
        );
        assert!(
            !is_binary_or_library(Path::new("target/debug/build")),
            "build directory should be skipped"
        );

        // Dependency tracking files
        assert!(
            !is_binary_or_library(Path::new("target/debug/foo.d")),
            "Dependency files (.d) should be skipped"
        );

        // Rust metadata
        assert!(
            !is_binary_or_library(Path::new("target/debug/foo.rmeta")),
            "Rust metadata (.rmeta) should be skipped"
        );

        // dSYM bundles (macOS debug symbols)
        assert!(
            !is_binary_or_library(Path::new("target/debug/myapp.dSYM")),
            "dSYM bundles should be skipped"
        );

        // Other common non-binary files
        assert!(
            !is_binary_or_library(Path::new("target/debug/foo.o")),
            "Object files (.o) should be skipped"
        );
        assert!(
            !is_binary_or_library(Path::new("target/debug/foo.pdb")),
            "PDB files should be skipped"
        );
    }

    /// Test that paths are correctly categorized into watch events.
    #[test]
    fn test_categorize_path_binaries() {
        let target_dir = PathBuf::from("/project/target/debug");
        let config_files = vec![
            PathBuf::from("/project/jonesy.toml"),
            PathBuf::from("/project/Cargo.toml"),
        ];

        // Binary executable
        let evt = categorize_path(
            Path::new("/project/target/debug/myapp"),
            &target_dir,
            &config_files,
        );
        assert!(
            matches!(evt, Some(WatchEvent::BinaryChanged(_))),
            "Binary should trigger BinaryChanged"
        );

        // Rust library (.rlib)
        let evt = categorize_path(
            Path::new("/project/target/debug/libfoo.rlib"),
            &target_dir,
            &config_files,
        );
        assert!(
            matches!(evt, Some(WatchEvent::BinaryChanged(_))),
            "Rust library should trigger BinaryChanged"
        );

        // Dynamic library (.dylib)
        let evt = categorize_path(
            Path::new("/project/target/debug/libfoo.dylib"),
            &target_dir,
            &config_files,
        );
        assert!(
            matches!(evt, Some(WatchEvent::BinaryChanged(_))),
            "Dynamic library should trigger BinaryChanged"
        );

        // Dynamic library (.so)
        let evt = categorize_path(
            Path::new("/project/target/debug/libfoo.so"),
            &target_dir,
            &config_files,
        );
        assert!(
            matches!(evt, Some(WatchEvent::BinaryChanged(_))),
            "Shared object should trigger BinaryChanged"
        );

        // Static library (.a)
        let evt = categorize_path(
            Path::new("/project/target/debug/libfoo.a"),
            &target_dir,
            &config_files,
        );
        assert!(
            matches!(evt, Some(WatchEvent::BinaryChanged(_))),
            "Static library should trigger BinaryChanged"
        );
    }

    /// Test that config files are correctly categorized.
    #[test]
    fn test_categorize_path_config_files() {
        let target_dir = PathBuf::from("/project/target/debug");
        let config_files = vec![
            PathBuf::from("/project/jonesy.toml"),
            PathBuf::from("/project/Cargo.toml"),
            PathBuf::from("/project/crate_a/Cargo.toml"),
        ];

        // jonesy.toml
        let evt = categorize_path(
            Path::new("/project/jonesy.toml"),
            &target_dir,
            &config_files,
        );
        assert!(
            matches!(evt, Some(WatchEvent::ConfigChanged(_))),
            "jonesy.toml should trigger ConfigChanged"
        );

        // Workspace Cargo.toml
        let evt = categorize_path(Path::new("/project/Cargo.toml"), &target_dir, &config_files);
        assert!(
            matches!(evt, Some(WatchEvent::ConfigChanged(_))),
            "Workspace Cargo.toml should trigger ConfigChanged"
        );

        // Member Cargo.toml
        let evt = categorize_path(
            Path::new("/project/crate_a/Cargo.toml"),
            &target_dir,
            &config_files,
        );
        assert!(
            matches!(evt, Some(WatchEvent::ConfigChanged(_))),
            "Member Cargo.toml should trigger ConfigChanged"
        );
    }

    #[test]
    fn test_watcher_config_default() {
        let config = WatcherConfig::default();
        assert_eq!(config.target_dir, PathBuf::from("target/debug"));
        assert!(config.config_files.is_empty());
        assert_eq!(config.debounce, Duration::from_millis(500));
    }

    #[test]
    fn test_start_watching_with_real_directory() {
        let temp_dir = std::env::temp_dir().join("jonesy_test_watcher");
        let _ = std::fs::create_dir_all(&temp_dir);

        let config = WatcherConfig {
            target_dir: temp_dir.clone(),
            config_files: vec![],
            debounce: Duration::from_millis(100),
        };

        let result = start_watching(config);
        assert!(
            result.is_ok(),
            "Should create watcher for existing directory"
        );

        // Drop the handle to stop watching
        drop(result);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_start_watching_nonexistent_target_dir() {
        let config = WatcherConfig {
            target_dir: PathBuf::from("/nonexistent/target/debug"),
            config_files: vec![],
            debounce: Duration::from_millis(100),
        };

        // Should not error — it watches the parent if target doesn't exist
        let _result = start_watching(config);
    }

    #[test]
    fn test_start_watching_with_config_files() {
        let temp_dir = std::env::temp_dir().join("jonesy_test_watcher_config");
        let _ = std::fs::create_dir_all(&temp_dir);

        let config_file = temp_dir.join("jonesy.toml");
        std::fs::write(&config_file, "# test").unwrap();

        let config = WatcherConfig {
            target_dir: temp_dir.clone(),
            config_files: vec![config_file],
            debounce: Duration::from_millis(100),
        };

        let result = start_watching(config);
        assert!(result.is_ok());

        drop(result);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_is_binary_or_library_exe() {
        assert!(is_binary_or_library(Path::new("target/debug/foo.exe")));
    }

    /// Test that unrelated files are ignored.
    #[test]
    fn test_categorize_path_unrelated() {
        let target_dir = PathBuf::from("/project/target/debug");
        let config_files = vec![PathBuf::from("/project/jonesy.toml")];

        // Source files
        let evt = categorize_path(
            Path::new("/project/src/main.rs"),
            &target_dir,
            &config_files,
        );
        assert!(evt.is_none(), "Source files should be ignored");

        // Files outside target directory
        let evt = categorize_path(
            Path::new("/other/project/target/debug/myapp"),
            &target_dir,
            &config_files,
        );
        assert!(
            evt.is_none(),
            "Files outside watched target should be ignored"
        );

        // Non-watched config files
        let evt = categorize_path(
            Path::new("/project/rustfmt.toml"),
            &target_dir,
            &config_files,
        );
        assert!(evt.is_none(), "Non-jonesy config files should be ignored");

        // Files in target that aren't binaries
        let evt = categorize_path(
            Path::new("/project/target/debug/foo.d"),
            &target_dir,
            &config_files,
        );
        assert!(
            evt.is_none(),
            "Dependency files in target should be ignored"
        );
    }
}
