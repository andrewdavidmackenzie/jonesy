use crate::args::parse_args;
use crate::sym::{
    CallGraph, DebugInfo, SymbolTable, find_symbol_address, find_symbol_containing,
    load_debug_info, read_symbols,
};
use cargo_toml::Manifest;
use dashmap::DashSet;
use goblin::mach::Mach::{Binary, Fat};
use goblin::mach::MachO;
use rayon::prelude::*;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::sync::Arc;

mod args;
#[cfg(target_os = "macos")]
mod sym;

/// A node in the call tree representing a function that can lead to the target symbol
#[derive(Debug)]
struct CallTreeNode {
    /// Symbol/function name
    name: String,
    /// Source file (if available from debug info)
    file: Option<String>,
    /// Line number (if available from debug info)
    line: Option<u32>,
    /// Functions that call this one
    callers: Vec<CallTreeNode>,
}

/// Returns true if the node's source file matches the crate source path
fn is_in_crate(node: &CallTreeNode, crate_src_path: &str) -> bool {
    if let Some(file) = &node.file {
        file.contains(crate_src_path)
    } else {
        false
    }
}

/// Returns true if the node is in a panic cleanup path (not a user-initiated panic)
fn is_cleanup_panic_path(node: &CallTreeNode) -> bool {
    // These are internal panic paths, not user panic! calls
    node.name.contains("panic_nounwind")
        || node.name.contains("panic_in_cleanup")
        || node.name.contains("panic_cannot_unwind")
}

/// Known panic causes with explanations and suggestions
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(dead_code)] // Some variants reserved for future detection
enum PanicCause {
    /// Explicit panic!() macro
    ExplicitPanic,
    /// Array or slice index out of bounds
    BoundsCheck,
    /// Arithmetic overflow (add, sub, mul, div, rem, neg)
    ArithmeticOverflow(String),
    /// Shift overflow (shl, shr)
    ShiftOverflow(String),
    /// Division by zero
    DivisionByZero,
    /// Unwrap on None
    UnwrapNone,
    /// Unwrap on Err
    UnwrapErr,
    /// Expect on None
    ExpectNone,
    /// Expect on Err
    ExpectErr,
    /// Assert failed
    AssertFailed,
    /// Debug assert failed
    DebugAssertFailed,
    /// Unreachable code reached
    Unreachable,
    /// Unimplemented code reached
    Unimplemented,
    /// Todo macro reached
    Todo,
    /// Unknown cause
    Unknown,
}

impl PanicCause {
    /// Get a short description of the panic cause
    fn description(&self) -> &'static str {
        match self {
            PanicCause::ExplicitPanic => "explicit panic!() call",
            PanicCause::BoundsCheck => "index out of bounds",
            PanicCause::ArithmeticOverflow(_) => "arithmetic overflow",
            PanicCause::ShiftOverflow(_) => "shift overflow",
            PanicCause::DivisionByZero => "division by zero",
            PanicCause::UnwrapNone => "unwrap() on None",
            PanicCause::UnwrapErr => "unwrap() on Err",
            PanicCause::ExpectNone => "expect() on None",
            PanicCause::ExpectErr => "expect() on Err",
            PanicCause::AssertFailed => "assertion failed",
            PanicCause::DebugAssertFailed => "debug assertion failed",
            PanicCause::Unreachable => "unreachable!() reached",
            PanicCause::Unimplemented => "unimplemented!() reached",
            PanicCause::Todo => "todo!() reached",
            PanicCause::Unknown => "unknown cause",
        }
    }

    /// Get a suggestion for how to avoid this panic
    fn suggestion(&self) -> &'static str {
        match self {
            PanicCause::ExplicitPanic => "Review if panic is intentional or add error handling",
            PanicCause::BoundsCheck => "Use .get() for safe access or validate index before use",
            PanicCause::ArithmeticOverflow(_) => {
                "Use checked_*, saturating_*, or wrapping_* methods"
            }
            PanicCause::ShiftOverflow(_) => "Validate shift amount is within valid range",
            PanicCause::DivisionByZero => "Check divisor is non-zero before division",
            PanicCause::UnwrapNone => "Use if let, match, unwrap_or, or ? operator instead",
            PanicCause::UnwrapErr => "Use if let, match, unwrap_or, or ? operator instead",
            PanicCause::ExpectNone => "Use if let, match, unwrap_or, or ? operator instead",
            PanicCause::ExpectErr => "Use if let, match, unwrap_or, or ? operator instead",
            PanicCause::AssertFailed => "Review assertion condition",
            PanicCause::DebugAssertFailed => "Review debug assertion condition",
            PanicCause::Unreachable => "Ensure code path is truly unreachable",
            PanicCause::Unimplemented => "Implement the missing functionality",
            PanicCause::Todo => "Complete the TODO implementation",
            PanicCause::Unknown => "",
        }
    }
}

