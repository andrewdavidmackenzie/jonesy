//! Call tree construction and processing.
//!
//! This module handles building and manipulating the call tree that traces
//! paths from panic symbols back to user code.

use crate::config::Config;
use crate::heuristics::detect_panic_cause;
use crate::heuristics::is_panic_triggering_function;
use crate::panic_cause::PanicCause;
use crate::sym::{CallGraph, ValidSourceFiles, matches_crate_pattern_validated};
use dashmap::DashSet;
use rayon::prelude::*;
use rustc_demangle::demangle;
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

/// Build a call tree by recursively finding callers of the target address.
/// Uses a thread-safe visited set to avoid infinite recursion when there are cycles.
/// Uses pre-computed CallGraph for O(1) lookups instead of re-scanning instructions.
/// Parallelizes exploration of top-level callers, with sequential recursion within each branch.
pub fn build_call_tree_parallel(
    call_graph: &CallGraph<'_>,
    target_addr: u64,
    visited: &Arc<DashSet<u64>>,
) -> Vec<CallTreeNode> {
    build_call_tree_parallel_filtered(call_graph, target_addr, visited, None, None)
}

/// Build a call tree with early filtering during construction.
/// Nodes that would be pruned (not in crate and no crate children) are never created.
pub fn build_call_tree_parallel_filtered(
    call_graph: &CallGraph<'_>,
    target_addr: u64,
    visited: &Arc<DashSet<u64>>,
    crate_src_path: Option<&str>,
    valid_files: Option<&ValidSourceFiles>,
) -> Vec<CallTreeNode> {
    // Use pre-computed call graph for O(1) lookup
    let callers = call_graph.get_callers(target_addr);

    // Process callers in parallel at this level.
    // Note: We share visited across all branches to avoid exponential blowup.
    // This means shared subtrees are only explored once, but all nodes are still created.
    callers
        .par_iter()
        .filter_map(|caller_info| {
            let caller_addr = caller_info.caller_start_address;

            // Atomically try to insert - if already present, skip recursion but still create node
            let should_recurse = visited.insert(caller_addr);

            // Create a new node for this caller
            // Use the function's declaration file for crate identification,
            // falling back to the call site file if not available
            let file = caller_info.caller_file.clone().or(caller_info.file.clone());
            let child_callers = if should_recurse {
                // Use sequential recursion within each branch to ensure deterministic behavior
                build_call_tree_sequential_filtered(
                    call_graph,
                    caller_addr,
                    visited,
                    crate_src_path,
                    valid_files,
                )
            } else {
                // Already visited - still get callers but don't recurse into them
                // This ensures all paths through the call graph are represented
                build_shallow_callers_filtered(call_graph, caller_addr, crate_src_path, valid_files)
            };

            // Early pruning: skip nodes with no children that aren't in crate
            if let Some(crate_path) = crate_src_path {
                if child_callers.is_empty() {
                    let in_crate = file.as_ref().is_some_and(|f| {
                        matches_crate_pattern_validated(f, crate_path, valid_files)
                    });
                    if !in_crate {
                        return None;
                    }
                }
            }

            Some(CallTreeNode {
                name: caller_info.caller_name.clone().into_owned(),
                file,
                line: caller_info.line,
                column: caller_info.column,
                callers: child_callers,
            })
        })
        .collect()
}

/// Sequential version for recursion within parallel branches.
/// Filters during construction to avoid creating nodes that would be pruned.
pub fn build_call_tree_sequential(
    call_graph: &CallGraph<'_>,
    target_addr: u64,
    visited: &Arc<DashSet<u64>>,
) -> Vec<CallTreeNode> {
    build_call_tree_sequential_filtered(call_graph, target_addr, visited, None, None)
}

