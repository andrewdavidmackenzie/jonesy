//! Call tree construction and processing.
//!
//! This module handles building and manipulating the call tree that traces
//! paths from panic symbols back to user code.

use crate::config::Config;
use crate::heuristics::detect_panic_cause;
use crate::heuristics::is_panic_triggering_function;
use crate::panic_cause::PanicCause;
use crate::project_context::ProjectContext;
use crate::string_tables::{FILE_NONE, FilePathInterner, NAME_NONE, NameInterner};
use crate::sym::CallGraph;
use dashmap::{DashMap, DashSet};
use rayon::prelude::*;
use rustc_demangle::demangle;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

/// Key for identifying a code point: (file_idx, line, column).
/// The `file_idx` is a `u16` index into a `FilePathInterner`, avoiding
/// expensive string cloning in hot paths (DashMap keys, HashSet lookups,
/// deduplication, merging). The file path string is resolved from the
/// interner only at the output boundary.
/// Two call sites at different columns on the same line are distinct points.
/// Column 0 means "unspecified".
pub type CodePointKey = (u16, u16, u16);

static PRE_FILTER_LOCATIONS: LazyLock<Mutex<HashSet<(String, u16)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Get pre-filter locations collected during analysis, then clear the store.
pub fn take_pre_filter_locations() -> HashSet<(String, u16)> {
    std::mem::take(&mut *PRE_FILTER_LOCATIONS.lock().unwrap())
}

fn collect_locations(
    points: &[CrateCodePoint],
    interner: &FilePathInterner,
    locations: &mut HashSet<(String, u16)>,
) {
    for p in points {
        let file = interner.resolve(p.file).unwrap_or_default();
        locations.insert((file, p.line));
        collect_locations(&p.children, interner, locations);
    }
}
use std::sync::Arc;

/// A node in the call tree representing a function that can lead to the target symbol
#[derive(Debug)]
pub struct CallTreeNode {
    /// Symbol/function name index into a shared name interner (`NameInterner`).
    /// `NAME_NONE` means no name is known.
    pub name: u32,
    /// Source file index into a shared `FilePathInterner`.
    /// `FILE_NONE` means no file path is known.
    pub file: u16,
    /// Line number (0 = absent)
    pub line: u16,
    /// Column number (0 = absent)
    pub column: u16,
    /// Functions that call this one
    pub callers: Vec<CallTreeNode>,
}

impl CallTreeNode {
    /// Create a new root node for a call tree
    pub fn new_root(name: u32) -> Self {
        CallTreeNode {
            name,
            file: FILE_NONE,
            line: 0,
            column: u16::MAX,
            callers: Vec::new(),
        }
    }
}

/// Build a call tree by recursively finding callers of the target address.
/// Uses a thread-safe visited set to avoid infinite recursion when there are cycles.
/// Uses pre-computed CallGraph for O(1) lookups instead of re-scanning instructions.
/// Parallelizes exploration of top-level callers, with sequential recursion within each branch.
/// Build a call tree with early filtering during construction.
/// Nodes that would be pruned (not in crate and no crate children) are never created.
pub fn build_call_tree_parallel_filtered(
    call_graph: &CallGraph,
    target_addr: u64,
    visited: &Arc<DashSet<u64>>,
    project_context: &ProjectContext,
    file_interner: &FilePathInterner,
) -> Vec<CallTreeNode> {
    let callers = call_graph.get_callers(target_addr);

    callers
        .par_iter()
        .filter_map(|caller_info| {
            let caller_addr = caller_info.caller_start_address;
            let should_recurse = visited.insert(caller_addr);

            let file = caller_info.file;
            let child_callers = if should_recurse {
                build_call_tree_sequential_filtered(
                    call_graph,
                    caller_addr,
                    visited,
                    project_context,
                    file_interner,
                )
            } else {
                build_shallow_callers_filtered(
                    call_graph,
                    caller_addr,
                    project_context,
                    file_interner,
                )
            };

            // Early pruning: skip leaf nodes that aren't in crate source
            let file_path = file_interner.resolve(file);
            if child_callers.is_empty()
                && !file_path
                    .as_ref()
                    .is_some_and(|f| project_context.is_crate_source(f))
            {
                return None;
            }

            Some(CallTreeNode {
                name: caller_info.caller_name,
                file,
                line: caller_info.line,
                column: caller_info.column,
                callers: child_callers,
            })
        })
        .collect()
}

