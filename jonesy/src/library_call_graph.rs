#![allow(unused_variables)] // TODO Just for now

use crate::binary_format::BinaryRef;
use crate::call_graph::CallerInfo;
use crate::function_index::load_dwarf_sections;
use crate::object_line_table::ObjectLineTable;
use crate::project_context::ProjectContext;
use crate::sym::SymbolIndex;
use goblin::container::{Container, Ctx, Endian};
use goblin::elf::Elf;
use goblin::mach::MachO;
use regex::Regex;
use rustc_demangle::demangle;
use std::borrow::Cow;
use std::collections::HashMap;

/// ARM64 relocation type for BL/B instructions (branch with 26-bit offset)
const ARM64_RELOC_BRANCH26: u8 = 2;

/// ELF ARM64 relocation type for BL/B instructions (call with 26-bit offset)
const R_AARCH64_CALL26: u32 = 283;

/// Call graph for library analysis - uses symbol names instead of addresses.
/// This allows cross-object-file resolution in archives (rlib/staticlib).
pub struct LibraryCallGraph {
    /// Maps target symbol name -> list of CallerInfo (aggregated from all .o files)
    /// Uses 'static lifetime because LibraryCallGraph owns all its data
    edges: HashMap<String, Vec<CallerInfo<'static>>>,
}

impl LibraryCallGraph {
    /// Build a library call graph from a single object file.
    /// Uses relocations to find call targets by symbol name.
    /// Also enriches caller info with file/line from DWARF debug info.
    ///
    /// # Arguments
    /// * `binary` - Parsed binary (MachO or ELF) from the object file
    /// * `buffer` - Raw bytes of the object file
    /// * `project_context` - Project context for source file ownership
    pub fn build_from_object(
        binary: &BinaryRef,
        buffer: &[u8],
        project_context: &ProjectContext,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        match binary {
            BinaryRef::MachO(macho) => {
                Self::build_from_macho_object(macho, buffer, project_context)
            }
            BinaryRef::Elf(elf) => Self::build_from_elf_object(elf, buffer, project_context),
        }
    }

