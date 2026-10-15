use crate::arch;
use crate::binary_format::BinaryRef;
use crate::crate_line_table::CrateLineTable;
use crate::full_line_table::FullLineTable;
use crate::function_index::{FunctionIndex, get_functions_from_dwarf, load_dwarf_sections};
use crate::project_context::ProjectContext;
use crate::string_tables::{FILE_NONE, FilePathInterner};
use crate::sym::SymbolIndex;
use dashmap::DashMap;
use goblin::mach::MachO;
use rayon::prelude::*;
use rustc_demangle::demangle;
use std::collections::HashMap;

/// Compact information about a call site.
///
/// Uses `Cow<str>` for the function name (24 bytes), a `u16` file index into
/// a shared `FilePathInterner` (2 bytes), and `u16` for line/column numbers.
/// Natural size is 56 bytes (with `repr(C)` padding).
///
/// The `file` field merges what was previously `caller_file` and `file`:
/// at construction time, the more specific call-site file is preferred,
/// falling back to the function declaration file.
///
/// A value of 0 in `line` or `column` means the field is absent.
/// A value of `FILE_NONE` in `file` means no file path is known.
///
/// ## Alignment
/// Aligned to the platform's cache line size for optimal packing:
/// - **x86_64** (64-byte cache lines): `align(64)` — 1 per cache line, no straddling
/// - **aarch64** (128-byte cache lines on Apple Silicon): `align(64)` — 2 per cache
///   line, no straddling. Using `align(128)` would waste 72 bytes per entry.
///
/// ## Field sizes
/// With `u16` indices for both name and file (replacing `Cow<str>` and `Option<Box<str>>`),
/// the natural size is 28 bytes. `align(64)` pads to 64 bytes but eliminates all heap
/// allocations — each `CallerInfo` is fully stack-resident.
#[derive(Debug, Clone, Copy)]
#[repr(C, align(64))]
pub struct CallerInfo {
    /// Function name index into a shared name interner (`FilePathInterner`).
    /// `NAME_NONE` means no name is known.
    pub caller_name: u16,
    /// Source file index into a shared `FilePathInterner`.
    /// `FILE_NONE` means no file path is known.
    pub file: u16,
    /// Start address of the calling function
    pub caller_start_address: u64,
    /// Address of the calling instruction
    pub call_site_addr: u64,
    /// Line number of the calling instruction (0 = unknown)
    pub line: u16,
    /// Column number of the calling instruction (0 = unknown)
    pub column: u16,
}

/// Get a named section's address and data from a binary.
/// This is a standalone helper that wraps BinaryRef::find_section for callers
/// that only have a raw MachO reference. For new code, use BinaryRef::find_section directly.
#[allow(dead_code)]
fn get_section_by_name<'a>(
    macho: &'a MachO<'a>,
    buffer: &'a [u8],
    name: &str,
) -> Option<(u64, &'a [u8])> {
    let binary_ref = BinaryRef::MachO(macho);
    binary_ref.find_section(buffer, name)
}

/// Pre-computed call graph mapping target addresses to their callers.
/// This allows O(1) lookup instead of O(n) scanning for each query.
/// All string data (function names, file paths) is stored as interned `u16`
/// indices, making the graph fully owned with no lifetime dependencies.
pub struct CallGraph {
    /// Maps target_addr -> list of CallerInfo
    edges: HashMap<u64, Vec<CallerInfo>>,
}

