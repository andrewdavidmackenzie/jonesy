//! Interned string tables for function names and file paths.
//!
//! Reduces memory by deduplicating strings and enables compact FunctionInfo.

use std::collections::HashMap;

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
}
