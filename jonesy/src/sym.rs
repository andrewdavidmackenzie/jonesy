#![allow(unused_variables)] // TODO Just for now
#![allow(dead_code)] // TODO Just for now

use capstone::arch::BuildsCapstone;
use capstone::{Capstone, Insn, arch};
use dashmap::DashMap;
/// Here's how to use gimli with a MachO binary to get function information and then find call sites
/// Note that DWARF doesn't directly encode "function A calls
/// function B" - it provides accurate function boundaries and source locations, which you combine with disassembly.
use gimli::{
    AttributeValue, ColumnType, DebuggingInformationEntry, Dwarf, EndianSlice, Reader,
    RunTimeEndian, SectionId, Unit,
};
use goblin::Object;
use goblin::archive::Archive;
use goblin::container::{Container, Ctx, Endian};
use goblin::mach::segment::SectionData;
use goblin::mach::segment::{Section, Segment};
use goblin::mach::symbols::N_OSO;
use goblin::mach::{Mach, MachO};
use ouroboros::self_referencing;
use rayon::prelude::*;
use regex::Regex;
use rustc_demangle::demangle;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

type DwarfReader<'a> = EndianSlice<'a, RunTimeEndian>;

/// Source location tuple: (file, line, column)
type SourceLocation = (Option<String>, Option<u32>, Option<u32>);

/// Check if a file path matches a crate source pattern.
/// Supports multi-pattern format with "|" separator for workspace mode.
/// Check if a file path matches the crate source pattern.
/// For single-crate projects with pattern "src/", uses the valid_files set
/// to exclude dependency paths that happen to start with src/.
pub fn matches_crate_pattern(file_path: &str, crate_pattern: &str) -> bool {
    matches_crate_pattern_validated(file_path, crate_pattern, None)
}

/// Check if a file path matches the crate source pattern, with optional validation.
/// When valid_files is provided and pattern is generic ("src/"), validates against actual project files.
pub fn matches_crate_pattern_validated(
    file_path: &str,
    crate_pattern: &str,
    valid_files: Option<&ValidSourceFiles>,
) -> bool {
    // Check if path matches the pattern first
    let matches = crate_pattern
        .split('|')
        .any(|pattern| !pattern.is_empty() && file_path.contains(pattern));

    if !matches {
        return false;
    }

    // For single-crate projects (pattern is just "src/"), validate against actual project files
    // This prevents false positives from dependencies with relative src/ paths in DWARF
    // The allowlist check comes BEFORE dependency heuristics to ensure legitimate project files
    // (like "library/src/..." or "src/__generated.rs") are not incorrectly excluded
    if let Some(valid) = valid_files {
        if valid.needs_validation(crate_pattern) {
            return valid.contains(file_path);
        }
    }

    // Only apply dependency heuristics when we don't have an allowlist for validation
    // This is for workspace patterns like "flowc/src/" where we rely on pattern specificity
    if is_dependency_path(file_path) {
        return false;
    }

    true
}

/// A set of valid source files for a project.
/// Used to filter out dependency paths that have relative src/ paths.
#[derive(Debug, Default)]
pub struct ValidSourceFiles {
    /// Set of valid file paths (relative to project root, e.g., "src/main.rs")
    files: std::collections::HashSet<String>,
    /// Canonical project root path for absolute path matching
    project_root: Option<std::path::PathBuf>,
}

impl ValidSourceFiles {
    /// Build a set of valid source files by scanning the entire project directory.
    /// This handles all cases including #[path] directives pointing outside src/.
    /// Excludes target/, .git/, and other non-source directories.
    pub fn from_project_root(project_root: &std::path::Path) -> Self {
        let mut files = std::collections::HashSet::new();

        // Scan the entire project for .rs files, excluding build artifacts
        Self::scan_project_directory(project_root, project_root, &mut files);

        // Store canonical project root for absolute path matching
        let canonical_root = std::fs::canonicalize(project_root).ok();

        Self {
            files,
            project_root: canonical_root,
        }
    }

    /// Recursively scan directory for .rs files, excluding non-source directories
    fn scan_project_directory(
        project_root: &std::path::Path,
        dir: &std::path::Path,
        files: &mut std::collections::HashSet<String>,
    ) {
        // Directories to exclude (build artifacts, version control, examples, tests, benches)
        const EXCLUDED_DIRS: &[&str] = &[
            "target",
            ".git",
            ".hg",
            ".svn",
            "node_modules",
            ".cargo",
            ".rustup",
            "examples",
            "tests",
            "benches",
        ];

        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if path.is_file() && name_str.ends_with(".rs") && name_str != "build.rs" {
                // Store path relative to project root
                if let Ok(rel_path) = path.strip_prefix(project_root) {
                    files.insert(rel_path.to_string_lossy().to_string());
                }
            } else if path.is_dir()
                && !name_str.starts_with('.')
                && !EXCLUDED_DIRS.contains(&name_str.as_ref())
            {
                Self::scan_project_directory(project_root, &path, files);
            }
        }
    }

    /// Check if this file set should be used for validation.
    /// Only validates when the pattern is generic (just "src/").
    fn needs_validation(&self, pattern: &str) -> bool {
        // Validate for single-crate projects where pattern is generic
        // Workspace patterns like "flowc/src/" are specific enough
        pattern == "src/" && !self.files.is_empty()
    }

    /// Check if a file path is in the valid set.
    fn contains(&self, file_path: &str) -> bool {
        // Direct match (relative paths)
        if self.files.contains(file_path) {
            return true;
        }

        // Also check with leading "./" stripped
        if let Some(stripped) = file_path.strip_prefix("./") {
            if self.files.contains(stripped) {
                return true;
            }
        }

        // For absolute paths, check if they resolve to a file in our project
        // Use canonical path comparison to avoid false positives from suffix matching
        if let Some(project_root) = &self.project_root {
            let file_path_buf = std::path::Path::new(file_path);
            if file_path_buf.is_absolute() {
                // Try to canonicalize and check if it starts with project root
                if let Ok(canonical_file) = std::fs::canonicalize(file_path_buf) {
                    if let Ok(relative) = canonical_file.strip_prefix(project_root) {
                        // Check if this relative path is in our set
                        let rel_str = relative.to_string_lossy();
                        return self.files.contains(rel_str.as_ref());
                    }
                }
            }
        }

        false
    }
}

/// Check if a file path is from a dependency or stdlib, not user code.
/// These paths should never be reported as user crate panic points.
fn is_dependency_path(file_path: &str) -> bool {
    // Cargo registry dependencies (absolute paths)
    if file_path.contains(".cargo/registry/") || file_path.contains(".cargo\\registry\\") {
        return true;
    }

    // Rust stdlib and compiler-generated paths
    if file_path.contains("/rustc/")
        || file_path.starts_with("/rust/deps/")
        || file_path.starts_with("library/")
    {
        return true;
    }

    // Internal/generated paths from dependencies (common patterns)
    // These use relative src/ paths that would match "src/" pattern for single-crate projects
    // The __ prefixes are used by macro-generated code in crates like objc2
    // Use segment-boundary checks to avoid false positives on user dirs like /Users/__myuser__/
    if file_path.contains("/__/") || file_path.starts_with("__") || file_path.starts_with("src/__")
    {
        return true;
    }

    false
}

