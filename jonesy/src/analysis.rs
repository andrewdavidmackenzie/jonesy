//! Binary and archive analysis functions.
//!
//! This module provides the core analysis functions for finding panic paths
//! in Mach-O binaries and library archives.

use crate::args::OutputFormat;
use crate::call_tree::{
    AnalysisSummary, CallTreeNode, CrateCodePoint, build_call_tree_parallel,
    collect_crate_code_points, prune_call_tree,
};
use crate::cargo::find_project_root;
use crate::config::Config;
use crate::sym::{
    CallGraph, DebugInfo, LibraryCallGraph, SymbolIndex, ValidSourceFiles, find_symbol_address,
    find_symbol_containing, load_debug_info, matches_crate_pattern_validated,
};
use dashmap::DashSet;
use goblin::mach::Mach::Binary;
use goblin::mach::MachO;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashSet;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// Panic symbol patterns to search for, in order of preference.
/// For binaries, rust_panic$ is the root. For libraries, we look for
/// the functions that call into the panic runtime.
pub const PANIC_SYMBOL_PATTERNS: &[&str] = &[
    "rust_panic$",        // Main panic entry point (binaries)
    "panic_fmt$",         // Core panic formatting (libraries)
    "panic_display",      // Panic display helper
    "slice_index_fail",   // Slice indexing panics (vec[i] where i >= len)
    "str_index_overflow", // String slice boundary panics
];

/// Panic symbol name patterns to search for in library call graphs.
/// These are the demangled names of functions that indicate panic.
pub const LIBRARY_PANIC_PATTERNS: &[&str] = &[
    // Direct panic functions
    "core::panicking::panic",
    "core::panicking::panic_fmt",
    "core::panicking::panic_display",
    "core::panicking::panic_in_cleanup",
    "core::panicking::panic_const",
    "core::panicking::panic_bounds_check",
    "core::panicking::panic_nounwind_fmt",
    "core::panicking::panic_cannot_unwind",
    "core::panicking::assert_failed",
    "std::panicking::begin_panic",
    "std::panicking::begin_panic_fmt",
    // Option panic functions
    "core::option::Option<T>::unwrap",
    "core::option::Option<T>::expect",
    "core::option::unwrap_failed",
    // Result panic functions
    "core::result::Result<T,E>::unwrap",
    "core::result::Result<T,E>::expect",
    "core::result::Result<T,E>::unwrap_err",
    "core::result::Result<T,E>::expect_err",
    "core::result::unwrap_failed",
];

/// Create a spinner for long-running operations.
/// Returns None if progress display is disabled or stderr is not a terminal.
pub fn create_spinner(show_progress: bool, message: &str) -> Option<ProgressBar> {
    if !show_progress || !io::stderr().is_terminal() {
        return None;
    }
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("  {spinner:.cyan} {msg} [{elapsed}]")
            .expect("valid template"),
    );
    spinner.set_message(message.to_string());
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));
    Some(spinner)
}

/// Finish a spinner with a completion message.
pub fn finish_spinner(spinner: Option<ProgressBar>, message: &str) {
    if let Some(s) = spinner {
        s.finish_with_message(message.to_string());
    }
}

/// Result of analyzing a single binary, includes summary and optionally code points.
pub struct BinaryAnalysisResult {
    pub summary: AnalysisSummary,
    pub code_points: Vec<CrateCodePoint>,
}

impl BinaryAnalysisResult {
    pub fn empty() -> Self {
        Self {
            summary: AnalysisSummary::default(),
            code_points: Vec::new(),
        }
    }
}

/// Represents a panic caller location for library analysis.
#[derive(Hash, Eq, PartialEq)]
struct PanicCaller {
    file: String,
    name: String,
    line: u32,
    column: Option<u32>,
    /// The panic symbol being called (e.g., "core::option::unwrap_failed")
    target: String,
}

/// Check if a function name belongs to the standard library.
/// Used to filter out stdlib functions from panic analysis results.
fn is_stdlib_function(name: &str) -> bool {
    name.starts_with("core::")
        || name.starts_with("std::")
        || name.starts_with("alloc::")
        || name.starts_with("<core::")
        || name.starts_with("<std::")
        || name.starts_with("<alloc::")
        || name.contains(" core::")
        || name.contains(" std::")
        || name.contains(" alloc::")
        || name.contains("::core::")
        || name.contains("::std::")
        || name.contains("::alloc::")
}

