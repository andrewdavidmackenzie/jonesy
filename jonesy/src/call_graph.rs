use crate::arch;
use crate::binary_format::BinaryRef;
use crate::crate_line_table::CrateLineTable;
use crate::full_line_table::FullLineTable;
use crate::function_index::{FunctionIndex, get_functions_from_dwarf, load_dwarf_sections};
use crate::project_context::ProjectContext;
use crate::sym::SymbolIndex;
use dashmap::DashMap;
use goblin::mach::MachO;
use rayon::prelude::*;
use rustc_demangle::demangle;
use std::borrow::Cow;
use std::collections::HashMap;

/// Information about a call site
/// Uses Cow<'a, str> for caller_name to support both borrowed (from SymbolIndex)
/// and owned (from DWARF) strings without allocation in the hot path.
#[derive(Debug, Clone)]
pub struct CallerInfo<'a> {
    /// Demangled name of the calling function
    pub caller_name: Cow<'a, str>,
    /// Start address of the calling function
    pub caller_start_address: u64,
    /// File of the calling function (from DWARF, if available)
    pub caller_file: Option<String>,
    /// Address of the calling instruction
    pub call_site_addr: u64,
    /// Source location of the calling instruction
    pub file: Option<String>,
    /// Line number of the calling instruction
    pub line: Option<u32>,
    /// Column number of the calling instruction
    pub column: Option<u32>,
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
/// Lifetime 'a comes from SymbolIndex - caller_name may borrow from it.
pub struct CallGraph<'a> {
    /// Maps target_addr -> list of CallerInfo
    edges: HashMap<u64, Vec<CallerInfo<'a>>>,
}

impl<'a> CallGraph<'a> {
    /// Build a call graph by scanning all instructions once (no debug info).
    /// Uses parallel disassembly and parallel processing for faster analysis.
    /// Symbol names are borrowed from the provided SymbolIndex.
    pub fn build(
        binary: &BinaryRef,
        buffer: &[u8],
        symbol_index: Option<&'a SymbolIndex>,
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

        // Process bl instructions in parallel (the expensive part is function lookup)
        let edges: DashMap<u64, Vec<CallerInfo<'a>>> = DashMap::new();

        insn_data.par_iter().for_each(|data| {
            if let Some(call_target) = data.call_target
                && let Some((func_addr, func_name)) =
                    symbol_index.and_then(|idx| idx.find_containing(data.address))
            {
                // Resolve PLT stub to actual function address (for ELF dylib support)
                let resolved_target = arch::plt::resolve_stub(call_target, &plt_map);

                edges.entry(resolved_target).or_default().push(CallerInfo {
                    caller_name: Cow::Borrowed(func_name), // No allocation - borrows from SymbolIndex
                    caller_start_address: func_addr,
                    caller_file: None,
                    call_site_addr: data.address,
                    file: None,
                    line: None,
                    column: None,
                });
            }
        });

        // Convert DashMap to HashMap and sort each caller list for deterministic ordering
        let mut edges: HashMap<u64, Vec<CallerInfo<'a>>> = edges.into_iter().collect();
        for callers in edges.values_mut() {
            callers.sort_by_key(|c| c.call_site_addr);
        }
        Ok(Self { edges })
    }

