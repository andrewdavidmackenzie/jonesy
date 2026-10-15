//! Interned string tables for function names and file paths.
//!
//! Reduces memory by deduplicating strings and enables compact FunctionInfo.
//!
//! `FilePathInterner` provides a thread-safe file path interning table for use
//! during code point collection, where file paths are frequently cloned as part
//! of `CodePointKey` tuples. By interning to a `u32` index, we eliminate string
//! cloning in hot paths and make hashing/comparison of keys much cheaper.

use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// Interned string tables for function names and file paths.
/// Reduces memory by deduplicating strings and enables compact FunctionInfo.
#[derive(Debug, Default)]
pub struct StringTables {
    /// Deduplicated function names
    names: Vec<String>,
    /// Map for O(1) name lookup during interning
    name_map: HashMap<String, u32>,
    /// Deduplicated file paths
    files: Vec<String>,
    /// Map for O(1) file lookup during interning
    file_map: HashMap<String, u32>,
}

impl StringTables {
    /// Create empty string tables
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern a function name, returning its index.
    /// If the name already exists, returns the existing index.
    pub fn intern_name(&mut self, name: String) -> u32 {
        if let Some(&idx) = self.name_map.get(&name) {
            idx
        } else {
            let idx = self.names.len() as u32;
            self.name_map.insert(name.clone(), idx);
            self.names.push(name);
            idx
        }
    }

    /// Intern a file path, returning its index + 1.
    /// Returns 0 for None. Index is offset by 1 to use 0 as sentinel.
    pub fn intern_file(&mut self, file: Option<String>) -> u32 {
        match file {
            None => 0,
            Some(f) => {
                if let Some(&idx) = self.file_map.get(&f) {
                    idx + 1 // Offset by 1 (0 = None)
                } else {
                    let idx = self.files.len() as u32;
                    self.file_map.insert(f.clone(), idx);
                    self.files.push(f);
                    idx + 1 // Offset by 1 (0 = None)
                }
            }
        }
    }

    /// Get a name by index
    #[inline]
    pub fn get_name(&self, idx: u32) -> &str {
        &self.names[idx as usize]
    }

    /// Get a file by index (0 = None)
    #[inline]
    pub fn get_file(&self, idx: u32) -> Option<&str> {
        if idx == 0 {
            None
        } else {
            Some(&self.files[(idx - 1) as usize])
        }
    }
}

/// Thread-safe interning table for function names and file paths.
///
/// Uses `DashMap` for concurrent interning during rayon `par_iter` phases,
/// then converts to a sequential `StringTables` via `into_string_tables()`
/// for fast O(1) lookups during the analysis phase.
///
/// Uses the same index scheme as `StringTables`: name indices start at 0,
/// file indices use 0 as a sentinel for `None` (actual indices are offset by 1).
#[derive(Debug)]
pub struct ConcurrentStringTables {
    names: DashMap<String, u32>,
    next_name_id: AtomicU32,
    files: DashMap<String, u32>,
    next_file_id: AtomicU32,
}

impl Default for ConcurrentStringTables {
    fn default() -> Self {
        Self::new()
    }
}

impl ConcurrentStringTables {
    /// Create empty concurrent string tables.
    pub fn new() -> Self {
        Self {
            names: DashMap::new(),
            next_name_id: AtomicU32::new(0),
            files: DashMap::new(),
            next_file_id: AtomicU32::new(0),
        }
    }

