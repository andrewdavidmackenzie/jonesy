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
