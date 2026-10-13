//! x86_64 architecture support
//!
//! This module contains all x86_64-specific logic for binary analysis:
//! - Instruction disassembly using iced-x86 (zero-allocation decoder)
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
//!
//! ## Decoder Choice
//!
//! This module uses `iced-x86` instead of Capstone for instruction decoding.
//! Key advantages:
//! - **Zero heap allocation**: `decode_out()` reuses a stack-allocated `Instruction` struct
//! - **`Send + Sync`**: enables true parallel disassembly (Capstone is not `Sync`)
//! - **Pure Rust**: no C FFI overhead

use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind, Register};
use rayon::prelude::*;

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

/// Minimum chunk size for parallel disassembly (avoid overhead for small sections).
const MIN_CHUNK_SIZE: usize = 256 * 1024; // 256KB

/// Scan a chunk of x86_64 code for CALL and JMP instructions using iced-x86.
///
/// Uses `decode_out()` to reuse a single stack-allocated `Instruction` struct,
/// avoiding the massive heap allocations that Capstone's `disasm_all()` incurs
/// (which allocated ~15 GB for a 213 MB binary).
fn scan_call_instructions(
    data: &[u8],
    base_addr: u64,
    got_cache: &ahash::AHashMap<u64, u64>,
) -> Vec<InsnData> {
    let mut results = Vec::new();
    let mut decoder = Decoder::with_ip(64, data, base_addr, DecoderOptions::NONE);
    let mut insn = Instruction::default();

    // Track register loads from GOT for resolving call *%reg patterns.
    // Map: register -> RegisterLoad
    // We track recent GOT loads but only within the same basic block
    // (cleared on any control flow).
    let mut register_loads: ahash::AHashMap<Register, RegisterLoad> = ahash::AHashMap::new();

    while decoder.can_decode() {
        decoder.decode_out(&mut insn);

        if insn.is_invalid() {
            continue;
        }

        let mnemonic = insn.mnemonic();

        // Clear register tracking on control flow instructions (except call).
        // This prevents false matches across basic block boundaries.
        match insn.flow_control() {
            FlowControl::ConditionalBranch
            | FlowControl::UnconditionalBranch
            | FlowControl::Return
                // For JMP (unconditional branch), we still want to process it
                // as a potential tail call below before clearing
                if mnemonic != Mnemonic::Jmp =>
            {
                register_loads.clear();
                continue;
            }
            _ => {}
        }

        // Track MOV instructions that load from GOT into registers.
        // Pattern: mov reg, [rip+offset]
        if mnemonic == Mnemonic::Mov
            && insn.op_count() == 2
            && insn.op0_kind() == OpKind::Register
            && insn.op1_kind() == OpKind::Memory
            && insn.memory_base() == Register::RIP
            && insn.memory_index() == Register::None
        {
            let dst_reg = insn.op0_register();
            let disp = insn.memory_displacement64();
            // RIP-relative: effective address = next_insn_addr + disp
            // iced-x86 already computes this for us in memory_displacement64()
            // when the base is RIP: the displacement is the absolute address
            let got_addr = disp;
            register_loads.insert(dst_reg, RegisterLoad { got_addr });
            continue;
        }

        // Any other write to a register invalidates our tracking for that register.
        // We only track MOV from RIP-relative memory, so clear on other MOVs
        // and LEA to tracked registers.
        if (mnemonic == Mnemonic::Mov || mnemonic == Mnemonic::Lea)
            && insn.op_count() >= 1
            && insn.op0_kind() == OpKind::Register
        {
            let dst_reg = insn.op0_register();
            register_loads.remove(&dst_reg);
        }

        // Track both CALL and JMP instructions.
        // JMP with an immediate target is a tail call (like aarch64 B instruction).
        // Without tracking JMPs, the call chain breaks at tail calls like
        // rust_begin_unwind -> rust_panic.
        if mnemonic == Mnemonic::Call || mnemonic == Mnemonic::Jmp {
            let call_target = if insn.op_count() == 1 {
                match insn.op0_kind() {
                    // Direct call/jmp with near branch target
                    OpKind::NearBranch64 | OpKind::NearBranch32 | OpKind::NearBranch16 => {
                        let addr = insn.near_branch_target();
                        Some(got_cache.get(&addr).copied().unwrap_or(addr))
                    }
                    // Indirect call through memory (likely GOT): call [rip+offset]
                    OpKind::Memory => {
                        if insn.memory_base() == Register::RIP
                            && insn.memory_index() == Register::None
                        {
                            let got_addr = insn.memory_displacement64();
                            got_cache.get(&got_addr).copied()
                        } else {
                            None
                        }
                    }
                    // Indirect call through register: call *%rax
                    // Check if this register was loaded from GOT recently
                    // (within same basic block)
                    OpKind::Register => {
                        let reg = insn.op0_register();
                        register_loads
                            .get(&reg)
                            .and_then(|load| got_cache.get(&load.got_addr).copied())
                    }
                    _ => None,
                }
            } else {
                None
            };

            results.push(InsnData {
                address: insn.ip(),
                call_target,
            });

            // Clear register tracking after JMP (end of basic block)
            if mnemonic == Mnemonic::Jmp {
                register_loads.clear();
            }
        }
    }

    results
}

