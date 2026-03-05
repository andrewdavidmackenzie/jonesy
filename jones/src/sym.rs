#![allow(unused_variables)] // TODO Just for now
#![allow(dead_code)] // TODO Just for now

use capstone::arch::BuildsCapstone;
use capstone::{arch, Capstone};
/// Here's how to use gimli with a MachO binary to get function information and then find call sites
/// Note that DWARF doesn't directly encode "function A calls
/// function B" - it provides accurate function boundaries and source locations, which you combine with disassembly.
use gimli::{
    AttributeValue, DebuggingInformationEntry, Dwarf, EndianSlice, Reader, RunTimeEndian,
    SectionId, Unit,
};
use goblin::mach::segment::SectionData;
use goblin::mach::segment::{Section, Segment};
use goblin::mach::{Mach, MachO};
use ouroboros::self_referencing;
use regex::Regex;
use rustc_demangle::demangle;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

type DwarfReader<'a> = EndianSlice<'a, RunTimeEndian>;

pub enum SymbolTable<'a> {
    MachO(Mach<'a>),
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

/// Information about a call site
#[derive(Debug, Clone, Default)]
pub struct CallerInfo {
    pub caller: FunctionInfo,
    /// Address of the calling instruction
    pub call_site_addr: u64,
    /// Source location of the calling instruction
    pub file: Option<String>,
    /// Line number of the calling instruction
    pub line: Option<u32>,
}

/// Self-referencing struct that owns the buffer and the parsed MachO that borrows from it
#[self_referencing]
pub struct DSymInfo {
    pub debug_buffer: Vec<u8>,
    #[borrows(debug_buffer)]
    #[covariant]
    pub debug_macho: Mach<'this>,
}

/// Debug info source - either embedded in binary or from a separate dSYM file/bundle
pub enum DebugInfo {
    /// Debug info is embedded in the binary
    Embedded,
    /// Debug info is in a separate dSYM bundle
    DSym(Box<DSymInfo>),
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

pub(crate) fn read_symbols(buffer: &'_ [u8]) -> io::Result<SymbolTable<'_>> {
    Ok(SymbolTable::MachO(Mach::parse(buffer).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidData, e)
    })?))
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
pub(crate) fn find_symbol_containing(
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
/// Returns the first symbol found whose name matches `name` exactly, plus the address it is at
pub(crate) fn find_symbol_address(macho: &MachO, name: &str) -> Option<(String, u64)> {
    let symbols = macho.symbols.as_ref()?;
    for symbol in symbols.iter() {
        if let Ok((sym_name, nlist)) = symbol
            && sym_name == name
        {
            return Some((sym_name.to_string(), nlist.n_value));
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

/// Returns (function_start_address, demangled_name) for the function containing `addr`
pub(crate) fn find_containing_function_with_addr(
    macho: &MachO,
    addr: u64,
) -> Option<(u64, String)> {
    let symbols = macho.symbols.as_ref()?;

    // Collect function symbols with their addresses
    // Filter out empty names - goblin may return duplicate entries with empty names
    let mut functions: Vec<(u64, &str)> = symbols
        .iter()
        .filter_map(|s| s.ok())
        .filter(|(name, nlist)| nlist.n_value > 0 && !name.is_empty())
        .map(|(name, nlist)| (nlist.n_value, name))
        .collect();

    functions.sort_by_key(|(a, _)| *a);

    // Find the function that contains this address
    let mut containing: Option<(u64, &str)> = None;
    for (func_addr, name) in &functions {
        if *func_addr <= addr {
            containing = Some((*func_addr, *name));
        } else {
            break;
        }
    }

    containing.map(|(func_addr, name)| {
        let stripped = name.strip_prefix("_").unwrap_or(name);
        (func_addr, format!("{:#}", demangle(stripped)))
    })
}

pub(crate) fn find_containing_function(macho: &MachO, addr: u64) -> Option<String> {
    find_containing_function_with_addr(macho, addr).map(|(_, name)| name)
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

// TODO Note that the address passed in is an n_value or Symbol table offset,
// which is not necessarily the same as the address of the symbol in memory.
// How can we fix that?
// TODO using [cfg] have implementations for other architectures
pub(crate) fn find_callers(
    macho: &MachO,
    buffer: &[u8],
    target_addr: u64,
) -> Result<Vec<CallerInfo>, Box<dyn std::error::Error>> {
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

    for instruction in instructions.iter() {
        // TODO is "bl" the only valid instruction for ARM64?
        if instruction.mnemonic() == Some("bl")
            && let Some(operand) = instruction.op_str()
        {
            let addr_str = operand.trim_start_matches("#0x");
            if let Ok(call_target) = u64::from_str_radix(addr_str, 16)
                && call_target == target_addr
                && let Some((func_addr, func_name)) =
                    find_containing_function_with_addr(macho, instruction.address())
            {
                callers.push(CallerInfo {
                    caller: FunctionInfo {
                        name: func_name.clone(),
                        start_address: func_addr,
                        ..Default::default()
                    },
                    call_site_addr: instruction.address(),
                    ..Default::default()
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

/// Extract all functions from DWARF debug info
pub fn get_functions_from_dwarf<'a>(
    macho: &'a MachO,
    buffer: &'a [u8],
) -> Result<Vec<FunctionInfo>, Box<dyn std::error::Error>> {
    let dwarf = load_dwarf_sections(macho, buffer)?;
    let mut functions = Vec::new();

    // Iterate through all compilation units
    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;
        let mut entries = unit.entries();

        while let Some((_, entry)) = entries.next_dfs()? {
            // Look for function DIEs (DW_TAG_subprogram)
            if entry.tag() == gimli::DW_TAG_subprogram
                && let Some(func) = parse_function_die(&dwarf, &unit, entry)?
            {
                functions.push(func.clone());
            }
        }
    }

    Ok(functions)
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

/// Find which function contains a given address
pub fn find_function_at_address(functions: &[FunctionInfo], addr: u64) -> Option<&FunctionInfo> {
    functions
        .iter()
        .find(|f| addr >= f.start_address && addr < f.end_address)
}

/// Build address-to-function lookup for efficient queries
pub fn build_function_lookup(functions: &[FunctionInfo]) -> HashMap<u64, &FunctionInfo> {
    // For quick lookups, you might want an interval tree in production
    // This simple version just maps low_pc to function
    functions.iter().map(|f| (f.start_address, f)).collect()
}
/// Find all functions that call a target address, with source info
///
/// # Arguments
/// * `binary_macho` - Parsed MachO from the executable binary (contains __text section)
/// * `binary_buffer` - Raw bytes of the executable binary
/// * `debug_macho` - Parsed MachO containing DWARF info (can be same as binary_macho, or from dSYM)
/// * `debug_buffer` - Raw bytes containing DWARF info (can be same as binary_buffer, or from dSYM)
/// * `target_addr` - Address of the function to find callers for
pub fn find_callers_with_debug_info(
    binary_macho: &MachO,
    binary_buffer: &[u8],
    debug_macho: &MachO,
    debug_buffer: &[u8],
    target_addr: u64,
) -> Result<Vec<CallerInfo>, Box<dyn std::error::Error>> {
    // Get function info and DWARF from debug info (dSYM or embedded)
    let functions = get_functions_from_dwarf(debug_macho, debug_buffer)?;
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

    for instruction in instructions.iter() {
        // Look for BL (branch with link) instructions
        if instruction.mnemonic() == Some("bl")
            && let Some(operand) = instruction.op_str()
        {
            let addr_str = operand.trim_start_matches("#0x");
            if let Ok(call_target) = u64::from_str_radix(addr_str, 16)
                && call_target == target_addr
            {
                // Find the function containing this call using DWARF info
                if let Some(func) = find_function_at_address(&functions, instruction.address()) {
                    // Get source line info for this specific address
                    let (file, line) = get_source_location(&dwarf, instruction.address())?;

                    callers.push(CallerInfo {
                        caller: func.clone(),
                        call_site_addr: instruction.address(),
                        file,
                        line,
                    });
                }
            }
        }
    }

    Ok(callers)
}

/// Get source file and line for an address using DWARF line info
fn get_source_location<R: Reader>(
    dwarf: &Dwarf<R>,
    addr: u64,
) -> Result<(Option<String>, Option<u32>), gimli::Error> {
    let mut units = dwarf.units();

    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;

        if let Some(program) = &unit.line_program {
            let mut rows = program.clone().rows();
            let mut prev_row: Option<(String, u32)> = None;

            while let Some((header, row)) = rows.next_row()? {
                if row.address() > addr {
                    // The previous row covers this address
                    if let Some((file, line)) = prev_row {
                        return Ok((Some(file), Some(line)));
                    }
                }

                if let Some(file_entry) = row.file(header) {
                    let file_name = dwarf
                        .attr_string(&unit, file_entry.path_name())?
                        .to_string_lossy()?
                        .into_owned();
                    prev_row = Some((file_name, row.line().map(|l| l.get() as u32).unwrap_or(0)));
                }
            }
        }
    }

    Ok((None, None))
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

/// More detailed check - returns which debug sections are present
pub fn get_dwarf_sections(macho: &MachO) -> Vec<String> {
    let mut sections = Vec::new();

    for segment in macho.segments.iter() {
        if let Ok(sects) = segment.sections() {
            for (section, _) in sects {
                if let Ok(name) = section.name()
                    && name.starts_with("__debug_")
                {
                    sections.push(name.to_string());
                }
            }
        }
    }
    sections
}

// 1) No embedded debug info, no dSYM
// 2) No embedded debug info, dSYM
// 3) Embedded debug info, no dSYM
// 4) Embedded debug info, dSYM
pub fn load_debug_info(macho: &MachO, binary_path: &Path) -> DebugInfo {
    // Look for dSYM symbol directory
    let binary_name = binary_path.file_stem().unwrap().to_str().unwrap();
    let dsym_dir_path = binary_path
        .with_extension("dSYM")
        .join("Contents/Resources/DWARF")
        .join(binary_name);
    if dsym_dir_path.exists() {
        println!("Using .dSYM bundle for debug info");
        let debug_buffer = fs::read(dsym_dir_path).unwrap();
        let dsym_info = DSymInfoBuilder {
            debug_buffer,
            debug_macho_builder: |buf: &Vec<u8>| Mach::parse(buf).unwrap(),
        }
        .build();
        return DebugInfo::DSym(Box::new(dsym_info));
    }

    if !get_dwarf_sections(macho).is_empty() {
        println!("Using embedded DWARF debugging info");
        return DebugInfo::Embedded;
    }

    println!("No Embedded or dSYM bundle DWARF info found");
    DebugInfo::None
}
