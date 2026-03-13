use crate::args::{Args, WorkspaceMember, parse_args};
use crate::call_tree::{
    AnalysisSummary, CallTreeNode, build_call_tree_parallel, count_crate_code_points_summary,
    print_call_tree, print_crate_code_points, prune_call_tree,
};
use crate::cargo::{
    derive_crate_src_path, detect_library_type, find_project_root, get_project_name,
};
use crate::config::Config;
use crate::sym::{
    CallGraph, DebugInfo, LibraryCallGraph, SymbolTable, find_symbol_address,
    find_symbol_containing, load_debug_info, read_symbols,
};
use dashmap::DashSet;
use goblin::mach::Mach::{Binary, Fat};
use goblin::mach::MachO;
use std::error::Error;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

mod args;
mod call_tree;
mod cargo;
mod config;
mod panic_cause;
#[cfg(target_os = "macos")]
mod sym;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();

    let parsed_args = parse_args(&args).unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        std::process::exit(255);
    });

    // Configure rayon thread pool with user-specified max threads
    rayon::ThreadPoolBuilder::new()
        .num_threads(parsed_args.max_threads)
        .build_global()
        .ok(); // Ignore error if pool already initialized

    // Handle workspace mode differently
    if let Some(ref workspace_members) = parsed_args.workspace_members {
        return analyze_workspace(workspace_members, &parsed_args);
    }

    let mut total_summary = AnalysisSummary::default();
    let mut project_name: Option<String> = None;
    let mut project_root_path: Option<String> = None;

    for binary_path in &parsed_args.binaries {
        // Canonicalize the binary path to ensure absolute paths for clickable links
        let binary_path = binary_path
            .canonicalize()
            .unwrap_or_else(|_| binary_path.clone());
        if !parsed_args.summary_only && !parsed_args.quiet {
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
        if !parsed_args.summary_only
            && !parsed_args.quiet
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
                let summary = analyze_macho(
                    &macho,
                    &binary_buffer,
                    &binary_path,
                    crate_src_path.as_deref(),
                    parsed_args.show_tree,
                    parsed_args.summary_only,
                    parsed_args.show_timings,
                    parsed_args.quiet,
                    parsed_args.no_hyperlinks,
                    &config,
                    project_root.as_deref(),
                );
                total_summary.add(&summary);
            }
            SymbolTable::MachO(Fat(multi_arch)) => {
                if !parsed_args.summary_only {
                    println!("FAT: {:?} architectures", multi_arch.arches()?);
                }
            }
            SymbolTable::Archive(archive) => {
                // Use relocation-based analysis for library archives
                let crate_src_path = derive_crate_src_path(&binary_path);
                let summary = analyze_archive(
                    &archive,
                    &binary_buffer,
                    &binary_path,
                    crate_src_path.as_deref(),
                    parsed_args.show_tree,
                    parsed_args.summary_only,
                    parsed_args.show_timings,
                    parsed_args.quiet,
                    parsed_args.no_hyperlinks,
                    &config,
                    project_root.as_deref(),
                );
                total_summary.add(&summary);
            }
        }

        if !parsed_args.summary_only && !parsed_args.quiet {
            println!();
        }
    }

    // Print summary
    println!("Summary:");
    if let Some(name) = &project_name {
        println!("  Project: {}", name);
    }
    if let Some(root) = &project_root_path {
        println!("  Root: {}", root);
    }
    println!(
        "  Panic points: {} in {} file(s)",
        total_summary.panic_points(),
        total_summary.files_affected()
    );

    // Exit with the number of panic points found (0 = passed, >0 = found panics)
    // Note: Unix exit codes are 8-bit (0-255), the values above wrap around
    std::process::exit(total_summary.panic_points() as i32);
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