/// Sequential version with early filtering during construction.
fn build_call_tree_sequential_filtered(
    call_graph: &CallGraph,
    target_addr: u64,
    visited: &Arc<DashSet<u64>>,
    project_context: &ProjectContext,
    file_interner: &FilePathInterner,
) -> Vec<CallTreeNode> {
    let callers = call_graph.get_callers(target_addr);

    callers
        .iter()
        .filter_map(|caller_info| {
            let caller_addr = caller_info.caller_start_address;
            let should_recurse = visited.insert(caller_addr);

            let file = caller_info.file;
            let child_callers = if should_recurse {
                build_call_tree_sequential_filtered(
                    call_graph,
                    caller_addr,
                    visited,
                    project_context,
                    file_interner,
                )
            } else {
                build_shallow_callers_filtered(
                    call_graph,
                    caller_addr,
                    project_context,
                    file_interner,
                )
            };

            // Early pruning: skip leaf nodes that aren't in crate source
            let file_path = file_interner.resolve(file);
            if child_callers.is_empty()
                && !file_path
                    .as_ref()
                    .is_some_and(|f| project_context.is_crate_source(f))
            {
                return None;
            }

            Some(CallTreeNode {
                name: caller_info.caller_name,
                file,
                line: caller_info.line,
                column: caller_info.column,
                callers: child_callers,
            })
        })
        .collect()
}

/// Build shallow caller nodes with filtering.
/// Only creates nodes that are in the crate (leaves are filtered).
fn build_shallow_callers_filtered(
    call_graph: &CallGraph,
    target_addr: u64,
    project_context: &ProjectContext,
    file_interner: &FilePathInterner,
) -> Vec<CallTreeNode> {
    call_graph
        .get_callers(target_addr)
        .iter()
        .filter_map(|caller_info| {
            let file = caller_info.file;

            // Shallow callers are leaves — only keep if in crate source
            let file_path = file_interner.resolve(file);
            if !file_path
                .as_ref()
                .is_some_and(|f| project_context.is_crate_source(f))
            {
                return None;
            }

            Some(CallTreeNode {
                name: caller_info.caller_name,
                file,
                line: caller_info.line,
                column: caller_info.column,
                callers: vec![], // No deeper recursion to prevent infinite loops
            })
        })
        .collect()
}

/// A crate code point with its hierarchical children (code points it calls toward panic).
/// The `file` field is a `u16` index into a shared `FilePathInterner`.
/// Use `resolve_file()` from `string_tables` to get the actual path string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateCodePoint {
    /// Function name index into a shared name interner (`NameInterner`).
    /// `NAME_NONE` means no name is known.
    pub name: u32,
    /// File path index into a shared `FilePathInterner`.
    /// `FILE_NONE` means no file path is known.
    pub file: u16,
    pub line: u16,
    /// Column number of the call site (0 = absent)
    pub column: u16,
    /// All detected causes of panic paths through this point
    pub causes: HashSet<PanicCause>,
    /// Code points that this one calls (closer to panic in the call chain)
    pub children: Vec<CrateCodePoint>,
    /// Whether this code point directly calls a panic-triggering function (e.g., unwrap, expect)
    /// vs calling another function that eventually panics
    pub is_direct_panic: bool,
    /// Name of the function called at this code point (for indirect panics).
    /// `NAME_NONE` means no called function (e.g., direct panics).
    pub called_function: u32,
}

