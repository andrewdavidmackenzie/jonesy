//! HTML output format for jonesy analysis results.
//!
//! This module generates self-contained HTML reports with inline CSS
//! for viewing panic analysis results in a browser.

use crate::args::VERSION;
use crate::call_tree::{AnalysisResult, CrateCodePoint};
use crate::json_output::WorkspaceResult;

/// Generate HTML output from analysis results.
///
/// When `summary_only` is true, the panic points section will be empty.
/// When `tree` is true, includes the full call tree with children.
pub fn generate_html_output(result: &AnalysisResult, tree: bool, summary_only: bool) -> String {
    let summary = result.summary();
    let code_points = if summary_only {
        &[][..]
    } else {
        &result.code_points[..]
    };

    generate_html_report(
        &result.project_name,
        &result.project_root,
        summary.panic_points(),
        summary.files_affected(),
        code_points,
        tree,
    )
}

/// Generate a complete HTML report from analysis results.
fn generate_html_report(
    project_name: &str,
    project_root: &str,
    panic_points: usize,
    files_affected: usize,
    code_points: &[CrateCodePoint],
    include_tree: bool,
) -> String {
    let mut html = String::new();

    // HTML header with inline CSS
    html.push_str(&format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Jonesy Report - {}</title>
    <style>
        :root {{
            --bg-color: #1a1a2e;
            --card-bg: #16213e;
            --text-color: #eee;
            --text-muted: #888;
            --accent: #e94560;
            --accent-light: #ff6b6b;
            --link-color: #4dabf7;
            --success: #51cf66;
            --warning: #fcc419;
            --border: #2a2a4a;
        }}
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
            background: var(--bg-color);
            color: var(--text-color);
            line-height: 1.6;
            padding: 2rem;
        }}
        .container {{ max-width: 1200px; margin: 0 auto; }}
        header {{
            border-bottom: 2px solid var(--accent);
            padding-bottom: 1rem;
            margin-bottom: 2rem;
        }}
        h1 {{
            font-size: 2rem;
            font-weight: 600;
            display: flex;
            align-items: center;
            gap: 0.5rem;
        }}
        h1 .logo {{ color: var(--accent); }}
        .subtitle {{ color: var(--text-muted); margin-top: 0.25rem; }}
        .summary {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 1rem;
            margin-bottom: 2rem;
        }}
        .stat-card {{
            background: var(--card-bg);
            border-radius: 8px;
            padding: 1.25rem;
            border: 1px solid var(--border);
        }}
        .stat-card .label {{ color: var(--text-muted); font-size: 0.875rem; }}
        .stat-card .value {{ font-size: 1.75rem; font-weight: 700; margin-top: 0.25rem; }}
        .stat-card .value.zero {{ color: var(--success); }}
        .stat-card .value.nonzero {{ color: var(--accent); }}
        .section-title {{
            font-size: 1.25rem;
            font-weight: 600;
            margin: 1.5rem 0 1rem;
            padding-bottom: 0.5rem;
            border-bottom: 1px solid var(--border);
        }}
        .panic-list {{ list-style: none; }}
        .panic-item {{
            background: var(--card-bg);
            border: 1px solid var(--border);
            border-radius: 8px;
            margin-bottom: 0.75rem;
            overflow: hidden;
        }}
        .panic-header {{
            padding: 1rem;
            display: flex;
            flex-wrap: wrap;
            gap: 0.5rem 1rem;
            align-items: baseline;
        }}
        .file-link {{
            color: var(--link-color);
            text-decoration: none;
            font-family: 'SF Mono', Monaco, 'Courier New', monospace;
            font-size: 0.9rem;
        }}
        .file-link:hover {{ text-decoration: underline; }}
        .function-name {{
            color: var(--text-muted);
            font-size: 0.875rem;
        }}
        .cause-badge {{
            display: inline-block;
            background: var(--accent);
            color: white;
            padding: 0.2rem 0.5rem;
            border-radius: 4px;
            font-size: 0.75rem;
            font-weight: 500;
        }}
        .cause-details {{
            background: rgba(0,0,0,0.2);
            padding: 0.75rem 1rem;
            border-top: 1px solid var(--border);
            font-size: 0.875rem;
        }}
        .suggestion {{
            color: var(--text-muted);
            margin-top: 0.25rem;
        }}
        .suggestion::before {{ content: "Suggestion: "; font-weight: 500; }}
        .warning {{
            color: var(--warning);
            margin-top: 0.25rem;
        }}
        .warning::before {{ content: "Warning: "; font-weight: 500; }}
        .children {{
            margin-left: 1.5rem;
            padding: 0.5rem 0 0.5rem 1rem;
            border-left: 2px solid var(--border);
        }}
        .child-item {{
            padding: 0.5rem 0;
        }}
        .no-panics {{
            text-align: center;
            padding: 3rem;
            color: var(--success);
            font-size: 1.25rem;
        }}
        footer {{
            margin-top: 3rem;
            padding-top: 1rem;
            border-top: 1px solid var(--border);
            color: var(--text-muted);
            font-size: 0.875rem;
            text-align: center;
        }}
        @media (max-width: 600px) {{
            body {{ padding: 1rem; }}
            h1 {{ font-size: 1.5rem; }}
        }}
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1><span class="logo">Jonesy</span> Panic Report</h1>
            <p class="subtitle">{} &mdash; {}</p>
        </header>
