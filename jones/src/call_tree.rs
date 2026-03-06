//! Call tree construction and processing.
//!
//! This module handles building and manipulating the call tree that traces
//! paths from panic symbols back to user code.

use crate::panic_cause::{PanicCause, detect_panic_cause};
use crate::sym::CallGraph;
use dashmap::DashSet;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A node in the call tree representing a function that can lead to the target symbol
#[derive(Debug)]
pub struct CallTreeNode {
    /// Symbol/function name
    pub name: String,
    /// Source file (if available from debug info)
    pub file: Option<String>,
    /// Line number (if available from debug info)
    pub line: Option<u32>,
    /// Functions that call this one
    pub callers: Vec<CallTreeNode>,
}

impl CallTreeNode {
    /// Create a new root node for a call tree
    pub fn new_root(name: String) -> Self {
        CallTreeNode {
            name,
            file: None,
            line: None,
            callers: Vec::new(),
        }
    }
}

/// Returns true if the node's source file matches the crate source path
pub fn is_in_crate(node: &CallTreeNode, crate_src_path: &str) -> bool {
    if let Some(file) = &node.file {
        file.contains(crate_src_path)
    } else {
        false
    }
}

/// Returns true if the node is in a panic cleanup path (not a user-initiated panic)
pub fn is_cleanup_panic_path(node: &CallTreeNode) -> bool {
    // These are internal panic paths, not user panic! calls
    node.name.contains("panic_nounwind")
        || node.name.contains("panic_in_cleanup")
        || node.name.contains("panic_cannot_unwind")
}

/// Prune branches that don't lead to a leaf node in the target crate's source.
/// Also removes panic cleanup paths (panic_nounwind, panic_in_cleanup) unless
/// show_drops is true.
/// Returns true if this node should be kept.
pub fn prune_call_tree(node: &mut CallTreeNode, crate_src_path: &str, show_drops: bool) -> bool {
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

/// Build a call tree by recursively finding callers of the target address.
/// Uses a thread-safe visited set to avoid infinite recursion when there are cycles.
/// Uses pre-computed CallGraph for O(1) lookups instead of re-scanning instructions.
/// Parallelizes exploration of top-level callers, with sequential recursion within each branch.
pub fn build_call_tree_parallel(
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
pub struct CrateCodePoint {
    pub name: String,
    pub file: String,
    pub line: u32,
    /// The detected cause of this panic path
    pub cause: Option<PanicCause>,
    /// Code points that this one calls (closer to panic in the call chain)
    pub children: Vec<CrateCodePoint>,
}

/// Key for identifying a code point: (file, line)
type CodePointKey = (String, u32);

/// Info stored for each code point: (function name, panic cause, set of child keys)
type CodePointInfo = (String, Option<PanicCause>, HashSet<CodePointKey>);

/// Map of code points: key -> info
type CodePointMap = HashMap<CodePointKey, CodePointInfo>;

/// Collect crate code points with hierarchy.
/// Returns a list of "root" code points (entry points) with their children.
pub fn collect_crate_code_points_hierarchical(
    node: &CallTreeNode,
    crate_src_path: &str,
) -> Vec<CrateCodePoint> {
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
        points
            .entry(key.clone())
            .or_insert_with(|| (node.name.clone(), detected_cause.clone(), HashSet::new()));

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
pub fn count_crate_code_points(node: &CallTreeNode, crate_src_path: &str) -> usize {
    let mut points = Vec::new();
    collect_crate_code_points(node, crate_src_path, &mut points);
    points.sort_by(|a, b| (&a.1, a.2).cmp(&(&b.1, b.2)));
    points.dedup();
    points.len()
}

/// Print only the crate code points without the full tree.
/// Returns the number of unique panic code points found.
pub fn print_crate_code_points(node: &CallTreeNode, crate_src_path: &str) -> usize {
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
pub fn print_call_tree(node: &CallTreeNode, depth: usize) {
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
