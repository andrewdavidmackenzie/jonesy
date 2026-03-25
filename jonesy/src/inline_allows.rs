//! Inline allow comment scanning.
//!
//! Scans source files for `// jonesy:allow(cause)` comments that suppress
//! specific panic causes at particular lines.
//!
//! Supported comment formats:
//! - `// jonesy:allow(unwrap)` - allow single cause
//! - `// jonesy:allow(unwrap, expect)` - allow multiple causes
//! - `// jonesy:allow(*)` - allow all causes
//!
//! The comment applies to the line it's on, making it easy to place at the
//! end of lines with potential panics.

#![allow(dead_code)] // Some functions reserved for future batch scanning use

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Map of (file, line) -> set of allowed cause IDs
pub type InlineAllows = HashMap<(String, u32), HashSet<String>>;

/// Parse inline allow comments from a source file.
/// Returns a map of line numbers to allowed cause IDs.
fn parse_file_allows(content: &str) -> HashMap<u32, HashSet<String>> {
    let mut allows: HashMap<u32, HashSet<String>> = HashMap::new();

    for (idx, line) in content.lines().enumerate() {
        let line_num = (idx + 1) as u32;

        // Look for // jonesy:allow(...) pattern
        if let Some(start) = line.find("// jonesy:allow(") {
            let rest = &line[start + 16..]; // Skip "// jonesy:allow("
            if let Some(end) = rest.find(')') {
                let causes_str = &rest[..end];
                let causes: HashSet<String> = causes_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();

                if !causes.is_empty() {
                    allows.insert(line_num, causes);
                }
            }
        }
    }

    allows
}

/// Scan source files and collect inline allow comments.
/// Takes a list of file paths to scan.
pub fn scan_inline_allows(file_paths: &[&str]) -> InlineAllows {
    let mut all_allows = InlineAllows::new();

    for file_path in file_paths {
        if let Ok(content) = fs::read_to_string(file_path) {
            let file_allows = parse_file_allows(&content);
            for (line, causes) in file_allows {
                all_allows.insert((file_path.to_string(), line), causes);
            }
        }
    }

    all_allows
}

/// Scan a single source file for inline allows.
pub fn scan_file_allows(file_path: &str) -> HashMap<u32, HashSet<String>> {
    if let Ok(content) = fs::read_to_string(file_path) {
        parse_file_allows(&content)
    } else {
        HashMap::new()
    }
}

/// Check if a cause is allowed at a specific file:line by inline comment.
/// Also checks the previous line (for comments above the code).
pub fn is_allowed_by_inline(
    allows: &InlineAllows,
    file_path: &str,
    line: u32,
    cause_id: &str,
) -> bool {
    // Normalize the file path for matching
    let key = (file_path.to_string(), line);

    if let Some(causes) = allows.get(&key) {
        if causes.contains("*") || causes.contains(cause_id) {
            return true;
        }
    }

    // Also check the line above (comment on previous line)
    if line > 1 {
        let prev_key = (file_path.to_string(), line - 1);
        if let Some(causes) = allows.get(&prev_key) {
            if causes.contains("*") || causes.contains(cause_id) {
                return true;
            }
        }
    }

    false
}

/// Lazily scan and check inline allows for a specific file and line.
/// This reads the file on demand, suitable for checking individual code points.
///
/// Due to DWARF debug info sometimes being off by a line or two, we check
/// a symmetric range around the reported line number (±2 lines).
///
/// The `workspace_root` parameter is optional - if provided, it's used to resolve
/// relative paths. If not provided, falls back to current directory.
pub fn check_inline_allow(
    file_path: &str,
    line: u32,
    cause_id: &str,
    workspace_root: Option<&Path>,
) -> bool {
    // Try to read the file - handle both absolute and relative paths
    let content = read_source_file_with_root(file_path, workspace_root);

    let content = match content {
        Some(c) => c,
        None => return false, // Can't read the file
    };

    let lines: Vec<&str> = content.lines().collect();

    // Check a symmetric range around the reported location (line-2 to line+2)
    // This handles cases where DWARF debug info is slightly off
    let start_line = line.saturating_sub(2);
    let end_line = line.saturating_add(2);

    for check_line in start_line..=end_line {
        if let Some(line_content) = lines.get((check_line as usize).saturating_sub(1)) {
            if let Some(causes) = parse_line_allows(line_content) {
                if causes.contains("*") || causes.contains(cause_id) {
                    return true;
                }
            }
        }
    }

    false
}

/// Try to read a source file, handling both absolute and relative paths.
/// DWARF often stores relative paths from the workspace root.
///
/// If `workspace_root` is provided, uses it to resolve relative paths.
/// Otherwise falls back to searching from current directory.
fn read_source_file_with_root(file_path: &str, workspace_root: Option<&Path>) -> Option<String> {
    let path = Path::new(file_path);

    // Try the path as-is first (handles absolute paths)
    if path.exists() {
        return fs::read_to_string(path).ok();
    }

    // If workspace_root is provided, try resolving from there first
    if let Some(root) = workspace_root {
        let candidate = root.join(file_path);
        if candidate.exists() {
            return fs::read_to_string(&candidate).ok();
        }
    }

    // For relative paths, try to find the workspace root by looking for Cargo.toml
    // and resolve from there
    if let Ok(cwd) = std::env::current_dir() {
        // Walk up from current directory looking for workspace root
        let mut dir = cwd.as_path();
        loop {
            let candidate = dir.join(file_path);
            if candidate.exists() {
                return fs::read_to_string(&candidate).ok();
            }

            // Check if we've found a workspace root (Cargo.toml with [workspace])
            let cargo_toml = dir.join("Cargo.toml");
            if cargo_toml.exists() {
                if let Ok(content) = fs::read_to_string(&cargo_toml) {
                    if content.contains("[workspace]") {
                        // This is the workspace root - try one more time
                        let candidate = dir.join(file_path);
                        if candidate.exists() {
                            return fs::read_to_string(&candidate).ok();
                        }
                        break; // Don't go above workspace root
                    }
                }
            }

            // Move up one directory
            match dir.parent() {
                Some(parent) => dir = parent,
                None => break,
            }
        }
    }

    None
}