#[allow(clippy::large_enum_variant)]
pub enum SymbolTable<'a> {
    MachO(Mach<'a>),
    Archive(Archive<'a>),
}

/// A line entry for crate source code, used for fast lookups
#[derive(Debug, Clone)]
pub struct CrateLineEntry {
    pub address: u64,
    pub line: u32,
    pub column: Option<u32>,
}

/// Pre-built line table containing only crate source entries, sorted by address.
/// Used for fast binary search in get_crate_line_at_address.
#[derive(Debug)]
pub struct CrateLineTable {
    entries: Vec<CrateLineEntry>,
}

impl CrateLineTable {
    /// Build a line table with only entries from crate source files.
    pub fn build<R: Reader>(dwarf: &Dwarf<R>, crate_src_path: &str) -> Result<Self, gimli::Error> {
        let mut entries = Vec::new();

        let mut units = dwarf.units();
        while let Some(header) = units.next()? {
            let unit = dwarf.unit(header)?;

            if let Some(program) = &unit.line_program {
                let mut rows = program.clone().rows();

                while let Some((header, row)) = rows.next_row()? {
                    if let Some(file_entry) = row.file(header) {
                        let file_name = dwarf
                            .attr_string(&unit, file_entry.path_name())?
                            .to_string_lossy()?
                            .into_owned();

                        let full_path = if let Some(dir) = file_entry.directory(header) {
                            let dir_str = dwarf
                                .attr_string(&unit, dir)?
                                .to_string_lossy()?
                                .into_owned();
                            if dir_str.is_empty() {
                                file_name
                            } else {
                                format!("{}/{}", dir_str, file_name)
                            }
                        } else {
                            file_name
                        };

                        // Only include entries from crate source
                        if matches_crate_pattern(&full_path, crate_src_path)
                            && let Some(line) = row.line()
                        {
                            let column = match row.column() {
                                ColumnType::LeftEdge => None,
                                ColumnType::Column(c) => Some(c.get() as u32),
                            };
                            entries.push(CrateLineEntry {
                                address: row.address(),
                                line: line.get() as u32,
                                column,
                            });
                        }
                    }
                }
            }
        }

        // Sort by address for binary search
        entries.sort_by_key(|e| e.address);

        Ok(Self { entries })
    }

    /// Find the crate line and column at a specific address within a function.
    /// Returns the line and column of the last entry in [func_start, call_site_addr].
    pub fn get_line(&self, func_start: u64, call_site_addr: u64) -> (Option<u32>, Option<u32>) {
        // Find entries in range [func_start, call_site_addr]
        let start_idx = self.entries.partition_point(|e| e.address < func_start);
        let end_idx = self
            .entries
            .partition_point(|e| e.address <= call_site_addr);

        // Return the last entry in range (highest address)
        if end_idx > start_idx {
            let entry = &self.entries[end_idx - 1];
            (Some(entry.line), entry.column)
        } else {
            (None, None)
        }
    }

    /// Get the number of entries
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the table has no entries
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A line entry for full source location lookups.
/// Uses file_id for string interning to reduce memory pressure.
#[derive(Debug, Clone)]
pub struct FullLineEntry {
    pub address: u64,
    /// Index into FullLineTable's file_pool for the file path
    file_id: u32,
    pub line: u32,
    pub column: Option<u32>,
}

/// Pre-built line table containing ALL line entries, sorted by address.
/// Uses string interning for file paths to reduce memory pressure.
/// Instead of storing 1M+ duplicate path strings, stores file IDs that
/// index into a deduplicated file pool.
#[derive(Debug)]
pub struct FullLineTable {
    entries: Vec<FullLineEntry>,
    /// Deduplicated file path pool
    file_pool: Vec<String>,
}

impl FullLineTable {
    /// Build a complete line table from DWARF debug info.
    /// Uses string interning to deduplicate file paths and reduce memory usage.
    pub fn build<R: Reader>(dwarf: &Dwarf<R>) -> Result<Self, gimli::Error> {
        let mut entries = Vec::new();
        let mut file_pool = Vec::new();
        let mut file_to_id: ahash::AHashMap<String, u32> = ahash::AHashMap::new();

        let mut units = dwarf.units();
        while let Some(header) = units.next()? {
            let unit = dwarf.unit(header)?;

            if let Some(program) = &unit.line_program {
                let mut rows = program.clone().rows();

                while let Some((header, row)) = rows.next_row()? {
                    if let Some(file_entry) = row.file(header) {
                        let file_name = dwarf
                            .attr_string(&unit, file_entry.path_name())?
                            .to_string_lossy()?
                            .into_owned();

                        let full_path = if let Some(dir) = file_entry.directory(header) {
                            let dir_str = dwarf
                                .attr_string(&unit, dir)?
                                .to_string_lossy()?
                                .into_owned();
                            if dir_str.is_empty() {
                                file_name
                            } else {
                                format!("{}/{}", dir_str, file_name)
                            }
                        } else {
                            file_name
                        };

                        // Intern the file path to reduce memory usage
                        // Use two-step get/insert to avoid cloning on cache hits
                        let file_id = if let Some(&id) = file_to_id.get(&full_path) {
                            id
                        } else {
                            let id = file_pool.len() as u32;
                            file_pool.push(full_path.clone());
                            file_to_id.insert(full_path, id);
                            id
                        };

                        // Include all entries, even without line numbers (use 0 like original)
                        // to match the original get_source_location behavior
                        let line = row.line().map(|l| l.get() as u32).unwrap_or(0);
                        let column = match row.column() {
                            ColumnType::LeftEdge => None,
                            ColumnType::Column(c) => Some(c.get() as u32),
                        };
                        entries.push(FullLineEntry {
                            address: row.address(),
                            file_id,
                            line,
                            column,
                        });
                    }
                }
            }
        }

        // Sort by address for binary search (parallel sort for large datasets)
        use rayon::prelude::*;
        entries.par_sort_by_key(|e| e.address);

        Ok(Self { entries, file_pool })
    }

    /// Get source location for an address using binary search.
    /// Returns the entry whose address is <= the query address.
    /// For entries with the same address, returns the first one (from earliest unit,
    /// matching original get_source_location behavior).
    pub fn get_source_location(&self, addr: u64) -> SourceLocation {
        // Find the first entry with address > addr
        let idx = self.entries.partition_point(|e| e.address <= addr);

        if idx > 0 {
            // Found an entry with address <= addr. Now find the FIRST entry at this address.
            // Use binary search to keep this O(log n) instead of linear scan.
            let target_addr = self.entries[idx - 1].address;
            let first_idx = self.entries[..idx].partition_point(|e| e.address < target_addr);
            let entry = &self.entries[first_idx];
            // Look up file path from the interned pool
            let file = self.file_pool.get(entry.file_id as usize).cloned();
            (file, Some(entry.line), entry.column)
        } else {
            (None, None, None)
        }
    }

    /// Get the number of entries
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the table has no entries
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Build both CrateLineTable and FullLineTable in a single pass over DWARF.
    /// This is more efficient than building each separately since we only iterate
    /// the line program once.
    pub fn build_both<R: Reader>(
        dwarf: &Dwarf<R>,
        crate_src_path: &str,
    ) -> Result<(CrateLineTable, FullLineTable), gimli::Error> {
        let mut crate_entries = Vec::new();
        let mut full_entries = Vec::new();
        let mut file_pool = Vec::new();
        let mut file_to_id: ahash::AHashMap<String, u32> = ahash::AHashMap::new();

        let mut units = dwarf.units();
        while let Some(header) = units.next()? {
            let unit = dwarf.unit(header)?;

            if let Some(program) = &unit.line_program {
                let mut rows = program.clone().rows();

                while let Some((header, row)) = rows.next_row()? {
                    if let Some(file_entry) = row.file(header) {
                        let file_name = dwarf
                            .attr_string(&unit, file_entry.path_name())?
                            .to_string_lossy()?
                            .into_owned();

                        let full_path = if let Some(dir) = file_entry.directory(header) {
                            let dir_str = dwarf
                                .attr_string(&unit, dir)?
                                .to_string_lossy()?
                                .into_owned();
                            if dir_str.is_empty() {
                                file_name
                            } else {
                                format!("{}/{}", dir_str, file_name)
                            }
                        } else {
                            file_name
                        };

                        // Check crate match before interning (needs &full_path)
                        let is_crate_match = matches_crate_pattern(&full_path, crate_src_path);

                        // Intern the file path (avoid double clone on cache miss)
                        let file_id = if let Some(&id) = file_to_id.get(&full_path) {
                            id
                        } else {
                            let id = file_pool.len() as u32;
                            file_pool.push(full_path.clone());
                            file_to_id.insert(full_path, id);
                            id
                        };

                        // Add to full line table (all entries)
                        let line = row.line().map(|l| l.get() as u32).unwrap_or(0);
                        let column = match row.column() {
                            ColumnType::LeftEdge => None,
                            ColumnType::Column(c) => Some(c.get() as u32),
                        };
                        full_entries.push(FullLineEntry {
                            address: row.address(),
                            file_id,
                            line,
                            column,
                        });

                        // Add to crate line table if matches pattern and has line
                        if is_crate_match && line > 0 {
                            crate_entries.push(CrateLineEntry {
                                address: row.address(),
                                line,
                                column,
                            });
                        }
                    }
                }
            }
        }

        // Sort both by address in parallel
        use rayon::prelude::*;
        rayon::join(
            || crate_entries.par_sort_by_key(|e| e.address),
            || full_entries.par_sort_by_key(|e| e.address),
        );

        Ok((
            CrateLineTable {
                entries: crate_entries,
            },
            FullLineTable {
                entries: full_entries,
                file_pool,
            },
        ))
    }
}

/// Function info extracted from DWARF of the calling function
#[derive(Debug, Clone, Default)]
pub struct FunctionInfo {
    /// Demangled name of the calling function
    pub name: String,
    /// Start address of the calling function
    pub start_address: u64,
    /// End address of the calling function
    pub end_address: u64,
    /// Source location of the calling function
    pub file: Option<String>,
    /// Line number of the calling function
    pub line: Option<u32>,
}

/// Index for O(log n) function lookup by address.
/// Functions are sorted by start_address for binary search.
/// Note: This assumes function ranges are non-overlapping. Overlapping ranges
/// from DWARF debug info may cause incorrect lookups.
#[derive(Debug)]
pub struct FunctionIndex {
    /// Functions sorted by start_address (non-inlined subprograms)
    functions: Vec<FunctionInfo>,
    /// Inlined functions sorted by start_address.
    /// These have smaller address ranges within their containing functions.
    inlined: Vec<FunctionInfo>,
}

impl FunctionIndex {
    /// Build a function index from a list of functions.
    /// Sorts the functions by start_address for binary search.
    pub fn new(mut functions: Vec<FunctionInfo>) -> Self {
        functions.sort_by_key(|f| f.start_address);
        // Note: DWARF may have overlapping function ranges (e.g., from inlining).
        // We don't assert non-overlapping here because it's common in real binaries.
        // The binary search will find one valid function, which is sufficient.
        Self {
            functions,
            inlined: Vec::new(),
        }
    }

    /// Build a function index with separate inlined function tracking.
    pub fn new_with_inlined(
        mut functions: Vec<FunctionInfo>,
        mut inlined: Vec<FunctionInfo>,
    ) -> Self {
        functions.sort_by_key(|f| f.start_address);
        inlined.sort_by_key(|f| f.start_address);
        Self { functions, inlined }
    }

    /// Find the function containing the given address using binary search.
    /// Returns a reference to the function if found.
    /// Note: This returns the containing function, not inlined functions.
    /// Use `find_function_name` to get the most specific function name.
    pub fn find_containing(&self, addr: u64) -> Option<&FunctionInfo> {
        if self.functions.is_empty() {
            return None;
        }

        // Binary search for the largest start_address <= addr
        let idx = match self
            .functions
            .binary_search_by_key(&addr, |f| f.start_address)
        {
            Ok(i) => i,            // Exact match on start_address
            Err(0) => return None, // addr is before first function
            Err(i) => i - 1,       // Use previous function
        };

        let func = &self.functions[idx];
        // Check if addr is within this function's range
        if addr >= func.start_address && addr < func.end_address {
            Some(func)
        } else {
            None
        }
    }

    /// Find the most specific function name for an address.
    /// This checks inlined functions first (more specific), then falls back
    /// to the containing function. Use this when displaying function names.
    pub fn find_function_name(&self, addr: u64) -> Option<&str> {
        // First check inlined functions (more specific)
        if let Some(inlined) = self.find_in_inlined(addr) {
            return Some(&inlined.name);
        }
        // Fall back to containing function
        self.find_containing(addr).map(|f| f.name.as_str())
    }

    /// Find an inlined function containing the given address.
    fn find_in_inlined(&self, addr: u64) -> Option<&FunctionInfo> {
        if self.inlined.is_empty() {
            return None;
        }

        // Binary search for the largest start_address <= addr
        let idx = match self
            .inlined
            .binary_search_by_key(&addr, |f| f.start_address)
        {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };

        // Check all entries that could contain addr - we want the smallest range
        // (deepest inline frame). Since list is sorted by start_address only,
        // we must check both backward and forward from idx.
        let mut best: Option<&FunctionInfo> = None;
        let mut best_size: u64 = u64::MAX;

        // Check backward from idx (entries with start_address <= addr)
        // Early termination: once start_address + best_size <= addr, no function
        // starting there or earlier can be smaller than our best AND contain addr.
        for i in (0..=idx).rev() {
            let func = &self.inlined[i];

            // Early termination: if we have a match and this function starts too early
            // to possibly be smaller while still containing addr, stop searching
            if best.is_some() && func.start_address.saturating_add(best_size) <= addr {
                break;
            }

            // Skip functions that can't contain addr
            if func.end_address <= addr {
                continue;
            }

            if addr >= func.start_address {
                let size = func.end_address - func.start_address;
                if size < best_size {
                    best = Some(func);
                    best_size = size;
                }
            }
        }

        // Check forward for entries with same start_address as idx
        // (siblings that binary search may have landed before)
        let start_addr_at_idx = self.inlined[idx].start_address;
        for i in (idx + 1)..self.inlined.len() {
            let func = &self.inlined[i];
            if func.start_address != start_addr_at_idx {
                break; // Past entries with same start_address
            }
            if addr >= func.start_address && addr < func.end_address {
                let size = func.end_address - func.start_address;
                if size < best_size {
                    best = Some(func);
                    best_size = size;
                }
            }
        }

        best
    }

    /// Get a reference to the underlying functions slice.
    pub fn functions(&self) -> &[FunctionInfo] {
        &self.functions
    }
}

/// Information about a call site
/// Uses Cow<'a, str> for caller_name to support both borrowed (from SymbolIndex)
/// and owned (from DWARF) strings without allocation in the hot path.
#[derive(Debug, Clone)]
pub struct CallerInfo<'a> {
    /// Demangled name of the calling function
    pub caller_name: Cow<'a, str>,
    /// Start address of the calling function
    pub caller_start_address: u64,
    /// File of the calling function (from DWARF, if available)
    pub caller_file: Option<String>,
    /// Address of the calling instruction
    pub call_site_addr: u64,
    /// Source location of the calling instruction
    pub file: Option<String>,
    /// Line number of the calling instruction
    pub line: Option<u32>,
    /// Column number of the calling instruction
    pub column: Option<u32>,
}

/// Self-referencing struct that owns the buffer and the parsed MachO that borrows from it
#[self_referencing]
pub struct DSymInfo {
    pub debug_buffer: Vec<u8>,
    #[borrows(debug_buffer)]
    #[covariant]
    pub debug_macho: Mach<'this>,
}

/// Information about an object file from the debug map
#[derive(Debug)]
pub struct ObjectFileInfo {
    /// Path to the object file
    pub path: PathBuf,
    /// Raw bytes of the object file
    pub buffer: Vec<u8>,
    /// Symbol address translations: object file address -> final binary address
    pub addr_map: HashMap<u64, u64>,
}

/// Debug map information parsed from the binary's symbol table
pub struct DebugMapInfo {
    /// Object files referenced by the debug map
    pub object_files: Vec<ObjectFileInfo>,
}

/// Debug info source - either embedded in binary or from a separate dSYM file/bundle
pub enum DebugInfo {
    /// Debug info is embedded in the binary
    Embedded,
    /// Debug info is in a separate dSYM bundle
    DSym(Box<DSymInfo>),
    /// Debug info from object files via debug map
    DebugMap(Box<DebugMapInfo>),
    /// No debug info available
    None,
}

impl DebugInfo {
    /// Returns a reference to the debug MachO if this is a DSym variant
    pub fn debug_macho(&self) -> Option<&Mach<'_>> {
        match self {
            DebugInfo::DSym(info) => Some(info.borrow_debug_macho()),
            _ => None,
        }
    }

    /// Returns a reference to the debug buffer if this is a DSym variant
    pub fn debug_buffer(&self) -> Option<&[u8]> {
        match self {
            DebugInfo::DSym(info) => Some(info.borrow_debug_buffer()),
            _ => None,
        }
    }
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

/// Return true if `macho` has a `__DWARF` segment or a section names `__debug_*` in any segment
pub(crate) fn has_dwarf_info(macho: &MachO) -> bool {
    for segment in macho.segments.iter() {
        if let Ok(name) = segment.name()
            && name == "__DWARF"
        {
            return true;
        }

        // Also check for debug sections in any segment
        if let Ok(sections) = segment.sections() {
            for (section, _) in sections {
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
fn get_text_section<'a>(macho: &MachO, buffer: &'a [u8]) -> Option<(u64, &'a [u8])> {
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

// TODO segments() seems to create copies that it returns, see if we can get references instead
fn find_sections<'a>(macho: &'a MachO, section_name: &str) -> Vec<(Section, SectionData<'a>)> {
    macho
        .segments
        .iter()
        .filter_map(|segment| segment.sections().ok())
        .flatten()
        .filter_map(move |(section, data)| {
            if section.name().unwrap() == section_name {
                Some((section, data))
            } else {
                None
            }
        })
        .collect()
}

/// Pre-computed call graph mapping target addresses to their callers.
/// This allows O(1) lookup instead of O(n) scanning for each query.
/// Lifetime 'a comes from SymbolIndex - caller_name may borrow from it.
pub struct CallGraph<'a> {
    /// Maps target_addr -> list of CallerInfo
    edges: HashMap<u64, Vec<CallerInfo<'a>>>,
}

/// Extracted instruction data for parallel processing (avoids Insn lifetime issues)
struct InsnData {
    address: u64,
    call_target: Option<u64>,
}

/// ARM64 instruction size in bytes (fixed-size ISA)
const ARM64_INSN_SIZE: usize = 4;

/// Minimum chunk size for parallel disassembly (avoid overhead for small sections)
const MIN_CHUNK_SIZE: usize = 64 * 1024; // 64KB

/// ARM64 BL instruction mask: bits [31:26] must be 100101
const BL_MASK: u32 = 0xFC000000;
const BL_OPCODE: u32 = 0x94000000;

/// ARM64 B instruction mask: bits [31:26] must be 000101
const B_MASK: u32 = 0xFC000000;
const B_OPCODE: u32 = 0x14000000;

/// Decode ARM64 BL/B instruction target address from raw bytes.
/// BL encoding: 100101 imm26
/// B encoding: 000101 imm26
/// Target = PC + sign_extend(imm26) * 4
fn decode_branch_target(insn_bytes: u32, pc: u64) -> u64 {
    // Extract 26-bit immediate
    let imm26 = insn_bytes & 0x03FFFFFF;
    // Sign-extend to 32 bits and multiply by 4 (shift left 2)
    let offset = ((imm26 as i32) << 6) >> 4;
    // Add to PC
    (pc as i64 + offset as i64) as u64
}

/// Scan for ARM64 BL and B instructions in parallel by dividing into chunks.
/// BL = branch with link (function calls)
/// B = unconditional branch (tail calls to other functions)
/// Directly scans raw bytes for patterns - no disassembly needed.
/// This is much faster than using Capstone for full disassembly.
fn parallel_disassemble_arm64(text_data: &[u8], text_addr: u64) -> Vec<InsnData> {
    let num_threads = rayon::current_num_threads();

    // Calculate chunk size, ensuring alignment to instruction boundary
    let ideal_chunk_size = text_data.len() / num_threads;
    let chunk_size = if ideal_chunk_size < MIN_CHUNK_SIZE {
        // Data too small to benefit from parallelization
        text_data.len()
    } else {
        // Align to 4-byte instruction boundary
        (ideal_chunk_size / ARM64_INSN_SIZE) * ARM64_INSN_SIZE
    };

    if chunk_size >= text_data.len() {
        // Single chunk - use sequential scanning
        return scan_branch_instructions(text_data, text_addr);
    }

    // Create chunks with their base addresses
    let chunks: Vec<(usize, &[u8], u64)> = text_data
        .chunks(chunk_size)
        .enumerate()
        .map(|(i, chunk)| {
            let chunk_addr = text_addr + (i * chunk_size) as u64;
            (i, chunk, chunk_addr)
        })
        .collect();

    // Scan chunks in parallel for BL and B instructions
    let results: Vec<Vec<InsnData>> = chunks
        .par_iter()
        .map(|(_i, chunk, chunk_addr)| scan_branch_instructions(chunk, *chunk_addr))
        .collect();

    // Flatten results from all chunks
    results.into_iter().flatten().collect()
}

/// Scan a chunk of ARM64 code for BL and B instructions.
/// BL = branch with link (function calls)
/// B = unconditional branch (tail calls to other functions)
/// Directly checks raw bytes against opcode patterns.
fn scan_branch_instructions(data: &[u8], base_addr: u64) -> Vec<InsnData> {
    data.chunks_exact(ARM64_INSN_SIZE)
        .enumerate()
        .filter_map(|(i, bytes)| {
            let insn = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            let is_bl = (insn & BL_MASK) == BL_OPCODE;
            let is_b = (insn & B_MASK) == B_OPCODE;
            if is_bl || is_b {
                let pc = base_addr + (i * ARM64_INSN_SIZE) as u64;
                let target = decode_branch_target(insn, pc);
                Some(InsnData {
                    address: pc,
                    call_target: Some(target),
                })
            } else {
                None
            }
        })
        .collect()
}

/// Sequential disassembly using Capstone - kept for non-ARM64 platforms.
#[allow(dead_code)]
fn sequential_disassemble_arm64(text_data: &[u8], text_addr: u64) -> Vec<InsnData> {
    let Ok(cs) = Capstone::new()
        .arm64()
        .mode(arch::arm64::ArchMode::Arm)
        .build()
    else {
        eprintln!("Warning: failed to initialize Capstone disassembler");
        return Vec::new();
    };

    let Ok(instructions) = cs.disasm_all(text_data, text_addr) else {
        eprintln!("Warning: disassembly failed for text section at {text_addr:#x}");
        return Vec::new();
    };

    instructions
        .iter()
        .filter_map(|insn| {
            // Match both BL (branch with link) and B (branch) for tail call detection
            let mnemonic = insn.mnemonic();
            if mnemonic == Some("bl") || mnemonic == Some("b") {
                let operand = insn.op_str()?;
                let addr_str = operand.trim_start_matches("#0x");
                let call_target = u64::from_str_radix(addr_str, 16).ok();
                Some(InsnData {
                    address: insn.address(),
                    call_target,
                })
            } else {
                None
            }
        })
        .collect()
}

impl<'a> CallGraph<'a> {
    /// Build a call graph by scanning all instructions once (no debug info).
    /// Uses parallel disassembly and parallel processing for faster analysis.
    /// Symbol names are borrowed from the provided SymbolIndex.
    pub fn build(
        macho: &MachO,
        buffer: &[u8],
        symbol_index: Option<&'a SymbolIndex>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let Some((text_addr, text_data)) = get_text_section(macho, buffer) else {
            return Ok(Self {
                edges: HashMap::new(),
            });
        };

        // Parallel disassembly - divides text section into chunks (ARM64 only)
        #[cfg(target_arch = "aarch64")]
        let insn_data = parallel_disassemble_arm64(text_data, text_addr);

        #[cfg(not(target_arch = "aarch64"))]
        let insn_data = sequential_disassemble_arm64(text_data, text_addr);

        // Process bl instructions in parallel (the expensive part is function lookup)
        let edges: DashMap<u64, Vec<CallerInfo<'a>>> = DashMap::new();

        insn_data.par_iter().for_each(|data| {
            if let Some(call_target) = data.call_target
                && let Some((func_addr, func_name)) =
                    symbol_index.and_then(|idx| idx.find_containing(data.address))
            {
                edges.entry(call_target).or_default().push(CallerInfo {
                    caller_name: Cow::Borrowed(func_name), // No allocation - borrows from SymbolIndex
                    caller_start_address: func_addr,
                    caller_file: None,
                    call_site_addr: data.address,
                    file: None,
                    line: None,
                    column: None,
                });
            }
        });

        // Convert DashMap to HashMap and sort each caller list for deterministic ordering
        let mut edges: HashMap<u64, Vec<CallerInfo<'a>>> = edges.into_iter().collect();
        for callers in edges.values_mut() {
            callers.sort_by_key(|c| c.call_site_addr);
        }
        Ok(Self { edges })
    }

    /// Build a call graph by scanning all instructions once (no debug info).
    /// Non-parallel version for comparison or single-threaded mode.
    #[allow(dead_code)]
    pub fn build_sequential(
        macho: &MachO,
        buffer: &[u8],
        symbol_index: Option<&'a SymbolIndex>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut edges: HashMap<u64, Vec<CallerInfo<'a>>> = HashMap::new();

        let Some((text_addr, text_data)) = get_text_section(macho, buffer) else {
            return Ok(Self { edges });
        };

        let cs = Capstone::new()
            .arm64()
            .mode(arch::arm64::ArchMode::Arm)
            .build()?;

        let instructions = cs
            .disasm_all(text_data, text_addr)
            .map_err(|e| format!("Disassembly failed: {e}"))?;

        for instruction in instructions.iter() {
            if let Some((call_target, caller_info)) =
                process_instruction_basic(symbol_index, instruction)
            {
                edges.entry(call_target).or_default().push(caller_info);
            }
        }

        Ok(Self { edges })
    }

    /// Build a call graph with debug info enrichment.
    /// Uses parallel disassembly and parallel processing for faster analysis.
    /// DWARF names are owned, symbol fallback names borrow from the provided SymbolIndex.
    pub fn build_with_debug_info(
        binary_macho: &MachO,
        binary_buffer: &[u8],
        debug_macho: &MachO,
        debug_buffer: &[u8],
        crate_src_path: Option<&str>,
        show_timings: bool,
        symbol_index: Option<&'a SymbolIndex>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        use std::time::Instant;

        // Pre-load DWARF info once (shared across threads)
        let step = Instant::now();
        let (functions, inlined) = get_functions_from_dwarf(debug_macho, debug_buffer)?;
        let num_functions = functions.len();
        let num_inlined = inlined.len();
        // Build function index for O(log n) lookups instead of O(n) linear search
        let function_index = FunctionIndex::new_with_inlined(functions, inlined);
        if show_timings {
            eprintln!(
                "    [cg timing] get_functions_from_dwarf: {:?} ({} functions, {} inlined)",
                step.elapsed(),
                num_functions,
                num_inlined
            );
        }

        let step = Instant::now();
        let dwarf = load_dwarf_sections(debug_macho, debug_buffer)?;
        if show_timings {
            eprintln!("    [cg timing] load_dwarf_sections: {:?}", step.elapsed());
        }

        let Some((text_addr, text_data)) = get_text_section(binary_macho, binary_buffer) else {
            return Ok(Self {
                edges: HashMap::new(),
            });
        };
        if show_timings {
            eprintln!(
                "    [cg timing] text section size: {} bytes",
                text_data.len()
            );
        }

        // Parallel disassembly - divides text section into chunks (ARM64 only)
        let step = Instant::now();
        #[cfg(target_arch = "aarch64")]
        let insn_data = parallel_disassemble_arm64(text_data, text_addr);

        #[cfg(not(target_arch = "aarch64"))]
        let insn_data = sequential_disassemble_arm64(text_data, text_addr);
        if show_timings {
            // insn_data contains only BL/B branch instructions (not all instructions)
            eprintln!(
                "    [cg timing] scan for branch instructions: {:?} ({} found)",
                step.elapsed(),
                insn_data.len()
            );
        }

        // Build both line tables in a single pass (saves iterating DWARF twice)
        let step = Instant::now();
        let (crate_line_table, full_line_table) = if let Some(path) = crate_src_path {
            let (crate_table, full_table) = FullLineTable::build_both(&dwarf, path)?;
            (Some(crate_table), full_table)
        } else {
            (None, FullLineTable::build(&dwarf)?)
        };
        if show_timings {
            eprintln!(
                "    [cg timing] build line tables: {:?} (crate: {} entries, full: {} entries)",
                step.elapsed(),
                crate_line_table.as_ref().map(|t| t.len()).unwrap_or(0),
                full_line_table.len()
            );
        }

        // Process bl instructions in parallel
        let step = Instant::now();
        let edges: DashMap<u64, Vec<CallerInfo<'a>>> = DashMap::new();

        insn_data.par_iter().for_each(|data| {
            if let Some(call_target) = data.call_target
                && let Some((target, caller_info)) = process_instruction_data_with_crate_table(
                    data,
                    &function_index,
                    crate_src_path,
                    &full_line_table,
                    crate_line_table.as_ref(),
                    symbol_index,
                )
            {
                edges.entry(target).or_default().push(caller_info);
            }
        });
        if show_timings {
            eprintln!("    [cg timing] process instructions: {:?}", step.elapsed(),);
        }

        // Convert DashMap to HashMap and sort each caller list for deterministic ordering
        let mut edges: HashMap<u64, Vec<CallerInfo<'a>>> = edges.into_iter().collect();
        for callers in edges.values_mut() {
            callers.sort_by_key(|c| c.call_site_addr);
        }
        Ok(Self { edges })
    }

    /// Get all callers of a target address.
    /// Returns a slice reference to avoid cloning CallerInfo instances.
    pub fn get_callers(&self, target_addr: u64) -> &[CallerInfo<'a>] {
        self.edges
            .get(&target_addr)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Create an empty call graph.
    pub fn empty() -> Self {
        Self {
            edges: HashMap::new(),
        }
    }
}

/// ARM64 relocation type for BL/B instructions (branch with 26-bit offset)
const ARM64_RELOC_BRANCH26: u8 = 2;

/// Line entry for DWARF line table lookups
#[derive(Debug, Clone)]
struct ObjectLineEntry {
    address: u64,
    file: String,
    line: u32,
    column: Option<u32>,
}

/// Line table built from DWARF debug info for address -> file/line lookups.
/// Used for enriching LibraryCallGraph with source location info.
struct ObjectLineTable {
    entries: Vec<ObjectLineEntry>,
}

impl ObjectLineTable {
    /// Build a line table from DWARF debug info.
    fn build<R: Reader>(dwarf: &Dwarf<R>) -> Result<Self, gimli::Error> {
        let mut entries = Vec::new();

        let mut units = dwarf.units();
        while let Some(header) = units.next()? {
            let unit = dwarf.unit(header)?;

            if let Some(program) = &unit.line_program {
                // Build file path lookup from line program header
                let header = program.header();
                let mut file_paths: Vec<String> = Vec::new();

                // Index 0 is reserved, start from index 1
                file_paths.push(String::new()); // placeholder for index 0

                for file_entry in header.file_names() {
                    let file_name = dwarf
                        .attr_string(&unit, file_entry.path_name())?
                        .to_string_lossy()?
                        .into_owned();

                    let full_path = if let Some(dir) = file_entry.directory(header) {
                        let dir_str = dwarf
                            .attr_string(&unit, dir)?
                            .to_string_lossy()?
                            .into_owned();
                        if dir_str.is_empty() {
                            file_name
                        } else {
                            format!("{}/{}", dir_str, file_name)
                        }
                    } else {
                        file_name
                    };
                    file_paths.push(full_path);
                }

                // Iterate rows and collect entries
                let mut rows = program.clone().rows();
                while let Some((_, row)) = rows.next_row()? {
                    let file_idx = row.file_index() as usize;
                    if let Some(line) = row.line() {
                        if file_idx < file_paths.len() && file_idx > 0 {
                            let column = match row.column() {
                                ColumnType::LeftEdge => None,
                                ColumnType::Column(c) => Some(c.get() as u32),
                            };
                            entries.push(ObjectLineEntry {
                                address: row.address(),
                                file: file_paths[file_idx].clone(),
                                line: line.get() as u32,
                                column,
                            });
                        }
                    }
                }
            }
        }

        // Sort by address for binary search
        entries.sort_by_key(|e| e.address);

        Ok(Self { entries })
    }

    /// Look up file/line/column for an address. Returns (file, line, column) if found.
    fn lookup(&self, address: u64) -> Option<(Option<String>, Option<u32>, Option<u32>)> {
        if self.entries.is_empty() {
            return None;
        }

        // Binary search for the largest address <= target
        let idx = match self.entries.binary_search_by_key(&address, |e| e.address) {
            Ok(i) => i,
            Err(0) => return None, // address is before first entry
            Err(i) => i - 1,       // use previous entry
        };

        let entry = &self.entries[idx];
        Some((Some(entry.file.clone()), Some(entry.line), entry.column))
    }

    /// Find the last crate source line entry in the range [func_start, call_site_addr].
    /// This provides more precise line numbers for calls within a function.
    fn get_crate_line_in_range(
        &self,
        func_start: u64,
        call_site_addr: u64,
        crate_src_path: &str,
    ) -> Option<(String, u32, Option<u32>)> {
        if self.entries.is_empty() {
            return None;
        }

        // Find entries in range [func_start, call_site_addr]
        let start_idx = self.entries.partition_point(|e| e.address < func_start);
        let end_idx = self
            .entries
            .partition_point(|e| e.address <= call_site_addr);

        // Search backwards from end to find the last crate source entry
        for i in (start_idx..end_idx).rev() {
            let entry = &self.entries[i];
            if matches_crate_pattern(&entry.file, crate_src_path) {
                return Some((entry.file.clone(), entry.line, entry.column));
            }
        }

        None
    }
}

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
    /// * `macho` - Parsed MachO from the object file
    /// * `buffer` - Raw bytes of the object file
    /// * `crate_src_path` - Optional crate source path pattern for precise line numbers
    pub fn build_from_object(
        macho: &MachO,
        buffer: &[u8],
        crate_src_path: Option<&str>,
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
        let symbol_index = SymbolIndex::new(macho);

        // Load DWARF for file/line lookups
        let dwarf = load_dwarf_sections(macho, buffer).ok();
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

                        // If call site points to library code, find the last crate source
                        // line between function start and call site for precise line numbers
                        let (file, line, column) = if file.as_ref().is_some_and(|f| {
                            f.starts_with("/rustc/")
                                || f.contains("/.cargo/")
                                || f.contains("/deps/")
                        }) {
                            // Try to find precise line in crate source
                            if let Some(crate_path) = crate_src_path
                                && let Some(lt) = line_lookup.as_ref()
                                && let Some((crate_file, crate_line, crate_col)) = lt
                                    .get_crate_line_in_range(func_addr, call_site_addr, crate_path)
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

/// Process a single instruction and extract call information (basic version without debug info).
/// Returns (call_target, CallerInfo) if this is a bl/b instruction, None otherwise.
fn process_instruction_basic<'a>(
    symbol_index: Option<&'a SymbolIndex>,
    instruction: &Insn,
) -> Option<(u64, CallerInfo<'a>)> {
    // Match both BL (branch with link) and B (branch) for tail call detection
    let mnemonic = instruction.mnemonic();
    if mnemonic != Some("bl") && mnemonic != Some("b") {
        return None;
    }

    let operand = instruction.op_str()?;
    let addr_str = operand.trim_start_matches("#0x");
    let call_target = u64::from_str_radix(addr_str, 16).ok()?;
    let (func_addr, func_name) =
        symbol_index.and_then(|idx| idx.find_containing(instruction.address()))?;

    Some((
        call_target,
        CallerInfo {
            caller_name: Cow::Borrowed(func_name),
            caller_start_address: func_addr,
            caller_file: None,
            call_site_addr: instruction.address(),
            file: None,
            line: None,
            column: None,
        },
    ))
}

/// Process instruction data using pre-built line tables for fast O(log n) lookups.
/// Falls back to symbol table lookup if DWARF doesn't contain the function.
/// DWARF names use Cow::Owned, symbol fallback uses Cow::Borrowed from SymbolIndex.
fn process_instruction_data_with_crate_table<'a>(
    data: &InsnData,
    function_index: &FunctionIndex,
    crate_src_path: Option<&str>,
    full_line_table: &FullLineTable,
    crate_line_table: Option<&CrateLineTable>,
    symbol_index: Option<&'a SymbolIndex>,
) -> Option<(u64, CallerInfo<'a>)> {
    let call_target = data.call_target?;

    // Find the function containing this call - try DWARF first, fall back to symbol table
    // Uses O(log n) binary search instead of O(n) linear search
    if let Some(func) = function_index.find_containing(data.address) {
        // Found in DWARF - use full debug info
        // Get source location using O(log n) binary search on pre-built table
        let (file, mut line, mut column) = if func.file.is_some() && func.line.is_some() {
            // Fast path: use DWARF function info directly
            (func.file.clone(), func.line, None)
        } else {
            // Fast path: use pre-built full line table for O(log n) lookup
            let (func_file, func_line, func_column) =
                full_line_table.get_source_location(func.start_address);
            (
                func.file.clone().or(func_file),
                func.line.or(func_line),
                func_column,
            )
        };

        // For functions in the crate source, find actual call line using pre-built table
        let file_in_crate = file.as_ref().is_some_and(|f| {
            crate_src_path.is_some_and(|crate_path| matches_crate_pattern(f, crate_path))
        });
        if file_in_crate {
            // Use pre-built crate line table for O(log n) lookup
            if let Some(table) = crate_line_table {
                let (crate_line, crate_column) = table.get_line(func.start_address, data.address);
                if crate_line.is_some() {
                    line = crate_line;
                    column = crate_column;
                }
            }
        }

        // Get function name - only do expensive inlined lookup for crate source functions
        // For non-crate functions, use the containing function's name (fast path)
        let display_name: Cow<'a, str> = if file_in_crate {
            // Crate function: get the most specific name (checks inlined functions)
            Cow::Owned(
                function_index
                    .find_function_name(data.address)
                    .map(|s| {
                        let stripped = s.strip_prefix('_').unwrap_or(s);
                        format!("{:#}", demangle(stripped))
                    })
                    .unwrap_or_else(|| func.name.clone()),
            )
        } else {
            // Non-crate function: use containing function name directly (skip inlined search)
            Cow::Owned(func.name.clone())
        };

        // Note: We intentionally use the inlined function's name but keep the
        // containing function's address range (start/end) for call graph building.
        // The caller.file/line fields are the function *definition* location,
        // while CallerInfo.file/line/column below are the actual *call site*.
        Some((
            call_target,
            CallerInfo {
                caller_name: display_name,
                caller_start_address: func.start_address,
                caller_file: func.file.clone(),
                call_site_addr: data.address,
                file,
                line,
                column,
            },
        ))
    } else if let Some((func_addr, func_name)) =
        symbol_index.and_then(|idx| idx.find_containing(data.address))
    {
        // Fallback: found in symbol table but not in DWARF (e.g., expect_failed, unwrap_failed)
        // Don't use line table for source location - addresses outside DWARF functions
        // would incorrectly get source info from adjacent functions.
        // These are typically stdlib internals that we don't need source info for anyway.
        Some((
            call_target,
            CallerInfo {
                caller_name: Cow::Borrowed(func_name), // No allocation - borrows from SymbolIndex
                caller_start_address: func_addr,
                caller_file: None,
                call_site_addr: data.address,
                file: None,
                line: None,
                column: None,
            },
        ))
    } else {
        // Function not found in DWARF or symbol table - skip this edge
        None
    }
}

// TODO Note that the address passed in is an n_value or Symbol table offset,
// which is not necessarily the same as the address of the symbol in memory.
// How can we fix that?
// TODO using [cfg] have implementations for other architectures
pub(crate) fn find_callers(
    macho: &MachO,
    buffer: &[u8],
    target_addr: u64,
) -> Result<Vec<CallerInfo<'static>>, Box<dyn std::error::Error>> {
    let mut callers = Vec::new();

    let Some((text_addr, text_data)) = get_text_section(macho, buffer) else {
        return Ok(callers);
    };

    let cs = Capstone::new()
        .arm64()
        .mode(arch::arm64::ArchMode::Arm)
        .build()?;

    let Ok(instructions) = cs.disasm_all(text_data, text_addr) else {
        return Ok(callers);
    };

    // Precompute symbol index once for efficient lookups
    let symbol_index = SymbolIndex::new(macho);

    for instruction in instructions.iter() {
        // Match both BL (branch with link) and B (branch) for tail call detection
        let mnemonic = instruction.mnemonic();
        if (mnemonic == Some("bl") || mnemonic == Some("b"))
            && let Some(operand) = instruction.op_str()
        {
            let addr_str = operand.trim_start_matches("#0x");
            if let Ok(call_target) = u64::from_str_radix(addr_str, 16)
                && call_target == target_addr
                && let Some((func_addr, func_name)) = symbol_index
                    .as_ref()
                    .and_then(|idx| idx.find_containing(instruction.address()))
            {
                callers.push(CallerInfo {
                    caller_name: Cow::Owned(func_name.to_string()),
                    caller_start_address: func_addr,
                    caller_file: None,
                    call_site_addr: instruction.address(),
                    file: None,
                    line: None,
                    column: None,
                });
            }
        }
    }

    Ok(callers)
}

/// Load DWARF sections from MachO binary
fn load_dwarf_sections<'a>(
    macho: &'a MachO,
    buffer: &'a [u8],
) -> Result<Dwarf<DwarfReader<'a>>, gimli::Error> {
    let endian = if macho.little_endian {
        RunTimeEndian::Little
    } else {
        RunTimeEndian::Big
    };

    // Helper to find a DWARF section in the MachO
    let find_section = |name: &str| -> Option<&'a [u8]> {
        for segment in macho.segments.iter() {
            if let Ok(sections) = segment.sections() {
                for (section, _) in sections {
                    // MachO DWARF sections are like "__debug_info" in the "__DWARF" segment
                    if let Ok(sect_name) = section.name() {
                        // Convert gimli section name to MachO format
                        // e.g., ".debug_info" -> "__debug_info"
                        let macho_name = format!("__{}", &name[1..]);
                        if sect_name == macho_name {
                            let start = section.offset as usize;
                            let end = start + section.size as usize;
                            return Some(&buffer[start..end]);
                        }
                    }
                }
            }
        }
        None
    };

    // Load each DWARF section
    let load_section = |id: SectionId| -> Result<DwarfReader<'a>, gimli::Error> {
        let data = find_section(id.name()).unwrap_or(&[]);
        Ok(EndianSlice::new(data, endian))
    };

    Dwarf::load(&load_section)
}

