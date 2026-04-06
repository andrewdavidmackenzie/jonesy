//! Binary format abstraction for Mach-O and ELF.
//!
//! Provides `BinaryRef` — a format-aware wrapper that gives uniform
//! access to sections and symbols across both binary formats.

use goblin::elf::Elf;
use goblin::mach::MachO;

/// A reference to either a Mach-O or ELF binary.
pub enum BinaryRef<'a> {
    MachO(&'a MachO<'a>),
    Elf(&'a Elf<'a>),
}

impl<'a> BinaryRef<'a> {
    /// Find a section by its platform-agnostic purpose.
    /// Handles naming differences: `__text` (MachO) vs `.text` (ELF).
    pub fn find_section(&self, buffer: &'a [u8], name: &str) -> Option<(u64, &'a [u8])> {
        match self {
            BinaryRef::MachO(macho) => find_macho_section(macho, buffer, name),
            BinaryRef::Elf(elf) => find_elf_section(elf, buffer, name),
        }
    }

    /// Get the text section name for this binary format.
    pub fn text_section_name(&self) -> &'static str {
        match self {
            BinaryRef::MachO(_) => "__text",
            BinaryRef::Elf(_) => ".text",
        }
    }

    /// Check if this binary has DWARF debug sections.
    pub fn has_dwarf(&self) -> bool {
        match self {
            BinaryRef::MachO(macho) => has_macho_dwarf(macho),
            BinaryRef::Elf(elf) => has_elf_dwarf(elf),
        }
    }

    /// Convert a gimli DWARF section name (e.g., ".debug_info") to the
    /// format-specific name used in this binary.
    pub fn dwarf_section_name(&self, gimli_name: &str) -> String {
        match self {
            BinaryRef::MachO(_) => {
                // ".debug_info" -> "__debug_info"
                format!("__{}", &gimli_name[1..])
            }
            BinaryRef::Elf(_) => {
                // ELF uses gimli names directly
                gimli_name.to_string()
            }
        }
    }

    /// Returns true if this is an ELF binary.
    pub fn is_elf(&self) -> bool {
        matches!(self, BinaryRef::Elf(_))
    }
}

fn find_macho_section<'a>(
    macho: &MachO<'a>,
    buffer: &'a [u8],
    name: &str,
) -> Option<(u64, &'a [u8])> {
    for segment in macho.segments.iter() {
        if let Ok(sections) = segment.sections() {
            for (section, _) in sections {
                if let Ok(section_name) = section.name() {
                    if section_name == name {
                        let offset = section.offset as usize;
                        let size = section.size as usize;
                        if offset + size <= buffer.len() {
                            return Some((section.addr, &buffer[offset..offset + size]));
                        }
                    }
                }
            }
        }
    }
    None
}

fn find_elf_section<'a>(elf: &Elf<'a>, buffer: &'a [u8], name: &str) -> Option<(u64, &'a [u8])> {
    for section_header in &elf.section_headers {
        if let Some(section_name) = elf.shdr_strtab.get_at(section_header.sh_name) {
            if section_name == name {
                let offset = section_header.sh_offset as usize;
                let size = section_header.sh_size as usize;
                if offset + size <= buffer.len() {
                    return Some((section_header.sh_addr, &buffer[offset..offset + size]));
                }
            }
        }
    }
    None
}

fn has_macho_dwarf(macho: &MachO) -> bool {
    for segment in macho.segments.iter() {
        if let Ok(name) = segment.name() {
            if name == "__DWARF" {
                return true;
            }
        }
        if let Ok(sections) = segment.sections() {
            for (section, _) in sections {
                if let Ok(name) = section.name() {
                    if name.starts_with("__debug_") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn has_elf_dwarf(elf: &Elf) -> bool {
    for section_header in &elf.section_headers {
        if let Some(name) = elf.shdr_strtab.get_at(section_header.sh_name) {
            if name.starts_with(".debug_") {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_section_name() {
        // We can't easily construct MachO/Elf in tests without real binaries,
        // but we can test the naming logic once we have a BinaryRef.
        // This test uses a real binary if available.
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(goblin::Object::Mach(goblin::mach::Mach::Binary(ref macho))) =
                goblin::Object::parse(&buffer)
            {
                let binary_ref = BinaryRef::MachO(macho);
                assert_eq!(binary_ref.text_section_name(), "__text");
                assert!(!binary_ref.is_elf());
            }
        }
    }

    #[test]
    fn test_dwarf_section_name_macho() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(goblin::Object::Mach(goblin::mach::Mach::Binary(ref macho))) =
                goblin::Object::parse(&buffer)
            {
                let binary_ref = BinaryRef::MachO(macho);
                assert_eq!(binary_ref.dwarf_section_name(".debug_info"), "__debug_info");
            }
        }
    }

    #[test]
    fn test_find_text_section() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(goblin::Object::Mach(goblin::mach::Mach::Binary(ref macho))) =
                goblin::Object::parse(&buffer)
            {
                let binary_ref = BinaryRef::MachO(macho);
                let text_name = binary_ref.text_section_name();
                let result = binary_ref.find_section(&buffer, text_name);
                assert!(result.is_some(), "Should find __text section");
                if let Some((addr, data)) = result {
                    assert!(addr > 0, "Section address should be non-zero");
                    assert!(!data.is_empty(), "Section data should not be empty");
                }
            }
        }
    }

    #[test]
    fn test_has_dwarf() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(goblin::Object::Mach(goblin::mach::Mach::Binary(ref macho))) =
                goblin::Object::parse(&buffer)
            {
                let binary_ref = BinaryRef::MachO(macho);
                // Don't assert true/false since it depends on build config,
                // just verify the method doesn't panic
                let _ = binary_ref.has_dwarf();
            }
        }
    }

    #[test]
    fn test_is_elf() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(goblin::Object::Mach(goblin::mach::Mach::Binary(ref macho))) =
                goblin::Object::parse(&buffer)
            {
                let binary_ref = BinaryRef::MachO(macho);
                assert!(!binary_ref.is_elf(), "MachO binary should not be ELF");
            }
        }
    }
}
