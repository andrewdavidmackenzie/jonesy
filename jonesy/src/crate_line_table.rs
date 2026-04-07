#![allow(unused_variables)] // TODO Just for now

pub use crate::project_context::ProjectContext;
use crate::sym::resolve_line_file_path;
use gimli::{ColumnType, Dwarf, Reader};

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

    /// Create a CrateLineTable directly from entries (used by build_both)
    pub(crate) fn from_entries(entries: Vec<CrateLineEntry>) -> Self {
        Self { entries }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_table(entries: &[(u64, u32, Option<u32>)]) -> CrateLineTable {
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
        assert_eq!(table.get_line(0, 100), (None, None));
    }

    #[test]
    fn test_single_entry() {
        let table = make_table(&[(100, 10, Some(5))]);
        assert!(!table.is_empty());
        assert_eq!(table.len(), 1);
        assert_eq!(table.get_line(100, 100), (Some(10), Some(5)));
    }

    #[test]
    fn test_get_line_exact_match() {
        let table = make_table(&[(100, 10, None), (200, 20, Some(3)), (300, 30, Some(7))]);
        assert_eq!(table.get_line(200, 200), (Some(20), Some(3)));
    }

    #[test]
    fn test_get_line_returns_last_in_range() {
        // Two entries in range — should return the last one (highest address)
        let table = make_table(&[(100, 10, None), (150, 15, Some(2)), (200, 20, None)]);
        assert_eq!(table.get_line(100, 200), (Some(20), None));
        assert_eq!(table.get_line(100, 160), (Some(15), Some(2)));
    }

    #[test]
    fn test_get_line_no_entries_in_range() {
        let table = make_table(&[(100, 10, None), (200, 20, None)]);
        // Range before all entries
        assert_eq!(table.get_line(0, 50), (None, None));
        // Range between entries
        assert_eq!(table.get_line(110, 190), (None, None));
    }

    #[test]
    fn test_get_line_func_start_equals_call_site() {
        let table = make_table(&[(100, 10, Some(1))]);
        assert_eq!(table.get_line(100, 100), (Some(10), Some(1)));
    }

    #[test]
    fn test_get_line_column_none() {
        let table = make_table(&[(100, 10, None)]);
        assert_eq!(table.get_line(100, 100), (Some(10), None));
    }

    #[test]
    fn test_from_entries_preserves_order() {
        let table = make_table(&[(300, 30, None), (100, 10, None), (200, 20, None)]);
        assert_eq!(table.len(), 3);
        // from_entries doesn't sort — entries are used as-is
        // (build() sorts, but from_entries trusts the caller)
    }
}