"#,
        escape_html(project_name),
        escape_html(project_name),
        escape_html(project_root)
    ));

    // Summary section
    let value_class = if panic_points == 0 { "zero" } else { "nonzero" };
    html.push_str(&format!(
        r#"        <div class="summary">
            <div class="stat-card">
                <div class="label">Panic Points</div>
                <div class="value {}">{}</div>
            </div>
            <div class="stat-card">
                <div class="label">Files Affected</div>
                <div class="value {}">{}</div>
            </div>
            <div class="stat-card">
                <div class="label">Jonesy Version</div>
                <div class="value" style="font-size: 1rem;">{}</div>
            </div>
        </div>
"#,
        value_class, panic_points, value_class, files_affected, VERSION
    ));

    // Panic points section
    // Only show "No panic points" message if truly no panics were found
    // (not when code_points is empty due to --summary-only mode)
    if code_points.is_empty() && panic_points == 0 {
        html.push_str(
            r#"        <div class="no-panics">No panic points found in crate!</div>
"#,
        );
    } else if !code_points.is_empty() {
        html.push_str(
            r#"        <h2 class="section-title">Panic Points</h2>
        <ul class="panic-list">
"#,
        );
        for point in code_points {
            render_panic_point(&mut html, point, project_root, include_tree, 0);
        }
        html.push_str("        </ul>\n");
    }

    // Footer
    html.push_str(&format!(
        r#"        <footer>
            Generated by <a href="https://github.com/andrewdavidmackenzie/jonesy" style="color: var(--link-color);">Jonesy</a> v{}
        </footer>
    </div>
</body>
</html>
"#,
        VERSION
    ));

    html
}

/// Render a single panic point as HTML.
fn render_panic_point(
    html: &mut String,
    point: &CrateCodePoint,
    project_root: &str,
    include_tree: bool,
    depth: usize,
) {
    let indent = "            ".repeat(depth + 1);
    let absolute_path = make_absolute_path(&point.file, project_root);
    let file_url = escape_html(&format!("file://{}", absolute_path));
    let location = if let Some(col) = point.column {
        format!("{}:{}:{}", point.file, point.line, col)
    } else {
        format!("{}:{}", point.file, point.line)
    };

    // Get all causes sorted by error code
    let sorted_causes: Vec<_> = {
        let mut causes: Vec<_> = point.causes.iter().collect();
        causes.sort_by_key(|c| c.error_code());
        causes
    };

    html.push_str(&format!(
        r#"{}<li class="panic-item">
{}    <div class="panic-header">
{}        <a href="{}" class="file-link">{}</a>
{}        <span class="function-name">in {}</span>
"#,
        indent,
        indent,
        indent,
        file_url,
        escape_html(&location),
        indent,
        escape_html(&point.name)
    ));

    // Show all causes as badges
    for c in &sorted_causes {
        html.push_str(&format!(
            "{}        <a href=\"{}\" class=\"cause-badge\" target=\"_blank\" rel=\"noopener\">{}: {}</a>\n",
            indent,
            escape_html(&c.docs_url()),
            escape_html(c.error_code()),
            escape_html(c.description())
        ));
    }

    html.push_str(&format!("{}    </div>\n", indent));

    // Cause details (show for primary cause only)
    if let Some(c) = sorted_causes.first() {
        let suggestion =
            c.format_suggestion(point.is_direct_panic, point.called_function.as_deref());
        let warning = c.release_warning();
        if !suggestion.is_empty() || warning.is_some() {
            html.push_str(&format!("{}    <div class=\"cause-details\">\n", indent));
            if !suggestion.is_empty() {
                html.push_str(&format!(
                    "{}        <div class=\"suggestion\">{}</div>\n",
                    indent,
                    escape_html(&suggestion)
                ));
            }
            if let Some(w) = warning {
                html.push_str(&format!(
                    "{}        <div class=\"warning\">{}</div>\n",
                    indent,
                    escape_html(w)
                ));
            }
            html.push_str(&format!("{}    </div>\n", indent));
        }
    }

    // Children (if tree mode)
    if include_tree && !point.children.is_empty() {
        html.push_str(&format!("{}    <div class=\"children\">\n", indent));
        for child in &point.children {
            render_child_point(html, child, project_root, depth + 1);
        }
        html.push_str(&format!("{}    </div>\n", indent));
    }

    html.push_str(&format!("{}</li>\n", indent));
}

