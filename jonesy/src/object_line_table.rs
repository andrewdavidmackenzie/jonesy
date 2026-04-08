use crate::project_context::ProjectContext;
use crate::sym::resolve_line_file_path;
use gimli::{AttributeValue, ColumnType, Dwarf, Reader};
use rustc_demangle::demangle;

/// Line entry for DWARF line table lookups
#[derive(Debug, Clone)]
struct ObjectLineEntry {
    address: u64,
    file: String,
    line: u32,
    column: Option<u32>,
    /// Function name - used to disambiguate overlapping section-relative addresses in .o files
    /// For ELF, multiple .text.* sections all start at 0x0, so we need function context
    function_name: Option<String>,
    /// Section name (e.g., ".text._ZN...") - captured from ELF relocations
    /// This is the definitive way to disambiguate addresses in .o files
    section_name: Option<String>,
}

/// Function address range for disambiguating line lookups in .o files
#[derive(Debug, Clone)]
pub struct FunctionRange {
    pub start: u64,
    pub end: u64,
}

/// Line table built from DWARF debug info for address -> file/line lookups.
/// Used for enriching LibraryCallGraph with source location info.
pub(crate) struct ObjectLineTable {
    entries: Vec<ObjectLineEntry>,
}

/// Extract address ranges from DWARF DIEs in a compilation unit, ignoring names
fn extract_cu_address_ranges<R: Reader>(
    _dwarf: &Dwarf<R>,
    unit: &gimli::Unit<R>,
) -> Vec<FunctionRange> {
    let mut ranges = Vec::new();

    let mut entries_iter = unit.entries();
    while let Some((_, entry)) = entries_iter.next_dfs().ok().flatten() {
        if entry.tag() != gimli::DW_TAG_subprogram {
            continue;
        }

        let mut low_pc: Option<u64> = None;
        let mut high_pc: Option<u64> = None;
        let mut high_pc_is_offset = false;

        let mut attrs = entry.attrs();
        while let Some(attr) = attrs.next().ok().flatten() {
            match attr.name() {
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
                _ => {}
            }
        }

        if let (Some(low), Some(high)) = (low_pc, high_pc) {
            let end_addr = if high_pc_is_offset { low + high } else { high };
            let range = FunctionRange {
                start: low,
                end: end_addr,
            };
            ranges.push(range);
        }
    }

    ranges
}

/// Match DWARF address ranges with section header function names
/// For ELF .o files, DIE names are corrupt but ranges are correct.
/// Section headers have correct names but overlapping addresses.
/// Match by comparing address ranges (specifically, end addresses which differ).
fn match_ranges_with_section_names(
    die_ranges: &[FunctionRange],
    section_map: &[(FunctionRange, String)],
) -> Vec<(FunctionRange, String)> {
    let mut matched = Vec::new();

    for die_range in die_ranges {
        // Find section header entry with matching end address
        // (Start addresses are all 0x0, but end addresses differ)
        if let Some((_sec_range, name)) = section_map.iter().find(|(sec_range, _)| {
            // Match if ranges are identical or very close (within 4 bytes for alignment)
            die_range.start == sec_range.start && die_range.end.abs_diff(sec_range.end) <= 4
        }) {
            matched.push((die_range.clone(), name.clone()));
        }
    }

    matched
}