/// Analyze a single MachO binary/object for panic points.
/// Returns a summary of panic code points found.
#[allow(clippy::too_many_arguments)]
fn analyze_macho(
    macho: &MachO,
    buffer: &[u8],
    binary_path: &Path,
    crate_src_path: Option<&str>,
    show_tree: bool,
    summary_only: bool,
    show_timings: bool,
    quiet: bool,
    no_hyperlinks: bool,
    config: &Config,
    project_root: Option<&Path>,
) -> AnalysisSummary {
    // Helper to print progress messages (respects quiet and summary_only flags)
    let show_progress = !quiet && !summary_only;
    let total_start = Instant::now();

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
            && let Some((_name, addr)) = find_symbol_address(macho, &sym)
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
        return AnalysisSummary::default();
    };

    if show_progress {
        eprintln!("  Loading debug information...");
    }
    let step_start = Instant::now();
    let debug_info = load_debug_info(macho, binary_path, summary_only || quiet);
    if show_timings {
        eprintln!("  [timing] Load debug info: {:?}", step_start.elapsed());
    }

    // Pre-compute the call graph by scanning all instructions once
    // Use debug info variant for source file/line enrichment
    if show_progress {
        eprintln!("  Scanning for function calls...");
    }
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
    if show_timings {
        eprintln!("  [timing] Build call graph: {:?}", step_start.elapsed());
    }

    // Create the root node for the call tree
    let mut root = CallTreeNode::new_root(demangled.clone());

    // Track visited addresses to avoid infinite recursion (thread-safe)
    let visited = Arc::new(DashSet::new());
    visited.insert(target_addr);

    // Build the call tree in parallel
    if show_progress {
        eprintln!("  Building call tree...");
    }
    let step_start = Instant::now();
    root.callers = build_call_tree_parallel(&call_graph, target_addr, &visited);
    if show_timings {
        eprintln!("  [timing] Build call tree: {:?}", step_start.elapsed());
    }

    // Prune to only show paths leading to user code
    if show_progress {
        eprintln!("  Pruning to crate code...");
    }
    let step_start = Instant::now();
    if let Some(crate_path) = crate_src_path {
        prune_call_tree(&mut root, crate_path);
    }
    if show_timings {
        eprintln!("  [timing] Prune call tree: {:?}", step_start.elapsed());
    }

    // Print output based on flags
    let step_start = Instant::now();
    let result = if summary_only {
        // Silent mode - just count without printing
        crate_src_path.map_or(AnalysisSummary::default(), |cp| {
            count_crate_code_points_summary(&root, cp, config)
        })
    } else if show_tree {
        println!("Full call tree:");
        print_call_tree(&root, 0);
        crate_src_path.map_or(AnalysisSummary::default(), |cp| {
            count_crate_code_points_summary(&root, cp, config)
        })
    } else if let Some(crate_path) = crate_src_path {
        print_crate_code_points(&root, crate_path, project_root, config, no_hyperlinks)
    } else {
        println!("Could not determine crate source path, showing full tree");
        print_call_tree(&root, 0);
        AnalysisSummary::default()
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
#[allow(clippy::too_many_arguments)]
fn analyze_archive(
    archive: &goblin::archive::Archive,
    buffer: &[u8],
    _binary_path: &Path,
    crate_src_path: Option<&str>,
    _show_tree: bool,
    summary_only: bool,
    show_timings: bool,
    quiet: bool,
    _no_hyperlinks: bool,
    _config: &Config,
    _project_root: Option<&Path>,
) -> AnalysisSummary {
    // See issue #56 for planned enhancements to these unused parameters
    use std::collections::HashSet;

    // Helper to check if a file path is within the crate/workspace scope
    let file_in_scope = |file: &str| {
        crate_src_path.map_or(true, |paths| {
            paths.split('|').any(|p| !p.is_empty() && file.contains(p))
        })
    };

    let show_progress = !quiet && !summary_only;
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
            Ok(obj_macho) => match LibraryCallGraph::build_from_object(&obj_macho, member_data) {
                Ok(obj_graph) => merged_graph.merge(obj_graph),
                Err(e) => {
                    if show_progress {
                        eprintln!(
                            "  Warning: Failed to build call graph for {}: {}",
                            member_name, e
                        );
                    }
                }
            },
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
        return AnalysisSummary::default();
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
        let is_panic_symbol = LIBRARY_PANIC_PATTERNS
            .iter()
            .any(|p| target_sym.contains(p))
            || target_sym.contains("core::panicking::")
            || target_sym.contains("std::panicking::");

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
        return AnalysisSummary::default();
    }

    // Build the summary from collected panic points
    let mut points: HashSet<(String, u32)> = HashSet::new();
    let mut files_affected: HashSet<String> = HashSet::new();

    // Sort for deterministic output
    let mut sorted_callers: Vec<_> = panic_callers.into_iter().collect();
    sorted_callers.sort_by(|a, b| (&a.file, a.line, &a.name).cmp(&(&b.file, b.line, &b.name)));

    // Collect points for summary
    for caller in &sorted_callers {
        points.insert((caller.file.clone(), caller.line));
        files_affected.insert(caller.file.clone());
    }

    // Print details if not summary-only
    if !summary_only {
        println!("\nPanic code points in library:");
        for caller in &sorted_callers {
            // Output in format expected by test framework: " --> file:line:col"
            println!(" --> {}:{}:1", caller.file, caller.line);
        }
    }

    if show_timings {
        eprintln!("  [timing] TOTAL: {:?}", total_start.elapsed());
    }

    AnalysisSummary::from_points(points, files_affected)
}

