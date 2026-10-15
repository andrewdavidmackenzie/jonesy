//! Text output format for jonesy analysis results.
//!
//! This module generates human-readable text output for terminal display,
//! with optional OSC 8 hyperlinks for clickable file locations.

use crate::args::VERSION;
use crate::call_tree::{AnalysisResult, CrateCodePoint};
use crate::panic_cause::PanicCause;
use crate::string_tables;
use is_terminal::IsTerminal;
use std::io::{self, Write};
use std::path::Path;
use url::Url;

/// Format a source location as `file:line:column`, omitting the column
/// when it is 0 (DWARF "left edge" — column unspecified by the compiler).
fn format_location(file: &str, line: u16, column: u16) -> String {
    if column == 0 {
        format!("{file}:{line}")
    } else {
        format!("{file}:{line}:{column}")
    }
}

/// Generate text output from analysis results to stdout.
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
    let is_tty = io::stdout().is_terminal();
    let mut stdout = io::stdout().lock();
    write_text_output(
        &mut stdout,
        result,
        tree,
        summary_only,
        no_hyperlinks,
        is_tty,
    )
    .expect("Failed to write to stdout");
}

/// Write text output to any writer.
///
/// This is the internal implementation that writes to a generic `Write` target,
/// allowing tests to capture output.
pub fn write_text_output<W: Write>(
    w: &mut W,
    result: &AnalysisResult,
    tree: bool,
    summary_only: bool,
    no_hyperlinks: bool,
    is_tty: bool,
) -> io::Result<()> {
    let summary = result.summary();

    if !summary_only {
        write_panic_points(
            w,
            &result.code_points,
            &result.file_paths,
            &result.name_paths,
            &result.project_root,
            tree,
            no_hyperlinks,
            is_tty,
        )?;
    }

    // Print summary
    writeln!(w, "Summary (jonesy v{}):", VERSION)?;
    writeln!(w, "  Project: {}", result.project_name)?;
    writeln!(w, "  Root: {}", result.project_root)?;
    writeln!(
        w,
        "  Panic points: {} in {} file(s)",
        summary.panic_points(),
        summary.files_affected()
    )?;
    Ok(())
}

/// Write panic code points.
#[allow(clippy::too_many_arguments)]
fn write_panic_points<W: Write>(
    w: &mut W,
    code_points: &[CrateCodePoint],
    file_paths: &[String],
    name_paths: &[String],
    project_root: &str,
    include_children: bool,
    no_hyperlinks: bool,
    is_tty: bool,
) -> io::Result<()> {
    if code_points.is_empty() {
        writeln!(w, "\nNo panics in crate")?;
        return Ok(());
    }

    let project_root_path = Path::new(project_root);

    writeln!(w, "\nPanic code points in crate:")?;

    // Use directory tree format when OSC 8 hyperlinks are available
    let use_tree_format = !no_hyperlinks && is_tty;

    if use_tree_format {
        writeln!(w)?;
        write_directory_tree(
            w,
            code_points,
            file_paths,
            name_paths,
            Some(project_root_path),
            None,
            no_hyperlinks,
            is_tty,
            include_children,
        )?;
    } else {
        write_flat_format(
            w,
            code_points,
            file_paths,
            name_paths,
            Some(project_root_path),
            include_children,
        )?;
    }
    Ok(())
}