/// Render a child panic point (simplified view).
fn render_child_point(html: &mut String, point: &CrateCodePoint, project_root: &str, depth: usize) {
    let indent = "            ".repeat(depth + 1);
    let absolute_path = make_absolute_path(&point.file, project_root);
    let file_url = escape_html(&format!("file://{}", absolute_path));
    let location = if let Some(col) = point.column {
        format!("{}:{}:{}", point.file, point.line, col)
    } else {
        format!("{}:{}", point.file, point.line)
    };

    // Get all causes sorted by error code
    let sorted_causes: Vec<_> = {
        let mut causes: Vec<_> = point.causes.iter().collect();
        causes.sort_by_key(|c| c.error_code());
        causes
    };

    html.push_str(&format!(
        r#"{}    <div class="child-item">
{}        <a href="{}" class="file-link">{}</a>
{}        <span class="function-name">in {}</span>
"#,
        indent,
        indent,
        file_url,
        escape_html(&location),
        indent,
        escape_html(&point.name)
    ));

    // Show all causes as badges
    for c in &sorted_causes {
        html.push_str(&format!(
            "{}        <a href=\"{}\" class=\"cause-badge\" target=\"_blank\" rel=\"noopener\">{}: {}</a>\n",
            indent,
            escape_html(&c.docs_url()),
            escape_html(c.error_code()),
            escape_html(c.description())
        ));
    }

    html.push_str(&format!("{}    </div>\n", indent));

    // Recurse for nested children
    if !point.children.is_empty() {
        html.push_str(&format!("{}    <div class=\"children\">\n", indent));
        for child in &point.children {
            render_child_point(html, child, project_root, depth + 1);
        }
        html.push_str(&format!("{}    </div>\n", indent));
    }
}

/// Make a file path absolute using the project root.
fn make_absolute_path(file: &str, project_root: &str) -> String {
    if file.starts_with('/') {
        file.to_string()
    } else {
        format!("{}/{}", project_root.trim_end_matches('/'), file)
    }
}

