//! ARM64/aarch64 architecture-specific code.
//!
//! This module contains all ARM64-specific logic for binary analysis:
//! - Instruction encoding and decoding
//! - Branch instruction disassembly
//! - Relocation type constants
//! - PLT (Procedure Linkage Table) resolution for ELF shared libraries
//!
//! # Architecture Characteristics
//!
//! ARM64 has a **fixed-size instruction set** where all instructions are exactly 4 bytes.
//! This makes instruction scanning and disassembly straightforward compared to variable-length
//! ISAs like x86_64.
//!
//! ## Branch Instructions
//!
//! ARM64 uses two primary branch instructions for function calls:
//! - **BL (Branch with Link)**: Used for direct function calls
//! - **B (Branch)**: Used for tail calls and jumps
//!
//! Both use PC-relative addressing with a 26-bit signed offset (±128MB range).

use capstone::Capstone;
use capstone::arch::BuildsCapstone;
use capstone::arch::arm64::ArchMode;
use rayon::prelude::*;

/// Extracted instruction data for parallel processing (avoids Insn lifetime issues).
pub(crate) struct InsnData {
    pub address: u64,
    pub call_target: Option<u64>,
}

/// ARM64 instruction size in bytes (fixed-size ISA).
pub const INSN_SIZE: usize = 4;

/// Minimum chunk size for parallel disassembly (avoid overhead for small sections).
const MIN_CHUNK_SIZE: usize = 64 * 1024; // 64KB

/// ARM64 BL instruction mask: bits [31:26] must be 100101
const BL_MASK: u32 = 0xFC000000;
const BL_OPCODE: u32 = 0x94000000;

/// ARM64 B instruction mask: bits [31:26] must be 000101
const B_MASK: u32 = 0xFC000000;
const B_OPCODE: u32 = 0x14000000;

// Relocation type constants

/// MachO ARM64 relocation type for BL/B instructions (branch with 26-bit offset).
pub(crate) const MACHO_RELOC_BRANCH26: u8 = 2;

/// ELF ARM64 relocation type for BL/B instructions (call with 26-bit offset).
pub(crate) const ELF_RELOC_CALL26: u32 = 283;

/// Decode ARM64 BL/B instruction target address from raw bytes.
/// BL encoding: 100101 imm26
/// B encoding: 000101 imm26
/// Target = PC + sign_extend(imm26) * 4
pub(crate) fn decode_branch_target(insn_bytes: u32, pc: u64) -> u64 {
    // Extract 26-bit immediate
    let imm26 = insn_bytes & 0x03FFFFFF;
    // Sign-extend to 32 bits and multiply by 4 (shift left 2)
    let offset = ((imm26 as i32) << 6) >> 4;
    // Add to PC
    (pc as i64 + offset as i64) as u64
}

/// Scan a chunk of ARM64 code for BL and B instructions.
/// BL = branch with link (function calls)
/// B = unconditional branch (tail calls to other functions)
/// Directly checks raw bytes against opcode patterns.
pub(crate) fn scan_branch_instructions(data: &[u8], base_addr: u64) -> Vec<InsnData> {
    data.chunks_exact(INSN_SIZE)
        .enumerate()
        .filter_map(|(i, bytes)| {
            let insn = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            let is_bl = (insn & BL_MASK) == BL_OPCODE;
            let is_b = (insn & B_MASK) == B_OPCODE;
            if is_bl || is_b {
                let pc = base_addr + (i * INSN_SIZE) as u64;
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

/// Scan for ARM64 BL and B instructions in parallel by dividing into chunks.
/// BL = branch with link (function calls)
/// B = unconditional branch (tail calls to other functions)
/// Directly scans raw bytes for patterns - no disassembly needed.
/// This is much faster than using Capstone for full disassembly.
pub(crate) fn parallel_disassemble(text_data: &[u8], text_addr: u64) -> Vec<InsnData> {
    let num_threads = rayon::current_num_threads();

    // Calculate chunk size, ensuring alignment to instruction boundary
    let ideal_chunk_size = text_data.len() / num_threads;
    let chunk_size = if ideal_chunk_size < MIN_CHUNK_SIZE {
        // Data too small to benefit from parallelization
        text_data.len()
    } else {
        // Align to 4-byte instruction boundary
        (ideal_chunk_size / INSN_SIZE) * INSN_SIZE
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

/// Sequential disassembly using Capstone - kept for non-ARM64 platforms.
#[allow(dead_code)]
pub(crate) fn sequential_disassemble(text_data: &[u8], text_addr: u64) -> Vec<InsnData> {
    let Ok(cs) = Capstone::new().arm64().mode(ArchMode::Arm).build() else {
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

/// PLT (Procedure Linkage Table) resolution for ELF shared libraries.
///
/// This submodule handles mapping PLT stub addresses to actual function addresses,
/// which is necessary for analyzing calls in ELF shared objects (.so files).
pub mod plt {
    // PLT constants and functions will be moved here in Phase 4
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests for decode_branch_target
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

    // Tests for parallel_disassemble
    #[test]
    fn test_parallel_disassemble_small_input() {
        // Small input falls through to sequential scan_branch_instructions
        let mut code = Vec::new();
        code.extend_from_slice(&0x94000002_u32.to_le_bytes()); // BL +8
        code.extend_from_slice(&0x91000000_u32.to_le_bytes()); // ADD (not branch)
        code.extend_from_slice(&0x14000001_u32.to_le_bytes()); // B +4

        let results = parallel_disassemble(&code, 0x1000);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].address, 0x1000);
        assert_eq!(results[1].address, 0x1008);
    }

    #[test]
    fn test_parallel_disassemble_empty() {
        let results = parallel_disassemble(&[], 0x1000);
        assert!(results.is_empty());
    }

    // Tests for sequential_disassemble
    #[test]
    fn test_sequential_disassemble_bl_instruction() {
        // BL +8 at address 0x1000
        let code: Vec<u8> = 0x94000002_u32.to_le_bytes().to_vec();
        let results = sequential_disassemble(&code, 0x1000);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].address, 0x1000);
        assert!(results[0].call_target.is_some());
    }

    #[test]
    fn test_sequential_disassemble_non_branch() {
        // MOV x0, x1 — not a branch
        let code: Vec<u8> = 0xAA0103E0_u32.to_le_bytes().to_vec();
        let results = sequential_disassemble(&code, 0x1000);
        assert!(results.is_empty());
    }

    #[test]
    fn test_sequential_disassemble_empty() {
        let results = sequential_disassemble(&[], 0x1000);
        assert!(results.is_empty());
    }
}