impl CallGraph {
    /// Build a call graph by scanning all instructions once (no debug info).
    /// Uses parallel disassembly and parallel processing for faster analysis.
    /// Function names from the symbol index are interned into `name_interner`.
    pub fn build(
        binary: &BinaryRef,
        buffer: &[u8],
        symbol_index: Option<&SymbolIndex>,
        name_interner: &FilePathInterner,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // Build PLT map for ELF binaries (for resolving PLT stubs to actual functions)
        let plt_map = if let BinaryRef::Elf(elf) = binary {
            arch::plt::build_map(elf, buffer)
        } else {
            HashMap::new()
        };

        let text_section_name = binary.text_section_name();
        let Some((text_addr, text_data)) = binary.find_section(buffer, text_section_name) else {
            return Ok(Self {
                edges: HashMap::new(),
            });
        };

        // Parallel disassembly - divides text section into chunks
        #[cfg(target_arch = "aarch64")]
        let insn_data = arch::parallel_disassemble(text_data, text_addr);

        #[cfg(target_arch = "x86_64")]
        let insn_data = arch::parallel_disassemble(text_data, text_addr, binary, buffer);

        // Process bl instructions in parallel (the expensive part is function lookup).
        // Pre-size the DashMap to reduce rehashing (~17 MB saved for large binaries).
        // Unique call targets are typically ~1/3 of total call instructions (measured ratio: 1:2.8 to 1:3.8 across different binary sizes).
        let edges: DashMap<u64, Vec<CallerInfo>> = DashMap::with_capacity(insn_data.len() / 3);

        insn_data.par_iter().for_each(|data| {
            if let Some(call_target) = data.call_target
                && let Some((func_addr, func_name)) =
                    symbol_index.and_then(|idx| idx.find_containing(data.address))
            {
                // Resolve PLT stub to actual function address (for ELF dylib support)
                let resolved_target = arch::plt::resolve_stub(call_target, &plt_map);

                edges.entry(resolved_target).or_default().push(CallerInfo {
                    caller_name: name_interner.intern(func_name),
                    file: FILE_NONE,
                    caller_start_address: func_addr,
                    call_site_addr: data.address,
                    line: 0,
                    column: u16::MAX,
                });
            }
        });

        // Convert DashMap to HashMap and sort each caller list for deterministic ordering
        let mut edges: HashMap<u64, Vec<CallerInfo>> = edges.into_iter().collect();
        for callers in edges.values_mut() {
            callers.sort_unstable_by_key(|c| c.call_site_addr);
        }
        Ok(Self { edges })
    }

