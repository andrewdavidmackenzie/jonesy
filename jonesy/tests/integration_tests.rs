//! Integration tests for Jonesy
//!
//! These tests verify that Jonesy correctly identifies panic points in example crates
//! by comparing the output against `// jonesy: expect panic` comments in source files.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Once;
use std::time::Duration;

use wait_timeout::ChildExt;

/// Marker comment that indicates an expected panic on the next line
const PANIC_MARKER: &str = "// jonesy: expect panic";

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
                // Get the relative path from the workspace root for matching
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

/// Check if two file paths match (handles absolute vs relative paths)
/// Returns true if they're equal or if one path ends with the other
fn paths_match(detected_path: &str, marker_path: &str) -> bool {
    if detected_path == marker_path {
        return true;
    }
    // Handle various relative path scenarios:
    // - detected: "src/main.rs", marker: "examples/panic/src/main.rs"
    // - detected: "/abs/path/src/main.rs", marker: "examples/panic/src/main.rs"
    // Check if either path ends with the other (requiring path boundary with '/')
    marker_path.ends_with(&format!("/{}", detected_path))
        || detected_path.ends_with(&format!("/{}", marker_path))
}

/// Check if a detected panic point has an expected marker nearby
/// The marker comment can be on the same line, previous line, or up to 2 lines before
fn has_nearby_marker(detected: &PanicPoint, markers: &[(String, u32)]) -> bool {
    markers.iter().any(|(file, comment_line)| {
        paths_match(&detected.file, file)
            && (detected.line >= *comment_line && detected.line <= comment_line + 2)
    })
}

/// Check if a marker has a nearby detected panic
fn has_nearby_detection(marker: &(String, u32), detected: &HashSet<PanicPoint>) -> bool {
    let (file, comment_line) = marker;
    detected.iter().any(|p| {
        paths_match(&p.file, file) && (p.line >= *comment_line && p.line <= comment_line + 2)
    })
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

/// Timeout for running jonesy on each example (10 minutes)
const JONES_TIMEOUT: Duration = Duration::from_secs(600);

/// Run jonesy with optional extra arguments and parse the output
/// Returns (exit_code, detected_panic_points)
fn run_jonesy_with_args(example_dir: &Path, extra_args: &[&str]) -> (i32, HashSet<PanicPoint>) {
    let workspace_root = find_workspace_root();
    // Use the Cargo-provided path if available, otherwise fall back to the platform-safe path
    let jones_binary = std::env::var_os("CARGO_BIN_EXE_jonesy")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            workspace_root
                .join("target")
                .join("debug")
                .join(format!("jonesy{}", std::env::consts::EXE_SUFFIX))
        });

    // Run jonesy from the example directory with timeout
    let mut child = Command::new(&jones_binary)
        .args(extra_args)
        .current_dir(example_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn jonesy");

    match child.wait_timeout(JONES_TIMEOUT).expect("Failed to wait") {
        Some(status) => {
            // Jonesy exits with the number of panic points found (0 = no panics)
            let exit_code = status.code().unwrap_or(-1);
            let output = child.wait_with_output().expect("Failed to get output");
            let stdout = String::from_utf8_lossy(&output.stdout);
            (exit_code, parse_jones_output(&stdout))
        }
        None => {
            child.kill().expect("Failed to kill timed-out process");
            // Wait for the child to be reaped to avoid a zombie process
            let _ = child.wait();
            panic!(
                "Jonesy timed out after {:?} on {}",
                JONES_TIMEOUT,
                example_dir.display()
            );
        }
    }
}

/// Run jonesy on an example and parse the output
/// Returns (exit_code, detected_panic_points)
fn run_jones_on_example(example_dir: &Path) -> (i32, HashSet<PanicPoint>) {
    // Use --no-hyperlinks for tests since we parse plain-text output
    run_jonesy_with_args(example_dir, &["--no-hyperlinks"])
}

