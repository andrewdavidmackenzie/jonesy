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
/// Can optionally specify expected cause: "// jonesy: expect panic(unwrap)"
const PANIC_MARKER: &str = "// jonesy: expect panic";

/// Represents a panic point location with causes
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PanicPoint {
    file: String,
    line: u32,
    /// The panic cause identifiers (e.g., "unwrap", "overflow", "bounds")
    /// A single code point may have multiple causes when different panic paths converge
    causes: Vec<String>,
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

/// Expected panic marker with file, line, and optional cause
#[derive(Debug, Clone)]
struct ExpectedMarker {
    file: String,
    line: u32,
    /// Expected cause identifier (e.g., "unwrap", "overflow")
    /// None means any cause is acceptable
    cause: Option<String>,
}

/// Parse expected panic marker locations from source files in a directory
/// Supports both "// jonesy: expect panic" and "// jonesy: expect panic(cause)"
fn find_expected_panic_markers(src_dir: &Path) -> Vec<ExpectedMarker> {
    let mut markers = Vec::new();
    let workspace_root = find_workspace_root();
    visit_rust_files(src_dir, &mut |file_path| {
        let content = fs::read_to_string(file_path).unwrap();

        for (i, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with(PANIC_MARKER) {
                // Get the relative path from the workspace root for matching
                let rel_path = file_path
                    .strip_prefix(&workspace_root)
                    .unwrap_or(file_path)
                    .to_string_lossy()
                    .to_string();

                // Parse optional cause: "// jonesy: expect panic(cause)"
                let cause = if let Some(rest) = trimmed.strip_prefix(PANIC_MARKER) {
                    let rest = rest.trim();
                    if rest.starts_with('(') && rest.contains(')') {
                        let cause_str = rest
                            .trim_start_matches('(')
                            .split(')')
                            .next()
                            .unwrap_or("")
                            .trim();
                        if !cause_str.is_empty() {
                            Some(cause_str.to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Store the line where the comment is (1-indexed)
                markers.push(ExpectedMarker {
                    file: rel_path,
                    line: (i + 1) as u32,
                    cause,
                });
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
/// If the marker specifies a cause, it must match the detected cause
fn has_nearby_marker(detected: &PanicPoint, markers: &[ExpectedMarker]) -> bool {
    markers.iter().any(|marker| {
        let location_matches = paths_match(&detected.file, &marker.file)
            && (detected.line >= marker.line && detected.line <= marker.line + 2);

        if !location_matches {
            return false;
        }

        // If marker specifies a cause, it must be in the detected causes
        match &marker.cause {
            Some(expected) => detected.causes.contains(expected),
            None => true, // No expected cause - any cause OK
        }
    })
}

/// Check if a marker has a nearby detected panic with matching cause
fn has_nearby_detection(marker: &ExpectedMarker, detected: &HashSet<PanicPoint>) -> bool {
    detected.iter().any(|p| {
        let location_matches = paths_match(&p.file, &marker.file)
            && (p.line >= marker.line && p.line <= marker.line + 2);

        if !location_matches {
            return false;
        }

        // If marker specifies a cause, it must be in the detected causes
        match &marker.cause {
            Some(expected) => p.causes.contains(expected),
            None => true, // No expected cause - any cause OK
        }
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

/// Run jonesy and return raw stdout (with timeout protection)
fn run_jonesy_raw_output(example_dir: &Path, extra_args: &[&str]) -> String {
    let workspace_root = find_workspace_root();
    let jonesy_binary = std::env::var_os("CARGO_BIN_EXE_jonesy")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            workspace_root
                .join("target")
                .join("debug")
                .join(format!("jonesy{}", std::env::consts::EXE_SUFFIX))
        });

    let mut child = Command::new(&jonesy_binary)
        .args(extra_args)
        .current_dir(example_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn jonesy");

    match child.wait_timeout(JONES_TIMEOUT).expect("Failed to wait") {
        Some(_status) => {
            let output = child.wait_with_output().expect("Failed to get output");
            String::from_utf8_lossy(&output.stdout).to_string()
        }
        None => {
            child.kill().expect("Failed to kill timed-out process");
            let _ = child.wait();
            panic!(
                "Jonesy timed out after {:?} on {}",
                JONES_TIMEOUT,
                example_dir.display()
            );
        }
    }
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
            // Extract all causes from square brackets: "[JP001: desc] [JP006: desc]"
            let mut causes = Vec::new();
            let mut remaining = location;
            while let Some(bracket_start) = remaining.find('[') {
                if let Some(bracket_end) = remaining[bracket_start..].find(']') {
                    let cause_desc = &remaining[bracket_start + 1..bracket_start + bracket_end];
                    // Map description to cause ID
                    causes.push(description_to_cause_id(cause_desc));
                    remaining = &remaining[bracket_start + bracket_end + 1..];
                } else {
                    break;
                }
            }

            // Strip the cause description for parsing file:line:col
            let location = location.split('[').next().unwrap_or(location).trim();

            // Parse file:line:column format
            let parts: Vec<&str> = location.rsplitn(3, ':').collect();
            if parts.len() >= 2
                && let Ok(line_num) = parts[1].parse::<u32>()
            {
                // parts[0] is the column, parts[1] is the line, parts[2] is the file path
                let file = if parts.len() > 2 { parts[2] } else { "" };
                points.insert(PanicPoint {
                    file: file.to_string(),
                    line: line_num,
                    causes,
                });
            }
        }
    }

    points
}

/// Map a cause description (from output) to the cause ID (used in markers)
fn description_to_cause_id(description: &str) -> String {
    // Handle exact matches from jonesy output
    if description.contains("unwrap()") && description.contains("None") {
        return "unwrap".to_string();
    }
    if description.contains("unwrap()") && description.contains("Err") {
        return "unwrap".to_string();
    }
    if description.contains("expect()") && description.contains("None") {
        return "expect".to_string();
    }
    if description.contains("expect()") && description.contains("Err") {
        return "expect".to_string();
    }
    if description.contains("panic!()") {
        return "panic".to_string();
    }
    if description.contains("arithmetic overflow") {
        return "overflow".to_string();
    }
    if description.contains("shift overflow") {
        return "overflow".to_string();
    }
    if description.contains("index out of bounds") {
        return "bounds".to_string();
    }
    if description.contains("division by zero") || description.contains("remainder by zero") {
        return "div_zero".to_string();
    }
    if description.contains("assertion failed") && !description.contains("debug") {
        return "assert".to_string();
    }
    if description.contains("debug assertion") || description.contains("debug_assert") {
        return "debug_assert".to_string();
    }
    if description.contains("unreachable") {
        return "unreachable".to_string();
    }
    if description.contains("unimplemented") {
        return "unimplemented".to_string();
    }
    if description.contains("todo") {
        return "todo".to_string();
    }
    if description.contains("capacity overflow") {
        return "capacity".to_string();
    }
    if description.contains("formatting error") {
        return "format".to_string();
    }
    if description.contains("invalid enum") {
        return "invalid_enum".to_string();
    }
    if description.contains("misaligned pointer") {
        return "misaligned_ptr".to_string();
    }
    if description.contains("panic during drop") || description.contains("panic in drop") {
        return "drop".to_string();
    }
    if description.contains("no-unwind") || description.contains("cannot unwind") {
        return "unwind".to_string();
    }
    if description.contains("out of memory") {
        return "oom".to_string();
    }
    if description.contains("string") && description.contains("slice") {
        return "str_slice".to_string();
    }
    if description.contains("unknown cause") {
        return "unknown".to_string();
    }
    // Fallback: normalize to lowercase with underscores
    description.to_lowercase().replace(' ', "_")
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
        for m in &missing {
            if let Some(cause) = &m.cause {
                eprintln!("  {}:{} [expected: {}]", m.file, m.line, cause);
            } else {
                eprintln!("  {}:{}", m.file, m.line);
            }
        }
    }

    if !unexpected.is_empty() {
        eprintln!("Unexpected panic points (detected but no marker):");
        for p in &unexpected {
            if p.causes.is_empty() {
                eprintln!("  {}:{} [NO CAUSE DETECTED]", p.file, p.line);
            } else {
                eprintln!("  {}:{} [{}]", p.file, p.line, p.causes.join(", "));
            }
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

/// Test staticlib analysis with DCE (dead code elimination).
///
/// Static libraries are designed for C FFI. Only `#[no_mangle]` functions are
/// preserved - other functions are eliminated by DCE since C code cannot call
/// mangled Rust symbols.
///
/// This test verifies:
/// 1. The `#[no_mangle]` exported function's panic IS detected
/// 2. The internal function's panic is NOT detected (eliminated by DCE)
#[test]
fn test_staticlib_example() {
    setup();
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("staticlib");

    // Run jonesy on the staticlib
    let stdout = run_jonesy_raw_output(&example_dir, &["--no-hyperlinks", "--lib"]);
    let detected = parse_jones_output(&stdout);

    // Should detect exactly 1 panic: the exported_function's panic at line 17
    let exported_panic = detected
        .iter()
        .any(|p| p.file.ends_with("lib.rs") && (16..=18).contains(&p.line));

    assert!(
        exported_panic,
        "Staticlib should detect panic in #[no_mangle] exported_function.\n\
         Detected: {:?}\n\
         Output:\n{}",
        detected, stdout
    );

    // Should NOT detect internal_function's panic (eliminated by DCE)
    // internal_function's panic is at line 26
    let internal_panic = detected
        .iter()
        .any(|p| p.file.ends_with("lib.rs") && (25..=27).contains(&p.line));

    assert!(
        !internal_panic,
        "Staticlib should NOT detect panic in internal function (DCE eliminates it).\n\
         Detected: {:?}",
        detected
    );

    // Verify exactly 1 panic point total
    assert_eq!(
        detected.len(),
        1,
        "Staticlib should have exactly 1 panic point (only the exported function).\n\
         Detected: {:?}",
        detected
    );
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
        for m in &missing {
            if let Some(cause) = &m.cause {
                eprintln!("  {}:{} [expected: {}]", m.file, m.line, cause);
            } else {
                eprintln!("  {}:{}", m.file, m.line);
            }
        }
    }

    if !unexpected.is_empty() {
        eprintln!("Unexpected panic points (detected but no marker):");
        for p in &unexpected {
            if p.causes.is_empty() {
                eprintln!("  {}:{} [NO CAUSE DETECTED]", p.file, p.line);
            } else {
                eprintln!("  {}:{} [{}]", p.file, p.line, p.causes.join(", "));
            }
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

/// Test scoped rules: allow explicit panic! in main.rs
/// The config allows "panic" cause in **/main.rs, so the direct panic!() call
/// at main.rs:9 should be filtered, reducing the total count.
#[test]
fn test_scoped_rules() {
    setup();
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("panic");
    let config_path = workspace_root
        .join("jonesy")
        .join("tests")
        .join("test_scoped_rules.toml");

    // Run without config to get baseline
    let (baseline_exit_code, baseline_detected) = run_jones_on_example(&example_dir);

    // Verify baseline actually detects the explicit panic at main.rs:9
    assert!(
        baseline_detected
            .iter()
            .any(|p| p.file.contains("main.rs") && p.line == 9),
        "Baseline should include explicit panic!() at main.rs:9"
    );

    // Run with scoped config that allows "panic" cause in **/main.rs
    let (scoped_exit_code, scoped_detected) = run_jonesy_with_config(&example_dir, &config_path);

    // The scoped config should result in fewer detected panics
    // (the explicit panic!() at main.rs:9 should be filtered)
    assert!(
        scoped_exit_code < baseline_exit_code,
        "Scoped rules should filter some panics: baseline={}, with_scoped_rules={}",
        baseline_exit_code,
        scoped_exit_code
    );

    // Scoped-filtered panics should be a subset of baseline panics
    assert!(
        scoped_detected.is_subset(&baseline_detected),
        "Scoped-filtered panics should be a subset of baseline panics"
    );

    // Verify that main.rs:9 (the explicit panic!) is NOT in the filtered results
    let explicit_panic_filtered = !scoped_detected
        .iter()
        .any(|p| p.file.contains("main.rs") && p.line == 9);
    assert!(
        explicit_panic_filtered,
        "Scoped rule should filter explicit panic!() at main.rs:9"
    );

    // Other panic types in main.rs should still be reported
    let other_main_panics: Vec<_> = scoped_detected
        .iter()
        .filter(|p| p.file.contains("main.rs") && p.line != 9)
        .collect();
    assert!(
        !other_main_panics.is_empty(),
        "Other panic types in main.rs should still be detected"
    );
}

/// Test that library analysis provides precise line numbers with column info.
/// This verifies issue #66 fix - expect() calls should show precise column numbers.
#[test]
fn test_rlib_line_precision() {
    setup();
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("rlib");

    // Run jonesy and get raw output (setup() already built the example)
    let stdout = run_jonesy_raw_output(&example_dir, &["--no-hyperlinks", "--lib"]);

    // Check that expect() calls have column precision (column > 1)
    // The expect_none function has `.expect(` starting at column 17 on line 21
    // The expect_err function has `.expect(` starting at column 25 on line 27
    // We verify that at least one panic point has a non-trivial column number

    let has_column_precision = stdout.lines().any(|line| {
        // Look for lines like "mod.rs:21:22" or "mod.rs:27:30"
        // where the column (last number) is > 1
        if line.contains("mod.rs:") && line.contains("[") {
            // Extract file:line:col pattern
            if let Some(loc_start) = line.find("mod.rs:") {
                let loc_part = &line[loc_start..];
                // Parse the colon-separated parts
                let parts: Vec<&str> = loc_part
                    .split('[')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .split(':')
                    .collect();
                if parts.len() >= 3 {
                    if let Ok(col) = parts[2].parse::<u32>() {
                        return col > 1;
                    }
                }
            }
        }
        false
    });

    assert!(
        has_column_precision,
        "Library analysis should provide column precision for some panic points.\n\
         Expected lines like 'mod.rs:21:22' with column > 1.\n\
         Output:\n{}",
        stdout
    );
}

/// Test that todo!() macro is detected in library analysis (issue #58).
/// This verifies that functions at address 0x0 in relocatable objects are not filtered out.
#[test]
fn test_rlib_todo_detection() {
    setup();
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("rlib");

    // Run jonesy and get raw output
    let stdout = run_jonesy_raw_output(&example_dir, &["--no-hyperlinks", "--lib"]);

    // The todo!() call is in cause_todo() at line 88 of mod.rs
    // We should detect this panic point
    let todo_detected = stdout.lines().any(|line| {
        // Look for mod.rs:88 (the line with todo!())
        line.contains("mod.rs:88:") || line.contains("mod.rs:87:") || line.contains("mod.rs:89:")
    });

    assert!(
        todo_detected,
        "Library analysis should detect todo!() macro panic (issue #58).\n\
         Expected a panic point at mod.rs:88 (±1 line).\n\
         Output:\n{}",
        stdout
    );
}

/// Test that conditional panics are detected in library analysis (issue #57).
/// This verifies that panic!() calls inside conditionals (if blocks) are detected.
#[test]
fn test_rlib_conditional_panic_detection() {
    setup();
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("rlib");

    // Run jonesy and get raw output
    let stdout = run_jonesy_raw_output(&example_dir, &["--no-hyperlinks", "--lib"]);

    // The conditional panic is in library_function() at line 12 of lib.rs:
    //   if std::env::args().len() > 1 {
    //       panic!("{}", PANIC_STR);  // line 12
    //   }
    // Use parse_jones_output for robust matching (not substring-based)
    let detected = parse_jones_output(&stdout);
    let conditional_panic_detected = detected
        .iter()
        .any(|p| p.file.ends_with("lib.rs") && (11..=13).contains(&p.line));

    assert!(
        conditional_panic_detected,
        "Library analysis should detect conditional panic!() calls (issue #57).\n\
         Expected a panic point at lib.rs:12 (±1 line).\n\
         Detected panic points: {:?}\n\
         Output:\n{}",
        detected, stdout
    );
}

/// Test that inlined functions report the correct function name.
/// When a function is inlined into main(), the panic point should still
/// report the original function name, not "main".
#[test]
fn test_inlined_function_names() {
    setup();
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("inlined");

    // Build the example
    let build_status = Command::new("cargo")
        .args(["build"])
        .current_dir(&example_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("Failed to build inlined example");
    assert!(build_status.success(), "Failed to build inlined example");

    // Run jonesy with JSON output
    let stdout = run_jonesy_raw_output(&example_dir, &["--format", "json"]);

    // Parse JSON and check function names
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("Failed to parse JSON output");

    let panic_points = json["panic_points"]
        .as_array()
        .expect("Expected panic_points array");

    // Verify we have the expected panic points
    assert!(
        panic_points.len() >= 2,
        "Expected at least 2 panic points, got {}",
        panic_points.len()
    );

    // Check that function names are the inlined function names, not "main"
    let function_names: Vec<&str> = panic_points
        .iter()
        .filter_map(|p| p["function"].as_str())
        .collect();

    // The functions should be "inlined::run" and "inlined::helper", not "main"
    assert!(
        function_names.iter().any(|n| n.contains("run")),
        "Expected a function name containing 'run', got: {:?}",
        function_names
    );
    assert!(
        function_names.iter().any(|n| n.contains("helper")),
        "Expected a function name containing 'helper', got: {:?}",
        function_names
    );
    assert!(
        !function_names.iter().any(|n| *n == "main"),
        "Function names should be the inlined function names, not 'main': {:?}",
        function_names
    );
}

/// Test that indirect panic messages include the called function name (issue #125).
/// For indirect panics, the help message should show "This calls `func_name` which may..."
#[test]
fn test_indirect_panic_shows_called_function() {
    setup();
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("panic");

    // Run jonesy and get raw output with text format
    let stdout = run_jonesy_raw_output(&example_dir, &["--no-hyperlinks"]);

    // Check that indirect panics show the called function name in help messages
    // The panic example has indirect panics that call functions like print_to
    // The help message should contain "This calls `" followed by a function name

    // Look for the pattern "This calls `<name>`" in the output
    let has_called_function_message = stdout.contains("This calls `")
        && stdout
            .lines()
            .any(|line| line.contains("= help: This calls `") && line.contains("` which"));

    assert!(
        has_called_function_message,
        "Indirect panic help messages should include the called function name.\n\
         Expected pattern like: 'This calls `func_name` which may...'\n\
         Output:\n{}",
        stdout
    );

    // Also verify with JSON output that the suggestion field contains the function name
    let json_stdout = run_jonesy_raw_output(&example_dir, &["--format", "json"]);
    let json: serde_json::Value =
        serde_json::from_str(&json_stdout).expect("Failed to parse JSON output");

    let panic_points = json["panic_points"]
        .as_array()
        .expect("Expected panic_points array");

    // Find at least one cause with a suggestion containing "This calls `"
    let has_function_in_json = panic_points.iter().any(|p| {
        p["causes"]
            .as_array()
            .map(|causes| {
                causes.iter().any(|c| {
                    c["suggestion"]
                        .as_str()
                        .map(|s| s.contains("This calls `"))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    });

    assert!(
        has_function_in_json,
        "JSON output should include called function name in suggestion field.\n\
         Expected suggestion containing: 'This calls `func_name`...'"
    );
}