/// Write panic points grouped by directory in a tree format.
#[allow(clippy::too_many_arguments)]
fn write_directory_tree<W: Write>(
    w: &mut W,
    points: &[CrateCodePoint],
    file_paths: &[String],
    name_paths: &[String],
    project_root: Option<&Path>,
    crate_root: Option<&Path>,
    no_hyperlinks: bool,
    is_tty: bool,
    include_children: bool,
) -> io::Result<()> {
    // Group points by directory
    let mut dir_groups: Vec<(String, Vec<&CrateCodePoint>)> = Vec::new();

    for point in points {
        let file_str = string_tables::resolve_file(file_paths, point.file).unwrap_or("");
        let display_path = get_display_path(file_str, project_root, crate_root);
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
            writeln!(w, "{}{}/", dir_connector, dir)?;
        }

        let file_count = files.len();
        for (j, point) in files.iter().enumerate() {
            let is_last_file = j == file_count - 1;
            write_file_entry(
                w,
                point,
                file_paths,
                name_paths,
                if dir.is_empty() { "" } else { child_prefix },
                is_last_file,
                dir.is_empty(),
                project_root,
                crate_root,
                no_hyperlinks,
                is_tty,
                include_children,
            )?;
        }
    }
    Ok(())
}

/// Write panic points in a flat format with relative paths (CI-friendly).
fn write_flat_format<W: Write>(
    w: &mut W,
    points: &[CrateCodePoint],
    file_paths: &[String],
    name_paths: &[String],
    project_root: Option<&Path>,
    include_children: bool,
) -> io::Result<()> {
    for point in points {
        write_flat_point(
            w,
            point,
            file_paths,
            name_paths,
            project_root,
            include_children,
        )?;
    }
    Ok(())
}

/// Write a single point in flat format with its children.
fn write_flat_point<W: Write>(
    w: &mut W,
    point: &CrateCodePoint,
    file_paths: &[String],
    name_paths: &[String],
    project_root: Option<&Path>,
    include_children: bool,
) -> io::Result<()> {
    // Use relative paths for CI-friendly output (works with GitHub problem matchers)
    let file_str = string_tables::resolve_file(file_paths, point.file).unwrap_or("");
    let display_path = get_relative_path(file_str, project_root);
    let location = format_location(&display_path, point.line, point.column);

    // Consider point a leaf if it has no children or we're not showing children
    let is_leaf = point.children.is_empty() || !include_children;
    let sorted_causes = get_sorted_causes(&point.causes);

    let cause_str = if is_leaf && !sorted_causes.is_empty() {
        format!(" {}", format_causes(&sorted_causes))
    } else if !is_leaf && !sorted_causes.is_empty() {
        // Show causes even for non-leaf when we have them
        format!(" {}", format_causes(&sorted_causes))
    } else {
        String::new()
    };

    writeln!(w, " --> {}{}", location, cause_str)?;

    if is_leaf {
        // Show help/warning for primary cause only (first in sorted order)
        if let Some(cause) = sorted_causes.first() {
            let called_fn = string_tables::resolve_name(name_paths, point.called_function);
            let suggestion = cause.format_suggestion(point.is_direct_panic, called_fn);
            if !suggestion.is_empty() {
                writeln!(w, "     = help: {}", suggestion)?;
            }
            if let Some(warning) = cause.release_warning() {
                writeln!(w, "     = warning: {}", warning)?;
            }
        }
    }

    if include_children && !point.children.is_empty() {
        let mut children = point.children.clone();
        children.sort_by_key(|a| (a.file, a.line, a.column));
        for child in &children {
            write_flat_child(
                w,
                child,
                file_paths,
                name_paths,
                project_root,
                "     ",
                include_children,
            )?;
        }
    }
    Ok(())
}

