//! Binary and archive analysis functions.
//!
//! This module provides the core analysis functions for finding panic paths
//! in Mach-O binaries and library archives.

use crate::args::OutputFormat;
use crate::binary_format::BinaryRef;
use crate::call_tree::filter_allowed_causes;
use crate::call_tree::{
    AnalysisSummary, CallTreeNode, CrateCodePoint, build_call_tree_parallel_filtered,
    collect_crate_code_points,
};
use crate::config::Config;
use crate::heuristics::{find_entry_points, is_library_panic_symbol};
use crate::project_context::ProjectContext;
use crate::sym::{
    CallGraph, DebugInfo, LibraryCallGraph, SymbolIndex, SymbolTable, load_debug_info,
};
use dashmap::DashSet;
use goblin::mach::Mach::Binary;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashSet;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// Create a spinner for long-running operations.
/// Returns None if progress display is disabled or stderr is not a terminal.
fn create_spinner(show_progress: bool, message: &str) -> Option<ProgressBar> {
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
fn finish_spinner(spinner: Option<ProgressBar>, message: &str) {
    if let Some(s) = spinner {
        s.finish_with_message(message.to_string());
    }
}

/// Result of analysing a single binary, includes summary and optionally code points.
#[derive(Default)]
pub struct BinaryAnalysisResult {
    pub summary: AnalysisSummary,
    pub code_points: Vec<CrateCodePoint>,
}

impl BinaryAnalysisResult {
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge another result into this one, combining code points.
    /// Code points at the same location have their causes merged.
    fn merge(&mut self, other: BinaryAnalysisResult) {
        use std::collections::HashMap;

        // Build a map of existing code points by (file, line)
        let mut points_map: HashMap<(String, u32), CrateCodePoint> = self
            .code_points
            .drain(..)
            .map(|cp| ((cp.file.clone(), cp.line), cp))
            .collect();

        // Merge other's code points
        for cp in other.code_points {
            let key = (cp.file.clone(), cp.line);
            if let Some(existing) = points_map.get_mut(&key) {
                // Merge causes (HashSet handles dedup)
                existing.causes.extend(cp.causes);
            } else {
                points_map.insert(key, cp);
            }
        }

        // Collect back to vec and sort
        self.code_points = points_map.into_values().collect();
        self.code_points
            .sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));

        // Recalculate summary from merged code points
        let points: HashSet<_> = self
            .code_points
            .iter()
            .map(|cp| (cp.file.clone(), cp.line))
            .collect();
        let files: HashSet<_> = self.code_points.iter().map(|cp| cp.file.clone()).collect();
        self.summary = AnalysisSummary::from_points(points, files);
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

/// Analyse a single MachO or ELF binary for panic points.
/// Returns a summary of panic code points found, plus code points.
pub fn analyze_binary_target(
    symbols: &SymbolTable,
    buffer: &[u8],
    binary_path: &Path,
    show_timings: bool,
    config: &Config,
    output: &OutputFormat,
    project_context: &ProjectContext,
) -> Result<BinaryAnalysisResult, String> {
    // Construct BinaryRef from the SymbolTable
    let binary_ref = match symbols {
        SymbolTable::MachO(Binary(macho)) => BinaryRef::MachO(macho),
        SymbolTable::Elf(elf) => BinaryRef::Elf(elf),
        _ => return Err("Expected MachO or ELF binary".to_string()),
    };

    let show_progress = output.show_progress();
    let total_start = show_timings.then(Instant::now);

    // Find all entry points: panic symbols + abort symbols
    if show_progress {
        eprintln!("  Finding entry points...");
    }
    let step_start = show_timings.then(Instant::now);

    let entry_points = find_entry_points(symbols);

    for (mangled, demangled, addr) in &entry_points {
        eprintln!(
            "  DEBUG: entry point: {:#x} {} ({})",
            addr, demangled, mangled
        );
    }

    if let Some(step_start) = step_start {
        eprintln!("  [timing] Find entry points: {:?}", step_start.elapsed());
    }

    if entry_points.is_empty() {
        // Not an error — binary has no panic symbols, so zero panic points
        return Ok(BinaryAnalysisResult::new());
    }

    if show_progress {
        eprintln!(
            "  Found {} entry points (panic + abort)",
            entry_points.len()
        );
    }

    if show_progress {
        eprintln!("  Loading debug information...");
    }
    let step_start = show_timings.then(Instant::now);
    let debug_info = load_debug_info(&binary_ref, binary_path, !show_progress);
    if let Some(step_start) = step_start {
        eprintln!("  [timing] Load debug info: {:?}", step_start.elapsed());
    }

    // Pre-compute the call graph by scanning all instructions once
    // Use debug info variant for source file/line enrichment
    let spinner = create_spinner(show_progress, "Scanning for function calls...");
    let step_start = show_timings.then(Instant::now);

    // Create SymbolIndex once - CallGraph borrows from it to avoid allocations in hot path
    let symbol_index = SymbolIndex::from_binary(&binary_ref);

    let call_graph = match &debug_info {
        DebugInfo::Embedded => {
            CallGraph::build_with_debug_info(
                &binary_ref,
                buffer,
                &binary_ref,
                buffer,
                show_timings,
                symbol_index.as_ref(),
                project_context,
            )
            .or_else(|e| {
                eprintln!("Warning: debug-enriched call graph failed: {e}. Falling back to symbol-only graph.");
                CallGraph::build(&binary_ref, buffer, symbol_index.as_ref())
            })
            .unwrap_or_else(|e| {
                eprintln!("Error: call graph build failed: {e}");
                CallGraph::empty()
            })
        }
        DebugInfo::DSym(dsym_info) => dsym_info.with_debug_macho(|debug_macho| {
            if let Binary(debug_mach) = debug_macho {
                let debug_binary_ref = BinaryRef::MachO(debug_mach);
                CallGraph::build_with_debug_info(
                    &binary_ref,
                    buffer,
                    &debug_binary_ref,
                    dsym_info.borrow_debug_buffer(),
                    show_timings,
                    symbol_index.as_ref(),
                    project_context,
                )
                .or_else(|e| {
                    eprintln!("Warning: debug-enriched call graph failed: {e}. Falling back to symbol-only graph.");
                    CallGraph::build(&binary_ref, buffer, symbol_index.as_ref())
                })
                .unwrap_or_else(|e| {
                    eprintln!("Error: call graph build failed: {e}");
                    CallGraph::empty()
                })
            } else {
                CallGraph::build(&binary_ref, buffer, symbol_index.as_ref()).unwrap_or_else(|e| {
                    eprintln!("Error: call graph build failed: {e}");
                    CallGraph::empty()
                })
            }
        }),
        DebugInfo::DebugMap(_) | DebugInfo::None => {
            CallGraph::build(&binary_ref, buffer, symbol_index.as_ref()).unwrap_or_else(|e| {
                eprintln!("Error: call graph build failed: {e}");
                CallGraph::empty()
            })
        }
    };
    finish_spinner(spinner, "Scanning complete");
    if let Some(step_start) = step_start {
        eprintln!("  [timing] Build call graph: {:?}", step_start.elapsed());
    }

    // Build call trees for all entry points and merge results
    let mut final_result = BinaryAnalysisResult::new();

    // Track visited addresses across all entry points to avoid redundant work
    let visited = Arc::new(DashSet::new());

    let spinner = create_spinner(show_progress, "Building call trees...");
    let step_start = show_timings.then(Instant::now);

    for (_mangled, demangled, target_addr) in &entry_points {
        let callers = call_graph.get_callers(*target_addr);
        eprintln!(
            "  DEBUG: entry {:#x} ({}) has {} direct callers in call graph",
            target_addr,
            demangled,
            callers.len()
        );

        // Skip if we've already visited this address from another entry point
        if !visited.insert(*target_addr) {
            continue;
        }

        // Create root node for this entry point
        let mut root = CallTreeNode::new_root(demangled.clone());

        // Build the call tree for this entry point
        root.callers =
            build_call_tree_parallel_filtered(&call_graph, *target_addr, &visited, project_context);

        // Collect code points from this tree
        let (code_points, _summary) = collect_crate_code_points(&root, config, project_context);
        let entry_result = BinaryAnalysisResult {
            summary: AnalysisSummary::default(),
            code_points,
        };
        final_result.merge(entry_result);
    }

    let total_nodes = visited.len();
    finish_spinner(
        spinner,
        &format!(
            "Built call trees ({} nodes from {} entry points)",
            total_nodes,
            entry_points.len()
        ),
    );
    if let (Some(step_start), Some(total_start)) = (step_start, total_start) {
        eprintln!(
            "  [timing] Build call trees (with pruning): {:?}",
            step_start.elapsed()
        );
        eprintln!("  [timing] TOTAL: {:?}", total_start.elapsed());
    }

    Ok(final_result)
}

/// Analyse an archive (rlib/staticlib) for panic points using a relocation-based call graph.
/// This works for library-only crates that don't have binary entry points.
pub fn analyze_archive(
    archive: &goblin::archive::Archive,
    buffer: &[u8],
    binary_path: &Path,
    show_timings: bool,
    config: &Config,
    output: &OutputFormat,
    project_context: &ProjectContext,
) -> Result<BinaryAnalysisResult, String> {
    // Helper to check if a file path is within the crate/workspace scope
    let show_progress = output.show_progress();
    let total_start = show_timings.then(Instant::now);

    if show_progress {
        eprintln!("  Building library call graph from relocations...");
    }
    let step_start = show_timings.then(Instant::now);

    // Build a merged call graph from all .o files in the archive
    let mut merged_graph = LibraryCallGraph::empty();

    // Get the library name to filter out stdlib/dependency .o files
    // For performance: only process .o files from the user's library
    // Extract library name from the binary path (e.g., "libstaticlib_example.a" -> "staticlib_example")
    // Note: .o files are named using the library name (from [lib] name in Cargo.toml),
    // not the package name, so we use lib_name directly for filtering
    let lib_name = binary_path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("lib"))
        .unwrap_or("");

    if show_progress {
        eprintln!("  DEBUG: lib_name = '{}'", lib_name);
    }

    // Collect member names for parallel processing
    let all_members: Vec<String> = archive.members().iter().map(|s| s.to_string()).collect();

    // Filter and categorise members
    let mut member_names = Vec::new();
    let mut skipped_count = 0;
    let mut sample_kept = Vec::new();
    let mut sample_skipped = Vec::new();

    for member_name in &all_members {
        // Skip non-object files (like .rmeta)
        if !member_name.ends_with(".o") {
            continue;
        }

        // Skip stdlib and dependency .o files - only process user library files
        // .o file names follow pattern: <lib_name>-<hash>.<module>.<hash>-cgu.<number>.rcgu.o
        // where lib_name matches the library name (e.g., "staticlib_example", "multi_bin_lib")
        if !lib_name.is_empty() {
            let normalized_lib_name = lib_name.replace('-', "_");
            if !member_name.starts_with(&normalized_lib_name)
                && !member_name.starts_with(&format!("{}-", lib_name))
            {
                if sample_skipped.len() < 2 {
                    sample_skipped.push(member_name.to_string());
                }
                skipped_count += 1;
                continue;
            } else if sample_kept.len() < 5 {
                sample_kept.push(member_name.to_string());
            }
        }

        member_names.push(member_name.clone());
    }

    // Pre-extract all member data to avoid potential serialization in archive.extract()
    if show_progress {
        eprintln!(
            "  Extracting {} .o files from archive...",
            member_names.len()
        );
    }
    let extraction_start = show_timings.then(Instant::now);

    let member_data_list: Vec<(String, Vec<u8>)> = member_names
        .iter()
        .filter_map(|member_name| match archive.extract(member_name, buffer) {
            Ok(data) => Some((member_name.clone(), data.to_vec())),
            Err(e) => {
                if show_progress {
                    eprintln!("  Warning: Failed to extract {}: {}", member_name, e);
                }
                None
            }
        })
        .collect();

    if let Some(extraction_start) = extraction_start {
        eprintln!(
            "  [timing] Extract .o files: {:?}",
            extraction_start.elapsed()
        );
    }

    // Process .o files in parallel
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    if show_progress {
        eprintln!(
            "  Processing {} .o files in parallel...",
            member_data_list.len()
        );
    }
    let processing_start = show_timings.then(Instant::now);

    let processed_count = AtomicUsize::new(0);
    let total_count = member_data_list.len();

    let graphs: Vec<LibraryCallGraph> = member_data_list
        .par_iter()
        .filter_map(|(member_name, member_data)| {
            // Parse the object file and build its call graph
            let result = match goblin::Object::parse(member_data.as_slice()) {
                Ok(goblin::Object::Mach(goblin::mach::Mach::Binary(obj_macho))) => {
                    let binary_ref = BinaryRef::MachO(&obj_macho);
                    LibraryCallGraph::build_from_object(
                        &binary_ref,
                        member_data.as_slice(),
                        project_context,
                    )
                    .map_err(|e| {
                        if show_progress {
                            eprintln!(
                                "  Warning: Failed to build call graph for {}: {}",
                                member_name, e
                            );
                        }
                    })
                    .ok()
                }
                Ok(goblin::Object::Elf(obj_elf)) => {
                    let binary_ref = BinaryRef::Elf(&obj_elf);
                    LibraryCallGraph::build_from_object(
                        &binary_ref,
                        member_data.as_slice(),
                        project_context,
                    )
                    .map_err(|e| {
                        if show_progress {
                            eprintln!(
                                "  Warning: Failed to build call graph for {}: {}",
                                member_name, e
                            );
                        }
                    })
                    .ok()
                }
                _ => {
                    if show_progress {
                        eprintln!("  Warning: Failed to parse {} as MachO or ELF", member_name);
                    }
                    None
                }
            };

            // Update progress counter
            let count = processed_count.fetch_add(1, Ordering::Relaxed) + 1;
            if show_progress && count % 10 == 0 {
                eprintln!("  Processing .o files: {}/{}", count, total_count);
            }

            result
        })
        .collect();

    if let Some(processing_start) = processing_start {
        eprintln!(
            "  [timing] Process .o files (parallel): {:?}",
            processing_start.elapsed()
        );
    }

    // Merge all graphs sequentially (fast operation)
    for graph in graphs {
        merged_graph.merge(graph);
    }

    let processed_count = processed_count.load(Ordering::Relaxed);

    if show_progress {
        if !sample_kept.is_empty() {
            eprintln!("  DEBUG: Sample kept files:");
            for file in &sample_kept {
                eprintln!("    {}", file);
            }
        }
        if !sample_skipped.is_empty() {
            eprintln!("  DEBUG: Sample skipped files:");
            for file in &sample_skipped {
                eprintln!("    {}", file);
            }
        }
        eprintln!(
            "  Processed {} .o files ({} stdlib/dependency files skipped)",
            processed_count, skipped_count
        );
    }

    if let Some(step_start) = step_start {
        eprintln!(
            "  [timing] Build library call graph: {:?}",
            step_start.elapsed()
        );
    }

    if merged_graph.is_empty() {
        // Not an error — archive has no relocations to panic functions
        if show_progress {
            println!("\nNo call graph data found in archive");
        }
        return Ok(BinaryAnalysisResult::new());
    }

    // Find all callers of panic-related functions
    if show_progress {
        eprintln!("  Finding panic callers...");
    }
    let step_start = show_timings.then(Instant::now);

    let mut panic_callers: HashSet<PanicCaller> = HashSet::new();

    // Search for callers of panic-related symbols
    for target_sym in merged_graph.target_symbols() {
        if !is_library_panic_symbol(target_sym) {
            continue;
        }

        for caller_info in merged_graph.get_callers(target_sym) {
            // Get file from DWARF info, filtering out non-crate code paths
            let dwarf_file = caller_info
                .caller_file
                .as_ref()
                .filter(|f| project_context.is_crate_source(f));

            // Only include entries with proper DWARF file/line info from user code
            // Skip entries without valid line numbers (would show confusing ":0" in output)
            if let Some(file) = dwarf_file
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

    if let Some(step_start) = step_start {
        eprintln!("  [timing] Find panic callers: {:?}", step_start.elapsed());
    }

    // Report results
    if panic_callers.is_empty() {
        // Not an error — analysis succeeded but found no panic points in user code
        if show_progress {
            println!("\nNo panics in crate");
        }
        return Ok(BinaryAnalysisResult::new());
    }

    // Convert PanicCaller to CrateCodePoint
    // Sort for deterministic output
    let mut sorted_callers: Vec<_> = panic_callers.into_iter().collect();
    sorted_callers.sort_by(|a, b| (&a.file, a.line, &a.name).cmp(&(&b.file, b.line, &b.name)));

    // Convert to CrateCodePoint structures with panic cause detection
    let mut code_points: Vec<CrateCodePoint> = sorted_callers
        .into_iter()
        .map(|caller| {
            let mut causes = HashSet::new();
            // Detect panic cause from the panic symbol being called (target),
            // not from the user's function name (caller.name)
            if let Some(cause) = crate::heuristics::detect_panic_cause(&caller.target) {
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

    // Filter out allowed causes using the same logic as binary analysis,
    // including inline allow comments (e.g., `// jonesy:allow(overflow)`)
    filter_allowed_causes(&mut code_points, config, project_context);

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

    if let Some(total_start) = total_start {
        eprintln!("  [timing] TOTAL: {:?}", total_start.elapsed());
    }

    // Build summary from code points
    let mut points: HashSet<(String, u32)> = HashSet::new();
    let mut files_affected: HashSet<String> = HashSet::new();
    for point in &deduped {
        points.insert((point.file.clone(), point.line));
        files_affected.insert(point.file.clone());
    }

    Ok(BinaryAnalysisResult {
        summary: AnalysisSummary::from_points(points, files_affected),
        code_points: deduped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panic_cause::PanicCause;

    // ========================================================================
    // BinaryAnalysisResult tests
    // ========================================================================

    #[test]
    fn test_binary_analysis_result_empty() {
        let result = BinaryAnalysisResult::new();
        assert!(result.code_points.is_empty());
        assert_eq!(result.summary.panic_points(), 0);
        assert_eq!(result.summary.files_affected(), 0);
    }

    fn make_code_point(file: &str, line: u32, cause: PanicCause) -> CrateCodePoint {
        let mut causes = HashSet::new();
        causes.insert(cause);
        CrateCodePoint {
            name: "test_func".to_string(),
            file: file.to_string(),
            line,
            column: None,
            causes,
            children: Vec::new(),
            is_direct_panic: true,
            called_function: None,
        }
    }

    #[test]
    fn test_binary_analysis_result_merge_disjoint() {
        let mut result1 = BinaryAnalysisResult {
            summary: AnalysisSummary::default(),
            code_points: vec![make_code_point("src/a.rs", 10, PanicCause::Unwrap)],
        };

        let result2 = BinaryAnalysisResult {
            summary: AnalysisSummary::default(),
            code_points: vec![make_code_point("src/b.rs", 20, PanicCause::Unwrap)],
        };

        result1.merge(result2);

        assert_eq!(result1.code_points.len(), 2);
        assert_eq!(result1.summary.panic_points(), 2);
        assert_eq!(result1.summary.files_affected(), 2);
    }

    #[test]
    fn test_binary_analysis_result_merge_same_location() {
        let mut result1 = BinaryAnalysisResult {
            summary: AnalysisSummary::default(),
            code_points: vec![make_code_point("src/main.rs", 10, PanicCause::Unwrap)],
        };

        let result2 = BinaryAnalysisResult {
            summary: AnalysisSummary::default(),
            code_points: vec![make_code_point("src/main.rs", 10, PanicCause::Expect)],
        };

        result1.merge(result2);

        // Same file:line should be merged, causes combined
        assert_eq!(result1.code_points.len(), 1);
        assert_eq!(result1.code_points[0].causes.len(), 2);
        assert!(result1.code_points[0].causes.contains(&PanicCause::Unwrap));
        assert!(result1.code_points[0].causes.contains(&PanicCause::Expect));
    }

    #[test]
    fn test_binary_analysis_result_merge_sorted() {
        let mut result1 = BinaryAnalysisResult {
            summary: AnalysisSummary::default(),
            code_points: vec![make_code_point("src/z.rs", 100, PanicCause::Unwrap)],
        };

        let result2 = BinaryAnalysisResult {
            summary: AnalysisSummary::default(),
            code_points: vec![
                make_code_point("src/a.rs", 10, PanicCause::Unwrap),
                make_code_point("src/a.rs", 5, PanicCause::ExplicitPanic),
            ],
        };

        result1.merge(result2);

        // Should be sorted by file, then line
        assert_eq!(result1.code_points.len(), 3);
        assert_eq!(result1.code_points[0].file, "src/a.rs");
        assert_eq!(result1.code_points[0].line, 5);
        assert_eq!(result1.code_points[1].file, "src/a.rs");
        assert_eq!(result1.code_points[1].line, 10);
        assert_eq!(result1.code_points[2].file, "src/z.rs");
        assert_eq!(result1.code_points[2].line, 100);
    }

    #[test]
    fn test_binary_analysis_result_merge_empty() {
        let mut result1 = BinaryAnalysisResult {
            summary: AnalysisSummary::default(),
            code_points: vec![make_code_point("src/main.rs", 10, PanicCause::Unwrap)],
        };

        let result2 = BinaryAnalysisResult::new();

        result1.merge(result2);

        assert_eq!(result1.code_points.len(), 1);
    }
}
