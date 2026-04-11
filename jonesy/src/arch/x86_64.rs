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

/// Extracted instruction data for parallel processing
pub(crate) struct InsnData {
    pub(crate) address: u64,
    pub(crate) call_target: Option<u64>,
}

/// Scan a chunk of x86_64 code for CALL instructions using Capstone disassembler.
fn scan_call_instructions(
    cs: &Capstone,
    data: &[u8],
    base_addr: u64,
    got_cache: &std::collections::HashMap<u64, u64>,
) -> Vec<InsnData> {
    let mut results = Vec::new();

    // Disassemble the code section
    let insns = match cs.disasm_all(data, base_addr) {
        Ok(insns) => insns,
        Err(_) => return results,
    };

    // Extract CALL instructions
    for insn in insns.iter() {
        let mnemonic = insn.mnemonic().unwrap_or("");
        if mnemonic == "call" {
            let detail = match cs.insn_detail(insn) {
                Ok(detail) => detail,
                Err(_) => continue,
            };

            let arch_detail = detail.arch_detail();
            let operands = arch_detail.operands();

            let call_target = if operands.len() == 1 {
                match &operands[0] {
                    arch::ArchOperand::X86Operand(op) => match op.op_type {
                        // Direct call with immediate target
                        arch::x86::X86OperandType::Imm(imm_val) => Some(imm_val as u64),
                        // Indirect call through memory (likely GOT)
                        arch::x86::X86OperandType::Mem(mem_op) => {
                            // Check for RIP-relative addressing
                            if mem_op.base()
                                == capstone::RegId(arch::x86::X86Reg::X86_REG_RIP as u16)
                            {
                                let rip_offset = mem_op.disp();
                                let insn_size = insn.bytes().len() as u64;
                                got::resolve_target(
                                    insn.address(),
                                    insn_size,
                                    rip_offset,
                                    got_cache,
                                )
                            } else {
                                None
                            }
                        }
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
/// Requires ELF and buffer for GOT resolution of indirect calls.
pub(crate) fn parallel_disassemble(
    text_data: &[u8],
    text_addr: u64,
    elf: Option<&goblin::elf::Elf>,
    buffer: &[u8],
) -> Vec<InsnData> {
    use std::collections::HashMap;

    // Build GOT cache for resolving indirect calls
    let got_cache = if let Some(elf) = elf {
        got::build_cache(elf, buffer)
    } else {
        HashMap::new()
    };

    // Create Capstone disassembler once (major optimization - was being created per chunk!)
    let cs = match Capstone::new()
        .x86()
        .mode(arch::x86::ArchMode::Mode64)
        .detail(true)
        .build()
    {
        Ok(cs) => cs,
        Err(_) => return Vec::new(),
    };

    // Note: Capstone is not Sync, so we can't parallelize disassembly across threads.
    // However, creating one instance and scanning sequentially is MUCH faster than
    // the previous approach of creating a new Capstone instance per chunk (which
    // happened 8-12 times on multi-core systems).
    //
    // Sequential scanning with one Capstone instance is the optimization here.
    scan_call_instructions(&cs, text_data, text_addr, &got_cache)
}

/// MachO relocation type for x86_64 PC-relative calls (X86_64_RELOC_BRANCH)
pub(crate) const MACHO_RELOC_BRANCH26: u8 = 2;

/// ELF relocation type for x86_64 PC-relative calls (R_X86_64_PLT32)
pub(crate) const ELF_RELOC_CALL26: u32 = 4;

/// Check if relocation type represents a function call.
pub(crate) fn is_call_relocation(r_type: u32) -> bool {
    r_type == ELF_RELOC_CALL26
}

/// GOT (Global Offset Table) resolution for x86_64 indirect calls.
///
/// On x86_64 Linux, external function calls often go through the GOT:
/// ```asm
/// call *0x1234(%rip)  ; Indirect call through GOT entry
/// ```
///
/// The GOT is populated at load time, but we can resolve it statically
/// using ELF relocations.
pub mod got {
    use goblin::elf::Elf;
    use std::collections::HashMap;

    /// Build a mapping from GOT entry addresses to target function addresses.
    ///
    /// This parses .rela.plt and .rela.dyn sections to build the mapping.
    /// Returns HashMap: GOT address -> target function address
    pub(crate) fn build_cache(elf: &Elf, buffer: &[u8]) -> HashMap<u64, u64> {
        let mut got_cache = HashMap::new();

        // Process .rela.plt relocations
        if let Some(rela_plt) = find_section(elf, ".rela.plt") {
            process_relocations(elf, buffer, rela_plt, &mut got_cache);
        }

        // Process .rela.dyn relocations
        if let Some(rela_dyn) = find_section(elf, ".rela.dyn") {
            process_relocations(elf, buffer, rela_dyn, &mut got_cache);
        }

        got_cache
    }

    /// Find a section by name
    fn find_section<'a>(elf: &'a Elf, name: &str) -> Option<&'a goblin::elf::SectionHeader> {
        elf.section_headers.iter().find(|sh| {
            elf.shdr_strtab
                .get_at(sh.sh_name)
                .map(|n| n == name)
                .unwrap_or(false)
        })
    }

    /// Process relocations from a .rela section
    fn process_relocations(
        elf: &Elf,
        buffer: &[u8],
        section: &goblin::elf::SectionHeader,
        got_cache: &mut HashMap<u64, u64>,
    ) {
        let offset = section.sh_offset as usize;
        let size = section.sh_size as usize;

        // Guard against malformed ELF
        let end = match offset.checked_add(size) {
            Some(e) if e <= buffer.len() => e,
            _ => return,
        };

        let data = &buffer[offset..end];
        let num_relocs = size / 24; // Each Rela entry is 24 bytes on 64-bit

        for i in 0..num_relocs {
            let reloc_offset = i * 24;
            if reloc_offset + 24 > data.len() {
                break;
            }

            // Parse Rela structure: r_offset (8), r_info (8), r_addend (8)
            let r_offset =
                u64::from_le_bytes(data[reloc_offset..reloc_offset + 8].try_into().unwrap());
            let r_info = u64::from_le_bytes(
                data[reloc_offset + 8..reloc_offset + 16]
                    .try_into()
                    .unwrap(),
            );
            let r_addend = i64::from_le_bytes(
                data[reloc_offset + 16..reloc_offset + 24]
                    .try_into()
                    .unwrap(),
            );

            let r_type = (r_info & 0xffffffff) as u32;
            let sym_index = (r_info >> 32) as usize;

            // R_X86_64_JUMP_SLOT (7) and R_X86_64_GLOB_DAT (6)
            if r_type == 7 || r_type == 6 {
                if let Some(sym) = elf.dynsyms.get(sym_index) {
                    let target = sym.st_value;
                    if target != 0 {
                        got_cache.insert(r_offset, target);
                    }
                }
            }
            // R_X86_64_RELATIVE (8)
            else if r_type == 8 {
                // For RELATIVE relocations, target = base + addend
                // Since we don't know the base address at analysis time,
                // we store the addend as the target
                got_cache.insert(r_offset, r_addend as u64);
            }
        }
    }

    /// Resolve a GOT-based call target.
    ///
    /// Given a RIP-relative memory operand, compute the GOT entry address
    /// and look up the target function address.
    pub(crate) fn resolve_target(
        insn_addr: u64,
        insn_size: u64,
        rip_offset: i64,
        got_cache: &HashMap<u64, u64>,
    ) -> Option<u64> {
        // Compute GOT entry address: next_insn_addr + rip_offset
        let next_insn = insn_addr.wrapping_add(insn_size);
        let got_addr = next_insn.wrapping_add(rip_offset as u64);

        got_cache.get(&got_addr).copied()
    }
}

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
