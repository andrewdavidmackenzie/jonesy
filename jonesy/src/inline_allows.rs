//! Inline allow comment scanning.
//!
//! Scans source files for `// jonesy:allow(cause)` comments that suppress
//! specific panic causes at particular lines.
//!
//! Supported comment formats:
//! - `// jonesy:allow(unwrap)` - allow single cause
//! - `// jonesy:allow(unwrap, expect)` - allow multiple causes
//! - `// jonesy:allow(*)` - allow all causes
//! - `// jonesy: allow(unwrap)` - space after colon is also accepted
//!
//! **Invariant**: An inline allow comment must be on the same line as the
//! reported panic point, or on the line immediately above it. No wider
//! range is checked.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Check if a cause is allowed at a specific file:line by inline comment.
/// Also checks the previous line (for comments above the code).
#[cfg(test)]
fn is_allowed_by_inline(
    allows: &std::collections::HashMap<(String, u32), HashSet<String>>,
    file_path: &str,
    line: u32,
    cause_id: &str,
) -> bool {
    let key = (file_path.to_string(), line);

    if let Some(causes) = allows.get(&key) {
        if causes.contains("*") || causes.contains(cause_id) {
            return true;
        }
    }

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

/// Parse inline allow comments from source content (test helper).
#[cfg(test)]
fn parse_file_allows(content: &str) -> std::collections::HashMap<u32, HashSet<String>> {
    let mut allows: std::collections::HashMap<u32, HashSet<String>> =
        std::collections::HashMap::new();
    for (idx, line) in content.lines().enumerate() {
        let line_num = (idx + 1) as u32;
        if let Some(causes) = parse_line_allows(line) {
            allows.insert(line_num, causes);
        }
    }
    allows
}

/// Lazily scan and check inline allows for a specific file and line.
/// This reads the file on demand, suitable for checking individual code points.
///
/// An inline allow comment must be on the same line as the reported panic point,
/// or on the line immediately above it. No wider range is checked.
///
/// The `workspace_root` parameter is optional - if provided, it's used to resolve
/// relative paths. If not provided, falls back to current directory.
pub fn check_inline_allow(
    file_path: &str,
    line: u32,
    cause_id: &str,
    workspace_root: Option<&Path>,
) -> bool {
    if line == 0 {
        return false;
    }

    let content = read_source_file_with_root(file_path, workspace_root);

    let content = match content {
        Some(c) => c,
        None => return false,
    };

    let lines: Vec<&str> = content.lines().collect();

    // Check the reported line and the line above it only
    for check_line in [line.saturating_sub(1), line] {
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
/// Accepts both `// jonesy:allow(...)` and `// jonesy: allow(...)` (space after colon).
fn parse_line_allows(line: &str) -> Option<HashSet<String>> {
    // Find the allow marker, tolerating an optional space after the colon
    let prefix_start = line
        .find("// jonesy:allow(")
        .or_else(|| line.find("// jonesy: allow("))?;
    let paren_start = line[prefix_start..].find('(')? + prefix_start + 1;
    let rest = &line[paren_start..];
    let end = rest.find(')')?;
    let causes_str = &rest[..end];
    let causes: HashSet<String> = causes_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if causes.is_empty() {
        None
    } else {
        Some(causes)
    }
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
    fn test_parse_line_allows_space_after_colon() {
        let line = "    let x = foo(); // jonesy: allow(overflow)";
        let causes = parse_line_allows(line).unwrap();
        assert!(causes.contains("overflow"));
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
        let mut allows = std::collections::HashMap::new();
        let mut causes = HashSet::new();
        causes.insert("unwrap".to_string());
        allows.insert(("test.rs".to_string(), 10), causes);

        assert!(is_allowed_by_inline(&allows, "test.rs", 10, "unwrap"));
        assert!(!is_allowed_by_inline(&allows, "test.rs", 10, "panic"));
    }

    #[test]
    fn test_is_allowed_by_inline_wildcard() {
        let mut allows = std::collections::HashMap::new();
        let mut causes = HashSet::new();
        causes.insert("*".to_string());
        allows.insert(("test.rs".to_string(), 10), causes);

        assert!(is_allowed_by_inline(&allows, "test.rs", 10, "unwrap"));
        assert!(is_allowed_by_inline(&allows, "test.rs", 10, "panic"));
        assert!(is_allowed_by_inline(&allows, "test.rs", 10, "anything"));
    }

    #[test]
    fn test_is_allowed_by_inline_previous_line() {
        let mut allows = std::collections::HashMap::new();
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
        let allows = std::collections::HashMap::new();
        assert!(!is_allowed_by_inline(&allows, "test.rs", 10, "unwrap"));
    }

    #[test]
    fn test_is_allowed_by_inline_line_one() {
        let mut allows = std::collections::HashMap::new();
        let mut causes = HashSet::new();
        causes.insert("unwrap".to_string());
        allows.insert(("test.rs".to_string(), 1), causes);

        // Line 1 should not try to check line 0 (which doesn't exist)
        assert!(is_allowed_by_inline(&allows, "test.rs", 1, "unwrap"));
    }

    #[test]
    fn test_check_inline_allow_with_file() {
        use std::io::Write;
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.rs");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "fn foo() {{").unwrap();
        writeln!(file, "    bar(); // jonesy:allow(unwrap)").unwrap();
        writeln!(file, "}}").unwrap();

        // Line 2 has the allow comment
        assert!(check_inline_allow(
            file_path.to_str().unwrap(),
            2,
            "unwrap",
            None
        ));
        // Line 3 is within ±2 range of line 2
        assert!(check_inline_allow(
            file_path.to_str().unwrap(),
            3,
            "unwrap",
            None
        ));
        // Different cause should not match
        assert!(!check_inline_allow(
            file_path.to_str().unwrap(),
            2,
            "panic",
            None
        ));
    }

    #[test]
    fn test_check_inline_allow_absolute_path() {
        use std::io::Write;
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.rs");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "x(); // jonesy:allow(*)").unwrap();
        drop(file);

        // Absolute path should work without workspace root
        let result = check_inline_allow(file_path.to_str().unwrap(), 1, "anything", None);
        assert!(result);
    }

    #[test]
    fn test_check_inline_allow_nonexistent_file() {
        assert!(!check_inline_allow(
            "/nonexistent/file.rs",
            1,
            "unwrap",
            None
        ));
    }

    #[test]
    fn test_check_inline_allow_wildcard() {
        use std::io::Write;
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.rs");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "dangerous(); // jonesy:allow(*)").unwrap();

        assert!(check_inline_allow(
            file_path.to_str().unwrap(),
            1,
            "anything",
            None
        ));
        assert!(check_inline_allow(
            file_path.to_str().unwrap(),
            1,
            "unwrap",
            None
        ));
    }

    #[test]
    fn test_is_allowed_by_inline_wrong_cause() {
        let mut allows = std::collections::HashMap::new();
        let mut causes = HashSet::new();
        causes.insert("unwrap".to_string());
        allows.insert(("test.rs".to_string(), 10), causes);

        // Wrong cause should not match
        assert!(!is_allowed_by_inline(&allows, "test.rs", 10, "panic"));
    }

    #[test]
    fn test_is_allowed_by_inline_prev_line_wildcard() {
        let mut allows = std::collections::HashMap::new();
        let mut causes = HashSet::new();
        causes.insert("*".to_string());
        // Comment on line 9
        allows.insert(("test.rs".to_string(), 9), causes);

        // Line 10 should match via previous line with wildcard
        assert!(is_allowed_by_inline(&allows, "test.rs", 10, "anything"));
    }
}