/// Extract all functions from DWARF debug info.
/// Returns a tuple of (regular functions, inlined functions).
pub fn get_functions_from_dwarf<'a>(
    macho: &'a MachO,
    buffer: &'a [u8],
) -> Result<(Vec<FunctionInfo>, Vec<FunctionInfo>), Box<dyn std::error::Error>> {
    let dwarf = load_dwarf_sections(macho, buffer)?;

    // Collect all unit headers first (fast)
    let mut headers = Vec::new();
    let mut units_iter = dwarf.units();
    while let Some(header) = units_iter.next()? {
        headers.push(header);
    }

    // Process compilation units in parallel
    let results: Vec<_> = headers
        .into_par_iter()
        .filter_map(|header| {
            let unit = dwarf.unit(header).ok()?;
            let mut funcs = Vec::new();
            let mut inl = Vec::new();

            let mut entries = unit.entries();
            while let Some((_, entry)) = entries.next_dfs().ok()? {
                match entry.tag() {
                    gimli::DW_TAG_subprogram => {
                        if let Ok(Some(func)) = parse_function_die(&dwarf, &unit, entry) {
                            funcs.push(func);
                        }
                    }
                    gimli::DW_TAG_inlined_subroutine => {
                        if let Ok(Some(func)) = parse_inlined_subroutine(&dwarf, &unit, entry) {
                            inl.push(func);
                        }
                    }
                    _ => {}
                }
            }
            Some((funcs, inl))
        })
        .collect();

    // Combine results
    let mut functions = Vec::new();
    let mut inlined = Vec::new();
    for (funcs, inl) in results {
        functions.extend(funcs);
        inlined.extend(inl);
    }

    Ok((functions, inlined))
}

