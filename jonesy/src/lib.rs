//! Jonesy library - analyze Rust binaries for panic paths
//!
//! This library provides the core functionality for analyzing Rust binaries
//! to find code paths that can lead to panics.

/// Saturating conversion from `u64` to `u16`.
/// Returns `u16::MAX` for values above 65535, avoiding silent wrapping
/// (where e.g. line 65536 would wrap to 0 and be treated as "absent").
pub(crate) fn to_u16_saturating(v: u64) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}

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
        // With u16 line field: 8+8+4+4+2 = 26 bytes, padded to 28 (4-byte alignment from u32 fields)
        assert!(
            mem::size_of::<FunctionInfo>() <= 32,
            "FunctionInfo should be <= 32 bytes (uses interned indices, u16 line), got {}",
            mem::size_of::<FunctionInfo>()
        );

        // CallerInfo uses u16 file index instead of Option<Box<str>>, keeping it compact.
        // Natural size is 56 bytes; align(64) pads to 64 bytes.
        // Cache line packing:
        //   x86_64 (64B lines): 1 per line, no straddling
        //   aarch64 (128B lines on Apple Silicon): 2 per line, no straddling
        // Using align(128) on aarch64 would waste 72B per entry for 600K+ entries.
        assert_eq!(
            mem::size_of::<CallerInfo>(),
            64,
            "CallerInfo should be exactly 64 bytes (u16 file index, u16 line/column, align(64))"
        );
        assert_eq!(
            mem::align_of::<CallerInfo>(),
            64,
            "CallerInfo should be 64-byte aligned (1 per x86 line, 2 per aarch64 line)"
        );

        // Line table entries should be compact since there are millions of them
        // With u16 line/column: CrateLineEntry = 8+2+2 = 12 bytes
        // FullLineEntry = 8+4+2+2 = 16 bytes
        assert!(
            mem::size_of::<CrateLineEntry>() <= 16,
            "CrateLineEntry should be <= 16 bytes, got {}",
            mem::size_of::<CrateLineEntry>()
        );
        assert!(
            mem::size_of::<FullLineEntry>() <= 16,
            "FullLineEntry should be <= 16 bytes, got {}",
            mem::size_of::<FullLineEntry>()
        );
    }
}
