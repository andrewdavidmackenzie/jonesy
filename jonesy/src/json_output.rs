//! JSON output format for jonesy analysis results.
//!
//! This module provides structures and serialization for machine-readable
//! JSON output of panic analysis results.

use crate::args::VERSION;
use crate::call_tree::{AnalysisResult, CrateCodePoint};
use crate::panic_cause::PanicCause;
use serde::Serialize;

/// Schema version for JSON output format
pub const JSON_SCHEMA_VERSION: &str = "1.0";

/// Root structure for JSON output (serialization format)
#[derive(Debug, Serialize)]
struct JsonOutput {
    version: String,
    jonesy_version: String,
    project: ProjectInfo,
    summary: Summary,
    panic_points: Vec<JsonPanicPoint>,
}

/// Project information (serialization format)
#[derive(Debug, Serialize)]
struct ProjectInfo {
    name: String,
    root: String,
}

/// Summary statistics (serialization format)
#[derive(Debug, Serialize)]
struct Summary {
    panic_points: usize,
    files_affected: usize,
}

/// A single panic point (serialization format)
#[derive(Debug, Serialize)]
struct JsonPanicPoint {
    file: String,
    line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    column: Option<u32>,
    function: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cause: Option<JsonPanicCause>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<JsonPanicPoint>,
}

/// Panic cause information (serialization format)
#[derive(Debug, Serialize)]
struct JsonPanicCause {
    #[serde(rename = "type")]
    cause_type: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggestion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
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

    let panic_points = if summary_only {
        Vec::new()
    } else {
        result
            .code_points
            .iter()
            .map(|p| JsonPanicPoint::from_code_point(p, tree))
            .collect()
    };

    let output = JsonOutput {
        version: JSON_SCHEMA_VERSION.to_string(),
        jonesy_version: VERSION.to_string(),
        project: ProjectInfo {
            name: result.project_name.clone(),
            root: result.project_root.clone(),
        },
        summary: Summary {
            panic_points: summary.panic_points(),
            files_affected: summary.files_affected(),
        },
        panic_points,
    };

    serde_json::to_string_pretty(&output)
}

impl JsonPanicPoint {
    fn from_code_point(point: &CrateCodePoint, include_children: bool) -> Self {
        let cause = {
            let mut causes: Vec<_> = point.causes.iter().collect();
            causes.sort_by_key(|c| c.description());
            causes.first().map(|c| JsonPanicCause::from_cause(c))
        };

        JsonPanicPoint {
            file: point.file.clone(),
            line: point.line,
            column: point.column,
            function: point.name.clone(),
            cause,
            children: if include_children {
                point
                    .children
                    .iter()
                    .map(|c| JsonPanicPoint::from_code_point(c, true))
                    .collect()
            } else {
                Vec::new()
            },
        }
    }
}

impl JsonPanicCause {
    fn from_cause(cause: &PanicCause) -> Self {
        let suggestion = cause.suggestion();
        JsonPanicCause {
            cause_type: cause.id().to_string(),
            description: cause.description().to_string(),
            suggestion: if suggestion.is_empty() {
                None
            } else {
                Some(suggestion.to_string())
            },
            warning: cause.release_warning().map(str::to_string),
        }
    }
}