/// Parse a DW_TAG_subprogram DIE into FunctionInfo
fn parse_function_die<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<R>,
) -> Result<Option<FunctionInfo>, gimli::Error> {
    let mut name: Option<String> = None;
    let mut low_pc: Option<u64> = None;
    let mut high_pc: Option<u64> = None;
    let mut high_pc_is_offset = false;
    let mut file: Option<String> = None;
    let mut line: Option<u32> = None;

    let mut attrs = entry.attrs();
    while let Some(attr) = attrs.next()? {
        match attr.name() {
            gimli::DW_AT_name => {
                if let Ok(s) = dwarf.attr_string(unit, attr.value()) {
                    name = Some(s.to_string_lossy()?.into_owned());
                }
            }
            gimli::DW_AT_linkage_name | gimli::DW_AT_MIPS_linkage_name => {
                // Prefer mangled name if available
                if let Ok(s) = dwarf.attr_string(unit, attr.value()) {
                    name = Some(s.to_string_lossy()?.into_owned());
                }
            }
            gimli::DW_AT_low_pc => {
                if let AttributeValue::Addr(addr) = attr.value() {
                    low_pc = Some(addr);
                }
            }
            gimli::DW_AT_high_pc => match attr.value() {
                AttributeValue::Addr(addr) => {
                    high_pc = Some(addr);
                }
                AttributeValue::Udata(offset) => {
                    high_pc = Some(offset);
                    high_pc_is_offset = true;
                }
                _ => {}
            },
            gimli::DW_AT_decl_file => {
                if let AttributeValue::FileIndex(idx) = attr.value()
                    && let Some(line_program) = &unit.line_program
                    && let Some(file_entry) = line_program.header().file(idx)
                    && let Some(dir) = file_entry.directory(line_program.header())
                {
                    let dir_str = dwarf.attr_string(unit, dir.clone())?;
                    let file_str = dwarf.attr_string(unit, file_entry.path_name())?;
                    file = Some(format!(
                        "{}/{}",
                        dir_str.to_string_lossy()?,
                        file_str.to_string_lossy()?
                    ));
                }
            }
            gimli::DW_AT_decl_line => {
                if let AttributeValue::Udata(l) = attr.value() {
                    line = Some(l as u32);
                }
            }
            _ => {}
        }
    }

    // Calculate actual high_pc if it was an offset
    let high_pc = match (low_pc, high_pc, high_pc_is_offset) {
        (Some(low), Some(high), true) => Some(low + high),
        (_, high, false) => high,
        _ => None,
    };

    match (name, low_pc, high_pc) {
        (Some(name), Some(low_pc), Some(high_pc)) => Ok(Some(FunctionInfo {
            name,
            start_address: low_pc,
            end_address: high_pc,
            file,
            line,
        })),
        _ => Ok(None),
    }
}

