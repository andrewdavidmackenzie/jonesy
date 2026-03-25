//! Text output format for jonesy analysis results.
//!
//! This module generates human-readable text output for terminal display,
//! with optional OSC 8 hyperlinks for clickable file locations.

use crate::args::VERSION;
use crate::call_tree::{AnalysisResult, CrateCodePoint};
use crate::panic_cause::PanicCause;
use is_terminal::IsTerminal;
use std::io;
use std::path::Path;
use url::Url;

/// Generate text output from analysis results.
///
/// When `summary_only` is true, only prints the summary statistics.
/// When `tree` is true, includes the full call tree with children.
/// When `no_hyperlinks` is false and stdout is a TTY, uses OSC 8 terminal hyperlinks.
pub fn generate_text_output(
    result: &AnalysisResult,
    tree: bool,
    summary_only: bool,
    no_hyperlinks: bool,
) {
    let summary = result.summary();

    if !summary_only {
        print_panic_points(
            &result.code_points,
            &result.project_root,
            tree,
            no_hyperlinks,
        );
    }

    // Print summary
    println!("Summary (jonesy v{}):", VERSION);
    println!("  Project: {}", result.project_name);
    println!("  Root: {}", result.project_root);
    println!(
        "  Panic points: {} in {} file(s)",
        summary.panic_points(),
        summary.files_affected()
    );
}

/// Print panic code points to stdout.
fn print_panic_points(
    code_points: &[CrateCodePoint],
    project_root: &str,
    _tree: bool,
    no_hyperlinks: bool,
) {
    if code_points.is_empty() {
        println!("\nNo panics in crate");
        return;
    }

    let project_root_path = Path::new(project_root);

    println!("\nPanic code points in crate:");

    // Use tree format only when OSC 8 hyperlinks are available
    let use_tree_format = !no_hyperlinks && io::stdout().is_terminal();

    if use_tree_format {
        println!();
        print_directory_tree(code_points, Some(project_root_path), None, no_hyperlinks);
    } else {
        print_flat_format(code_points, Some(project_root_path));
    }
}

/// Print panic points grouped by directory in a tree format.
fn print_directory_tree(
    points: &[CrateCodePoint],
    project_root: Option<&Path>,
    crate_root: Option<&Path>,
    no_hyperlinks: bool,
) {
    // Group points by directory
    let mut dir_groups: Vec<(String, Vec<&CrateCodePoint>)> = Vec::new();

    for point in points {
        let display_path = get_display_path(&point.file, project_root, crate_root);
        let dir = if let Some(pos) = display_path.rfind('/') {
            display_path[..pos].to_string()
        } else {
            String::new()
        };

        if let Some((last_dir, files)) = dir_groups.last_mut() {
            if last_dir == &dir {
                files.push(point);
                continue;
            }
        }
        dir_groups.push((dir, vec![point]));
    }

    let group_count = dir_groups.len();
    for (i, (dir, files)) in dir_groups.iter().enumerate() {
        let is_last_dir = i == group_count - 1;
        let dir_connector = if is_last_dir {
            "└── "
        } else {
            "├── "
        };
        let child_prefix = if is_last_dir { "    " } else { "│   " };

        if !dir.is_empty() {
            println!("{}{}/", dir_connector, dir);
        }

        let file_count = files.len();
        for (j, point) in files.iter().enumerate() {
            let is_last_file = j == file_count - 1;
            print_file_entry(
                point,
                if dir.is_empty() { "" } else { child_prefix },
                is_last_file,
                dir.is_empty(),
                project_root,
                crate_root,
                no_hyperlinks,
            );
        }
    }
}

/// Print panic points in a flat format with relative paths (CI-friendly).
fn print_flat_format(points: &[CrateCodePoint], project_root: Option<&Path>) {
    for point in points {
        print_flat_point(point, project_root);
    }
}