/// Parse inline allow causes from a single line.
fn parse_line_allows(line: &str) -> Option<HashSet<String>> {
    if let Some(start) = line.find("// jonesy:allow(") {
        let rest = &line[start + 16..];
        if let Some(end) = rest.find(')') {
            let causes_str = &rest[..end];
            let causes: HashSet<String> = causes_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            if !causes.is_empty() {
                return Some(causes);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_cause() {
        let content = r#"
fn foo() {
    let x = None.unwrap(); // jonesy:allow(unwrap)
}
"#;
        let allows = parse_file_allows(content);
        assert!(allows.get(&3).unwrap().contains("unwrap"));
    }

    #[test]
    fn test_parse_multiple_causes() {
        let content = r#"
fn foo() {
    something(); // jonesy:allow(unwrap, expect, panic)
}
"#;
        let allows = parse_file_allows(content);
        let causes = allows.get(&3).unwrap();
        assert!(causes.contains("unwrap"));
        assert!(causes.contains("expect"));
        assert!(causes.contains("panic"));
    }

    #[test]
    fn test_parse_wildcard() {
        let content = r#"
fn foo() {
    dangerous(); // jonesy:allow(*)
}
"#;
        let allows = parse_file_allows(content);
        assert!(allows.get(&3).unwrap().contains("*"));
    }

    #[test]
    fn test_parse_line_allows() {
        let line = "    let x = foo(); // jonesy:allow(unwrap, bounds)";
        let causes = parse_line_allows(line).unwrap();
        assert!(causes.contains("unwrap"));
        assert!(causes.contains("bounds"));
    }

    #[test]
    fn test_parse_line_allows_no_match() {
        let line = "    let x = foo(); // regular comment";
        assert!(parse_line_allows(line).is_none());
    }

    #[test]
    fn test_parse_line_allows_empty() {
        let line = "    let x = foo(); // jonesy:allow()";
        assert!(parse_line_allows(line).is_none());
    }

    #[test]
    fn test_parse_file_allows_no_comments() {
        let content = "fn foo() {\n    bar();\n}\n";
        let allows = parse_file_allows(content);
        assert!(allows.is_empty());
    }

    #[test]
    fn test_parse_file_allows_multiple_lines() {
        let content = r#"
fn foo() {
    bar(); // jonesy:allow(panic)
    baz();
    qux(); // jonesy:allow(unwrap)
}
"#;
        let allows = parse_file_allows(content);
        assert_eq!(allows.len(), 2);
        assert!(allows.get(&3).unwrap().contains("panic"));
        assert!(allows.get(&5).unwrap().contains("unwrap"));
    }

    #[test]
    fn test_is_allowed_by_inline_exact_match() {
        let mut allows = InlineAllows::new();
        let mut causes = HashSet::new();
        causes.insert("unwrap".to_string());
        allows.insert(("test.rs".to_string(), 10), causes);

        assert!(is_allowed_by_inline(&allows, "test.rs", 10, "unwrap"));
        assert!(!is_allowed_by_inline(&allows, "test.rs", 10, "panic"));
    }

    #[test]
    fn test_is_allowed_by_inline_wildcard() {
        let mut allows = InlineAllows::new();
        let mut causes = HashSet::new();
        causes.insert("*".to_string());
        allows.insert(("test.rs".to_string(), 10), causes);

        assert!(is_allowed_by_inline(&allows, "test.rs", 10, "unwrap"));
        assert!(is_allowed_by_inline(&allows, "test.rs", 10, "panic"));
        assert!(is_allowed_by_inline(&allows, "test.rs", 10, "anything"));
    }

    #[test]
    fn test_is_allowed_by_inline_previous_line() {
        let mut allows = InlineAllows::new();
        let mut causes = HashSet::new();
        causes.insert("unwrap".to_string());
        // Comment on line 9
        allows.insert(("test.rs".to_string(), 9), causes);

        // Should match for line 10 (checks previous line)
        assert!(is_allowed_by_inline(&allows, "test.rs", 10, "unwrap"));
        // Should not match for line 11
        assert!(!is_allowed_by_inline(&allows, "test.rs", 11, "unwrap"));
    }

    #[test]
    fn test_is_allowed_by_inline_no_match() {
        let allows = InlineAllows::new();
        assert!(!is_allowed_by_inline(&allows, "test.rs", 10, "unwrap"));
    }

    #[test]
    fn test_is_allowed_by_inline_line_one() {
        let mut allows = InlineAllows::new();
        let mut causes = HashSet::new();
        causes.insert("unwrap".to_string());
        allows.insert(("test.rs".to_string(), 1), causes);

        // Line 1 should not try to check line 0 (which doesn't exist)
        assert!(is_allowed_by_inline(&allows, "test.rs", 1, "unwrap"));
    }

    #[test]
    fn test_scan_file_allows_nonexistent() {
        let allows = scan_file_allows("/nonexistent/path/to/file.rs");
        assert!(allows.is_empty());
    }
}