    /// Build a call graph with debug info enrichment.
    /// Uses parallel disassembly and parallel processing for faster analysis.
    /// Function names and file paths are interned into the shared interners.
    #[allow(clippy::too_many_arguments)]
    pub fn build_with_debug_info(
        binary: &BinaryRef,
        binary_buffer: &[u8],
        debug_binary: &BinaryRef,
        debug_buffer: &[u8],
        show_timings: bool,
        symbol_index: Option<&SymbolIndex>,
        project_context: &ProjectContext,
        file_interner: &FilePathInterner,
        name_interner: &FilePathInterner,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        use std::time::Instant;

        // Build PLT map for ELF binaries (for resolving PLT stubs to actual functions)
        let plt_map = if let BinaryRef::Elf(elf) = binary {
            arch::plt::build_map(elf, binary_buffer)
        } else {
            HashMap::new()
        };

        // Pre-load DWARF info once (shared across threads)
        let step = Instant::now();
        let (functions, inlined, strings) =
            get_functions_from_dwarf(debug_binary, debug_buffer, project_context.project_root())?;
        let num_functions = functions.len();
        let num_inlined = inlined.len();
        // Build function index for O(log n) lookups instead of O(n) linear search
        let function_index = FunctionIndex::new_with_inlined(functions, inlined, strings);
        if show_timings {
            eprintln!(
                "    [cg timing] get_functions_from_dwarf: {:?} ({} functions, {} inlined)",
                step.elapsed(),
                num_functions,
                num_inlined
            );
        }

        let step = Instant::now();
        let dwarf = load_dwarf_sections(debug_binary, debug_buffer)?;
        if show_timings {
            eprintln!("    [cg timing] load_dwarf_sections: {:?}", step.elapsed());
        }

        let text_section_name = binary.text_section_name();
        let Some((text_addr, text_data)) = binary.find_section(binary_buffer, text_section_name)
        else {
            return Ok(Self {
                edges: HashMap::new(),
            });
        };
        if show_timings {
            eprintln!(
                "    [cg timing] text section size: {} bytes",
                text_data.len()
            );
        }

        // Parallel disassembly - divides text section into chunks
        let step = Instant::now();
        #[cfg(target_arch = "aarch64")]
        let insn_data = arch::parallel_disassemble(text_data, text_addr);

        #[cfg(target_arch = "x86_64")]
        let insn_data = arch::parallel_disassemble(text_data, text_addr, binary, binary_buffer);
        if show_timings {
            // insn_data contains only BL/B branch instructions (not all instructions)
            eprintln!(
                "    [cg timing] scan for branch instructions: {:?} ({} found)",
                step.elapsed(),
                insn_data.len()
            );
        }

        // Build both line tables in a single pass (saves iterating DWARF twice)
        let step = Instant::now();
        let (crate_line_table, full_line_table) =
            FullLineTable::build_both(&dwarf, project_context, text_data.len())?;
        if show_timings {
            eprintln!(
                "    [cg timing] build line tables: {:?} (crate: {} entries, full: {} entries)",
                step.elapsed(),
                crate_line_table.len(),
                full_line_table.len()
            );
        }

        // Process bl instructions in parallel.
        // Pre-size the DashMap to reduce rehashing.
        let step = Instant::now();
        let edges: DashMap<u64, Vec<CallerInfo>> = DashMap::with_capacity(insn_data.len() / 3);

        insn_data.par_iter().for_each(|data| {
            if let Some(_call_target) = data.call_target
                && let Some((target, caller_info)) = process_instruction_data_with_crate_table(
                    data,
                    &function_index,
                    &full_line_table,
                    &crate_line_table,
                    symbol_index,
                    project_context,
                    file_interner,
                    name_interner,
                )
            {
                // Resolve PLT stub to actual function address (for ELF dylib support)
                let resolved_target = arch::plt::resolve_stub(target, &plt_map);
                edges.entry(resolved_target).or_default().push(caller_info);
            }
        });
        if show_timings {
            eprintln!("    [cg timing] process instructions: {:?}", step.elapsed(),);
        }

        // Convert DashMap to HashMap and sort each caller list for deterministic ordering
        let mut edges: HashMap<u64, Vec<CallerInfo>> = edges.into_iter().collect();
        for callers in edges.values_mut() {
            callers.sort_unstable_by_key(|c| c.call_site_addr);
        }
        Ok(Self { edges })
    }

