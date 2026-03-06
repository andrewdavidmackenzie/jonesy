//! Integration tests for Jones
//!
//! These tests verify that Jones correctly identifies panic points in example crates
//! by comparing the output against `// jones: expect panic -` comments in source files.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    visit_rust_files(src_dir, &mut |file_path| {
        let content = fs::read_to_string(file_path).unwrap();

        for (i, line) in content.lines().enumerate() {
            if line.trim().starts_with(PANIC_MARKER) {
                // Get relative path from workspace root for matching
                let rel_path = file_path
                    .strip_prefix(find_workspace_root())
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

/// Run jones on an example and parse the output
fn run_jones_on_example(example_dir: &Path) -> HashSet<PanicPoint> {
    let workspace_root = find_workspace_root();
    let jones_binary = workspace_root.join("target/debug/jones");

    // Run jones from the example directory
    let output = Command::new(&jones_binary)
        .current_dir(example_dir)
        .output()
        .expect("Failed to run jones");

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_jones_output(&stdout)
}

/// Parse jones output to extract panic points
/// Output format: "  examples/panic/src/main.rs:9 in 'main'"
fn parse_jones_output(output: &str) -> HashSet<PanicPoint> {
    let mut points = HashSet::new();

    for line in output.lines() {
        let line = line.trim();
        // Look for lines like "examples/panic/src/main.rs:9 in 'main'"
        if line.contains(":")
            && line.contains(" in '")
            && let Some(file_line) = line.split(" in '").next()
            && let Some((file, line_str)) = file_line.rsplit_once(':')
            && let Ok(line_num) = line_str.parse::<u32>()
        {
            points.insert(PanicPoint {
                file: file.to_string(),
                line: line_num,
            });
        }
    }

    points
}

/// Build the jones binary and all examples
fn setup() {
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
}

/// Test a specific example
fn test_example(example_name: &str) {
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join(example_name);
    let src_dir = example_dir.join("src");

    // Get expected panic markers from source comments
    let markers = find_expected_panic_markers(&src_dir);

    // Run jones and get detected panics
    let detected = run_jones_on_example(&example_dir);

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
    let detected = run_jones_on_example(&example_dir);

    assert!(
        markers.is_empty(),
        "Perfect example should have no expected panic markers"
    );
    assert!(
        detected.is_empty(),
        "Perfect example should have no detected panics"
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