/// Write a child point in flat format with indentation.
fn write_flat_child<W: Write>(
    w: &mut W,
    point: &CrateCodePoint,
    file_paths: &[String],
    name_paths: &[String],
    project_root: Option<&Path>,
    indent: &str,
    include_children: bool,
) -> io::Result<()> {
    // Use relative paths for CI-friendly output
    let file_str = string_tables::resolve_file(file_paths, point.file).unwrap_or("");
    let display_path = get_relative_path(file_str, project_root);
    let location = format_location(&display_path, point.line, point.column);

    let is_leaf = point.children.is_empty() || !include_children;
    let sorted_causes = get_sorted_causes(&point.causes);

    let cause_str = if !sorted_causes.is_empty() {
        format!(" {}", format_causes(&sorted_causes))
    } else {
        String::new()
    };

    writeln!(w, "{}└──  --> {}{}", indent, location, cause_str)?;

    if is_leaf {
        // Show help/warning for primary cause only (first in sorted order)
        if let Some(cause) = sorted_causes.first() {
            let called_fn = string_tables::resolve_name(name_paths, point.called_function);
            let suggestion = cause.format_suggestion(point.is_direct_panic, called_fn);
            if !suggestion.is_empty() {
                writeln!(w, "{}     = help: {}", indent, suggestion)?;
            }
            if let Some(warning) = cause.release_warning() {
                writeln!(w, "{}     = warning: {}", indent, warning)?;
            }
        }
    }

    if include_children && !point.children.is_empty() {
        let mut children = point.children.clone();
        children.sort_by_key(|a| (a.file, a.line, a.column));
        let child_indent = format!("{}     ", indent);
        for child in &children {
            write_flat_child(
                w,
                child,
                file_paths,
                name_paths,
                project_root,
                &child_indent,
                include_children,
            )?;
        }
    }
    Ok(())
}

/// Write a single file entry within a directory group.
#[allow(clippy::too_many_arguments)]
fn write_file_entry<W: Write>(
    w: &mut W,
    point: &CrateCodePoint,
    file_paths: &[String],
    name_paths: &[String],
    prefix: &str,
    is_last: bool,
    is_root_level: bool,
    project_root: Option<&Path>,
    crate_root: Option<&Path>,
    no_hyperlinks: bool,
    is_tty: bool,
    include_children: bool,
) -> io::Result<()> {
    let file_str = string_tables::resolve_file(file_paths, point.file).unwrap_or("");
    let display_path = get_display_path(file_str, project_root, crate_root);
    let filename = display_path
        .rfind('/')
        .map(|pos| &display_path[pos + 1..])
        .unwrap_or(&display_path);

    let absolute_path = make_absolute(file_str, project_root);
    let use_hyperlinks = !no_hyperlinks && is_tty;

    let location = if use_hyperlinks {
        if let Ok(mut file_url) = Url::from_file_path(&absolute_path) {
            file_url.set_fragment(Some(&format!("L{}", point.line)));
            let display = format_location(filename, point.line, point.column);
            format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", file_url, display)
        } else {
            format_location(&absolute_path, point.line, point.column)
        }
    } else {
        format_location(&absolute_path, point.line, point.column)
    };

    let sorted_causes = get_sorted_causes(&point.causes);
    // Consider point a leaf if it has no children or we're not showing children
    let is_leaf = point.children.is_empty() || !include_children;

    let cause_str = if !sorted_causes.is_empty() {
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

    writeln!(w, "{}{} {}{}", prefix, connector, location, cause_str)?;

    if is_leaf {
        // Show help/warning for primary cause only (first in sorted order)
        if let Some(cause) = sorted_causes.first() {
            let called_fn = string_tables::resolve_name(name_paths, point.called_function);
            let suggestion = cause.format_suggestion(point.is_direct_panic, called_fn);
            if !suggestion.is_empty() {
                let help_prefix = if is_root_level { "     " } else { prefix };
                writeln!(w, "{}    = help: {}", help_prefix, suggestion)?;
            }
            if let Some(warning) = cause.release_warning() {
                let warn_prefix = if is_root_level { "     " } else { prefix };
                writeln!(w, "{}    = warning: {}", warn_prefix, warning)?;
            }
        }
    }

    if include_children && !point.children.is_empty() {
        let mut children = point.children.clone();
        children.sort_by_key(|a| (a.file, a.line, a.column));

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
            write_crate_point(
                w,
                child,
                file_paths,
                name_paths,
                &child_prefix,
                is_last_child,
                false,
                project_root,
                crate_root,
                no_hyperlinks,
                is_tty,
                include_children,
            )?;
        }
    }
    Ok(())
}

