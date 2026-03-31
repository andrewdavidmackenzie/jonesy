//! Binary and archive analysis functions.
//!
//! This module provides the core analysis functions for finding panic paths
//! in Mach-O binaries and library archives.

use crate::args::OutputFormat;
use crate::call_tree::{
    AnalysisSummary, CallTreeNode, CrateCodePoint, build_call_tree_parallel_filtered,
    collect_crate_code_points,
};
use crate::cargo::find_project_root;
use crate::config::Config;
use crate::heuristics::{ABORT_SYMBOL_PATTERNS, PANIC_SYMBOL_PATTERNS, is_library_panic_symbol};
use crate::project_context::ProjectContext;
use crate::sym::{
    CallGraph, DebugInfo, LibraryCallGraph, SymbolIndex, SymbolTable, load_debug_info,
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

    /// Merge another result into this one, combining code points.
    /// Code points at the same location have their causes merged.
    pub fn merge(&mut self, other: BinaryAnalysisResult) {
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
    // Create SymbolTable for method calls
    let symbols = match SymbolTable::from(buffer) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: Failed to create symbol table: {}", e);
            return BinaryAnalysisResult::empty();
        }
    };
    let show_progress = output.show_progress();
    let total_start = Instant::now();

    // Build set of valid source files for crate source filtering.
    // All DWARF paths are absolute, so we need the project root to determine
    // which files belong to the user's crate.
    let Some(project_root) = find_project_root(binary_path) else {
        eprintln!(
            "Error: Cannot find project root for {}",
            binary_path.display()
        );
        return BinaryAnalysisResult::empty();
    };
    let project_context = ProjectContext::from_project_root(&project_root);

    // Find all entry points: panic symbols + abort symbols
    if show_progress {
        eprintln!("  Finding entry points...");
    }
    let step_start = Instant::now();

    // Collect all entry points with their addresses
    let mut entry_points: Vec<(String, String, u64)> = Vec::new(); // (mangled, demangled, addr)

    // Find panic entry points (first match from PANIC_SYMBOL_PATTERNS)
    for pattern in PANIC_SYMBOL_PATTERNS {
        if let Ok(Some((sym, dem))) = symbols.find_symbol_containing(pattern)
            && let Some(addr) = symbols.find_symbol_address(&sym)
        {
            entry_points.push((sym, dem, addr));
            break; // Only need one panic entry point
        }
    }

    // Find abort entry points
    if let Ok(abort_symbols) = symbols.find_all_symbols_matching(ABORT_SYMBOL_PATTERNS) {
        for (sym, dem) in abort_symbols {
            if let Some(addr) = symbols.find_symbol_address(&sym) {
                // Avoid duplicates
                if !entry_points.iter().any(|(_, _, a)| *a == addr) {
                    entry_points.push((sym, dem, addr));
                }
            }
        }
    }

    if show_timings {
        eprintln!("  [timing] Find entry points: {:?}", step_start.elapsed());
    }

    if entry_points.is_empty() {
        // No entry points found in this object
        return BinaryAnalysisResult::empty();
    }

    if show_progress && entry_points.len() > 1 {
        eprintln!(
            "  Found {} entry points (panic + abort)",
            entry_points.len()
        );
    }

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
                &project_context,
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
                    &project_context,
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

    // Build call trees for all entry points and merge results
    let mut final_result = BinaryAnalysisResult::empty();

    // Track visited addresses across all entry points to avoid redundant work
    let visited = Arc::new(DashSet::new());

    let spinner = create_spinner(show_progress, "Building call trees...");
    let step_start = Instant::now();

    for (_mangled, demangled, target_addr) in &entry_points {
        // Skip if we've already visited this address from another entry point
        if !visited.insert(*target_addr) {
            continue;
        }

        // Create root node for this entry point
        let mut root = CallTreeNode::new_root(demangled.clone());

        // Build the call tree for this entry point
        root.callers = build_call_tree_parallel_filtered(
            &call_graph,
            *target_addr,
            &visited,
            &project_context,
        );

        // Collect code points from this tree
        if crate_src_path.is_some() {
            let (code_points, _summary) = collect_crate_code_points(
                &root,
                config,
                &project_context,
                Some(project_root.as_path()),
            );
            let entry_result = BinaryAnalysisResult {
                summary: AnalysisSummary::default(),
                code_points,
            };
            final_result.merge(entry_result);
        }
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
    if show_timings {
        eprintln!(
            "  [timing] Build call trees (with pruning): {:?}",
            step_start.elapsed()
        );
        eprintln!("  [timing] TOTAL: {:?}", total_start.elapsed());
    }

    final_result
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
    // Build set of valid source files for crate source filtering.
    let Some(project_root) = find_project_root(binary_path) else {
        eprintln!(
            "Error: Cannot find project root for {}",
            binary_path.display()
        );
        return BinaryAnalysisResult::empty();
    };
    let project_context = ProjectContext::from_project_root(&project_root);

    // Helper to check if a file path is within the crate/workspace scope
    let file_in_scope = |file: &str| {
        crate_src_path.is_none_or(|_| {
            let file = file.replace('\\', "/");
            project_context.is_crate_source(&file)
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
                match LibraryCallGraph::build_from_object(
                    &obj_macho,
                    member_data,
                    crate_src_path,
                    &project_context,
                ) {
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

    // Filter out code points whose causes are ALL allowed (not denied) by config
    // Use is_denied_at to support scoped rules based on file/function patterns
    code_points.retain(|point| {
        // Keep points with any denied cause
        // Check both the containing function and the called function (for indirect panics)
        point.causes.iter().any(|c| {
            let denied_in_func = config.is_denied_at(c, Some(&point.file), Some(&point.name));
            let denied_in_called = point
                .called_function
                .as_ref()
                .map(|cf| config.is_denied_at(c, Some(&point.file), Some(cf)))
                .unwrap_or(true);
            denied_in_func && denied_in_called
        })
    });

    // Remove allowed causes, keeping only denied ones
    for point in &mut code_points {
        let file = point.file.clone();
        let name = point.name.clone();
        let called = point.called_function.clone();
        point.causes.retain(|c| {
            let denied_in_func = config.is_denied_at(c, Some(&file), Some(&name));
            let denied_in_called = called
                .as_ref()
                .map(|cf| config.is_denied_at(c, Some(&file), Some(cf)))
                .unwrap_or(true);
            denied_in_func && denied_in_called
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panic_cause::PanicCause;

    // ========================================================================
    // BinaryAnalysisResult tests
    // ========================================================================

    #[test]
    fn test_binary_analysis_result_empty() {
        let result = BinaryAnalysisResult::empty();
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

        let result2 = BinaryAnalysisResult::empty();

        result1.merge(result2);

        assert_eq!(result1.code_points.len(), 1);
    }
}