    /// Intern a function name, returning its index.
    /// Thread-safe: can be called concurrently from multiple rayon threads.
    pub fn intern_name(&self, name: String) -> u32 {
        if let Some(entry) = self.names.get(&name) {
            return *entry.value();
        }
        *self
            .names
            .entry(name)
            .or_insert_with(|| self.next_name_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Intern a file path, returning its index + 1.
    /// Returns 0 for `None`. Index is offset by 1 to use 0 as sentinel.
    /// Thread-safe: can be called concurrently from multiple rayon threads.
    pub fn intern_file(&self, file: Option<String>) -> u32 {
        match file {
            None => 0,
            Some(f) => {
                if let Some(entry) = self.files.get(&f) {
                    return *entry.value() + 1;
                }
                let idx = *self
                    .files
                    .entry(f)
                    .or_insert_with(|| self.next_file_id.fetch_add(1, Ordering::Relaxed));
                idx + 1
            }
        }
    }

    /// Convert to a sequential `StringTables` for fast O(1) lookups.
    /// Consumes self, producing the compact lookup table used by `FunctionIndex`.
    pub fn into_string_tables(self) -> StringTables {
        let name_count = self.next_name_id.load(Ordering::Relaxed) as usize;
        let mut names = vec![String::new(); name_count];
        let mut name_map = HashMap::with_capacity(name_count);
        for entry in self.names.into_iter() {
            let idx = entry.1 as usize;
            if idx < names.len() {
                name_map.insert(entry.0.clone(), entry.1);
                names[idx] = entry.0;
            }
        }

        let file_count = self.next_file_id.load(Ordering::Relaxed) as usize;
        let mut files = vec![String::new(); file_count];
        let mut file_map = HashMap::with_capacity(file_count);
        for entry in self.files.into_iter() {
            let idx = entry.1 as usize;
            if idx < files.len() {
                file_map.insert(entry.0.clone(), entry.1);
                files[idx] = entry.0;
            }
        }

        StringTables {
            names,
            name_map,
            files,
            file_map,
        }
    }
}

/// Sentinel value meaning "no file path". Used by `CallerInfo.file`,
/// `CallTreeNode.file`, and `CrateCodePoint.file` to represent the absence
/// of a file path without needing `Option<u16>`.
/// The maximum valid index is 65534, supporting up to 65535 unique file paths
/// which is sufficient for any realistic project.
///
/// Also used as `NAME_NONE` for function name indices (same sentinel value).
pub const FILE_NONE: u16 = u16::MAX;

/// Sentinel value meaning "no function name". Same value as `FILE_NONE`
/// but semantically distinct — used for `CallerInfo.caller_name`,
/// `CrateCodePoint.called_function`, etc.
pub const NAME_NONE: u16 = u16::MAX;

/// Thread-safe string interner for concurrent use during rayon `par_iter` phases.
///
/// Uses a `DashMap` for concurrent interning (safe with `par_iter`) and an
/// `AtomicU32` counter for lock-free ID allocation. Valid indices are `u16`,
/// supporting up to 65535 unique file paths (more than any realistic project).
/// After the parallel phase, call `into_vec()` to produce a `Vec<String>` for
/// index-to-path resolution.
///
/// Valid indices start at 0. The sentinel `FILE_NONE` (`u16::MAX`) represents
/// the absence of a file path.
///
/// A concurrent reverse index (`RwLock<Vec<String>>`) is maintained for O(1)
/// index-to-path resolution during the parallel phase, avoiding expensive
/// `DashMap` linear scans.
#[derive(Debug)]
pub struct FilePathInterner {
    map: DashMap<String, u16>,
    /// Atomic counter for allocating new indices. Uses `AtomicU32` internally
    /// and saturates at `u16::MAX - 1` to avoid colliding with `FILE_NONE`.
    next_id: AtomicU32,
    /// Reverse index: id -> path string. Grown under a write lock on insertion,
    /// read without contention during lookups.
    reverse: std::sync::RwLock<Vec<String>>,
}

impl Default for FilePathInterner {
    fn default() -> Self {
        Self::new()
    }
}

impl FilePathInterner {
    /// Create a new empty interner.
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
            next_id: AtomicU32::new(0),
            reverse: std::sync::RwLock::new(Vec::new()),
        }
    }

    /// Create a new interner with pre-allocated capacity.
    /// Use when the approximate number of unique strings is known or estimable.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            map: DashMap::with_capacity(capacity),
            next_id: AtomicU32::new(0),
            reverse: std::sync::RwLock::new(Vec::with_capacity(capacity)),
        }
    }

    /// Pre-size the interner if it is still empty.
    /// Called when the expected number of entries becomes known (e.g., after
    /// reading the binary but before the parallel analysis phase).
    /// No-op if the interner already contains entries.
    pub fn hint_capacity(&self, capacity: usize) {
        if self.next_id.load(Ordering::Relaxed) == 0 {
            // Pre-size the reverse index Vec (the DashMap grows dynamically
            // and cannot be reserved through a shared reference).
            let mut rev = self.reverse.write().unwrap();
            rev.reserve(capacity);
        }
    }

    /// Intern a file path, returning its `u16` index.
    /// Thread-safe: can be called concurrently from multiple rayon threads.
    /// If the path is already interned, returns the existing index without allocating.
    /// Saturates at `FILE_NONE - 1` if more than 65534 unique paths are interned.
    pub fn intern(&self, path: &str) -> u16 {
        // Fast path: check if already interned
        if let Some(entry) = self.map.get(path) {
            return *entry.value();
        }

        // Slow path: allocate a new ID and insert
        let path_owned = path.to_string();
        *self.map.entry(path_owned.clone()).or_insert_with(|| {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let idx = id.min((FILE_NONE - 1) as u32) as u16;
            // Maintain reverse index for O(1) resolve
            let mut rev = self.reverse.write().unwrap();
            if (idx as usize) >= rev.len() {
                rev.resize(idx as usize + 1, String::new());
            }
            rev[idx as usize] = path_owned;
            idx
        })
    }

    /// Intern an owned String, avoiding a clone if not yet interned.
    pub fn intern_owned(&self, path: String) -> u16 {
        if let Some(entry) = self.map.get(&path) {
            return *entry.value();
        }

        let path_clone = path.clone();
        *self.map.entry(path).or_insert_with(|| {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let idx = id.min((FILE_NONE - 1) as u32) as u16;
            let mut rev = self.reverse.write().unwrap();
            if (idx as usize) >= rev.len() {
                rev.resize(idx as usize + 1, String::new());
            }
            rev[idx as usize] = path_clone;
            idx
        })
    }

    /// Intern an optional file path. Returns `FILE_NONE` for `None`.
    pub fn intern_option(&self, path: Option<&str>) -> u16 {
        match path {
            Some(p) => self.intern(p),
            None => FILE_NONE,
        }
    }

    /// Resolve an index back to its file path string.
    /// Returns `None` for `FILE_NONE` or if the index is not found.
    /// Uses the reverse index for O(1) lookup.
    pub(crate) fn resolve(&self, idx: u16) -> Option<String> {
        if idx == FILE_NONE {
            return None;
        }
        let rev = self.reverse.read().unwrap();
        rev.get(idx as usize).filter(|s| !s.is_empty()).cloned()
    }

    /// Consume the interner and produce a `Vec<String>` indexed by the interned IDs.
    /// This is the efficient way to do bulk index-to-path resolution after the parallel phase.
    pub fn into_vec(self) -> Vec<String> {
        let count = self.next_id.load(Ordering::Relaxed) as usize;
        let mut result = vec![String::new(); count];
        for entry in self.map.into_iter() {
            let idx = entry.1 as usize;
            if idx < result.len() {
                result[idx] = entry.0;
            }
        }
        result
    }

    /// Get the number of interned paths.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }
}