/// Print a single point in flat format with its children.
fn print_flat_point(point: &CrateCodePoint, project_root: Option<&Path>) {
    // Use relative paths for CI-friendly output (works with GitHub problem matchers)
    let display_path = get_relative_path(&point.file, project_root);
    let column = point.column.unwrap_or(1);
    let location = format!("{}:{}:{}", display_path, point.line, column);

    let is_leaf = point.children.is_empty();
    let sorted_causes = get_sorted_causes(&point.causes);

    let cause_str = if is_leaf && !sorted_causes.is_empty() {
        format!(" {}", format_causes(&sorted_causes))
    } else {
        String::new()
    };

    println!(" --> {}{}", location, cause_str);

    if is_leaf {
        // Show help/warning for primary cause only (first in sorted order)
        if let Some(cause) = sorted_causes.first() {
            let suggestion =
                cause.format_suggestion(point.is_direct_panic, point.called_function.as_deref());
            if !suggestion.is_empty() {
                println!("     = help: {}", suggestion);
            }
            if let Some(warning) = cause.release_warning() {
                println!("     = warning: {}", warning);
            }
        }
    }

    if !point.children.is_empty() {
        let mut children = point.children.clone();
        children.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
        for child in &children {
            print_flat_child(child, project_root, "     ");
        }
    }
}

/// Print a child point in flat format with indentation.
fn print_flat_child(point: &CrateCodePoint, project_root: Option<&Path>, indent: &str) {
    // Use relative paths for CI-friendly output
    let display_path = get_relative_path(&point.file, project_root);
    let column = point.column.unwrap_or(1);
    let location = format!("{}:{}:{}", display_path, point.line, column);

    let is_leaf = point.children.is_empty();
    let sorted_causes = get_sorted_causes(&point.causes);

    let cause_str = if is_leaf && !sorted_causes.is_empty() {
        format!(" {}", format_causes(&sorted_causes))
    } else {
        String::new()
    };

    println!("{}└──  --> {}{}", indent, location, cause_str);

    if is_leaf {
        // Show help/warning for primary cause only (first in sorted order)
        if let Some(cause) = sorted_causes.first() {
            let suggestion =
                cause.format_suggestion(point.is_direct_panic, point.called_function.as_deref());
            if !suggestion.is_empty() {
                println!("{}     = help: {}", indent, suggestion);
            }
            if let Some(warning) = cause.release_warning() {
                println!("{}     = warning: {}", indent, warning);
            }
        }
    }

    if !point.children.is_empty() {
        let mut children = point.children.clone();
        children.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
        let child_indent = format!("{}     ", indent);
        for child in &children {
            print_flat_child(child, project_root, &child_indent);
        }
    }
}

/// Print a single file entry within a directory group.
fn print_file_entry(
    point: &CrateCodePoint,
    prefix: &str,
    is_last: bool,
    is_root_level: bool,
    project_root: Option<&Path>,
    crate_root: Option<&Path>,
    no_hyperlinks: bool,
) {
    let display_path = get_display_path(&point.file, project_root, crate_root);
    let filename = display_path
        .rfind('/')
        .map(|pos| &display_path[pos + 1..])
        .unwrap_or(&display_path);

    let absolute_path = make_absolute(&point.file, project_root);
    let use_hyperlinks = !no_hyperlinks && io::stdout().is_terminal();
    let column = point.column.unwrap_or(1);

    let location = if use_hyperlinks {
        if let Ok(mut file_url) = Url::from_file_path(&absolute_path) {
            file_url.set_fragment(Some(&format!("L{}", point.line)));
            let display = format!("{}:{}:{}", filename, point.line, column);
            format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", file_url, display)
        } else {
            format!("{}:{}:{}", absolute_path, point.line, column)
        }
    } else {
        format!("{}:{}:{}", absolute_path, point.line, column)
    };

    let sorted_causes = get_sorted_causes(&point.causes);
    let is_leaf = point.children.is_empty();

    let cause_str = if is_leaf && !sorted_causes.is_empty() {
        format!(" {}", format_causes(&sorted_causes))
    } else {
        String::new()
    };

    let connector = if is_root_level {
        " -->"
    } else if is_last {
        "└──>"
    } else {
        "├──>"
    };

    println!("{}{} {}{}", prefix, connector, location, cause_str);

    if is_leaf {
        // Show help/warning for primary cause only (first in sorted order)
        if let Some(cause) = sorted_causes.first() {
            let suggestion =
                cause.format_suggestion(point.is_direct_panic, point.called_function.as_deref());
            if !suggestion.is_empty() {
                let help_prefix = if is_root_level { "     " } else { prefix };
                println!("{}    = help: {}", help_prefix, suggestion);
            }
            if let Some(warning) = cause.release_warning() {
                let warn_prefix = if is_root_level { "     " } else { prefix };
                println!("{}    = warning: {}", warn_prefix, warning);
            }
        }
    }

    if !point.children.is_empty() {
        let mut children = point.children.clone();
        children.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

        let child_prefix = if is_root_level {
            "     ".to_string()
        } else if is_last {
            format!("{}    ", prefix)
        } else {
            format!("{}│   ", prefix)
        };

        let child_count = children.len();
        for (k, child) in children.iter().enumerate() {
            let is_last_child = k == child_count - 1;
            print_crate_point(
                child,
                &child_prefix,
                is_last_child,
                false,
                project_root,
                crate_root,
                no_hyperlinks,
            );
        }
    }
}

