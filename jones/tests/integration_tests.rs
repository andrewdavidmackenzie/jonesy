//! Integration tests for Jones
//!
//! These tests verify that Jones correctly identifies panic points in example crates
//! by comparing the output against `// jones: expect panic -` comments in source files.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Once;
use std::time::Duration;

use wait_timeout::ChildExt;

/// Marker comment that indicates an expected panic on the next line
const PANIC_MARKER: &str = "// jones: expect panic -";

/// Represents a panic point location
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PanicPoint {
    file: String,
    line: u32,
}

/// Find the workspace root by looking for Cargo.toml with [workspace]
fn find_workspace_root() -> PathBuf {
    let mut current = std::env::current_dir().unwrap();
    loop {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists() {
            let content = fs::read_to_string(&cargo_toml).unwrap_or_default();
            if content.contains("[workspace]") {
                return current;
            }
        }
        if !current.pop() {
            panic!("Could not find workspace root");
        }
    }
}

/// Parse expected panic marker locations from source files in a directory
/// Returns (file, comment_line) tuples where the comment marks an expected panic
fn find_expected_panic_markers(src_dir: &Path) -> Vec<(String, u32)> {
    let mut markers = Vec::new();
    let workspace_root = find_workspace_root();
    visit_rust_files(src_dir, &mut |file_path| {
        let content = fs::read_to_string(file_path).unwrap();

        for (i, line) in content.lines().enumerate() {
            if line.trim().starts_with(PANIC_MARKER) {
                // Get relative path from workspace root for matching
                let rel_path = file_path
                    .strip_prefix(&workspace_root)
                    .unwrap_or(file_path)
                    .to_string_lossy()
                    .to_string();

                // Store the line where the comment is (1-indexed)
                markers.push((rel_path, (i + 1) as u32));
            }
        }
    });
    markers
}

/// Check if a detected panic point has an expected marker nearby
/// The marker comment can be on the same line, previous line, or up to 2 lines before
fn has_nearby_marker(detected: &PanicPoint, markers: &[(String, u32)]) -> bool {
    markers.iter().any(|(file, comment_line)| {
        file == &detected.file
            && (detected.line >= *comment_line && detected.line <= comment_line + 2)
    })
}

/// Check if a marker has a nearby detected panic
fn has_nearby_detection(marker: &(String, u32), detected: &HashSet<PanicPoint>) -> bool {
    let (file, comment_line) = marker;
    detected
        .iter()
        .any(|p| &p.file == file && (p.line >= *comment_line && p.line <= comment_line + 2))
}

/// Recursively visit all .rs files in a directory
fn visit_rust_files<F>(dir: &Path, callback: &mut F)
where
    F: FnMut(&Path),
{
    if dir.is_dir() {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                visit_rust_files(&path, callback);
            } else if path.extension().is_some_and(|e| e == "rs") {
                callback(&path);
            }
        }
    }
}

/// Timeout for running jones on each example (10 minutes)
const JONES_TIMEOUT: Duration = Duration::from_secs(600);

/// Run jones on an example and parse the output
/// Returns (exit_code, detected_panic_points)
fn run_jones_on_example(example_dir: &Path) -> (i32, HashSet<PanicPoint>) {
    let workspace_root = find_workspace_root();
    // Use Cargo-provided path if available, otherwise fall back to platform-safe path
    let jones_binary = std::env::var_os("CARGO_BIN_EXE_jones")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            workspace_root
                .join("target")
                .join("debug")
                .join(format!("jones{}", std::env::consts::EXE_SUFFIX))
        });

    // Run jones from the example directory with timeout
    let mut child = Command::new(&jones_binary)
        .current_dir(example_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn jones");

    match child.wait_timeout(JONES_TIMEOUT).expect("Failed to wait") {
        Some(status) => {
            // Jones exits with the number of panic points found (0 = no panics)
            let exit_code = status.code().unwrap_or(-1);
            let output = child.wait_with_output().expect("Failed to get output");
            let stdout = String::from_utf8_lossy(&output.stdout);
            (exit_code, parse_jones_output(&stdout))
        }
        None => {
            child.kill().expect("Failed to kill timed-out process");
            // Wait for child to be reaped to avoid zombie process
            let _ = child.wait();
            panic!(
                "Jones timed out after {:?} on {}",
                JONES_TIMEOUT,
                example_dir.display()
            );
        }
    }
}