/// Detect panic cause from a function name in the call chain
fn detect_panic_cause(func_name: &str) -> Option<PanicCause> {
    // Check for specific panic functions
    if func_name.contains("panic_bounds_check") {
        return Some(PanicCause::BoundsCheck);
    }
    if func_name.contains("panic_const_add_overflow") {
        return Some(PanicCause::ArithmeticOverflow("addition".to_string()));
    }
    if func_name.contains("panic_const_sub_overflow") {
        return Some(PanicCause::ArithmeticOverflow("subtraction".to_string()));
    }
    if func_name.contains("panic_const_mul_overflow") {
        return Some(PanicCause::ArithmeticOverflow("multiplication".to_string()));
    }
    if func_name.contains("panic_const_div_overflow") {
        return Some(PanicCause::ArithmeticOverflow("division".to_string()));
    }
    if func_name.contains("panic_const_rem_overflow") {
        return Some(PanicCause::ArithmeticOverflow("remainder".to_string()));
    }
    if func_name.contains("panic_const_neg_overflow") {
        return Some(PanicCause::ArithmeticOverflow("negation".to_string()));
    }
    if func_name.contains("panic_const_shl_overflow") {
        return Some(PanicCause::ShiftOverflow("left".to_string()));
    }
    if func_name.contains("panic_const_shr_overflow") {
        return Some(PanicCause::ShiftOverflow("right".to_string()));
    }
    if func_name.contains("panic_const_div_by_zero") {
        return Some(PanicCause::DivisionByZero);
    }
    if func_name.contains("panic_const_rem_by_zero") {
        return Some(PanicCause::DivisionByZero);
    }
    // unwrap/expect detection
    if func_name.contains("unwrap_failed") {
        // Could be Option or Result - context would tell us which
        return Some(PanicCause::UnwrapNone);
    }
    if func_name.contains("expect_failed") {
        return Some(PanicCause::ExpectNone);
    }
    // Assert macros
    if func_name.contains("assert_failed") {
        return Some(PanicCause::AssertFailed);
    }
    // panic_display is explicit panic! with a simple message
    if func_name.contains("panic_display") {
        return Some(PanicCause::ExplicitPanic);
    }
    // Check for unreachable/unimplemented/todo patterns
    if func_name.contains("unreachable") && func_name.contains("panic") {
        return Some(PanicCause::Unreachable);
    }
    // panic_fmt is the core panic function - if we reach here without a more
    // specific match, it's likely an explicit panic!() call
    if func_name.contains("panic_fmt") {
        return Some(PanicCause::ExplicitPanic);
    }

    None
}

/// Prune branches that don't lead to a leaf node in the target crate's source.
/// Also removes panic cleanup paths (panic_nounwind, panic_in_cleanup) unless
/// show_drops is true.
/// Returns true if this node should be kept.
fn prune_call_tree(node: &mut CallTreeNode, crate_src_path: &str, show_drops: bool) -> bool {
    // Remove cleanup panic paths unless the --drops option is specified
    if !show_drops && is_cleanup_panic_path(node) {
        return false;
    }

    // Recursively prune children first
    node.callers
        .retain_mut(|caller| prune_call_tree(caller, crate_src_path, show_drops));

    // Keep this node if:
    // 1. It's a leaf AND in the crate source, OR
    // 2. It still has children after pruning (meaning it leads to crate code)
    if node.callers.is_empty() {
        // Leaf node: keep only if it's in the crate source
        is_in_crate(node, crate_src_path)
    } else {
        // Has children that lead to crate code, so keep it
        true
    }
}