/// Scan for x86_64 CALL instructions in parallel by dividing into chunks.
///
/// Uses iced-x86 for zero-allocation instruction decoding. Since iced-x86's
/// `Decoder` is `Send + Sync`, we can truly parallelise the disassembly across
/// threads, unlike Capstone which required sequential processing.
///
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

    let num_threads = rayon::current_num_threads();

    // Calculate chunk size — we cannot split at arbitrary boundaries on x86_64
    // because instructions are variable-length. However, since we start decoding
    // from the section start, the decoder will establish correct boundaries.
    // For parallel chunks, we accept that a chunk boundary might land mid-instruction;
    // the decoder will recover within a few bytes (self-synchronising property of
    // x86_64 instruction encoding in compiler-generated code).
    let ideal_chunk_size = text_data.len() / num_threads;
    let chunk_size = if ideal_chunk_size < MIN_CHUNK_SIZE {
        text_data.len()
    } else {
        ideal_chunk_size
    };

    if chunk_size >= text_data.len() {
        // Single chunk — sequential scanning
        return scan_call_instructions(text_data, text_addr, &got_cache);
    }

    // Scan chunks in parallel
    let chunks: Vec<(usize, &[u8], u64)> = text_data
        .chunks(chunk_size)
        .enumerate()
        .map(|(i, chunk)| {
            let chunk_addr = text_addr + (i * chunk_size) as u64;
            (i, chunk, chunk_addr)
        })
        .collect();

    let results: Vec<Vec<InsnData>> = chunks
        .par_iter()
        .map(|(_i, chunk, chunk_addr)| scan_call_instructions(chunk, *chunk_addr, &got_cache))
        .collect();

    results.into_iter().flatten().collect()
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

    /// Build a cache mapping stub addresses to actual function addresses
    /// for MachO x86_64 binaries.
    ///
    /// On x86_64 MachO, even internal function calls go through `__stubs`.
    /// Each stub/pointer section entry corresponds to an entry in the indirect
    /// symbol table (LC_DYSYMTAB). The `reserved1` field of the section header
    /// gives the starting index. We read `reserved1` from the raw load command
    /// data since goblin's parsed `Section` struct does not expose it.
    pub(crate) fn build_macho_stub_cache(
        macho: &MachO,
        buffer: &[u8],
    ) -> ahash::AHashMap<u64, u64> {
        use goblin::mach::constants::{
            S_LAZY_SYMBOL_POINTERS, S_NON_LAZY_SYMBOL_POINTERS, S_SYMBOL_STUBS,
        };
        use goblin::mach::load_command::CommandVariant;

        let mut cache = ahash::AHashMap::new();

        // Find the LC_DYSYMTAB command for the indirect symbol table
        let dysymtab = macho.load_commands.iter().find_map(|lc| match lc.command {
            CommandVariant::Dysymtab(ref cmd) => Some(cmd),
            _ => None,
        });
        let Some(dysymtab) = dysymtab else {
            return cache;
        };

        let indirect_offset = dysymtab.indirectsymoff as usize;
        let indirect_count = dysymtab.nindirectsyms as usize;
        if indirect_offset + indirect_count * 4 > buffer.len() {
            return cache;
        }

        // LC_SEGMENT_64 header: 72 bytes; Section64: 80 bytes each
        // Section64 field offsets: addr=32, size=40, flags=64, reserved1=68, reserved2=72
        const SECTION64_SIZE: usize = 80;
        const SEGMENT64_HDR: usize = 72;

        for lc in &macho.load_commands {
            let (nsects, lc_off) = match lc.command {
                CommandVariant::Segment64(ref seg) => (seg.nsects as usize, lc.offset),
                _ => continue,
            };

            for i in 0..nsects {
                let s = lc_off + SEGMENT64_HDR + i * SECTION64_SIZE;
                if s + SECTION64_SIZE > buffer.len() {
                    break;
                }

                let flags = u32::from_le_bytes(buffer[s + 64..s + 68].try_into().unwrap());
                let section_type = flags & 0xff;
                if section_type != S_SYMBOL_STUBS
                    && section_type != S_LAZY_SYMBOL_POINTERS
                    && section_type != S_NON_LAZY_SYMBOL_POINTERS
                {
                    continue;
                }

                let sec_addr = u64::from_le_bytes(buffer[s + 32..s + 40].try_into().unwrap());
                let sec_size = u64::from_le_bytes(buffer[s + 40..s + 48].try_into().unwrap());
                let reserved1 = u32::from_le_bytes(buffer[s + 68..s + 72].try_into().unwrap());
                let reserved2 = u32::from_le_bytes(buffer[s + 72..s + 76].try_into().unwrap());

                // Entry size: reserved2 for stubs (6 on x86_64), 8 for pointers
                let entry_size = if section_type == S_SYMBOL_STUBS {
                    reserved2 as u64
                } else {
                    8u64
                };
                if entry_size == 0 {
                    continue;
                }

                let num_entries = sec_size / entry_size;
                let indirect_start = reserved1 as usize;

                for j in 0..num_entries as usize {
                    let idx = indirect_start + j;
                    if idx >= indirect_count {
                        break;
                    }
                    let sym_off = indirect_offset + idx * 4;
                    if sym_off + 4 > buffer.len() {
                        break;
                    }
                    let sym_idx =
                        u32::from_le_bytes(buffer[sym_off..sym_off + 4].try_into().unwrap());

                    // Skip INDIRECT_SYMBOL_LOCAL / INDIRECT_SYMBOL_ABS sentinels
                    if sym_idx & 0xc0000000 != 0 {
                        continue;
                    }

                    if let Some(ref symbols) = macho.symbols {
                        if let Ok((_name, nlist)) = symbols.get(sym_idx as usize) {
                            if nlist.n_value != 0 {
                                let entry_addr = sec_addr + (j as u64) * entry_size;
                                cache.insert(entry_addr, nlist.n_value);
                            }
                        }
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
