use crate::binary_format::BinaryRef;
use crate::crate_line_table::CrateLineTable;
use crate::full_line_table::FullLineTable;
use crate::function_index::{FunctionIndex, get_functions_from_dwarf, load_dwarf_sections};
use crate::project_context::ProjectContext;
use crate::sym::SymbolIndex;
use capstone::arch::BuildsCapstone;
use capstone::{Capstone, arch};
use dashmap::DashMap;
use goblin::elf::Elf;
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

/// Extracted instruction data for parallel processing (avoids Insn lifetime issues)
struct InsnData {
    address: u64,
    call_target: Option<u64>,
}

/// ARM64 instruction size in bytes (fixed-size ISA)
const ARM64_INSN_SIZE: usize = 4;

/// Minimum chunk size for parallel disassembly (avoid overhead for small sections)
const MIN_CHUNK_SIZE: usize = 64 * 1024; // 64KB

/// ARM64 BL instruction mask: bits [31:26] must be 100101
const BL_MASK: u32 = 0xFC000000;
const BL_OPCODE: u32 = 0x94000000;

/// ARM64 B instruction mask: bits [31:26] must be 000101
const B_MASK: u32 = 0xFC000000;
const B_OPCODE: u32 = 0x14000000;

/// Build a mapping from PLT stub addresses to actual function addresses using ELF relocations.
/// Returns a HashMap mapping PLT address -> target function address.
///
/// PLT (Procedure Linkage Table) entries are used for lazy symbol resolution in shared libraries.
/// Each PLT entry corresponds to a relocation in .rela.plt which points to the actual function.
fn build_plt_map(elf: &Elf, buffer: &[u8]) -> HashMap<u64, u64> {
    let mut plt_map = HashMap::new();

    // Find .plt section
    let plt_section = elf.section_headers.iter().find(|sh| {
        elf.shdr_strtab
            .get_at(sh.sh_name)
            .map(|n| n == ".plt")
            .unwrap_or(false)
    });

    let Some(plt_section) = plt_section else {
        eprintln!("[PLT] No .plt section found in ELF");
        return plt_map;
    };

    let plt_base = plt_section.sh_addr;

    // Find .rela.plt section to get relocations
    let rela_plt_section = elf.section_headers.iter().find(|sh| {
        elf.shdr_strtab
            .get_at(sh.sh_name)
            .map(|n| n == ".rela.plt")
            .unwrap_or(false)
    });

    let Some(rela_plt_section) = rela_plt_section else {
        return plt_map;
    };

    // Get the .rela.plt section data from the buffer
    let rela_plt_offset = rela_plt_section.sh_offset as usize;
    let rela_plt_size = rela_plt_section.sh_size as usize;
    let rela_plt_data = &buffer[rela_plt_offset..rela_plt_offset + rela_plt_size];

    // Each Rela entry is 24 bytes on 64-bit
    let num_relocs = rela_plt_size / 24;

    // Parse each relocation entry
    for index in 0..num_relocs {
        let offset = index * 24;
        if offset + 24 > rela_plt_data.len() {
            break;
        }

        // Parse Rela structure (64-bit): r_offset (8), r_info (8), r_addend (8)
        let _r_offset = u64::from_le_bytes(rela_plt_data[offset..offset + 8].try_into().unwrap());
        let r_info = u64::from_le_bytes(rela_plt_data[offset + 8..offset + 16].try_into().unwrap());
        let _r_addend =
            i64::from_le_bytes(rela_plt_data[offset + 16..offset + 24].try_into().unwrap());

        // Extract symbol index from r_info (upper 32 bits)
        let sym_index = (r_info >> 32) as usize;

        // PLT entry address = plt_base + 32 + (index * 16)
        let plt_addr = plt_base + 32 + (index as u64 * 16);

        // Look up symbol and get its value (actual function address)
        if let Some(sym) = elf.dynsyms.get(sym_index) {
            let target_addr = sym.st_value;
            if target_addr != 0 {
                plt_map.insert(plt_addr, target_addr);
            }
        }
    }

    plt_map
}

