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

/// Print panic points in a flat format with absolute paths.
fn print_flat_format(points: &[CrateCodePoint], project_root: Option<&Path>) {
    for point in points {
        print_flat_point(point, project_root);
    }
}

/// Print a single point in flat format with its children.
fn print_flat_point(point: &CrateCodePoint, project_root: Option<&Path>) {
    let display_path = get_clickable_path(&point.file, project_root);
    let column = point.column.unwrap_or(1);
    let location = format!("{}:{}:{}", display_path, point.line, column);

    let is_leaf = point.children.is_empty();
    let primary_cause = get_primary_cause(&point.causes);

    let cause_str = if is_leaf {
        primary_cause
            .map(|c| format!(" {}", format_cause(c)))
            .unwrap_or_default()
    } else {
        String::new()
    };

    println!(" --> {}{}", location, cause_str);

    if is_leaf {
        if let Some(cause) = primary_cause {
            let suggestion = cause.suggestion();
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
    let display_path = get_clickable_path(&point.file, project_root);
    let column = point.column.unwrap_or(1);
    let location = format!("{}:{}:{}", display_path, point.line, column);

    let is_leaf = point.children.is_empty();
    let primary_cause = get_primary_cause(&point.causes);

    let cause_str = if is_leaf {
        primary_cause
            .map(|c| format!(" {}", format_cause(c)))
            .unwrap_or_default()
    } else {
        String::new()
    };

    println!("{}└──  --> {}{}", indent, location, cause_str);

    if is_leaf {
        if let Some(cause) = primary_cause {
            let suggestion = cause.suggestion();
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

    let primary_cause = get_primary_cause(&point.causes);
    let is_leaf = point.children.is_empty();

    let cause_str = if is_leaf {
        primary_cause
            .map(|c| format!(" {}", format_cause(c)))
            .unwrap_or_default()
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
        if let Some(cause) = primary_cause {
            let suggestion = cause.suggestion();
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
        let root_str = root.to_string_lossy();
        absolute_path
            .strip_prefix(&format!("{}/", root_str))
            .unwrap_or(&absolute_path)
            .to_string()
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

    let primary_cause = get_primary_cause(&point.causes);
    let is_leaf = point.children.is_empty();

    let cause_str = if is_leaf {
        primary_cause
            .map(|c| format!(" {}", format_cause(c)))
            .unwrap_or_default()
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
        if let Some(cause) = primary_cause {
            let suggestion = cause.suggestion();
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

/// Get the primary cause from a set of causes (sorted for determinism).
fn get_primary_cause(causes: &std::collections::HashSet<PanicCause>) -> Option<&PanicCause> {
    let mut sorted: Vec<_> = causes.iter().collect();
    sorted.sort_by_key(|c| c.description());
    sorted.first().copied()
}

/// Format the cause string with error code, e.g., "[JP001: explicit panic!() call]"
fn format_cause(cause: &PanicCause) -> String {
    format!("[{}: {}]", cause.error_code(), cause.description())
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

/// Get a clickable path for terminal output (absolute path).
fn get_clickable_path(file: &str, project_root: Option<&Path>) -> String {
    make_absolute(file, project_root)
}

/// Get the display path for a file (relative to crate root).
fn get_display_path(file: &str, project_root: Option<&Path>, crate_root: Option<&Path>) -> String {
    let absolute_path = make_absolute(file, project_root);
    let display_root = crate_root.or(project_root);

    if let Some(root) = display_root {
        let root_str = root.to_string_lossy();
        absolute_path
            .strip_prefix(&format!("{}/", root_str))
            .unwrap_or(&absolute_path)
            .to_string()
    } else {
        absolute_path
    }
}