/// Try to derive the crate source path from the binary path.
/// For a binary at "target/panic/panic", looks for the source in common locations.
/// For libraries like "liblibrary.rlib", strips the "lib" prefix.
fn derive_crate_src_path(binary_path: &Path) -> Option<String> {
    // Get the binary name (e.g. "panic" from "target/panic/panic")
    let file_stem = binary_path.file_stem()?.to_str()?;

    // For libraries, strip "lib" prefix (e.g., "liblibrary" -> "library")
    let binary_name = file_stem.strip_prefix("lib").unwrap_or(file_stem);

    // Common patterns:
    // 1. examples/<name>/src/ for crates in examples
    // 2. <name>/src/ for workspace members
    // 3. src/ for the main crate

    // Try to find the workspace root by looking for Cargo.toml
    let mut current = binary_path.parent();
    while let Some(dir) = current {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            // Check for examples/<binary_name>/src/
            let example_src = dir.join("examples").join(binary_name).join("src");
            if example_src.exists() {
                return Some(format!("examples/{}/src/", binary_name));
            }

            // Check for <binary_name>/src/
            let member_src = dir.join(binary_name).join("src");
            if member_src.exists() {
                return Some(format!("{}/src/", binary_name));
            }

            // For libraries, the directory name may not match the lib name.
            // Search workspace members to find the matching lib.
            if let Some(path) = find_lib_src_path(dir, binary_name) {
                return Some(path);
            }

            // Check for src/ in the workspace root
            let root_src = dir.join("src");
            if root_src.exists() {
                return Some("src/".to_string());
            }
        }
        current = dir.parent();
    }

    None
}

/// Search workspace members to find the source path for a library by its name.
/// Returns the relative path to the src directory (e.g., "examples/cdylib/src/").
fn find_lib_src_path(workspace_root: &Path, lib_name: &str) -> Option<String> {
    let cargo_toml = workspace_root.join("Cargo.toml");
    let content = fs::read_to_string(&cargo_toml).ok()?;
    let manifest = Manifest::from_slice(content.as_bytes()).ok()?;

    let workspace = manifest.workspace.as_ref()?;

    for member_pattern in &workspace.members {
        // Handle glob patterns
        let member_paths: Vec<_> = if member_pattern.contains('*') {
            let base = member_pattern.trim_end_matches("/*");
            let base_path = workspace_root.join(base);
            if base_path.is_dir() {
                fs::read_dir(&base_path)
                    .ok()?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.path())
                    .collect()
            } else {
                vec![]
            }
        } else {
            vec![workspace_root.join(member_pattern)]
        };

        for member_path in member_paths {
            let member_cargo_toml = member_path.join("Cargo.toml");
            if !member_cargo_toml.exists() {
                continue;
            }

            if let Ok(member_content) = fs::read_to_string(&member_cargo_toml)
                && let Ok(member_manifest) = Manifest::from_slice(member_content.as_bytes())
                && let Some(lib) = &member_manifest.lib
            {
                let manifest_lib_name = lib
                    .name
                    .clone()
                    .or_else(|| member_manifest.package.as_ref().map(|p| p.name.clone()))
                    .unwrap_or_default();

                // Check if this lib matches the target name
                if manifest_lib_name == lib_name || manifest_lib_name.replace('-', "_") == lib_name
                {
                    // Return relative path from workspace root
                    if let Ok(rel_path) = member_path.strip_prefix(workspace_root) {
                        return Some(format!("{}/src/", rel_path.display()));
                    }
                }
            }
        }
    }

    None
}

