//! Jonesy library - analyze Rust binaries for panic paths
//!
//! This library provides the core functionality for analyzing Rust binaries
//! to find code paths that can lead to panics.

pub mod analysis;
pub mod analysis_cache;
pub(crate) mod arch;
pub mod args;
pub mod binary_format;
pub mod call_graph;
pub mod call_tree;
pub mod cargo;
pub mod config;
pub mod crate_line_table;
pub mod debug_info;
pub(crate) mod elf_relocations;
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

#[cfg(test)]
mod struct_size_tests {
    use std::mem;

    /// Audit struct sizes to detect accidental growth and identify optimisation opportunities.
    /// Run with `cargo test -p jonesy struct_size_audit -- --nocapture` to see sizes.
    #[test]
    fn struct_size_audit() {
        use crate::call_graph::CallerInfo;
        use crate::call_tree::{CallTreeNode, CrateCodePoint};
        use crate::crate_line_table::CrateLineEntry;
        use crate::full_line_table::FullLineEntry;
        use crate::function_index::FunctionInfo;
        use crate::panic_cause::PanicCause;

        let sizes = [
            (
                "FunctionInfo",
                mem::size_of::<FunctionInfo>(),
                mem::align_of::<FunctionInfo>(),
            ),
            (
                "CrateLineEntry",
                mem::size_of::<CrateLineEntry>(),
                mem::align_of::<CrateLineEntry>(),
            ),
            (
                "FullLineEntry",
                mem::size_of::<FullLineEntry>(),
                mem::align_of::<FullLineEntry>(),
            ),
            (
                "CallerInfo",
                mem::size_of::<CallerInfo>(),
                mem::align_of::<CallerInfo>(),
            ),
            (
                "CallTreeNode",
                mem::size_of::<CallTreeNode>(),
                mem::align_of::<CallTreeNode>(),
            ),
            (
                "CrateCodePoint",
                mem::size_of::<CrateCodePoint>(),
                mem::align_of::<CrateCodePoint>(),
            ),
            (
                "PanicCause",
                mem::size_of::<PanicCause>(),
                mem::align_of::<PanicCause>(),
            ),
        ];

        eprintln!("\n=== Struct Size Audit ===");
        eprintln!("{:<20} {:>6} {:>6}", "Struct", "Size", "Align");
        eprintln!("{}", "-".repeat(34));
        for (name, size, align) in &sizes {
            eprintln!("{:<20} {:>5}B {:>5}B", name, size, align);
        }
        eprintln!();

        // Assert key struct sizes don't regress
        // FunctionInfo is deliberately compact (Copy, cache-friendly)
        assert_eq!(
            mem::size_of::<FunctionInfo>(),
            32,
            "FunctionInfo should stay at 32 bytes (uses interned indices)"
        );

        // CallerInfo is compact (64 bytes, 64-byte aligned for cache-line packing)
        assert_eq!(
            mem::size_of::<CallerInfo>(),
            64,
            "CallerInfo should be exactly 64 bytes (merged file, Box<str>, u16 line/column)"
        );
        assert_eq!(
            mem::align_of::<CallerInfo>(),
            64,
            "CallerInfo should be 64-byte aligned (2 per 128B cache line, no straddling)"
        );

        // Line table entries should be compact since there are millions of them
        assert!(
            mem::size_of::<CrateLineEntry>() <= 24,
            "CrateLineEntry should be <= 24 bytes, got {}",
            mem::size_of::<CrateLineEntry>()
        );
        assert!(
            mem::size_of::<FullLineEntry>() <= 24,
            "FullLineEntry should be <= 24 bytes, got {}",
            mem::size_of::<FullLineEntry>()
        );
    }
}
