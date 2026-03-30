//! Jonesy library - analyze Rust binaries for panic paths
//!
//! This library provides the core functionality for analyzing Rust binaries
//! to find code paths that can lead to panics.

#[cfg(target_os = "macos")]
pub mod analysis;
pub mod analysis_cache;
pub mod args;
#[cfg(target_os = "macos")]
pub mod call_graph;
pub mod call_tree;
pub mod cargo;
pub mod config;
#[cfg(target_os = "macos")]
pub mod crate_line_table;
#[cfg(target_os = "macos")]
pub mod debug_info;
pub mod file_watcher;
#[cfg(target_os = "macos")]
pub mod full_line_table;
#[cfg(target_os = "macos")]
pub mod function_index;
pub mod heuristics;
pub mod html_output;
pub mod inline_allows;
pub mod json_output;
#[cfg(target_os = "macos")]
pub mod library_call_graph;
pub mod lsp;
#[cfg(target_os = "macos")]
pub mod object_line_table;
pub mod panic_cause;
pub mod project_context;
#[cfg(target_os = "macos")]
pub mod string_tables;
#[cfg(target_os = "macos")]
pub mod sym;
pub mod text_output;
