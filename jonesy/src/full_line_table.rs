#![allow(unused_variables)] // TODO Just for now

use crate::crate_line_table::{CrateLineEntry, CrateLineTable};
use crate::project_context::ProjectContext;
use crate::sym::resolve_line_file_path;
use gimli::{ColumnType, Dwarf, Reader};
use rayon::prelude::*;

/// Source location tuple: (file, line, column)
type SourceLocation = (Option<String>, Option<u32>, Option<u32>);

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
                        let full_path = resolve_line_file_path(dwarf, &unit, file_entry, header)?;

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

        // Sort by address for binary search (stable sort preserves unit order for
        // entries with same address, which get_source_location relies on)
        entries.sort_by_key(|e| e.address);

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

    /// Find the best crate-source line for a call site where stdlib code is inlined.
    ///
    /// When stdlib code (like `unwrap()`) is inlined into a crate function,
    /// the DWARF line table at the call instruction address points to the stdlib
    /// source (e.g., `option.rs:1014`), not the crate source line where the call
    /// was written. This method finds a better line by:
    ///
    /// 1. First searching backward from the call site for the nearest crate-source
    ///    entry (handles cases where there's a crate entry just before the inlined code).
    /// 2. If backward search only finds the function-start line, searches forward
    ///    to find the next crate-source entry after the inlined code and reports the
    ///    line just before it (typically the line containing the actual call expression).
    ///
    /// The search is bounded by `func_start`/`func_end` to avoid crossing function
    /// boundaries.
    pub fn get_nearest_crate_line(
        &self,
        addr: u64,
        func_start: u64,
        func_end: u64,
        func_start_line: Option<u32>,
        crate_src_path: &str,
        project_context: &ProjectContext,
    ) -> (Option<u32>, Option<u32>) {
        // Find entries up to and including addr
        let end_idx = self.entries.partition_point(|e| e.address <= addr);
        if end_idx == 0 {
            return (None, None);
        }

        // Search backward from the call site for the nearest crate-source entry
        for i in (0..end_idx).rev() {
            let entry = &self.entries[i];
            if entry.address < func_start {
                break;
            }
            if let Some(file) = self.file_pool.get(entry.file_id as usize) {
                if project_context.is_crate_source(file) && entry.line > 0 {
                    // Found a crate entry, but if it's just the function start,
                    // try searching forward for a better line
                    if func_start_line.is_some_and(|fl| entry.line == fl) {
                        break; // Fall through to forward search
                    }
                    return (Some(entry.line), entry.column);
                }
            }
        }

        // Backward search only found the function start. Search forward from
        // the call site to find the next crate-source entry (typically the
        // function epilogue / closing brace). The line just before that is
        // likely where the actual call expression is.
        for i in end_idx..self.entries.len() {
            let entry = &self.entries[i];
            if entry.address >= func_end {
                break;
            }
            if let Some(file) = self.file_pool.get(entry.file_id as usize) {
                if project_context.is_crate_source(file) && entry.line > 0 {
                    // Found the next crate entry (e.g., closing brace).
                    // The call is on the line before the epilogue.
                    if let Some(func_line) = func_start_line {
                        if entry.line > func_line + 1 {
                            return (Some(entry.line - 1), None);
                        }
                    }
                    return (Some(entry.line), entry.column);
                }
            }
        }

        (None, None)
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
        project_context: &ProjectContext,
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
                // Track last crate entry to carry forward at crate→stdlib transitions.
                // When inlined stdlib code (e.g., unwrap) appears, the DWARF line table
                // switches from crate source to stdlib source. We emit a synthetic crate
                // entry at the transition address so get_line() resolves to the call site
                // rather than falling back to the function start.
                let mut last_crate_line: Option<(u32, Option<u32>)> = None;
                let mut last_was_crate = false;

                while let Some((header, row)) = rows.next_row()? {
                    if let Some(file_entry) = row.file(header) {
                        let full_path = resolve_line_file_path(dwarf, &unit, file_entry, header)?;

                        // Check crate match before interning (needs &full_path)
                        let is_crate_match = project_context.is_crate_source(&full_path);

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
                            last_crate_line = Some((line, column));
                            last_was_crate = true;
                        } else {
                            // Transition from crate to non-crate (stdlib): emit synthetic
                            // crate entry at this address with the last crate line info
                            if last_was_crate {
                                if let Some((prev_line, prev_col)) = last_crate_line {
                                    crate_entries.push(CrateLineEntry {
                                        address: row.address(),
                                        line: prev_line,
                                        column: prev_col,
                                    });
                                }
                            }
                            last_was_crate = false;
                        }
                    }
                }
            }
        }

        // Sort by address for binary search
        // crate_entries: par_sort is safe (get_line returns last entry, order doesn't matter)
        // full_entries: stable sort needed (get_source_location returns first entry)
        crate_entries.par_sort_by_key(|e| e.address);
        full_entries.sort_by_key(|e| e.address);

        Ok((
            CrateLineTable::from_entries(crate_entries),
            FullLineTable {
                entries: full_entries,
                file_pool,
            },
        ))
    }
}