    /// Get all callers of a target address.
    /// Returns a slice reference to avoid cloning CallerInfo instances.
    pub fn get_callers(&self, target_addr: u64) -> &[CallerInfo] {
        self.edges
            .get(&target_addr)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Create an empty call graph.
    pub fn empty() -> Self {
        Self {
            edges: HashMap::new(),
        }
    }
}

/// Process a single instruction and extract call information using pre-built
/// line tables for fast O(log n) lookups. Falls back to symbol table lookup
/// if DWARF doesn't contain the function. Function names and file paths are
/// interned into the shared interners (no heap String allocation).
#[allow(clippy::too_many_arguments)]
fn process_instruction_data_with_crate_table(
    data: &arch::InsnData,
    function_index: &FunctionIndex,
    full_line_table: &FullLineTable,
    crate_line_table: &CrateLineTable,
    symbol_index: Option<&SymbolIndex>,
    project_context: &ProjectContext,
    file_interner: &FilePathInterner,
    name_interner: &FilePathInterner,
) -> Option<(u64, CallerInfo)> {
    let call_target = data.call_target?;

    // Find the function containing this call - try DWARF first, fall back to symbol table
    // Uses O(log n) binary search instead of O(n) linear search
    if let Some(func) = function_index.find_containing(data.address) {
        // Found in DWARF - use full debug info
        // Get source location using O(log n) binary search on pre-built table
        // All file paths are borrowed (&str) until final interning
        let func_file: Option<&str> = function_index.get_file(func);
        let func_line = function_index.get_line(func);
        // Keep a reference for fallback file resolution
        let decl_file = func_file;
        let (mut file, mut line, mut column): (Option<&str>, u16, u16) =
            match (func_file, func_line) {
                (Some(f), l) if l > 0 => {
                    // Fast path: use DWARF function info directly
                    // Look up column from the full line table at the function start address,
                    // but only use it if the line table entry matches our file and line
                    let (lt_file, lt_line, lt_column) =
                        full_line_table.get_source_location(func.start_address);
                    let col = if lt_line == l && lt_file == Some(f) {
                        lt_column
                    } else {
                        0 // DWARF left edge — column unspecified
                    };
                    (Some(f), l, col)
                }
                (func_file, func_line) => {
                    // Use pre-built full line table for O(log n) lookup
                    let (line_file, line_line, line_column) =
                        full_line_table.get_source_location(func.start_address);
                    (
                        func_file.or(line_file),
                        if func_line > 0 { func_line } else { line_line },
                        line_column,
                    )
                }
            };

        // For functions in the crate source, find actual call line using pre-built table
        let file_in_crate = file.is_some_and(|f| project_context.is_crate_source(f));
        if file_in_crate {
            // Use pre-built crate line table for O(log n) lookup
            {
                let (crate_line, crate_column) =
                    crate_line_table.get_line(func.start_address, data.address);
                if crate_line > 0 {
                    if crate_line == func_line {
                        // Only found function-start line. Try full line table.
                        let (full_line, full_column) = full_line_table.get_nearest_crate_line(
                            data.address,
                            func.start_address,
                            func.end_address,
                            func_line,
                            project_context,
                        );
                        if full_line > 0 && full_line != func_line {
                            line = full_line;
                            column = full_column;
                        } else {
                            line = crate_line;
                            column = crate_column;
                        }
                    } else {
                        let near_prologue = if crate_line > 0 && func_line > 0 {
                            crate_line.saturating_sub(func_line) <= 5
                        } else {
                            false
                        };
                        if near_prologue {
                            if let Some((call_file, call_line_num, _)) =
                                function_index.get_inlined_call_site(data.address)
                            {
                                if project_context.is_crate_source(call_file)
                                    && call_line_num > 0
                                    && call_line_num != crate_line
                                {
                                    file = Some(call_file);
                                    line = call_line_num;
                                    let (lt_file, lt_line, lt_column) =
                                        full_line_table.get_source_location(data.address);
                                    column =
                                        if lt_line == call_line_num && lt_file == Some(call_file) {
                                            lt_column
                                        } else {
                                            0
                                        };
                                } else {
                                    line = crate_line;
                                    column = crate_column;
                                }
                            } else {
                                line = crate_line;
                                column = crate_column;
                            }
                        } else {
                            line = crate_line;
                            column = crate_column;
                        }
                    }
                }
            }
        }

        // Get function name - only do expensive inlined lookup for crate source functions.
        // Intern the name to avoid heap String allocation.
        let func_name = function_index.get_name(func);
        let name_idx = if file_in_crate {
            let name = function_index
                .find_function_name(data.address)
                .map(|s| {
                    let stripped = s.strip_prefix('_').unwrap_or(s);
                    format!("{:#}", demangle(stripped))
                })
                .unwrap_or_else(|| func_name.to_string());
            name_interner.intern_owned(name)
        } else {
            name_interner.intern(func_name)
        };

        // Merge file fields: prefer call-site file, fall back to declaration file.
        let merged_file = file_interner.intern_option(decl_file.or(file));

        Some((
            call_target,
            CallerInfo {
                caller_name: name_idx,
                file: merged_file,
                caller_start_address: func.start_address,
                call_site_addr: data.address,
                line,
                column,
            },
        ))
    } else if let Some((func_addr, func_name)) =
        symbol_index.and_then(|idx| idx.find_containing(data.address))
    {
        // Fallback: found in symbol table but not in DWARF
        Some((
            call_target,
            CallerInfo {
                caller_name: name_interner.intern(func_name),
                file: FILE_NONE,
                caller_start_address: func_addr,
                call_site_addr: data.address,
                line: 0,
                column: u16::MAX,
            },
        ))
    } else {
        // Function not found in DWARF or symbol table - skip this edge
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_section_by_name_nonexistent() {
        // Create a minimal test binary to parse
        // We'll use jonesy's own test binary if it exists
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let test_binary = format!("{}/target/debug/jonesy", manifest_dir);

        if let Ok(buffer) = std::fs::read(&test_binary) {
            if let Ok(macho) = MachO::parse(&buffer, 0) {
                // Test that a non-existent section returns None
                let result = get_section_by_name(&macho, &buffer, "__nonexistent_section");
                assert!(
                    result.is_none(),
                    "get_section_by_name should return None for non-existent section"
                );

                // Verify __text section exists (sanity check)
                let text_result = get_section_by_name(&macho, &buffer, "__text");
                assert!(
                    text_result.is_some(),
                    "__text section should exist in binary"
                );
            }
        }
        // If binary doesn't exist, test passes (this is a unit test, not integration)
    }

    // Tests for CallGraph
    #[test]
    fn test_empty_call_graph() {
        let graph: CallGraph = CallGraph::empty();
        assert!(graph.get_callers(0x1000).is_empty());
        assert!(graph.get_callers(0).is_empty());
    }

    #[test]
    fn test_get_callers_returns_empty_for_unknown_target() {
        let graph: CallGraph = CallGraph::empty();
        let callers = graph.get_callers(0xDEADBEEF);
        assert!(callers.is_empty());
    }

    // Tests for CallGraph::build using real binary
    #[test]
    fn test_call_graph_build_from_real_binary() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(macho) = MachO::parse(&buffer, 0) {
                let binary_ref = BinaryRef::MachO(&macho);
                let symbol_index = SymbolIndex::new(&macho);
                let name_interner = FilePathInterner::new();
                let graph =
                    CallGraph::build(&binary_ref, &buffer, symbol_index.as_ref(), &name_interner);
                assert!(graph.is_ok());
                let graph = graph.unwrap();
                // A real binary should have many call edges
                // Just verify it built without panic
                let _ = graph.get_callers(0);
            }
        }
    }