/// Info stored for each code point: (function name index, set of causes, set of child keys, is_direct_panic, called_function index)
pub type CodePointInfo = (
    u32,
    HashSet<PanicCause>,
    HashSet<CodePointKey>,
    bool, // is_direct_panic
    u32,  // called_function (NAME_NONE for direct panics / no called function)
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
/// The `file_interner` is the shared interner used for all file path indices.
/// The `name_interner` is the shared interner used for all function name indices.
pub fn collect_crate_code_points_hierarchical(
    node: &CallTreeNode,
    project_context: &ProjectContext,
    file_interner: &FilePathInterner,
    name_interner: &NameInterner,
) -> Vec<CrateCodePoint> {
    let points: DashMap<CodePointKey, CodePointInfo> = DashMap::new();
    let workspace_root = Some(std::path::Path::new(project_context.project_root()));
    let source_cache = crate::inline_allows::ConcurrentSourceFileCache::new();

    collect_crate_relationships_parallel(
        node,
        &points,
        file_interner,
        name_interner,
        None,
        None,
        None,
        project_context,
        workspace_root,
        &source_cache,
    );

    // Convert DashMap to HashMap for the subtree builder
    let points: CodePointMap = points.into_iter().collect();

    // All crate code points should be reported as roots.
    // Each point that can lead to a panic deserves its own entry,
    // regardless of whether it's also called by another crate function.
    // Children show what each function calls (toward the panic).
    let mut roots: Vec<CodePointKey> = points.keys().copied().collect();
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
                name: cached.name,
                file: cached.file,
                line: cached.line,
                column: cached.column,
                causes: cached.causes.clone(),
                children: vec![], // Don't clone children - they're at first occurrence
                is_direct_panic: cached.is_direct_panic,
                called_function: cached.called_function,
            });
        }

        path.insert(*key);

        let (name, causes, child_keys_set, is_direct_panic, called_function) = points.get(key)?;
        // Sort child keys for deterministic output
        let mut child_keys: Vec<_> = child_keys_set.iter().copied().collect();
        child_keys.sort();
        let mut children: Vec<CrateCodePoint> = child_keys
            .iter()
            .filter_map(|child_key| build_subtree(child_key, points, path, cache))
            .collect();
        children.sort_by_key(|a| (a.file, a.line, a.column));
        path.remove(key);

        let point = CrateCodePoint {
            name: *name,
            file: key.0,
            line: key.1,
            column: key.2,
            causes: causes.clone(),
            children,
            is_direct_panic: *is_direct_panic,
            called_function: *called_function,
        };

        // Cache a shallow copy (without children) for reuse by other parents.
        // The cache-hit path at line 361 always returns children: vec![],
        // so storing children in the cache is wasted memory and cloning cost.
        cache.insert(
            *key,
            CrateCodePoint {
                name: point.name,
                file: point.file,
                line: point.line,
                column: point.column,
                causes: point.causes.clone(),
                children: vec![],
                is_direct_panic: point.is_direct_panic,
                called_function: point.called_function,
            },
        );
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

/// Parallel version of `collect_crate_relationships` that uses `DashMap` for
/// concurrent code point collection and `par_iter` at each recursion level.
///
/// Each branch of the call tree carries its own cause/child context, while
/// the shared `DashMap` accumulates code points from all branches. Causes
/// are stored in `HashSet`s so concurrent insertions from different paths
/// are safe (duplicates are ignored).
#[allow(clippy::too_many_arguments)]
fn collect_crate_relationships_parallel(
    node: &CallTreeNode,
    points: &DashMap<CodePointKey, CodePointInfo>,
    interner: &FilePathInterner,
    name_interner: &NameInterner,
    child_crate_key: Option<CodePointKey>,
    current_cause: Option<PanicCause>,
    immediate_callee: Option<&str>,
    project_context: &ProjectContext,
    workspace_root: Option<&std::path::Path>,
    source_cache: &crate::inline_allows::ConcurrentSourceFileCache,
) {
    // Try to detect panic cause from this node's function name
    let node_name = name_interner.resolve(node.name).unwrap_or_default();
    let detected_cause = detect_panic_cause(&node_name).or(current_cause);

    // Check if file matches any of the patterns
    let file_path = interner.resolve(node.file);
    let file_matches = file_path
        .as_ref()
        .is_some_and(|f| project_context.is_crate_source(f));

    let node_key = if node.file != FILE_NONE && file_matches && node.line > 0 {
        Some((node.file, node.line, node.column))
    } else {
        None
    };

    if let Some(key) = &node_key {
        let is_direct = immediate_callee
            .map(is_panic_triggering_function)
            .unwrap_or(false);

        let called_fn_idx = if !is_direct {
            immediate_callee
                .map(|callee| {
                    let qualified = extract_qualified_function_name(callee);
                    name_interner.intern_owned(qualified)
                })
                .unwrap_or(NAME_NONE)
        } else {
            NAME_NONE
        };

        let mut entry = points.entry(*key).or_insert_with(|| {
            (
                node.name,
                HashSet::new(),
                HashSet::new(),
                is_direct,
                called_fn_idx,
            )
        });

        if is_direct {
            entry.3 = true;
            entry.4 = NAME_NONE;
        } else if entry.4 == NAME_NONE && called_fn_idx != NAME_NONE {
            entry.4 = called_fn_idx;
        }

        if let Some(cause) = &detected_cause {
            entry.1.insert(*cause);
        }

        if let Some(child_key) = &child_crate_key {
            entry.2.insert(*child_key);
        }
    }

    let propagated_cause = if let (Some(key), Some(cause)) = (&node_key, &detected_cause) {
        // Use the already-resolved file path for inline allow check
        let cause_id = cause.id();
        if let Some(ref fp) = file_path
            && crate::inline_allows::check_inline_allow_concurrent(
                fp,
                key.1,
                cause_id,
                workspace_root,
                source_cache,
            )
        {
            None
        } else {
            detected_cause
        }
    } else {
        detected_cause
    };

    let next_child = node_key.or(child_crate_key);

    if node.callers.len() >= 4 {
        node.callers.par_iter().for_each(|caller| {
            collect_crate_relationships_parallel(
                caller,
                points,
                interner,
                name_interner,
                next_child,
                propagated_cause,
                Some(node_name.as_str()),
                project_context,
                workspace_root,
                source_cache,
            );
        });
    } else {
        for caller in &node.callers {
            collect_crate_relationships_parallel(
                caller,
                points,
                interner,
                name_interner,
                next_child,
                propagated_cause,
                Some(node_name.as_str()),
                project_context,
                workspace_root,
                source_cache,
            );
        }
    }
}

