//! HTML output format for jonesy analysis results.
//!
//! This module generates self-contained HTML reports with inline CSS
//! for viewing panic analysis results in a browser.

use crate::args::VERSION;
use crate::call_tree::CrateCodePoint;

/// Generate a complete HTML report from analysis results.
pub fn generate_html_report(
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
    if code_points.is_empty() {
        html.push_str(
            r#"        <div class="no-panics">No panic points found in crate!</div>
"#,
        );
    } else {
        html.push_str(
            r#"        <h2 class="section-title">Panic Points</h2>
        <ul class="panic-list">
"#,
        );
        for point in code_points {
            render_panic_point(&mut html, point, include_tree, 0);
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
fn render_panic_point(html: &mut String, point: &CrateCodePoint, include_tree: bool, depth: usize) {
    let indent = "            ".repeat(depth + 1);
    let file_url = format!("file://{}", point.file);
    let location = if let Some(col) = point.column {
        format!("{}:{}:{}", point.file, point.line, col)
    } else {
        format!("{}:{}", point.file, point.line)
    };

    // Get primary cause
    let cause = {
        let mut causes: Vec<_> = point.causes.iter().collect();
        causes.sort_by_key(|c| c.description());
        causes.first().copied()
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

    if let Some(c) = cause {
        html.push_str(&format!(
            "{}        <span class=\"cause-badge\">{}</span>\n",
            indent,
            escape_html(c.description())
        ));
    }

    html.push_str(&format!("{}    </div>\n", indent));

    // Cause details
    if let Some(c) = cause {
        let suggestion = c.suggestion();
        let warning = c.release_warning();
        if !suggestion.is_empty() || warning.is_some() {
            html.push_str(&format!("{}    <div class=\"cause-details\">\n", indent));
            if !suggestion.is_empty() {
                html.push_str(&format!(
                    "{}        <div class=\"suggestion\">{}</div>\n",
                    indent,
                    escape_html(suggestion)
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
            render_child_point(html, child, depth + 1);
        }
        html.push_str(&format!("{}    </div>\n", indent));
    }

    html.push_str(&format!("{}</li>\n", indent));
}

/// Render a child panic point (simplified view).
fn render_child_point(html: &mut String, point: &CrateCodePoint, depth: usize) {
    let indent = "            ".repeat(depth + 1);
    let file_url = format!("file://{}", point.file);
    let location = if let Some(col) = point.column {
        format!("{}:{}:{}", point.file, point.line, col)
    } else {
        format!("{}:{}", point.file, point.line)
    };

    let cause = {
        let mut causes: Vec<_> = point.causes.iter().collect();
        causes.sort_by_key(|c| c.description());
        causes.first().copied()
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

    if let Some(c) = cause {
        html.push_str(&format!(
            "{}        <span class=\"cause-badge\">{}</span>\n",
            indent,
            escape_html(c.description())
        ));
    }

    html.push_str(&format!("{}    </div>\n", indent));

    // Recurse for nested children
    if !point.children.is_empty() {
        html.push_str(&format!("{}    <div class=\"children\">\n", indent));
        for child in &point.children {
            render_child_point(html, child, depth + 1);
        }
        html.push_str(&format!("{}    </div>\n", indent));
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
