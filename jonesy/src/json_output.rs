//! JSON output format for jonesy analysis results.
//!
//! This module provides structures and serialization for machine-readable
//! JSON output of panic analysis results.

use crate::args::VERSION;
use crate::call_tree::CrateCodePoint;
use crate::panic_cause::PanicCause;
use serde::Serialize;

/// Schema version for JSON output format
pub const JSON_SCHEMA_VERSION: &str = "1.0";

/// Root structure for JSON output
#[derive(Debug, Serialize)]
pub struct JsonOutput {
    /// Schema version for compatibility
    pub version: String,
    /// Jonesy version that produced this output
    pub jonesy_version: String,
    /// Project information
    pub project: ProjectInfo,
    /// Summary statistics
    pub summary: Summary,
    /// List of panic points found
    pub panic_points: Vec<JsonPanicPoint>,
}

/// Project information
#[derive(Debug, Serialize)]
pub struct ProjectInfo {
    /// Name of the project/crate
    pub name: String,
    /// Root path of the project
    pub root: String,
}

/// Summary statistics
#[derive(Debug, Serialize)]
pub struct Summary {
    /// Total number of panic points
    pub panic_points: usize,
    /// Number of unique files affected
    pub files_affected: usize,
}

/// A single panic point in JSON format
#[derive(Debug, Serialize)]
pub struct JsonPanicPoint {
    /// Source file path
    pub file: String,
    /// Line number
    pub line: u32,
    /// Column number (if available)
    pub column: Option<u32>,
    /// Function name
    pub function: String,
    /// Panic cause information (if detected)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<JsonPanicCause>,
    /// Child panic points (called from this point)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<JsonPanicPoint>,
}

/// Panic cause information
#[derive(Debug, Serialize)]
pub struct JsonPanicCause {
    /// Type identifier for the panic cause
    #[serde(rename = "type")]
    pub cause_type: String,
    /// Human-readable description
    pub description: String,
    /// Suggestion for fixing the issue
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

impl JsonOutput {
    /// Create a new JSON output structure
    pub fn new(project_name: String, project_root: String) -> Self {
        Self {
            version: JSON_SCHEMA_VERSION.to_string(),
            jonesy_version: VERSION.to_string(),
            project: ProjectInfo {
                name: project_name,
                root: project_root,
            },
            summary: Summary {
                panic_points: 0,
                files_affected: 0,
            },
            panic_points: Vec::new(),
        }
    }

    /// Set the summary statistics
    pub fn with_summary(mut self, panic_points: usize, files_affected: usize) -> Self {
        self.summary = Summary {
            panic_points,
            files_affected,
        };
        self
    }

    /// Set the panic points
    pub fn with_panic_points(mut self, points: Vec<JsonPanicPoint>) -> Self {
        self.panic_points = points;
        self
    }

    /// Serialize to JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

impl From<&CrateCodePoint> for JsonPanicPoint {
    fn from(point: &CrateCodePoint) -> Self {
        // Get the primary cause (first one, sorted for determinism)
        let cause = {
            let mut causes: Vec<_> = point.causes.iter().collect();
            causes.sort_by_key(|c| c.description());
            causes.first().map(|c| JsonPanicCause::from(*c))
        };

        JsonPanicPoint {
            file: point.file.clone(),
            line: point.line,
            column: point.column,
            function: point.name.clone(),
            cause,
            children: point.children.iter().map(JsonPanicPoint::from).collect(),
        }
    }
}

impl From<&PanicCause> for JsonPanicCause {
    fn from(cause: &PanicCause) -> Self {
        let suggestion = cause.suggestion();
        JsonPanicCause {
            cause_type: cause.id().to_string(),
            description: cause.description().to_string(),
            suggestion: if suggestion.is_empty() {
                None
            } else {
                Some(suggestion.to_string())
            },
        }
    }
}

/// Convert a list of CrateCodePoints to JSON panic points
pub fn convert_to_json_points(points: &[CrateCodePoint]) -> Vec<JsonPanicPoint> {
    points.iter().map(JsonPanicPoint::from).collect()
}
