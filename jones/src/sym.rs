#![allow(unused_variables)] // TODO Just for now
#![allow(dead_code)] // TODO Just for now

use capstone::arch::BuildsCapstone;
use capstone::{Capstone, Insn, arch};
use dashmap::DashMap;
/// Here's how to use gimli with a MachO binary to get function information and then find call sites
/// Note that DWARF doesn't directly encode "function A calls
/// function B" - it provides accurate function boundaries and source locations, which you combine with disassembly.
use gimli::{
    AttributeValue, DebuggingInformationEntry, Dwarf, EndianSlice, Reader, RunTimeEndian,
    SectionId, Unit,
};
use goblin::Object;
use goblin::archive::Archive;
use goblin::mach::segment::SectionData;
use goblin::mach::segment::{Section, Segment};
use goblin::mach::symbols::N_OSO;
use goblin::mach::{Mach, MachO};
use ouroboros::self_referencing;
use rayon::prelude::*;
use regex::Regex;
use rustc_demangle::demangle;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

type DwarfReader<'a> = EndianSlice<'a, RunTimeEndian>;

#[allow(clippy::large_enum_variant)]
pub enum SymbolTable<'a> {
    MachO(Mach<'a>),
    Archive(Archive<'a>),
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

pub(crate) fn read_symbols(buffer: &'_ [u8]) -> io::Result<SymbolTable<'_>> {
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

/// Pre-computed call graph mapping target addresses to their callers.
/// This allows O(1) lookup instead of O(n) scanning for each query.
pub struct CallGraph {
    /// Maps target_addr -> list of CallerInfo
    edges: HashMap<u64, Vec<CallerInfo>>,
}

/// Extracted instruction data for parallel processing (avoids Insn lifetime issues)
struct InsnData {
    address: u64,
    is_bl: bool,
    call_target: Option<u64>,
}

impl CallGraph {
    /// Build a call graph by scanning all instructions once (no debug info).
    /// Uses parallel processing for faster analysis of large binaries.
    pub fn build(macho: &MachO, buffer: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        let Some((text_addr, text_data)) = get_text_section(macho, buffer) else {
            return Ok(Self {
                edges: HashMap::new(),
            });
        };

        let cs = Capstone::new()
            .arm64()
            .mode(arch::arm64::ArchMode::Arm)
            .build()?;

        let Ok(instructions) = cs.disasm_all(text_data, text_addr) else {
            return Ok(Self {
                edges: HashMap::new(),
            });
        };

        // Extract instruction data (sequential, fast)
        let insn_data: Vec<InsnData> = instructions
            .iter()
            .filter_map(|insn| {
                if insn.mnemonic() == Some("bl") {
                    let operand = insn.op_str()?;
                    let addr_str = operand.trim_start_matches("#0x");
                    let call_target = u64::from_str_radix(addr_str, 16).ok();
                    Some(InsnData {
                        address: insn.address(),
                        is_bl: true,
                        call_target,
                    })
                } else {
                    None
                }
            })
            .collect();

        // Process bl instructions in parallel (the expensive part is function lookup)
        let edges: DashMap<u64, Vec<CallerInfo>> = DashMap::new();

        insn_data.par_iter().for_each(|data| {
            if let Some(call_target) = data.call_target {
                if let Some((func_addr, func_name)) =
                    find_containing_function_with_addr(macho, data.address)
                {
                    edges.entry(call_target).or_default().push(CallerInfo {
                        caller: FunctionInfo {
                            name: func_name,
                            start_address: func_addr,
                            ..Default::default()
                        },
                        call_site_addr: data.address,
                        ..Default::default()
                    });
                }
            }
        });

        // Convert DashMap to HashMap
        Ok(Self {
            edges: edges.into_iter().collect(),
        })
    }

    /// Build a call graph by scanning all instructions once (no debug info).
    /// Non-parallel version for comparison or single-threaded mode.
    #[allow(dead_code)]
    pub fn build_sequential(
        macho: &MachO,
        buffer: &[u8],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut edges: HashMap<u64, Vec<CallerInfo>> = HashMap::new();

        let Some((text_addr, text_data)) = get_text_section(macho, buffer) else {
            return Ok(Self { edges });
        };

        let cs = Capstone::new()
            .arm64()
            .mode(arch::arm64::ArchMode::Arm)
            .build()?;

        let Ok(instructions) = cs.disasm_all(text_data, text_addr) else {
            return Ok(Self { edges });
        };

        for instruction in instructions.iter() {
            if let Some((call_target, caller_info)) = process_instruction_basic(macho, &instruction)
            {
                edges.entry(call_target).or_default().push(caller_info);
            }
        }

        Ok(Self { edges })
    }

    /// Build a call graph with debug info enrichment.
    /// Uses parallel processing for faster analysis of large binaries.
    pub fn build_with_debug_info(
        binary_macho: &MachO,
        binary_buffer: &[u8],
        debug_macho: &MachO,
        debug_buffer: &[u8],
        crate_src_path: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // Pre-load DWARF info once (shared across threads)
        let functions = get_functions_from_dwarf(debug_macho, debug_buffer)?;
        let dwarf = load_dwarf_sections(debug_macho, debug_buffer)?;

        let Some((text_addr, text_data)) = get_text_section(binary_macho, binary_buffer) else {
            return Ok(Self {
                edges: HashMap::new(),
            });
        };

        let cs = Capstone::new()
            .arm64()
            .mode(arch::arm64::ArchMode::Arm)
            .build()?;

        let Ok(instructions) = cs.disasm_all(text_data, text_addr) else {
            return Ok(Self {
                edges: HashMap::new(),
            });
        };

        // Extract instruction data (sequential, fast)
        let insn_data: Vec<InsnData> = instructions
            .iter()
            .filter_map(|insn| {
                if insn.mnemonic() == Some("bl") {
                    let operand = insn.op_str()?;
                    let addr_str = operand.trim_start_matches("#0x");
                    let call_target = u64::from_str_radix(addr_str, 16).ok();
                    Some(InsnData {
                        address: insn.address(),
                        is_bl: true,
                        call_target,
                    })
                } else {
                    None
                }
            })
            .collect();

        // Process bl instructions in parallel
        let edges: DashMap<u64, Vec<CallerInfo>> = DashMap::new();

        insn_data.par_iter().for_each(|data| {
            if let Some(call_target) = data.call_target {
                if let Some((target, caller_info)) =
                    process_instruction_data_with_debug(data, &functions, &dwarf, crate_src_path)
                {
                    edges.entry(target).or_default().push(caller_info);
                }
            }
        });

        Ok(Self {
            edges: edges.into_iter().collect(),
        })
    }

    /// Get all callers of a target address.
    pub fn get_callers(&self, target_addr: u64) -> Vec<CallerInfo> {
        self.edges.get(&target_addr).cloned().unwrap_or_default()
    }

    /// Create an empty call graph.
    pub fn empty() -> Self {
        Self {
            edges: HashMap::new(),
        }
    }
}

/// Process a single instruction and extract call information (basic version without debug info).
/// Returns (call_target, CallerInfo) if this is a bl instruction, None otherwise.
fn process_instruction_basic(macho: &MachO, instruction: &Insn) -> Option<(u64, CallerInfo)> {
    if instruction.mnemonic() != Some("bl") {
        return None;
    }

    let operand = instruction.op_str()?;
    let addr_str = operand.trim_start_matches("#0x");
    let call_target = u64::from_str_radix(addr_str, 16).ok()?;
    let (func_addr, func_name) = find_containing_function_with_addr(macho, instruction.address())?;

    Some((
        call_target,
        CallerInfo {
            caller: FunctionInfo {
                name: func_name,
                start_address: func_addr,
                ..Default::default()
            },
            call_site_addr: instruction.address(),
            ..Default::default()
        },
    ))
}

/// Process extracted instruction data and enrich with debug info.
/// Returns (call_target, CallerInfo) if successful, None otherwise.
fn process_instruction_data_with_debug(
    data: &InsnData,
    functions: &[FunctionInfo],
    dwarf: &Dwarf<DwarfReader>,
    crate_src_path: Option<&str>,
) -> Option<(u64, CallerInfo)> {
    let call_target = data.call_target?;

    // Find the function containing this call using DWARF info
    let func = find_function_at_address(functions, data.address)?;

    // Get source info
    let (func_file, func_line) =
        get_source_location(dwarf, func.start_address).unwrap_or((None, None));

    let file = func.file.clone().or(func_file);
    let mut line = func.line.or(func_line);

    // For functions in the crate source, find actual call line
    if let (Some(f), Some(crate_path)) = (&file, crate_src_path)
        && f.contains(crate_path)
        && let Ok(Some(crate_line)) =
            get_crate_line_at_address(dwarf, func.start_address, data.address, crate_path)
    {
        line = Some(crate_line);
    }

    Some((
        call_target,
        CallerInfo {
            caller: func.clone(),
            call_site_addr: data.address,
            file,
            line,
        },
    ))
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
/// * `crate_src_path` - Optional crate source path for precise line numbers in user code
pub fn find_callers_with_debug_info(
    binary_macho: &MachO,
    binary_buffer: &[u8],
    debug_macho: &MachO,
    debug_buffer: &[u8],
    target_addr: u64,
    crate_src_path: Option<&str>,
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
                    // Get source info from function's start address to avoid inlined code locations
                    let (func_file, func_line) = get_source_location(&dwarf, func.start_address)?;

                    // Prefer function's declaration file/line if available, then function start's line info
                    let file = func.file.clone().or(func_file);
                    let mut line = func.line.or(func_line);

                    // For functions in the crate source, find the actual line where the call originates
                    if let (Some(f), Some(crate_path)) = (&file, crate_src_path)
                        && f.contains(crate_path)
                        && let Ok(Some(crate_line)) = get_crate_line_at_address(
                            &dwarf,
                            func.start_address,
                            instruction.address(),
                            crate_path,
                        )
                    {
                        line = Some(crate_line);
                    }

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

                    prev_row = Some((full_path, row.line().map(|l| l.get() as u32).unwrap_or(0)));
                }
            }
        }
    }

    Ok((None, None))
}

/// Find the user code line closest to a specific address within a function.
/// This finds the last line entry in user code before the given call site address.
fn get_crate_line_at_address<R: Reader>(
    dwarf: &Dwarf<R>,
    func_start: u64,
    call_site_addr: u64,
    crate_src_path: &str,
) -> Result<Option<u32>, gimli::Error> {
    let mut units = dwarf.units();
    let mut best_line: Option<u32> = None;
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
                    if full_path.contains(crate_src_path)
                        && let Some(line) = row.line()
                        && addr >= best_addr
                    {
                        best_addr = addr;
                        best_line = Some(line.get() as u32);
                    }
                }
            }
        }
    }

    Ok(best_line)
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
) -> Result<Vec<CallerInfo>, Box<dyn std::error::Error>> {
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
        let Ok(functions) = get_functions_from_dwarf(&obj_macho, &obj_info.buffer) else {
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
        if let Some((file, line)) = func_source_map.get(&caller.caller.name) {
            caller.caller.file = file.clone();
            caller.caller.line = *line;
            caller.file = file.clone();
            caller.line = *line;
        }
    }

    Ok(callers)
}