/// Write a crate code point with tree-style indentation.
#[allow(clippy::too_many_arguments)]
fn write_crate_point<W: Write>(
    w: &mut W,
    point: &CrateCodePoint,
    file_paths: &[String],
    name_paths: &[String],
    prefix: &str,
    is_last: bool,
    is_root: bool,
    project_root: Option<&Path>,
    crate_root: Option<&Path>,
    no_hyperlinks: bool,
    is_tty: bool,
    include_children: bool,
) -> io::Result<()> {
    let file_str = string_tables::resolve_file(file_paths, point.file).unwrap_or("");
    let absolute_path = make_absolute(file_str, project_root);
    let display_root = crate_root.or(project_root);
    let display_path = if let Some(root) = display_root {
        Path::new(&absolute_path)
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| absolute_path.clone())
    } else {
        absolute_path.clone()
    };

    let use_hyperlinks = !no_hyperlinks && is_tty;

    let location = if use_hyperlinks {
        if let Ok(mut file_url) = Url::from_file_path(&absolute_path) {
            file_url.set_fragment(Some(&format!("L{}", point.line)));
            let display = format_location(&display_path, point.line, point.column);
            format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", file_url, display)
        } else {
            format_location(&absolute_path, point.line, point.column)
        }
    } else {
        format_location(&absolute_path, point.line, point.column)
    };

    let sorted_causes = get_sorted_causes(&point.causes);
    // Consider point a leaf if it has no children or we're not showing children
    let is_leaf = point.children.is_empty() || !include_children;

    let cause_str = if !sorted_causes.is_empty() {
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

    writeln!(w, "{}{} {}{}", prefix, connector, location, cause_str)?;

    if is_leaf {
        // Show help/warning for primary cause only (first in sorted order)
        if let Some(cause) = sorted_causes.first() {
            let called_fn = string_tables::resolve_name(name_paths, point.called_function);
            let suggestion = cause.format_suggestion(point.is_direct_panic, called_fn);
            if !suggestion.is_empty() {
                let help_prefix = if is_root { "     " } else { prefix };
                writeln!(w, "{}    = help: {}", help_prefix, suggestion)?;
            }
            if let Some(warning) = cause.release_warning() {
                let warn_prefix = if is_root { "     " } else { prefix };
                writeln!(w, "{}    = warning: {}", warn_prefix, warning)?;
            }
        }
    }

    if include_children && !point.children.is_empty() {
        let mut children = point.children.clone();
        children.sort_by_key(|a| (a.file, a.line, a.column));

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
            write_crate_point(
                w,
                child,
                file_paths,
                name_paths,
                &child_prefix,
                is_last_child,
                false,
                project_root,
                crate_root,
                no_hyperlinks,
                is_tty,
                include_children,
            )?;
        }
    }
    Ok(())
}

/// Get all causes sorted by error code for deterministic output.
/// Returns causes sorted by error code (JP001 < JP002 < ... < JP022).
fn get_sorted_causes(causes: &std::collections::HashSet<PanicCause>) -> Vec<&PanicCause> {
    let mut sorted: Vec<_> = causes.iter().collect();
    sorted.sort_by_key(|c| c.error_code());
    sorted
}