/// Collect crate code points without printing.
/// Returns the filtered, deduplicated, and sorted code points along with summary.
///
/// The `workspace_root` parameter is used to resolve relative file paths for inline allow checks.
pub fn collect_crate_code_points(
    node: &CallTreeNode,
    config: &Config,
    project_context: &ProjectContext,
    file_interner: &FilePathInterner,
    name_interner: &NameInterner,
) -> (Vec<CrateCodePoint>, AnalysisSummary) {
    let mut roots =
        collect_crate_code_points_hierarchical(node, project_context, file_interner, name_interner);

    assign_unknown_causes(&mut roots);

    // Collect pre-filter locations for unused-rule detection
    collect_locations(
        &roots,
        file_interner,
        &mut PRE_FILTER_LOCATIONS.lock().unwrap(),
    );

    filter_allowed_causes(
        &mut roots,
        config,
        project_context,
        file_interner,
        name_interner,
    );

    // Deduplicate roots by (file, line, column)
    dedupe_crate_points(&mut roots);

    // Sort roots by file then line number
    roots.sort_by_key(|a| (a.file, a.line, a.column));

    let summary = count_crate_points_and_files(&roots, file_interner);
    (roots, summary)
}

/// Assign `Unknown` cause to points that have no identified causes.
/// This ensures every `CrateCodePoint` has at least one cause, making all
/// points filterable by allow mechanisms (inline comments, config rules).
///
/// Special case: if the point is a direct panic (user code directly calls a
/// panic-triggering function like `panic_fmt`), assign `ExplicitPanic` instead
/// of `Unknown`. This handles `panic!("literal")` in Rust 1.78+ where the macro
/// routes through `panic_fmt` without an intermediate function that
/// `detect_panic_cause` recognises.
fn assign_unknown_causes(points: &mut [CrateCodePoint]) {
    for point in points.iter_mut() {
        assign_unknown_causes(&mut point.children);

        if point.causes.is_empty() {
            if point.is_direct_panic {
                point.causes.insert(PanicCause::ExplicitPanic);
            } else {
                point.causes.insert(PanicCause::Unknown);
            }
        }
    }
}

/// Filter out code points whose causes are ALL allowed (not denied) by config or inline comments.
/// A point is kept if ANY of its causes is denied.
/// Also removes allowed causes from the causes set so only denied causes are displayed.
///
/// The `workspace_root` parameter is used to resolve relative file paths for inline allow checks.
pub fn filter_allowed_causes(
    points: &mut Vec<CrateCodePoint>,
    config: &Config,
    project_context: &ProjectContext,
    file_interner: &FilePathInterner,
    name_interner: &NameInterner,
) {
    let mut cache = crate::inline_allows::SourceFileCache::new();
    filter_allowed_causes_cached(
        points,
        config,
        project_context,
        file_interner,
        name_interner,
        &mut cache,
    );
}

