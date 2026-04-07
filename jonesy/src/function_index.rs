use crate::binary_format::BinaryRef;
use crate::string_tables::StringTables;
use gimli::{
    AttributeValue, DebuggingInformationEntry, Dwarf, EndianSlice, Reader, RunTimeEndian,
    SectionId, Unit,
};
use rayon::prelude::*;
use rustc_demangle::demangle;
use std::borrow::Cow;
use std::collections::HashMap;

type DwarfReader<'a> = EndianSlice<'a, RunTimeEndian>;

/// Result type for get_functions_from_dwarf: (functions, inlined, string_tables)
type DwarfFunctionResult = (Vec<FunctionInfo>, Vec<FunctionInfo>, StringTables);

/// Resolved specification data: (name, file, line)
type SpecificationResult = (Option<String>, Option<String>, Option<u32>);

/// Function info extracted from DWARF of the calling function.
/// Uses indices into StringTables for compact memory layout (32 bytes).
#[derive(Debug, Clone, Copy)]
pub struct FunctionInfo {
    /// Start address of the function
    pub start_address: u64,
    /// End address of the function
    pub end_address: u64,
    /// Index into StringTables.names
    pub name_idx: u32,
    /// Index into StringTables.files (0 = None, otherwise idx + 1)
    pub file_idx: u32,
    /// Line number (0 = None)
    pub line: u32,
}

/// Bucket size for spatial partitioning of inlined functions.
/// Using 64 bytes provides fine-grained partitioning with low per-bucket counts.
const INLINED_BUCKET_SHIFT: u32 = 12; // 2^12 = 4096 bytes (optimal per benchmarking)

/// Index for O(log n) function lookup by address.
/// Functions are sorted by start_address for binary search.
/// Inlined functions use bucketed lookup for faster searches.
/// Owns the StringTables for name/file lookups.
#[derive(Debug)]
pub struct FunctionIndex {
    /// Functions sorted by start_address (non-inlined subprograms)
    functions: Vec<FunctionInfo>,
    /// Inlined functions (stored for ownership)
    inlined: Vec<FunctionInfo>,
    /// Bucketed index for fast inlined function lookup.
    /// Maps bucket_id -> indices into `inlined` vec.
    /// Each function appears in all buckets its address range spans.
    inlined_buckets: HashMap<u64, Vec<usize>>,
    /// Interned strings for function names and file paths
    strings: StringTables,
}

impl FunctionIndex {
    /// Compute bucket ID for an address
    #[inline]
    fn bucket_id(addr: u64) -> u64 {
        addr >> INLINED_BUCKET_SHIFT
    }

    /// Build a function index from a list of functions.
    /// Sorts the functions by start_address for binary search.
    pub fn new(mut functions: Vec<FunctionInfo>, strings: StringTables) -> Self {
        functions.sort_by_key(|f| f.start_address);
        // Note: DWARF may have overlapping function ranges (e.g., from inlining).
        // We don't assert non-overlapping here because it's common in real binaries.
        // The binary search will find one valid function, which is sufficient.
        Self {
            functions,
            inlined: Vec::new(),
            inlined_buckets: HashMap::new(),
            strings,
        }
    }

    /// Build a function index with separate inlined function tracking.
    /// Uses bucketed spatial partitioning for fast inlined function lookup.
    pub fn new_with_inlined(
        mut functions: Vec<FunctionInfo>,
        inlined: Vec<FunctionInfo>,
        strings: StringTables,
    ) -> Self {
        functions.sort_by_key(|f| f.start_address);

        // Build bucket index for inlined functions
        // Each function is added to all buckets its address range spans
        let mut inlined_buckets: HashMap<u64, Vec<usize>> = HashMap::new();
        for (idx, func) in inlined.iter().enumerate() {
            let start_bucket = Self::bucket_id(func.start_address);
            let end_bucket = Self::bucket_id(func.end_address.saturating_sub(1));
            for bucket in start_bucket..=end_bucket {
                inlined_buckets.entry(bucket).or_default().push(idx);
            }
        }

        Self {
            functions,
            inlined,
            inlined_buckets,
            strings,
        }
    }

