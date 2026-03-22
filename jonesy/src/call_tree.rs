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
    /// Whether this code point directly calls a panic-triggering function (e.g., unwrap, expect)
    /// vs calling another function that eventually panics
    pub is_direct_panic: bool,
    /// Name of the function called at this code point (for indirect panics)
    /// Used to show "This calls `foo` which may panic" in help messages
    pub called_function: Option<String>,
}

/// Key for identifying a code point: (file, line)
type CodePointKey = (String, u32);

/// Info stored for each code point: (function name, column, set of causes, set of child keys, is_direct_panic, called_function)
type CodePointInfo = (
    String,
    Option<u32>,
    HashSet<PanicCause>,
    HashSet<CodePointKey>,
    bool,           // is_direct_panic
    Option<String>, // called_function (for indirect panics)
);

/// Map of code points: key -> info
type CodePointMap = HashMap<CodePointKey, CodePointInfo>;

/// Check if a function name represents a panic-triggering function.
/// These are functions that directly cause a panic (unwrap, expect, panic!, etc.)
/// as opposed to functions that call other functions that eventually panic.
fn is_panic_triggering_function(func_name: &str) -> bool {
    // Unwrap/expect variants
    func_name.contains("unwrap_failed")
        || func_name.contains("expect_failed")
        // Direct unwrap/expect calls (before they reach _failed)
        || (func_name.contains("unwrap") && !func_name.contains("unwrap_or"))
        || (func_name.contains("expect") && func_name.contains("Option"))
        || (func_name.contains("expect") && func_name.contains("Result"))
        // Panic functions
        || func_name.contains("panic_fmt")
        || func_name.contains("panic_display")
        || func_name.contains("panic_bounds_check")
        || func_name.contains("panic_const_")
        || func_name.contains("panic_in_cleanup")
        || func_name.contains("panic_cannot_unwind")
        || func_name.contains("panic_nounwind")
        || func_name.contains("panic_misaligned_pointer")
        || func_name.contains("panic_invalid_enum")
        // Assert
        || func_name.contains("assert_failed")
        // Capacity/allocation
        || func_name.contains("capacity_overflow")
        || func_name.contains("handle_alloc_error")
        // String/slice errors
        || func_name.contains("slice_error_fail")
        || func_name.contains("str_index_overflow_fail")
        // Index trait - direct bounds check
        || func_name.starts_with("index<")
        || func_name.contains("::index<")
        || func_name.contains("Index::index")
}

/// Extract a simple, readable function name from a potentially complex DWARF name.
/// E.g., "my_crate::module::init" -> "init"
///       "init<T>" -> "init"
///       "collect<std::env::Args>" -> "collect"
fn extract_simple_function_name(full_name: &str) -> String {
    // Remove generic parameters first (handles nested generics)
    let name = if let Some(bracket_pos) = full_name.find('<') {
        &full_name[..bracket_pos]
    } else {
        full_name
    };

    // Take the last segment after ::
    let name = name.rsplit("::").next().unwrap_or(name);

    // Clean up any remaining special characters
    let name = name.trim_end_matches('>').trim();

    name.to_string()
}