/// Resolve a file index to a path string from a resolved file path table.
/// Returns `None` for `FILE_NONE` or out-of-bounds indices.
#[inline]
pub fn resolve_file(file_paths: &[String], idx: u16) -> Option<&str> {
    if idx == FILE_NONE {
        None
    } else {
        file_paths.get(idx as usize).map(|s| s.as_str())
    }
}

/// Resolve a file index to an owned path string from a resolved file path table.
/// Returns an empty string for `FILE_NONE` or out-of-bounds indices.
#[inline]
pub fn resolve_file_owned(file_paths: &[String], idx: u16) -> String {
    if idx == FILE_NONE {
        String::new()
    } else {
        file_paths.get(idx as usize).cloned().unwrap_or_default()
    }
}

/// Resolve a name index to a function name string from a resolved name table.
/// Returns `None` for `NAME_NONE` or out-of-bounds indices.
#[inline]
pub fn resolve_name(name_paths: &[String], idx: u16) -> Option<&str> {
    if idx == NAME_NONE {
        None
    } else {
        name_paths.get(idx as usize).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intern_name_returns_same_index_for_same_string() {
        let mut tables = StringTables::new();
        let idx1 = tables.intern_name("my_function".to_string());
        let idx2 = tables.intern_name("my_function".to_string());
        assert_eq!(idx1, idx2);
    }

    #[test]
    fn test_intern_name_different_indexes_for_different_strings() {
        let mut tables = StringTables::new();
        let idx1 = tables.intern_name("func1".to_string());
        let idx2 = tables.intern_name("func2".to_string());
        assert_ne!(idx1, idx2);
    }

    #[test]
    fn test_intern_file_returns_zero_for_none() {
        let mut tables = StringTables::new();
        let idx = tables.intern_file(None);
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_intern_file_returns_same_index_for_same_path() {
        let mut tables = StringTables::new();
        let idx1 = tables.intern_file(Some("/path/to/file.rs".to_string()));
        let idx2 = tables.intern_file(Some("/path/to/file.rs".to_string()));
        assert_eq!(idx1, idx2);
    }

    #[test]
    fn test_intern_file_different_indexes_for_different_paths() {
        let mut tables = StringTables::new();
        let idx1 = tables.intern_file(Some("/path/to/file1.rs".to_string()));
        let idx2 = tables.intern_file(Some("/path/to/file2.rs".to_string()));
        assert_ne!(idx1, idx2);
    }

    #[test]
    fn test_get_name_returns_interned_string() {
        let mut tables = StringTables::new();
        let idx = tables.intern_name("test_function".to_string());
        assert_eq!(tables.get_name(idx), "test_function");
    }

    #[test]
    fn test_get_file_returns_none_for_index_zero() {
        let tables = StringTables::new();
        assert_eq!(tables.get_file(0), None);
    }

    #[test]
    fn test_get_file_returns_interned_path() {
        let mut tables = StringTables::new();
        let idx = tables.intern_file(Some("/src/main.rs".to_string()));
        assert_eq!(tables.get_file(idx), Some("/src/main.rs"));
    }

    #[test]
    fn test_intern_file_none_then_some() {
        let mut tables = StringTables::new();
        let idx_none = tables.intern_file(None);
        let idx_some = tables.intern_file(Some("/file.rs".to_string()));
        assert_eq!(idx_none, 0);
        assert_ne!(idx_some, 0);
        assert_eq!(tables.get_file(idx_none), None);
        assert_eq!(tables.get_file(idx_some), Some("/file.rs"));
    }

    #[test]
    fn test_file_path_interner_basic() {
        let interner = FilePathInterner::new();
        let idx1 = interner.intern("src/main.rs");
        let idx2 = interner.intern("src/main.rs");
        let idx3 = interner.intern("src/lib.rs");
        assert_eq!(idx1, idx2, "same path should return same index");
        assert_ne!(
            idx1, idx3,
            "different paths should return different indices"
        );
        assert_eq!(interner.len(), 2);
    }

    #[test]
    fn test_file_path_interner_intern_owned() {
        let interner = FilePathInterner::new();
        let idx1 = interner.intern_owned("src/main.rs".to_string());
        let idx2 = interner.intern("src/main.rs");
        assert_eq!(idx1, idx2);
    }

    #[test]
    fn test_file_path_interner_resolve() {
        let interner = FilePathInterner::new();
        let idx = interner.intern("src/main.rs");
        assert_eq!(interner.resolve(idx), Some("src/main.rs".to_string()));
        assert_eq!(interner.resolve(999), None);
    }

    #[test]
    fn test_file_path_interner_into_vec() {
        let interner = FilePathInterner::new();
        let idx_a = interner.intern("aaa.rs");
        let idx_b = interner.intern("bbb.rs");
        let vec = interner.into_vec();
        assert_eq!(vec.len(), 2);
        assert_eq!(vec[idx_a as usize], "aaa.rs");
        assert_eq!(vec[idx_b as usize], "bbb.rs");
    }

    #[test]
    fn test_file_path_interner_intern_option() {
        let interner = FilePathInterner::new();
        let idx_some = interner.intern_option(Some("src/main.rs"));
        let idx_none = interner.intern_option(None);
        assert_ne!(idx_some, FILE_NONE);
        assert_eq!(idx_none, FILE_NONE);
        assert_eq!(interner.resolve(idx_some), Some("src/main.rs".to_string()));
        assert_eq!(interner.resolve(idx_none), None);
    }

    #[test]
    fn test_file_path_interner_resolve_file_none() {
        let interner = FilePathInterner::new();
        assert_eq!(interner.resolve(FILE_NONE), None);
    }

    #[test]
    fn test_file_path_interner_default() {
        let interner = FilePathInterner::default();
        assert_eq!(interner.len(), 0);
    }

    #[test]
    fn test_file_path_interner_concurrent() {
        use rayon::prelude::*;

        let interner = FilePathInterner::new();
        let paths: Vec<String> = (0..100).map(|i| format!("src/file_{i}.rs")).collect();

        // Intern all paths concurrently
        let indices: Vec<u16> = paths.par_iter().map(|p| interner.intern(p)).collect();

        // Verify all paths were interned with unique indices
        assert_eq!(interner.len(), 100);

        // Re-interning the same paths should return the same indices
        let indices2: Vec<u16> = paths.par_iter().map(|p| interner.intern(p)).collect();
        assert_eq!(indices, indices2);

        // Resolve should return the correct path for each index
        for (i, idx) in indices.iter().enumerate() {
            assert_eq!(
                interner.resolve(*idx),
                Some(paths[i].clone()),
                "index {idx} should resolve to {}",
                paths[i]
            );
        }
    }

    #[test]
    fn test_resolve_file_function() {
        let file_paths = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        assert_eq!(resolve_file(&file_paths, 0), Some("src/main.rs"));
        assert_eq!(resolve_file(&file_paths, 1), Some("src/lib.rs"));
        assert_eq!(resolve_file(&file_paths, 2), None); // out of bounds
        assert_eq!(resolve_file(&file_paths, FILE_NONE), None);
    }

    #[test]
    fn test_resolve_file_owned_function() {
        let file_paths = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        assert_eq!(resolve_file_owned(&file_paths, 0), "src/main.rs");
        assert_eq!(resolve_file_owned(&file_paths, 1), "src/lib.rs");
        assert_eq!(resolve_file_owned(&file_paths, 2), ""); // out of bounds
        assert_eq!(resolve_file_owned(&file_paths, FILE_NONE), ""); // sentinel
    }

    #[test]
    fn test_resolve_file_empty_table() {
        let file_paths: Vec<String> = Vec::new();
        assert_eq!(resolve_file(&file_paths, 0), None);
        assert_eq!(resolve_file_owned(&file_paths, 0), "");
    }
}