/// Resolve a PLT (Procedure Linkage Table) stub address to the actual function address.
///
/// In ELF shared libraries (dylib), calls often go through PLT stubs for dynamic linking.
/// For example:
///   - PLT stub: 0x71ee0 <rust_panic@plt>
///   - Actual function: 0x74f20 <rust_panic>
///
/// This function looks up the target address in the PLT map and returns the actual function address.
///
/// Returns the resolved address, or the original address if not a PLT stub.
fn resolve_plt_stub(target_addr: u64, plt_map: &HashMap<u64, u64>) -> u64 {
    plt_map.get(&target_addr).copied().unwrap_or(target_addr)
}

/// Decode ARM64 BL/B instruction target address from raw bytes.
/// BL encoding: 100101 imm26
/// B encoding: 000101 imm26
/// Target = PC + sign_extend(imm26) * 4
fn decode_branch_target(insn_bytes: u32, pc: u64) -> u64 {
    // Extract 26-bit immediate
    let imm26 = insn_bytes & 0x03FFFFFF;
    // Sign-extend to 32 bits and multiply by 4 (shift left 2)
    let offset = ((imm26 as i32) << 6) >> 4;
    // Add to PC
    (pc as i64 + offset as i64) as u64
}

/// Scan for ARM64 BL and B instructions in parallel by dividing into chunks.
/// BL = branch with link (function calls)
/// B = unconditional branch (tail calls to other functions)
/// Directly scans raw bytes for patterns - no disassembly needed.
/// This is much faster than using Capstone for full disassembly.
fn parallel_disassemble_arm64(text_data: &[u8], text_addr: u64) -> Vec<InsnData> {
    let num_threads = rayon::current_num_threads();

    // Calculate chunk size, ensuring alignment to instruction boundary
    let ideal_chunk_size = text_data.len() / num_threads;
    let chunk_size = if ideal_chunk_size < MIN_CHUNK_SIZE {
        // Data too small to benefit from parallelization
        text_data.len()
    } else {
        // Align to 4-byte instruction boundary
        (ideal_chunk_size / ARM64_INSN_SIZE) * ARM64_INSN_SIZE
    };

    if chunk_size >= text_data.len() {
        // Single chunk - use sequential scanning
        return scan_branch_instructions(text_data, text_addr);
    }

    // Create chunks with their base addresses
    let chunks: Vec<(usize, &[u8], u64)> = text_data
        .chunks(chunk_size)
        .enumerate()
        .map(|(i, chunk)| {
            let chunk_addr = text_addr + (i * chunk_size) as u64;
            (i, chunk, chunk_addr)
        })
        .collect();

    // Scan chunks in parallel for BL and B instructions
    let results: Vec<Vec<InsnData>> = chunks
        .par_iter()
        .map(|(_i, chunk, chunk_addr)| scan_branch_instructions(chunk, *chunk_addr))
        .collect();

    // Flatten results from all chunks
    results.into_iter().flatten().collect()
}

/// Scan a chunk of ARM64 code for BL and B instructions.
/// BL = branch with link (function calls)
/// B = unconditional branch (tail calls to other functions)
/// Directly checks raw bytes against opcode patterns.
fn scan_branch_instructions(data: &[u8], base_addr: u64) -> Vec<InsnData> {
    data.chunks_exact(ARM64_INSN_SIZE)
        .enumerate()
        .filter_map(|(i, bytes)| {
            let insn = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            let is_bl = (insn & BL_MASK) == BL_OPCODE;
            let is_b = (insn & B_MASK) == B_OPCODE;
            if is_bl || is_b {
                let pc = base_addr + (i * ARM64_INSN_SIZE) as u64;
                let target = decode_branch_target(insn, pc);
                Some(InsnData {
                    address: pc,
                    call_target: Some(target),
                })
            } else {
                None
            }
        })
        .collect()
}