/// Detect if a library is a cdylib or dylib by checking Cargo.toml
/// Returns Some("cdylib"), Some("dylib"), or None if not determinable
fn detect_library_type(binary_path: &Path) -> Option<String> {
    // Extract library name from path (e.g., "liblibrary.dylib" -> "library")
    let file_stem = binary_path.file_stem()?.to_str()?;
    let lib_name = file_stem.strip_prefix("lib")?;

    // Walk up to find Cargo.toml
    let mut current = binary_path.parent();
    while let Some(dir) = current {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists()
            && let Ok(content) = fs::read_to_string(&cargo_toml)
            && let Ok(manifest) = Manifest::from_slice(content.as_bytes())
        {
            // Check if this is a workspace, look for member with matching lib name
            if let Some(workspace) = &manifest.workspace {
                for member in &workspace.members {
                    let member_path = dir.join(member);
                    if let Some(lib_type) = check_member_lib_type(&member_path, lib_name) {
                        return Some(lib_type);
                    }
                }
            }

            // Check if this manifest has a matching lib
            if let Some(lib) = &manifest.lib {
                let manifest_lib_name = lib
                    .name
                    .clone()
                    .or_else(|| manifest.package.as_ref().map(|p| p.name.clone()))
                    .unwrap_or_default();

                if manifest_lib_name == lib_name || manifest_lib_name.replace('-', "_") == lib_name
                {
                    // Check crate types
                    for crate_type in &lib.crate_type {
                        if crate_type == "cdylib" {
                            return Some("cdylib".to_string());
                        }
                        if crate_type == "dylib" {
                            return Some("dylib".to_string());
                        }
                    }
                }
            }
        }
        current = dir.parent();
    }
    None
}

/// Check a workspace member for matching library type
fn check_member_lib_type(member_path: &Path, lib_name: &str) -> Option<String> {
    let cargo_toml = member_path.join("Cargo.toml");
    if !cargo_toml.exists() {
        return None;
    }

    let content = fs::read_to_string(&cargo_toml).ok()?;
    let manifest = Manifest::from_slice(content.as_bytes()).ok()?;

    if let Some(lib) = &manifest.lib {
        let manifest_lib_name = lib
            .name
            .clone()
            .or_else(|| manifest.package.as_ref().map(|p| p.name.clone()))
            .unwrap_or_default();

        if manifest_lib_name == lib_name || manifest_lib_name.replace('-', "_") == lib_name {
            for crate_type in &lib.crate_type {
                if crate_type == "cdylib" {
                    return Some("cdylib".to_string());
                }
                if crate_type == "dylib" {
                    return Some("dylib".to_string());
                }
            }
        }
    }
    None
}

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

    for binary_path in parsed_args.binaries {
        println!("Processing {}", binary_path.display());

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
                    parsed_args.show_drops,
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
                            parsed_args.show_drops,
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
    show_drops: bool,
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
    let mut root = CallTreeNode {
        name: demangled.clone(),
        file: None,
        line: None,
        callers: Vec::new(),
    };

    // Track visited addresses to avoid infinite recursion (thread-safe)
    let visited = Arc::new(DashSet::new());
    visited.insert(target_addr);

    // Build the call tree in parallel
    root.callers = build_call_tree_parallel(&call_graph, target_addr, &visited);

    // Prune to only show paths leading to user code
    if let Some(crate_path) = crate_src_path {
        prune_call_tree(&mut root, crate_path, show_drops);
    }

    // Print output based on --tree flag
    if show_tree {
        println!("Full call tree:");
        print_call_tree(&root, 0);
        crate_src_path.map_or(0, |cp| count_crate_code_points(&root, cp))
    } else if let Some(crate_path) = crate_src_path {
        print_crate_code_points(&root, crate_path)
    } else {
        println!("Could not determine crate source path, showing full tree");
        print_call_tree(&root, 0);
        0
    }
}