/// Escape HTML special characters.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Generate HTML output from workspace analysis results.
///
/// When `summary_only` is true, the panic points sections will be empty.
/// When `tree` is true, includes the full call tree with children.
pub fn generate_workspace_html_output(
    result: &WorkspaceResult,
    tree: bool,
    summary_only: bool,
) -> String {
    let mut html = String::new();

    // HTML header with inline CSS (extended for workspace)
    html.push_str(&format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Jonesy Workspace Report</title>
    <style>
        :root {{
            --bg-color: #1a1a2e;
            --card-bg: #16213e;
            --text-color: #eee;
            --text-muted: #888;
            --accent: #e94560;
            --accent-light: #ff6b6b;
            --link-color: #4dabf7;
            --success: #51cf66;
            --warning: #fcc419;
            --border: #2a2a4a;
            --member-bg: #1e2a45;
        }}
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
            background: var(--bg-color);
            color: var(--text-color);
            line-height: 1.6;
            padding: 2rem;
        }}
        .container {{ max-width: 1200px; margin: 0 auto; }}
        header {{
            border-bottom: 2px solid var(--accent);
            padding-bottom: 1rem;
            margin-bottom: 2rem;
        }}
        h1 {{
            font-size: 2rem;
            font-weight: 600;
            display: flex;
            align-items: center;
            gap: 0.5rem;
        }}
        h1 .logo {{ color: var(--accent); }}
        .subtitle {{ color: var(--text-muted); margin-top: 0.25rem; }}
        .summary {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 1rem;
            margin-bottom: 2rem;
        }}
        .stat-card {{
            background: var(--card-bg);
            border-radius: 8px;
            padding: 1.25rem;
            border: 1px solid var(--border);
        }}
        .stat-card .label {{ color: var(--text-muted); font-size: 0.875rem; }}
        .stat-card .value {{ font-size: 1.75rem; font-weight: 700; margin-top: 0.25rem; }}
        .stat-card .value.zero {{ color: var(--success); }}
        .stat-card .value.nonzero {{ color: var(--accent); }}
        .section-title {{
            font-size: 1.25rem;
            font-weight: 600;
            margin: 1.5rem 0 1rem;
            padding-bottom: 0.5rem;
            border-bottom: 1px solid var(--border);
        }}
        .member-section {{
            background: var(--member-bg);
            border-radius: 8px;
            padding: 1.5rem;
            margin-bottom: 1.5rem;
            border: 1px solid var(--border);
        }}
        .member-section > summary {{
            list-style: none;
        }}
        .member-section > summary::-webkit-details-marker {{
            display: none;
        }}
        .member-section > summary::before {{
            content: "▼ ";
            font-size: 0.75rem;
            margin-right: 0.5rem;
            transition: transform 0.2s;
        }}
        .member-section:not([open]) > summary::before {{
            content: "▶ ";
        }}
        .member-header {{
            display: flex;
            justify-content: space-between;
            align-items: center;
            margin-bottom: 1rem;
            cursor: pointer;
        }}
        .member-name {{
            font-size: 1.1rem;
            font-weight: 600;
            color: var(--link-color);
        }}
        .member-path {{
            color: var(--text-muted);
            font-size: 0.875rem;
            font-family: 'SF Mono', Monaco, 'Courier New', monospace;
        }}
        .member-stats {{
            display: flex;
            gap: 1rem;
            font-size: 0.875rem;
        }}
        .member-stats .count {{ font-weight: 600; }}
        .member-stats .count.zero {{ color: var(--success); }}
        .member-stats .count.nonzero {{ color: var(--accent); }}
        .member-content {{
            margin-top: 1rem;
        }}
        .panic-list {{ list-style: none; }}
        .panic-item {{
            background: var(--card-bg);
            border: 1px solid var(--border);
            border-radius: 8px;
            margin-bottom: 0.75rem;
            overflow: hidden;
        }}
        .panic-header {{
            padding: 1rem;
            display: flex;
            flex-wrap: wrap;
            gap: 0.5rem 1rem;
            align-items: baseline;
        }}
        .file-link {{
            color: var(--link-color);
            text-decoration: none;
            font-family: 'SF Mono', Monaco, 'Courier New', monospace;
            font-size: 0.9rem;
        }}
        .file-link:hover {{ text-decoration: underline; }}
        .function-name {{
            color: var(--text-muted);
            font-size: 0.875rem;
        }}
        .cause-badge {{
            display: inline-block;
            background: var(--accent);
            color: white;
            padding: 0.2rem 0.5rem;
            border-radius: 4px;
            font-size: 0.75rem;
            font-weight: 500;
        }}
        .cause-details {{
            background: rgba(0,0,0,0.2);
            padding: 0.75rem 1rem;
            border-top: 1px solid var(--border);
            font-size: 0.875rem;
        }}
        .suggestion {{
            color: var(--text-muted);
            margin-top: 0.25rem;
        }}
        .suggestion::before {{ content: "Suggestion: "; font-weight: 500; }}
        .warning {{
            color: var(--warning);
            margin-top: 0.25rem;
        }}
        .warning::before {{ content: "Warning: "; font-weight: 500; }}
        .children {{
            margin-left: 1.5rem;
            padding: 0.5rem 0 0.5rem 1rem;
            border-left: 2px solid var(--border);
        }}
        .child-item {{
            padding: 0.5rem 0;
        }}
        .no-panics {{
            text-align: center;
            padding: 2rem;
            color: var(--success);
            font-size: 1rem;
        }}
        footer {{
            margin-top: 3rem;
            padding-top: 1rem;
            border-top: 1px solid var(--border);
            color: var(--text-muted);
            font-size: 0.875rem;
            text-align: center;
        }}
        @media (max-width: 600px) {{
            body {{ padding: 1rem; }}
            h1 {{ font-size: 1.5rem; }}
        }}
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1><span class="logo">Jonesy</span> Workspace Report</h1>
            <p class="subtitle">{}</p>
        </header>
