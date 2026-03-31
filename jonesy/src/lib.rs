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
pub mod inline_allows;
#[cfg(target_os = "macos")]
pub mod library_call_graph;
pub mod lsp;
#[cfg(target_os = "macos")]
pub(crate) mod object_line_table;
pub mod output;
pub mod panic_cause;
pub mod project_context;
pub mod string_tables;
#[cfg(target_os = "macos")]
pub mod sym;
