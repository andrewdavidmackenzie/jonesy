//! Call tree construction and processing.
//!
//! This module handles building and manipulating the call tree that traces
//! paths from panic symbols back to user code.

use crate::config::Config;
use crate::panic_cause::{PanicCause, detect_panic_cause};
use crate::sym::CallGraph;
use dashmap::DashSet;
use is_terminal::IsTerminal;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;
use std::sync::Arc;
use url::Url;

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

/// Prune branches that don't lead to a leaf node in the target crate's source.
/// Note: Allowed cause filtering is done during code point collection, not here,
/// to avoid incorrectly pruning shared subtrees that are reachable via denied causes.
/// Returns true if this node should be kept.
pub fn prune_call_tree(node: &mut CallTreeNode, crate_src_path: &str) -> bool {
    // Recursively prune children first
    node.callers
        .retain_mut(|caller| prune_call_tree(caller, crate_src_path));

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
    // Note: We share visited across all branches to avoid exponential blowup.
    // This means shared subtrees are only explored once, but all nodes are still created.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateCodePoint {
    pub name: String,
    pub file: String,
    pub line: u32,
    /// All detected causes of panic paths through this point
    pub causes: HashSet<PanicCause>,
    /// Code points that this one calls (closer to panic in the call chain)
    pub children: Vec<CrateCodePoint>,
}

/// Key for identifying a code point: (file, line)
type CodePointKey = (String, u32);