/// Sequential version with optional early filtering during construction.
/// When crate_src_path is provided, nodes are filtered as they're built,
/// avoiding creation of nodes that would be pruned later.
pub fn build_call_tree_sequential_filtered(
    call_graph: &CallGraph<'_>,
    target_addr: u64,
    visited: &Arc<DashSet<u64>>,
    crate_src_path: Option<&str>,
    valid_files: Option<&ValidSourceFiles>,
) -> Vec<CallTreeNode> {
    let callers = call_graph.get_callers(target_addr);

    callers
        .iter()
        .filter_map(|caller_info| {
            let caller_addr = caller_info.caller_start_address;
            let should_recurse = visited.insert(caller_addr);

            let file = caller_info.caller_file.clone().or(caller_info.file.clone());
            let child_callers = if should_recurse {
                build_call_tree_sequential_filtered(
                    call_graph,
                    caller_addr,
                    visited,
                    crate_src_path,
                    valid_files,
                )
            } else {
                // Already visited - still get callers but don't recurse into them
                build_shallow_callers_filtered(call_graph, caller_addr, crate_src_path, valid_files)
            };

            // Early pruning: skip nodes with no children that aren't in crate
            if let Some(crate_path) = crate_src_path {
                if child_callers.is_empty() {
                    // Leaf node: only keep if in crate
                    let in_crate = file.as_ref().is_some_and(|f| {
                        matches_crate_pattern_validated(f, crate_path, valid_files)
                    });
                    if !in_crate {
                        return None;
                    }
                }
            }

            Some(CallTreeNode {
                name: caller_info.caller_name.clone().into_owned(),
                file,
                line: caller_info.line,
                column: caller_info.column,
                callers: child_callers,
            })
        })
        .collect()
}

/// Build shallow caller nodes without recursion.
/// Used when a function was already visited through another path.
/// This ensures we still capture caller relationships even for visited functions.
pub fn build_shallow_callers(call_graph: &CallGraph<'_>, target_addr: u64) -> Vec<CallTreeNode> {
    build_shallow_callers_filtered(call_graph, target_addr, None, None)
}