/// Inner implementation of `filter_allowed_causes` that threads a source file cache
/// to avoid redundant disk reads during recursive filtering.
fn filter_allowed_causes_cached(
    points: &mut Vec<CrateCodePoint>,
    config: &Config,
    project_context: &ProjectContext,
    file_interner: &FilePathInterner,
    name_interner: &NameInterner,
    cache: &mut crate::inline_allows::SourceFileCache,
) {
    use crate::inline_allows::check_inline_allow;

    let workspace_root = Some(std::path::Path::new(project_context.project_root()));

    points.retain_mut(|point| {
        let file_path = file_interner.resolve(point.file).unwrap_or_default();
        let point_name = name_interner.resolve(point.name).unwrap_or_default();

        point.causes.retain(|cause| {
            let cause_id = cause.id();

            if check_inline_allow(
                &file_path,
                point.line,
                cause_id,
                workspace_root,
                Some(cache),
            ) {
                return false;
            }

            // Suppress arithmetic/shift overflow for safe arithmetic patterns:
            // - Safe arithmetic methods like checked_add, saturating_mul (issue #267)
            // - Const-only expressions that are provably safe (issue #266)
            if cause.is_debug_only() {
                if let Some(source_lines) =
                    crate::inline_allows::read_source_lines(&file_path, workspace_root, Some(cache))
                {
                    // Check if the source line uses a safe arithmetic method
                    if let Some(source_line) =
                        source_lines.get((point.line as usize).saturating_sub(1))
                    {
                        if crate::heuristics::is_safe_arithmetic_method(source_line) {
                            return false;
                        }
                    }

                    // Check if the expression is const-only and provably safe
                    if crate::heuristics::evaluate_const_arithmetic(&source_lines, point.line)
                        == Some(crate::heuristics::ConstArithmeticResult::Safe)
                    {
                        return false;
                    }
                }
            }

            if !config.is_denied_at(cause, Some(&file_path), Some(&point_name)) {
                return false;
            }

            if point.called_function != NAME_NONE {
                let called_fn = name_interner
                    .resolve(point.called_function)
                    .unwrap_or_default();
                if !config.is_denied_at(cause, Some(&file_path), Some(&called_fn)) {
                    return false;
                }
            }

            true
        });

        let should_keep = !point.causes.is_empty();

        if should_keep {
            filter_allowed_causes_cached(
                &mut point.children,
                config,
                project_context,
                file_interner,
                name_interner,
                cache,
            );
        }

        should_keep
    });
}

/// Complete analysis results ready for output rendering.
/// This is the shared structure used by all output formats (text, JSON, HTML).
/// Contains a `file_paths` table for resolving `u16` file indices in
/// `CrateCodePoint.file` back to path strings at the output boundary,
/// and a `name_paths` table for resolving `u16` name indices in
/// `CrateCodePoint.name` and `CrateCodePoint.called_function`.
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    /// Name of the project/crate
    pub project_name: String,
    /// Root path of the project
    pub project_root: String,
    /// All panic code points found (filtered and deduplicated)
    pub code_points: Vec<CrateCodePoint>,
    /// Resolved file path table for `CrateCodePoint.file` index resolution.
    /// Index `i` maps to the file path string for `CrateCodePoint.file == i`.
    pub file_paths: Vec<String>,
    /// Resolved name table for `CrateCodePoint.name` and `called_function` index resolution.
    /// Index `i` maps to the function name string for name index `i`.
    pub name_paths: Vec<String>,
}

impl AnalysisResult {
    /// Create a new analysis result
    pub fn new(
        project_name: impl Into<String>,
        project_root: impl Into<String>,
        code_points: Vec<CrateCodePoint>,
        file_paths: Vec<String>,
        name_paths: Vec<String>,
    ) -> Self {
        Self {
            project_name: project_name.into(),
            project_root: project_root.into(),
            code_points,
            file_paths,
            name_paths,
        }
    }

    /// Resolve a file index to a path string.
    /// Returns an empty string for `FILE_NONE` or out-of-bounds indices.
    pub fn resolve_file(&self, idx: u16) -> &str {
        if idx == FILE_NONE {
            ""
        } else {
            self.file_paths
                .get(idx as usize)
                .map(|s| s.as_str())
                .unwrap_or("")
        }
    }

    /// Resolve a name index to a function name string.
    /// Returns an empty string for `NAME_NONE` or out-of-bounds indices.
    pub fn resolve_name(&self, idx: u32) -> &str {
        if idx == NAME_NONE {
            ""
        } else {
            self.name_paths
                .get(idx as usize)
                .map(|s| s.as_str())
                .unwrap_or("")
        }
    }