/// Parse a DW_TAG_inlined_subroutine DIE into FunctionInfo.
/// Inlined subroutines use DW_AT_abstract_origin to reference the original function.
/// Handles both DW_AT_low_pc/DW_AT_high_pc and DW_AT_ranges (DWARF 5).
fn parse_inlined_subroutine<R: Reader<Offset = usize>>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<R>,
) -> Result<Option<FunctionInfo>, gimli::Error> {
    let mut abstract_origin: Option<gimli::UnitOffset<usize>> = None;
    let mut low_pc: Option<u64> = None;
    let mut high_pc: Option<u64> = None;
    let mut high_pc_is_offset = false;
    let mut ranges_attr: Option<AttributeValue<R>> = None;

    let mut attrs = entry.attrs();
    while let Some(attr) = attrs.next()? {
        match attr.name() {
            gimli::DW_AT_abstract_origin => {
                if let AttributeValue::UnitRef(offset) = attr.value() {
                    abstract_origin = Some(offset);
                }
            }
            gimli::DW_AT_low_pc => {
                if let AttributeValue::Addr(addr) = attr.value() {
                    low_pc = Some(addr);
                }
            }
            gimli::DW_AT_high_pc => match attr.value() {
                AttributeValue::Addr(addr) => {
                    high_pc = Some(addr);
                }
                AttributeValue::Udata(offset) => {
                    high_pc = Some(offset);
                    high_pc_is_offset = true;
                }
                _ => {}
            },
            gimli::DW_AT_ranges => {
                // DWARF 5: inlined subroutines can use DW_AT_ranges for non-contiguous ranges
                ranges_attr = Some(attr.value());
            }
            _ => {}
        }
    }

    // Calculate actual high_pc if it was an offset
    let high_pc = match (low_pc, high_pc, high_pc_is_offset) {
        (Some(low), Some(high), true) => Some(low + high),
        (_, high, false) => high,
        _ => None,
    };

    // If we have ranges instead of low_pc/high_pc, use the first range
    // (FunctionInfo only stores a single range; multi-range support would require Vec)
    let (final_low_pc, final_high_pc) = if low_pc.is_some() && high_pc.is_some() {
        (low_pc, high_pc)
    } else if let Some(ranges_value) = ranges_attr {
        // Try to get the first range from DW_AT_ranges using gimli's attr_ranges helper
        if let Ok(Some(mut ranges)) = dwarf.attr_ranges(unit, ranges_value) {
            if let Ok(Some(range)) = ranges.next() {
                (Some(range.begin), Some(range.end))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    // Resolve the function name from abstract_origin
    let name = if let Some(offset) = abstract_origin {
        resolve_abstract_origin_name(dwarf, unit, offset)?
    } else {
        None
    };

    match (name, final_low_pc, final_high_pc) {
        (Some(name), Some(low_pc), Some(high_pc)) => Ok(Some(FunctionInfo {
            name,
            start_address: low_pc,
            end_address: high_pc,
            file: None,
            line: None,
        })),
        _ => Ok(None),
    }
}

/// Resolve the function name from an abstract_origin reference.
fn resolve_abstract_origin_name<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    offset: gimli::UnitOffset<R::Offset>,
) -> Result<Option<String>, gimli::Error> {
    let entry = unit.entry(offset)?;
    let mut name: Option<String> = None;

    let mut attrs = entry.attrs();
    while let Some(attr) = attrs.next()? {
        match attr.name() {
            gimli::DW_AT_linkage_name | gimli::DW_AT_MIPS_linkage_name => {
                // Prefer mangled name if available
                if let Ok(s) = dwarf.attr_string(unit, attr.value()) {
                    name = Some(s.to_string_lossy()?.into_owned());
                }
            }
            gimli::DW_AT_name => {
                if name.is_none() {
                    if let Ok(s) = dwarf.attr_string(unit, attr.value()) {
                        name = Some(s.to_string_lossy()?.into_owned());
                    }
                }
            }
            _ => {}
        }
    }

    Ok(name)
}

/// Find all functions that call a target address, with source info
///
/// # Arguments
/// * `binary_macho` - Parsed MachO from the executable binary (contains __text section)
/// * `binary_buffer` - Raw bytes of the executable binary
/// * `debug_macho` - Parsed MachO containing DWARF info (can be same as binary_macho, or from dSYM)
/// * `debug_buffer` - Raw bytes containing DWARF info (can be same as binary_buffer, or from dSYM)
/// * `target_addr` - Address of the function to find callers for
/// * `crate_src_path` - Optional crate source path for precise line numbers in user code
pub fn find_callers_with_debug_info(
    binary_macho: &MachO,
    binary_buffer: &[u8],
    debug_macho: &MachO,
    debug_buffer: &[u8],
    target_addr: u64,
    crate_src_path: Option<&str>,
) -> Result<Vec<CallerInfo<'static>>, Box<dyn std::error::Error>> {
    // Get function info and DWARF from debug info (dSYM or embedded)
    let (functions, inlined) = get_functions_from_dwarf(debug_macho, debug_buffer)?;
    // Build function index for O(log n) lookups
    let function_index = FunctionIndex::new_with_inlined(functions, inlined);
    let dwarf = load_dwarf_sections(debug_macho, debug_buffer)?;
    let mut callers = Vec::new();

    // Get __text section from the binary (not dSYM)
    let Some((text_addr, text_data)) = get_text_section(binary_macho, binary_buffer) else {
        return Ok(callers);
    };

    // Use capstone for ARM64 disassembly
    let cs = Capstone::new()
        .arm64()
        .mode(arch::arm64::ArchMode::Arm)
        .build()?;

    let instructions = cs.disasm_all(text_data, text_addr)?;

    // Precompute symbol index once for efficient fallback lookups
    let symbol_index = SymbolIndex::new(binary_macho);

    for instruction in instructions.iter() {
        // Look for BL (branch with link) and B (branch) instructions
        // B is used for tail calls where the compiler optimizes `call; ret` into a single jump
        let mnemonic = instruction.mnemonic();
        if (mnemonic == Some("bl") || mnemonic == Some("b"))
            && let Some(operand) = instruction.op_str()
        {
            let addr_str = operand.trim_start_matches("#0x");
            if let Ok(call_target) = u64::from_str_radix(addr_str, 16)
                && call_target == target_addr
            {
                // Find the function containing this call - try DWARF first, fall back to symbol table
                // Uses O(log n) binary search instead of O(n) linear search
                if let Some(func) = function_index.find_containing(instruction.address()) {
                    // Found in DWARF - use full debug info
                    let (func_file, func_line, func_column) =
                        get_source_location(&dwarf, func.start_address)?;

                    // Prefer function's declaration file/line if available, then function start's line info
                    let file = func.file.clone().or(func_file);
                    let mut line = func.line.or(func_line);
                    let mut column = func_column;

                    // For functions in the crate source, find the actual line where the call originates
                    if let (Some(f), Some(crate_path)) = (&file, crate_src_path)
                        && f.contains(crate_path)
                        && let Ok(Some((crate_line, crate_column))) = get_crate_line_at_address(
                            &dwarf,
                            func.start_address,
                            instruction.address(),
                            crate_path,
                        )
                    {
                        line = Some(crate_line);
                        column = crate_column;
                    }

                    // Get the most specific function name (checks inlined functions first)
                    // Demangle if it's a mangled Rust name
                    let display_name = function_index
                        .find_function_name(instruction.address())
                        .map(|s| {
                            let stripped = s.strip_prefix('_').unwrap_or(s);
                            format!("{:#}", demangle(stripped))
                        })
                        .unwrap_or_else(|| func.name.clone());

                    // Note: See comment in process_instruction_data_with_crate_table
                    // for why we use inlined name but containing function's address range
                    callers.push(CallerInfo {
                        caller_name: Cow::Owned(display_name),
                        caller_start_address: func.start_address,
                        caller_file: func.file.clone(),
                        call_site_addr: instruction.address(),
                        file,
                        line,
                        column,
                    });
                } else if let Some((func_addr, func_name)) = symbol_index
                    .as_ref()
                    .and_then(|idx| idx.find_containing(instruction.address()))
                {
                    // Fallback: found in symbol table but not in DWARF (e.g., expect_failed, unwrap_failed)
                    let (file, line, column) =
                        get_source_location(&dwarf, func_addr).unwrap_or((None, None, None));

                    callers.push(CallerInfo {
                        caller_name: Cow::Owned(func_name.to_string()),
                        caller_start_address: func_addr,
                        caller_file: None,
                        call_site_addr: instruction.address(),
                        file,
                        line,
                        column,
                    });
                }
            }
        }
    }

    Ok(callers)
}

/// Get source file, line, and column for an address using DWARF line info
fn get_source_location<R: Reader>(
    dwarf: &Dwarf<R>,
    addr: u64,
) -> Result<SourceLocation, gimli::Error> {
    let mut units = dwarf.units();

    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;

        if let Some(program) = &unit.line_program {
            let mut rows = program.clone().rows();
            let mut prev_row: Option<(String, u32, Option<u32>)> = None;

            while let Some((header, row)) = rows.next_row()? {
                if row.address() > addr {
                    // The previous row covers this address
                    if let Some((file, line, column)) = prev_row {
                        return Ok((Some(file), Some(line), column));
                    }
                }

                if let Some(file_entry) = row.file(header) {
                    // Get the directory and filename to build the full path
                    let file_name = dwarf
                        .attr_string(&unit, file_entry.path_name())?
                        .to_string_lossy()?
                        .into_owned();

                    let full_path = if let Some(dir) = file_entry.directory(header) {
                        let dir_str = dwarf
                            .attr_string(&unit, dir)?
                            .to_string_lossy()?
                            .into_owned();
                        if dir_str.is_empty() {
                            file_name
                        } else {
                            format!("{}/{}", dir_str, file_name)
                        }
                    } else {
                        file_name
                    };

                    let column = match row.column() {
                        ColumnType::LeftEdge => None,
                        ColumnType::Column(c) => Some(c.get() as u32),
                    };
                    prev_row = Some((
                        full_path,
                        row.line().map(|l| l.get() as u32).unwrap_or(0),
                        column,
                    ));
                }
            }
        }
    }

    Ok((None, None, None))
}