/// Build a call tree by recursively finding callers of the target address.
/// Uses a thread-safe visited set to avoid infinite recursion when there are cycles.
/// Uses pre-computed CallGraph for O(1) lookups instead of re-scanning instructions.
/// Parallelizes exploration of top-level callers, with sequential recursion within each branch.
fn build_call_tree_parallel(
    call_graph: &CallGraph,
    target_addr: u64,
    visited: &Arc<DashSet<u64>>,
) -> Vec<CallTreeNode> {
    // Use pre-computed call graph for O(1) lookup
    let callers = call_graph.get_callers(target_addr);

    // Process callers in parallel at this level.
    // Note: We intentionally share visited across all branches. This means if function C
    // is called from both A→C and B→C paths, only one branch will recurse into C's callers.
    // This is correct because:
    // 1. All caller nodes are still added to the tree (node creation is unconditional)
    // 2. We only skip redundant exploration of the same subtree
    // 3. The set of leaf code points found is the same regardless of which branch explores C
    callers
        .into_par_iter()
        .filter_map(|caller_info| {
            let caller_addr = caller_info.caller.start_address;

            // Atomically try to insert - if already present, skip recursion but still create node
            let should_recurse = visited.insert(caller_addr);

            // Create a new node for this caller
            // Use the function's declaration file for crate identification,
            // falling back to the call site file if not available
            let file = caller_info.caller.file.clone().or(caller_info.file.clone());
            let child_callers = if should_recurse {
                // Use sequential recursion within each branch to ensure deterministic behavior
                build_call_tree_sequential(call_graph, caller_addr, visited)
            } else {
                Vec::new()
            };

            Some(CallTreeNode {
                name: caller_info.caller.name.clone(),
                file,
                line: caller_info.line,
                callers: child_callers,
            })
        })
        .collect()
}

/// Sequential version for recursion within parallel branches.
fn build_call_tree_sequential(
    call_graph: &CallGraph,
    target_addr: u64,
    visited: &Arc<DashSet<u64>>,
) -> Vec<CallTreeNode> {
    let callers = call_graph.get_callers(target_addr);

    callers
        .into_iter()
        .map(|caller_info| {
            let caller_addr = caller_info.caller.start_address;
            let should_recurse = visited.insert(caller_addr);

            let file = caller_info.caller.file.clone().or(caller_info.file.clone());
            let child_callers = if should_recurse {
                build_call_tree_sequential(call_graph, caller_addr, visited)
            } else {
                Vec::new()
            };

            CallTreeNode {
                name: caller_info.caller.name.clone(),
                file,
                line: caller_info.line,
                callers: child_callers,
            }
        })
        .collect()
}

/// A crate code point with its hierarchical children (code points it calls toward panic)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CrateCodePoint {
    name: String,
    file: String,
    line: u32,
    /// The detected cause of this panic path
    cause: Option<PanicCause>,
    /// Code points that this one calls (closer to panic in the call chain)
    children: Vec<CrateCodePoint>,
}

/// Key for identifying a code point: (file, line)
type CodePointKey = (String, u32);

/// Info stored for each code point: (function name, panic cause, set of child keys)
type CodePointInfo = (
    String,
    Option<PanicCause>,
    std::collections::HashSet<CodePointKey>,
);

/// Map of code points: key -> info
type CodePointMap = std::collections::HashMap<CodePointKey, CodePointInfo>;

/// Collect crate code points with hierarchy.
/// Returns a list of "root" code points (entry points) with their children.
fn collect_crate_code_points_hierarchical(
    node: &CallTreeNode,
    crate_src_path: &str,
) -> Vec<CrateCodePoint> {
    use std::collections::HashSet;

    // First pass: collect all crate code points and their relationships
    // Map: (file, line) -> (name, cause, set of child keys)
    let mut points: CodePointMap = CodePointMap::new();

    collect_crate_relationships(node, crate_src_path, &mut points, None, None);

    // Find roots: points that are not in any other point's children
    let all_children: HashSet<(String, u32)> = points
        .values()
        .flat_map(|(_, _, children)| children.iter().cloned())
        .collect();

    let mut roots: Vec<CodePointKey> = points
        .keys()
        .filter(|k| !all_children.contains(*k))
        .cloned()
        .collect();

    // Cyclic relationship fallback: still emit collected points
    if roots.is_empty() && !points.is_empty() {
        roots = points.keys().cloned().collect();
        roots.sort();
    }

    // Build tree from roots
    fn build_subtree(
        key: &CodePointKey,
        points: &CodePointMap,
        path: &mut HashSet<CodePointKey>,
    ) -> Option<CrateCodePoint> {
        // Prevent cycles only on current DFS path (not across sibling/root branches)
        if !path.insert(key.clone()) {
            return None;
        }

        let (name, cause, child_keys_set) = points.get(key)?;
        // Sort child keys for deterministic output
        let mut child_keys: Vec<_> = child_keys_set.iter().cloned().collect();
        child_keys.sort();
        let mut children: Vec<CrateCodePoint> = child_keys
            .iter()
            .filter_map(|child_key| build_subtree(child_key, points, path))
            .collect();
        children.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
        path.remove(key);

        Some(CrateCodePoint {
            name: name.clone(),
            file: key.0.clone(),
            line: key.1,
            cause: cause.clone(),
            children,
        })
    }

    roots
        .iter()
        .filter_map(|root| build_subtree(root, &points, &mut HashSet::new()))
        .collect()
}

