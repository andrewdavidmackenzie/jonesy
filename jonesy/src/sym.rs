pub use crate::call_graph::{CallGraph, CallerInfo};
pub use crate::crate_line_table::{CrateLineEntry, CrateLineTable};
pub use crate::debug_info::load_debug_info;
pub use crate::debug_info::{DSymInfo, DebugInfo, DebugMapInfo, ObjectFileInfo};
pub use crate::full_line_table::{FullLineEntry, FullLineTable};
pub(crate) use crate::function_index::resolve_line_file_path;
pub use crate::function_index::{FunctionIndex, FunctionInfo, get_functions_from_dwarf};
pub use crate::library_call_graph::LibraryCallGraph;
pub use crate::project_context::ProjectContext;
pub use crate::string_tables::StringTables;

use crate::binary_format::BinaryRef;
use goblin::Object;
use goblin::elf::Elf;
use goblin::mach::MachO;
use regex::Regex;
use rustc_demangle::demangle;
use std::io;

#[allow(clippy::large_enum_variant)]
pub enum SymbolTable<'a> {
    MachO(goblin::mach::Mach<'a>),
    Elf(goblin::elf::Elf<'a>),
    Archive(goblin::archive::Archive<'a>),
}

impl<'a> SymbolTable<'a> {
    /// Parse a binary buffer into a SymbolTable.
    pub fn from(buffer: &'a [u8]) -> io::Result<Self> {
        match Object::parse(buffer).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))? {
            Object::Mach(mach) => Ok(SymbolTable::MachO(mach)),
            Object::Elf(elf) => Ok(SymbolTable::Elf(elf)),
            Object::Archive(archive) => Ok(SymbolTable::Archive(archive)),
            Object::PE(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PE format not supported",
            )),
            Object::COFF(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "COFF format not supported",
            )),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unknown binary format",
            )),
        }
    }

    /// Get the MachO binary if this is a MachO (not an archive).
    /// Panics on fat binaries — callers should handle that case
    pub fn macho(&self) -> Option<&MachO<'_>> {
        match self {
            SymbolTable::MachO(goblin::mach::Mach::Binary(macho)) => Some(macho),
            _ => None,
        }
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
/// Stores mangled name and caches the demangled result on first access.
/// Uses OnceLock for thread-safe lazy initialization (required for rayon).
#[derive(Debug)]
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