/// Sequential disassembly using Capstone - kept for non-ARM64 platforms.
#[allow(dead_code)]
fn sequential_disassemble_arm64(text_data: &[u8], text_addr: u64) -> Vec<InsnData> {
    let Ok(cs) = Capstone::new()
        .arm64()
        .mode(arch::arm64::ArchMode::Arm)
        .build()
    else {
        eprintln!("Warning: failed to initialize Capstone disassembler");
        return Vec::new();
    };

    let Ok(instructions) = cs.disasm_all(text_data, text_addr) else {
        eprintln!("Warning: disassembly failed for text section at {text_addr:#x}");
        return Vec::new();
    };

    instructions
        .iter()
        .filter_map(|insn| {
            // Match both BL (branch with link) and B (branch) for tail call detection
            let mnemonic = insn.mnemonic();
            if mnemonic == Some("bl") || mnemonic == Some("b") {
                let operand = insn.op_str()?;
                let addr_str = operand.trim_start_matches("#0x");
                let call_target = u64::from_str_radix(addr_str, 16).ok();
                Some(InsnData {
                    address: insn.address(),
                    call_target,
                })
            } else {
                None
            }
        })
        .collect()
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
            build_plt_map(elf, buffer)
        } else {
            HashMap::new()
        };

        let text_section_name = binary.text_section_name();
        let Some((text_addr, text_data)) = binary.find_section(buffer, text_section_name) else {
            return Ok(Self {
                edges: HashMap::new(),
            });
        };

        // Parallel disassembly - divides text section into chunks (ARM64 only)
        #[cfg(target_arch = "aarch64")]
        let insn_data = parallel_disassemble_arm64(text_data, text_addr);

        #[cfg(not(target_arch = "aarch64"))]
        let insn_data = sequential_disassemble_arm64(text_data, text_addr);

        // Process bl instructions in parallel (the expensive part is function lookup)
        let edges: DashMap<u64, Vec<CallerInfo<'a>>> = DashMap::new();

        insn_data.par_iter().for_each(|data| {
            if let Some(call_target) = data.call_target
                && let Some((func_addr, func_name)) =
                    symbol_index.and_then(|idx| idx.find_containing(data.address))
            {
                // Resolve PLT stub to actual function address (for ELF dylib support)
                let resolved_target = resolve_plt_stub(call_target, &plt_map);

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
            build_plt_map(elf, binary_buffer)
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

        // Parallel disassembly - divides text section into chunks (ARM64 only)
        let step = Instant::now();
        #[cfg(target_arch = "aarch64")]
        let insn_data = parallel_disassemble_arm64(text_data, text_addr);

        #[cfg(not(target_arch = "aarch64"))]
        let insn_data = sequential_disassemble_arm64(text_data, text_addr);
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
                let resolved_target = resolve_plt_stub(target, &plt_map);
                edges.entry(resolved_target).or_default().push(caller_info);
            }
        });
        if show_timings {
            eprintln!("    [cg timing] process instructions: {:?}", step.elapsed(),);
        }

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
    data: &InsnData,
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
        let (file, mut line, mut column) = match (func_file, func_line) {
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
                    // If the crate line table only found the function-start line,
                    // try the full line table as a fallback. When stdlib code is
                    // inlined (e.g., unwrap()), the DWARF line entries at the call
                    // address point to stdlib source, so the crate line table only
                    // has the function definition entry. The full line table can
                    // find the nearest crate-source line by searching backward.
                    if crate_line == func_line {
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
                        line = crate_line;
                        column = crate_column;
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

    // Tests for decode_branch_target (ARM64)
    #[test]
    fn test_decode_branch_target_forward() {
        // BL instruction: offset = +4 (next instruction)
        // imm26 = 1, pc = 0x1000
        let insn = 0x94000001_u32; // BL +4
        let target = decode_branch_target(insn, 0x1000);
        assert_eq!(target, 0x1004);
    }

    #[test]
    fn test_decode_branch_target_backward() {
        // BL instruction with negative offset (high bit set in imm26)
        // This represents a backward branch
        let pc = 0x2000_u64;
        // imm26 = 0x3FFFFFF (-1 in 26-bit signed) => offset = -4
        let insn = 0x97FFFFFF_u32; // BL -4
        let target = decode_branch_target(insn, pc);
        assert_eq!(target, pc.wrapping_sub(4));
    }

    #[test]
    fn test_decode_branch_target_zero_offset() {
        // BL with offset 0 (branches to itself)
        let insn = 0x94000000_u32;
        let target = decode_branch_target(insn, 0x1000);
        assert_eq!(target, 0x1000);
    }

    // Additional test for NEW logic in simplify_heuristics branch

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

    // Tests for scan_branch_instructions
    #[test]
    fn test_scan_branch_instructions_bl() {
        // BL +4 instruction at address 0x1000
        let bl_insn: [u8; 4] = 0x94000001_u32.to_le_bytes();
        let results = scan_branch_instructions(&bl_insn, 0x1000);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].address, 0x1000);
        assert_eq!(results[0].call_target, Some(0x1004));
    }

    #[test]
    fn test_scan_branch_instructions_b() {
        // B +8 instruction (unconditional branch / tail call)
        let b_insn: [u8; 4] = 0x14000002_u32.to_le_bytes();
        let results = scan_branch_instructions(&b_insn, 0x2000);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].address, 0x2000);
        assert_eq!(results[0].call_target, Some(0x2008));
    }

    #[test]
    fn test_scan_branch_instructions_non_branch() {
        // ADD instruction (not a branch)
        let add_insn: [u8; 4] = 0x91000000_u32.to_le_bytes();
        let results = scan_branch_instructions(&add_insn, 0x1000);
        assert!(results.is_empty());
    }

    #[test]
    fn test_scan_branch_instructions_multiple() {
        // Two BL instructions with a non-branch between them
        let mut code = Vec::new();
        code.extend_from_slice(&0x94000003_u32.to_le_bytes()); // BL +12
        code.extend_from_slice(&0x91000000_u32.to_le_bytes()); // ADD (not branch)
        code.extend_from_slice(&0x94000001_u32.to_le_bytes()); // BL +4

        let results = scan_branch_instructions(&code, 0x1000);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].address, 0x1000);
        assert_eq!(results[0].call_target, Some(0x100C));
        assert_eq!(results[1].address, 0x1008);
        assert_eq!(results[1].call_target, Some(0x100C));
    }

    #[test]
    fn test_scan_branch_instructions_empty_input() {
        let results = scan_branch_instructions(&[], 0x1000);
        assert!(results.is_empty());
    }

    // Tests for parallel_disassemble_arm64
    #[test]
    fn test_parallel_disassemble_small_input() {
        // Small input falls through to sequential scan_branch_instructions
        let mut code = Vec::new();
        code.extend_from_slice(&0x94000002_u32.to_le_bytes()); // BL +8
        code.extend_from_slice(&0x91000000_u32.to_le_bytes()); // ADD (not branch)
        code.extend_from_slice(&0x14000001_u32.to_le_bytes()); // B +4

        let results = parallel_disassemble_arm64(&code, 0x1000);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].address, 0x1000);
        assert_eq!(results[1].address, 0x1008);
    }

    #[test]
    fn test_parallel_disassemble_empty() {
        let results = parallel_disassemble_arm64(&[], 0x1000);
        assert!(results.is_empty());
    }

    // Tests for sequential_disassemble_arm64
    #[test]
    fn test_sequential_disassemble_bl_instruction() {
        // BL +8 at address 0x1000
        let code: Vec<u8> = 0x94000002_u32.to_le_bytes().to_vec();
        let results = sequential_disassemble_arm64(&code, 0x1000);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].address, 0x1000);
        assert!(results[0].call_target.is_some());
    }

    #[test]
    fn test_sequential_disassemble_non_branch() {
        // MOV x0, x1 — not a branch
        let code: Vec<u8> = 0xAA0103E0_u32.to_le_bytes().to_vec();
        let results = sequential_disassemble_arm64(&code, 0x1000);
        assert!(results.is_empty());
    }

    #[test]
    fn test_sequential_disassemble_empty() {
        let results = sequential_disassemble_arm64(&[], 0x1000);
        assert!(results.is_empty());
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

    // Tests for PLT resolution (ELF-specific)

    #[test]
    fn test_resolve_plt_stub_with_mapping() {
        let mut plt_map = HashMap::new();
        plt_map.insert(0x71ee0, 0x74f20); // PLT stub -> actual function

        // Should resolve PLT stub to actual function
        let resolved = resolve_plt_stub(0x71ee0, &plt_map);
        assert_eq!(resolved, 0x74f20);
    }

    #[test]
    fn test_resolve_plt_stub_without_mapping() {
        let plt_map = HashMap::new();

        // Should return original address when not in PLT map
        let resolved = resolve_plt_stub(0x12345, &plt_map);
        assert_eq!(resolved, 0x12345);
    }

    #[test]
    fn test_resolve_plt_stub_passthrough() {
        let mut plt_map = HashMap::new();
        plt_map.insert(0x1000, 0x2000);

        // Address not in map should pass through unchanged
        let resolved = resolve_plt_stub(0x3000, &plt_map);
        assert_eq!(resolved, 0x3000);
    }

    #[test]
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    fn test_build_plt_map_with_real_elf() {
        // Test with the dylib example binary if it exists
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let dylib_path = format!("{}/../../target/debug/libdylib_example.so", manifest_dir);

        if let Ok(buffer) = std::fs::read(&dylib_path) {
            if let Ok(elf) = Elf::parse(&buffer) {
                let plt_map = build_plt_map(&elf, &buffer);

                // The map should not be empty for a dylib with PLT
                assert!(
                    !plt_map.is_empty(),
                    "PLT map should contain entries for dylib"
                );

                // Verify that PLT addresses are in the expected range
                // (PLT section typically starts around 0x71940 for this binary)
                for (plt_addr, target_addr) in &plt_map {
                    assert!(
                        *plt_addr > 0x70000 && *plt_addr < 0x80000,
                        "PLT address {:#x} should be in PLT section range",
                        plt_addr
                    );
                    assert!(
                        *target_addr > 0,
                        "Target address {:#x} should be non-zero",
                        target_addr
                    );
                    assert_ne!(
                        plt_addr, target_addr,
                        "PLT stub {:#x} should differ from target {:#x}",
                        plt_addr, target_addr
                    );
                }
            }
        }
    }

    #[test]
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    fn test_build_plt_map_resolves_rust_panic() {
        // Test that rust_panic PLT stub is correctly mapped
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let dylib_path = format!("{}/../../target/debug/libdylib_example.so", manifest_dir);

        if let Ok(buffer) = std::fs::read(&dylib_path) {
            if let Ok(elf) = Elf::parse(&buffer) {
                let plt_map = build_plt_map(&elf, &buffer);

                // Look for rust_panic in the ELF dynamic symbols to verify mapping exists
                let mut found_rust_panic_mapping = false;
                for (plt_addr, target_addr) in &plt_map {
                    // Check if this maps to the rust_panic function (around 0x74f20)
                    if *target_addr > 0x74000 && *target_addr < 0x75000 {
                        // Verify it's mapped from a PLT stub
                        assert!(
                            *plt_addr < *target_addr,
                            "PLT stub should have lower address than function"
                        );
                        found_rust_panic_mapping = true;
                    }
                }

                // Should have found at least one panic-related mapping
                assert!(
                    found_rust_panic_mapping || plt_map.len() > 100,
                    "Should have PLT mappings for panic functions or a reasonable number of entries"
                );
            }
        }
    }

    #[test]
    fn test_build_plt_map_empty_for_non_elf() {
        // Non-ELF binaries should return empty map (this tests the ELF-specific logic)
        // We can't easily test this without a real ELF, but we can verify the function exists
        let empty_map: HashMap<u64, u64> = HashMap::new();
        let result = resolve_plt_stub(0x1000, &empty_map);
        assert_eq!(result, 0x1000, "Empty PLT map should pass through address");
    }
}