/// Parse jones output to extract panic points
/// Output format: "  examples/panic/src/main.rs:9 in 'main'"
fn parse_jones_output(output: &str) -> HashSet<PanicPoint> {
    let mut points = HashSet::new();

    for line in output.lines() {
        let line = line.trim();
        // Strip tree characters (├── └── │) from the start of the line
        let line = line
            .trim_start_matches("├── ")
            .trim_start_matches("└── ")
            .trim_start_matches("│   ")
            .trim_start_matches("│");

        // Look for lines with " --> " followed by file:line:column
        // Format: " --> examples/panic/src/main.rs:9:1" or "└──  --> path:line:col"
        if let Some(arrow_pos) = line.find(" --> ") {
            let location = &line[arrow_pos + 5..]; // Skip " --> "
            // Parse file:line:column format
            let parts: Vec<&str> = location.rsplitn(3, ':').collect();
            if parts.len() >= 2
                && let Ok(line_num) = parts[1].parse::<u32>()
            {
                // parts[0] is column, parts[1] is line, parts[2..] is file path
                let file = if parts.len() > 2 { parts[2] } else { "" };
                points.insert(PanicPoint {
                    file: file.to_string(),
                    line: line_num,
                });
            }
        } else if line.starts_with("-->") {
            // Handle " --> path:line:col" at start (after trim)
            let location = line.trim_start_matches("-->").trim();
            let parts: Vec<&str> = location.rsplitn(3, ':').collect();
            if parts.len() >= 2
                && let Ok(line_num) = parts[1].parse::<u32>()
            {
                let file = if parts.len() > 2 { parts[2] } else { "" };
                points.insert(PanicPoint {
                    file: file.to_string(),
                    line: line_num,
                });
            }
        }
    }

    points
}

/// One-time setup initialization
static SETUP: Once = Once::new();

/// Build the jones binary and all examples (runs only once)
fn setup() {
    SETUP.call_once(|| {
        let workspace_root = find_workspace_root();

        // Build jones
        let status = Command::new("cargo")
            .args(["build", "-p", "jones"])
            .current_dir(&workspace_root)
            .status()
            .expect("Failed to build jones");
        assert!(status.success(), "Failed to build jones");

        // Build all examples
        let status = Command::new("cargo")
            .arg("build")
            .current_dir(&workspace_root)
            .status()
            .expect("Failed to build examples");
        assert!(status.success(), "Failed to build examples");
    });
}

/// Test a specific example
fn test_example(example_name: &str) {
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join(example_name);
    let src_dir = example_dir.join("src");

    // Get expected panic markers from source comments
    let markers = find_expected_panic_markers(&src_dir);

    // Run jones and get detected panics
    let (exit_code, detected) = run_jones_on_example(&example_dir);

    // Verify exit code matches expected marker count
    let expected_count = markers.len() as i32;
    assert_eq!(
        exit_code, expected_count,
        "Exit code {} doesn't match expected panic count {} for '{}'",
        exit_code, expected_count, example_name
    );

    // Check each detected panic has a nearby marker
    let unexpected: Vec<_> = detected
        .iter()
        .filter(|p| !has_nearby_marker(p, &markers))
        .collect();

    // Check each marker has a nearby detection
    let missing: Vec<_> = markers
        .iter()
        .filter(|m| !has_nearby_detection(m, &detected))
        .collect();

    // Report results
    if !missing.is_empty() {
        eprintln!("Missing panic points (markers without detections):");
        for (file, line) in &missing {
            eprintln!("  {}:{}", file, line);
        }
    }

    if !unexpected.is_empty() {
        eprintln!("Unexpected panic points (detected but no marker):");
        for p in &unexpected {
            eprintln!("  {}:{}", p.file, p.line);
        }
    }

    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "Panic point mismatch for example '{}': {} missing, {} unexpected",
        example_name,
        missing.len(),
        unexpected.len()
    );
}

#[test]
fn test_panic_example() {
    setup();
    test_example("panic");
}

#[test]
fn test_array_access_example() {
    setup();
    test_example("array_access");
}

#[test]
fn test_oom_example() {
    setup();
    test_example("oom");
}

#[test]
fn test_perfect_example() {
    setup();
    // Perfect should have no panics and no expected markers
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("perfect");
    let src_dir = example_dir.join("src");

    let markers = find_expected_panic_markers(&src_dir);
    let (exit_code, detected) = run_jones_on_example(&example_dir);

    assert!(
        markers.is_empty(),
        "Perfect example should have no expected panic markers"
    );
    assert!(
        detected.is_empty(),
        "Perfect example should have no detected panics"
    );
    assert_eq!(
        exit_code, 0,
        "Perfect example should exit with 0 (no panics)"
    );
}

#[test]
fn test_cdylib_example() {
    setup();
    test_example("cdylib");
}

#[test]
fn test_dylib_example() {
    setup();
    test_example("dylib");
}