/// Find the user code line and column closest to a specific address within a function.
/// This finds the last line entry in user code before the given call site address.
fn get_crate_line_at_address<R: Reader>(
    dwarf: &Dwarf<R>,
    func_start: u64,
    call_site_addr: u64,
    crate_src_path: &str,
) -> Result<Option<(u32, Option<u32>)>, gimli::Error> {
    let mut units = dwarf.units();
    let mut best_line: Option<u32> = None;
    let mut best_column: Option<u32> = None;
    let mut best_addr: u64 = 0;

    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;

        if let Some(program) = &unit.line_program {
            let mut rows = program.clone().rows();

            while let Some((header, row)) = rows.next_row()? {
                let addr = row.address();

                // Look for entries between function start and call site
                if addr >= func_start
                    && addr <= call_site_addr
                    && let Some(file_entry) = row.file(header)
                {
                    let file_name = dwarf
                        .attr_string(&unit, file_entry.path_name())?
                        .to_string_lossy()?
                        .into_owned();

                    let full_path = if let Some(dir) = file_entry.directory(header) {
                        let dir_str = dwarf
                            .attr_string(&unit, dir)?
                            .to_string_lossy()?
                            .into_owned();
                        if dir_str.is_empty() {
                            file_name
                        } else {
                            format!("{}/{}", dir_str, file_name)
                        }
                    } else {
                        file_name
                    };

                    // Check if this line is in the crate source
                    if matches_crate_pattern(&full_path, crate_src_path)
                        && let Some(line) = row.line()
                        && addr >= best_addr
                    {
                        best_addr = addr;
                        best_line = Some(line.get() as u32);
                        best_column = match row.column() {
                            ColumnType::LeftEdge => None,
                            ColumnType::Column(c) => Some(c.get() as u32),
                        };
                    }
                }
            }
        }
    }

    Ok(best_line.map(|l| (l, best_column)))
}