/// Collect crate code points with hierarchy.
/// Returns a list of "root" code points (entry points) with their children.
pub fn collect_crate_code_points_hierarchical(
    node: &CallTreeNode,
    crate_src_path: &str,
) -> Vec<CrateCodePoint> {
    // First pass: collect all crate code points and their relationships
    // Map: (file, line) -> (name, cause, set of child keys)
    let mut points: CodePointMap = CodePointMap::new();

    collect_crate_relationships(node, crate_src_path, &mut points, None, None, None);

    // Find roots: points that are not in any other point's children
    let all_children: HashSet<(String, u32)> = points
        .values()
        .flat_map(|(_, _, _, children, _, _)| children.iter().cloned())
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

    // Build tree from roots with caching to avoid exponential rebuilding of shared subtrees
    // Cache: key -> built subtree (avoids rebuilding same subtree multiple times)
    let mut cache: HashMap<CodePointKey, CrateCodePoint> = HashMap::new();

    fn build_subtree(
        key: &CodePointKey,
        points: &CodePointMap,
        path: &mut HashSet<CodePointKey>,
        cache: &mut HashMap<CodePointKey, CrateCodePoint>,
    ) -> Option<CrateCodePoint> {
        // Prevent cycles only on current DFS path
        if path.contains(key) {
            return None;
        }

        // Return cached result if already built
        // Return a shallow copy WITHOUT children to avoid exponential memory from deep cloning.
        // The full subtree is available at its first occurrence.
        if let Some(cached) = cache.get(key) {
            return Some(CrateCodePoint {
                name: cached.name.clone(),
                file: cached.file.clone(),
                line: cached.line,
                column: cached.column,
                causes: cached.causes.clone(),
                children: vec![], // Don't clone children - they're at first occurrence
                is_direct_panic: cached.is_direct_panic,
                called_function: cached.called_function.clone(),
            });
        }

        path.insert(key.clone());

        let (name, column, causes, child_keys_set, is_direct_panic, called_function) =
            points.get(key)?;
        // Sort child keys for deterministic output
        let mut child_keys: Vec<_> = child_keys_set.iter().cloned().collect();
        child_keys.sort();
        let mut children: Vec<CrateCodePoint> = child_keys
            .iter()
            .filter_map(|child_key| build_subtree(child_key, points, path, cache))
            .collect();
        children.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
        path.remove(key);

        let point = CrateCodePoint {
            name: name.clone(),
            file: key.0.clone(),
            line: key.1,
            column: *column,
            causes: causes.clone(),
            children,
            is_direct_panic: *is_direct_panic,
            called_function: called_function.clone(),
        };

        // Cache for reuse by other parents
        cache.insert(key.clone(), point.clone());
        Some(point)
    }

    roots
        .iter()
        .filter_map(|root| build_subtree(root, &points, &mut HashSet::new(), &mut cache))
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
/// Tracks `immediate_callee` to determine if panic is direct (calling unwrap/expect directly)
/// or indirect (calling a function that eventually panics).
fn collect_crate_relationships(
    node: &CallTreeNode,
    crate_src_path: &str,
    points: &mut CodePointMap,
    child_crate_key: Option<CodePointKey>,
    current_cause: Option<PanicCause>,
    immediate_callee: Option<&str>, // Name of function this node calls (toward panic)
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
        // Determine if this is a direct panic (immediate callee is a panic-triggering function)
        let is_direct = immediate_callee
            .map(is_panic_triggering_function)
            .unwrap_or(false);

        // For indirect panics, store the called function name for help messages
        let called_fn = if !is_direct {
            immediate_callee.map(extract_simple_function_name)
        } else {
            None
        };

        // Ensure this point exists in the map and accumulate all causes
        let entry = points.entry(key.clone()).or_insert_with(|| {
            (
                node.name.clone(),
                node.column,
                HashSet::new(),
                HashSet::new(),
                is_direct,
                called_fn.clone(),
            )
        });

        // Update is_direct_panic: true if ANY path through this point is direct
        // (conservative: if one path is direct, user could be calling directly)
        if is_direct {
            entry.4 = true;
            entry.5 = None; // Clear called_function for direct panics
        } else if entry.5.is_none() && called_fn.is_some() {
            // Store called function if not already set
            entry.5 = called_fn;
        }

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
            Some(&node.name), // Current node is the callee for the caller
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

    // Assign Unknown cause to leaf points without identified causes
    assign_unknown_causes(&mut roots);

    // Filter out code points with allowed causes
    filter_allowed_causes(&mut roots, config);

    // Deduplicate roots by (file, line)
    dedupe_crate_points(&mut roots);

    // Sort roots by file then line number
    roots.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

    let summary = count_crate_points_and_files(&roots);
    (roots, summary)
}

/// Assign `Unknown` cause to leaf points that have no identified causes.
/// A leaf point is one with no children (closest to the panic in the call chain).
/// This makes it clear that jonesy detected a panic path but couldn't identify the specific cause.
fn assign_unknown_causes(points: &mut [CrateCodePoint]) {
    for point in points.iter_mut() {
        // Recursively process children first
        assign_unknown_causes(&mut point.children);

        // If this is a leaf (no children) with no causes, assign Unknown
        if point.children.is_empty() && point.causes.is_empty() {
            point.causes.insert(PanicCause::Unknown);
        }
    }
}

/// Filter out code points whose causes are ALL allowed (not denied) by config or inline comments.
/// A point is kept if ANY of its causes is denied.
/// Also removes allowed causes from the causes set so only denied causes are displayed.
/// If a point has no causes, it's kept (conservative - assume denied).
fn filter_allowed_causes(points: &mut Vec<CrateCodePoint>, config: &Config) {
    use crate::inline_allows::check_inline_allow;

    points.retain_mut(|point| {
        // Track if we originally had no causes (conservative - keep these unless inline allowed)
        let originally_empty = point.causes.is_empty();

        // For points with no causes, check if there's a wildcard inline allow
        // (points with no cause are otherwise kept conservatively)
        if originally_empty && check_inline_allow(&point.file, point.line, "*") {
            // Wildcard inline allow - filter out this point
            return false;
        }

        // Remove allowed causes from the set - only keep denied ones
        // Check both config rules (is_denied_at) and inline comments (check_inline_allow)
        point.causes.retain(|cause| {
            let cause_id = cause.id();

            // First check inline comments - if allowed there, filter it out
            if check_inline_allow(&point.file, point.line, cause_id) {
                return false; // Not denied (allowed by inline comment)
            }

            // Then check config rules
            config.is_denied_at(cause, Some(&point.file), Some(&point.name))
        });

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
