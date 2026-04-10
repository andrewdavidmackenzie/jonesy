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

use rayon::prelude::*;

/// Extracted instruction data for parallel processing (avoids Insn lifetime issues).
pub(crate) struct InsnData {
    pub(crate) address: u64,
    pub(crate) call_target: Option<u64>,
}

/// ARM64 instruction size in bytes (fixed-size ISA).
const INSN_SIZE: usize = 4;

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

/// ELF ARM64 relocation type for BL instructions (call with 26-bit offset).
pub(crate) const ELF_RELOC_CALL26: u32 = 283; // R_AARCH64_CALL26

/// ELF ARM64 relocation type for B instructions (unconditional branch/tail call).
pub(crate) const ELF_RELOC_JUMP26: u32 = 282; // R_AARCH64_JUMP26

/// Check if relocation type represents a function call (BL or B tail call).
pub(crate) fn is_call_relocation(r_type: u32) -> bool {
    r_type == ELF_RELOC_CALL26 || r_type == ELF_RELOC_JUMP26
}

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

/// PLT (Procedure Linkage Table) resolution for ELF shared libraries.
///
/// This submodule handles mapping PLT stub addresses to actual function addresses,
/// which is necessary for analyzing calls in ELF shared objects (.so files).
pub mod plt {
    use goblin::elf::Elf;
    use std::collections::HashMap;

    /// Size of each PLT entry in bytes for ARM64.
    const ENTRY_SIZE: u64 = 16;

    /// Size of PLT resolver stub (first entry) in bytes for ARM64.
    const RESOLVER_SIZE: u64 = 32;

    /// Build a mapping from PLT stub addresses to actual function addresses using ELF relocations.
    /// Returns a HashMap mapping PLT address -> target function address.
    ///
    /// PLT (Procedure Linkage Table) entries are used for lazy symbol resolution in shared libraries.
    /// Each PLT entry corresponds to a relocation in .rela.plt which points to the actual function.
    pub(crate) fn build_map(elf: &Elf, buffer: &[u8]) -> HashMap<u64, u64> {
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

        // Guard against malformed/truncated ELF files
        let Some(rela_plt_end) = rela_plt_offset.checked_add(rela_plt_size) else {
            return plt_map;
        };
        if rela_plt_end > buffer.len() {
            return plt_map;
        }

        let rela_plt_data = &buffer[rela_plt_offset..rela_plt_end];

        // Each Rela entry is 24 bytes on 64-bit
        let num_relocs = rela_plt_size / 24;

        // Parse each relocation entry
        for index in 0..num_relocs {
            let offset = index * 24;
            if offset + 24 > rela_plt_data.len() {
                break;
            }

            // Parse Rela structure (64-bit): r_offset (8), r_info (8), r_addend (8)
            let _r_offset =
                u64::from_le_bytes(rela_plt_data[offset..offset + 8].try_into().unwrap());
            let r_info =
                u64::from_le_bytes(rela_plt_data[offset + 8..offset + 16].try_into().unwrap());
            let _r_addend =
                i64::from_le_bytes(rela_plt_data[offset + 16..offset + 24].try_into().unwrap());

            // Extract symbol index from r_info (upper 32 bits)
            let sym_index = (r_info >> 32) as usize;

            // PLT entry address = plt_base + RESOLVER_SIZE + (index * ENTRY_SIZE)
            let plt_addr = plt_base + RESOLVER_SIZE + (index as u64 * ENTRY_SIZE);

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
    pub(crate) fn resolve_stub(target_addr: u64, plt_map: &HashMap<u64, u64>) -> u64 {
        plt_map.get(&target_addr).copied().unwrap_or(target_addr)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_resolve_stub_with_mapping() {
            let mut plt_map = HashMap::new();
            plt_map.insert(0x71ee0, 0x74f20); // PLT stub -> actual function

            // Should resolve PLT stub to actual function
            let resolved = resolve_stub(0x71ee0, &plt_map);
            assert_eq!(resolved, 0x74f20);
        }

        #[test]
        fn test_resolve_stub_without_mapping() {
            let plt_map = HashMap::new();

            // Should return original address when not in PLT map
            let resolved = resolve_stub(0x12345, &plt_map);
            assert_eq!(resolved, 0x12345);
        }

        #[test]
        fn test_resolve_stub_passthrough() {
            let mut plt_map = HashMap::new();
            plt_map.insert(0x1000, 0x2000);

            // Address not in map should pass through unchanged
            let resolved = resolve_stub(0x3000, &plt_map);
            assert_eq!(resolved, 0x3000);
        }

        #[test]
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        fn test_build_map_with_real_elf() {
            // Test with the dylib example binary if it exists
            let manifest_dir = env!("CARGO_MANIFEST_DIR");
            let dylib_path = format!("{}/../../target/debug/libdylib_example.so", manifest_dir);

            if let Ok(buffer) = std::fs::read(&dylib_path) {
                if let Ok(elf) = Elf::parse(&buffer) {
                    let plt_map = build_map(&elf, &buffer);

                    // The map should not be empty for a dylib with PLT
                    assert!(
                        !plt_map.is_empty(),
                        "PLT map should contain entries for dylib"
                    );

                    // Get actual .plt section bounds from ELF metadata
                    let plt_section = elf
                        .section_headers
                        .iter()
                        .find(|sh| {
                            elf.shdr_strtab
                                .get_at(sh.sh_name)
                                .map(|n| n == ".plt")
                                .unwrap_or(false)
                        })
                        .expect("ELF dylib should have .plt section");

                    let plt_start = plt_section.sh_addr;
                    let plt_end = plt_start + plt_section.sh_size;

                    // Verify that PLT addresses fall within the actual .plt section
                    for (plt_addr, target_addr) in &plt_map {
                        assert!(
                            *plt_addr >= plt_start && *plt_addr < plt_end,
                            "PLT address {:#x} should be in .plt section [{:#x}, {:#x})",
                            plt_addr,
                            plt_start,
                            plt_end
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
        fn test_build_map_resolves_rust_panic() {
            // Test that rust_panic PLT stub is correctly mapped
            let manifest_dir = env!("CARGO_MANIFEST_DIR");
            let dylib_path = format!("{}/../../target/debug/libdylib_example.so", manifest_dir);

            if let Ok(buffer) = std::fs::read(&dylib_path) {
                if let Ok(elf) = Elf::parse(&buffer) {
                    let plt_map = build_map(&elf, &buffer);

                    // Look for rust_panic in the ELF dynamic symbols
                    let rust_panic_addr = elf.dynsyms.iter().find_map(|sym| {
                        if sym.st_value > 0 {
                            if let Some(name) = elf.dynstrtab.get_at(sym.st_name) {
                                if name.contains("rust_panic") {
                                    return Some(sym.st_value);
                                }
                            }
                        }
                        None
                    });

                    // Verify PLT map contains the rust_panic function if it exists
                    if let Some(rust_panic_target) = rust_panic_addr {
                        let found_mapping =
                            plt_map.values().any(|&target| target == rust_panic_target);
                        assert!(
                            found_mapping,
                            "PLT map should contain mapping to rust_panic at {:#x}",
                            rust_panic_target
                        );
                    }

                    // At minimum, should have a reasonable number of PLT entries
                    assert!(
                        plt_map.len() > 50,
                        "Dylib should have substantial number of PLT entries, found {}",
                        plt_map.len()
                    );
                }
            }
        }
    }
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
}