/// Build shallow caller nodes with optional filtering.
/// Only creates nodes that are in the crate (leaves are filtered).
fn build_shallow_callers_filtered(
    call_graph: &CallGraph<'_>,
    target_addr: u64,
    crate_src_path: Option<&str>,
    valid_files: Option<&ValidSourceFiles>,
) -> Vec<CallTreeNode> {
    call_graph
        .get_callers(target_addr)
        .iter()
        .filter_map(|caller_info| {
            let file = caller_info.caller_file.clone().or(caller_info.file.clone());

            // Early pruning: shallow callers are leaves, only keep if in crate
            if let Some(crate_path) = crate_src_path {
                let in_crate = file
                    .as_ref()
                    .is_some_and(|f| matches_crate_pattern_validated(f, crate_path, valid_files));
                if !in_crate {
                    return None;
                }
            }

            Some(CallTreeNode {
                name: caller_info.caller_name.clone().into_owned(),
                file,
                line: caller_info.line,
                column: caller_info.column,
                callers: vec![], // No deeper recursion to prevent infinite loops
            })
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
pub type CodePointKey = (String, u32);

/// Info stored for each code point: (function name, column, set of causes, set of child keys, is_direct_panic, called_function)
pub type CodePointInfo = (
    String,
    Option<u32>,
    HashSet<PanicCause>,
    HashSet<CodePointKey>,
    bool,           // is_direct_panic
    Option<String>, // called_function (for indirect panics)
);

/// Map of code points: key -> info
pub type CodePointMap = HashMap<CodePointKey, CodePointInfo>;

/// Extract the full qualified function name from a potentially mangled name.
/// Demangles Rust symbols first, strips generics and hash suffixes.
/// Returns the full path for use in config rules (exact match).
/// E.g., "my_crate::module::TimeStamp::now" -> "my_crate::module::TimeStamp::now"
///       "_ZN3std11collections4hash3set16HashSet$LT$T$GT$3new17h..." -> "std::collections::hash::set::HashSet::new"
///       "simple_function" -> "simple_function"
fn extract_qualified_function_name(full_name: &str) -> String {
    // Demangle the name first
    let demangled = demangle(full_name).to_string();

    // Remove all generic parameters while preserving the rest
    let mut cleaned = String::new();
    let mut depth = 0;
    for c in demangled.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            _ if depth == 0 => cleaned.push(c),
            _ => {}
        }
    }

    // Collect non-empty segments, only dropping a trailing Rust hash suffix
    let mut segments: Vec<&str> = cleaned
        .split("::")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    // Only strip the last segment if it looks like a Rust hash (e.g., "h1234abcdef")
    // This preserves legitimate names like "h2::send"
    if let Some(last) = segments.last() {
        if last.starts_with('h')
            && last.len() > 1
            && last[1..].chars().all(|c| c.is_ascii_hexdigit())
        {
            segments.pop();
        }
    }

    if segments.is_empty() {
        cleaned.trim().to_string()
    } else {
        segments.join("::")
    }
}

/// Collect crate code points with hierarchy.
/// Returns a list of "root" code points (entry points) with their children.
pub fn collect_crate_code_points_hierarchical(
    node: &CallTreeNode,
    crate_src_path: &str,
    valid_files: Option<&ValidSourceFiles>,
) -> Vec<CrateCodePoint> {
    // First pass: collect all crate code points and their relationships
    // Map: (file, line) -> (name, cause, set of child keys)
    let mut points: CodePointMap = CodePointMap::new();

    collect_crate_relationships(
        node,
        crate_src_path,
        &mut points,
        None,
        None,
        None,
        valid_files,
    );

    // All crate code points should be reported as roots.
    // Each point that can lead to a panic deserves its own entry,
    // regardless of whether it's also called by another crate function.
    // Children show what each function calls (toward the panic).
    let mut roots: Vec<CodePointKey> = points.keys().cloned().collect();
    roots.sort(); // Deterministic ordering

    // Build each root with its own cache so each top-level root keeps a full subtree.
    // (Still caches within a root to avoid repeated rebuilding in that root.)
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
        .filter_map(|root| {
            let mut cache: HashMap<CodePointKey, CrateCodePoint> = HashMap::new();
            build_subtree(root, &points, &mut HashSet::new(), &mut cache)
        })
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
pub fn collect_crate_relationships(
    node: &CallTreeNode,
    crate_src_path: &str,
    points: &mut CodePointMap,
    child_crate_key: Option<CodePointKey>,
    current_cause: Option<PanicCause>,
    immediate_callee: Option<&str>, // Name of function this node calls (toward panic)
    valid_files: Option<&ValidSourceFiles>,
) {
    // Try to detect panic cause from this node's function name and file path
    let detected_cause = detect_panic_cause(&node.name, node.file.as_deref()).or(current_cause);

    // Check if file matches any of the patterns
    let file_matches = node
        .file
        .as_ref()
        .is_some_and(|file| matches_crate_pattern_validated(file, crate_src_path, valid_files));

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
            immediate_callee.map(extract_qualified_function_name)
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
            valid_files,
        );
    }
}

