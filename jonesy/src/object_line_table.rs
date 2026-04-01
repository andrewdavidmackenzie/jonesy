use crate::project_context::ProjectContext;
use crate::sym::resolve_line_file_path;
use gimli::{ColumnType, Dwarf, Reader};

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
pub(crate) struct ObjectLineTable {
    entries: Vec<ObjectLineEntry>,
}

impl ObjectLineTable {
    /// Build a line table from DWARF debug info.
    pub(crate) fn build<R: Reader>(dwarf: &Dwarf<R>) -> Result<Self, gimli::Error> {
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
                    let full_path = resolve_line_file_path(dwarf, &unit, file_entry, header)?;
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
    pub(crate) fn lookup(
        &self,
        address: u64,
    ) -> Option<(Option<String>, Option<u32>, Option<u32>)> {
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
    pub(crate) fn get_crate_line_in_range(
        &self,
        func_start: u64,
        call_site_addr: u64,
        project_context: &ProjectContext,
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
            if project_context.is_crate_source(&entry.file) {
                return Some((entry.file.clone(), entry.line, entry.column));
            }
        }

        None
    }
}