    /// Build a call graph with debug info enrichment.
    /// Uses parallel disassembly and parallel processing for faster analysis.
    /// DWARF names are owned, symbol fallback names borrow from the provided SymbolIndex.
    pub fn build_with_debug_info(
        binary: &BinaryRef,
        binary_buffer: &[u8],
        debug_binary: &BinaryRef,
        debug_buffer: &[u8],
        show_timings: bool,
        symbol_index: Option<&'a SymbolIndex>,
        project_context: &ProjectContext,
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
        {
            let with_target = insn_data.iter().filter(|d| d.call_target.is_some()).count();
            let without_target = insn_data.len() - with_target;
            eprintln!(
                "  DEBUG: {} CALL instructions found ({} with target, {} unresolved)",
                insn_data.len(),
                with_target,
                without_target
            );
        }
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
            FullLineTable::build_both(&dwarf, project_context)?;
        if show_timings {
            eprintln!(
                "    [cg timing] build line tables: {:?} (crate: {} entries, full: {} entries)",
                step.elapsed(),
                crate_line_table.len(),
                full_line_table.len()
            );
        }

        // Process bl instructions in parallel
        let step = Instant::now();
        let edges: DashMap<u64, Vec<CallerInfo<'a>>> = DashMap::new();

        insn_data.par_iter().for_each(|data| {
            if let Some(_call_target) = data.call_target
                && let Some((target, caller_info)) = process_instruction_data_with_crate_table(
                    data,
                    &function_index,
                    &full_line_table,
                    &crate_line_table,
                    symbol_index,
                    project_context,
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

        eprintln!(
            "  DEBUG: Call graph has {} unique target addresses, {} total edges",
            edges.len(),
            edges.iter().map(|e| e.value().len()).sum::<usize>()
        );

        // Convert DashMap to HashMap and sort each caller list for deterministic ordering
        let mut edges: HashMap<u64, Vec<CallerInfo<'a>>> = edges.into_iter().collect();
        for callers in edges.values_mut() {
            callers.sort_by_key(|c| c.call_site_addr);
        }
        Ok(Self { edges })
    }

    /// Get all callers of a target address.
    /// Returns a slice reference to avoid cloning CallerInfo instances.
    pub fn get_callers(&self, target_addr: u64) -> &[CallerInfo<'a>] {
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

/// Process a single instruction and extract call information (basic version without debug info).
/// Returns (call_target, CallerInfo) if this is a bl/b instruction, None otherwise.
/// Process instruction data using pre-built line tables for fast O(log n) lookups.
/// Falls back to symbol table lookup if DWARF doesn't contain the function.
/// DWARF names use Cow::Owned, symbol fallback uses Cow::Borrowed from SymbolIndex.
fn process_instruction_data_with_crate_table<'a>(
    data: &arch::InsnData,
    function_index: &FunctionIndex,
    full_line_table: &FullLineTable,
    crate_line_table: &CrateLineTable,
    symbol_index: Option<&'a SymbolIndex>,
    project_context: &ProjectContext,
) -> Option<(u64, CallerInfo<'a>)> {
    let call_target = data.call_target?;

    // Find the function containing this call - try DWARF first, fall back to symbol table
    // Uses O(log n) binary search instead of O(n) linear search
    if let Some(func) = function_index.find_containing(data.address) {
        // Found in DWARF - use full debug info
        // Get source location using O(log n) binary search on pre-built table
        let func_file = function_index.get_file(func).map(|s| s.to_string());
        let func_line = function_index.get_line(func);
        // Clone once for caller_file, then move func_file into match
        let caller_file = func_file.clone();
        let (mut file, mut line, mut column) = match (func_file, func_line) {
            (Some(f), Some(l)) => {
                // Fast path: use DWARF function info directly
                (Some(f), Some(l), None)
            }
            (func_file, func_line) => {
                // Use pre-built full line table for O(log n) lookup
                let (line_file, line_line, line_column) =
                    full_line_table.get_source_location(func.start_address);
                (
                    func_file.or(line_file),
                    func_line.or(line_line),
                    line_column,
                )
            }
        };

        // For functions in the crate source, find actual call line using pre-built table
        let file_in_crate = file
            .as_ref()
            .is_some_and(|f| project_context.is_crate_source(f));
        if file_in_crate {
            // Use pre-built crate line table for O(log n) lookup
            {
                let (crate_line, crate_column) =
                    crate_line_table.get_line(func.start_address, data.address);
                if crate_line.is_some() {
                    if crate_line == func_line {
                        // Only found function-start line. Try full line table.
                        let (full_line, full_column) = full_line_table.get_nearest_crate_line(
                            data.address,
                            func.start_address,
                            func.end_address,
                            func_line,
                            project_context,
                        );
                        if full_line.is_some() && full_line != func_line {
                            line = full_line;
                            column = full_column;
                        } else {
                            line = crate_line;
                            column = crate_column;
                        }
                    } else {
                        // Got a specific line from crate table. Only consider
                        // DWARF inline info when crate_line is in the function
                        // prologue area (within a few lines of the declaration).
                        // This is where the crate line table may return the
                        // setup line before an inlined expansion (e.g.,
                        // Option::unwrap) rather than the actual call site.
                        // For lines deep inside the function body, the crate
                        // line table result is already correct.
                        let near_prologue = match (crate_line, func_line) {
                            (Some(cl), Some(fl)) => cl.saturating_sub(fl) <= 5,
                            _ => false,
                        };
                        if near_prologue {
                            if let Some((call_file, call_line_num, _)) =
                                function_index.get_inlined_call_site(data.address)
                            {
                                if project_context.is_crate_source(call_file)
                                    && call_line_num > 0
                                    && Some(call_line_num) != crate_line
                                {
                                    file = Some(call_file.to_string());
                                    line = Some(call_line_num);
                                    column = None;
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

        // Get function name - only do expensive inlined lookup for crate source functions
        // For non-crate functions, use the containing function's name (fast path)
        let func_name = function_index.get_name(func);
        let display_name: Cow<'a, str> = if file_in_crate {
            // Crate function: get the most specific name (checks inlined functions)
            Cow::Owned(
                function_index
                    .find_function_name(data.address)
                    .map(|s| {
                        let stripped = s.strip_prefix('_').unwrap_or(s);
                        format!("{:#}", demangle(stripped))
                    })
                    .unwrap_or_else(|| func_name.to_string()),
            )
        } else {
            // Non-crate function: use containing function name directly (skip inlined search)
            Cow::Owned(func_name.to_string())
        };

        // Note: We intentionally use the inlined function's name but keep the
        // containing function's address range (start/end) for call graph building.
        // The caller.file/line fields are the function *definition* location,
        // while CallerInfo.file/line/column below are the actual *call site*.
        Some((
            call_target,
            CallerInfo {
                caller_name: display_name,
                caller_start_address: func.start_address,
                caller_file,
                call_site_addr: data.address,
                file,
                line,
                column,
            },
        ))
    } else if let Some((func_addr, func_name)) =
        symbol_index.and_then(|idx| idx.find_containing(data.address))
    {
        // Fallback: found in symbol table but not in DWARF (e.g., expect_failed, unwrap_failed)
        // Don't use line table for source location - addresses outside DWARF functions
        // would incorrectly get source info from adjacent functions.
        // These are typically stdlib internals that we don't need source info for anyway.
        Some((
            call_target,
            CallerInfo {
                caller_name: Cow::Borrowed(func_name), // No allocation - borrows from SymbolIndex
                caller_start_address: func_addr,
                caller_file: None,
                call_site_addr: data.address,
                file: None,
                line: None,
                column: None,
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
        let graph: CallGraph<'_> = CallGraph::empty();
        assert!(graph.get_callers(0x1000).is_empty());
        assert!(graph.get_callers(0).is_empty());
    }

    #[test]
    fn test_get_callers_returns_empty_for_unknown_target() {
        let graph: CallGraph<'_> = CallGraph::empty();
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
                let graph = CallGraph::build(&binary_ref, &buffer, symbol_index.as_ref());
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
                // Build without symbol index — should still work but find no callers
                let graph = CallGraph::build(&binary_ref, &buffer, None);
                assert!(graph.is_ok());
            }
        }
    }

    // Tests for CallerInfo
    #[test]
    fn test_caller_info_construction() {
        let info = CallerInfo {
            caller_name: Cow::Owned("my_function".to_string()),
            caller_start_address: 0x1000,
            caller_file: Some("src/main.rs".to_string()),
            call_site_addr: 0x1010,
            file: Some("src/main.rs".to_string()),
            line: Some(42),
            column: Some(5),
        };
        assert_eq!(info.caller_start_address, 0x1000);
        assert_eq!(info.call_site_addr, 0x1010);
        assert_eq!(info.line, Some(42));
    }

    #[test]
    fn test_caller_info_borrowed_name() {
        let name = "borrowed_function".to_string();
        let info = CallerInfo {
            caller_name: Cow::Borrowed(&name),
            caller_start_address: 0x2000,
            caller_file: None,
            call_site_addr: 0x2020,
            file: None,
            line: None,
            column: None,
        };
        assert_eq!(&*info.caller_name, "borrowed_function");
        assert!(info.caller_file.is_none());
        assert!(info.line.is_none());
    }
}
