//! x86_64 architecture support
//!
//! This module contains all x86_64-specific logic for binary analysis:
//! - Instruction disassembly using Capstone
//! - CALL instruction detection
//! - Relocation type constants
//! - GOT (Global Offset Table) resolution for indirect calls
//!
//! # Architecture Characteristics
//!
//! x86_64 has a **variable-length instruction set** where instructions can be 1-15 bytes.
//! This requires proper disassembly rather than simple pattern matching.
//!
//! ## Call Instructions
//!
//! x86_64 uses several forms of call instructions:
//! - **CALL rel32**: Direct PC-relative call (E8 opcode)
//! - **CALL r/m64**: Indirect call through register or memory
//! - **CALL [rip+offset]**: RIP-relative indirect call (common for GOT/PLT)

use capstone::prelude::*;
use rayon::prelude::*;

/// Extracted instruction data for parallel processing
pub(crate) struct InsnData {
    pub(crate) address: u64,
    pub(crate) call_target: Option<u64>,
}

/// Minimum chunk size for parallel disassembly
const MIN_CHUNK_SIZE: usize = 64 * 1024; // 64KB

/// Scan a chunk of x86_64 code for CALL instructions using Capstone disassembler.
fn scan_call_instructions(data: &[u8], base_addr: u64) -> Vec<InsnData> {
    let mut results = Vec::new();

    // Create Capstone disassembler for x86_64
    let cs = match Capstone::new()
        .x86()
        .mode(arch::x86::ArchMode::Mode64)
        .detail(true)
        .build()
    {
        Ok(cs) => cs,
        Err(_) => return results,
    };

    // Disassemble the code section
    let insns = match cs.disasm_all(data, base_addr) {
        Ok(insns) => insns,
        Err(_) => return results,
    };

    // Extract CALL instructions
    for insn in insns.iter() {
        let mnemonic = insn.mnemonic().unwrap_or("");
        if mnemonic == "call" {
            // For direct calls, compute the target from the immediate operand
            let detail = match cs.insn_detail(insn) {
                Ok(detail) => detail,
                Err(_) => continue,
            };

            let arch_detail = detail.arch_detail();
            let operands = arch_detail.operands();

            let call_target = if operands.len() == 1 {
                match &operands[0] {
                    arch::ArchOperand::X86Operand(op) => match op.op_type {
                        arch::x86::X86OperandType::Imm(imm_val) => Some(imm_val as u64),
                        _ => None,
                    },
                    _ => None,
                }
            } else {
                None
            };

            results.push(InsnData {
                address: insn.address(),
                call_target,
            });
        }
    }

    results
}

/// Scan for x86_64 CALL instructions in parallel by dividing into chunks.
/// Uses Capstone disassembler to handle variable-length instructions.
pub(crate) fn parallel_disassemble(text_data: &[u8], text_addr: u64) -> Vec<InsnData> {
    let num_threads = rayon::current_num_threads();

    // Calculate chunk size
    let ideal_chunk_size = text_data.len() / num_threads;
    let chunk_size = if ideal_chunk_size < MIN_CHUNK_SIZE {
        // Data too small to benefit from parallelization
        text_data.len()
    } else {
        ideal_chunk_size
    };

    if chunk_size >= text_data.len() {
        // Single chunk - use sequential scanning
        return scan_call_instructions(text_data, text_addr);
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

    // Scan chunks in parallel for CALL instructions
    let results: Vec<Vec<InsnData>> = chunks
        .par_iter()
        .map(|(_i, chunk, chunk_addr)| scan_call_instructions(chunk, *chunk_addr))
        .collect();

    // Flatten results from all chunks
    results.into_iter().flatten().collect()
}

/// MachO relocation type for x86_64 PC-relative calls (X86_64_RELOC_BRANCH)
pub(crate) const MACHO_RELOC_BRANCH26: u8 = 2;

/// ELF relocation type for x86_64 PC-relative calls (R_X86_64_PLT32)
pub(crate) const ELF_RELOC_CALL26: u32 = 4;

/// PLT (Procedure Linkage Table) resolution stub module
pub mod plt {
    use goblin::elf::Elf;
    use std::collections::HashMap;

    /// Build a mapping from PLT stub addresses to actual function addresses (stub)
    pub(crate) fn build_map(_elf: &Elf, _buffer: &[u8]) -> HashMap<u64, u64> {
        HashMap::new()
    }

    /// Resolve a PLT stub address to the actual function address (stub)
    pub(crate) fn resolve_stub(target_addr: u64, _plt_map: &HashMap<u64, u64>) -> u64 {
        target_addr
    }
}
