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

/// Extract a literal substring hint from a regex pattern for pre-filtering.
///
/// Strips common regex anchors (`^`, `$`, `.*`) and checks that the remainder
/// contains no regex metacharacters. For patterns containing `::`, extracts the
/// last path component as the most selective hint (e.g. `core::panicking::` yields
/// `panicking`).
///
/// Returns `Some(hint)` if a usable literal was extracted, `None` otherwise.
fn extract_literal_hint(pattern: &str) -> Option<&str> {
    let stripped = pattern
        .trim_start_matches('^')
        .trim_end_matches('$')
        .trim_end_matches(".*");

    // For patterns with `::`, use the last non-empty component (most selective)
    let hint = if stripped.contains("::") {
        stripped
            .split("::")
            .filter(|s| !s.is_empty())
            .last()
            .unwrap_or(stripped)
    } else {
        stripped
    };

    if !hint.is_empty() && !hint.contains(['(', ')', '[', ']', '|', '?', '+', '*', '.', '\\']) {
        Some(hint)
    } else {
        None
    }
}

/// Check whether a symbol name could match based on a literal hint.
/// Returns `true` if there is no hint (no pre-filtering possible) or if the
/// symbol name contains the hint substring.
fn matches_hint(hint: &Option<&str>, name: &str) -> bool {
    match hint {
        Some(h) => name.contains(h),
        None => true,
    }
}

