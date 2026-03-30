#![allow(unused_variables)] // TODO Just for now
#![allow(dead_code)] // TODO Just for now

pub use crate::call_graph::{CallGraph, CallerInfo};
pub use crate::crate_line_table::{CrateLineEntry, CrateLineTable};
pub use crate::debug_info::load_debug_info;
pub use crate::debug_info::{DSymInfo, DebugInfo, DebugMapInfo, ObjectFileInfo, find_dsym};
pub use crate::full_line_table::{FullLineEntry, FullLineTable};
pub(crate) use crate::function_index::resolve_line_file_path;
pub use crate::function_index::{FunctionIndex, FunctionInfo, get_functions_from_dwarf};
pub use crate::library_call_graph::LibraryCallGraph;
pub use crate::project_context::ProjectContext;
pub use crate::string_tables::StringTables;

use goblin::Object;
use goblin::mach::MachO;
use regex::Regex;
use rustc_demangle::demangle;
use std::io;

#[allow(clippy::large_enum_variant)]
pub enum SymbolTable<'a> {
    MachO(goblin::mach::Mach<'a>),
    Archive(goblin::archive::Archive<'a>),
}

impl<'a> SymbolTable<'a> {
    /// Parse a binary buffer into a SymbolTable.
    pub fn from(buffer: &'a [u8]) -> io::Result<Self> {
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

    /// Get the MachO binary, if this is a MachO (not an archive).
    /// Panics on fat binaries — callers should handle that case.
    pub fn macho(&self) -> Option<&MachO<'_>> {
        match self {
            SymbolTable::MachO(goblin::mach::Mach::Binary(macho)) => Some(macho),
            _ => None,
        }
    }

    /// Check if the binary has any DWARF debug sections.
    pub fn has_dwarf_sections(&self) -> bool {
        let Some(macho) = self.macho() else {
            return false;
        };
        for segment in macho.segments.iter() {
            if let Ok(sects) = segment.sections() {
                for (section, _) in sects {
                    if let Ok(name) = section.name()
                        && name.starts_with("__debug_")
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Returns the first symbol found whose name matches the given regex pattern.
    /// The pattern is matched against the demangled symbol name.
    pub fn find_symbol_containing(
        &self,
        pattern: &str,
    ) -> Result<Option<(String, String)>, regex::Error> {
        let Some(macho) = self.macho() else {
            return Ok(None);
        };
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
    pub fn find_all_symbols_matching(
        &self,
        patterns: &[&str],
    ) -> Result<Vec<(String, String)>, regex::Error> {
        let Some(macho) = self.macho() else {
            return Ok(Vec::new());
        };
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
                    break;
                }
            }
        }
        Ok(results)
    }

    /// Returns the address of the first defined symbol found whose name matches `name` exactly.
    pub fn find_symbol_address(&self, name: &str) -> Option<u64> {
        let macho = self.macho()?;
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
}

/// Entry in the symbol index with lazy demangling.
/// Stores mangled name and caches demangled result on first access.
/// Uses OnceLock for thread-safe lazy initialization (required for rayon).
struct SymbolEntry {
    address: u64,
    /// Mangled symbol name (without leading underscore)
    mangled: String,
    /// Lazily computed demangled name (thread-safe)
    demangled: std::sync::OnceLock<String>,
}

impl SymbolEntry {
    fn new(address: u64, mangled: String) -> Self {
        Self {
            address,
            mangled,
            demangled: std::sync::OnceLock::new(),
        }
    }

    /// Get the demangled name, computing it lazily on first access.
    fn demangled(&self) -> &str {
        self.demangled
            .get_or_init(|| format!("{:#}", demangle(&self.mangled)))
    }
}

impl std::fmt::Debug for SymbolEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SymbolEntry")
            .field("address", &self.address)
            .field("mangled", &self.mangled)
            .field("demangled", &self.demangled.get())
            .finish()
    }
}

/// Precomputed sorted symbol index for efficient function lookups.
/// Build once with `SymbolIndex::new()` and reuse for many lookups.
/// Uses lazy demangling to avoid upfront cost of demangling all symbols.
#[derive(Debug)]
pub struct SymbolIndex {
    /// Sorted by address, with lazy demangling
    entries: Vec<SymbolEntry>,
}

impl SymbolIndex {
    /// Build a symbol index from a MachO binary. Call once, reuse for many lookups.
    /// Symbol names are demangled lazily on first access for better performance.
    /// Uses parallel sorting for large symbol tables.
    pub fn new(macho: &MachO) -> Option<Self> {
        use rayon::prelude::*;

        let symbols = macho.symbols.as_ref()?;

        // First pass: collect raw symbol data (sequential - iterator not Send)
        let raw_symbols: Vec<(u64, &str)> = symbols
            .iter()
            .filter_map(|s| s.ok())
            .filter(|(name, nlist)| !nlist.is_undefined() && !name.is_empty())
            .map(|(name, nlist)| (nlist.n_value, name))
            .collect();

        // Second pass: process in parallel (strip prefix, create entries)
        let mut entries: Vec<SymbolEntry> = raw_symbols
            .par_iter()
            .map(|(addr, name)| {
                let stripped = name.strip_prefix("_").unwrap_or(name);
                SymbolEntry::new(*addr, stripped.to_string())
            })
            .collect();

        // Parallel sort by address
        entries.par_sort_by_key(|e| e.address);
        Some(Self { entries })
    }

    /// Find the function containing `addr` using binary search.
    /// Returns a borrowed reference to the name (caller must ensure SymbolIndex outlives usage).
    /// Demangling is performed lazily on first access to each symbol.
    pub fn find_containing(&self, addr: u64) -> Option<(u64, &str)> {
        // Binary search for the largest address <= addr
        match self.entries.binary_search_by_key(&addr, |e| e.address) {
            Ok(i) => Some((self.entries[i].address, self.entries[i].demangled())),
            Err(0) => None, // addr is before all functions
            Err(i) => Some((self.entries[i - 1].address, self.entries[i - 1].demangled())),
        }
    }
}
