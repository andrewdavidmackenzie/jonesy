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

/// Bytes decoded before each chunk start so the decoder resynchronises with the
/// real instruction stream. x86_64 instructions are at most 15 bytes, and the
/// decoder typically resynchronises within a few instructions. 256 bytes
/// provides a generous margin.
const RESYNC_LOOKBACK: usize = 256;

/// Scan a chunk of x86_64 code for CALL and JMP instructions using iced-x86.
///
/// Uses `decode_out()` to reuse a single stack-allocated `Instruction` struct,
/// avoiding the massive heap allocations that Capstone's `disasm_all()` incurs
/// (which allocated ~15 GB for a 213 MB binary).
///
/// The `stop_addr` parameter controls when to stop decoding. When used for
/// parallel chunking, each chunk decodes until the first instruction at or
/// past `stop_addr`, ensuring instructions that cross chunk boundaries are
/// decoded fully and reported by exactly one chunk.
fn scan_call_instructions(
    data: &[u8],
    base_addr: u64,
    stop_addr: u64,
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

    while decoder.can_decode() && decoder.ip() < stop_addr {
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
/// ## Chunk boundary handling
///
/// x86_64 has variable-length instructions (1-15 bytes), so splitting at
/// arbitrary byte offsets can land mid-instruction. To handle this safely:
///
/// 1. Each chunk starts decoding `RESYNC_LOOKBACK` bytes early, giving the
///    decoder enough context to resynchronise with the real instruction stream
///    before reaching the chunk's logical start.
/// 2. Only results at or after the chunk's logical start are retained.
/// 3. Each chunk decodes until the first instruction at or past the chunk's
///    logical end, so instructions crossing boundaries are decoded fully and
///    reported by exactly one chunk.
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
    let text_end = text_addr + text_data.len() as u64;

    let ideal_chunk_size = text_data.len() / num_threads;
    let chunk_size = if ideal_chunk_size < MIN_CHUNK_SIZE {
        text_data.len()
    } else {
        ideal_chunk_size
    };

    if chunk_size >= text_data.len() {
        // Single chunk — sequential scanning
        return scan_call_instructions(text_data, text_addr, text_end, &got_cache);
    }

    // Build chunk start offsets
    let starts: Vec<usize> = (0..text_data.len()).step_by(chunk_size).collect();

    let results: Vec<Vec<InsnData>> = starts
        .par_iter()
        .map(|&start| {
            let chunk_end_offset = (start + chunk_size).min(text_data.len());
            let chunk_start_addr = text_addr + start as u64;
            let chunk_end_addr = text_addr + chunk_end_offset as u64;

            // Start decoding early so the decoder resynchronises before the
            // chunk's logical start. The first chunk (start == 0) needs no
            // lookback.
            let decode_start = start.saturating_sub(RESYNC_LOOKBACK);
            let decode_start_addr = text_addr + decode_start as u64;

            // For the last chunk, decode to the end of the section
            let stop_addr = if chunk_end_offset >= text_data.len() {
                text_end
            } else {
                chunk_end_addr
            };

            let mut found = scan_call_instructions(
                &text_data[decode_start..],
                decode_start_addr,
                stop_addr,
                &got_cache,
            );

            // Discard results from the lookback region
            found.retain(|d| d.address >= chunk_start_addr);
            found
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: assemble a small code buffer and scan it
    fn scan(code: &[u8], base: u64, got: &ahash::AHashMap<u64, u64>) -> Vec<InsnData> {
        let stop = base + code.len() as u64;
        scan_call_instructions(code, base, stop, got)
    }

    // --- Direct CALL rel32 ---

    #[test]
    fn test_direct_call_detected() {
        // CALL rel32: E8 xx xx xx xx  (call +5, i.e. next instruction)
        let code: &[u8] = &[0xE8, 0x00, 0x00, 0x00, 0x00];
        let results = scan(code, 0x1000, &ahash::AHashMap::new());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].address, 0x1000);
        // Target = next_insn (0x1005) + 0 = 0x1005
        assert_eq!(results[0].call_target, Some(0x1005));
    }

    #[test]
    fn test_direct_call_with_offset() {
        // CALL rel32: E8 FB FF FF FF  (call -5, i.e. call self)
        let code: &[u8] = &[0xE8, 0xFB, 0xFF, 0xFF, 0xFF];
        let results = scan(code, 0x2000, &ahash::AHashMap::new());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].call_target, Some(0x2000));
    }

    // --- Direct JMP rel32 (tail call) ---

    #[test]
    fn test_jmp_detected_as_tail_call() {
        // JMP rel32: E9 00 00 00 00  (jmp +5)
        let code: &[u8] = &[0xE9, 0x00, 0x00, 0x00, 0x00];
        let results = scan(code, 0x3000, &ahash::AHashMap::new());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].address, 0x3000);
        assert_eq!(results[0].call_target, Some(0x3005));
    }

    // --- RIP-relative CALL [rip+disp32] ---

    #[test]
    fn test_rip_relative_call_with_got() {
        // FF 15 xx xx xx xx = CALL [rip+disp32]
        // disp32 = 0x00001000
        // instruction at 0x4000, len=6, so rip=0x4006
        // GOT addr = 0x4006 + 0x1000 = 0x5006
        let code: &[u8] = &[0xFF, 0x15, 0x00, 0x10, 0x00, 0x00];
        let mut got = ahash::AHashMap::new();
        got.insert(0x5006, 0xDEAD);
        let results = scan(code, 0x4000, &got);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].call_target, Some(0xDEAD));
    }

    #[test]
    fn test_rip_relative_call_without_got_entry() {
        // Same encoding but no GOT entry — target should be None
        let code: &[u8] = &[0xFF, 0x15, 0x00, 0x10, 0x00, 0x00];
        let results = scan(code, 0x4000, &ahash::AHashMap::new());
        assert_eq!(results.len(), 1);
        assert!(results[0].call_target.is_none());
    }

    // --- GOT stub resolution for direct CALL ---

    #[test]
    fn test_direct_call_resolves_through_got_cache() {
        // CALL rel32 to address 0x2005 which is a stub in the GOT cache
        let code: &[u8] = &[0xE8, 0x00, 0x00, 0x00, 0x00]; // call +5 -> 0x2005
        let mut got = ahash::AHashMap::new();
        got.insert(0x2005, 0xBEEF);
        let results = scan(code, 0x2000, &got);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].call_target, Some(0xBEEF));
    }

    // --- Register-indirect CALL with MOV tracking ---

    #[test]
    fn test_register_indirect_call_with_mov_tracking() {
        // mov rax, [rip+0x1000]  ; load from GOT
        // call rax               ; indirect call
        //
        // REX.W MOV rax, [rip+disp32]: 48 8B 05 00 10 00 00
        //   instruction at 0x5000, len=7, rip=0x5007
        //   GOT addr = 0x5007 + 0x1000 = 0x6007
        // CALL rax: FF D0
        let code: &[u8] = &[
            0x48, 0x8B, 0x05, 0x00, 0x10, 0x00, 0x00, // mov rax, [rip+0x1000]
            0xFF, 0xD0, // call rax
        ];
        let mut got = ahash::AHashMap::new();
        got.insert(0x6007, 0xCAFE);
        let results = scan(code, 0x5000, &got);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].address, 0x5007); // address of the call rax
        assert_eq!(results[0].call_target, Some(0xCAFE));
    }

    // --- Control flow clears register tracking ---

    #[test]
    fn test_conditional_branch_clears_register_tracking() {
        // mov rax, [rip+0x1000]  ; load from GOT
        // je +0                  ; conditional branch (clears tracking)
        // call rax               ; should NOT resolve (tracking was cleared)
        let code: &[u8] = &[
            0x48, 0x8B, 0x05, 0x00, 0x10, 0x00, 0x00, // mov rax, [rip+0x1000]
            0x74, 0x00, // je +0 (jump to next insn)
            0xFF, 0xD0, // call rax
        ];
        let mut got = ahash::AHashMap::new();
        got.insert(0x6007, 0xCAFE);
        let results = scan(code, 0x5000, &got);
        // The call rax should be detected but with None target
        // (register tracking cleared by the conditional branch)
        let call_results: Vec<_> = results.iter().filter(|r| r.address == 0x5009).collect();
        assert_eq!(call_results.len(), 1);
        assert!(call_results[0].call_target.is_none());
    }

    // --- Non-RIP MOV invalidates tracking ---

    #[test]
    fn test_non_rip_mov_invalidates_register() {
        // mov rax, [rip+0x1000]  ; load from GOT
        // mov rax, rbx           ; overwrite rax (invalidates tracking)
        // call rax               ; should NOT resolve
        let code: &[u8] = &[
            0x48, 0x8B, 0x05, 0x00, 0x10, 0x00, 0x00, // mov rax, [rip+0x1000]
            0x48, 0x89, 0xD8, // mov rax, rbx
            0xFF, 0xD0, // call rax
        ];
        let mut got = ahash::AHashMap::new();
        got.insert(0x6007, 0xCAFE);
        let results = scan(code, 0x5000, &got);
        let call_results: Vec<_> = results.iter().filter(|r| r.address == 0x500A).collect();
        assert_eq!(call_results.len(), 1);
        assert!(call_results[0].call_target.is_none());
    }

    // --- Empty input ---

    #[test]
    fn test_empty_input() {
        let results = scan(&[], 0x1000, &ahash::AHashMap::new());
        assert!(results.is_empty());
    }

    // --- Non-call instructions produce no results ---

    #[test]
    fn test_non_call_instructions_ignored() {
        // NOP; NOP; RET
        let code: &[u8] = &[0x90, 0x90, 0xC3];
        let results = scan(code, 0x1000, &ahash::AHashMap::new());
        assert!(results.is_empty());
    }

    // --- Multiple calls ---

    #[test]
    fn test_multiple_calls_detected() {
        // call +5; nop; call +5
        let code: &[u8] = &[
            0xE8, 0x00, 0x00, 0x00, 0x00, // call rel32
            0x90, // nop
            0xE8, 0x00, 0x00, 0x00, 0x00, // call rel32
        ];
        let results = scan(code, 0x1000, &ahash::AHashMap::new());
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].address, 0x1000);
        assert_eq!(results[1].address, 0x1006);
    }

    // --- stop_addr parameter ---

    #[test]
    fn test_stop_addr_limits_decoding() {
        // Two calls but stop_addr cuts off before the second
        let code: &[u8] = &[
            0xE8, 0x00, 0x00, 0x00, 0x00, // call at 0x1000
            0xE8, 0x00, 0x00, 0x00, 0x00, // call at 0x1005
        ];
        let got = ahash::AHashMap::new();
        // Stop before the second call
        let results = scan_call_instructions(code, 0x1000, 0x1005, &got);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].address, 0x1000);
    }

    // --- Parallel vs sequential equivalence on a real binary ---

    #[test]
    fn test_parallel_matches_sequential_on_real_binary() {
        use std::path::PathBuf;

        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let binary_path = PathBuf::from(manifest_dir).join("../target/debug/jonesy");

        let Ok(buffer) = std::fs::read(&binary_path) else {
            // Binary not built yet — skip
            return;
        };

        let elf = match goblin::Object::parse(&buffer) {
            Ok(goblin::Object::Elf(elf)) => elf,
            _ => return,
        };

        // Find .text section
        let text_sh = elf.section_headers.iter().find(|sh| {
            elf.shdr_strtab
                .get_at(sh.sh_name)
                .is_some_and(|n| n == ".text")
        });
        let Some(text_sh) = text_sh else {
            return;
        };
        let offset = text_sh.sh_offset as usize;
        let size = text_sh.sh_size as usize;
        if offset + size > buffer.len() {
            return;
        }
        let text_data = &buffer[offset..offset + size];
        let text_addr = text_sh.sh_addr;

        // Build GOT cache
        let got_cache = got::build_cache(&elf, &buffer);
        let text_end = text_addr + text_data.len() as u64;
        let sequential = scan_call_instructions(text_data, text_addr, text_end, &got_cache);

        // Run parallel (forces multiple chunks by using a small chunk size)
        // We simulate this by manually chunking with RESYNC_LOOKBACK
        let chunk_size = text_data.len() / 4;
        let starts: Vec<usize> = (0..text_data.len()).step_by(chunk_size).collect();

        let mut parallel_results: Vec<InsnData> = Vec::new();
        for &start in &starts {
            let chunk_end_offset = (start + chunk_size).min(text_data.len());
            let chunk_start_addr = text_addr + start as u64;
            let chunk_end_addr = text_addr + chunk_end_offset as u64;

            let decode_start = start.saturating_sub(RESYNC_LOOKBACK);
            let decode_start_addr = text_addr + decode_start as u64;

            let stop_addr = if chunk_end_offset >= text_data.len() {
                text_end
            } else {
                chunk_end_addr
            };

            let mut found = scan_call_instructions(
                &text_data[decode_start..],
                decode_start_addr,
                stop_addr,
                &got_cache,
            );
            found.retain(|d| d.address >= chunk_start_addr);
            parallel_results.extend(found);
        }

        // Compare: same count and same addresses
        assert_eq!(
            sequential.len(),
            parallel_results.len(),
            "Sequential found {} calls, chunked found {}",
            sequential.len(),
            parallel_results.len()
        );

        for (i, (s, p)) in sequential.iter().zip(parallel_results.iter()).enumerate() {
            assert_eq!(
                s.address, p.address,
                "Mismatch at index {}: sequential addr={:#x}, parallel addr={:#x}",
                i, s.address, p.address
            );
            assert_eq!(
                s.call_target, p.call_target,
                "Mismatch at index {} (addr {:#x}): sequential target={:?}, parallel target={:?}",
                i, s.address, s.call_target, p.call_target
            );
        }
    }
}