pub fn find_dsym(binary_path: &Path) -> Option<PathBuf> {
    // dSYM is typically at: /path/to/binary.dSYM/Contents/Resources/DWARF/binary
    let dsym_bundle = binary_path.with_extension("dSYM");

    if dsym_bundle.exists() {
        let binary_name = binary_path.file_name()?;
        let dwarf_path = dsym_bundle
            .join("Contents")
            .join("Resources")
            .join("DWARF")
            .join(binary_name);

        if dwarf_path.exists() {
            return Some(dwarf_path);
        }
    }
    None
}

/// Check if the binary has any DWARF debug sections
pub fn has_dwarf_sections(macho: &MachO) -> bool {
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

/// Extract object file paths from the debug map (OSO stab entries)
fn get_oso_paths(macho: &MachO) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(symbols) = &macho.symbols {
        for (name, nlist) in symbols.iter().flatten() {
            // N_OSO (0x66) indicates an object file reference
            if nlist.n_type == N_OSO && !name.is_empty() {
                paths.push(PathBuf::from(name));
            }
        }
    }

    // Deduplicate paths
    paths.sort();
    paths.dedup();
    paths
}

/// Build address translation map from object file symbols to final binary addresses
fn build_addr_translation_map(binary_macho: &MachO, obj_macho: &MachO) -> HashMap<u64, u64> {
    let mut addr_map = HashMap::new();

    // Get symbols from both binary and object file
    let Some(binary_symbols) = &binary_macho.symbols else {
        return addr_map;
    };
    let Some(obj_symbols) = &obj_macho.symbols else {
        return addr_map;
    };

    // Build a map of symbol name -> address in binary
    let mut binary_sym_addrs: HashMap<String, u64> = HashMap::new();
    for (name, nlist) in binary_symbols.iter().flatten() {
        if nlist.n_value > 0 && !name.is_empty() {
            binary_sym_addrs.insert(name.to_string(), nlist.n_value);
        }
    }

    // For each symbol in the object file, find its final address in the binary
    for (name, nlist) in obj_symbols.iter().flatten() {
        if nlist.n_value > 0
            && !name.is_empty()
            && let Some(&binary_addr) = binary_sym_addrs.get(name)
        {
            addr_map.insert(nlist.n_value, binary_addr);
        }
    }

    addr_map
}