/// Print a crate code point with tree-style indentation.
fn print_crate_point(
    point: &CrateCodePoint,
    prefix: &str,
    is_last: bool,
    is_root: bool,
    project_root: Option<&Path>,
    crate_root: Option<&Path>,
    no_hyperlinks: bool,
) {
    let absolute_path = make_absolute(&point.file, project_root);
    let display_root = crate_root.or(project_root);
    let display_path = if let Some(root) = display_root {
        Path::new(&absolute_path)
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| absolute_path.clone())
    } else {
        absolute_path.clone()
    };

    let use_hyperlinks = !no_hyperlinks && io::stdout().is_terminal();
    let column = point.column.unwrap_or(1);

    let location = if use_hyperlinks {
        if let Ok(mut file_url) = Url::from_file_path(&absolute_path) {
            file_url.set_fragment(Some(&format!("L{}", point.line)));
            let display = format!("{}:{}:{}", display_path, point.line, column);
            format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", file_url, display)
        } else {
            format!("{}:{}:{}", absolute_path, point.line, column)
        }
    } else {
        format!("{}:{}:{}", absolute_path, point.line, column)
    };

    let sorted_causes = get_sorted_causes(&point.causes);
    let is_leaf = point.children.is_empty();

    let cause_str = if is_leaf && !sorted_causes.is_empty() {
        format!(" {}", format_causes(&sorted_causes))
    } else {
        String::new()
    };

    let connector = if is_root {
        " -->"
    } else if is_last {
        "└──>"
    } else {
        "├──>"
    };

    println!("{}{} {}{}", prefix, connector, location, cause_str);

    if is_leaf {
        // Show help/warning for primary cause only (first in sorted order)
        if let Some(cause) = sorted_causes.first() {
            let suggestion =
                cause.format_suggestion(point.is_direct_panic, point.called_function.as_deref());
            if !suggestion.is_empty() {
                let help_prefix = if is_root { "     " } else { prefix };
                println!("{}    = help: {}", help_prefix, suggestion);
            }
            if let Some(warning) = cause.release_warning() {
                let warn_prefix = if is_root { "     " } else { prefix };
                println!("{}    = warning: {}", warn_prefix, warning);
            }
        }
    }

    if !point.children.is_empty() {
        let mut children = point.children.clone();
        children.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

        let child_prefix = if is_root {
            "     ".to_string()
        } else if is_last {
            format!("{}    ", prefix)
        } else {
            format!("{}│   ", prefix)
        };

        let child_count = children.len();
        for (k, child) in children.iter().enumerate() {
            let is_last_child = k == child_count - 1;
            print_crate_point(
                child,
                &child_prefix,
                is_last_child,
                false,
                project_root,
                crate_root,
                no_hyperlinks,
            );
        }
    }
}

/// Get all causes sorted by error code for deterministic output.
/// Returns causes sorted by error code (JP001 < JP002 < ... < JP022).
fn get_sorted_causes(causes: &std::collections::HashSet<PanicCause>) -> Vec<&PanicCause> {
    let mut sorted: Vec<_> = causes.iter().collect();
    sorted.sort_by_key(|c| c.error_code());
    sorted
}