    /// Get the function name for a FunctionInfo
    #[inline]
    pub fn get_name(&self, func: &FunctionInfo) -> &str {
        self.strings.get_name(func.name_idx)
    }

    /// Get the file path for a FunctionInfo (None if not available)
    #[inline]
    pub fn get_file(&self, func: &FunctionInfo) -> Option<&str> {
        self.strings.get_file(func.file_idx)
    }

    /// Get the line number for a FunctionInfo (None if 0)
    #[inline]
    pub fn get_line(&self, func: &FunctionInfo) -> Option<u32> {
        if func.line == 0 {
            None
        } else {
            Some(func.line)
        }
    }

    /// Get a reference to the string tables
    pub fn strings(&self) -> &StringTables {
        &self.strings
    }

    /// Find the function containing the given address using binary search.
    /// Returns a reference to the function if found.
    /// Note: This returns the containing function, not inlined functions.
    /// Use `find_function_name` to get the most specific function name.
    #[inline]
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
    #[inline]
    pub fn find_function_name(&self, addr: u64) -> Option<&str> {
        // First check inlined functions (more specific)
        if let Some(inlined) = self.find_in_inlined(addr) {
            return Some(self.strings.get_name(inlined.name_idx));
        }
        // Fall back to containing function
        self.find_containing(addr)
            .map(|f| self.strings.get_name(f.name_idx))
    }

    /// Find an inlined function containing the given address.
    /// Uses bucketed lookup for O(k) where k is functions in the bucket,
    /// instead of O(n) scanning all inlined functions.
    #[inline]
    fn find_in_inlined(&self, addr: u64) -> Option<&FunctionInfo> {
        // Look up the bucket for this address
        let bucket = Self::bucket_id(addr);
        let indices = self.inlined_buckets.get(&bucket)?;

        // Search only functions in this bucket for the smallest containing range
        let mut best: Option<&FunctionInfo> = None;
        let mut best_size: u64 = u64::MAX;

        for &idx in indices {
            let func = &self.inlined[idx];
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

/// Intermediate struct for parsing - uses owned strings before interning
struct ParsedFunctionInfo {
    name: String,
    start_address: u64,
    end_address: u64,
    file: Option<String>,
    line: Option<u32>,
}

/// Load DWARF sections from binary
/// Load DWARF sections with ELF relocation support for .o files
/// This version uses gimli::RelocateReader to track which section each address belongs to
#[allow(dead_code)] // TODO: integrate into ELF .o file processing
pub(crate) fn load_dwarf_sections_with_relocations_elf<'a>(
    elf: &goblin::elf::Elf,
    buffer: &'a [u8],
    endian: RunTimeEndian,
) -> Result<
    Dwarf<gimli::RelocateReader<DwarfReader<'a>, crate::elf_relocations::RelocationMap>>,
    gimli::Error,
> {
    use gimli::RelocateReader;

    // Parse relocations for .debug_line section
    let debug_line_relocs =
        crate::elf_relocations::RelocationMap::parse_from_elf(elf, buffer, ".rela.debug_line");

    // Helper to find a DWARF section in the binary
    let find_section = |name: &str| -> Option<&'a [u8]> {
        let section_name = if name.starts_with('.') {
            Cow::Borrowed(name)
        } else {
            Cow::Owned(format!(".debug_{}", name))
        };

        for sh in &elf.section_headers {
            if let Some(sh_name) = elf.shdr_strtab.get_at(sh.sh_name) {
                if sh_name == section_name.as_ref() {
                    let offset = sh.sh_offset as usize;
                    let size = sh.sh_size as usize;
                    return buffer.get(offset..offset + size);
                }
            }
        }
        None
    };

    // Load each DWARF section, wrapping .debug_line with RelocateReader
    let load_section = |id: SectionId| -> Result<
        RelocateReader<DwarfReader<'a>, crate::elf_relocations::RelocationMap>,
        gimli::Error,
    > {
        let data = find_section(id.name()).unwrap_or(&[]);
        let slice = EndianSlice::new(data, endian);

        // Only use RelocateReader for .debug_line (where we need section tracking)
        if id == SectionId::DebugLine {
            Ok(RelocateReader::new(slice, debug_line_relocs.clone()))
        } else {
            // For other sections, use an empty relocation map
            Ok(RelocateReader::new(
                slice,
                crate::elf_relocations::RelocationMap::empty(),
            ))
        }
    };

    Dwarf::load(&load_section)
}

pub(crate) fn load_dwarf_sections<'a>(
    binary: &BinaryRef<'a>,
    buffer: &'a [u8],
) -> Result<Dwarf<DwarfReader<'a>>, gimli::Error> {
    let endian = match binary {
        BinaryRef::MachO(macho) => {
            if macho.little_endian {
                RunTimeEndian::Little
            } else {
                RunTimeEndian::Big
            }
        }
        BinaryRef::Elf(elf) => {
            if elf.little_endian {
                RunTimeEndian::Little
            } else {
                RunTimeEndian::Big
            }
        }
    };

    // Helper to find a DWARF section in the binary
    let find_section = |name: &str| -> Option<&'a [u8]> {
        let section_name = binary.dwarf_section_name(name);
        binary
            .find_section(buffer, &section_name)
            .map(|(_, data)| data)
    };

    // Load each DWARF section
    let load_section = |id: SectionId| -> Result<DwarfReader<'a>, gimli::Error> {
        let data = find_section(id.name()).unwrap_or(&[]);
        Ok(EndianSlice::new(data, endian))
    };

    Dwarf::load(&load_section)
}

