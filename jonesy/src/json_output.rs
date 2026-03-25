//! JSON output format for jonesy analysis results.
//!
//! This module generates machine-readable JSON output directly from AnalysisResult.

use crate::args::VERSION;
use crate::call_tree::{AnalysisResult, AnalysisSummary, CrateCodePoint};
use serde_json::{Value, json};

/// Schema version for JSON output format (single crate)
/// 1.2: Changed "cause" to "causes" array to show all causes at a code point
pub const JSON_SCHEMA_VERSION: &str = "1.2";

/// Schema version for workspace JSON output format
/// 1.2: Changed "cause" to "causes" array to show all causes at a code point
pub const JSON_WORKSPACE_SCHEMA_VERSION: &str = "1.2";

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
    // Get all causes sorted by error code for determinism
    let causes_json: Vec<Value> = {
        let mut causes: Vec<_> = point.causes.iter().collect();
        causes.sort_by_key(|c| c.error_code());
        causes
            .iter()
            .map(|c| {
                let suggestion =
                    c.format_suggestion(point.is_direct_panic, point.called_function.as_deref());
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
            .collect()
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
    if !causes_json.is_empty() {
        obj["causes"] = json!(causes_json);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call_tree::AnalysisSummary;
    use crate::panic_cause::PanicCause;
    use std::collections::HashSet;

    fn make_test_code_point(
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

    #[test]
    fn test_make_absolute_path_already_absolute() {
        let path = make_absolute_path("/home/user/project/src/main.rs", "/home/user/project");
        assert_eq!(path, "/home/user/project/src/main.rs");
    }

    #[test]
    fn test_make_absolute_path_relative() {
        let path = make_absolute_path("src/main.rs", "/home/user/project");
        assert_eq!(path, "/home/user/project/src/main.rs");
    }

    #[test]
    fn test_make_absolute_path_with_trailing_slash() {
        let path = make_absolute_path("src/main.rs", "/home/user/project/");
        assert_eq!(path, "/home/user/project/src/main.rs");
    }

    #[test]
    fn test_generate_json_output_empty() {
        let result = AnalysisResult {
            project_name: "test_project".to_string(),
            project_root: "/test".to_string(),
            code_points: vec![],
        };

        let json = generate_json_output(&result, false, false).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["version"], JSON_SCHEMA_VERSION);
        assert_eq!(parsed["project"]["name"], "test_project");
        assert_eq!(parsed["project"]["root"], "/test");
        assert_eq!(parsed["summary"]["panic_points"], 0);
        assert_eq!(parsed["summary"]["files_affected"], 0);
        assert!(parsed["panic_points"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_generate_json_output_with_code_points() {
        let result = AnalysisResult {
            project_name: "test_project".to_string(),
            project_root: "/test".to_string(),
            code_points: vec![make_test_code_point(
                "test_func",
                "src/main.rs",
                10,
                vec![PanicCause::UnwrapNone],
            )],
        };

        let json = generate_json_output(&result, false, false).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["summary"]["panic_points"], 1);
        assert_eq!(parsed["summary"]["files_affected"], 1);

        let points = parsed["panic_points"].as_array().unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0]["file"], "/test/src/main.rs");
        assert_eq!(points[0]["line"], 10);
        assert_eq!(points[0]["function"], "test_func");
    }

    #[test]
    fn test_generate_json_output_summary_only() {
        let result = AnalysisResult {
            project_name: "test_project".to_string(),
            project_root: "/test".to_string(),
            code_points: vec![make_test_code_point(
                "test_func",
                "src/main.rs",
                10,
                vec![PanicCause::UnwrapNone],
            )],
        };

        let json = generate_json_output(&result, false, true).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        // Summary should still show counts
        assert_eq!(parsed["summary"]["panic_points"], 1);
        // But panic_points array should be empty
        assert!(parsed["panic_points"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_generate_json_output_with_children() {
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

        let result = AnalysisResult {
            project_name: "test".to_string(),
            project_root: "/test".to_string(),
            code_points: vec![parent],
        };

        // Without tree flag, children should be empty
        let json = generate_json_output(&result, false, false).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        let points = parsed["panic_points"].as_array().unwrap();
        assert!(
            points[0].get("children").is_none()
                || points[0]["children"].as_array().unwrap().is_empty()
        );

        // With tree flag, children should be included
        let json_tree = generate_json_output(&result, true, false).unwrap();
        let parsed_tree: Value = serde_json::from_str(&json_tree).unwrap();
        let points_tree = parsed_tree["panic_points"].as_array().unwrap();
        let children = points_tree[0]["children"].as_array().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["function"], "child_func");
    }

    #[test]
    fn test_code_point_to_json_includes_causes() {
        let point = make_test_code_point(
            "test",
            "src/main.rs",
            10,
            vec![PanicCause::UnwrapNone, PanicCause::UnwrapErr],
        );

        let json = code_point_to_json(&point, "/test", false);
        let causes = json["causes"].as_array().unwrap();
        assert_eq!(causes.len(), 2);
        // Causes should be sorted by error code
        assert_eq!(causes[0]["code"], "JP006");
        assert_eq!(causes[1]["code"], "JP007");
    }

    #[test]
    fn test_workspace_json_output() {
        let member = WorkspaceMemberResult {
            name: "crate_a".to_string(),
            path: "crate_a".to_string(),
            summary: AnalysisSummary::from_points(
                vec![("file1".to_string(), 1), ("file1".to_string(), 2)]
                    .into_iter()
                    .collect(),
                vec!["file1".to_string()].into_iter().collect(),
            ),
            code_points: vec![make_test_code_point(
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
                vec![("file1".to_string(), 1), ("file1".to_string(), 2)]
                    .into_iter()
                    .collect(),
                vec!["file1".to_string()].into_iter().collect(),
            ),
        };

        let json = generate_workspace_json_output(&workspace, false, false).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["version"], JSON_WORKSPACE_SCHEMA_VERSION);
        assert_eq!(parsed["workspace"]["root"], "/workspace");
        assert_eq!(parsed["summary"]["total_panic_points"], 2);
        assert_eq!(parsed["summary"]["members_analyzed"], 1);

        let members = parsed["workspace"]["members"].as_array().unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0]["name"], "crate_a");
    }

    #[test]
    fn test_workspace_json_output_summary_only() {
        let member = WorkspaceMemberResult {
            name: "crate_a".to_string(),
            path: "crate_a".to_string(),
            summary: AnalysisSummary::from_points(
                vec![("file".to_string(), 1)].into_iter().collect(),
                vec!["file".to_string()].into_iter().collect(),
            ),
            code_points: vec![make_test_code_point(
                "func",
                "src/lib.rs",
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

        let json = generate_workspace_json_output(&workspace, false, true).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        let members = parsed["workspace"]["members"].as_array().unwrap();
        // panic_points should be empty in summary-only mode
        assert!(members[0]["panic_points"].as_array().unwrap().is_empty());
    }
}
