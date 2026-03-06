use crate::args::parse_args;
use crate::call_tree::{
    CallTreeNode, build_call_tree_parallel, count_crate_code_points, print_call_tree,
    print_crate_code_points, prune_call_tree,
};
use crate::cargo::{derive_crate_src_path, detect_library_type, find_project_root};
use crate::config::Config;
use crate::sym::{
    CallGraph, DebugInfo, SymbolTable, find_symbol_address, find_symbol_containing,
    load_debug_info, read_symbols,
};
use dashmap::DashSet;
use goblin::mach::Mach::{Binary, Fat};
use goblin::mach::MachO;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::sync::Arc;

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

    let mut total_panic_points: usize = 0;

    for binary_path in &parsed_args.binaries {
        // Canonicalize the binary path to ensure absolute paths for clickable links
        let binary_path = binary_path
            .canonicalize()
            .unwrap_or_else(|_| binary_path.clone());
        println!("Processing {}", binary_path.display());

        // Find project root from binary path for config loading and absolute paths
        let project_root = find_project_root(&binary_path);

        // Load configuration per-crate
        // If no project root found, use defaults plus any explicit --config only
        let config = if let Some(ref root) = project_root {
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
        if is_dylib && let Some(lib_type) = detect_library_type(&binary_path) {
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

        match symbols {
            SymbolTable::MachO(Binary(macho)) => {
                let crate_src_path = derive_crate_src_path(&binary_path);
                total_panic_points += analyze_macho(
                    &macho,
                    &binary_buffer,
                    &binary_path,
                    crate_src_path.as_deref(),
                    parsed_args.show_tree,
                    &config,
                    project_root.as_deref(),
                );
            }
            SymbolTable::MachO(Fat(multi_arch)) => {
                println!("FAT: {:?} architectures", multi_arch.arches().unwrap());
            }
            SymbolTable::Archive(archive) => {
                // Process each object file in the archive
                let crate_src_path = derive_crate_src_path(&binary_path);

                for member_name in archive.members() {
                    // Skip non-object files (like .rmeta)
                    if !member_name.ends_with(".o") {
                        continue;
                    }

                    // Extract the member data
                    let Ok(member_data) = archive.extract(member_name, &binary_buffer) else {
                        continue;
                    };

                    // Parse the object file as Mach-O
                    if let Ok(obj_macho) = MachO::parse(member_data, 0) {
                        total_panic_points += analyze_macho(
                            &obj_macho,
                            member_data,
                            &binary_path,
                            crate_src_path.as_deref(),
                            parsed_args.show_tree,
                            &config,
                            project_root.as_deref(),
                        );
                    }
                }
            }
        }

        println!();
    }

    // Exit with the number of panic points found (0 = passed, >0 = found panics)
    // Note: Unix exit codes are 8-bit (0-255), the values above wrap around
    std::process::exit(total_panic_points as i32);
}

/// Panic symbol patterns to search for, in order of preference.
/// For binaries, rust_panic$ is the root. For libraries, we look for
/// the functions that call into the panic runtime.
const PANIC_SYMBOL_PATTERNS: &[&str] = &[
    "rust_panic$",   // Main panic entry point (binaries)
    "panic_fmt$",    // Core panic formatting (libraries)
    "panic_display", // Panic display helper
];

/// Analyze a single MachO binary/object for panic points.
/// Returns the number of panic code points found.
fn analyze_macho(
    macho: &MachO,
    buffer: &[u8],
    binary_path: &Path,
    crate_src_path: Option<&str>,
    show_tree: bool,
    config: &Config,
    project_root: Option<&Path>,
) -> usize {
    // Try each panic symbol pattern until we find one
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

    let Some(_) = panic_symbol else {
        // No panic symbols found in this object
        return 0;
    };

    let debug_info = load_debug_info(macho, binary_path);

    // Pre-compute call graph by scanning all instructions once
    // Use debug info variant for source file/line enrichment
    let call_graph = match &debug_info {
        DebugInfo::Embedded => {
            CallGraph::build_with_debug_info(macho, buffer, macho, buffer, crate_src_path)
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

    // Create the root node for the call tree
    let mut root = CallTreeNode::new_root(demangled.clone());

    // Track visited addresses to avoid infinite recursion (thread-safe)
    let visited = Arc::new(DashSet::new());
    visited.insert(target_addr);

    // Build the call tree in parallel
    root.callers = build_call_tree_parallel(&call_graph, target_addr, &visited);

    // Prune to only show paths leading to user code
    if let Some(crate_path) = crate_src_path {
        prune_call_tree(&mut root, crate_path, config);
    }

    // Print output based on --tree flag
    if show_tree {
        println!("Full call tree:");
        print_call_tree(&root, 0);
        crate_src_path.map_or(0, |cp| count_crate_code_points(&root, cp))
    } else if let Some(crate_path) = crate_src_path {
        print_crate_code_points(&root, crate_path, project_root)
    } else {
        println!("Could not determine crate source path, showing full tree");
        print_call_tree(&root, 0);
        0
    }
}