    /// Build a library call graph from a MachO object file.
    fn build_from_macho_object(
        macho: &MachO,
        buffer: &[u8],
        project_context: &ProjectContext,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut edges: HashMap<String, Vec<CallerInfo<'static>>> = HashMap::new();

        // Get symbols for lookup
        let symbols: Vec<(String, u64)> = macho
            .symbols
            .as_ref()
            .map(|s| {
                s.iter()
                    .filter_map(|sym| {
                        let (name, nlist) = sym.ok()?;
                        Some((name.to_string(), nlist.n_value))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Build symbol index for finding containing functions
        let binary_ref = BinaryRef::MachO(macho);
        let symbol_index = SymbolIndex::from_binary(&binary_ref);

        // Load DWARF for file/line lookups
        let dwarf = load_dwarf_sections(&binary_ref, buffer).ok();
        let line_lookup = dwarf.as_ref().and_then(|d| ObjectLineTable::build(d).ok());

        // Create a context for parsing relocations
        let container = if macho.is_64 {
            Container::Big
        } else {
            Container::Little
        };
        let endian = if macho.little_endian {
            Endian::Little
        } else {
            Endian::Big
        };
        let ctx = Ctx::new(container, endian);

        // Find __text section and process its relocations
        for segment in macho.segments.iter() {
            if let Ok(sections) = segment.sections() {
                for (section, _data) in sections {
                    let section_name = section.name().unwrap_or("");
                    if section_name != "__text" {
                        continue;
                    }

                    // Get the section's base address for calculating call site addresses
                    let section_addr = section.addr;

                    // Iterate relocations for this section
                    for reloc in section.iter_relocations(buffer, ctx) {
                        let Ok(reloc_info) = reloc else {
                            continue;
                        };

                        // Only process ARM64_RELOC_BRANCH26 (BL/B instructions)
                        if reloc_info.r_type() != ARM64_RELOC_BRANCH26 {
                            continue;
                        }

                        // Must be an external symbol reference
                        if !reloc_info.is_extern() {
                            continue;
                        }

                        // Get the symbol name being called
                        let sym_index = reloc_info.r_symbolnum();
                        let Some((target_sym_name, _)) = symbols.get(sym_index) else {
                            continue;
                        };

                        // Calculate the call site address
                        let call_site_addr = section_addr + reloc_info.r_address as u64;

                        // Find what function contains this call site
                        let Some((func_addr, func_name)) = symbol_index
                            .as_ref()
                            .and_then(|idx| idx.find_containing(call_site_addr))
                        else {
                            continue;
                        };
                        let func_name = func_name.to_string();

                        // Demangle the target symbol name
                        let target_demangled = {
                            let stripped =
                                target_sym_name.strip_prefix("_").unwrap_or(target_sym_name);
                            format!("{:#}", demangle(stripped))
                        };

                        // Look up file/line/column from DWARF at call site
                        let (file, line, column) = line_lookup
                            .as_ref()
                            .and_then(|lt| lt.lookup(call_site_addr))
                            .unwrap_or((None, None, None));

                        // If call site points to non-crate code (stdlib/dependency), find
                        // the last crate source line between function start and call site
                        let (file, line, column) = if file
                            .as_ref()
                            .is_some_and(|f| !project_context.is_crate_source(f))
                        {
                            // Try to find precise line in crate source
                            if let Some(lt) = line_lookup.as_ref()
                                && let Some((crate_file, crate_line, crate_col)) = lt
                                    .get_crate_line_in_range(
                                        func_addr,
                                        call_site_addr,
                                        project_context,
                                    )
                            {
                                (Some(crate_file), Some(crate_line), crate_col)
                            } else {
                                // Fall back to function start address
                                line_lookup
                                    .as_ref()
                                    .and_then(|lt| lt.lookup(func_addr))
                                    .unwrap_or((None, None, None))
                            }
                        } else {
                            (file, line, column)
                        };

                        // Record the call: target_symbol -> caller
                        edges.entry(target_demangled).or_default().push(CallerInfo {
                            caller_name: Cow::Owned(func_name),
                            caller_start_address: func_addr,
                            caller_file: file.clone(),
                            call_site_addr,
                            file,
                            line,
                            column,
                        });
                    }
                }
            }
        }

        Ok(Self { edges })
    }

    /// Build a library call graph from an ELF object file.
    fn build_from_elf_object(
        elf: &Elf,
        buffer: &[u8],
        project_context: &ProjectContext,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut edges: HashMap<String, Vec<CallerInfo<'static>>> = HashMap::new();

        // Get symbols for lookup
        let symbols: Vec<(String, u64)> = elf
            .syms
            .iter()
            .filter_map(|sym| {
                let name = elf.strtab.get_at(sym.st_name)?;
                Some((name.to_string(), sym.st_value))
            })
            .collect();

        // Build symbol index for finding containing functions
        let binary_ref = BinaryRef::Elf(elf);
        let symbol_index = SymbolIndex::from_binary(&binary_ref);

        // Load DWARF for file/line lookups
        let dwarf = load_dwarf_sections(&binary_ref, buffer).ok();
        let line_lookup = dwarf.as_ref().and_then(|d| ObjectLineTable::build(d).ok());

        // Debug: list all section names to understand the ELF layout
        eprintln!("  ELF sections ({} total):", elf.section_headers.len());
        for (i, sh) in elf.section_headers.iter().enumerate() {
            let name = elf.shdr_strtab.get_at(sh.sh_name).unwrap_or("???");
            if name.contains("text") || name.contains("rela") {
                eprintln!(
                    "    [{i}] {name} (type={}, info={}, size={})",
                    sh.sh_type, sh.sh_info, sh.sh_size
                );
            }
        }
        eprintln!("  ELF symbols: {} total", symbols.len());
        eprintln!(
            "  Symbol index: {}",
            if symbol_index.is_some() {
                "built"
            } else {
                "empty"
            }
        );

        // Find .text section index
        let text_section_idx = elf
            .section_headers
            .iter()
            .position(|sh| elf.shdr_strtab.get_at(sh.sh_name) == Some(".text"));

        let Some(text_idx) = text_section_idx else {
            eprintln!("  No .text section found!");
            return Ok(Self { edges });
        };
        eprintln!("  .text section index: {text_idx}");

        // Find relocation sections for .text (may be .rela.text or .rela.text.*)
        for sh in &elf.section_headers {
            let name = elf.shdr_strtab.get_at(sh.sh_name).unwrap_or("");
            if !name.starts_with(".rela.text") {
                continue;
            }

            // Get the section this relocation applies to
            let target_section_idx = sh.sh_info as usize;
            let target_name = elf
                .section_headers
                .get(target_section_idx)
                .and_then(|s| elf.shdr_strtab.get_at(s.sh_name))
                .unwrap_or("");
            // Only process relocations for .text sections
            if !target_name.starts_with(".text") {
                continue;
            }

            eprintln!(
                "  Processing relocations: {name} -> {target_name} (idx={target_section_idx})"
            );

            // Get the target section's base address
            let text_section = &elf.section_headers[target_section_idx];
            let text_addr = text_section.sh_addr;

            // Parse relocations
            let rela_offset = sh.sh_offset as usize;
            let rela_size = sh.sh_size as usize;
            let rela_entsize = sh.sh_entsize as usize;

            if rela_entsize == 0 || rela_size % rela_entsize != 0 {
                continue;
            }

            let num_relocs = rela_size / rela_entsize;
            let mut call26_count = 0u32;

            for i in 0..num_relocs {
                let offset = rela_offset + i * rela_entsize;
                if offset + rela_entsize > buffer.len() {
                    break;
                }

                // Parse Rela entry (64-bit: r_offset, r_info, r_addend - each 8 bytes)
                let r_offset =
                    u64::from_le_bytes(buffer[offset..offset + 8].try_into().unwrap_or([0; 8]));
                let r_info = u64::from_le_bytes(
                    buffer[offset + 8..offset + 16].try_into().unwrap_or([0; 8]),
                );

                // Extract relocation type and symbol index from r_info
                let r_type = (r_info & 0xffffffff) as u32;
                let r_sym = (r_info >> 32) as usize;

                // Only process R_AARCH64_CALL26 relocations
                if r_type != R_AARCH64_CALL26 {
                    continue;
                }
                call26_count += 1;

                // Get the target symbol name
                let Some((target_sym_name, _)) = symbols.get(r_sym) else {
                    continue;
                };

                // Calculate the call site address
                let call_site_addr = text_addr + r_offset;

                // Find what function contains this call site
                let Some((func_addr, func_name)) = symbol_index
                    .as_ref()
                    .and_then(|idx| idx.find_containing(call_site_addr))
                else {
                    continue;
                };
                let func_name = func_name.to_string();

                // Demangle the target symbol name (ELF doesn't use leading underscore)
                let target_demangled = format!("{:#}", demangle(target_sym_name));

                // Look up file/line/column from DWARF at call site
                let (file, line, column) = line_lookup
                    .as_ref()
                    .and_then(|lt| lt.lookup(call_site_addr))
                    .unwrap_or((None, None, None));

                // If call site points to non-crate code (stdlib/dependency), find
                // the last crate source line between function start and call site
                let (file, line, column) = if file
                    .as_ref()
                    .is_some_and(|f| !project_context.is_crate_source(f))
                {
                    // Try to find precise line in crate source
                    if let Some(lt) = line_lookup.as_ref()
                        && let Some((crate_file, crate_line, crate_col)) =
                            lt.get_crate_line_in_range(func_addr, call_site_addr, project_context)
                    {
                        (Some(crate_file), Some(crate_line), crate_col)
                    } else {
                        // Fall back to function start address
                        line_lookup
                            .as_ref()
                            .and_then(|lt| lt.lookup(func_addr))
                            .unwrap_or((None, None, None))
                    }
                } else {
                    (file, line, column)
                };

                // Record the call: target_symbol -> caller
                edges.entry(target_demangled).or_default().push(CallerInfo {
                    caller_name: Cow::Owned(func_name),
                    caller_start_address: func_addr,
                    caller_file: file.clone(),
                    call_site_addr,
                    file,
                    line,
                    column,
                });
            }
            eprintln!("    {num_relocs} relocs total, {call26_count} CALL26");
        }

        eprintln!("  ELF call graph: {} edges", edges.len());
        Ok(Self { edges })
    }

    /// Merge another LibraryCallGraph into this one.
    pub fn merge(&mut self, other: Self) {
        for (target, callers) in other.edges {
            self.edges.entry(target).or_default().extend(callers);
        }
    }

    /// Get all callers of a symbol by name (demangled).
    pub fn get_callers(&self, symbol_name: &str) -> Vec<CallerInfo<'static>> {
        self.edges.get(symbol_name).cloned().unwrap_or_default()
    }

    /// Get all callers of symbols matching a pattern.
    pub fn get_callers_matching(&self, pattern: &Regex) -> Vec<(&str, &[CallerInfo<'static>])> {
        self.edges
            .iter()
            .filter(|(name, _)| pattern.is_match(name))
            .map(|(name, callers)| (name.as_str(), callers.as_slice()))
            .collect()
    }

    /// Get all target symbol names in the call graph.
    pub fn target_symbols(&self) -> impl Iterator<Item = &str> {
        self.edges.keys().map(|s| s.as_str())
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Create an empty library call graph.
    pub fn empty() -> Self {
        Self {
            edges: HashMap::new(),
        }
    }
}