/// Analyze a workspace with multiple member crates.
/// Produces per-crate reports and an aggregate workspace summary.
fn analyze_workspace(members: &[WorkspaceMember], args: &Args) -> Result<(), Box<dyn Error>> {
    let workspace_root = std::env::current_dir()?;

    if !args.summary_only && !args.quiet {
        println!(
            "Analyzing workspace with {} member crate(s)...\n",
            members.len()
        );
    }

    let mut workspace_summary = AnalysisSummary::default();
    let mut crate_summaries: Vec<(String, AnalysisSummary)> = Vec::new();

    // Collect all member source paths for filtering
    // File paths in debug info are relative like "crate_a/src/main.rs"
    // Join patterns with "|" separator for is_in_crate to check
    // Include trailing "/" to match the format used in non-workspace mode
    // Use directory basenames (not package names) for source path matching
    // This handles cases where directory name differs from package name
    let workspace_root_name = workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let workspace_src_path = members
        .iter()
        .filter_map(|m| {
            let path_str = m.path.to_string_lossy();
            if path_str == "." {
                // Root member: use workspace directory name
                if workspace_root_name.is_empty() {
                    Some("src/".to_string())
                } else {
                    Some(format!("{}/src/", workspace_root_name))
                }
            } else {
                // Regular member: use directory basename
                m.path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|dir| format!("{}/src/", dir))
            }
        })
        .collect::<Vec<_>>()
        .join("|");

    for member in members {
        if !args.summary_only && !args.quiet {
            println!("=== {} ===", member.name);
        }

        let mut member_summary = AnalysisSummary::default();

        for binary_path in &member.binaries {
            let binary_path = binary_path
                .canonicalize()
                .unwrap_or_else(|_| binary_path.clone());

            if !args.summary_only && !args.quiet {
                println!("Processing {}", binary_path.display());
            }

            // Load configuration for this member crate
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

            // Check if this is a library and detect its type
            let is_dylib = binary_path.extension().is_some_and(|ext| ext == "dylib");
            if !args.summary_only
                && !args.quiet
                && is_dylib
                && let Some(lib_type) = detect_library_type(&binary_path)
            {
                println!("Library type: {}", lib_type);
            }

            let binary_buffer = fs::read(&binary_path)?;
            let symbols = read_symbols(&binary_buffer)?;

            match symbols {
                SymbolTable::MachO(Binary(macho)) => {
                    // Use workspace root path to include panics from all member crates
                    let summary = analyze_macho(
                        &macho,
                        &binary_buffer,
                        &binary_path,
                        Some(&workspace_src_path),
                        args.show_tree,
                        args.summary_only,
                        args.show_timings,
                        args.quiet,
                        args.no_hyperlinks,
                        &config,
                        Some(workspace_root.as_path()),
                    );
                    member_summary.add(&summary);
                }
                SymbolTable::MachO(Fat(multi_arch)) => {
                    if !args.summary_only {
                        println!("FAT: {:?} architectures", multi_arch.arches()?);
                    }
                }
                SymbolTable::Archive(archive) => {
                    // Use relocation-based analysis for consistent cross-object resolution
                    let summary = analyze_archive(
                        &archive,
                        &binary_buffer,
                        &binary_path,
                        Some(&workspace_src_path),
                        args.show_tree,
                        args.summary_only,
                        args.show_timings,
                        args.quiet,
                        args.no_hyperlinks,
                        &config,
                        Some(workspace_root.as_path()),
                    );
                    member_summary.add(&summary);
                }
            }
        }

        if !args.summary_only && !args.quiet {
            println!(
                "Panic points: {} in {} file(s)\n",
                member_summary.panic_points(),
                member_summary.files_affected()
            );
        }

        crate_summaries.push((member.name.clone(), member_summary.clone()));
        workspace_summary.add(&member_summary);
    }

    // Print workspace summary
    println!("=== Workspace Summary ===");
    println!("  Root: {}", workspace_root.display());
    println!("  Members analyzed: {}", members.len());
    for (name, summary) in &crate_summaries {
        println!(
            "    {}: {} panic point(s) in {} file(s)",
            name,
            summary.panic_points(),
            summary.files_affected()
        );
    }
    println!(
        "  Total panic points: {} across {} crate(s)",
        workspace_summary.panic_points(),
        members.len()
    );

    // Exit with the number of panic points found
    std::process::exit(workspace_summary.panic_points() as i32);
}