/// Collect crate code points without printing.
/// Returns the filtered, deduplicated, and sorted code points along with summary.
///
/// The `workspace_root` parameter is used to resolve relative file paths for inline allow checks.
pub fn collect_crate_code_points(
    node: &CallTreeNode,
    crate_src_path: &str,
    config: &Config,
    valid_files: Option<&ValidSourceFiles>,
    workspace_root: Option<&std::path::Path>,
) -> (Vec<CrateCodePoint>, AnalysisSummary) {
    let mut roots = collect_crate_code_points_hierarchical(node, crate_src_path, valid_files);

    // Assign Unknown cause to leaf points without identified causes
    assign_unknown_causes(&mut roots);

    // Filter out phantom async panic points (false positives from generated code)
    // This is configurable via `filter_phantom_async = false` in jonesy.toml
    if config.filter_phantom_async() {
        filter_phantom_async_panics(&mut roots);
    }

    // Filter out code points with allowed causes
    filter_allowed_causes(&mut roots, config, workspace_root);

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

/// Filter out "phantom" async panic points.
///
/// These are false positives caused by Rust's generated async state machine code.
/// An empty async function like `async fn empty() {}` gets compiled to a Future
/// with drop handlers that technically have panic paths (e.g., misaligned pointer),
/// but these can never actually be triggered by user code.
///
/// Criteria for filtering:
/// 1. Function name matches async function/block patterns (not closures)
/// 2. The only cause is Unknown (no specific panic identified)
/// 3. Has no children (no real panic-inducing code in the call chain)
///
/// Why this works:
/// - If the async function had real panic-inducing code, there would be children
/// - If the panic cause could be identified from the call chain, it wouldn't be Unknown
/// - The combination strongly indicates phantom panics from generated drop handlers
///
/// Note: We only filter `{async_fn#N}` patterns since these represent
/// empty or trivial async functions. We keep `{async_block#N}` since those
/// typically contain real async code that might legitimately panic.
fn filter_phantom_async_panics(points: &mut Vec<CrateCodePoint>) {
    points.retain_mut(|point| {
        // Recursively filter children first
        filter_phantom_async_panics(&mut point.children);

        // Check if this is a phantom async panic point
        let is_phantom = is_phantom_async_function(&point.name)
            && point.causes.len() == 1
            && point.causes.contains(&PanicCause::Unknown)
            && point.children.is_empty();

        // Keep if NOT a phantom
        !is_phantom
    });
}

/// Check if a function name represents a likely phantom async function.
///
/// We're conservative here - only filter `{async_fn#N}` which represents
/// the entire async function as a state machine. This is most likely to be
/// a phantom when it has no children and Unknown cause (indicates empty
/// or very simple async function where only generated drop code can panic).
fn is_phantom_async_function(name: &str) -> bool {
    // Handle both simple names like "{async_fn#0}" and
    // fully qualified names like "crate::mod::foo::{async_fn#0}"
    name.rsplit("::")
        .next()
        .is_some_and(|tail| tail.starts_with("{async_fn#"))
}

/// Filter out code points whose causes are ALL allowed (not denied) by config or inline comments.
/// A point is kept if ANY of its causes is denied.
/// Also removes allowed causes from the causes set so only denied causes are displayed.
/// If a point has no causes, it's kept (conservative - assume denied).
///
/// The `workspace_root` parameter is used to resolve relative file paths for inline allow checks.
pub fn filter_allowed_causes(
    points: &mut Vec<CrateCodePoint>,
    config: &Config,
    workspace_root: Option<&std::path::Path>,
) {
    use crate::inline_allows::check_inline_allow;

    points.retain_mut(|point| {
        // Track if we originally had no causes (conservative - keep these unless inline allowed)
        let originally_empty = point.causes.is_empty();

        // For points with no causes, check if there's a wildcard inline allow
        // (points with no cause are otherwise kept conservatively)
        if originally_empty && check_inline_allow(&point.file, point.line, "*", workspace_root) {
            // Wildcard inline allow - filter out this point
            return false;
        }

        // Remove allowed causes from the set - only keep denied ones
        // Check both config rules (is_denied_at) and inline comments (check_inline_allow)
        point.causes.retain(|cause| {
            let cause_id = cause.id();

            // First check inline comments - if allowed there, filter it out
            if check_inline_allow(&point.file, point.line, cause_id, workspace_root) {
                return false; // Not denied (allowed by inline comment)
            }

            // Then check config rules against the containing function
            if !config.is_denied_at(cause, Some(&point.file), Some(&point.name)) {
                return false; // Allowed by rule on containing function
            }

            // Also check config rules against the called function (for indirect panics)
            if let Some(ref called_fn) = point.called_function {
                if !config.is_denied_at(cause, Some(&point.file), Some(called_fn)) {
                    return false; // Allowed by rule on the called function
                }
            }

            true // Denied (no rule allows it)
        });

        // Keep if originally empty (conservative) or if any denied causes remain
        let should_keep = originally_empty || !point.causes.is_empty();

        if should_keep {
            // Recursively filter children
            filter_allowed_causes(&mut point.children, config, workspace_root);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_phantom_async_point() -> CrateCodePoint {
        let mut causes = HashSet::new();
        causes.insert(PanicCause::Unknown);
        CrateCodePoint {
            name: "{async_fn#0}".to_string(),
            file: "src/main.rs".to_string(),
            line: 10,
            column: Some(1),
            causes,
            children: vec![],
            is_direct_panic: false,
            called_function: None,
        }
    }

    fn make_qualified_phantom_async_point() -> CrateCodePoint {
        let mut causes = HashSet::new();
        causes.insert(PanicCause::Unknown);
        CrateCodePoint {
            name: "my_crate::module::{async_fn#0}".to_string(),
            file: "src/lib.rs".to_string(),
            line: 20,
            column: Some(1),
            causes,
            children: vec![],
            is_direct_panic: false,
            called_function: None,
        }
    }

    fn make_real_async_point() -> CrateCodePoint {
        let mut causes = HashSet::new();
        causes.insert(PanicCause::UnwrapNone);
        CrateCodePoint {
            name: "{async_fn#0}".to_string(),
            file: "src/main.rs".to_string(),
            line: 15,
            column: Some(5),
            causes,
            children: vec![],
            is_direct_panic: true,
            called_function: None,
        }
    }

    fn make_async_with_children() -> CrateCodePoint {
        let mut causes = HashSet::new();
        causes.insert(PanicCause::Unknown);
        let mut child_causes = HashSet::new();
        child_causes.insert(PanicCause::UnwrapNone);
        CrateCodePoint {
            name: "{async_fn#0}".to_string(),
            file: "src/main.rs".to_string(),
            line: 25,
            column: Some(1),
            causes,
            children: vec![CrateCodePoint {
                name: "inner_fn".to_string(),
                file: "src/main.rs".to_string(),
                line: 30,
                column: Some(10),
                causes: child_causes,
                children: vec![],
                is_direct_panic: true,
                called_function: None,
            }],
            is_direct_panic: false,
            called_function: None,
        }
    }

    #[test]
    fn test_is_phantom_async_function_simple() {
        assert!(is_phantom_async_function("{async_fn#0}"));
        assert!(is_phantom_async_function("{async_fn#42}"));
    }

    #[test]
    fn test_is_phantom_async_function_qualified() {
        assert!(is_phantom_async_function("my_crate::{async_fn#0}"));
        assert!(is_phantom_async_function("my_crate::module::{async_fn#0}"));
        assert!(is_phantom_async_function("crate::foo::bar::{async_fn#1}"));
    }

    #[test]
    fn test_is_phantom_async_function_not_async() {
        assert!(!is_phantom_async_function("main"));
        assert!(!is_phantom_async_function("{closure#0}"));
        assert!(!is_phantom_async_function("{async_block#0}"));
        assert!(!is_phantom_async_function("my_crate::regular_fn"));
    }

    #[test]
    fn test_filter_phantom_async_filters_simple() {
        let mut points = vec![make_phantom_async_point()];
        filter_phantom_async_panics(&mut points);
        assert!(points.is_empty(), "Phantom async point should be filtered");
    }

    #[test]
    fn test_filter_phantom_async_filters_qualified() {
        let mut points = vec![make_qualified_phantom_async_point()];
        filter_phantom_async_panics(&mut points);
        assert!(
            points.is_empty(),
            "Qualified phantom async point should be filtered"
        );
    }

    #[test]
    fn test_filter_phantom_async_keeps_real_panics() {
        let mut points = vec![make_real_async_point()];
        filter_phantom_async_panics(&mut points);
        assert_eq!(points.len(), 1, "Real async panic should not be filtered");
    }

    #[test]
    fn test_filter_phantom_async_keeps_with_children() {
        let mut points = vec![make_async_with_children()];
        filter_phantom_async_panics(&mut points);
        assert_eq!(
            points.len(),
            1,
            "Async with children should not be filtered"
        );
    }

    #[test]
    fn test_filter_phantom_async_mixed() {
        let mut points = vec![
            make_phantom_async_point(),
            make_real_async_point(),
            make_qualified_phantom_async_point(),
            make_async_with_children(),
        ];
        filter_phantom_async_panics(&mut points);
        assert_eq!(
            points.len(),
            2,
            "Should keep only real panics and those with children"
        );
    }

    #[test]
    fn test_extract_qualified_function_name() {
        // Full path preserved
        assert_eq!(
            extract_qualified_function_name("my_crate::TimeStamp::now"),
            "my_crate::TimeStamp::now"
        );
        assert_eq!(
            extract_qualified_function_name("std::collections::HashMap::new"),
            "std::collections::HashMap::new"
        );
        assert_eq!(
            extract_qualified_function_name("my_crate::module::init"),
            "my_crate::module::init"
        );
        // Simple function (no ::)
        assert_eq!(
            extract_qualified_function_name("simple_function"),
            "simple_function"
        );
        // Mangled Rust symbol - full demangled path
        assert_eq!(
            extract_qualified_function_name(
                "_ZN3std11collections4hash3set16HashSet$LT$T$GT$3new17ha7a7fdf7dbcd659dE"
            ),
            "std::collections::hash::set::HashSet::new"
        );
        // Short qualified name preserved as-is
        assert_eq!(
            extract_qualified_function_name("Option::unwrap"),
            "Option::unwrap"
        );
        // Legitimate crate names starting with 'h' are preserved (not stripped as hash)
        assert_eq!(
            extract_qualified_function_name("h2::client::send"),
            "h2::client::send"
        );
    }

    #[test]
    fn test_filter_allows_called_function_rule() {
        use crate::config::Config;
        use std::io::Write;

        // Create a temp jonesy.toml with a rule allowing unwrap on my_crate::time::TimeStamp::now
        let dir = tempfile::tempdir().unwrap();
        let toml_path = dir.path().join("jonesy.toml");
        let mut f = std::fs::File::create(&toml_path).unwrap();
        writeln!(
            f,
            "[[rules]]\nfunction = \"my_crate::time::TimeStamp::now\"\nallow = [\"unwrap\"]"
        )
        .unwrap();

        let mut config = Config::with_defaults();
        config.load_from_jones_toml(&toml_path);

        // Code point in "app::run" that calls "my_crate::time::TimeStamp::now" which may unwrap
        let mut points = vec![CrateCodePoint {
            name: "app::run".to_string(),
            file: "src/main.rs".to_string(),
            line: 42,
            column: Some(5),
            causes: vec![PanicCause::UnwrapNone].into_iter().collect(),
            children: vec![],
            is_direct_panic: false,
            called_function: Some("my_crate::time::TimeStamp::now".to_string()),
        }];

        filter_allowed_causes(&mut points, &config, None);

        // The point should be filtered out because the rule allows unwrap on that function
        assert!(
            points.is_empty(),
            "Point should be filtered out by called-function allow rule, but found: {points:?}"
        );
    }

    #[test]
    fn test_filter_global_allow_capacity_single_cause() {
        use crate::config::Config;
        use std::io::Write;

        // Create a jonesy.toml with a global (crate-level) allow for "capacity"
        let dir = tempfile::tempdir().unwrap();
        let toml_path = dir.path().join("jonesy.toml");
        let mut f = std::fs::File::create(&toml_path).unwrap();
        writeln!(f, "allow = [\"capacity\"]").unwrap();

        let mut config = Config::with_defaults();
        config.load_from_jones_toml(&toml_path);

        // Code point with only CapacityOverflow cause
        let mut points = vec![CrateCodePoint {
            name: "meshchat::device::DeviceIdentifier::hash".to_string(),
            file: "src/device.rs".to_string(),
            line: 74,
            column: Some(22),
            causes: vec![PanicCause::CapacityOverflow].into_iter().collect(),
            children: vec![],
            is_direct_panic: false,
            called_function: Some("core::hash::Hash::hash".to_string()),
        }];

        filter_allowed_causes(&mut points, &config, None);

        // The point should be filtered out because capacity is globally allowed
        assert!(
            points.is_empty(),
            "Point should be filtered out by global allow for 'capacity', but found: {points:?}"
        );
    }

    #[test]
    fn test_filter_global_allow_capacity_with_other_cause_remaining() {
        use crate::config::Config;
        use std::io::Write;

        // Create a jonesy.toml with a global (crate-level) allow for "capacity"
        let dir = tempfile::tempdir().unwrap();
        let toml_path = dir.path().join("jonesy.toml");
        let mut f = std::fs::File::create(&toml_path).unwrap();
        writeln!(f, "allow = [\"capacity\"]").unwrap();

        let mut config = Config::with_defaults();
        config.load_from_jones_toml(&toml_path);

        // Code point with BOTH CapacityOverflow and Unknown causes
        // (simulates multiple panic paths through the same code point)
        let mut points = vec![CrateCodePoint {
            name: "meshchat::device::DeviceIdentifier::hash".to_string(),
            file: "src/device.rs".to_string(),
            line: 74,
            column: Some(22),
            causes: vec![PanicCause::CapacityOverflow, PanicCause::Unknown]
                .into_iter()
                .collect(),
            children: vec![],
            is_direct_panic: false,
            called_function: Some("core::hash::Hash::hash".to_string()),
        }];

        filter_allowed_causes(&mut points, &config, None);

        // The point should remain because Unknown is NOT allowed
        assert_eq!(
            points.len(),
            1,
            "Point should remain because Unknown cause is not allowed"
        );
        // But CapacityOverflow should have been removed from causes
        assert!(
            !points[0].causes.contains(&PanicCause::CapacityOverflow),
            "CapacityOverflow should have been removed"
        );
        assert!(
            points[0].causes.contains(&PanicCause::Unknown),
            "Unknown should remain"
        );
    }

    #[test]
    fn test_filter_keeps_non_matching_called_function() {
        use crate::config::Config;
        use std::io::Write;

        // Rule allows "unwrap" on calls to "my_crate::time::TimeStamp::now"
        let dir = tempfile::tempdir().unwrap();
        let toml_path = dir.path().join("jonesy.toml");
        let mut f = std::fs::File::create(&toml_path).unwrap();
        writeln!(
            f,
            "[[rules]]\nfunction = \"my_crate::time::TimeStamp::now\"\nallow = [\"unwrap\"]"
        )
        .unwrap();

        let mut config = Config::with_defaults();
        config.load_from_jones_toml(&toml_path);

        // Code point calling a DIFFERENT function - should NOT be filtered
        let mut points = vec![CrateCodePoint {
            name: "app::run".to_string(),
            file: "src/main.rs".to_string(),
            line: 42,
            column: Some(5),
            causes: vec![PanicCause::UnwrapNone].into_iter().collect(),
            children: vec![],
            is_direct_panic: false,
            called_function: Some("other_crate::Config::parse".to_string()),
        }];

        filter_allowed_causes(&mut points, &config, None);

        // The point should be kept because the rule doesn't match Config::parse
        assert_eq!(
            points.len(),
            1,
            "Point should be kept - rule doesn't match different called function"
        );
    }
}
