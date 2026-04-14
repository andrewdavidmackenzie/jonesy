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

/// Tracks RIP-relative loads into registers for resolving register-indirect calls
struct RegisterLoad {
    /// Address where the GOT entry will be after the instruction executes
    got_addr: u64,
}

/// Scan a chunk of x86_64 code for CALL instructions using Capstone disassembler.
fn scan_call_instructions(
    cs: &Capstone,
    data: &[u8],
    base_addr: u64,
    got_cache: &ahash::AHashMap<u64, u64>,
) -> Vec<InsnData> {
    let mut results = Vec::new();

    // Disassemble the code section
    let insns = match cs.disasm_all(data, base_addr) {
        Ok(insns) => insns,
        Err(_) => return results,
    };

    // Track register loads from GOT for resolving call *%reg patterns
    // Map: register_id -> RegisterLoad
    // We track recent GOT loads but only within the same basic block (cleared on any control flow)
    let mut register_loads: ahash::AHashMap<u16, RegisterLoad> = ahash::AHashMap::new();

    // Extract CALL instructions
    for insn in insns.iter() {
        let mnemonic = insn.mnemonic().unwrap_or("");

        // Clear register tracking on control flow instructions (except call)
        // This prevents false matches across basic block boundaries
        if matches!(
            mnemonic,
            "jmp"
                | "je"
                | "jne"
                | "jz"
                | "jnz"
                | "ja"
                | "jae"
                | "jb"
                | "jbe"
                | "jg"
                | "jge"
                | "jl"
                | "jle"
                | "ret"
        ) {
            register_loads.clear();
        }

        // Track MOV instructions that load from GOT into registers
        if mnemonic == "mov" {
            if let Ok(detail) = cs.insn_detail(insn) {
                let arch_detail = detail.arch_detail();
                let operands = arch_detail.operands();

                // Pattern: mov offset(%rip), %reg
                if operands.len() == 2 {
                    if let (
                        arch::ArchOperand::X86Operand(dst),
                        arch::ArchOperand::X86Operand(src),
                    ) = (&operands[0], &operands[1])
                    {
                        // Check if destination is a register (this will get overwritten)
                        if let arch::x86::X86OperandType::Reg(dst_reg) = dst.op_type {
                            // Check if source is RIP-relative memory load
                            if let arch::x86::X86OperandType::Mem(mem_op) = src.op_type {
                                if mem_op.base()
                                    == capstone::RegId(arch::x86::X86Reg::X86_REG_RIP as u16)
                                {
                                    let rip_offset = mem_op.disp();
                                    let insn_size = insn.bytes().len() as u64;
                                    let next_insn = insn.address().wrapping_add(insn_size);
                                    let got_addr = next_insn.wrapping_add(rip_offset as u64);

                                    // Update this register's GOT load
                                    register_loads.insert(dst_reg.0, RegisterLoad { got_addr });
                                } else {
                                    // Non-RIP-relative write to register - clear it
                                    register_loads.remove(&dst_reg.0);
                                }
                            } else {
                                // Register being written with non-memory source - clear it
                                register_loads.remove(&dst_reg.0);
                            }
                        }
                    }
                }
            }
        }

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
                        // Direct call with immediate target — resolve through
                        // stub cache if the target is a stub address
                        arch::x86::X86OperandType::Imm(imm_val) => {
                            let addr = imm_val as u64;
                            Some(got_cache.get(&addr).copied().unwrap_or(addr))
                        }
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
                        // Indirect call through register: call *%rax
                        // Check if this register was loaded from GOT recently (within same basic block)
                        arch::x86::X86OperandType::Reg(reg) => register_loads
                            .get(&reg.0)
                            .and_then(|load| got_cache.get(&load.got_addr).copied()),
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
/// Builds an indirect call cache from ELF GOT or MachO stubs.
pub(crate) fn parallel_disassemble(
    text_data: &[u8],
    text_addr: u64,
    binary: &crate::binary_format::BinaryRef,
    buffer: &[u8],
) -> Vec<InsnData> {
    // Build indirect call cache: GOT for ELF, stub resolution for MachO
    let got_cache = match binary {
        crate::binary_format::BinaryRef::Elf(elf) => got::build_cache(elf, buffer),
        crate::binary_format::BinaryRef::MachO(macho) => got::build_macho_stub_cache(macho, buffer),
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
pub(crate) const ELF_RELOC_PLT32: u32 = 4;

/// ELF relocation type for x86_64 GOT-relative calls (R_X86_64_GOTPCREL)
/// Used for indirect calls through the Global Offset Table
pub(crate) const ELF_RELOC_GOTPCREL: u32 = 9;

/// Check if relocation type represents a function call.
/// Accepts both direct PLT calls and GOT-based indirect calls.
pub(crate) fn is_call_relocation(r_type: u32) -> bool {
    r_type == ELF_RELOC_PLT32 || r_type == ELF_RELOC_GOTPCREL
}

/// Indirect call resolution for x86_64 binaries.
///
/// On x86_64, external function calls use indirect addressing:
/// - **ELF**: calls go through the GOT (Global Offset Table), resolved via
///   `.rela.plt` and `.rela.dyn` relocations
/// - **MachO**: calls go through `__stubs` which jump via `__la_symbol_ptr`,
///   resolved via the indirect symbol table
///
/// Both are resolved statically into the same `AHashMap<u64, u64>` format
/// mapping pointer/GOT addresses to target function addresses.
pub mod got {
    use goblin::elf::Elf;
    use goblin::mach::MachO;

    /// Build a mapping from GOT entry addresses to target function addresses.
    ///
    /// This parses .rela.plt and .rela.dyn sections to build the mapping.
    /// Returns AHashMap: GOT address -> target function address (faster than HashMap)
    pub(crate) fn build_cache(elf: &Elf, buffer: &[u8]) -> ahash::AHashMap<u64, u64> {
        let mut got_cache = ahash::AHashMap::new();

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

    /// Build a cache mapping stub and pointer addresses to actual function
    /// addresses for MachO x86_64 binaries.
    ///
    /// On x86_64 MachO, even internal function calls go through `__stubs`:
    ///   CALL stub_addr → JMP *ptr_addr(%rip) → actual_function
    ///
    /// We parse each stub's JMP instruction to find the `__la_symbol_ptr`
    /// entry it targets, then read the pointer value from the binary to get
    /// the actual function address.
    pub(crate) fn build_macho_stub_cache(
        macho: &MachO,
        _buffer: &[u8],
    ) -> ahash::AHashMap<u64, u64> {
        let mut cache = ahash::AHashMap::new();

        // Collect __la_symbol_ptr and __nl_symbol_ptr section data for pointer lookups
        let mut ptr_sections: Vec<(u64, Vec<u8>)> = Vec::new();
        for segment in macho.segments.iter() {
            if let Ok(sections) = segment.sections() {
                for (section, data) in sections {
                    let name = section.name().unwrap_or("");
                    if name == "__la_symbol_ptr" || name == "__nl_symbol_ptr" || name == "__got" {
                        ptr_sections.push((section.addr, data.to_vec()));
                    }
                }
            }
        }

        // Helper: read an 8-byte pointer from any pointer section given a virtual address
        let read_ptr = |vaddr: u64| -> Option<u64> {
            for (sec_addr, data) in &ptr_sections {
                if vaddr >= *sec_addr {
                    let offset = (vaddr - sec_addr) as usize;
                    if offset + 8 <= data.len() {
                        return Some(u64::from_le_bytes(
                            data[offset..offset + 8].try_into().unwrap(),
                        ));
                    }
                }
            }
            None
        };

        // Parse __stubs: each is a 6-byte JMP *offset(%rip) instruction
        for segment in macho.segments.iter() {
            if let Ok(sections) = segment.sections() {
                for (section, data) in sections {
                    if section.name().unwrap_or("") != "__stubs" {
                        continue;
                    }
                    let stub_size = 6usize;
                    let num_stubs = data.len() / stub_size;
                    for i in 0..num_stubs {
                        let off = i * stub_size;
                        let stub_addr = section.addr + off as u64;
                        if off + 6 <= data.len() && data[off] == 0xFF && data[off + 1] == 0x25 {
                            let rip_offset =
                                i32::from_le_bytes(data[off + 2..off + 6].try_into().unwrap());
                            let ptr_addr =
                                (stub_addr + stub_size as u64).wrapping_add(rip_offset as u64);
                            if let Some(target) = read_ptr(ptr_addr) {
                                if target != 0 {
                                    cache.insert(stub_addr, target);
                                    // Also map the pointer address for CALL *offset(%rip)
                                    cache.insert(ptr_addr, target);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Also map __got pointer values for CALL *offset(%rip) through __got
        for (sec_addr, data) in &ptr_sections {
            let entry_size = 8usize;
            let num_entries = data.len() / entry_size;
            for i in 0..num_entries {
                let off = i * entry_size;
                let addr = sec_addr + off as u64;
                if !cache.contains_key(&addr) && off + 8 <= data.len() {
                    let value = u64::from_le_bytes(data[off..off + 8].try_into().unwrap());
                    if value != 0 {
                        cache.insert(addr, value);
                    }
                }
            }
        }

        cache
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
        got_cache: &mut ahash::AHashMap<u64, u64>,
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
        got_cache: &ahash::AHashMap<u64, u64>,
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