/// Check whether a symbol name could match any of a set of literal hints.
/// Returns `true` if any hint is `None` (pattern without usable literal) or
/// if the symbol name contains at least one hint substring.
fn matches_any_hint(hints: &[Option<&str>], name: &str) -> bool {
    hints
        .iter()
        .any(|h| h.is_none_or(|literal| name.contains(literal)))
}

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
    ///
    /// As an optimisation, extracts a literal substring from the pattern and
    /// skips demangling for symbols whose mangled name doesn't contain it.
    /// This avoids demangling ~50K symbols when searching for specific names
    /// like `rust_panic$` or `rust_begin_unwind$`.
    pub fn find_symbol_containing(
        &self,
        pattern: &str,
    ) -> Result<Option<(String, String)>, regex::Error> {
        use rayon::prelude::*;

        let regex = Regex::new(pattern)?;
        let hint = extract_literal_hint(pattern);

        // Phase 1: collect candidate symbols sequentially (goblin iterator not Send)
        let candidates: Vec<&str> = match self {
            SymbolTable::MachO(goblin::mach::Mach::Binary(macho)) => match macho.symbols.as_ref() {
                Some(symbols) => symbols
                    .iter()
                    .flatten()
                    .filter(|(name, _)| matches_hint(&hint, name))
                    .map(|(name, _)| name)
                    .collect(),
                None => return Ok(None),
            },
            SymbolTable::Elf(elf) => elf
                .syms
                .iter()
                .filter(|sym| sym.st_value != 0 && sym.st_shndx != 0)
                .filter_map(|sym| {
                    let name = elf.strtab.get_at(sym.st_name)?;
                    if name.is_empty() || !matches_hint(&hint, name) {
                        return None;
                    }
                    Some(name)
                })
                .collect(),
            _ => return Ok(None),
        };

        // Phase 2: demangle and match in parallel, return first match
        let result = candidates.par_iter().find_map_any(|sym_name| {
            let stripped = sym_name.strip_prefix('_').unwrap_or(sym_name);
            let demangled = format!("{:#}", demangle(stripped));
            if regex.is_match(&demangled) {
                Some((sym_name.to_string(), demangled))
            } else {
                None
            }
        });

        Ok(result)
    }

    /// Returns all symbols whose demangled names match any of the given patterns.
    /// Uses parallel demangling and matching via rayon.
    pub fn find_all_symbols_matching(
        &self,
        patterns: &[&str],
    ) -> Result<Vec<(String, String)>, regex::Error> {
        use rayon::prelude::*;

        let regexes: Vec<Regex> = patterns
            .iter()
            .map(|p| Regex::new(p))
            .collect::<Result<Vec<_>, _>>()?;

        // Phase 1: collect raw symbol names sequentially
        let raw_symbols: Vec<&str> = match self {
            SymbolTable::MachO(goblin::mach::Mach::Binary(macho)) => match macho.symbols.as_ref() {
                Some(symbols) => symbols.iter().flatten().map(|(name, _)| name).collect(),
                None => return Ok(Vec::new()),
            },
            SymbolTable::Elf(elf) => elf
                .syms
                .iter()
                .filter(|sym| sym.st_value != 0 && sym.st_shndx != 0)
                .filter_map(|sym| {
                    let name = elf.strtab.get_at(sym.st_name)?;
                    if name.is_empty() {
                        return None;
                    }
                    Some(name)
                })
                .collect(),
            _ => return Ok(Vec::new()),
        };

        // Phase 2: demangle and match in parallel
        let results = raw_symbols
            .par_iter()
            .filter_map(|sym_name| {
                let stripped = sym_name.strip_prefix('_').unwrap_or(sym_name);
                let demangled = format!("{:#}", demangle(stripped));
                for regex in &regexes {
                    if regex.is_match(&demangled) {
                        return Some((sym_name.to_string(), demangled));
                    }
                }
                None
            })
            .collect();

        Ok(results)
    }

    /// Returns all defined symbols with addresses whose demangled names match any pattern.
    /// Unlike `find_all_symbols_matching` + `find_symbol_address`, this returns all
    /// instances including multiple monomorphisations at different addresses.
    ///
    /// Uses a two-phase approach: first collects raw symbol data sequentially
    /// (goblin's iterator is not `Send`), then demangles and matches in parallel
    /// via rayon. Uses literal substring pre-filtering to skip symbols whose
    /// mangled names cannot possibly match.
    pub fn find_all_symbols_with_addresses(
        &self,
        patterns: &[&str],
    ) -> Result<Vec<(String, String, u64)>, regex::Error> {
        use rayon::prelude::*;

        let regexes: Vec<Regex> = patterns
            .iter()
            .map(|p| Regex::new(p))
            .collect::<Result<Vec<_>, _>>()?;

        let hints: Vec<Option<&str>> = patterns.iter().map(|p| extract_literal_hint(p)).collect();

        // Phase 1: collect raw symbol data sequentially (goblin iterator not Send).
        // Apply cheap pre-filters (undefined, zero address, hint mismatch) to
        // minimise the data passed to the parallel phase.
        let raw_symbols: Vec<(&str, u64)> = match self {
            SymbolTable::MachO(goblin::mach::Mach::Binary(macho)) => match macho.symbols.as_ref() {
                Some(symbols) => symbols
                    .iter()
                    .flatten()
                    .filter(|(_, nlist)| !nlist.is_undefined() && nlist.n_value != 0)
                    .filter(|(name, _)| matches_any_hint(&hints, name))
                    .map(|(name, nlist)| (name, nlist.n_value))
                    .collect(),
                None => Vec::new(),
            },
            SymbolTable::Elf(elf) => elf
                .syms
                .iter()
                .filter(|sym| sym.st_value != 0 && sym.st_shndx != 0)
                .filter_map(|sym| {
                    let name = elf.strtab.get_at(sym.st_name)?;
                    if name.is_empty() || !matches_any_hint(&hints, name) {
                        return None;
                    }
                    Some((name, sym.st_value))
                })
                .collect(),
            _ => Vec::new(),
        };

        // Phase 2: demangle and regex-match in parallel (the expensive part).
        let results = raw_symbols
            .par_iter()
            .filter_map(|(sym_name, addr)| {
                let stripped = sym_name.strip_prefix('_').unwrap_or(sym_name);
                let demangled = format!("{:#}", demangle(stripped));
                for regex in &regexes {
                    if regex.is_match(&demangled) {
                        return Some((sym_name.to_string(), demangled, *addr));
                    }
                }
                None
            })
            .collect();

        Ok(results)
    }

    /// Returns the address of the first defined symbol found whose name matches `name` exactly.
    pub fn find_symbol_address(&self, name: &str) -> Option<u64> {
        match self {
            SymbolTable::MachO(goblin::mach::Mach::Binary(macho)) => {
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
            SymbolTable::Elf(elf) => {
                for sym in elf.syms.iter() {
                    if sym.st_value == 0 || sym.st_shndx == 0 {
                        continue;
                    }
                    if let Some(sym_name) = elf.strtab.get_at(sym.st_name) {
                        if sym_name == name {
                            return Some(sym.st_value);
                        }
                    }
                }
                None
            }
            _ => None,
        }
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
    // -- Pre-filter hint extraction and matching tests --

    #[test]
    fn test_extract_literal_hint_simple() {
        assert_eq!(extract_literal_hint("rust_panic$"), Some("rust_panic"));
    }

    #[test]
    fn test_extract_literal_hint_with_anchors() {
        assert_eq!(extract_literal_hint("^rust_panic$"), Some("rust_panic"));
    }

    #[test]
    fn test_extract_literal_hint_with_path() {
        // Should extract last path component
        assert_eq!(extract_literal_hint("core::panicking::"), Some("panicking"));
    }

    #[test]
    fn test_extract_literal_hint_with_wildcard_suffix() {
        assert_eq!(extract_literal_hint("panic.*"), Some("panic"));
    }

    #[test]
    fn test_extract_literal_hint_metachar_returns_none() {
        assert_eq!(extract_literal_hint("rust.panic$"), None);
        assert_eq!(extract_literal_hint("(foo|bar)"), None);
        assert_eq!(extract_literal_hint("[abc]"), None);
        assert_eq!(extract_literal_hint("foo?"), None);
    }

    #[test]
    fn test_extract_literal_hint_empty_returns_none() {
        assert_eq!(extract_literal_hint("$"), None);
        assert_eq!(extract_literal_hint("^$"), None);
    }

    #[test]
    fn test_matches_hint_with_some() {
        let hint = Some("panic");
        assert!(matches_hint(&hint, "_ZN4core9panicking9panic_fmt"));
        assert!(!matches_hint(&hint, "_ZN3std3env4args"));
    }

    #[test]
    fn test_matches_hint_with_none() {
        let hint = None;
        // No hint means no pre-filtering — always matches
        assert!(matches_hint(&hint, "anything"));
        assert!(matches_hint(&hint, ""));
    }

    #[test]
    fn test_matches_any_hint_mixed() {
        let hints = vec![Some("panicking"), Some("abort")];
        assert!(matches_any_hint(&hints, "_ZN4core9panicking"));
        assert!(matches_any_hint(&hints, "std_process_abort"));
        assert!(!matches_any_hint(&hints, "_ZN3std3env4args"));
    }

    #[test]
    fn test_matches_any_hint_with_none_always_matches() {
        // If any hint is None, every symbol passes (can't pre-filter)
        let hints = vec![Some("panicking"), None];
        assert!(matches_any_hint(&hints, "anything_at_all"));
    }

    // -- Pre-filter integration tests on real binary --

    #[test]
    fn test_find_symbol_containing_prefilter_skips_non_matching() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                let result = symbols.find_symbol_containing("rust_begin_unwind$");
                assert!(result.is_ok());
                if let Ok(Some((_mangled, demangled))) = result {
                    assert!(
                        demangled.contains("rust_begin_unwind"),
                        "Should find rust_begin_unwind via pre-filtered search"
                    );
                }
            }
        }
    }

    #[test]
    fn test_find_symbol_containing_regex_metachar_disables_hint() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                let result = symbols.find_symbol_containing("rust.panic$");
                assert!(result.is_ok());
                if let Ok(Some((_mangled, demangled))) = result {
                    assert!(demangled.contains("rust_panic"));
                }
            }
        }
    }

    #[test]
    fn test_find_all_symbols_with_addresses_prefilter() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                let result = symbols.find_all_symbols_with_addresses(&["core::panicking::"]);
                assert!(result.is_ok());
                let matches = result.unwrap();
                assert!(
                    !matches.is_empty(),
                    "Should find core::panicking:: symbols via pre-filtered search"
                );
                for (_mangled, demangled, addr) in &matches {
                    assert!(
                        demangled.contains("core::panicking::"),
                        "Matched symbol '{}' should contain 'core::panicking::'",
                        demangled
                    );
                    assert!(*addr > 0);
                }
            }
        }
    }

    #[test]
    fn test_find_all_symbols_with_addresses_no_match_with_hint() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                let result = symbols.find_all_symbols_with_addresses(&["zzz_nonexistent_module::"]);
                assert!(result.is_ok());
                assert!(result.unwrap().is_empty());
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

    // -- Parallel symbol search tests --

    /// Test that find_symbol_containing finds known panic symbols
    /// using the parallel two-phase approach.
    #[test]
    fn test_find_symbol_containing_parallel() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                // rust_panic should exist in a debug binary
                let result = symbols.find_symbol_containing("rust_panic$");
                assert!(result.is_ok());
                if let Ok(Some((mangled, demangled))) = result {
                    assert!(!mangled.is_empty());
                    assert!(
                        demangled.contains("rust_panic"),
                        "demangled should contain rust_panic, got: {demangled}"
                    );
                }
            }
        }
    }

    /// Test that find_symbol_containing returns None for non-existent patterns.
    #[test]
    fn test_find_symbol_containing_not_found() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                let result =
                    symbols.find_symbol_containing("this_symbol_definitely_does_not_exist_xyz$");
                assert!(matches!(result, Ok(None)));
            }
        }
    }

    /// Test that find_all_symbols_with_addresses finds core::panicking symbols
    /// using the parallel two-phase approach.
    #[test]
    fn test_find_all_symbols_with_addresses_parallel() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                let result = symbols.find_all_symbols_with_addresses(&["core::panicking::"]);
                assert!(result.is_ok());
                let matches = result.unwrap();
                // A debug Rust binary should have core::panicking functions
                assert!(
                    !matches.is_empty(),
                    "should find at least one core::panicking symbol"
                );
                for (mangled, demangled, addr) in &matches {
                    assert!(!mangled.is_empty());
                    assert!(
                        demangled.contains("core::panicking"),
                        "demangled should contain core::panicking, got: {demangled}"
                    );
                    assert!(*addr > 0, "address should be non-zero");
                }
            }
        }
    }

    /// Test that find_all_symbols_matching finds symbols using parallel demangling.
    #[test]
    fn test_find_all_symbols_matching_parallel() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                let result = symbols.find_all_symbols_matching(&["core::panicking::"]);
                assert!(result.is_ok());
                let matches = result.unwrap();
                assert!(
                    !matches.is_empty(),
                    "should find at least one core::panicking symbol"
                );
                for (_, demangled) in &matches {
                    assert!(demangled.contains("core::panicking"));
                }
            }
        }
    }

    /// Test that find_all_symbols_with_addresses works with multiple patterns.
    #[test]
    fn test_find_all_symbols_multiple_patterns() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                let result = symbols
                    .find_all_symbols_with_addresses(&["core::panicking::", "std::process::abort"]);
                assert!(result.is_ok());
                let matches = result.unwrap();
                // Should find symbols from both patterns
                let has_panicking = matches
                    .iter()
                    .any(|(_, d, _)| d.contains("core::panicking"));
                assert!(
                    has_panicking,
                    "should find at least one core::panicking symbol"
                );
            }
        }
    }

    /// Test that empty pattern list returns empty results.
    #[test]
    fn test_find_all_symbols_empty_patterns() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(symbols) = SymbolTable::from(&buffer) {
                let result = symbols.find_all_symbols_with_addresses(&[]);
                assert!(result.is_ok());
                assert!(result.unwrap().is_empty());
            }
        }
    }
}