/// Format all causes as a string, e.g., "[JP001/panic: explicit_panic] [JP006/unwrap: unwrap_failed]"
fn format_causes(causes: &[&PanicCause]) -> String {
    causes
        .iter()
        .map(|c| format!("[{}/{}: {}]", c.error_code(), c.id(), c.description()))
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
    use crate::call_tree::AnalysisResult;
    use crate::panic_cause::PanicCause;
    use crate::string_tables::NAME_NONE;
    use std::collections::HashSet;

    /// Create a test point with a file path index.
    /// The file path index should correspond to an entry in the file_paths vec
    /// used with the test result.
    fn make_test_point(
        _name: &str,
        file: u16,
        line: u16,
        causes: Vec<PanicCause>,
    ) -> CrateCodePoint {
        CrateCodePoint {
            name: 0,
            file,
            line,
            column: 5,
            causes: causes.into_iter().collect(),
            children: vec![],
            is_direct_panic: true,
            called_function: NAME_NONE,
        }
    }

    fn make_test_result(
        code_points: Vec<CrateCodePoint>,
        file_paths: Vec<String>,
    ) -> AnalysisResult {
        AnalysisResult {
            project_name: "test_project".to_string(),
            project_root: "/test/project".to_string(),
            code_points,
            file_paths,
            name_paths: vec!["test_func".to_string()],
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
        causes.insert(PanicCause::Unwrap);
        let sorted = get_sorted_causes(&causes);
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].error_code(), "JP006");
    }

    #[test]
    fn test_get_sorted_causes_multiple() {
        let mut causes = HashSet::new();
        causes.insert(PanicCause::Unwrap); // JP006
        causes.insert(PanicCause::BoundsCheck); // JP002
        causes.insert(PanicCause::ExplicitPanic); // JP001
        let sorted = get_sorted_causes(&causes);
        assert_eq!(sorted.len(), 3);
        // Should be sorted by error code
        assert_eq!(sorted[0].error_code(), "JP001");
        assert_eq!(sorted[1].error_code(), "JP002");
        assert_eq!(sorted[2].error_code(), "JP006");
    }

    #[test]
    fn test_format_causes_empty() {
        let causes: Vec<&PanicCause> = vec![];
        let formatted = format_causes(&causes);
        assert_eq!(formatted, "");
    }

    #[test]
    fn test_format_causes_single() {
        let cause = PanicCause::Unwrap;
        let causes = vec![&cause];
        let formatted = format_causes(&causes);
        assert_eq!(formatted, "[JP006/unwrap: unwrap_failed]");
    }

    #[test]
    fn test_format_causes_multiple() {
        let cause1 = PanicCause::ExplicitPanic;
        let cause2 = PanicCause::BoundsCheck;
        let causes = vec![&cause1, &cause2];
        let formatted = format_causes(&causes);
        assert_eq!(
            formatted,
            "[JP001/panic: explicit_panic] [JP002/bounds: index_out_of_bounds]"
        );
    }

    // Tests using write_text_output to capture output

    #[test]
    fn test_write_text_output_empty() {
        let result = make_test_result(vec![], vec![]);
        let mut output = Vec::new();
        write_text_output(&mut output, &result, false, false, true, false).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("No panics in crate"));
        assert!(output_str.contains("Summary"));
        assert!(output_str.contains("test_project"));
        assert!(output_str.contains("Panic points: 0"));
    }

    #[test]
    fn test_write_text_output_summary_only() {
        let file_paths = vec!["src/main.rs".to_string()];
        let result = make_test_result(
            vec![make_test_point("test", 0, 10, vec![PanicCause::Unwrap])],
            file_paths,
        );
        let mut output = Vec::new();
        write_text_output(&mut output, &result, false, true, true, false).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // Should have summary but no panic points listing
        assert!(output_str.contains("Summary"));
        assert!(output_str.contains("Panic points: 1"));
        assert!(!output_str.contains("Panic code points in crate"));
    }

    #[test]
    fn test_write_text_output_flat_format() {
        let file_paths = vec!["src/main.rs".to_string()];
        let result = make_test_result(
            vec![make_test_point(
                "test_func",
                0,
                42,
                vec![PanicCause::Unwrap],
            )],
            file_paths,
        );
        let mut output = Vec::new();
        // no_hyperlinks=true forces flat format
        write_text_output(&mut output, &result, false, false, true, false).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("Panic code points in crate"));
        assert!(output_str.contains("src/main.rs:42:5"));
        assert!(output_str.contains("[JP006/unwrap: unwrap_failed]"));
        assert!(output_str.contains("= help:"));
    }

    #[test]
    fn test_write_text_output_with_children() {
        let file_paths = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        let child = CrateCodePoint {
            name: 0,
            file: 1,
            line: 20,
            column: 1,
            causes: vec![PanicCause::BoundsCheck].into_iter().collect(),
            children: vec![],
            is_direct_panic: true,
            called_function: NAME_NONE,
        };
        let parent = CrateCodePoint {
            name: 1,
            file: 0,
            line: 10,
            column: 1,
            causes: HashSet::new(),
            children: vec![child],
            is_direct_panic: false,
            called_function: 0,
        };
        let result = AnalysisResult {
            project_name: "test_project".to_string(),
            project_root: "/test/project".to_string(),
            code_points: vec![parent],
            file_paths,
            name_paths: vec!["child_func".to_string(), "parent_func".to_string()],
        };
        let mut output = Vec::new();
        write_text_output(&mut output, &result, true, false, true, false).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("src/main.rs:10:1"));
        assert!(output_str.contains("src/lib.rs:20:1"));
        assert!(output_str.contains("[JP002/bounds: index_out_of_bounds]"));
    }

    #[test]
    fn test_write_text_output_with_warning() {
        let file_paths = vec!["src/math.rs".to_string()];
        let result = make_test_result(
            vec![make_test_point(
                "overflow_func",
                0,
                5,
                vec![PanicCause::ArithmeticOverflow("add")],
            )],
            file_paths,
        );
        let mut output = Vec::new();
        write_text_output(&mut output, &result, false, false, true, false).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("[JP003/overflow: arithmetic_overflow]"));
        assert!(output_str.contains("= warning:"));
        assert!(output_str.contains("overflow-checks"));
    }

    #[test]
    fn test_write_flat_point_multiple_causes() {
        let file_paths = vec!["src/main.rs".to_string()];
        let name_paths = vec!["test".to_string()];
        let mut causes = HashSet::new();
        causes.insert(PanicCause::Unwrap);
        causes.insert(PanicCause::BoundsCheck);
        let point = CrateCodePoint {
            name: 0,
            file: 0,
            line: 10,
            column: 1,
            causes,
            children: vec![],
            is_direct_panic: true,
            called_function: NAME_NONE,
        };
        let mut output = Vec::new();
        write_flat_point(
            &mut output,
            &point,
            &file_paths,
            &name_paths,
            Some(Path::new("/test")),
            true,
        )
        .unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // Both causes should appear, sorted by error code
        assert!(output_str.contains("[JP002/bounds: index_out_of_bounds]"));
        assert!(output_str.contains("[JP006/unwrap: unwrap_failed]"));
    }

    #[test]
    fn test_write_directory_tree_format() {
        let file_paths = vec![
            "/test/src/main.rs".to_string(),
            "/test/src/lib.rs".to_string(),
        ];
        let name_paths = vec!["test_func".to_string()];
        let points = vec![
            make_test_point("func1", 0, 10, vec![PanicCause::Todo]),
            make_test_point("func2", 1, 20, vec![PanicCause::Unreachable]),
        ];
        let mut output = Vec::new();
        write_directory_tree(
            &mut output,
            &points,
            &file_paths,
            &name_paths,
            Some(Path::new("/test")),
            None,
            true,
            false,
            true, // include_children
        )
        .unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // Files should be present
        assert!(output_str.contains("main.rs"));
        assert!(output_str.contains("lib.rs"));
        // Causes should be present
        assert!(output_str.contains("[JP014/todo: todo_reached]"));
        assert!(output_str.contains("[JP012/unreachable: unreachable_reached]"));
    }

    #[test]
    fn test_write_text_output_indirect_panic() {
        let file_paths = vec!["src/main.rs".to_string()];
        let point = CrateCodePoint {
            name: 0,
            file: 0,
            line: 10,
            column: 1,
            causes: vec![PanicCause::Unwrap].into_iter().collect(),
            children: vec![],
            is_direct_panic: false,
            called_function: 1,
        };
        let result = AnalysisResult {
            project_name: "test_project".to_string(),
            project_root: "/test/project".to_string(),
            code_points: vec![point],
            file_paths,
            name_paths: vec!["caller".to_string(), "parse_config".to_string()],
        };
        let mut output = Vec::new();
        write_text_output(&mut output, &result, false, false, true, false).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // Indirect panic should mention the called function
        assert!(output_str.contains("parse_config"));
    }

    #[test]
    fn test_write_text_output_tree_with_children() {
        // Test that tree=true shows children in flat format
        let file_paths = vec!["src/main.rs".to_string(), "src/child.rs".to_string()];
        let child = CrateCodePoint {
            name: 0,
            file: 1,
            line: 5,
            column: 1,
            causes: vec![PanicCause::ExplicitPanic].into_iter().collect(),
            children: vec![],
            is_direct_panic: true,
            called_function: NAME_NONE,
        };
        let parent = CrateCodePoint {
            name: 1,
            file: 0,
            line: 10,
            column: 1,
            causes: std::collections::HashSet::new(),
            children: vec![child],
            is_direct_panic: false,
            called_function: NAME_NONE,
        };
        let result = make_test_result(vec![parent], file_paths);
        let mut output = Vec::new();
        // Params: tree, summary_only, no_hyperlinks, is_tty
        // tree=true (include_children), summary_only=false, no_hyperlinks=true, is_tty=false
        write_text_output(&mut output, &result, true, false, true, false).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // With include_children=true, children should be shown (uses └── format)
        assert!(
            output_str.contains("child.rs"),
            "Expected child.rs in output:\n{}",
            output_str
        );
        assert!(output_str.contains("[JP001/panic: explicit_panic]"));
    }

    #[test]
    fn test_write_text_output_no_tree_hides_children() {
        // Test that tree=false hides children
        let file_paths = vec!["src/main.rs".to_string(), "src/child.rs".to_string()];
        let child = CrateCodePoint {
            name: 0,
            file: 1,
            line: 5,
            column: 1,
            causes: vec![PanicCause::ExplicitPanic].into_iter().collect(),
            children: vec![],
            is_direct_panic: true,
            called_function: NAME_NONE,
        };
        let parent = CrateCodePoint {
            name: 1,
            file: 0,
            line: 10,
            column: 1,
            causes: std::collections::HashSet::new(),
            children: vec![child],
            is_direct_panic: false,
            called_function: NAME_NONE,
        };
        let result = make_test_result(vec![parent], file_paths);
        let mut output = Vec::new();
        // Params: tree, summary_only, no_hyperlinks, is_tty
        // tree=false, summary_only=false, no_hyperlinks=true, is_tty=false
        write_text_output(&mut output, &result, false, false, true, false).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // With tree=false, children should NOT be shown
        assert!(!output_str.contains("child.rs"));
        // Parent should still be shown
        assert!(output_str.contains("main.rs"));
    }

    #[test]
    fn test_write_flat_child_recursive() {
        // Test flat format with nested children
        let file_paths = vec![
            "src/top.rs".to_string(),
            "src/middle.rs".to_string(),
            "src/deep.rs".to_string(),
        ];
        let name_paths = vec![
            "grandchild".to_string(),
            "child".to_string(),
            "parent".to_string(),
        ];
        let grandchild = CrateCodePoint {
            name: 0,
            file: 2,
            line: 1,
            column: 1,
            causes: vec![PanicCause::Todo].into_iter().collect(),
            children: vec![],
            is_direct_panic: true,
            called_function: NAME_NONE,
        };
        let child = CrateCodePoint {
            name: 1,
            file: 1,
            line: 5,
            column: 1,
            causes: std::collections::HashSet::new(),
            children: vec![grandchild],
            is_direct_panic: false,
            called_function: NAME_NONE,
        };
        let parent = CrateCodePoint {
            name: 2,
            file: 0,
            line: 10,
            column: 1,
            causes: std::collections::HashSet::new(),
            children: vec![child],
            is_direct_panic: false,
            called_function: NAME_NONE,
        };

        let mut output = Vec::new();
        write_flat_point(
            &mut output,
            &parent,
            &file_paths,
            &name_paths,
            Some(Path::new("/test")),
            true,
        )
        .unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // All levels should be present
        assert!(output_str.contains("top.rs"));
        assert!(output_str.contains("middle.rs"));
        assert!(output_str.contains("deep.rs"));
        assert!(output_str.contains("[JP014/todo: todo_reached]"));
    }

    #[test]
    fn test_write_directory_tree_multiple_dirs() {
        // Test tree format with multiple directories
        let file_paths = vec![
            "/test/src/a/file1.rs".to_string(),
            "/test/src/b/file2.rs".to_string(),
            "/test/src/a/file3.rs".to_string(),
        ];
        let name_paths = vec!["test_func".to_string()];
        let points = vec![
            make_test_point("func1", 0, 10, vec![PanicCause::Todo]),
            make_test_point("func2", 1, 20, vec![PanicCause::Unreachable]),
            make_test_point("func3", 2, 30, vec![PanicCause::Unimplemented]),
        ];
        let mut output = Vec::new();
        write_directory_tree(
            &mut output,
            &points,
            &file_paths,
            &name_paths,
            Some(Path::new("/test")),
            None,
            true,
            false,
            true,
        )
        .unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // All files should be present
        assert!(output_str.contains("file1.rs"));
        assert!(output_str.contains("file2.rs"));
        assert!(output_str.contains("file3.rs"));
        // Directory markers
        assert!(output_str.contains("src/a/"));
        assert!(output_str.contains("src/b/"));
    }

    #[test]
    fn test_write_directory_tree_root_level_files() {
        // Test tree format with files at root level (no directory)
        let file_paths = vec!["/test/main.rs".to_string(), "/test/lib.rs".to_string()];
        let name_paths = vec!["test_func".to_string()];
        let points = vec![
            make_test_point("func1", 0, 10, vec![PanicCause::Todo]),
            make_test_point("func2", 1, 20, vec![PanicCause::Unreachable]),
        ];
        let mut output = Vec::new();
        write_directory_tree(
            &mut output,
            &points,
            &file_paths,
            &name_paths,
            Some(Path::new("/test")),
            None,
            true,
            false,
            true,
        )
        .unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // Files should be present without directory prefix
        assert!(output_str.contains("main.rs"));
        assert!(output_str.contains("lib.rs"));
    }

    #[test]
    fn test_format_causes_with_three_causes() {
        let causes = vec![
            &PanicCause::Unwrap,
            &PanicCause::BoundsCheck,
            &PanicCause::ExplicitPanic,
        ];
        let result = format_causes(&causes);
        assert!(result.contains("[JP001/panic:"));
        assert!(result.contains("[JP002/bounds:"));
        assert!(result.contains("[JP006/unwrap:"));
    }

    #[test]
    fn test_get_display_path_no_roots() {
        let result = get_display_path("src/main.rs", None, None);
        assert_eq!(result, "src/main.rs");
    }

    #[test]
    fn test_make_absolute_relative_path() {
        let result = make_absolute("src/lib.rs", Some(Path::new("/project")));
        assert_eq!(result, "/project/src/lib.rs");
    }

    #[test]
    fn test_format_location_with_column() {
        assert_eq!(format_location("src/main.rs", 42, 10), "src/main.rs:42:10");
    }

    #[test]
    fn test_format_location_column_zero() {
        // DWARF "left edge" — column unspecified, omit from output
        assert_eq!(format_location("src/main.rs", 42, 0), "src/main.rs:42");
    }

    #[test]
    fn test_format_location_column_one() {
        // Column 1 is a real column, include it
        assert_eq!(format_location("src/main.rs", 42, 1), "src/main.rs:42:1");
    }
}
