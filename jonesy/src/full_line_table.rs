#![allow(unused_variables)] // TODO Just for now

use crate::crate_line_table::{CrateLineEntry, CrateLineTable};
use crate::project_context::ProjectContext;
use crate::sym::resolve_line_file_path;
use gimli::{ColumnType, Dwarf, Reader};
use rayon::prelude::*;

/// Source location tuple: (file, line, column)
type SourceLocation = (Option<String>, u16, u16);

/// A line entry for full source location lookups.
/// Uses file_id for string interning to reduce memory pressure.
/// Uses `u16` for line/column with 0 meaning absent (saves space vs `Option<u16>`).
#[derive(Debug, Clone)]
#[repr(C, align(16))]
pub struct FullLineEntry {
    pub address: u64,
    /// Index into FullLineTable's file_pool for the file path
    file_id: u32,
    pub line: u16,
    pub column: u16,
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
            (file, entry.line, entry.column)
        } else {
            (None, 0, 0)
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
        func_start_line: u16,
        project_context: &ProjectContext,
    ) -> (u16, u16) {
        // Find entries up to and including addr
        let end_idx = self.entries.partition_point(|e| e.address <= addr);
        if end_idx == 0 {
            return (0, 0);
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
                    if func_start_line > 0 && entry.line == func_start_line {
                        break; // Fall through to forward search
                    }
                    return (entry.line, entry.column);
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
                    if func_start_line > 0 && entry.line > func_start_line.saturating_add(1) {
                        return (entry.line - 1, 0);
                    }
                    return (entry.line, entry.column);
                }
            }
        }

        (0, 0)
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
    ///
    /// File paths are pre-resolved from the line program header (typically hundreds
    /// of entries per compilation unit) and cached in a Vec indexed by DWARF file
    /// index. This avoids calling `resolve_line_file_path` per row (millions of
    /// times), eliminating ~1.15 GB of transient string allocations.
    pub fn build_both<R: Reader>(
        dwarf: &Dwarf<R>,
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
                // Pre-resolve all file paths from the line program header.
                // The header contains one entry per unique source file
                // (typically tens to hundreds), compared to millions of row
                // iterations. This turns O(rows * path_alloc) into
                // O(files * path_alloc + rows * lookup).
                let prog_header = program.header();
                let mut header_file_ids: Vec<u32> = Vec::new();
                let mut header_is_crate: Vec<bool> = Vec::new();

                // Index 0 is reserved in DWARF file tables
                header_file_ids.push(0);
                header_is_crate.push(false);

                for file_entry in prog_header.file_names() {
                    let full_path = resolve_line_file_path(
                        dwarf,
                        &unit,
                        file_entry,
                        prog_header,
                        project_context.project_root(),
                    )?;

                    let is_crate_match = project_context.is_crate_source(&full_path);

                    // Intern the file path
                    let file_id = if let Some(&id) = file_to_id.get(&full_path) {
                        id
                    } else {
                        let id = file_pool.len() as u32;
                        file_pool.push(full_path.clone());
                        file_to_id.insert(full_path, id);
                        id
                    };

                    header_file_ids.push(file_id);
                    header_is_crate.push(is_crate_match);
                }

                let mut rows = program.clone().rows();
                // Track last crate entry to carry forward at crate->stdlib transitions.
                // When inlined stdlib code (e.g., unwrap) appears, the DWARF line table
                // switches from crate source to stdlib source. We emit a synthetic crate
                // entry at the transition address so get_line() resolves to the call site
                // rather than falling back to the function start.
                let mut last_crate_line: Option<(u16, u16)> = None;
                let mut last_was_crate = false;

                while let Some((_header, row)) = rows.next_row()? {
                    let file_idx = row.file_index() as usize;
                    if file_idx >= header_file_ids.len() {
                        continue;
                    }

                    let file_id = header_file_ids[file_idx];
                    let is_crate_match = header_is_crate[file_idx];

                    // Add to full line table (all entries)
                    let line = row
                        .line()
                        .map(|l| crate::to_u16_saturating(l.get()))
                        .unwrap_or(0);
                    let column: u16 = match row.column() {
                        ColumnType::LeftEdge => 0,
                        ColumnType::Column(c) => crate::to_u16_saturating(c.get()),
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

        // Sort by address for binary search.
        // crate_entries: unstable par_sort is safe (get_line returns last entry,
        // so tie-breaking order doesn't matter).
        // full_entries: stable sort preserves insertion order for entries at the
        // same address, which matters for get_source_location (returns the first
        // entry from the earliest compilation unit).
        crate_entries.par_sort_unstable_by_key(|e| e.address);
        full_entries.par_sort_by_key(|e| e.address);

        Ok((
            CrateLineTable::from_entries(crate_entries),
            FullLineTable {
                entries: full_entries,
                file_pool,
            },
        ))
    }
}
