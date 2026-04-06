//! Jonesy library - analyze Rust binaries for panic paths
//!
//! This library provides the core functionality for analyzing Rust binaries
//! to find code paths that can lead to panics.

pub mod analysis;
pub mod analysis_cache;
pub mod args;
pub mod call_graph;
pub mod call_tree;
pub mod cargo;
pub mod config;
pub mod crate_line_table;
pub mod debug_info;
pub mod file_watcher;
pub mod full_line_table;
pub mod function_index;
pub mod heuristics;
pub mod inline_allows;
pub mod library_call_graph;
pub mod lsp;
pub(crate) mod object_line_table;
pub mod output;
pub mod panic_cause;
pub mod project_context;
pub mod string_tables;
pub mod sym;