"#,
        escape_html(&result.root)
    ));

    // Summary section
    let total_panic_points = result.total_summary.panic_points();
    let total_files = result.total_summary.files_affected();
    let value_class = if total_panic_points == 0 {
        "zero"
    } else {
        "nonzero"
    };
    html.push_str(&format!(
        r#"        <div class="summary">
            <div class="stat-card">
                <div class="label">Total Panic Points</div>
                <div class="value {}">{}</div>
            </div>
            <div class="stat-card">
                <div class="label">Total Files Affected</div>
                <div class="value {}">{}</div>
            </div>
            <div class="stat-card">
                <div class="label">Members Analyzed</div>
                <div class="value">{}</div>
            </div>
            <div class="stat-card">
                <div class="label">Jonesy Version</div>
                <div class="value" style="font-size: 1rem;">{}</div>
            </div>
        </div>
"#,
        value_class,
        total_panic_points,
        value_class,
        total_files,
        result.members.len(),
        VERSION
    ));

    // Members section
    html.push_str(
        r#"        <h2 class="section-title">Workspace Members</h2>
"#,
    );

    for member in &result.members {
        let member_panic_points = member.summary.panic_points();
        let member_files = member.summary.files_affected();
        let count_class = if member_panic_points == 0 {
            "zero"
        } else {
            "nonzero"
        };

        html.push_str(&format!(
            r#"        <details class="member-section" open>
            <summary class="member-header">
                <div>
                    <span class="member-name">{}</span>
                    <span class="member-path">{}</span>
                </div>
                <div class="member-stats">
                    <span><span class="count {}">{}</span> panic points</span>
                    <span><span class="count {}">{}</span> files</span>
                </div>
            </summary>
"#,
            escape_html(&member.name),
            escape_html(&member.path),
            count_class,
            member_panic_points,
            count_class,
            member_files
        ));

        // Member content
        let code_points = if summary_only {
            &[][..]
        } else {
            &member.code_points[..]
        };

        if code_points.is_empty() && member_panic_points == 0 {
            html.push_str(
                r#"            <div class="no-panics">No panic points found in this crate</div>
"#,
            );
        } else if !code_points.is_empty() {
            html.push_str(
                r#"            <div class="member-content">
                <ul class="panic-list">
"#,
            );
            for point in code_points {
                render_panic_point(&mut html, point, &result.root, tree, 1);
            }
            html.push_str(
                r#"                </ul>
            </div>
"#,
            );
        }

        html.push_str("        </details>\n");
    }

    // Footer
    html.push_str(&format!(
        r#"        <footer>
            Generated by <a href="https://github.com/andrewdavidmackenzie/jonesy" style="color: var(--link-color);">Jonesy</a> v{}
        </footer>
    </div>
</body>
</html>
"#,
        VERSION
    ));

    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call_tree::AnalysisSummary;
    use crate::json_output::WorkspaceMemberResult;
    use crate::panic_cause::PanicCause;
    use std::collections::HashSet;

    fn make_test_point(
        name: &str,
        file: &str,
        line: u32,
        causes: Vec<PanicCause>,
    ) -> CrateCodePoint {
        CrateCodePoint {
            name: name.to_string(),
            file: file.to_string(),
            line,
            column: Some(1),
            causes: causes.into_iter().collect(),
            children: vec![],
            is_direct_panic: true,
            called_function: None,
        }
    }

    fn make_test_result(code_points: Vec<CrateCodePoint>) -> AnalysisResult {
        AnalysisResult {
            project_name: "test_project".to_string(),
            project_root: "/test".to_string(),
            code_points,
        }
    }

    #[test]
    fn test_generate_html_output_empty() {
        let result = make_test_result(vec![]);
        let html = generate_html_output(&result, false, false);

        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("test_project"));
        assert!(html.contains("No panic points found"));
        assert!(html.contains("</html>"));
    }

    #[test]
    fn test_generate_html_output_with_panic_points() {
        let result = make_test_result(vec![make_test_point(
            "test_func",
            "src/main.rs",
            42,
            vec![PanicCause::Unwrap],
        )]);
        let html = generate_html_output(&result, false, false);

        assert!(html.contains("src/main.rs"));
        assert!(html.contains(":42:"));
        assert!(html.contains("test_func"));
        assert!(html.contains("JP006"));
        assert!(html.contains("unwrap() failed"));
    }

    #[test]
    fn test_generate_html_output_summary_only() {
        let result = make_test_result(vec![make_test_point(
            "test_func",
            "src/main.rs",
            42,
            vec![PanicCause::Unwrap],
        )]);
        let html = generate_html_output(&result, false, true);

        // Summary-only should have empty panic points list
        assert!(html.contains("Panic Points"));
        // But no individual panic point entries
        assert!(!html.contains("test_func"));
    }

    #[test]
    fn test_generate_html_output_with_tree() {
        let child = CrateCodePoint {
            name: "child_func".to_string(),
            file: "src/lib.rs".to_string(),
            line: 20,
            column: Some(5),
            causes: vec![PanicCause::BoundsCheck].into_iter().collect(),
            children: vec![],
            is_direct_panic: true,
            called_function: None,
        };
        let parent = CrateCodePoint {
            name: "parent_func".to_string(),
            file: "src/main.rs".to_string(),
            line: 10,
            column: Some(1),
            causes: HashSet::new(),
            children: vec![child],
            is_direct_panic: false,
            called_function: Some("child_func".to_string()),
        };
        let result = make_test_result(vec![parent]);
        let html = generate_html_output(&result, true, false);

        // Both parent and child should appear
        assert!(html.contains("parent_func"));
        assert!(html.contains("child_func"));
        assert!(html.contains("src/main.rs"));
        assert!(html.contains("src/lib.rs"));
    }

    #[test]
    fn test_generate_html_output_escapes_html() {
        let result = AnalysisResult {
            project_name: "test<script>alert('xss')</script>".to_string(),
            project_root: "/test".to_string(),
            code_points: vec![],
        };
        let html = generate_html_output(&result, false, false);

        // HTML should be escaped
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn test_generate_workspace_html_output() {
        let member = WorkspaceMemberResult {
            name: "crate_a".to_string(),
            path: "crate_a".to_string(),
            summary: AnalysisSummary::from_points(
                vec![("file".to_string(), 1)].into_iter().collect(),
                vec!["file".to_string()].into_iter().collect(),
            ),
            code_points: vec![make_test_point(
                "func",
                "crate_a/src/lib.rs",
                5,
                vec![PanicCause::Todo],
            )],
        };
        let workspace = WorkspaceResult {
            root: "/workspace".to_string(),
            members: vec![member],
            total_summary: AnalysisSummary::from_points(
                vec![("file".to_string(), 1)].into_iter().collect(),
                vec!["file".to_string()].into_iter().collect(),
            ),
        };

        let html = generate_workspace_html_output(&workspace, false, false);

        assert!(html.contains("Workspace Members"));
        assert!(html.contains("crate_a"));
        assert!(html.contains("JP014"));
        assert!(html.contains("todo!()"));
    }

    #[test]
    fn test_escape_html() {
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
        assert_eq!(escape_html("a & b"), "a &amp; b");
        assert_eq!(escape_html("\"quoted\""), "&quot;quoted&quot;");
        assert_eq!(escape_html("plain text"), "plain text");
    }

    #[test]
    fn test_html_structure() {
        let result = make_test_result(vec![]);
        let html = generate_html_output(&result, false, false);

        // Check HTML structure
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<html lang=\"en\">"));
        assert!(html.contains("<head>"));
        assert!(html.contains("<body>"));
        assert!(html.contains("</head>"));
        assert!(html.contains("</body>"));
        assert!(html.ends_with("</html>\n"));
    }

    #[test]
    fn test_html_includes_version() {
        let result = make_test_result(vec![]);
        let html = generate_html_output(&result, false, false);

        assert!(html.contains(VERSION));
    }
}
