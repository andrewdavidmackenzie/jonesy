//! ELF relocation support for DWARF sections in .o files
//!
//! Per-function sections in ELF .o files (.text.func1, .text.func2, etc.) all start at
//! address 0x0, creating overlapping address spaces. DWARF line tables reference these
//! addresses without specifying which section they belong to.
//!
//! Section membership is encoded via ELF relocations in .rela.debug_line, .rela.debug_info, etc.
//! This module parses those relocations and provides a gimli::Relocate implementation.

use goblin::elf::Elf;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Tracks the current section being processed during DWARF parsing
    /// This is set by the Relocate implementation when it sees a relocation
    static CURRENT_SECTION_CONTEXT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Get the current section context (set by relocations during DWARF parsing)
pub fn get_current_section() -> Option<String> {
    CURRENT_SECTION_CONTEXT.with(|ctx| ctx.borrow().clone())
}

/// Maps byte offsets in a DWARF section to their target section names
#[derive(Debug, Clone)]
pub struct RelocationMap {
    /// Maps byte offset in DWARF section → target section name
    /// For example: offset 0x146 in .debug_line → ".text._ZN...cause_assert_eq..."
    relocations: HashMap<usize, String>,
}

impl RelocationMap {
    /// Create an empty relocation map
    pub fn empty() -> Self {
        Self {
            relocations: HashMap::new(),
        }
    }

    /// Parse relocations for a specific DWARF section from an ELF file
    ///
    /// # Arguments
    /// * `elf` - Parsed ELF file
    /// * `buffer` - Raw bytes of the ELF file
    /// * `rela_section_name` - Name of relocation section (e.g., ".rela.debug_line")
    pub fn parse_from_elf(elf: &Elf, buffer: &[u8], rela_section_name: &str) -> Self {
        let mut relocations = HashMap::new();

        // Find the relocation section
        let rela_section = elf
            .section_headers
            .iter()
            .find(|sh| elf.shdr_strtab.get_at(sh.sh_name) == Some(rela_section_name));

        let Some(rela_sh) = rela_section else {
            return Self::empty();
        };

        // Parse relocation entries
        let rela_offset = rela_sh.sh_offset as usize;
        let rela_size = rela_sh.sh_size as usize;
        let rela_entsize = rela_sh.sh_entsize as usize;

        if rela_entsize == 0 || rela_size % rela_entsize != 0 {
            return Self::empty();
        }

        let num_relocs = rela_size / rela_entsize;

        for i in 0..num_relocs {
            let offset = rela_offset + i * rela_entsize;
            if offset + rela_entsize > buffer.len() {
                break;
            }

            // Parse Rela entry (64-bit: r_offset, r_info, r_addend - each 8 bytes)
            let r_offset =
                u64::from_le_bytes(buffer[offset..offset + 8].try_into().unwrap_or([0; 8]));
            let r_info =
                u64::from_le_bytes(buffer[offset + 8..offset + 16].try_into().unwrap_or([0; 8]));
            // r_addend is at offset+16, but we don't need it for our use case

            // Extract symbol index from r_info
            let r_sym = (r_info >> 32) as usize;

            // Look up the symbol to get the target section
            let symbol = match elf.syms.get(r_sym) {
                Some(sym) => sym,
                None => continue,
            };

            // Get the section this symbol refers to
            let target_section_idx = symbol.st_shndx;
            if target_section_idx >= elf.section_headers.len() {
                continue;
            }

            let target_section = &elf.section_headers[target_section_idx];
            let Some(target_name) = elf.shdr_strtab.get_at(target_section.sh_name) else {
                continue;
            };

            // Only track relocations to .text.* sections (function sections)
            if target_name.starts_with(".text.") {
                relocations.insert(r_offset as usize, target_name.to_string());
            }
        }

        Self { relocations }
    }

    /// Look up which section a byte offset in the DWARF section refers to
    pub fn lookup_section(&self, offset: usize) -> Option<&str> {
        self.relocations.get(&offset).map(|s| s.as_str())
    }
}

impl gimli::Relocate<usize> for RelocationMap {
    fn relocate_address(&self, offset: usize, value: u64) -> Result<u64, gimli::Error> {
        // Look up which section this address belongs to based on byte offset in DWARF section
        if let Some(section_name) = self.lookup_section(offset) {
            // Store the section name in thread-local storage so line entry creation can access it
            CURRENT_SECTION_CONTEXT.with(|ctx| {
                *ctx.borrow_mut() = Some(section_name.to_string());
            });
        } else {
            // No relocation at this offset - clear the context
            CURRENT_SECTION_CONTEXT.with(|ctx| {
                *ctx.borrow_mut() = None;
            });
        }

        // We don't modify the address value - .o files use section-relative addresses
        // The section membership is what matters, not the numeric value
        Ok(value)
    }

    fn relocate_offset(&self, _offset: usize, value: usize) -> Result<usize, gimli::Error> {
        // Offsets don't need relocation for our use case
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_map() {
        let map = RelocationMap::empty();
        assert_eq!(map.lookup_section(0), None);
    }
}
