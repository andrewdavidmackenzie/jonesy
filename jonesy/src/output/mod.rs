//! Output formats for jonesy analysis results.
//!
//! This module contains the `OutputFormat` enum and submodules for each
//! output format: text, JSON, and HTML.

pub mod html;
pub mod json;
pub mod text;

/// Output format and display configuration for analysis results.
/// Consolidates format selection with display options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable terminal output
    Text {
        /// Show full call tree instead of just crate code points
        tree: bool,
        /// Only show summary, not detailed panic points
        summary_only: bool,
        /// Suppress progress messages
        quiet: bool,
        /// Use terminal hyperlinks for file paths
        hyperlinks: bool,
    },
    /// Machine-readable JSON output (implies quiet)
    Json {
        /// Show full call tree (children) instead of flat list
        tree: bool,
        /// Only include summary, not detailed panic points
        summary_only: bool,
    },
    /// Self-contained HTML report (implies quiet)
    Html {
        /// Show full call tree instead of flat list
        tree: bool,
        /// Only include summary, not detailed panic points
        summary_only: bool,
    },
}

impl Default for OutputFormat {
    fn default() -> Self {
        OutputFormat::Text {
            tree: false,
            summary_only: false,
            quiet: false,
            hyperlinks: true,
        }
    }
}

impl OutputFormat {
    /// Create a text output format with the given options
    pub fn text(tree: bool, summary_only: bool, quiet: bool, hyperlinks: bool) -> Self {
        OutputFormat::Text {
            tree,
            summary_only,
            quiet,
            hyperlinks,
        }
    }

    /// Create a JSON output format with the given options
    pub fn json(tree: bool, summary_only: bool) -> Self {
        OutputFormat::Json { tree, summary_only }
    }

    /// Create an HTML output format with the given options
    pub fn html(tree: bool, summary_only: bool) -> Self {
        OutputFormat::Html { tree, summary_only }
    }

    /// Create a quiet text output format (for LSP/programmatic use)
    pub fn quiet() -> Self {
        OutputFormat::Text {
            tree: false,
            summary_only: false,
            quiet: true,
            hyperlinks: false,
        }
    }

    /// Returns true if this is JSON output
    pub fn is_json(&self) -> bool {
        matches!(self, OutputFormat::Json { .. })
    }

    /// Returns true if this is HTML output
    pub fn is_html(&self) -> bool {
        matches!(self, OutputFormat::Html { .. })
    }

    /// Returns true if this is text output
    pub fn is_text(&self) -> bool {
        matches!(self, OutputFormat::Text { .. })
    }

    /// Returns true if progress messages should be shown
    pub fn show_progress(&self) -> bool {
        match self {
            OutputFormat::Text {
                quiet,
                summary_only,
                ..
            } => !quiet && !summary_only,
            OutputFormat::Json { .. } | OutputFormat::Html { .. } => false,
        }
    }

    /// Returns true if only the summary should be shown (no panic point details)
    pub fn is_summary_only(&self) -> bool {
        match self {
            OutputFormat::Text { summary_only, .. }
            | OutputFormat::Json { summary_only, .. }
            | OutputFormat::Html { summary_only, .. } => *summary_only,
        }
    }

    /// Returns true if the full call tree should be shown
    pub fn show_tree(&self) -> bool {
        match self {
            OutputFormat::Text { tree, .. }
            | OutputFormat::Json { tree, .. }
            | OutputFormat::Html { tree, .. } => *tree,
        }
    }

    /// Returns true if hyperlinks should be used in output
    pub fn use_hyperlinks(&self) -> bool {
        match self {
            OutputFormat::Text { hyperlinks, .. } => *hyperlinks,
            OutputFormat::Json { .. } | OutputFormat::Html { .. } => false,
        }
    }
}
