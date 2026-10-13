#![allow(unused_variables)] // TODO Just for now

pub use crate::project_context::ProjectContext;
use crate::sym::resolve_line_file_path;
use gimli::{ColumnType, Dwarf, Reader};

/// A line entry for crate source code, used for fast lookups.
/// Uses `u16` for line/column with 0 meaning absent (saves space vs `Option<u16>`).
#[derive(Debug, Clone)]
#[repr(C, align(16))]
pub struct CrateLineEntry {
    pub address: u64,
    pub line: u16,
    pub column: u16,
}

/// Pre-built line table containing only crate source entries, sorted by address.
/// Used for fast binary search in get_crate_line_at_address.
#[derive(Debug)]
pub struct CrateLineTable {
    entries: Vec<CrateLineEntry>,
}

impl CrateLineTable {
    /// Build a line table with only entries from crate source files.
    pub fn build<R: Reader>(
        dwarf: &Dwarf<R>,
        project_context: &ProjectContext,
    ) -> Result<Self, gimli::Error> {
        let mut entries = Vec::new();

        let mut units = dwarf.units();
        while let Some(header) = units.next()? {
            let unit = dwarf.unit(header)?;

            if let Some(program) = &unit.line_program {
                let mut rows = program.clone().rows();

                while let Some((header, row)) = rows.next_row()? {
                    if let Some(file_entry) = row.file(header) {
                        let full_path = resolve_line_file_path(
                            dwarf,
                            &unit,
                            file_entry,
                            header,
                            project_context.project_root(),
                        )?;

                        // Only include entries from crate source
                        if project_context.is_crate_source(&full_path)
                            && let Some(line) = row.line()
                        {
                            let column = match row.column() {
                                ColumnType::LeftEdge => 0,
                                ColumnType::Column(c) => c.get() as u16,
                            };
                            entries.push(CrateLineEntry {
                                address: row.address(),
                                line: line.get() as u16,
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
    pub fn get_line(&self, func_start: u64, call_site_addr: u64) -> (u16, u16) {
        // Find entries in range [func_start, call_site_addr]
        let start_idx = self.entries.partition_point(|e| e.address < func_start);
        let end_idx = self
            .entries
            .partition_point(|e| e.address <= call_site_addr);

        // Return the last entry in range (highest address)
        if end_idx > start_idx {
            let entry = &self.entries[end_idx - 1];
            (entry.line, entry.column)
        } else {
            (0, 0)
        }
    }

    /// Find the nearest crate line entry to a call address within a function.
    ///
    /// Searches both backward and forward from `call_site_addr` and returns the
    /// entry closest by address. This handles inlined stdlib code correctly:
    /// when the call address is inside inlined code (e.g., `Option::unwrap`),
    /// the backward entry is the setup code before the inlined expansion, while
    /// the forward entry is the source line that triggered the inline call.
    /// Picking the nearest entry returns the correct call site line.
    pub fn get_nearest_line(
        &self,
        func_start: u64,
        call_site_addr: u64,
        func_end: u64,
    ) -> (u16, u16) {
        let start_idx = self.entries.partition_point(|e| e.address < func_start);
        let end_idx = self
            .entries
            .partition_point(|e| e.address <= call_site_addr);

        // Last backward entry in [func_start, call_site_addr]
        let backward = if end_idx > start_idx {
            Some(&self.entries[end_idx - 1])
        } else {
            None
        };

        // First forward entry in (call_site_addr, func_end)
        let forward = if end_idx < self.entries.len() {
            let entry = &self.entries[end_idx];
            if entry.address < func_end {
                Some(entry)
            } else {
                None
            }
        } else {
            None
        };

        match (backward, forward) {
            (Some(bw), Some(fw)) => {
                let bw_dist = call_site_addr - bw.address;
                let fw_dist = fw.address - call_site_addr;
                let entry = if fw_dist < bw_dist { fw } else { bw };
                (entry.line, entry.column)
            }
            (Some(bw), None) => (bw.line, bw.column),
            (None, _) => (0, 0),
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

    /// Create a CrateLineTable directly from entries (used by build_both)
    pub(crate) fn from_entries(entries: Vec<CrateLineEntry>) -> Self {
        Self { entries }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_table(entries: &[(u64, u16, u16)]) -> CrateLineTable {
        CrateLineTable::from_entries(
            entries
                .iter()
                .map(|&(address, line, column)| CrateLineEntry {
                    address,
                    line,
                    column,
                })
                .collect(),
        )
    }

    #[test]
    fn test_empty_table() {
        let table = make_table(&[]);
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
        assert_eq!(table.get_line(0, 100), (0, 0));
    }

    #[test]
    fn test_single_entry() {
        let table = make_table(&[(100, 10, 5)]);
        assert!(!table.is_empty());
        assert_eq!(table.len(), 1);
        assert_eq!(table.get_line(100, 100), (10, 5));
    }

    #[test]
    fn test_get_line_exact_match() {
        let table = make_table(&[(100, 10, 0), (200, 20, 3), (300, 30, 7)]);
        assert_eq!(table.get_line(200, 200), (20, 3));
    }

    #[test]
    fn test_get_line_returns_last_in_range() {
        // Two entries in range — should return the last one (highest address)
        let table = make_table(&[(100, 10, 0), (150, 15, 2), (200, 20, 0)]);
        assert_eq!(table.get_line(100, 200), (20, 0));
        assert_eq!(table.get_line(100, 160), (15, 2));
    }

    #[test]
    fn test_get_line_no_entries_in_range() {
        let table = make_table(&[(100, 10, 0), (200, 20, 0)]);
        // Range before all entries
        assert_eq!(table.get_line(0, 50), (0, 0));
        // Range between entries
        assert_eq!(table.get_line(110, 190), (0, 0));
    }

    #[test]
    fn test_get_line_func_start_equals_call_site() {
        let table = make_table(&[(100, 10, 1)]);
        assert_eq!(table.get_line(100, 100), (10, 1));
    }

    #[test]
    fn test_get_line_column_none() {
        let table = make_table(&[(100, 10, 0)]);
        assert_eq!(table.get_line(100, 100), (10, 0));
    }

    #[test]
    fn test_from_entries_preserves_order() {
        let table = make_table(&[(300, 30, 0), (100, 10, 0), (200, 20, 0)]);
        assert_eq!(table.len(), 3);
        // from_entries doesn't sort — entries are used as-is
        // (build() sorts, but from_entries trusts the caller)
    }

    // -- get_nearest_line tests --

    #[test]
    fn test_nearest_line_prefers_forward_when_closer() {
        // Simulates inlined stdlib code: crate entries at 100 (line 26) and 200 (line 28)
        // with a call at 180 (inside inlined code). Forward (200) is closer than backward (100).
        let table = make_table(&[(100, 26, 0), (200, 28, 29)]);
        assert_eq!(table.get_nearest_line(100, 180, 300), (28, 29));
    }

    #[test]
    fn test_nearest_line_prefers_backward_when_closer() {
        // Normal call: crate entries at 100 (line 10) and 200 (line 11)
        // with a call at 104 (just after line 10 start). Backward (100) is closer.
        let table = make_table(&[(100, 10, 5), (200, 11, 0)]);
        assert_eq!(table.get_nearest_line(100, 104, 300), (10, 5));
    }

    #[test]
    fn test_nearest_line_exact_match() {
        let table = make_table(&[(100, 10, 5), (200, 20, 0)]);
        assert_eq!(table.get_nearest_line(100, 100, 300), (10, 5));
    }

    #[test]
    fn test_nearest_line_no_entries() {
        let table = make_table(&[]);
        assert_eq!(table.get_nearest_line(100, 150, 300), (0, 0));
    }

    #[test]
    fn test_nearest_line_forward_beyond_func_end() {
        // Forward entry exists but is beyond func_end — should use backward
        let table = make_table(&[(100, 10, 0), (400, 40, 0)]);
        assert_eq!(table.get_nearest_line(100, 150, 300), (10, 0));
    }

    #[test]
    fn test_nearest_line_equal_distance_prefers_backward() {
        // Equal distance: prefer backward (the call instruction is on that source line)
        let table = make_table(&[(100, 10, 0), (200, 20, 0)]);
        assert_eq!(table.get_nearest_line(100, 150, 300), (10, 0));
    }
}