/// Load debug map from the binary's symbol table
/// This reads OSO entries and loads DWARF from the referenced object files
fn load_debug_map(macho: &MachO) -> Option<DebugMapInfo> {
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
        if get_dwarf_sections(&obj_macho).is_empty() {
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

    println!(
        "Using debug map: loaded {} object files with DWARF",
        loaded_count
    );

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
pub fn load_debug_info(macho: &MachO, binary_path: &Path) -> DebugInfo {
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
                println!("dSYM is stale, will regenerate");
            } else {
                println!("Using .dSYM bundle for debug info");
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

    if !get_dwarf_sections(macho).is_empty() {
        println!("Using embedded DWARF debugging info");
        return DebugInfo::Embedded;
    }

    // Try to auto-generate dSYM using dsymutil
    if let Some(dsym_info) = auto_generate_dsym(binary_path) {
        return DebugInfo::DSym(Box::new(dsym_info));
    }

    // Fall back to debug map (reading DWARF from object files)
    if let Some(debug_map) = load_debug_map(macho) {
        return DebugInfo::DebugMap(Box::new(debug_map));
    }

    println!("No debug info found (no dSYM, embedded DWARF, or debug map)");
    println!(
        "Tip: Install dsymutil or run 'dsymutil {}' to generate debug symbols",
        binary_path.display()
    );
    DebugInfo::None
}

/// Auto-generate dSYM by running dsymutil
fn auto_generate_dsym(binary_path: &Path) -> Option<DSymInfo> {
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
            println!("Generated .dSYM bundle for debug info");
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