/// Analyze a single MachO binary/object for panic points.
/// Returns a summary of panic code points found, plus code points.
#[allow(clippy::too_many_arguments)]
pub fn analyze_macho(
    macho: &MachO,
    buffer: &[u8],
    binary_path: &Path,
    crate_src_path: Option<&str>,
    show_timings: bool,
    config: &Config,
    output: &OutputFormat,
) -> BinaryAnalysisResult {
    let show_progress = output.show_progress();
    let total_start = Instant::now();

    // Build set of valid source files for single-crate project filtering
    // This prevents false positives from dependencies with relative src/ paths
    let project_root = find_project_root(binary_path);
    let valid_files = project_root
        .as_ref()
        .map(|root| ValidSourceFiles::from_project_root(root));

    // Try each panic symbol pattern until we find one
    if show_progress {
        eprintln!("  Finding panic entry point...");
    }
    let step_start = Instant::now();
    let mut panic_symbol = None;
    let mut demangled = String::new();
    let mut target_addr = 0u64;

    for pattern in PANIC_SYMBOL_PATTERNS {
        if let Ok(Some((sym, dem))) = find_symbol_containing(macho, pattern)
            && let Some(addr) = find_symbol_address(macho, &sym)
        {
            panic_symbol = Some(sym);
            demangled = dem;
            target_addr = addr;
            break;
        }
    }
    if show_timings {
        eprintln!("  [timing] Find panic symbol: {:?}", step_start.elapsed());
    }

    let Some(_) = panic_symbol else {
        // No panic symbols found in this object
        return BinaryAnalysisResult::empty();
    };

    if show_progress {
        eprintln!("  Loading debug information...");
    }
    let step_start = Instant::now();
    let debug_info = load_debug_info(macho, binary_path, !show_progress);
    if show_timings {
        eprintln!("  [timing] Load debug info: {:?}", step_start.elapsed());
    }

    // Pre-compute the call graph by scanning all instructions once
    // Use debug info variant for source file/line enrichment
    let spinner = create_spinner(show_progress, "Scanning for function calls...");
    let step_start = Instant::now();

    // Create SymbolIndex once - CallGraph borrows from it to avoid allocations in hot path
    let symbol_index = SymbolIndex::new(macho);

    let call_graph = match &debug_info {
        DebugInfo::Embedded => {
            CallGraph::build_with_debug_info(
                macho,
                buffer,
                macho,
                buffer,
                crate_src_path,
                show_timings,
                symbol_index.as_ref(),
            )
            .or_else(|e| {
                eprintln!("Warning: debug-enriched call graph failed: {e}. Falling back to symbol-only graph.");
                CallGraph::build(macho, buffer, symbol_index.as_ref())
            })
            .unwrap_or_else(|e| {
                eprintln!("Error: call graph build failed: {e}");
                CallGraph::empty()
            })
        }
        DebugInfo::DSym(dsym_info) => dsym_info.with_debug_macho(|debug_macho| {
            if let Binary(debug_mach) = debug_macho {
                CallGraph::build_with_debug_info(
                    macho,
                    buffer,
                    debug_mach,
                    dsym_info.borrow_debug_buffer(),
                    crate_src_path,
                    show_timings,
                    symbol_index.as_ref(),
                )
                .or_else(|e| {
                    eprintln!("Warning: debug-enriched call graph failed: {e}. Falling back to symbol-only graph.");
                    CallGraph::build(macho, buffer, symbol_index.as_ref())
                })
                .unwrap_or_else(|e| {
                    eprintln!("Error: call graph build failed: {e}");
                    CallGraph::empty()
                })
            } else {
                CallGraph::build(macho, buffer, symbol_index.as_ref()).unwrap_or_else(|e| {
                    eprintln!("Error: call graph build failed: {e}");
                    CallGraph::empty()
                })
            }
        }),
        DebugInfo::DebugMap(_) | DebugInfo::None => {
            CallGraph::build(macho, buffer, symbol_index.as_ref()).unwrap_or_else(|e| {
                eprintln!("Error: call graph build failed: {e}");
                CallGraph::empty()
            })
        }
    };
    finish_spinner(spinner, "Scanning complete");
    if show_timings {
        eprintln!("  [timing] Build call graph: {:?}", step_start.elapsed());
    }

    // Create the root node for the call tree
    let mut root = CallTreeNode::new_root(demangled);

    // Track visited addresses to avoid infinite recursion (thread-safe)
    let visited = Arc::new(DashSet::new());
    visited.insert(target_addr);

    // Build the call tree in parallel
    let spinner = create_spinner(show_progress, "Building call tree...");
    let step_start = Instant::now();
    root.callers = build_call_tree_parallel(&call_graph, target_addr, &visited);
    let nodes_visited = visited.len();
    finish_spinner(
        spinner,
        &format!("Built call tree ({} nodes)", nodes_visited),
    );
    if show_timings {
        eprintln!("  [timing] Build call tree: {:?}", step_start.elapsed());
    }

    // Prune to only show paths leading to user code
    let spinner = create_spinner(show_progress, "Pruning to crate code...");
    let step_start = Instant::now();
    if let Some(crate_path) = crate_src_path {
        prune_call_tree(&mut root, crate_path, valid_files.as_ref());
    }
    finish_spinner(spinner, "Pruning complete");
    if show_timings {
        eprintln!("  [timing] Prune call tree: {:?}", step_start.elapsed());
    }

    // Always collect code points for unified output handling in main
    let step_start = Instant::now();
    let result = if let Some(crate_path) = crate_src_path {
        let (code_points, summary) =
            collect_crate_code_points(&root, crate_path, config, valid_files.as_ref());
        BinaryAnalysisResult {
            summary,
            code_points,
        }
    } else {
        // No crate path - can't collect code points
        BinaryAnalysisResult::empty()
    };
    if show_timings {
        eprintln!("  [timing] Collect/output: {:?}", step_start.elapsed());
        eprintln!("  [timing] TOTAL: {:?}", total_start.elapsed());
    }
    result
}

