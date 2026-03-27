//! Jonesy library - analyze Rust binaries for panic paths
//!
//! This library provides the core functionality for analyzing Rust binaries
//! to find code paths that can lead to panics.

#[cfg(target_os = "macos")]
pub mod analysis;
pub mod analysis_cache;
pub mod args;
pub mod call_tree;
pub mod cargo;
pub mod config;
pub mod file_watcher;
pub mod heuristics;
pub mod html_output;
pub mod inline_allows;
pub mod json_output;
pub mod lsp;
pub mod panic_cause;
#[cfg(target_os = "macos")]
pub mod sym;
pub mod text_output;