/// Build function ranges for a single compilation unit
fn build_cu_function_ranges<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &gimli::Unit<R>,
) -> Vec<(FunctionRange, String)> {
    let mut ranges = Vec::new();

    let mut entries_iter = unit.entries();
    while let Some((_, entry)) = entries_iter.next_dfs().ok().flatten() {
        if entry.tag() != gimli::DW_TAG_subprogram {
            continue;
        }

        let mut linkage_name: Option<String> = None;
        let mut plain_name: Option<String> = None;
        let mut low_pc: Option<u64> = None;
        let mut high_pc: Option<u64> = None;
        let mut high_pc_is_offset = false;

        let mut attrs = entry.attrs();
        while let Some(attr) = attrs.next().ok().flatten() {
            match attr.name() {
                gimli::DW_AT_linkage_name | gimli::DW_AT_MIPS_linkage_name => {
                    if let Ok(s) = dwarf.attr_string(unit, attr.value()) {
                        if let Ok(mangled) = s.to_string_lossy() {
                            let demangled = format!("{:#}", demangle(&mangled));
                            // Validate - reject compiler identifier strings
                            if !demangled.contains("clang") && !demangled.contains("LLVM") {
                                linkage_name = Some(demangled);
                            }
                        }
                    }
                }
                gimli::DW_AT_name => {
                    // Fallback to plain name if linkage name is missing/corrupt
                    if let Ok(s) = dwarf.attr_string(unit, attr.value()) {
                        if let Ok(n) = s.to_string_lossy() {
                            let name_str = n.to_string();
                            // Validate - reject compiler identifier strings
                            if !name_str.contains("clang")
                                && !name_str.contains("LLVM")
                                && !name_str.contains("rustc")
                            {
                                plain_name = Some(name_str);
                            }
                        }
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
                _ => {}
            }
        }

        // Prefer linkage_name, fallback to plain_name
        let name = linkage_name.or(plain_name);

        if let (Some(name), Some(low), Some(high)) = (name, low_pc, high_pc) {
            let end_addr = if high_pc_is_offset { low + high } else { high };
            ranges.push((
                FunctionRange {
                    start: low,
                    end: end_addr,
                },
                name,
            ));
        }
    }

    ranges
}

impl ObjectLineTable {
    /// Build a line table from DWARF debug info.
    /// For ELF .o files, associates line entries with their containing function
    /// to disambiguate overlapping section-relative addresses.
    ///
    /// # Arguments
    /// * `function_map` - Maps address ranges to function names (from ELF section headers)
    pub(crate) fn build<R: Reader>(
        dwarf: &Dwarf<R>,
        project_root: &str,
        function_map: &[(FunctionRange, String)],
    ) -> Result<Self, gimli::Error> {
        let mut entries = Vec::new();

        let mut units = dwarf.units();
        while let Some(header) = units.next()? {
            let unit = dwarf.unit(header)?;

            // For ELF .o files: Extract address ranges from DWARF DIEs and match them
            // with section header names (which have correct names but overlapping global addresses).
            // Match by address range to associate each CU with its specific functions.
            let cu_function_ranges = if function_map.is_empty() {
                // No section headers provided (MacHO), use DIE ranges directly
                build_cu_function_ranges(dwarf, &unit)
            } else {
                // ELF: Get DIE ranges and match with section headers by address
                let die_ranges = extract_cu_address_ranges(dwarf, &unit);
                match_ranges_with_section_names(&die_ranges, function_map)
            };

            if let Some(program) = &unit.line_program {
                // Build file path lookup from line program header
                let header = program.header();
                let mut file_paths: Vec<String> = Vec::new();

                // Index 0 is reserved, start from index 1
                file_paths.push(String::new()); // placeholder for index 0

                for file_entry in header.file_names() {
                    let full_path =
                        resolve_line_file_path(dwarf, &unit, file_entry, header, project_root)?;
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

                            // Determine which function(s) this line entry belongs to
                            // In ELF .o files with per-function sections, multiple functions may have
                            // overlapping address ranges (all starting at 0x0). We can't disambiguate
                            // which section an address belongs to from the line program alone.
                            // Solution: store one entry per matching function so lookup can find the right one.
                            let address = row.address();

                            let matching_functions: Vec<_> = cu_function_ranges
                                .iter()
                                .filter(|(range, _)| address >= range.start && address < range.end)
                                .map(|(_, name)| name.clone())
                                .collect();

                            // Capture section name from relocations (if available)
                            // This is set by the Relocate implementation when it processes the address
                            let section_name = crate::elf_relocations::get_current_section();

                            // Store one entry for each matching function
                            // This allows lookup_for_function to find the entry with the correct function name
                            for function_name in matching_functions {
                                entries.push(ObjectLineEntry {
                                    address,
                                    file: file_paths[file_idx].clone(),
                                    line: line.get() as u32,
                                    column,
                                    function_name: Some(function_name),
                                    section_name: section_name.clone(),
                                });
                            }

                            // If no function matched, store without function name (for linked binaries)
                            if cu_function_ranges.is_empty() {
                                entries.push(ObjectLineEntry {
                                    address,
                                    file: file_paths[file_idx].clone(),
                                    line: line.get() as u32,
                                    column,
                                    function_name: None,
                                    section_name: section_name.clone(),
                                });
                            }
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

    /// Look up file/line/column for an address, constrained to a specific function/section.
    /// This is critical for ELF .o files where section-relative addresses overlap between functions.
    ///
    /// # Arguments
    /// * `address` - The address to look up
    /// * `function_name` - Function name (for matching, but section_name takes precedence)
    /// * `section_name` - Optional section name (e.g., ".text._ZN...") for precise matching
    pub(crate) fn lookup_for_function_and_section(
        &self,
        address: u64,
        function_name: &str,
        section_name: Option<&str>,
    ) -> Option<(Option<String>, Option<u32>, Option<u32>)> {
        if self.entries.is_empty() {
            return None;
        }

        // Extract the plain function name (last component after ::)
        let plain_name = function_name.rsplit("::").next().unwrap_or(function_name);

        // Find entries matching this address
        let candidates: Vec<&ObjectLineEntry> = self
            .entries
            .iter()
            .filter(|e| {
                // Must be at or before the target address
                if e.address > address {
                    return false;
                }

                // If we have a section name, filter by it (most precise)
                if let Some(section) = section_name {
                    if let Some(entry_section) = &e.section_name {
                        return entry_section == section;
                    }
                    // If we're filtering by section but entry has no section, skip it
                    return false;
                }

                // Otherwise fall back to function name matching
                e.function_name
                    .as_ref()
                    .is_some_and(|fn_name| fn_name == function_name || fn_name == plain_name)
            })
            .collect();

        if candidates.is_empty() {
            return None;
        }

        // Find the entry with the largest address <= target address
        let entry = candidates.iter().max_by_key(|e| e.address)?;

        Some((Some(entry.file.clone()), Some(entry.line), entry.column))
    }

    /// Look up file/line/column for an address, constrained to a specific function.
    /// This is critical for ELF .o files where section-relative addresses overlap between functions.
    pub(crate) fn lookup_for_function(
        &self,
        address: u64,
        function_name: &str,
    ) -> Option<(Option<String>, Option<u32>, Option<u32>)> {
        // Call the section-aware version without a section filter
        self.lookup_for_function_and_section(address, function_name, None)
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

    /// Find the last crate source line entry for a specific function/section up to call_site_addr.
    /// This works for .o files with section-relative addresses (where func_addr may be 0).
    ///
    /// # Arguments
    /// * `call_site_addr` - The call site address (section-relative for .o files)
    /// * `function_name` - Function name for matching
    /// * `section_name` - Optional section name for precise matching
    /// * `project_context` - Project context to identify crate source files
    pub(crate) fn get_crate_line_for_function_and_section(
        &self,
        call_site_addr: u64,
        function_name: &str,
        section_name: Option<&str>,
        project_context: &ProjectContext,
    ) -> Option<(String, u32, Option<u32>)> {
        if self.entries.is_empty() {
            return None;
        }

        // Extract the plain function name (last component after ::)
        let plain_name = function_name.rsplit("::").next().unwrap_or(function_name);

        // Find all entries for this function/section up to call_site_addr
        let candidates: Vec<&ObjectLineEntry> = self
            .entries
            .iter()
            .filter(|e| {
                e.address <= call_site_addr
                    && e.function_name
                        .as_ref()
                        .is_some_and(|f| f.contains(plain_name) || f.contains(function_name))
                    && (section_name.is_none()
                        || e.section_name.as_deref() == section_name
                        || e.section_name
                            .as_ref()
                            .is_some_and(|s| section_name.is_some_and(|sn| s.contains(sn))))
            })
            .collect();

        // Search backwards through candidates to find the last crate source entry
        for entry in candidates.iter().rev() {
            if project_context.is_crate_source(&entry.file) {
                return Some((entry.file.clone(), entry.line, entry.column));
            }
        }

        None
    }
}