/// Precomputed sorted symbol index for efficient function lookups.
/// Build once with `SymbolIndex::new()` and reuse for many lookups.
/// Uses lazy demangling to avoid the upfront cost of demangling all symbols.
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

    /// Build a symbol index from an ELF binary. Call once, reuse for many lookups.
    /// Symbol names are demangled lazily on first access for better performance.
    /// Uses parallel sorting for large symbol tables.
    pub fn from_elf(elf: &Elf) -> Option<Self> {
        use rayon::prelude::*;

        // First pass: collect raw symbol data from ELF symbol table
        // Filter for defined, non-empty, function-like symbols (matching MachO behavior)
        let raw_symbols: Vec<(u64, String)> = elf
            .syms
            .iter()
            .filter_map(|sym| {
                // Only include defined symbols with valid addresses
                if sym.st_value == 0 || sym.st_shndx == 0 {
                    return None;
                }
                // Get symbol name from string table
                elf.strtab.get_at(sym.st_name).map(|name| {
                    // ELF symbols don't have leading underscore (unlike MachO)
                    (sym.st_value, name.to_string())
                })
            })
            .filter(|(_, name)| !name.is_empty())
            .collect();

        if raw_symbols.is_empty() {
            return None;
        }

        // Second pass: create entries in parallel
        let mut entries: Vec<SymbolEntry> = raw_symbols
            .par_iter()
            .map(|(addr, name)| SymbolEntry::new(*addr, name.clone()))
            .collect();

        // Parallel sort by address
        entries.par_sort_by_key(|e| e.address);
        Some(Self { entries })
    }

    /// Build a symbol index from any binary format. Convenience wrapper around new/from_elf.
    pub fn from_binary(binary: &BinaryRef) -> Option<Self> {
        match binary {
            BinaryRef::MachO(macho) => Self::new(macho),
            BinaryRef::Elf(elf) => Self::from_elf(elf),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    // -- SymbolEntry tests --

    #[test]
    fn test_symbol_entry_demangling() {
        let entry = SymbolEntry::new(0x1000, "std::io::Read::read".to_string());
        // First access computes demangled name
        assert_eq!(entry.demangled(), "std::io::Read::read");
        // Second access returns cached value
        assert_eq!(entry.demangled(), "std::io::Read::read");
    }

    #[test]
    fn test_symbol_entry_mangled_rust_symbol() {
        // A mangled Rust symbol should be demangled
        let entry = SymbolEntry::new(
            0x2000,
            "ZN3std2io4Read4read17h1234567890abcdefE".to_string(),
        );
        let name = entry.demangled();
        // Should not contain mangled hash
        assert!(!name.contains("h1234567890abcdef"));
    }

    #[test]
    fn test_symbol_entry_debug_format() {
        let entry = SymbolEntry::new(0x1000, "main".to_string());
        let debug = format!("{:?}", entry);
        assert!(debug.contains("address: 4096")); // 0x1000
        assert!(debug.contains("main"));
    }

    // -- SymbolTable::from tests --

    #[test]
    fn test_symbol_table_from_invalid_data() {
        let result = SymbolTable::from(b"not a valid binary");
        assert!(result.is_err());
    }

    #[test]
    fn test_symbol_table_from_empty_data() {
        let result = SymbolTable::from(b"");
        assert!(result.is_err());
    }

    #[test]
    fn test_symbol_table_from_real_binary() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            let symbols = SymbolTable::from(&buffer);
            assert!(symbols.is_ok());
            let symbols = symbols.unwrap();
            assert!(symbols.macho().is_some());
        }
    }

    // -- SymbolTable method tests (using real binary) --

    #[test]
    fn test_find_symbol_containing_on_real_binary() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                // Should find rust_panic in jonesy binary
                let result = symbols.find_symbol_containing("rust_panic$");
                assert!(result.is_ok());
                if let Ok(Some((_mangled, demangled))) = result {
                    assert!(demangled.contains("rust_panic"));
                }
            }
        }
    }

    #[test]
    fn test_find_symbol_containing_no_match() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                let result = symbols.find_symbol_containing("zzz_nonexistent_symbol_zzz$");
                assert!(result.is_ok());
                assert!(result.unwrap().is_none());
            }
        }
    }

    #[test]
    fn test_find_symbol_containing_invalid_regex() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                let result = symbols.find_symbol_containing("[invalid regex");
                assert!(result.is_err());
            }
        }
    }

    #[test]
    fn test_find_all_symbols_matching() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                let result =
                    symbols.find_all_symbols_matching(&["rust_panic$", "zzz_no_match_zzz"]);
                assert!(result.is_ok());
                let matches = result.unwrap();
                // Should find at least rust_panic
                assert!(
                    matches.iter().any(|(_, d)| d.contains("rust_panic")),
                    "Should find rust_panic"
                );
            }
        }
    }

    #[test]
    fn test_find_symbol_address() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                // Find a known symbol first, then look up its address
                if let Ok(Some((mangled, _))) = symbols.find_symbol_containing("rust_panic$") {
                    let addr = symbols.find_symbol_address(&mangled);
                    assert!(addr.is_some(), "Should find address for rust_panic");
                    assert!(addr.unwrap() > 0);
                }
            }
        }
    }

    #[test]
    fn test_find_symbol_address_nonexistent() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                assert!(symbols.find_symbol_address("_zzz_nonexistent").is_none());
            }
        }
    }

    // -- SymbolIndex tests --

    #[test]
    fn test_symbol_index_from_real_binary() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                if let Some(macho) = symbols.macho() {
                    let index = SymbolIndex::new(macho);
                    assert!(index.is_some(), "Should build symbol index from binary");
                }
            }
        }
    }

    #[test]
    fn test_symbol_index_find_containing() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                if let Some(macho) = symbols.macho() {
                    if let Some(index) = SymbolIndex::new(macho) {
                        // Address 0 should return None (before all symbols)
                        assert!(index.find_containing(0).is_none());

                        // A very high address should find some symbol
                        if let Some((addr, name)) = index.find_containing(0x100000000) {
                            assert!(addr > 0);
                            assert!(!name.is_empty());
                        }
                    }
                }
            }
        }
    }
}