/// Collect crate code point relationships by walking the call tree.
/// For each crate code point, records which other crate code points it "calls"
/// (i.e., are closer to panic in the call chain).
///
/// In the CallTreeNode tree, "callers" are functions that CALL this node.
/// So if A.callers contains B, then B calls A.
/// For our output hierarchy: B is the parent (entry point), A is the child (closer to panic).
///
/// Also detects panic causes from function names in the call path.
fn collect_crate_relationships(
    node: &CallTreeNode,
    crate_src_path: &str,
    points: &mut CodePointMap,
    child_crate_key: Option<CodePointKey>,
    current_cause: Option<PanicCause>,
) {
    // Try to detect panic cause from this node's function name
    let detected_cause = detect_panic_cause(&node.name).or(current_cause);

    let node_key = if let (Some(file), Some(line)) = (&node.file, &node.line)
        && file.contains(crate_src_path)
        && *line > 0
    {
        Some((file.clone(), *line))
    } else {
        None
    };

    if let Some(key) = &node_key {
        // Ensure this point exists in the map with detected cause
        points.entry(key.clone()).or_insert_with(|| {
            (
                node.name.clone(),
                detected_cause.clone(),
                std::collections::HashSet::new(),
            )
        });

        // If there's a child crate code point (closer to panic), add it as a child of this node
        if let Some(child_key) = &child_crate_key
            && let Some((_, _, children)) = points.get_mut(key)
        {
            children.insert(child_key.clone());
        }
    }

    // When recursing to callers, the current node becomes the child
    // (since callers are further from panic, they are parents in our hierarchy)
    let next_child = node_key.or(child_crate_key);

    for caller in &node.callers {
        collect_crate_relationships(
            caller,
            crate_src_path,
            points,
            next_child.clone(),
            detected_cause.clone(),
        );
    }
}

/// Collect all crate code points from the tree (nodes whose source is in the crate)
/// Flat collection for backward compatibility (used by count_crate_code_points)
fn collect_crate_code_points(
    node: &CallTreeNode,
    crate_src_path: &str,
    points: &mut Vec<(String, String, u32)>,
) {
    // Add this node if it's in the crate source
    if let (Some(file), Some(line)) = (&node.file, &node.line)
        && file.contains(crate_src_path)
    {
        points.push((node.name.clone(), file.clone(), *line));
    }
    // Recurse to children
    for caller in &node.callers {
        collect_crate_code_points(caller, crate_src_path, points);
    }
}

/// Count unique crate code points without printing
fn count_crate_code_points(node: &CallTreeNode, crate_src_path: &str) -> usize {
    let mut points = Vec::new();
    collect_crate_code_points(node, crate_src_path, &mut points);
    points.sort_by(|a, b| (&a.1, a.2).cmp(&(&b.1, b.2)));
    points.dedup();
    points.len()
}