    #[test]
    fn test_call_graph_build_no_symbol_index() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(macho) = MachO::parse(&buffer, 0) {
                let binary_ref = BinaryRef::MachO(&macho);
                let name_interner = FilePathInterner::new();
                // Build without symbol index — should still work but find no callers
                let graph = CallGraph::build(&binary_ref, &buffer, None, &name_interner);
                assert!(graph.is_ok());
            }
        }
    }

    // Tests for CallerInfo
    #[test]
    fn test_caller_info_construction() {
        let interner = FilePathInterner::new();
        let file_idx = interner.intern("src/main.rs");
        let name_interner = FilePathInterner::new();
        let name_idx = name_interner.intern("my_function");
        let info = CallerInfo {
            caller_name: name_idx,
            file: file_idx,
            caller_start_address: 0x1000,
            call_site_addr: 0x1010,
            line: 42,
            column: 5,
        };
        assert_eq!(info.caller_start_address, 0x1000);
        assert_eq!(info.call_site_addr, 0x1010);
        assert_eq!(info.line, 42);
        assert_ne!(info.file, FILE_NONE);
    }

    #[test]
    fn test_caller_info_name_none() {
        use crate::string_tables::NAME_NONE;
        let info = CallerInfo {
            caller_name: NAME_NONE,
            file: FILE_NONE,
            caller_start_address: 0x2000,
            call_site_addr: 0x2020,
            line: 0,
            column: 0,
        };
        assert_eq!(info.caller_name, NAME_NONE);
        assert_eq!(info.file, FILE_NONE);
        assert_eq!(info.line, 0);
    }
}
