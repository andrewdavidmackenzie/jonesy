//! JSON output format for jonesy analysis results.
//!
//! This module generates machine-readable JSON output directly from AnalysisResult.

use crate::args::VERSION;
use crate::call_tree::{AnalysisResult, AnalysisSummary, CrateCodePoint};
use serde_json::{Value, json};

/// Schema version for JSON output format (single crate)
pub const JSON_SCHEMA_VERSION: &str = "1.0";

/// Schema version for workspace JSON output format
pub const JSON_WORKSPACE_SCHEMA_VERSION: &str = "1.1";

/// A workspace member's analysis result for JSON/HTML output
pub struct WorkspaceMemberResult {
    /// Name of the member crate
    pub name: String,
    /// Relative path to the member crate
    pub path: String,
    /// Analysis summary for this member
    pub summary: AnalysisSummary,
    /// Panic code points found in this member
    pub code_points: Vec<CrateCodePoint>,
}

/// Complete workspace analysis results
pub struct WorkspaceResult {
    /// Root path of the workspace
    pub root: String,
    /// Results for each member crate
    pub members: Vec<WorkspaceMemberResult>,
    /// Aggregate summary across all members
    pub total_summary: AnalysisSummary,
}

/// Generate JSON output from analysis results.
///
/// When `summary_only` is true, the panic_points array will be empty.
/// When `tree` is true, includes the full call tree with children.
pub fn generate_json_output(
    result: &AnalysisResult,
    tree: bool,
    summary_only: bool,
) -> Result<String, serde_json::Error> {
    let summary = result.summary();

    let panic_points: Vec<Value> = if summary_only {
        Vec::new()
    } else {
        result
            .code_points
            .iter()
            .map(|p| code_point_to_json(p, &result.project_root, tree))
            .collect()
    };

    let output = json!({
        "version": JSON_SCHEMA_VERSION,
        "jonesy_version": VERSION,
        "project": {
            "name": result.project_name,
            "root": result.project_root,
        },
        "summary": {
            "panic_points": summary.panic_points(),
            "files_affected": summary.files_affected(),
        },
        "panic_points": panic_points,
    });

    serde_json::to_string_pretty(&output)
}

/// Convert a CrateCodePoint to JSON value.
fn code_point_to_json(point: &CrateCodePoint, project_root: &str, include_children: bool) -> Value {
    // Get primary cause (sorted for determinism)
    let cause = {
        let mut causes: Vec<_> = point.causes.iter().collect();
        causes.sort_by_key(|c| c.description());
        causes.first().map(|c| {
            let suggestion = c.suggestion();
            let mut cause_obj = json!({
                "code": c.error_code(),
                "type": c.id(),
                "description": c.description(),
                "docs_url": c.docs_url(),
            });
            if !suggestion.is_empty() {
                cause_obj["suggestion"] = json!(suggestion);
            }
            if let Some(warning) = c.release_warning() {
                cause_obj["warning"] = json!(warning);
            }
            cause_obj
        })
    };

    let children: Vec<Value> = if include_children {
        point
            .children
            .iter()
            .map(|c| code_point_to_json(c, project_root, true))
            .collect()
    } else {
        Vec::new()
    };

    let mut obj = json!({
        "file": make_absolute_path(&point.file, project_root),
        "line": point.line,
        "function": point.name,
    });

    if let Some(col) = point.column {
        obj["column"] = json!(col);
    }
    if let Some(c) = cause {
        obj["cause"] = c;
    }
    if !children.is_empty() {
        obj["children"] = json!(children);
    }

    obj
}

/// Make a file path absolute using the project root.
fn make_absolute_path(file: &str, project_root: &str) -> String {
    if file.starts_with('/') {
        file.to_string()
    } else {
        format!("{}/{}", project_root.trim_end_matches('/'), file)
    }
}

/// Generate JSON output from workspace analysis results.
///
/// When `summary_only` is true, the panic_points arrays will be empty.
/// When `tree` is true, includes the full call tree with children.
pub fn generate_workspace_json_output(
    result: &WorkspaceResult,
    tree: bool,
    summary_only: bool,
) -> Result<String, serde_json::Error> {
    let members: Vec<Value> = result
        .members
        .iter()
        .map(|m| {
            let panic_points: Vec<Value> = if summary_only {
                Vec::new()
            } else {
                m.code_points
                    .iter()
                    .map(|p| code_point_to_json(p, &result.root, tree))
                    .collect()
            };

            json!({
                "name": m.name,
                "path": m.path,
                "summary": {
                    "panic_points": m.summary.panic_points(),
                    "files_affected": m.summary.files_affected(),
                },
                "panic_points": panic_points,
            })
        })
        .collect();

    let output = json!({
        "version": JSON_WORKSPACE_SCHEMA_VERSION,
        "jonesy_version": VERSION,
        "workspace": {
            "root": result.root,
            "members": members,
        },
        "summary": {
            "total_panic_points": result.total_summary.panic_points(),
            "total_files_affected": result.total_summary.files_affected(),
            "members_analyzed": result.members.len(),
        },
    });

    serde_json::to_string_pretty(&output)
}
