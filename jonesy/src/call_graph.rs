#![allow(unused_variables)] // TODO Just for now
#![allow(dead_code)] // TODO Just for now

use crate::crate_line_table::CrateLineTable;
use crate::full_line_table::FullLineTable;
use crate::function_index::{
    FunctionIndex, get_crate_line_at_address, get_functions_from_dwarf, load_dwarf_sections,
};
use crate::project_context::ProjectContext;
use crate::sym::SymbolIndex;
use capstone::arch::BuildsCapstone;
use capstone::{Capstone, Insn, arch};
use dashmap::DashMap;
use goblin::Object;
use goblin::mach::{Mach, MachO};
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

/// Get the __text section's address and data from a MachO binary.
/// This is a standalone helper for callers that only have a raw MachO reference.
/// Get a named section's address and data from a MachO binary.
pub fn get_section_by_name<'a>(
    macho: &MachO,
    buffer: &'a [u8],
    name: &str,
) -> Option<(u64, &'a [u8])> {
    for segment in &macho.segments {
        for (section, _section_data) in segment.sections().unwrap() {
            if section.name().unwrap() == name {
                let offset = section.offset as usize;
                let size = section.size as usize;
                return Some((section.addr, &buffer[offset..offset + size]));
            }
        }
    }
    None
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
        macho: &MachO,
        buffer: &[u8],
        symbol_index: Option<&'a SymbolIndex>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let Some((text_addr, text_data)) = get_section_by_name(macho, buffer, "__text") else {
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
                edges.entry(call_target).or_default().push(CallerInfo {
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

    /// Build a call graph by scanning all instructions once (no debug info).
    /// Non-parallel version for comparison or single-threaded mode.
    #[allow(dead_code)]
    pub fn build_sequential(
        macho: &MachO,
        buffer: &[u8],
        symbol_index: Option<&'a SymbolIndex>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut edges: HashMap<u64, Vec<CallerInfo<'a>>> = HashMap::new();

        let Some((text_addr, text_data)) = get_section_by_name(macho, buffer, "__text") else {
            return Ok(Self { edges });
        };

        let cs = Capstone::new()
            .arm64()
            .mode(arch::arm64::ArchMode::Arm)
            .build()?;

        let instructions = cs
            .disasm_all(text_data, text_addr)
            .map_err(|e| format!("Disassembly failed: {e}"))?;

        for instruction in instructions.iter() {
            if let Some((call_target, caller_info)) =
                process_instruction_basic(symbol_index, instruction)
            {
                edges.entry(call_target).or_default().push(caller_info);
            }
        }

        Ok(Self { edges })
    }

    /// Build a call graph with debug info enrichment.
    /// Uses parallel disassembly and parallel processing for faster analysis.
    /// DWARF names are owned, symbol fallback names borrow from the provided SymbolIndex.
    #[allow(clippy::too_many_arguments)]
    pub fn build_with_debug_info(
        binary_macho: &MachO,
        binary_buffer: &[u8],
        debug_macho: &MachO,
        debug_buffer: &[u8],
        crate_src_path: Option<&str>,
        show_timings: bool,
        symbol_index: Option<&'a SymbolIndex>,
        project_context: &ProjectContext,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        use std::time::Instant;

        // Pre-load DWARF info once (shared across threads)
        let step = Instant::now();
        let (functions, inlined, strings) = get_functions_from_dwarf(debug_macho, debug_buffer)?;
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
        let dwarf = load_dwarf_sections(debug_macho, debug_buffer)?;
        if show_timings {
            eprintln!("    [cg timing] load_dwarf_sections: {:?}", step.elapsed());
        }

        let Some((text_addr, text_data)) =
            get_section_by_name(binary_macho, binary_buffer, "__text")
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
        let (crate_line_table, full_line_table) = if let Some(path) = crate_src_path {
            let (crate_table, full_table) =
                FullLineTable::build_both(&dwarf, path, project_context)?;
            (Some(crate_table), full_table)
        } else {
            (None, FullLineTable::build(&dwarf)?)
        };
        if show_timings {
            eprintln!(
                "    [cg timing] build line tables: {:?} (crate: {} entries, full: {} entries)",
                step.elapsed(),
                crate_line_table.as_ref().map(|t| t.len()).unwrap_or(0),
                full_line_table.len()
            );
        }

        // Process bl instructions in parallel
        let step = Instant::now();
        let edges: DashMap<u64, Vec<CallerInfo<'a>>> = DashMap::new();

        insn_data.par_iter().for_each(|data| {
            if let Some(call_target) = data.call_target
                && let Some((target, caller_info)) = process_instruction_data_with_crate_table(
                    data,
                    &function_index,
                    crate_src_path,
                    &full_line_table,
                    crate_line_table.as_ref(),
                    symbol_index,
                    project_context,
                )
            {
                edges.entry(target).or_default().push(caller_info);
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
fn process_instruction_basic<'a>(
    symbol_index: Option<&'a SymbolIndex>,
    instruction: &Insn,
) -> Option<(u64, CallerInfo<'a>)> {
    // Match both BL (branch with link) and B (branch) for tail call detection
    let mnemonic = instruction.mnemonic();
    if mnemonic != Some("bl") && mnemonic != Some("b") {
        return None;
    }

    let operand = instruction.op_str()?;
    let addr_str = operand.trim_start_matches("#0x");
    let call_target = u64::from_str_radix(addr_str, 16).ok()?;
    let (func_addr, func_name) =
        symbol_index.and_then(|idx| idx.find_containing(instruction.address()))?;

    Some((
        call_target,
        CallerInfo {
            caller_name: Cow::Borrowed(func_name),
            caller_start_address: func_addr,
            caller_file: None,
            call_site_addr: instruction.address(),
            file: None,
            line: None,
            column: None,
        },
    ))
}

/// Process instruction data using pre-built line tables for fast O(log n) lookups.
/// Falls back to symbol table lookup if DWARF doesn't contain the function.
/// DWARF names use Cow::Owned, symbol fallback uses Cow::Borrowed from SymbolIndex.
fn process_instruction_data_with_crate_table<'a>(
    data: &InsnData,
    function_index: &FunctionIndex,
    crate_src_path: Option<&str>,
    full_line_table: &FullLineTable,
    crate_line_table: Option<&CrateLineTable>,
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
        let file_in_crate = file.as_ref().is_some_and(|f| {
            crate_src_path.is_some_and(|crate_path| project_context.is_crate_source(f))
        });
        if file_in_crate {
            // Use pre-built crate line table for O(log n) lookup
            if let Some(table) = crate_line_table {
                let (crate_line, crate_column) = table.get_line(func.start_address, data.address);
                if crate_line.is_some() {
                    // If the crate line table only found the function-start line,
                    // try the full line table as a fallback. When stdlib code is
                    // inlined (e.g., unwrap()), the DWARF line entries at the call
                    // address point to stdlib source, so the crate line table only
                    // has the function definition entry. The full line table can
                    // find the nearest crate-source line by searching backward.
                    if crate_line == func_line {
                        if let Some(crate_path) = crate_src_path {
                            let (full_line, full_column) = full_line_table.get_nearest_crate_line(
                                data.address,
                                func.start_address,
                                func.end_address,
                                func_line,
                                crate_path,
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

// TODO Note that the address passed in is an n_value or Symbol table offset,
// which is not necessarily the same as the address of the symbol in memory.
// How can we fix that?
// TODO using [cfg] have implementations for other architectures
pub(crate) fn find_callers(
    macho: &MachO,
    buffer: &[u8],
    target_addr: u64,
) -> Result<Vec<CallerInfo<'static>>, Box<dyn std::error::Error>> {
    let mut callers = Vec::new();

    let Some((text_addr, text_data)) = get_section_by_name(macho, buffer, "__text") else {
        return Ok(callers);
    };

    let cs = Capstone::new()
        .arm64()
        .mode(arch::arm64::ArchMode::Arm)
        .build()?;

    let Ok(instructions) = cs.disasm_all(text_data, text_addr) else {
        return Ok(callers);
    };

    // Precompute symbol index once for efficient lookups
    let symbol_index = SymbolIndex::new(macho);

    for instruction in instructions.iter() {
        // Match both BL (branch with link) and B (branch) for tail call detection
        let mnemonic = instruction.mnemonic();
        if (mnemonic == Some("bl") || mnemonic == Some("b"))
            && let Some(operand) = instruction.op_str()
        {
            let addr_str = operand.trim_start_matches("#0x");
            if let Ok(call_target) = u64::from_str_radix(addr_str, 16)
                && call_target == target_addr
                && let Some((func_addr, func_name)) = symbol_index
                    .as_ref()
                    .and_then(|idx| idx.find_containing(instruction.address()))
            {
                callers.push(CallerInfo {
                    caller_name: Cow::Owned(func_name.to_string()),
                    caller_start_address: func_addr,
                    caller_file: None,
                    call_site_addr: instruction.address(),
                    file: None,
                    line: None,
                    column: None,
                });
            }
        }
    }

    Ok(callers)
}

/// Find all functions that call a target address, with source info
///
/// # Arguments
/// * `binary_macho` - Parsed MachO from the executable binary (contains __text section)
/// * `binary_buffer` - Raw bytes of the executable binary
/// * `debug_macho` - Parsed MachO containing DWARF info (can be same as binary_macho, or from dSYM)
/// * `debug_buffer` - Raw bytes containing DWARF info (can be same as binary_buffer, or from dSYM)
/// * `target_addr` - Address of the function to find callers for
/// * `crate_src_path` - Optional crate source path for precise line numbers in user code
pub fn find_callers_with_debug_info(
    binary_macho: &MachO,
    binary_buffer: &[u8],
    debug_macho: &MachO,
    debug_buffer: &[u8],
    target_addr: u64,
    crate_src_path: Option<&str>,
    project_context: &ProjectContext,
) -> Result<Vec<CallerInfo<'static>>, Box<dyn std::error::Error>> {
    // Get function info and DWARF from debug info (dSYM or embedded)
    let (functions, inlined, strings) = get_functions_from_dwarf(debug_macho, debug_buffer)?;
    // Build function index for O(log n) lookups
    let function_index = FunctionIndex::new_with_inlined(functions, inlined, strings);
    let dwarf = load_dwarf_sections(debug_macho, debug_buffer)?;
    // Build line table for consistent source location lookups (first entry semantics)
    let full_line_table = FullLineTable::build(&dwarf)?;
    let mut callers = Vec::new();

    // Get __text section from the binary (not dSYM)
    let Some((text_addr, text_data)) = get_section_by_name(binary_macho, binary_buffer, "__text")
    else {
        return Ok(callers);
    };

    // Use capstone for ARM64 disassembly
    let cs = Capstone::new()
        .arm64()
        .mode(arch::arm64::ArchMode::Arm)
        .build()?;

    let instructions = cs.disasm_all(text_data, text_addr)?;

    // Precompute symbol index once for efficient fallback lookups
    let symbol_index = SymbolIndex::new(binary_macho);

    for instruction in instructions.iter() {
        // Look for BL (branch with link) and B (branch) instructions
        // B is used for tail calls where the compiler optimizes `call; ret` into a single jump
        let mnemonic = instruction.mnemonic();
        if (mnemonic == Some("bl") || mnemonic == Some("b"))
            && let Some(operand) = instruction.op_str()
        {
            let addr_str = operand.trim_start_matches("#0x");
            if let Ok(call_target) = u64::from_str_radix(addr_str, 16)
                && call_target == target_addr
            {
                // Find the function containing this call - try DWARF first, fall back to symbol table
                // Uses O(log n) binary search instead of O(n) linear search
                if let Some(func) = function_index.find_containing(instruction.address()) {
                    // Found in DWARF - use full debug info with first-entry semantics
                    let (line_file, line_line, line_column) =
                        full_line_table.get_source_location(func.start_address);

                    // Prefer function's declaration file/line if available, then function start's line info
                    let decl_file = function_index.get_file(func).map(|s| s.to_string());
                    let decl_line = function_index.get_line(func);
                    let file = decl_file.clone().or(line_file);
                    let mut line = decl_line.or(line_line);
                    let mut column = line_column;

                    // For functions in the crate source, find the actual line where the call originates
                    if let (Some(f), Some(crate_path)) = (&file, crate_src_path)
                        && project_context.is_crate_source(f)
                        && let Ok(Some((crate_line, crate_column))) = get_crate_line_at_address(
                            &dwarf,
                            func.start_address,
                            instruction.address(),
                            project_context,
                        )
                    {
                        line = Some(crate_line);
                        column = crate_column;
                    }

                    // Get the most specific function name (checks inlined functions first)
                    // Demangle if it's a mangled Rust name
                    let func_name = function_index.get_name(func);
                    let display_name = function_index
                        .find_function_name(instruction.address())
                        .map(|s| {
                            let stripped = s.strip_prefix('_').unwrap_or(s);
                            format!("{:#}", demangle(stripped))
                        })
                        .unwrap_or_else(|| func_name.to_string());

                    // Note: See comment in process_instruction_data_with_crate_table
                    // for why we use inlined name but containing function's address range
                    callers.push(CallerInfo {
                        caller_name: Cow::Owned(display_name),
                        caller_start_address: func.start_address,
                        caller_file: decl_file,
                        call_site_addr: instruction.address(),
                        file,
                        line,
                        column,
                    });
                } else if let Some((func_addr, func_name)) = symbol_index
                    .as_ref()
                    .and_then(|idx| idx.find_containing(instruction.address()))
                {
                    // Fallback: found in symbol table but not in DWARF (e.g., expect_failed, unwrap_failed)
                    let (file, line, column) = full_line_table.get_source_location(func_addr);

                    callers.push(CallerInfo {
                        caller_name: Cow::Owned(func_name.to_string()),
                        caller_start_address: func_addr,
                        caller_file: None,
                        call_site_addr: instruction.address(),
                        file,
                        line,
                        column,
                    });
                }
            }
        }
    }

    Ok(callers)
}

/// Find callers with source info using the debug map
/// This searches all object files for DWARF info
pub fn find_callers_with_debug_map(
    binary_macho: &MachO,
    binary_buffer: &[u8],
    debug_map: &crate::debug_info::DebugMapInfo,
    target_addr: u64,
    _crate_src_path: Option<&str>,
) -> Result<Vec<CallerInfo<'static>>, Box<dyn std::error::Error>> {
    // First, get callers from the binary's text section
    let mut callers = find_callers(binary_macho, binary_buffer, target_addr)?;

    // Build a map of function name -> source info from all object files
    let mut func_source_map: HashMap<String, (Option<String>, Option<u32>)> = HashMap::new();

    for obj_info in &debug_map.object_files {
        // Parse the object file
        let Ok(Object::Mach(Mach::Binary(obj_macho))) = Object::parse(&obj_info.buffer) else {
            continue;
        };

        // Get functions from this object file
        // TODO: _inlined is currently unused in the debug-map fallback path.
        // Using inlined functions here would require mapping object file addresses
        // to binary addresses, which is complex. The main path (dSYM/embedded DWARF)
        // handles inlined functions correctly via FunctionIndex.find_function_name().
        let Ok((functions, _inlined, strings)) =
            get_functions_from_dwarf(&obj_macho, &obj_info.buffer)
        else {
            continue;
        };

        // Store source info by function name (both mangled and demangled)
        for func in functions {
            let file = strings.get_file(func.file_idx);
            if file.is_some() {
                let name = strings.get_name(func.name_idx);
                let line = if func.line == 0 {
                    None
                } else {
                    Some(func.line)
                };
                // Store by original name
                func_source_map.insert(name.to_string(), (file.map(|s| s.to_string()), line));

                // Also store by demangled name for matching
                let stripped = name.strip_prefix("_").unwrap_or(name);
                let demangled = format!("{:#}", demangle(stripped));
                if demangled != name {
                    func_source_map.insert(demangled, (file.map(|s| s.to_string()), line));
                }
            }
        }
    }

    // Enrich caller info with source locations from DWARF by matching function names
    for caller in &mut callers {
        // Try to find source info by function name
        // Note: caller_name is Cow<'static, str>, convert to string for lookup
        if let Some((file, line)) = func_source_map.get(caller.caller_name.as_ref()) {
            caller.caller_file = file.clone();
            caller.file = file.clone();
            caller.line = *line;
        }
    }

    Ok(callers)
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
}
