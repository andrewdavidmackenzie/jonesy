#![allow(unused_variables)] // TODO Just for now
#![allow(dead_code)] // TODO Just for now

// Re-export types and functions from the extracted modules for backward compatibility
pub use crate::call_graph::{CallGraph, CallerInfo, find_callers_with_debug_info};
pub use crate::crate_line_table::{CrateLineEntry, CrateLineTable};
pub use crate::debug_info::{DSymInfo, DebugInfo, find_dsym, has_dwarf_sections, load_debug_info};
pub use crate::full_line_table::{FullLineEntry, FullLineTable};
pub use crate::function_index::{FunctionIndex, FunctionInfo, get_functions_from_dwarf};
pub use crate::library_call_graph::LibraryCallGraph;
pub use crate::project_context::ProjectContext;
pub use crate::string_tables::StringTables;

use goblin::Object;
use goblin::archive::Archive;
use goblin::mach::segment::{Section, SectionData, Segment};
use goblin::mach::{Mach, MachO};
use regex::Regex;
use rustc_demangle::demangle;
use std::io;
use std::sync::OnceLock;

#[allow(clippy::large_enum_variant)]
pub enum SymbolTable<'a> {
    MachO(Mach<'a>),
    Archive(Archive<'a>),
}

pub fn read_symbols(buffer: &'_ [u8]) -> io::Result<SymbolTable<'_>> {
    // Use goblin's Object::parse to auto-detect the file type
    match Object::parse(buffer).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))? {
        Object::Mach(mach) => Ok(SymbolTable::MachO(mach)),
        Object::Archive(archive) => Ok(SymbolTable::Archive(archive)),
        Object::Elf(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ELF format not supported (macOS only)",
        )),
        Object::PE(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "PE format not supported (macOS only)",
        )),
        Object::COFF(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "COFF format not supported (macOS only)",
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unknown binary format",
        )),
    }
}

/// Returns the first symbol found whose name matches the given regex pattern.
/// The pattern is matched against the demangled symbol name.
/// Example: "rust_panic$" matches symbols ending in "rust_panic"
pub fn find_symbol_containing(
    macho: &MachO,
    pattern: &str,
) -> Result<Option<(String, String)>, regex::Error> {
    let regex = Regex::new(pattern)?;
    let symbols = match macho.symbols.as_ref() {
        Some(s) => s,
        None => return Ok(None),
    };
    for (sym_name, _) in symbols.iter().flatten() {
        let stripped = sym_name.strip_prefix("_").unwrap_or(sym_name);
        let demangled = format!("{:#}", demangle(stripped));
        if regex.is_match(&demangled) {
            return Ok(Some((sym_name.to_string(), demangled)));
        }
    }
    Ok(None)
}

/// Returns all symbols whose demangled names match any of the given patterns.
/// Returns a vector of (mangled_name, demangled_name) tuples.
pub fn find_all_symbols_matching(
    macho: &MachO,
    patterns: &[&str],
) -> Result<Vec<(String, String)>, regex::Error> {
    let regexes: Vec<Regex> = patterns
        .iter()
        .map(|p| Regex::new(p))
        .collect::<Result<Vec<_>, _>>()?;

    let mut results = Vec::new();
    let symbols = match macho.symbols.as_ref() {
        Some(s) => s,
        None => return Ok(results),
    };

    for (sym_name, _) in symbols.iter().flatten() {
        let stripped = sym_name.strip_prefix("_").unwrap_or(sym_name);
        let demangled = format!("{:#}", demangle(stripped));
        for regex in &regexes {
            if regex.is_match(&demangled) {
                results.push((sym_name.to_string(), demangled.clone()));
                break; // Only add once even if multiple patterns match
            }
        }
    }
    Ok(results)
}

// TODO Restrict this to text segments?
/// Returns the address of the first defined symbol found whose name matches `name` exactly.
/// Skips undefined/import symbols which have n_value == 0.
pub fn find_symbol_address(macho: &MachO, name: &str) -> Option<u64> {
    let symbols = macho.symbols.as_ref()?;
    for symbol in symbols.iter() {
        if let Ok((sym_name, nlist)) = symbol
            && sym_name == name
            && !nlist.is_undefined()
            && nlist.n_value != 0
        {
            return Some(nlist.n_value);
        }
    }
    None
}