    /// Compute the summary from the code points.
    /// Resolves file indices from `self.file_paths` for cross-interner safety.
    pub fn summary(&self) -> AnalysisSummary {
        let mut seen_points = HashSet::new();
        let mut seen_files = HashSet::new();
        Self::collect_summary_points(
            &self.code_points,
            &self.file_paths,
            &mut seen_points,
            &mut seen_files,
        );
        AnalysisSummary {
            points: seen_points,
            files: seen_files,
        }
    }

    fn collect_summary_points(
        points: &[CrateCodePoint],
        file_paths: &[String],
        seen_points: &mut HashSet<(String, u16, u16)>,
        seen_files: &mut HashSet<String>,
    ) {
        for p in points {
            let file = crate::string_tables::resolve_file_owned(file_paths, p.file);
            seen_files.insert(file.clone());
            seen_points.insert((file, p.line, p.column));
            Self::collect_summary_points(&p.children, file_paths, seen_points, seen_files);
        }
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
    /// Unique panic code points (file_path, line, column).
    /// Uses owned path strings (not interned indices) so that summaries from
    /// different `FilePathInterner` instances can be merged without collision.
    points: HashSet<(String, u16, u16)>,
    /// Unique files with panic points
    files: HashSet<String>,
}

impl AnalysisSummary {
    /// Create a new summary from collected points and files
    pub fn from_points(points: HashSet<(String, u16, u16)>, files: HashSet<String>) -> Self {
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

/// Count unique crate code points and files in the hierarchy.
/// Resolves file indices to path strings so summaries can be safely merged
/// across different `FilePathInterner` instances.
fn count_crate_points_and_files(
    points: &[CrateCodePoint],
    interner: &FilePathInterner,
) -> AnalysisSummary {
    let mut seen_points = HashSet::new();
    let mut seen_files = HashSet::new();
    collect_unique_point_keys_and_files(points, interner, &mut seen_points, &mut seen_files);
    AnalysisSummary {
        points: seen_points,
        files: seen_files,
    }
}

/// Collect unique (file_path, line, column) keys and unique file paths from the hierarchy
fn collect_unique_point_keys_and_files(
    points: &[CrateCodePoint],
    interner: &FilePathInterner,
    seen_points: &mut HashSet<(String, u16, u16)>,
    seen_files: &mut HashSet<String>,
) {
    for p in points {
        let file = interner.resolve(p.file).unwrap_or_default();
        seen_files.insert(file.clone());
        seen_points.insert((file, p.line, p.column));
        collect_unique_point_keys_and_files(&p.children, interner, seen_points, seen_files);
    }
}

/// Deduplicate crate code points by (file, line, column), merging children.
fn dedupe_crate_points(points: &mut Vec<CrateCodePoint>) {
    // Group by (file_idx, line, column) -- file is already a u32 index
    let mut seen: HashMap<CodePointKey, usize> = HashMap::new();
    let mut result: Vec<CrateCodePoint> = Vec::new();

    for point in points.drain(..) {
        let key = (point.file, point.line, point.column);
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

        let interner = FilePathInterner::new();
        let main_rs = interner.intern("src/main.rs");
        let name_interner = NameInterner::new();
        let name_app_run = name_interner.intern("app::run");
        let name_called = name_interner.intern("my_crate::time::TimeStamp::now");

        // Code point in "app::run" that calls "my_crate::time::TimeStamp::now" which may unwrap
        let mut points = vec![CrateCodePoint {
            name: name_app_run,
            file: main_rs,
            line: 42,
            column: 5,
            causes: vec![PanicCause::Unwrap].into_iter().collect(),
            children: vec![],
            is_direct_panic: false,
            called_function: name_called,
        }];

        let ctx = ProjectContext::default();
        filter_allowed_causes(&mut points, &config, &ctx, &interner, &name_interner);

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

        let interner = FilePathInterner::new();
        let device_rs = interner.intern("src/device.rs");
        let name_interner = NameInterner::new();
        let name_hash = name_interner.intern("meshchat::device::DeviceIdentifier::hash");
        let name_called = name_interner.intern("core::hash::Hash::hash");

        // Code point with only CapacityOverflow cause
        let mut points = vec![CrateCodePoint {
            name: name_hash,
            file: device_rs,
            line: 74,
            column: 22,
            causes: vec![PanicCause::CapacityOverflow].into_iter().collect(),
            children: vec![],
            is_direct_panic: false,
            called_function: name_called,
        }];

        let ctx = ProjectContext::default();
        filter_allowed_causes(&mut points, &config, &ctx, &interner, &name_interner);

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

        let interner = FilePathInterner::new();
        let device_rs = interner.intern("src/device.rs");
        let name_interner = NameInterner::new();
        let name_hash = name_interner.intern("meshchat::device::DeviceIdentifier::hash");
        let name_called = name_interner.intern("core::hash::Hash::hash");

        // Code point with BOTH CapacityOverflow and Unknown causes
        // (simulates multiple panic paths through the same code point)
        let mut points = vec![CrateCodePoint {
            name: name_hash,
            file: device_rs,
            line: 74,
            column: 22,
            causes: vec![PanicCause::CapacityOverflow, PanicCause::Unknown]
                .into_iter()
                .collect(),
            children: vec![],
            is_direct_panic: false,
            called_function: name_called,
        }];

        let ctx = ProjectContext::default();
        filter_allowed_causes(&mut points, &config, &ctx, &interner, &name_interner);

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

        let interner = FilePathInterner::new();
        let main_rs = interner.intern("src/main.rs");
        let name_interner = NameInterner::new();
        let name_app_run = name_interner.intern("app::run");
        let name_called = name_interner.intern("other_crate::Config::parse");

        // Code point calling a DIFFERENT function - should NOT be filtered
        let mut points = vec![CrateCodePoint {
            name: name_app_run,
            file: main_rs,
            line: 42,
            column: 5,
            causes: vec![PanicCause::Unwrap].into_iter().collect(),
            children: vec![],
            is_direct_panic: false,
            called_function: name_called,
        }];

        let ctx = ProjectContext::default();
        filter_allowed_causes(&mut points, &config, &ctx, &interner, &name_interner);

        // The point should be kept because the rule doesn't match Config::parse
        assert_eq!(
            points.len(),
            1,
            "Point should be kept - rule doesn't match different called function"
        );
    }

    #[test]
    fn test_assign_unknown_causes_non_leaf() {
        let mut points = vec![CrateCodePoint {
            name: 0u32,
            file: 0,
            line: 10,
            column: 1,
            causes: HashSet::new(),
            children: vec![CrateCodePoint {
                name: 1u32,
                file: 0,
                line: 20,
                column: 1,
                causes: HashSet::new(),
                children: vec![],
                is_direct_panic: false,
                called_function: NAME_NONE,
            }],
            is_direct_panic: false,
            called_function: NAME_NONE,
        }];

        assign_unknown_causes(&mut points);

        assert!(
            points[0].causes.contains(&PanicCause::Unknown),
            "Non-leaf point with empty causes should get Unknown assigned"
        );
        assert!(
            points[0].children[0].causes.contains(&PanicCause::Unknown),
            "Leaf child with empty causes should get Unknown assigned"
        );
    }

    #[test]
    fn test_filter_unknown_cause_with_allow() {
        use crate::config::Config;
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let toml_path = dir.path().join("jonesy.toml");
        let mut f = std::fs::File::create(&toml_path).unwrap();
        writeln!(f, "allow = [\"unknown\"]").unwrap();

        let mut config = Config::with_defaults();
        config.load_from_jones_toml(&toml_path);

        let interner = FilePathInterner::new();
        let main_rs = interner.intern("src/main.rs");
        let name_interner = NameInterner::new();
        let name_app_run = name_interner.intern("app::run");

        let mut points = vec![CrateCodePoint {
            name: name_app_run,
            file: main_rs,
            line: 42,
            column: 5,
            causes: vec![PanicCause::Unknown].into_iter().collect(),
            children: vec![],
            is_direct_panic: false,
            called_function: NAME_NONE,
        }];

        let ctx = ProjectContext::default();
        filter_allowed_causes(&mut points, &config, &ctx, &interner, &name_interner);

        assert!(
            points.is_empty(),
            "Point with Unknown cause should be filtered out by allow = [\"unknown\"]"
        );
    }

    #[test]
    fn test_analysis_result_resolve_file() {
        let result = AnalysisResult::new(
            "test",
            "/project",
            Vec::new(),
            vec!["src/main.rs".to_string(), "src/lib.rs".to_string()],
            Vec::new(),
        );
        assert_eq!(result.resolve_file(0), "src/main.rs");
        assert_eq!(result.resolve_file(1), "src/lib.rs");
        assert_eq!(result.resolve_file(2), ""); // out of bounds
        assert_eq!(result.resolve_file(FILE_NONE), ""); // sentinel
    }

    #[test]
    fn test_analysis_result_summary_counts() {
        let interner = FilePathInterner::new();
        let file_a = interner.intern("src/a.rs");
        let file_b = interner.intern("src/b.rs");
        let name_interner = NameInterner::new();
        let name_fn_a = name_interner.intern("fn_a");
        let name_fn_b = name_interner.intern("fn_b");
        let name_fn_a2 = name_interner.intern("fn_a2");

        let result = AnalysisResult::new(
            "test",
            "/project",
            vec![
                CrateCodePoint {
                    name: name_fn_a,
                    file: file_a,
                    line: 10,
                    column: 5,
                    causes: vec![PanicCause::Unwrap].into_iter().collect(),
                    children: vec![],
                    is_direct_panic: true,
                    called_function: NAME_NONE,
                },
                CrateCodePoint {
                    name: name_fn_b,
                    file: file_b,
                    line: 20,
                    column: 3,
                    causes: vec![PanicCause::Expect].into_iter().collect(),
                    children: vec![],
                    is_direct_panic: true,
                    called_function: NAME_NONE,
                },
                CrateCodePoint {
                    name: name_fn_a2,
                    file: file_a,
                    line: 15,
                    column: 1,
                    causes: vec![PanicCause::ExplicitPanic].into_iter().collect(),
                    children: vec![],
                    is_direct_panic: true,
                    called_function: NAME_NONE,
                },
            ],
            interner.into_vec(),
            name_interner.into_vec(),
        );

        let summary = result.summary();
        assert_eq!(summary.panic_points(), 3);
        assert_eq!(summary.files_affected(), 2);
    }

    #[test]
    fn test_analysis_result_summary_with_children() {
        let interner = FilePathInterner::new();
        let file_idx = interner.intern("src/main.rs");
        let name_interner = NameInterner::new();
        let name_parent = name_interner.intern("parent");
        let name_child = name_interner.intern("child");

        let result = AnalysisResult::new(
            "test",
            "/project",
            vec![CrateCodePoint {
                name: name_parent,
                file: file_idx,
                line: 10,
                column: 0,
                causes: vec![PanicCause::Unknown].into_iter().collect(),
                children: vec![CrateCodePoint {
                    name: name_child,
                    file: file_idx,
                    line: 20,
                    column: 0,
                    causes: vec![PanicCause::Unwrap].into_iter().collect(),
                    children: vec![],
                    is_direct_panic: true,
                    called_function: NAME_NONE,
                }],
                is_direct_panic: false,
                called_function: NAME_NONE,
            }],
            interner.into_vec(),
            name_interner.into_vec(),
        );

        let summary = result.summary();
        assert_eq!(summary.panic_points(), 2); // parent + child
        assert_eq!(summary.files_affected(), 1); // same file
    }

    #[test]
    fn test_dedupe_crate_points_merges_same_location() {
        let interner = FilePathInterner::new();
        let file_idx = interner.intern("src/main.rs");

        let mut points = vec![
            CrateCodePoint {
                name: 0u32,
                file: file_idx,
                line: 10,
                column: 5,
                causes: vec![PanicCause::Unwrap].into_iter().collect(),
                children: vec![],
                is_direct_panic: true,
                called_function: NAME_NONE,
            },
            CrateCodePoint {
                name: 0u32,
                file: file_idx,
                line: 10,
                column: 5,
                causes: vec![PanicCause::Expect].into_iter().collect(),
                children: vec![],
                is_direct_panic: true,
                called_function: NAME_NONE,
            },
        ];

        dedupe_crate_points(&mut points);
        assert_eq!(points.len(), 1);
        // Children from the duplicate should be merged
    }

    #[test]
    fn test_analysis_summary_add_cross_interner() {
        // Simulate two interners with index 0 mapping to different files
        let mut summary_a = AnalysisSummary::from_points(
            vec![("src/a.rs".to_string(), 10, 5)].into_iter().collect(),
            vec!["src/a.rs".to_string()].into_iter().collect(),
        );

        let summary_b = AnalysisSummary::from_points(
            vec![("src/b.rs".to_string(), 10, 5)].into_iter().collect(),
            vec!["src/b.rs".to_string()].into_iter().collect(),
        );

        summary_a.add(&summary_b);

        // Even though both had the same line:column, different files => 2 points
        assert_eq!(summary_a.panic_points(), 2);
        assert_eq!(summary_a.files_affected(), 2);
    }
}