/// Analyze an archive (rlib/staticlib) for panic points using relocation-based call graph.
/// This works for library-only crates that don't have binary entry points.
pub fn analyze_archive(
    archive: &goblin::archive::Archive,
    buffer: &[u8],
    binary_path: &Path,
    crate_src_path: Option<&str>,
    show_timings: bool,
    config: &Config,
    output: &OutputFormat,
) -> BinaryAnalysisResult {
    // Build set of valid source files for single-crate project filtering
    let project_root = find_project_root(binary_path);
    let valid_files = project_root
        .as_ref()
        .map(|root| ValidSourceFiles::from_project_root(root));

    // Helper to check if a file path is within the crate/workspace scope
    // Uses ValidSourceFiles for single-crate validation to prevent dependency false positives
    let file_in_scope = |file: &str| {
        crate_src_path.is_none_or(|paths| {
            let file = file.replace('\\', "/");
            // Use the same validated matching as binary analysis
            matches_crate_pattern_validated(&file, paths, valid_files.as_ref())
        })
    };

    let show_progress = output.show_progress();
    let total_start = Instant::now();

    if show_progress {
        eprintln!("  Building library call graph from relocations...");
    }
    let step_start = Instant::now();

    // Build a merged call graph from all .o files in the archive
    let mut merged_graph = LibraryCallGraph::empty();

    for member_name in archive.members() {
        // Skip non-object files (like .rmeta)
        if !member_name.ends_with(".o") {
            continue;
        }

        // Extract the member data
        let member_data = match archive.extract(member_name, buffer) {
            Ok(data) => data,
            Err(e) => {
                if show_progress {
                    eprintln!("  Warning: Failed to extract {}: {}", member_name, e);
                }
                continue;
            }
        };

        // Parse the object file as Mach-O and build its call graph
        match MachO::parse(member_data, 0) {
            Ok(obj_macho) => {
                match LibraryCallGraph::build_from_object(&obj_macho, member_data, crate_src_path) {
                    Ok(obj_graph) => merged_graph.merge(obj_graph),
                    Err(e) => {
                        if show_progress {
                            eprintln!(
                                "  Warning: Failed to build call graph for {}: {}",
                                member_name, e
                            );
                        }
                    }
                }
            }
            Err(e) => {
                if show_progress {
                    eprintln!(
                        "  Warning: Failed to parse {} as Mach-O: {}",
                        member_name, e
                    );
                }
            }
        }
    }

    if show_timings {
        eprintln!(
            "  [timing] Build library call graph: {:?}",
            step_start.elapsed()
        );
    }

    if merged_graph.is_empty() {
        if show_progress {
            println!("\nNo call graph data found in archive");
        }
        return BinaryAnalysisResult::empty();
    }

    // Find all callers of panic-related functions
    if show_progress {
        eprintln!("  Finding panic callers...");
    }
    let step_start = Instant::now();

    let mut panic_callers: HashSet<PanicCaller> = HashSet::new();

    // Search for callers of panic-related symbols
    for target_sym in merged_graph.target_symbols() {
        // Check if this is a panic-related symbol
        // Note: be careful with std::panicking:: - set_hook/take_hook are NOT panic functions
        let is_panic_symbol = LIBRARY_PANIC_PATTERNS
            .iter()
            .any(|p| target_sym.contains(p))
            || target_sym.contains("core::panicking::")
            || (target_sym.contains("std::panicking::")
                && !target_sym.contains("set_hook")
                && !target_sym.contains("take_hook"));

        if !is_panic_symbol {
            continue;
        }

        for caller_info in merged_graph.get_callers(target_sym) {
            // Skip standard library functions - we only want user code
            if is_stdlib_function(&caller_info.caller_name) {
                continue;
            }

            // Get file from DWARF info, filtering out library code paths
            let dwarf_file = caller_info.caller_file.as_ref().filter(|f| {
                // Skip standard library and dependency paths
                !f.starts_with("/rustc/")
                    && !f.starts_with("/rust/")
                    && !f.starts_with("library/")
                    && !f.starts_with("src/arch/")
                    && !f.starts_with("src/raw/")
                    && !f.contains("/.cargo/")
                    && !f.contains("/deps/")
            });

            // Only include entries with proper DWARF file/line info from user code
            // Skip entries without valid line numbers (would show confusing ":0" in output)
            // Also filter by crate_src_path if provided (for workspace scoping)
            if let Some(file) = dwarf_file
                && file_in_scope(file)
                && let Some(line) = caller_info.line
            {
                panic_callers.insert(PanicCaller {
                    file: file.clone(),
                    name: caller_info.caller_name.to_string(),
                    line,
                    column: caller_info.column,
                    target: target_sym.to_string(),
                });
            }
        }
    }

    if show_timings {
        eprintln!("  [timing] Find panic callers: {:?}", step_start.elapsed());
    }

    // Report results
    if panic_callers.is_empty() {
        if show_progress {
            println!("\nNo panics in crate");
        }
        return BinaryAnalysisResult::empty();
    }

    // Convert PanicCaller to CrateCodePoint
    // Sort for deterministic output
    let mut sorted_callers: Vec<_> = panic_callers.into_iter().collect();
    sorted_callers.sort_by(|a, b| (&a.file, a.line, &a.name).cmp(&(&b.file, b.line, &b.name)));

    // Convert to CrateCodePoint structures with panic cause detection
    let mut code_points: Vec<CrateCodePoint> = sorted_callers
        .into_iter()
        .map(|caller| {
            let mut causes = std::collections::HashSet::new();
            // Detect panic cause from the panic symbol being called (target),
            // not from the user's function name (caller.name)
            if let Some(cause) =
                crate::panic_cause::detect_panic_cause(&caller.target, Some(&caller.file))
            {
                causes.insert(cause);
            }
            CrateCodePoint {
                name: caller.name,
                file: caller.file,
                line: caller.line,
                column: caller.column,
                causes,
                children: Vec::new(),  // Archives don't have call tree hierarchy
                is_direct_panic: true, // Archive analysis detects direct panic calls
                called_function: None, // Direct panics don't need called function name
            }
        })
        .collect();

    // Assign Unknown cause to points without identified causes (archives have no hierarchy)
    for point in &mut code_points {
        if point.causes.is_empty() {
            point.causes.insert(crate::panic_cause::PanicCause::Unknown);
        }
    }

    // Filter out code points whose causes are ALL allowed (not denied) by config
    // Use is_denied_at to support scoped rules based on file/function patterns
    code_points.retain(|point| {
        // Keep points with any denied cause
        point
            .causes
            .iter()
            .any(|c| config.is_denied_at(c, Some(&point.file), Some(&point.name)))
    });

    // Remove allowed causes, keeping only denied ones
    for point in &mut code_points {
        let file = point.file.clone();
        let name = point.name.clone();
        point
            .causes
            .retain(|c| config.is_denied_at(c, Some(&file), Some(&name)));
    }

    // Deduplicate by (file, line)
    let mut seen: std::collections::HashMap<(String, u32), usize> =
        std::collections::HashMap::new();
    let mut deduped: Vec<CrateCodePoint> = Vec::new();
    for point in code_points {
        let key = (point.file.clone(), point.line);
        if let Some(&idx) = seen.get(&key) {
            // Merge causes into existing point
            deduped[idx].causes.extend(point.causes);
        } else {
            seen.insert(key, deduped.len());
            deduped.push(point);
        }
    }

    if show_timings {
        eprintln!("  [timing] TOTAL: {:?}", total_start.elapsed());
    }

    // Build summary from code points
    let mut points: HashSet<(String, u32)> = HashSet::new();
    let mut files_affected: HashSet<String> = HashSet::new();
    for point in &deduped {
        points.insert((point.file.clone(), point.line));
        files_affected.insert(point.file.clone());
    }

    BinaryAnalysisResult {
        summary: AnalysisSummary::from_points(points, files_affected),
        code_points: deduped,
    }
}
