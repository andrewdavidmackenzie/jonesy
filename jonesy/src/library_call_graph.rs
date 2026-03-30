use crate::call_graph::CallerInfo;
use crate::object_line_table::ObjectLineTable;
use crate::project_context::ProjectContext;
use crate::sym::SymbolIndex;
use goblin::container::{Container, Ctx, Endian};
use goblin::mach::MachO;
use regex::Regex;
use rustc_demangle::demangle;
use std::borrow::Cow;
use std::collections::HashMap;

/// ARM64 relocation type for BL/B instructions (branch with 26-bit offset)
const ARM64_RELOC_BRANCH26: u8 = 2;

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
        project_context: &ProjectContext,
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
        let dwarf = crate::function_index::load_dwarf_sections(macho, buffer).ok();
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

                        // If call site points to non-crate code (stdlib/dependency), find
                        // the last crate source line between function start and call site
                        let (file, line, column) = if let Some(_crate_path) = crate_src_path
                            && file
                                .as_ref()
                                .is_some_and(|f| !project_context.is_crate_source(f))
                        {
                            // Try to find precise line in crate source
                            if let Some(lt) = line_lookup.as_ref()
                                && let Some((crate_file, crate_line, crate_col)) = lt
                                    .get_crate_line_in_range(
                                        func_addr,
                                        call_site_addr,
                                        project_context,
                                    )
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
            .map(|(name, callers): (&String, &Vec<CallerInfo<'static>>)| {
                (name.as_str(), callers.as_slice())
            })
            .collect()
    }

    /// Get all target symbol names in the call graph.
    pub fn target_symbols(&self) -> impl Iterator<Item = &str> {
        self.edges.keys().map(|s: &String| s.as_str())
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