/// Find callers with source info using the debug map
/// This searches all object files for DWARF info
pub fn find_callers_with_debug_map(
    binary_macho: &MachO,
    binary_buffer: &[u8],
    debug_map: &DebugMapInfo,
    target_addr: u64,
    _crate_src_path: Option<&str>,
) -> Result<Vec<CallerInfo<'static>>, Box<dyn std::error::Error>> {
    // First, get callers from the binary's text section
    let mut callers = find_callers(binary_macho, binary_buffer, target_addr)?;

    // Build a map of function name -> source info from all object files
    let mut func_source_map: HashMap<String, (Option<String>, Option<u32>)> = HashMap::new();

    for obj_info in &debug_map.object_files {
        // Parse the object file
        let Ok(Object::Mach(Mach::Binary(obj_macho))) = Object::parse(&obj_info.buffer) else {
            continue;
        };

        // Get functions from this object file
        // TODO: _inlined is currently unused in the debug-map fallback path.
        // Using inlined functions here would require mapping object file addresses
        // to binary addresses, which is complex. The main path (dSYM/embedded DWARF)
        // handles inlined functions correctly via FunctionIndex.find_function_name().
        let Ok((functions, _inlined)) = get_functions_from_dwarf(&obj_macho, &obj_info.buffer)
        else {
            continue;
        };

        // Store source info by function name (both mangled and demangled)
        for func in functions {
            if func.file.is_some() {
                // Store by original name
                func_source_map.insert(func.name.clone(), (func.file.clone(), func.line));

                // Also store by demangled name for matching
                let stripped = func.name.strip_prefix("_").unwrap_or(&func.name);
                let demangled = format!("{:#}", demangle(stripped));
                if demangled != func.name {
                    func_source_map.insert(demangled, (func.file.clone(), func.line));
                }
            }
        }
    }

    // Enrich caller info with source locations from DWARF by matching function names
    for caller in &mut callers {
        // Try to find source info by function name
        // Note: caller_name is Cow<'static, str>, convert to string for lookup
        if let Some((file, line)) = func_source_map.get(caller.caller_name.as_ref()) {
            caller.caller_file = file.clone();
            caller.file = file.clone();
            caller.line = *line;
        }
    }

    Ok(callers)
}

/// Load debug map from the binary's symbol table
/// This reads OSO entries and loads DWARF from the referenced object files
fn load_debug_map(macho: &MachO, quiet: bool) -> Option<DebugMapInfo> {
    let oso_paths = get_oso_paths(macho);

    if oso_paths.is_empty() {
        return None;
    }

    let mut object_files = Vec::new();
    let mut loaded_count = 0;

    for path in oso_paths {
        // Skip if object file doesn't exist
        if !path.exists() {
            continue;
        }

        // Read the object file
        let Ok(buffer) = fs::read(&path) else {
            continue;
        };

        // Parse as MachO and check for DWARF
        let Ok(Object::Mach(Mach::Binary(obj_macho))) = Object::parse(&buffer) else {
            continue;
        };

        // Only include if it has debug info
        if !has_dwarf_sections(&obj_macho) {
            continue;
        }

        // Build address translation map
        let addr_map = build_addr_translation_map(macho, &obj_macho);

        object_files.push(ObjectFileInfo {
            path,
            buffer,
            addr_map,
        });
        loaded_count += 1;
    }

    if object_files.is_empty() {
        return None;
    }

    if !quiet {
        println!(
            "Using debug map: loaded {} object files with DWARF",
            loaded_count
        );
    }

    Some(DebugMapInfo { object_files })
}

/// Check if a dSYM bundle is stale (binary is newer than the dSYM)
fn is_dsym_stale(binary_path: &Path, dsym_path: &Path) -> bool {
    let binary_modified = match fs::metadata(binary_path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return false, // Can't check, assume not stale
    };

    let dsym_modified = match fs::metadata(dsym_path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return true, // Can't read dSYM metadata, regenerate
    };

    binary_modified > dsym_modified
}

// 1) No embedded debug info, no dSYM
// 2) No embedded debug info, dSYM
// 3) Embedded debug info, no dSYM
// 4) Embedded debug info, dSYM
pub fn load_debug_info(macho: &MachO, binary_path: &Path, quiet: bool) -> DebugInfo {
    // Look for dSYM symbol directory
    // Try both with and without extension since dsymutil behavior varies
    let file_name = binary_path.file_name().unwrap().to_str().unwrap();
    let file_stem = binary_path.file_stem().unwrap().to_str().unwrap();

    // Try .dSYM bundle with full filename first
    let dsym_base = binary_path.parent().unwrap_or(Path::new("."));
    let dsym_paths = [
        // Pattern: binary.dSYM/Contents/Resources/DWARF/binary
        dsym_base
            .join(format!("{}.dSYM", file_stem))
            .join("Contents/Resources/DWARF")
            .join(file_name),
        // Pattern: binary.dSYM/Contents/Resources/DWARF/binary (without extension)
        dsym_base
            .join(format!("{}.dSYM", file_stem))
            .join("Contents/Resources/DWARF")
            .join(file_stem),
        // Pattern: binary.ext.dSYM/Contents/Resources/DWARF/binary.ext
        binary_path
            .with_extension("dSYM")
            .join("Contents/Resources/DWARF")
            .join(file_name),
    ];

    for dsym_path in &dsym_paths {
        if dsym_path.exists() {
            // Check if dSYM is stale (binary is newer than dSYM)
            let dsym_stale = is_dsym_stale(binary_path, dsym_path);
            if dsym_stale {
                if !quiet {
                    println!("  dSYM is stale, will regenerate");
                }
            } else {
                if !quiet {
                    println!("  Using .dSYM bundle for debug info");
                }
                let debug_buffer = fs::read(dsym_path).unwrap();
                let dsym_info = DSymInfoBuilder {
                    debug_buffer,
                    debug_macho_builder: |buf: &Vec<u8>| Mach::parse(buf).unwrap(),
                }
                .build();
                return DebugInfo::DSym(Box::new(dsym_info));
            }
        }
    }

    if has_dwarf_sections(macho) {
        if !quiet {
            println!("  Using embedded DWARF debugging info");
        }
        return DebugInfo::Embedded;
    }

    // Try to auto-generate dSYM using dsymutil
    if let Some(dsym_info) = auto_generate_dsym(binary_path, quiet) {
        return DebugInfo::DSym(Box::new(dsym_info));
    }

    // Fall back to debug map (reading DWARF from object files)
    if let Some(debug_map) = load_debug_map(macho, quiet) {
        return DebugInfo::DebugMap(Box::new(debug_map));
    }

    if !quiet {
        println!("  No debug info found (no dSYM, embedded DWARF, or debug map)");
        println!(
            "Tip: Install dsymutil or run 'dsymutil {}' to generate debug symbols",
            binary_path.display()
        );
    }
    DebugInfo::None
}

/// Auto-generate dSYM by running dsymutil
fn auto_generate_dsym(binary_path: &Path, quiet: bool) -> Option<DSymInfo> {
    use std::process::Command;

    let dsym_path = binary_path.with_extension("dSYM");

    // Check if dsymutil is available
    let status = Command::new("dsymutil")
        .arg(binary_path)
        .arg("-o")
        .arg(&dsym_path)
        .status()
        .ok()?;

    if !status.success() {
        return None;
    }

    // Find the DWARF file inside the dSYM bundle
    let file_name = binary_path.file_name()?.to_str()?;
    let file_stem = binary_path.file_stem()?.to_str()?;

    let dwarf_paths = [
        dsym_path.join("Contents/Resources/DWARF").join(file_name),
        dsym_path.join("Contents/Resources/DWARF").join(file_stem),
    ];

    for dwarf_path in &dwarf_paths {
        if dwarf_path.exists() {
            if !quiet {
                println!("  Generated .dSYM bundle for debug info");
            }
            let debug_buffer = fs::read(dwarf_path).ok()?;
            let dsym_info = DSymInfoBuilder {
                debug_buffer,
                debug_macho_builder: |buf: &Vec<u8>| Mach::parse(buf).unwrap(),
            }
            .build();
            return Some(dsym_info);
        }
    }

    None
}
