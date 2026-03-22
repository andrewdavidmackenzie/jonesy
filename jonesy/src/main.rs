use crate::args::{Args, OutputFormat, VERSION, WorkspaceMember, parse_args};
use crate::call_tree::{
    AnalysisResult, AnalysisSummary, CallTreeNode, CrateCodePoint, build_call_tree_parallel,
    collect_crate_code_points, prune_call_tree,
};
use crate::cargo::{
    derive_crate_src_path, detect_library_type, find_project_root, get_project_name,
};
use crate::config::Config;
use crate::html_output::{generate_html_output, generate_workspace_html_output};
use crate::json_output::{
    WorkspaceMemberResult, WorkspaceResult, generate_json_output, generate_workspace_json_output,
};
use crate::sym::{
    CallGraph, DebugInfo, LibraryCallGraph, SymbolTable, ValidSourceFiles, find_symbol_address,
    find_symbol_containing, load_debug_info, matches_crate_pattern_validated, read_symbols,
};
use crate::text_output::generate_text_output;
use dashmap::DashSet;
use goblin::mach::Mach::{Binary, Fat};
use goblin::mach::MachO;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::error::Error;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

mod args;
mod call_tree;
mod cargo;
mod config;
mod html_output;
mod inline_allows;
mod json_output;
mod lsp;
mod panic_cause;
#[cfg(target_os = "macos")]
mod sym;
mod text_output;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();

    let parsed_args = parse_args(&args).unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        std::process::exit(255);
    });

    // Handle LSP mode
    if parsed_args.lsp_mode {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(lsp::run_lsp_server());
        return Ok(());
    }

    // Configure rayon thread pool with user-specified max threads
    rayon::ThreadPoolBuilder::new()
        .num_threads(parsed_args.max_threads)
        .build_global()
        .ok(); // Ignore error if pool already initialized

    // Handle workspace mode differently
    if let Some(ref workspace_members) = parsed_args.workspace_members {
        return analyze_workspace(workspace_members, &parsed_args);
    }

    use std::collections::HashSet;

    let mut total_summary = AnalysisSummary::default();
    let mut all_code_points: Vec<CrateCodePoint> = Vec::new();
    let mut seen_code_points: HashSet<(String, u32)> = HashSet::new();
    let mut project_name: Option<String> = None;
    let mut project_root_path: Option<String> = None;

    for binary_path in &parsed_args.binaries {
        // Canonicalize the binary path to ensure absolute paths for clickable links
        let binary_path = binary_path
            .canonicalize()
            .unwrap_or_else(|_| binary_path.clone());
        if parsed_args.output.show_progress() {
            println!("Processing {}", binary_path.display());
        }

        // Find the project/workspace root from the binary path
        let project_root = find_project_root(&binary_path);

        // Find the member crate directory for config loading
        // In workspaces, derive_crate_src_path returns paths like "flowc/src/" or "examples/panic/src/"
        // We want the crate directory (parent of src/) for config loading
        let crate_dir = derive_crate_src_path(&binary_path).and_then(|src_path| {
            // Strip trailing "src/" to get the crate directory
            let crate_rel = src_path.strip_suffix("src/").unwrap_or(&src_path);
            project_root
                .as_ref()
                .map(|root| root.join(crate_rel.trim_end_matches('/')))
        });

        // Load configuration: prefer crate-specific config, fall back to workspace root
        let config = if let Some(ref crate_path) = crate_dir
            && crate_path.join("Cargo.toml").exists()
        {
            // Load from the member crate directory
            Config::load_for_project(crate_path, parsed_args.config_path.as_deref())
        } else if let Some(ref root) = project_root {
            // Fall back to workspace/project root
            Config::load_for_project(root, parsed_args.config_path.as_deref())
        } else {
            // No project root found - use defaults plus explicit --config only
            let mut config = Config::with_defaults();
            if let Some(config_path) = parsed_args.config_path.as_deref() {
                config.load_from_config_file(config_path)?;
            }
            Ok(config)
        }
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            std::process::exit(255);
        });

        // Check if this is a library and detect its type
        let is_dylib = binary_path.extension().is_some_and(|ext| ext == "dylib");
        if parsed_args.output.show_progress()
            && is_dylib
            && let Some(lib_type) = detect_library_type(&binary_path)
        {
            println!("Library type: {}", lib_type);
            if lib_type == "dylib" {
                println!(
                    "Note: Rust dylib includes the standard library runtime. \
                     Analysis may take longer."
                );
            }
        }

        let binary_buffer = fs::read(&binary_path)?;
        let symbols = read_symbols(&binary_buffer)?;

        // Capture project info from the first binary processed
        if project_name.is_none() {
            // Prefer project name from Cargo manifest, fall back to the binary filename
            project_name = project_root
                .as_ref()
                .and_then(|root| get_project_name(root))
                .or_else(|| {
                    binary_path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                });
            project_root_path = project_root
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
        }

        match symbols {
            SymbolTable::MachO(Binary(macho)) => {
                let crate_src_path = derive_crate_src_path(&binary_path);
                let result = analyze_macho(
                    &macho,
                    &binary_buffer,
                    &binary_path,
                    crate_src_path.as_deref(),
                    parsed_args.show_timings,
                    &config,
                    &parsed_args.output,
                );
                total_summary.add(&result.summary);
                // Deduplicate code points across binaries, merging causes
                for point in result.code_points {
                    let key = (point.file.clone(), point.line);
                    if seen_code_points.insert(key) {
                        all_code_points.push(point);
                    } else if let Some(existing) = all_code_points
                        .iter_mut()
                        .find(|p| p.file == point.file && p.line == point.line)
                    {
                        existing.causes.extend(point.causes);
                    }
                }
            }
            SymbolTable::MachO(Fat(multi_arch)) => {
                if !parsed_args.output.is_summary_only() {
                    println!("FAT: {:?} architectures", multi_arch.arches()?);
                }
            }
            SymbolTable::Archive(archive) => {
                // Use relocation-based analysis for library archives
                let crate_src_path = derive_crate_src_path(&binary_path);
                let result = analyze_archive(
                    &archive,
                    &binary_buffer,
                    &binary_path,
                    crate_src_path.as_deref(),
                    parsed_args.show_timings,
                    &config,
                    &parsed_args.output,
                );
                total_summary.add(&result.summary);
                // Deduplicate code points across binaries, merging causes
                for point in result.code_points {
                    let key = (point.file.clone(), point.line);
                    if seen_code_points.insert(key) {
                        all_code_points.push(point);
                    } else if let Some(existing) = all_code_points
                        .iter_mut()
                        .find(|p| p.file == point.file && p.line == point.line)
                    {
                        existing.causes.extend(point.causes);
                    }
                }
            }
        }

        if parsed_args.output.show_progress() {
            println!();
        }
    }

    // Create unified analysis result
    let result = AnalysisResult::new(
        project_name.unwrap_or_else(|| "unknown".to_string()),
        project_root_path.unwrap_or_else(|| ".".to_string()),
        all_code_points,
    );

    // Output results based on format
    let tree = parsed_args.output.show_tree();
    let summary_only = parsed_args.output.is_summary_only();

    if parsed_args.output.is_json() {
        match generate_json_output(&result, tree, summary_only) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("Error serializing JSON: {}", e);
                std::process::exit(255);
            }
        }
    } else if parsed_args.output.is_html() {
        let html = generate_html_output(&result, tree, summary_only);
        println!("{}", html);
    } else {
        let no_hyperlinks = !parsed_args.output.use_hyperlinks();
        generate_text_output(&result, tree, summary_only, no_hyperlinks);
    }

    // Exit with the number of panic points found (0 = passed, >0 = found panics)
    // Note: Unix exit codes are 8-bit (0-255), the values above wrap around
    std::process::exit(result.panic_points() as i32);
}