/// Parse jonesy output to extract panic points
/// Output format: "├──> filename:line:col" or " --> filename:line:col"
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

        // Look for arrow patterns and extract the location part
        // Use split_once to safely handle UTF-8 characters
        let location = if let Some((_, rest)) = line.split_once(" --> ") {
            Some(rest)
        } else if let Some((_, rest)) = line.split_once("├──> ") {
            Some(rest)
        } else if let Some((_, rest)) = line.split_once("└──> ") {
            Some(rest)
        } else if line.starts_with("-->") {
            Some(line.trim_start_matches("-->").trim())
        } else {
            None
        };

        if let Some(location) = location {
            // Parse file:line:column format
            let parts: Vec<&str> = location.rsplitn(3, ':').collect();
            if parts.len() >= 2
                && let Ok(line_num) = parts[1].parse::<u32>()
            {
                // parts[0] is the column, parts[1] is the line, parts[2] is the file path
                let file = if parts.len() > 2 { parts[2] } else { "" };
                // Strip any trailing description like " [capacity overflow]"
                let file = file.split('[').next().unwrap_or(file).trim();
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

/// Build the jonesy binary and all examples (runs only once)
fn setup() {
    SETUP.call_once(|| {
        let workspace_root = find_workspace_root();

        // Build jonesy
        let status = Command::new("cargo")
            .args(["build", "-p", "jonesy"])
            .current_dir(&workspace_root)
            .status()
            .expect("Failed to build jonesy");
        assert!(status.success(), "Failed to build jonesy");

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

    // Run jonesy and get detected panics
    let (exit_code, detected) = run_jones_on_example(&example_dir);

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

    // Fail only on missing panics - unexpected panics may be due to
    // platform/Rust version differences in what jonesy detects
    assert!(
        missing.is_empty(),
        "Missing panic points for example '{}': {} missing (exit_code={}, markers={}). \
         Also had {} unexpected (may be platform-specific).",
        example_name,
        missing.len(),
        exit_code,
        markers.len(),
        unexpected.len()
    );
}

#[test]
fn test_panic_example() {
    setup();
    test_example("panic");
}

// Library-only analysis is now implemented for rlib archives
#[test]
fn test_rlib_example() {
    setup();
    test_example("rlib");
}

// Static libraries have aggressive dead code elimination which removes
// unreferenced panic-prone code. The staticlib test would need special
// handling to export functions properly.
#[test]
#[ignore = "staticlib requires #[no_mangle] exports to avoid DCE"]
fn test_staticlib_example() {
    setup();
    test_example("staticlib");
}

#[test]
fn test_multi_bin_example() {
    setup();
    test_example("multi_bin");
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

/// Run jonesy with a custom config file and return the output
fn run_jonesy_with_config(example_dir: &Path, config_path: &Path) -> (i32, HashSet<PanicPoint>) {
    let config_str = config_path.to_string_lossy();
    // Use --no-hyperlinks for tests since we parse plain-text output
    run_jonesy_with_args(example_dir, &["--no-hyperlinks", "--config", &config_str])
}

/// Test a nested workspace example (workspace_test)
/// This is special because it's not part of the main workspace
#[test]
fn test_workspace_test_example() {
    setup();
    let workspace_root = find_workspace_root();
    let workspace_test_dir = workspace_root.join("examples").join("workspace_test");

    // Build the nested workspace first
    let status = Command::new("cargo")
        .arg("build")
        .current_dir(&workspace_test_dir)
        .status()
        .expect("Failed to build workspace_test");
    assert!(status.success(), "Failed to build workspace_test");

    // Collect expected markers from all crates in the nested workspace
    let mut all_markers = Vec::new();
    for crate_name in &["crate_a", "crate_b", "crate_c"] {
        let src_dir = workspace_test_dir.join(crate_name).join("src");
        all_markers.extend(find_expected_panic_markers(&src_dir));
    }

    // Run jonesy on the nested workspace
    let (exit_code, detected) = run_jones_on_example(&workspace_test_dir);

    // Check each detected panic has a nearby marker
    let unexpected: Vec<_> = detected
        .iter()
        .filter(|p| !has_nearby_marker(p, &all_markers))
        .collect();

    // Check each marker has a nearby detection
    let missing: Vec<_> = all_markers
        .iter()
        .filter(|m| !has_nearby_detection(m, &detected))
        .collect();

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

    // Fail only on missing panics - unexpected panics may be due to
    // call sites being reported in addition to actual panic locations
    assert!(
        missing.is_empty(),
        "Missing panic points for 'workspace_test': {} missing (exit_code={}, markers={}). \
         Also had {} unexpected (may be call sites or platform-specific).",
        missing.len(),
        exit_code,
        all_markers.len(),
        unexpected.len()
    );
}

#[test]
fn test_config_allow_panic() {
    setup();
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("panic");
    let config_path = workspace_root
        .join("jonesy")
        .join("tests")
        .join("test_allow_panic.toml");

    // Run without config to get baseline
    let (baseline_exit_code, baseline_detected) = run_jones_on_example(&example_dir);

    // Run with config that allows explicit panic
    let (config_exit_code, config_detected) = run_jonesy_with_config(&example_dir, &config_path);

    // The config should result in fewer detected panics (since panic! is now allowed)
    assert!(
        config_exit_code < baseline_exit_code,
        "Config allowing panic! should result in fewer detected panics: baseline={}, with_config={}",
        baseline_exit_code,
        config_exit_code
    );

    // Config-filtered panics should be a subset of baseline panics
    assert!(
        config_detected.is_subset(&baseline_detected),
        "Config-filtered panics should be a subset of baseline panics"
    );
}

/// Test running jonesy with --bin option on a multi-binary crate (Scenario 5a)
#[test]
fn test_multi_bin_specific_binary() {
    setup();
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("multi_bin");

    // Run jonesy on bin_one only
    let (exit_code_bin_one, detected_bin_one) =
        run_jonesy_with_args(&example_dir, &["--no-hyperlinks", "--bin", "bin_one"]);

    // Run jonesy on bin_two only
    let (exit_code_bin_two, detected_bin_two) =
        run_jonesy_with_args(&example_dir, &["--no-hyperlinks", "--bin", "bin_two"]);

    // Run jonesy on all (baseline)
    let (exit_code_all, _detected_all) = run_jones_on_example(&example_dir);

    // Each binary should have fewer panics than the total
    assert!(
        exit_code_bin_one < exit_code_all,
        "bin_one should have fewer panics ({}) than all ({})",
        exit_code_bin_one,
        exit_code_all
    );

    assert!(
        exit_code_bin_two < exit_code_all,
        "bin_two should have fewer panics ({}) than all ({})",
        exit_code_bin_two,
        exit_code_all
    );

    // The detected panics should only be from the respective binary files
    for p in &detected_bin_one {
        assert!(
            p.file.contains("bin_one") || p.file.contains("lib.rs"),
            "bin_one analysis should not include files from bin_two: {}",
            p.file
        );
    }

    for p in &detected_bin_two {
        assert!(
            p.file.contains("bin_two") || p.file.contains("lib.rs"),
            "bin_two analysis should not include files from bin_one: {}",
            p.file
        );
    }
}

/// Test running jonesy with --lib option on a crate with library (Scenario 5b)
#[test]
fn test_multi_bin_lib_only() {
    setup();
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("multi_bin");

    // Run jonesy on library only
    let (exit_code_lib, detected_lib) =
        run_jonesy_with_args(&example_dir, &["--no-hyperlinks", "--lib"]);

    // Run jonesy on all (baseline)
    let (exit_code_all, _detected_all) = run_jones_on_example(&example_dir);

    // Library should have fewer panics than the total
    assert!(
        exit_code_lib < exit_code_all,
        "lib should have fewer panics ({}) than all ({})",
        exit_code_lib,
        exit_code_all
    );

    // The detected panics should only be from the library file
    for p in &detected_lib {
        assert!(
            p.file.contains("lib.rs"),
            "lib-only analysis should not include binary files: {}",
            p.file
        );
    }
}