/// Info stored for each code point: (function name, set of causes, set of child keys)
type CodePointInfo = (String, HashSet<PanicCause>, HashSet<CodePointKey>);

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

        let (name, causes, child_keys_set) = points.get(key)?;
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
            causes: causes.clone(),
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
        // Ensure this point exists in the map and accumulate all causes
        let entry = points
            .entry(key.clone())
            .or_insert_with(|| (node.name.clone(), HashSet::new(), HashSet::new()));

        // Add this path's cause to the set of causes (if detected)
        if let Some(cause) = &detected_cause {
            entry.1.insert(cause.clone());
        }

        // If there's a child crate code point (closer to panic), add it as a child of this node
        if let Some(child_key) = &child_crate_key {
            entry.2.insert(child_key.clone());
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

/// Count crate code points without printing (for --summary-only mode).
/// Returns a summary with count of panic points and affected files.
/// The config is used to filter out code points with allowed (not denied) causes.
pub fn count_crate_code_points_summary(
    node: &CallTreeNode,
    crate_src_path: &str,
    config: &Config,
) -> AnalysisSummary {
    let mut roots = collect_crate_code_points_hierarchical(node, crate_src_path);

    // Filter out code points with allowed causes
    filter_allowed_causes(&mut roots, config);

    // Deduplicate roots by (file, line)
    dedupe_crate_points(&mut roots);

    count_crate_points_and_files(&roots)
}

/// Print only the crate code points without the full tree.
/// Returns a summary with count of panic points and affected files.
/// The project_root is used to make relative paths absolute for clickable links.
/// The config is used to filter out code points with allowed (not denied) causes.
/// When no_hyperlinks is false, uses OSC 8 terminal hyperlinks for clickable short paths.
pub fn print_crate_code_points(
    node: &CallTreeNode,
    crate_src_path: &str,
    project_root: Option<&Path>,
    config: &Config,
    no_hyperlinks: bool,
) -> AnalysisSummary {
    let mut roots = collect_crate_code_points_hierarchical(node, crate_src_path);

    // Filter out code points with allowed causes
    filter_allowed_causes(&mut roots, config);

    // Deduplicate roots by (file, line)
    dedupe_crate_points(&mut roots);

    // Sort roots by file then line number
    roots.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

    let summary = count_crate_points_and_files(&roots);
    if summary.panic_points() == 0 {
        println!("\nNo panics in crate");
    } else {
        // Compute crate root for display (absolute path to crate directory)
        let crate_root = project_root.map(|root| {
            // crate_src_path is like "flowc/src/" - we want just "flowc/"
            let crate_dir = crate_src_path
                .strip_suffix("src/")
                .unwrap_or(crate_src_path);
            root.join(crate_dir.trim_end_matches('/'))
        });

        if let Some(ref root) = crate_root {
            println!("\nPanic code points in crate {}:", root.display());
        } else {
            println!("\nPanic code points in crate:");
        }
        println!();
        for point in &roots {
            print_crate_point(point, "", true, true, project_root, no_hyperlinks);
        }
    }
    summary
}

/// Filter out code points whose causes are ALL allowed (not denied) by config.
/// A point is kept if ANY of its causes is denied.
/// Also removes allowed causes from the causes set so only denied causes are displayed.
/// If a point has no causes, it's kept (conservative - assume denied).
fn filter_allowed_causes(points: &mut Vec<CrateCodePoint>, config: &Config) {
    points.retain_mut(|point| {
        // Track if we originally had no causes (conservative - keep these)
        let originally_empty = point.causes.is_empty();

        // Remove allowed causes from the set - only keep denied ones
        point.causes.retain(|cause| config.is_denied(cause));

        // Keep if originally empty (conservative) or if any denied causes remain
        let should_keep = originally_empty || !point.causes.is_empty();

        if should_keep {
            // Recursively filter children
            filter_allowed_causes(&mut point.children, config);
        }

        should_keep
    });
}

/// Summary of analysis results.
/// Uses HashSets internally to avoid double-counting when merging summaries
/// from multiple artifacts (e.g., multi-bin workspaces).
#[derive(Debug, Default, Clone)]
pub struct AnalysisSummary {
    /// Unique panic code points: (file, line)
    points: HashSet<(String, u32)>,
    /// Unique files with panic points
    files: HashSet<String>,
}

impl AnalysisSummary {
    /// Merge another summary into this one (union of sets, no double-counting)
    pub fn add(&mut self, other: &AnalysisSummary) {
        self.points.extend(other.points.iter().cloned());
        self.files.extend(other.files.iter().cloned());
    }

    /// Get the number of unique panic code points
    pub fn panic_points(&self) -> usize {
        self.points.len()
    }

    /// Get the number of unique files with panic points
    pub fn files_affected(&self) -> usize {
        self.files.len()
    }
}

/// Count unique crate code points and files in the hierarchy
fn count_crate_points_and_files(points: &[CrateCodePoint]) -> AnalysisSummary {
    let mut seen_points = HashSet::new();
    let mut seen_files = HashSet::new();
    collect_unique_point_keys_and_files(points, &mut seen_points, &mut seen_files);
    AnalysisSummary {
        points: seen_points,
        files: seen_files,
    }
}

/// Collect unique (file, line) keys and unique files from the hierarchy
fn collect_unique_point_keys_and_files(
    points: &[CrateCodePoint],
    seen_points: &mut HashSet<(String, u32)>,
    seen_files: &mut HashSet<String>,
) {
    for p in points {
        seen_points.insert((p.file.clone(), p.line));
        seen_files.insert(p.file.clone());
        collect_unique_point_keys_and_files(&p.children, seen_points, seen_files);
    }
}

/// Print a crate code point with tree-style indentation
/// Uses rustc-style " --> file:line:column" format for terminal-clickable links.
/// When no_hyperlinks is false, uses OSC 8 terminal hyperlinks for shorter display.
fn print_crate_point(
    point: &CrateCodePoint,
    prefix: &str,
    is_last: bool,
    is_root: bool,
    project_root: Option<&Path>,
    no_hyperlinks: bool,
) {
    // Make path absolute for clickable terminal links
    let absolute_path = if point.file.starts_with('/') {
        // Already absolute
        point.file.clone()
    } else if let Some(root) = project_root {
        // Make relative path absolute
        root.join(&point.file).to_string_lossy().to_string()
    } else {
        // No project root, use as-is
        point.file.clone()
    };

    // Compute short display path (just the relative part like "src/main.rs")
    let display_path = if let Some(root) = project_root {
        let root_str = root.to_string_lossy();
        // Strip project root prefix to get shorter display path
        absolute_path
            .strip_prefix(&format!("{}/", root_str))
            .unwrap_or(&absolute_path)
            .to_string()
    } else {
        absolute_path.clone()
    };

    // Format location - use OSC 8 hyperlinks when stdout is a TTY and not disabled
    let use_hyperlinks = !no_hyperlinks && io::stdout().is_terminal();
    let location = if use_hyperlinks {
        // OSC 8 hyperlink: \x1b]8;;URL\x1b\\DISPLAY\x1b]8;;\x1b\\
        // Use url crate for proper percent-encoding of paths with spaces/special chars
        if let Ok(mut file_url) = Url::from_file_path(&absolute_path) {
            file_url.set_fragment(Some(&format!("L{}", point.line)));
            let display = format!("{}:{}:1", display_path, point.line);
            format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", file_url, display)
        } else {
            // Fallback to plain path if URL conversion fails
            format!("{}:{}:1", absolute_path, point.line)
        }
    } else {
        // Plain absolute path (works in all terminals, or when redirected/piped)
        format!("{}:{}:1", absolute_path, point.line)
    };

    // Only show cause and help on leaf nodes (no children)
    let is_leaf = point.children.is_empty();

    // Get the primary cause for display (first one, sorted for determinism)
    let primary_cause: Option<&PanicCause> = {
        let mut causes: Vec<_> = point.causes.iter().collect();
        causes.sort_by_key(|c| c.description());
        causes.first().copied()
    };

    // Format cause description if available (only for leaf nodes)
    let cause_str = if is_leaf {
        if let Some(cause) = primary_cause {
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
        // Print suggestion and warning if we have a cause (only for leaf nodes)
        if is_leaf && let Some(cause) = primary_cause {
            let suggestion = cause.suggestion();
            if !suggestion.is_empty() {
                println!("     = help: {}", suggestion);
            }
            if let Some(warning) = cause.release_warning() {
                println!("     = warning: {}", warning);
            }
        }
    } else {
        let connector = if is_last { "└── " } else { "├── " };
        // Indent to align with parent, show tree connector, then clickable location
        println!("     {}{} --> {}{}", prefix, connector, location, cause_str);
        // Print suggestion and warning if we have a cause (only for leaf nodes)
        if is_leaf && let Some(cause) = primary_cause {
            let suggestion = cause.suggestion();
            if !suggestion.is_empty() {
                println!("     {}     = help: {}", prefix, suggestion);
            }
            if let Some(warning) = cause.release_warning() {
                println!("     {}     = warning: {}", prefix, warning);
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
        print_crate_point(
            child,
            &child_prefix,
            is_last_child,
            false,
            project_root,
            no_hyperlinks,
        );
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