/// Format all causes as a string, e.g., "[JP001: explicit panic!() call] [JP006: unwrap() on None]"
fn format_causes(causes: &[&PanicCause]) -> String {
    causes
        .iter()
        .map(|c| format!("[{}: {}]", c.error_code(), c.description()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Make a path absolute using the project root.
fn make_absolute(file: &str, project_root: Option<&Path>) -> String {
    if file.starts_with('/') {
        file.to_string()
    } else if let Some(root) = project_root {
        root.join(file).to_string_lossy().to_string()
    } else {
        file.to_string()
    }
}

/// Get a relative path for CI-friendly output (works with GitHub problem matchers).
fn get_relative_path(file: &str, project_root: Option<&Path>) -> String {
    let absolute_path = make_absolute(file, project_root);
    if let Some(root) = project_root {
        Path::new(&absolute_path)
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(absolute_path)
    } else {
        absolute_path
    }
}

/// Get the display path for a file (relative to crate root).
fn get_display_path(file: &str, project_root: Option<&Path>, crate_root: Option<&Path>) -> String {
    let absolute_path = make_absolute(file, project_root);
    let display_root = crate_root.or(project_root);

    if let Some(root) = display_root {
        Path::new(&absolute_path)
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(absolute_path)
    } else {
        absolute_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panic_cause::PanicCause;
    use std::collections::HashSet;

    fn make_test_point(causes: Vec<PanicCause>) -> CrateCodePoint {
        CrateCodePoint {
            name: "test_func".to_string(),
            file: "src/main.rs".to_string(),
            line: 10,
            column: Some(5),
            causes: causes.into_iter().collect(),
            children: vec![],
            is_direct_panic: true,
            called_function: None,
        }
    }

    #[test]
    fn test_make_absolute_already_absolute() {
        let path = make_absolute("/home/user/src/main.rs", None);
        assert_eq!(path, "/home/user/src/main.rs");
    }

    #[test]
    fn test_make_absolute_with_project_root() {
        let root = Path::new("/home/user/project");
        let path = make_absolute("src/main.rs", Some(root));
        assert_eq!(path, "/home/user/project/src/main.rs");
    }

    #[test]
    fn test_make_absolute_no_root() {
        let path = make_absolute("src/main.rs", None);
        assert_eq!(path, "src/main.rs");
    }

    #[test]
    fn test_get_relative_path_absolute() {
        let root = Path::new("/home/user/project");
        let path = get_relative_path("/home/user/project/src/main.rs", Some(root));
        assert_eq!(path, "src/main.rs");
    }

    #[test]
    fn test_get_relative_path_already_relative() {
        let root = Path::new("/home/user/project");
        let path = get_relative_path("src/main.rs", Some(root));
        assert_eq!(path, "src/main.rs");
    }

    #[test]
    fn test_get_relative_path_no_root() {
        let path = get_relative_path("src/main.rs", None);
        assert_eq!(path, "src/main.rs");
    }

    #[test]
    fn test_get_display_path_with_crate_root() {
        let project_root = Path::new("/workspace");
        let crate_root = Path::new("/workspace/crate_a");
        let path = get_display_path(
            "/workspace/crate_a/src/lib.rs",
            Some(project_root),
            Some(crate_root),
        );
        assert_eq!(path, "src/lib.rs");
    }

    #[test]
    fn test_get_display_path_project_root_only() {
        let project_root = Path::new("/workspace");
        let path = get_display_path("/workspace/src/main.rs", Some(project_root), None);
        assert_eq!(path, "src/main.rs");
    }

    #[test]
    fn test_get_sorted_causes_empty() {
        let causes: HashSet<PanicCause> = HashSet::new();
        let sorted = get_sorted_causes(&causes);
        assert!(sorted.is_empty());
    }

    #[test]
    fn test_get_sorted_causes_single() {
        let mut causes = HashSet::new();
        causes.insert(PanicCause::UnwrapNone);
        let sorted = get_sorted_causes(&causes);
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].error_code(), "JP006");
    }

    #[test]
    fn test_get_sorted_causes_multiple() {
        let mut causes = HashSet::new();
        causes.insert(PanicCause::UnwrapErr); // JP007
        causes.insert(PanicCause::BoundsCheck); // JP002
        causes.insert(PanicCause::ExplicitPanic); // JP001
        let sorted = get_sorted_causes(&causes);
        assert_eq!(sorted.len(), 3);
        // Should be sorted by error code
        assert_eq!(sorted[0].error_code(), "JP001");
        assert_eq!(sorted[1].error_code(), "JP002");
        assert_eq!(sorted[2].error_code(), "JP007");
    }

    #[test]
    fn test_format_causes_empty() {
        let causes: Vec<&PanicCause> = vec![];
        let formatted = format_causes(&causes);
        assert_eq!(formatted, "");
    }

    #[test]
    fn test_format_causes_single() {
        let cause = PanicCause::UnwrapNone;
        let causes = vec![&cause];
        let formatted = format_causes(&causes);
        assert_eq!(formatted, "[JP006: unwrap() on None]");
    }

    #[test]
    fn test_format_causes_multiple() {
        let cause1 = PanicCause::ExplicitPanic;
        let cause2 = PanicCause::BoundsCheck;
        let causes = vec![&cause1, &cause2];
        let formatted = format_causes(&causes);
        assert_eq!(
            formatted,
            "[JP001: explicit panic!() call] [JP002: index out of bounds]"
        );
    }
}