/// Print only the crate code points without the full tree.
/// Returns the number of unique panic code points found.
fn print_crate_code_points(node: &CallTreeNode, crate_src_path: &str) -> usize {
    let mut roots = collect_crate_code_points_hierarchical(node, crate_src_path);

    // Deduplicate roots by (file, line)
    dedupe_crate_points(&mut roots);

    // Sort roots by file then line number
    roots.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

    let count = count_crate_points(&roots);
    if count == 0 {
        println!("\nNo panics in crate");
    } else {
        println!("\nPanic code points in crate:");
        for point in &roots {
            print_crate_point(point, "", true, true);
        }
    }
    count
}

/// Count total crate code points in the hierarchy
fn count_crate_points(points: &[CrateCodePoint]) -> usize {
    points
        .iter()
        .map(|p| 1 + count_crate_points(&p.children))
        .sum()
}

/// Print a crate code point with tree-style indentation
/// Uses rustc-style " --> file:line:column" format for terminal-clickable links
fn print_crate_point(point: &CrateCodePoint, prefix: &str, is_last: bool, is_root: bool) {
    // Format like rustc/clippy: " --> file:line:column" which is widely recognized as clickable
    let location = format!("{}:{}:1", point.file, point.line);

    // Only show cause and help on leaf nodes (no children)
    let is_leaf = point.children.is_empty();

    // Format cause description if available (only for leaf nodes)
    let cause_str = if is_leaf {
        if let Some(cause) = &point.cause {
            format!(" [{}]", cause.description())
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Print the current node - location on its own line for clickability
    if is_root {
        println!(" --> {}{}", location, cause_str);
        // Print suggestion if we have a cause (only for leaf nodes)
        if is_leaf && let Some(cause) = &point.cause {
            let suggestion = cause.suggestion();
            if !suggestion.is_empty() {
                println!("     = help: {}", suggestion);
            }
        }
    } else {
        let connector = if is_last { "└── " } else { "├── " };
        // Indent to align with parent, show tree connector, then clickable location
        println!("     {}{} --> {}{}", prefix, connector, location, cause_str);
        // Print suggestion if we have a cause (only for leaf nodes)
        if is_leaf && let Some(cause) = &point.cause {
            let suggestion = cause.suggestion();
            if !suggestion.is_empty() {
                println!("     {}     = help: {}", prefix, suggestion);
            }
        }
    }

    // Sort and print children
    let mut children = point.children.clone();
    children.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

    let child_count = children.len();
    for (i, child) in children.iter().enumerate() {
        let is_last_child = i == child_count - 1;
        // Build the prefix for children
        let child_prefix = if is_root {
            String::new()
        } else if is_last {
            format!("{}    ", prefix)
        } else {
            format!("{}│   ", prefix)
        };
        print_crate_point(child, &child_prefix, is_last_child, false);
    }
}

/// Deduplicate crate code points by (file, line), merging children
fn dedupe_crate_points(points: &mut Vec<CrateCodePoint>) {
    use std::collections::HashMap;

    // Group by (file, line)
    let mut seen: HashMap<(String, u32), usize> = HashMap::new();
    let mut result: Vec<CrateCodePoint> = Vec::new();

    for point in points.drain(..) {
        let key = (point.file.clone(), point.line);
        if let Some(&idx) = seen.get(&key) {
            // Merge children into existing point
            result[idx].children.extend(point.children);
        } else {
            seen.insert(key, result.len());
            result.push(point);
        }
    }

    // Recursively dedupe children
    for point in &mut result {
        dedupe_crate_points(&mut point.children);
    }

    *points = result;
}

/// Print the call tree with indentation
fn print_call_tree(node: &CallTreeNode, depth: usize) {
    let indent = "    ".repeat(depth);

    if depth == 0 {
        // Root node (the panic symbol)
        println!("{}{}", indent, node.name);
    }

    for caller in &node.callers {
        match (&caller.file, &caller.line) {
            (Some(filename), Some(line)) => {
                println!(
                    "{}Called from: '{}' (source: {}:{})",
                    indent, caller.name, filename, line
                );
            }
            (Some(filename), None) => {
                println!(
                    "{}Called from: '{}' (source: {})",
                    indent, caller.name, filename
                );
            }
            _ => {
                println!("{}Called from: '{}'", indent, caller.name);
            }
        }
        print_call_tree(caller, depth + 1);
    }
}
