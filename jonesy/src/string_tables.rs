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

/// Thread-safe file path interner for use during code point collection.
///
/// Uses a `DashMap` for concurrent interning (safe with `par_iter`) and an
/// `AtomicU32` counter for allocating new indices. After the parallel phase,
/// call `into_vec()` to get a `Vec<String>` for index-to-path resolution.
///
/// The index `0` is valid (unlike `StringTables::intern_file` which uses 0 as
/// a sentinel for `None`). Every interned path gets a non-negative index.
#[derive(Debug)]
pub struct FilePathInterner {
    map: DashMap<String, u32>,
    next_id: AtomicU32,
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
        }
    }

    /// Intern a file path, returning its index.
    /// Thread-safe: can be called concurrently from multiple rayon threads.
    /// If the path is already interned, returns the existing index without allocating.
    pub fn intern(&self, path: &str) -> u32 {
        // Fast path: check if already interned
        if let Some(entry) = self.map.get(path) {
            return *entry.value();
        }

        // Slow path: allocate a new ID and insert
        // Use entry API to handle races -- two threads may both reach here for the same path.
        // The `or_insert_with` closure runs at most once per unique key.
        *self
            .map
            .entry(path.to_string())
            .or_insert_with(|| self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Intern an owned String, avoiding a clone if not yet interned.
    pub fn intern_owned(&self, path: String) -> u32 {
        if let Some(entry) = self.map.get(&path) {
            return *entry.value();
        }

        *self
            .map
            .entry(path)
            .or_insert_with(|| self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Resolve an index back to its file path string.
    /// Returns `None` if the index is out of bounds.
    ///
    /// **Note:** This performs a linear scan of the DashMap. For bulk resolution,
    /// prefer converting to a `Vec<String>` with `into_vec()` first.
    pub fn resolve(&self, idx: u32) -> Option<String> {
        self.map
            .iter()
            .find(|entry| *entry.value() == idx)
            .map(|entry| entry.key().clone())
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
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Check if the interner is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
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
}