pub(crate) fn get_text_section<'a>(macho: &MachO, buffer: &'a [u8]) -> Option<(u64, &'a [u8])> {
    get_section_by_name(macho, buffer, "__text")
}

fn get_section_by_name<'a>(macho: &MachO, buffer: &'a [u8], name: &str) -> Option<(u64, &'a [u8])> {
    for segment in &macho.segments {
        for (section, section_data) in segment.sections().unwrap() {
            if section.name().unwrap() == name {
                let offset = section.offset as usize;
                let size = section.size as usize;
                return Some((section.addr, &buffer[offset..offset + size]));
            }
        }
    }
    None
}

/// Find a segment by name
fn find_segment<'a>(macho: &'a MachO, segment_name: &str) -> Option<&'a Segment<'a>> {
    for segment in macho.segments.iter() {
        if let Ok(name) = segment.name()
            && name == segment_name
        {
            return Some(segment);
        }
    }
    None
}

/// Entry in the symbol index with lazy demangling.
/// Stores mangled name and caches demangled result on first access.
/// Uses OnceLock for thread-safe lazy initialization (required for rayon).
struct SymbolEntry {
    address: u64,
    mangled_name: String,
    demangled_cache: OnceLock<String>,
}

impl SymbolEntry {
    fn new(address: u64, mangled_name: String) -> Self {
        Self {
            address,
            mangled_name,
            demangled_cache: OnceLock::new(),
        }
    }

    fn demangled_name(&self) -> &str {
        self.demangled_cache.get_or_init(|| {
            let stripped = self
                .mangled_name
                .strip_prefix('_')
                .unwrap_or(&self.mangled_name);
            format!("{:#}", demangle(stripped))
        })
    }
}

impl std::fmt::Debug for SymbolEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SymbolEntry")
            .field("address", &self.address)
            .field("mangled_name", &self.mangled_name)
            .field("demangled_name", &self.demangled_name())
            .finish()
    }
}

/// Index for fast function lookup by address using binary search.
/// Sorts functions by address on creation and uses binary search for O(log n) lookups.
#[derive(Debug)]
pub struct SymbolIndex {
    symbols: Vec<SymbolEntry>,
}

impl SymbolIndex {
    /// Build a symbol index from a MachO binary.
    /// Extracts symbols, sorts by address, and enables O(log n) binary search lookups.
    pub fn new(macho: &MachO) -> Option<Self> {
        let symbols = macho.symbols.as_ref()?;

        let mut symbol_entries: Vec<SymbolEntry> = symbols
            .iter()
            .filter_map(|sym| {
                let (name, nlist) = sym.ok()?;
                // Skip undefined symbols and symbols with no address
                if nlist.is_undefined() || nlist.n_value == 0 {
                    return None;
                }
                Some(SymbolEntry::new(nlist.n_value, name.to_string()))
            })
            .collect();

        // Sort by address for binary search
        symbol_entries.sort_by_key(|e| e.address);

        Some(Self {
            symbols: symbol_entries,
        })
    }

    /// Find the function containing the given address using binary search.
    /// Returns (function_address, function_name) if found.
    /// Time complexity: O(log n) via binary search.
    pub fn find_containing(&self, addr: u64) -> Option<(u64, &str)> {
        if self.symbols.is_empty() {
            return None;
        }

        // Binary search for the largest start address <= addr
        let idx = match self.symbols.binary_search_by_key(&addr, |s| s.address) {
            Ok(i) => i,            // Exact match on address
            Err(0) => return None, // addr is before first symbol
            Err(i) => i - 1,       // Use previous symbol
        };

        let symbol = &self.symbols[idx];
        // We don't have end addresses, so we can only check that we found a symbol
        // at or before the target address. The caller should validate the range.
        Some((symbol.address, symbol.demangled_name()))
    }
}

fn find_sections<'a>(macho: &'a MachO, section_name: &str) -> Vec<(Section, SectionData<'a>)> {
    let mut results = Vec::new();
    for segment in macho.segments.iter() {
        if let Ok(sections) = segment.sections() {
            for (section, data) in sections {
                if let Ok(name) = section.name()
                    && name == section_name
                {
                    results.push((section, data));
                }
            }
        }
    }
    results
}