/// Panic symbol patterns to search for, in order of preference.
/// For binaries, rust_panic$ is the root. For libraries, we look for
/// the functions that call into the panic runtime.
const PANIC_SYMBOL_PATTERNS: &[&str] = &[
    "rust_panic$",        // Main panic entry point (binaries)
    "panic_fmt$",         // Core panic formatting (libraries)
    "panic_display",      // Panic display helper
    "slice_index_fail",   // Slice indexing panics (vec[i] where i >= len)
    "str_index_overflow", // String slice boundary panics
];

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
pub(crate) struct BinaryAnalysisResult {
    pub summary: AnalysisSummary,
    pub code_points: Vec<CrateCodePoint>,
}

impl BinaryAnalysisResult {
    fn empty() -> Self {
        Self {
            summary: AnalysisSummary::default(),
            code_points: Vec::new(),
        }
    }
}

/// Analyze a single MachO binary/object for panic points.
/// Returns a summary of panic code points found, plus code points.
#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_macho(
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
    let call_graph = match &debug_info {
        DebugInfo::Embedded => {
            CallGraph::build_with_debug_info(macho, buffer, macho, buffer, crate_src_path, show_timings)
                .or_else(|e| {
                    eprintln!("Warning: debug-enriched call graph failed: {e}. Falling back to symbol-only graph.");
                    CallGraph::build(macho, buffer)
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
                )
                .or_else(|e| {
                    eprintln!("Warning: debug-enriched call graph failed: {e}. Falling back to symbol-only graph.");
                    CallGraph::build(macho, buffer)
                })
                .unwrap_or_else(|e| {
                    eprintln!("Error: call graph build failed: {e}");
                    CallGraph::empty()
                })
            } else {
                CallGraph::build(macho, buffer).unwrap_or_else(|e| {
                    eprintln!("Error: call graph build failed: {e}");
                    CallGraph::empty()
                })
            }
        }),
        DebugInfo::DebugMap(_) | DebugInfo::None => {
            CallGraph::build(macho, buffer).unwrap_or_else(|e| {
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

/// Panic symbol name patterns to search for in library call graphs.
/// These are the demangled names of functions that indicate panic.
const LIBRARY_PANIC_PATTERNS: &[&str] = &[
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

/// Analyze an archive (rlib/staticlib) for panic points using relocation-based call graph.
/// This works for library-only crates that don't have binary entry points.
pub(crate) fn analyze_archive(
    archive: &goblin::archive::Archive,
    buffer: &[u8],
    binary_path: &Path,
    crate_src_path: Option<&str>,
    show_timings: bool,
    config: &Config,
    output: &OutputFormat,
) -> BinaryAnalysisResult {
    use std::collections::HashSet;

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
            if is_stdlib_function(&caller_info.caller.name) {
                continue;
            }

            // Get file from DWARF info, filtering out library code paths
            let dwarf_file = caller_info.caller.file.as_ref().filter(|f| {
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
                    name: caller_info.caller.name.clone(),
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

/// Analyze a workspace with multiple member crates.
/// Produces per-crate reports and an aggregate workspace summary.
fn analyze_workspace(members: &[WorkspaceMember], args: &Args) -> Result<(), Box<dyn Error>> {
    let workspace_root = std::env::current_dir()?;

    if args.output.show_progress() {
        println!(
            "Analyzing workspace with {} member crate(s)...\n",
            members.len()
        );
    }

    let mut workspace_summary = AnalysisSummary::default();
    let mut member_results: Vec<WorkspaceMemberResult> = Vec::new();

    // Collect all source paths from actual binary [[bin]] paths
    // This handles non-standard layouts like [[bin]] path = "crates/core/main.rs"
    // Join patterns with "|" separator for is_in_crate to check
    let workspace_src_path = members
        .iter()
        .flat_map(|m| {
            m.binaries
                .iter()
                .filter_map(|binary_path| derive_crate_src_path(binary_path))
        })
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join("|");

    for member in members {
        if args.output.show_progress() {
            println!("=== {} ===", member.name);
        }

        // Load configuration once for this member crate (same for all binaries)
        // If user explicitly provided --config, fail fast on errors
        let config = match Config::load_for_project(&member.path, args.config_path.as_deref()) {
            Ok(c) => c,
            Err(e) if args.config_path.is_some() => {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Failed to load config for {}: {}", member.name, e),
                )));
            }
            Err(e) => {
                eprintln!("Warning: Failed to load config for {}: {}", member.name, e);
                Config::with_defaults()
            }
        };

        // Analyze binaries in parallel for better performance
        if args.output.show_progress() && member.binaries.len() > 1 {
            println!(
                "Analyzing {} binaries in parallel...",
                member.binaries.len()
            );
        }

        // Parallel analysis of all binaries in this member
        let binary_results: Vec<(PathBuf, BinaryAnalysisResult)> = member
            .binaries
            .par_iter()
            .filter_map(|binary_path| {
                let binary_path = binary_path
                    .canonicalize()
                    .unwrap_or_else(|_| binary_path.clone());
                let binary_buffer = fs::read(&binary_path).ok()?;
                let symbols = read_symbols(&binary_buffer).ok()?;

                let result = match symbols {
                    SymbolTable::MachO(Binary(macho)) => analyze_macho(
                        &macho,
                        &binary_buffer,
                        &binary_path,
                        Some(&workspace_src_path),
                        args.show_timings,
                        &config,
                        &args.output,
                    ),
                    SymbolTable::MachO(Fat(_)) => {
                        // FAT binaries not fully supported - return empty result
                        BinaryAnalysisResult::empty()
                    }
                    SymbolTable::Archive(archive) => analyze_archive(
                        &archive,
                        &binary_buffer,
                        &binary_path,
                        Some(&workspace_src_path),
                        args.show_timings,
                        &config,
                        &args.output,
                    ),
                };
                Some((binary_path, result))
            })
            .collect();

        // Merge results sequentially
        let mut member_summary = AnalysisSummary::default();
        let mut member_code_points: Vec<CrateCodePoint> = Vec::new();
        let mut seen_code_points: std::collections::HashSet<(String, u32)> =
            std::collections::HashSet::new();

        for (binary_path, result) in binary_results {
            if args.output.show_progress() {
                println!("Processed {}", binary_path.display());
            }
            member_summary.add(&result.summary);
            // Collect code points with deduplication, merging causes
            for point in result.code_points {
                let key = (point.file.clone(), point.line);
                if seen_code_points.insert(key) {
                    member_code_points.push(point);
                } else if let Some(existing) = member_code_points
                    .iter_mut()
                    .find(|p| p.file == point.file && p.line == point.line)
                {
                    existing.causes.extend(point.causes);
                }
            }
        }

        // For text output, print immediately; for JSON/HTML, collect for later
        if args.output.is_text() {
            if !args.output.is_summary_only() {
                let member_result = AnalysisResult::new(
                    member.name.clone(),
                    workspace_root.to_string_lossy().to_string(),
                    member_code_points.clone(),
                );
                let no_hyperlinks = !args.output.use_hyperlinks();
                generate_text_output(
                    &member_result,
                    args.output.show_tree(),
                    false,
                    no_hyperlinks,
                );
            } else if args.output.show_progress() {
                println!(
                    "Panic points: {} in {} file(s)\n",
                    member_summary.panic_points(),
                    member_summary.files_affected()
                );
            }
        }

        // Store member results for workspace output
        member_results.push(WorkspaceMemberResult {
            name: member.name.clone(),
            path: member.path.to_string_lossy().to_string(),
            summary: member_summary.clone(),
            code_points: member_code_points,
        });
        workspace_summary.add(&member_summary);
    }

    // Build workspace result
    let workspace_result = WorkspaceResult {
        root: workspace_root.to_string_lossy().to_string(),
        members: member_results,
        total_summary: workspace_summary.clone(),
    };

    let tree = args.output.show_tree();
    let summary_only = args.output.is_summary_only();

    // Output based on format
    if args.output.is_json() {
        match generate_workspace_json_output(&workspace_result, tree, summary_only) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("Error serializing JSON: {}", e);
                std::process::exit(255);
            }
        }
    } else if args.output.is_html() {
        let html = generate_workspace_html_output(&workspace_result, tree, summary_only);
        println!("{}", html);
    } else {
        // Text output: print workspace summary
        println!("=== Workspace Summary (jonesy v{}) ===", VERSION);
        println!("  Root: {}", workspace_root.display());
        println!("  Members analyzed: {}", workspace_result.members.len());
        for member in &workspace_result.members {
            println!(
                "    {}: {} panic point(s) in {} file(s)",
                member.name,
                member.summary.panic_points(),
                member.summary.files_affected()
            );
        }
        println!(
            "  Total panic points: {} across {} crate(s)",
            workspace_summary.panic_points(),
            members.len()
        );
    }

    // Exit with the number of panic points found
    std::process::exit(workspace_summary.panic_points() as i32);
}