/// Extract all functions from DWARF debug info.
/// Returns functions, inlined functions, and the shared string tables.
pub fn get_functions_from_dwarf<'a>(
    binary: &BinaryRef<'a>,
    buffer: &'a [u8],
    project_root: &str,
) -> Result<DwarfFunctionResult, Box<dyn std::error::Error>> {
    let dwarf = load_dwarf_sections(binary, buffer)?;

    // Collect all unit headers first (fast)
    let mut headers = Vec::new();
    let mut units_iter = dwarf.units();
    while let Some(header) = units_iter.next()? {
        headers.push(header);
    }

    // Process compilation units in parallel - collect with owned strings
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
                        if let Ok(Some(func)) =
                            parse_function_die(&dwarf, &unit, entry, project_root)
                        {
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

    // Combine results and intern strings
    let mut strings = StringTables::new();
    let mut functions = Vec::new();
    let mut inlined = Vec::new();

    for (funcs, inl) in results {
        for parsed in funcs {
            functions.push(FunctionInfo {
                start_address: parsed.start_address,
                end_address: parsed.end_address,
                name_idx: strings.intern_name(parsed.name),
                file_idx: strings.intern_file(parsed.file),
                line: parsed.line.unwrap_or(0),
            });
        }
        for parsed in inl {
            inlined.push(FunctionInfo {
                start_address: parsed.start_address,
                end_address: parsed.end_address,
                name_idx: strings.intern_name(parsed.name),
                file_idx: strings.intern_file(parsed.file),
                line: parsed.line.unwrap_or(0),
            });
        }
    }

    Ok((functions, inlined, strings))
}

/// Parse a DW_TAG_subprogram DIE into ParsedFunctionInfo
fn parse_function_die<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<R>,
    project_root: &str,
) -> Result<Option<ParsedFunctionInfo>, gimli::Error> {
    let mut name: Option<String> = None;
    let mut has_linkage_name = false;
    let mut low_pc: Option<u64> = None;
    let mut high_pc: Option<u64> = None;
    let mut high_pc_is_offset = false;
    let mut file: Option<String> = None;
    let mut line: Option<u32> = None;
    let mut specification: Option<gimli::UnitOffset<R::Offset>> = None;

    let mut attrs = entry.attrs();
    while let Some(attr) = attrs.next()? {
        match attr.name() {
            gimli::DW_AT_name => {
                // Only use DW_AT_name as fallback if no linkage name was found
                if !has_linkage_name {
                    if let Ok(s) = dwarf.attr_string(unit, attr.value()) {
                        name = Some(s.to_string_lossy()?.into_owned());
                    }
                }
            }
            gimli::DW_AT_linkage_name | gimli::DW_AT_MIPS_linkage_name => {
                // Prefer linkage name — contains full qualified path after demangling
                if let Ok(s) = dwarf.attr_string(unit, attr.value()) {
                    let mangled = s.to_string_lossy()?.into_owned();
                    let stripped = mangled.strip_prefix('_').unwrap_or(&mangled);
                    name = Some(format!("{:#}", demangle(stripped)));
                    has_linkage_name = true;
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
                if let AttributeValue::FileIndex(idx) = attr.value() {
                    file = resolve_decl_file(dwarf, unit, idx, project_root)?;
                }
            }
            gimli::DW_AT_decl_line => {
                if let AttributeValue::Udata(l) = attr.value() {
                    line = Some(l as u32);
                }
            }
            gimli::DW_AT_specification => {
                // Reference to the declaration DIE that has name/file/line
                if let AttributeValue::UnitRef(offset) = attr.value() {
                    specification = Some(offset);
                }
            }
            _ => {}
        }
    }

    // If we have a specification reference but missing name/file/line, resolve from it
    if let Some(spec_offset) = specification {
        if name.is_none() || file.is_none() || line.is_none() {
            let (spec_name, spec_file, spec_line) =
                resolve_specification(dwarf, unit, spec_offset, project_root)?;
            if name.is_none() {
                name = spec_name;
            }
            if file.is_none() {
                file = spec_file;
            }
            if line.is_none() {
                line = spec_line;
            }
        }
    }

    // Calculate actual high_pc if it was an offset
    let high_pc = match (low_pc, high_pc, high_pc_is_offset) {
        (Some(low), Some(high), true) => Some(low + high),
        (_, high, false) => high,
        _ => None,
    };

    match (name, low_pc, high_pc) {
        (Some(name), Some(low_pc), Some(high_pc)) => Ok(Some(ParsedFunctionInfo {
            name,
            start_address: low_pc,
            end_address: high_pc,
            file,
            line,
        })),
        _ => Ok(None),
    }
}

/// Parse a DW_TAG_inlined_subroutine DIE into ParsedFunctionInfo.
/// Inlined subroutines use DW_AT_abstract_origin to reference the original function.
/// Handles both DW_AT_low_pc/DW_AT_high_pc and DW_AT_ranges (DWARF 5).
fn parse_inlined_subroutine<R: Reader<Offset = usize>>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<R>,
) -> Result<Option<ParsedFunctionInfo>, gimli::Error> {
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
        (Some(name), Some(low_pc), Some(high_pc)) => Ok(Some(ParsedFunctionInfo {
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
                // Prefer linkage name — contains full qualified path after demangling
                if let Ok(s) = dwarf.attr_string(unit, attr.value()) {
                    let mangled = s.to_string_lossy()?.into_owned();
                    let stripped = mangled.strip_prefix('_').unwrap_or(&mangled);
                    name = Some(format!("{:#}", demangle(stripped)));
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

/// Resolve file path from DW_AT_decl_file attribute value.
/// Handles cases where directory may be absent or empty (basename-only entries).
/// Resolve a file path from a DWARF line program file entry.
/// Constructs the full path from directory + file name, and prepends comp_dir
/// for relative paths to produce an absolute path.
pub(crate) fn resolve_line_file_path<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    file_entry: &gimli::FileEntry<R, R::Offset>,
    header: &gimli::LineProgramHeader<R, R::Offset>,
    project_root: &str,
) -> Result<String, gimli::Error> {
    let file_name = dwarf
        .attr_string(unit, file_entry.path_name())?
        .to_string_lossy()?
        .into_owned();

    let full_path = if let Some(dir) = file_entry.directory(header) {
        let dir_str = dwarf
            .attr_string(unit, dir)?
            .to_string_lossy()?
            .into_owned();
        if dir_str.is_empty() {
            file_name
        } else {
            format!("{dir_str}/{file_name}")
        }
    } else {
        file_name
    };

    // Prepend comp_dir for relative paths to make them absolute
    if !full_path.starts_with('/') {
        if let Some(comp_dir) = &unit.comp_dir {
            let comp_dir_str = comp_dir.to_string_lossy()?;
            // Only use comp_dir if it looks like a valid directory path.
            // On some ELF binaries, comp_dir contains compiler info like
            // "clang LLVM (rustc version ...)" instead of an actual path.
            if !comp_dir_str.is_empty() && is_valid_directory_path(&comp_dir_str) {
                return Ok(format!("{comp_dir_str}/{full_path}"));
            }
        }

        // If comp_dir is missing or invalid, make the path absolute
        // using the project root
        return Ok(format!("{}/{}", project_root, full_path));
    }

    Ok(full_path)
}

/// Check if a string looks like a valid directory path.
/// Returns false for strings that appear to be compiler identifiers or other non-path data.
fn is_valid_directory_path(path: &str) -> bool {
    // Valid paths start with '/', '.', or '~'
    if path.starts_with('/') || path.starts_with('.') || path.starts_with('~') {
        return true;
    }

    // Reject strings that look like compiler identifiers
    // (e.g., "clang LLVM (rustc version ...)")
    if path.contains("LLVM")
        || path.contains("clang")
        || path.contains("rustc")
        || path.contains('(')
        || path.contains(')')
    {
        return false;
    }

    // Treat other cases as potentially valid relative paths
    true
}

fn resolve_decl_file<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    file_idx: u64,
    project_root: &str,
) -> Result<Option<String>, gimli::Error> {
    let Some(line_program) = &unit.line_program else {
        return Ok(None);
    };
    let Some(file_entry) = line_program.header().file(file_idx) else {
        return Ok(None);
    };
    Ok(Some(resolve_line_file_path(
        dwarf,
        unit,
        file_entry,
        line_program.header(),
        project_root,
    )?))
}

/// Resolve name, file, and line from a DW_AT_specification reference.
/// Used when a function definition references a separate declaration.
fn resolve_specification<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    offset: gimli::UnitOffset<R::Offset>,
    project_root: &str,
) -> Result<SpecificationResult, gimli::Error> {
    let entry = unit.entry(offset)?;
    let mut name: Option<String> = None;
    let mut file: Option<String> = None;
    let mut line: Option<u32> = None;

    let mut attrs = entry.attrs();
    while let Some(attr) = attrs.next()? {
        match attr.name() {
            gimli::DW_AT_linkage_name | gimli::DW_AT_MIPS_linkage_name => {
                // Prefer linkage name — contains full qualified path after demangling
                if let Ok(s) = dwarf.attr_string(unit, attr.value()) {
                    let mangled = s.to_string_lossy()?.into_owned();
                    let stripped = mangled.strip_prefix('_').unwrap_or(&mangled);
                    name = Some(format!("{:#}", demangle(stripped)));
                }
            }
            gimli::DW_AT_name => {
                if name.is_none() {
                    if let Ok(s) = dwarf.attr_string(unit, attr.value()) {
                        name = Some(s.to_string_lossy()?.into_owned());
                    }
                }
            }
            gimli::DW_AT_decl_file => {
                if let AttributeValue::FileIndex(idx) = attr.value() {
                    file = resolve_decl_file(dwarf, unit, idx, project_root)?;
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

    Ok((name, file, line))
}
