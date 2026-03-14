//! Call tree construction and processing.
//!
//! This module handles building and manipulating the call tree that traces
//! paths from panic symbols back to user code.

use crate::config::Config;
use crate::panic_cause::{PanicCause, detect_panic_cause};
use crate::sym::{CallGraph, matches_crate_pattern};
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
    /// Column number (if available from debug info)
    pub column: Option<u32>,
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
            column: None,
            callers: Vec::new(),
        }
    }
}

/// Returns true if the node's source file matches the crate source path.
/// For workspace mode (when crate_src_path contains "|"), checks against multiple paths.
pub fn is_in_crate(node: &CallTreeNode, crate_src_path: &str) -> bool {
    node.file
        .as_ref()
        .is_some_and(|file| matches_crate_pattern(file, crate_src_path))
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
                // Already visited - still get callers but don't recurse into them
                // This ensures all paths through the call graph are represented
                build_shallow_callers(call_graph, caller_addr)
            };

            Some(CallTreeNode {
                name: caller_info.caller.name.clone(),
                file,
                line: caller_info.line,
                column: caller_info.column,
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
                // Already visited - still get callers but don't recurse into them
                build_shallow_callers(call_graph, caller_addr)
            };

            CallTreeNode {
                name: caller_info.caller.name.clone(),
                file,
                line: caller_info.line,
                column: caller_info.column,
                callers: child_callers,
            }
        })
        .collect()
}

/// Build shallow caller nodes without recursion.
/// Used when a function was already visited through another path.
/// This ensures we still capture caller relationships even for visited functions.
fn build_shallow_callers(call_graph: &CallGraph, target_addr: u64) -> Vec<CallTreeNode> {
    call_graph
        .get_callers(target_addr)
        .into_iter()
        .map(|caller_info| {
            let file = caller_info.caller.file.clone().or(caller_info.file.clone());
            CallTreeNode {
                name: caller_info.caller.name.clone(),
                file,
                line: caller_info.line,
                column: caller_info.column,
                callers: vec![], // No deeper recursion to prevent infinite loops
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
    /// Column number of the call site (if available from DWARF)
    pub column: Option<u32>,
    /// All detected causes of panic paths through this point
    pub causes: HashSet<PanicCause>,
    /// Code points that this one calls (closer to panic in the call chain)
    pub children: Vec<CrateCodePoint>,
}

/// Key for identifying a code point: (file, line)
type CodePointKey = (String, u32);

/// Info stored for each code point: (function name, column, set of causes, set of child keys)
type CodePointInfo = (
    String,
    Option<u32>,
    HashSet<PanicCause>,
    HashSet<CodePointKey>,
);

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
        .flat_map(|(_, _, _, children)| children.iter().cloned())
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

    // Build tree from roots, propagating causes from children to parents
    fn build_subtree(
        key: &CodePointKey,
        points: &CodePointMap,
        path: &mut HashSet<CodePointKey>,
    ) -> Option<CrateCodePoint> {
        // Prevent cycles only on current DFS path (not across sibling/root branches)
        if !path.insert(key.clone()) {
            return None;
        }

        let (name, column, causes, child_keys_set) = points.get(key)?;
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
            column: *column,
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
    // Try to detect panic cause from this node's function name and file path
    let detected_cause = detect_panic_cause(&node.name, node.file.as_deref()).or(current_cause);

    // Check if file matches any of the patterns
    let file_matches = node
        .file
        .as_ref()
        .is_some_and(|file| matches_crate_pattern(file, crate_src_path));

    let node_key = if let (Some(file), Some(line)) = (&node.file, &node.line)
        && file_matches
        && *line > 0
    {
        Some((file.clone(), *line))
    } else {
        None
    };

    if let Some(key) = &node_key {
        // Ensure this point exists in the map and accumulate all causes
        let entry = points.entry(key.clone()).or_insert_with(|| {
            (
                node.name.clone(),
                node.column,
                HashSet::new(),
                HashSet::new(),
            )
        });

        // Add this path's cause to the set of causes (if detected)
        if let Some(cause) = &detected_cause {
            entry.2.insert(cause.clone());
        }

        // If there's a child crate code point (closer to panic), add it as a child of this node
        if let Some(child_key) = &child_crate_key {
            entry.3.insert(child_key.clone());
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

/// Collect crate code points without printing.
/// Returns the filtered, deduplicated, and sorted code points along with summary.
pub fn collect_crate_code_points(
    node: &CallTreeNode,
    crate_src_path: &str,
    config: &Config,
) -> (Vec<CrateCodePoint>, AnalysisSummary) {
    let mut roots = collect_crate_code_points_hierarchical(node, crate_src_path);

    // Filter out code points with allowed causes
    filter_allowed_causes(&mut roots, config);

    // Deduplicate roots by (file, line)
    dedupe_crate_points(&mut roots);

    // Sort roots by file then line number
    roots.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

    let summary = count_crate_points_and_files(&roots);
    (roots, summary)
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

/// Complete analysis results ready for output rendering.
/// This is the shared structure used by all output formats (text, JSON, HTML).
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    /// Name of the project/crate
    pub project_name: String,
    /// Root path of the project
    pub project_root: String,
    /// All panic code points found (filtered and deduplicated)
    pub code_points: Vec<CrateCodePoint>,
}

impl AnalysisResult {
    /// Create a new analysis result
    pub fn new(
        project_name: impl Into<String>,
        project_root: impl Into<String>,
        code_points: Vec<CrateCodePoint>,
    ) -> Self {
        Self {
            project_name: project_name.into(),
            project_root: project_root.into(),
            code_points,
        }
    }

    /// Compute the summary from the code points
    pub fn summary(&self) -> AnalysisSummary {
        count_crate_points_and_files(&self.code_points)
    }

    /// Get the number of panic points
    pub fn panic_points(&self) -> usize {
        self.summary().panic_points()
    }
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
    /// Create a new summary from collected points
    pub fn from_points(points: HashSet<(String, u32)>, files: HashSet<String>) -> Self {
        Self { points, files }
    }

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
