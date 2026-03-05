use crate::args::parse_args;
use crate::sym::{
    find_callers, find_callers_with_debug_info, find_symbol_address, find_symbol_containing, load_debug_info,
    read_symbols, DebugInfo, SymbolTable,
};
use goblin::mach::Mach::{Binary, Fat};
use goblin::mach::MachO;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::path::Path;

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

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();

    let parsed_args = parse_args(&args).unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        std::process::exit(255);
    });

    let mut total_panic_points: usize = 0;

    for binary_path in parsed_args.binaries {
        println!("Processing {}", binary_path.display());

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
    "rust_panic$",      // Main panic entry point (binaries)
    "panic_fmt$",       // Core panic formatting (libraries)
    "panic_display",    // Panic display helper
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

    // Create the root node for the call tree
    let mut root = CallTreeNode {
        name: demangled.clone(),
        file: None,
        line: None,
        callers: Vec::new(),
    };

    // Track visited addresses to avoid infinite recursion
    let mut visited = HashSet::new();
    visited.insert(target_addr);

    build_call_tree(
        macho,
        buffer,
        &debug_info,
        target_addr,
        &mut root,
        &mut visited,
        crate_src_path,
    );

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
/// Uses a visited set to avoid infinite recursion when there are cycles.
fn build_call_tree(
    binary_macho: &goblin::mach::MachO,
    binary_buffer: &[u8],
    debug_source: &DebugInfo,
    target_addr: u64,
    node: &mut CallTreeNode,
    visited: &mut HashSet<u64>,
    crate_src_path: Option<&str>,
) {
    let callers = match debug_source {
        DebugInfo::Embedded => {
            // Binary and debug are the same
            find_callers_with_debug_info(
                binary_macho,
                binary_buffer,
                binary_macho,
                binary_buffer,
                target_addr,
                crate_src_path,
            )
            .unwrap()
        }
        DebugInfo::DSym(dsym_info) => {
            // Binary for code, dSYM for debug info
            dsym_info.with_debug_macho(|debug_macho| {
                if let Binary(macho) = debug_macho {
                    find_callers_with_debug_info(
                        binary_macho,
                        binary_buffer,
                        macho,
                        dsym_info.borrow_debug_buffer(),
                        target_addr,
                        crate_src_path,
                    )
                    .unwrap()
                } else {
                    find_callers(binary_macho, binary_buffer, target_addr).unwrap()
                }
            })
        }
        DebugInfo::None => {
            // No debug info, use symbol table only
            find_callers(binary_macho, binary_buffer, target_addr).unwrap()
        }
    };

    for caller_info in callers {
        let caller_addr = caller_info.caller.start_address;

        // Create a new node for this caller
        // Use the function's declaration file for crate identification,
        // falling back to the call site file if not available
        let file = caller_info.caller.file.clone().or(caller_info.file.clone());
        let mut caller_node = CallTreeNode {
            name: caller_info.caller.name.clone(),
            file,
            line: caller_info.line,
            callers: Vec::new(),
        };

        // Only recurse if we haven't visited this address before
        if !visited.contains(&caller_addr) {
            visited.insert(caller_addr);
            build_call_tree(
                binary_macho,
                binary_buffer,
                debug_source,
                caller_addr,
                &mut caller_node,
                visited,
                crate_src_path,
            );
        }

        node.callers.push(caller_node);
    }
}

/// Collect all crate code points from the tree (nodes whose source is in the crate)
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
    let mut points = Vec::new();
    collect_crate_code_points(node, crate_src_path, &mut points);

    // Sort by file then line number for readable output
    points.sort_by(|a, b| (&a.1, a.2).cmp(&(&b.1, b.2)));

    // Remove duplicates
    points.dedup();

    let count = points.len();
    if count == 0 {
        println!("\nNo panics in crate");
    } else {
        println!("\nPanic code points in crate:");
        for (name, file, line) in &points {
            println!("  {}:{} in '{}'", file, line, name);
        }
    }
    count
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
