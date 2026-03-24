//! Analysis cache for incremental re-analysis.
//!
//! Stores analysis state in `target/jonesy/` to avoid unnecessary re-analysis.
//! Tracks:
//! - Binary/library modification times
//! - Cargo.toml content hashes (for target/member changes)
//! - jonesy.toml content hash (for rule changes)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Cache file location within target directory.
const CACHE_FILE: &str = "jonesy/cache.json";

/// Cached state for a single target (binary or library).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetState {
    /// Path to the target file.
    pub path: PathBuf,
    /// Last modification time (as milliseconds since epoch for subsecond precision).
    pub mtime: u128,
    /// Number of panic points found in last analysis.
    pub panic_count: usize,
}

/// Cached state for a config file (Cargo.toml or jonesy.toml).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigState {
    /// Path to the config file.
    pub path: PathBuf,
    /// Hash of relevant content (for change detection).
    pub content_hash: u64,
}

/// Workspace structure snapshot for detecting membership changes.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceState {
    /// Workspace members (crate names).
    pub members: Vec<String>,
    /// Binary targets (name -> path).
    pub binaries: HashMap<String, PathBuf>,
    /// Library targets (name -> path).
    pub libraries: HashMap<String, PathBuf>,
}

/// The full analysis cache.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnalysisCache {
    /// Cache format version (for migration).
    pub version: u32,
    /// Cached target states.
    pub targets: HashMap<PathBuf, TargetState>,
    /// Cached config file states.
    pub configs: HashMap<PathBuf, ConfigState>,
    /// Last known workspace structure.
    pub workspace: WorkspaceState,
}

impl AnalysisCache {
    /// Current cache format version.
    const VERSION: u32 = 1;

    /// Load cache from disk, or return empty cache if not found/invalid.
    pub fn load(workspace_root: &Path) -> Self {
        let cache_path = workspace_root.join("target").join(CACHE_FILE);

        let cache = fs::read_to_string(&cache_path)
            .ok()
            .and_then(|content| serde_json::from_str::<AnalysisCache>(&content).ok())
            .filter(|cache| cache.version == Self::VERSION)
            .unwrap_or_default();

        // Return with current version
        Self {
            version: Self::VERSION,
            ..cache
        }
    }

    /// Save cache to disk.
    pub fn save(&self, workspace_root: &Path) -> Result<(), String> {
        let cache_dir = workspace_root.join("target/jonesy");
        fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create cache directory: {}", e))?;

        let cache_path = cache_dir.join("cache.json");
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize cache: {}", e))?;

        fs::write(&cache_path, content).map_err(|e| format!("Failed to write cache: {}", e))?;

        Ok(())
    }

    /// Check if a target needs re-analysis based on modification time.
    pub fn target_needs_analysis(&self, target_path: &Path) -> bool {
        let Some(cached) = self.targets.get(target_path) else {
            return true; // Not in cache, needs analysis
        };

        // Check if file has been modified
        let current_mtime = get_mtime(target_path).unwrap_or(0);
        current_mtime != cached.mtime
    }

    /// Update cache after analyzing a target.
    pub fn update_target(&mut self, target_path: &Path, panic_count: usize) {
        let mtime = get_mtime(target_path).unwrap_or(0);
        self.targets.insert(
            target_path.to_path_buf(),
            TargetState {
                path: target_path.to_path_buf(),
                mtime,
                panic_count,
            },
        );
    }

    /// Check if a config file has changed.
    pub fn config_changed(&self, config_path: &Path) -> bool {
        let Some(cached) = self.configs.get(config_path) else {
            return true; // Not in cache
        };

        let current_hash = hash_file_content(config_path).unwrap_or(0);
        current_hash != cached.content_hash
    }

    /// Update cache for a config file.
    pub fn update_config(&mut self, config_path: &Path) {
        let content_hash = hash_file_content(config_path).unwrap_or(0);
        self.configs.insert(
            config_path.to_path_buf(),
            ConfigState {
                path: config_path.to_path_buf(),
                content_hash,
            },
        );
    }

    /// Check what kind of workspace changes occurred.
    pub fn detect_workspace_changes(&self, current: &WorkspaceState) -> WorkspaceChanges {
        let mut changes = WorkspaceChanges::default();

        // Check for added/removed members
        for member in &current.members {
            if !self.workspace.members.contains(member) {
                changes.added_members.push(member.clone());
            }
        }
        for member in &self.workspace.members {
            if !current.members.contains(member) {
                changes.removed_members.push(member.clone());
            }
        }

        // Check for added/removed/changed binaries
        for (name, path) in &current.binaries {
            match self.workspace.binaries.get(name) {
                None => changes.added_binaries.push(name.clone()),
                Some(old_path) if old_path != path => changes.changed_binaries.push(name.clone()),
                _ => {}
            }
        }
        for name in self.workspace.binaries.keys() {
            if !current.binaries.contains_key(name) {
                changes.removed_binaries.push(name.clone());
            }
        }

        // Check for added/removed/changed libraries
        for (name, path) in &current.libraries {
            match self.workspace.libraries.get(name) {
                None => changes.added_libraries.push(name.clone()),
                Some(old_path) if old_path != path => changes.changed_libraries.push(name.clone()),
                _ => {}
            }
        }
        for name in self.workspace.libraries.keys() {
            if !current.libraries.contains_key(name) {
                changes.removed_libraries.push(name.clone());
            }
        }

        changes
    }

    /// Update the workspace state snapshot.
    pub fn update_workspace(&mut self, state: WorkspaceState) {
        self.workspace = state;
    }

    /// Remove stale entries for targets that no longer exist.
    pub fn prune_stale_targets(&mut self) {
        self.targets.retain(|path, _| path.exists());
    }
}

/// Detected changes in workspace structure.
#[derive(Debug, Default)]
pub struct WorkspaceChanges {
    pub added_members: Vec<String>,
    pub removed_members: Vec<String>,
    pub added_binaries: Vec<String>,
    pub removed_binaries: Vec<String>,
    pub changed_binaries: Vec<String>,
    pub added_libraries: Vec<String>,
    pub removed_libraries: Vec<String>,
    pub changed_libraries: Vec<String>,
}

impl WorkspaceChanges {
    /// Check if any changes were detected.
    pub fn has_changes(&self) -> bool {
        !self.added_members.is_empty()
            || !self.removed_members.is_empty()
            || !self.added_binaries.is_empty()
            || !self.removed_binaries.is_empty()
            || !self.changed_binaries.is_empty()
            || !self.added_libraries.is_empty()
            || !self.removed_libraries.is_empty()
            || !self.changed_libraries.is_empty()
    }

    /// Check if full re-analysis is needed (member changes affect everything).
    pub fn needs_full_reanalysis(&self) -> bool {
        !self.added_members.is_empty() || !self.removed_members.is_empty()
    }

    /// Get targets that need re-analysis.
    pub fn affected_targets(&self) -> Vec<String> {
        let mut targets = Vec::new();
        targets.extend(self.added_binaries.iter().cloned());
        targets.extend(self.changed_binaries.iter().cloned());
        targets.extend(self.added_libraries.iter().cloned());
        targets.extend(self.changed_libraries.iter().cloned());
        targets
    }

    /// Check if a specific target path is affected by workspace changes.
    ///
    /// This checks if the target's name (derived from its file path) matches
    /// any added or changed binaries/libraries.
    pub fn affects_target(&self, target_path: &Path) -> bool {
        let target_name = target_path
            .file_stem()
            .and_then(|n| n.to_str())
            .map(|n| n.strip_prefix("lib").unwrap_or(n))
            .unwrap_or("");

        self.added_binaries.iter().any(|n| n == target_name)
            || self.changed_binaries.iter().any(|n| n == target_name)
            || self.added_libraries.iter().any(|n| n == target_name)
            || self.changed_libraries.iter().any(|n| n == target_name)
    }
}

/// Get file modification time as milliseconds since epoch for subsecond precision.
fn get_mtime(path: &Path) -> Option<u128> {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
}

/// Simple hash of file content for change detection.
fn hash_file_content(path: &Path) -> Option<u64> {
    let content = fs::read(path).ok()?;
    // Simple FNV-1a hash
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in content {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Some(hash)
}

/// Build current workspace state from Cargo.toml files.
pub fn build_workspace_state(workspace_root: &Path) -> WorkspaceState {
    let mut state = WorkspaceState::default();

    let cargo_toml = workspace_root.join("Cargo.toml");
    let Ok(content) = fs::read_to_string(&cargo_toml) else {
        return state;
    };
    let Ok(manifest) = cargo_toml::Manifest::from_slice(content.as_bytes()) else {
        return state;
    };

    // Collect workspace members
    if let Some(workspace) = &manifest.workspace {
        for member in &workspace.members {
            if member.contains('*') {
                // Expand glob
                if let Ok(paths) = glob::glob(&workspace_root.join(member).to_string_lossy()) {
                    for path in paths.flatten() {
                        if path.is_dir() && path.join("Cargo.toml").exists() {
                            if let Some(name) = path.file_name() {
                                state.members.push(name.to_string_lossy().to_string());
                            }
                        }
                    }
                }
            } else {
                state.members.push(member.clone());
            }
        }
    }

    // Collect targets from root manifest
    collect_targets_from_manifest(&manifest, workspace_root, &mut state);

    // Collect targets from workspace members
    if let Some(workspace) = &manifest.workspace {
        for member in &workspace.members {
            let member_paths: Vec<PathBuf> = if member.contains('*') {
                glob::glob(&workspace_root.join(member).to_string_lossy())
                    .ok()
                    .map(|paths| paths.flatten().collect())
                    .unwrap_or_default()
            } else {
                vec![workspace_root.join(member)]
            };

            for member_path in member_paths {
                let member_cargo = member_path.join("Cargo.toml");
                if let Ok(content) = fs::read_to_string(&member_cargo) {
                    if let Ok(member_manifest) =
                        cargo_toml::Manifest::from_slice(content.as_bytes())
                    {
                        collect_targets_from_manifest(&member_manifest, &member_path, &mut state);
                    }
                }
            }
        }
    }

    state
}

/// Extract binary and library targets from a Cargo manifest.
fn collect_targets_from_manifest(
    manifest: &cargo_toml::Manifest,
    crate_root: &Path,
    state: &mut WorkspaceState,
) {
    let Some(pkg) = &manifest.package else {
        return;
    };

    // Collect binaries
    for bin in &manifest.bin {
        let name = bin.name.as_deref().unwrap_or(&pkg.name);
        let path = bin
            .path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| crate_root.join("src/main.rs"));
        state.binaries.insert(name.to_string(), path);
    }

    // If no explicit bins but has src/main.rs, add implicit bin
    if manifest.bin.is_empty() && crate_root.join("src/main.rs").exists() {
        state
            .binaries
            .insert(pkg.name.clone(), crate_root.join("src/main.rs"));
    }

    // Collect library
    if let Some(lib) = &manifest.lib {
        let name = lib.name.as_deref().unwrap_or(&pkg.name);
        let path = lib
            .path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| crate_root.join("src/lib.rs"));
        state.libraries.insert(name.to_string(), path);
    } else if crate_root.join("src/lib.rs").exists() {
        // Implicit library
        state
            .libraries
            .insert(pkg.name.clone(), crate_root.join("src/lib.rs"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_changes_detection() {
        let old = WorkspaceState {
            members: vec!["crate_a".to_string(), "crate_b".to_string()],
            binaries: [("app".to_string(), PathBuf::from("src/main.rs"))]
                .into_iter()
                .collect(),
            libraries: [("mylib".to_string(), PathBuf::from("src/lib.rs"))]
                .into_iter()
                .collect(),
        };

        let cache = AnalysisCache {
            workspace: old,
            ..Default::default()
        };

        // New state with changes
        let new = WorkspaceState {
            members: vec![
                "crate_a".to_string(),
                "crate_c".to_string(), // Added
            ],
            binaries: [
                ("app".to_string(), PathBuf::from("src/bin/app.rs")), // Changed path
                ("cli".to_string(), PathBuf::from("src/bin/cli.rs")), // Added
            ]
            .into_iter()
            .collect(),
            libraries: HashMap::new(), // Removed
        };

        let changes = cache.detect_workspace_changes(&new);

        assert_eq!(changes.added_members, vec!["crate_c"]);
        assert_eq!(changes.removed_members, vec!["crate_b"]);
        assert_eq!(changes.added_binaries, vec!["cli"]);
        assert_eq!(changes.changed_binaries, vec!["app"]);
        assert_eq!(changes.removed_libraries, vec!["mylib"]);
        assert!(changes.has_changes());
        assert!(changes.needs_full_reanalysis()); // Member change
    }

    #[test]
    fn test_no_changes() {
        let state = WorkspaceState {
            members: vec!["crate_a".to_string()],
            binaries: [("app".to_string(), PathBuf::from("src/main.rs"))]
                .into_iter()
                .collect(),
            libraries: HashMap::new(),
        };

        let cache = AnalysisCache {
            workspace: state.clone(),
            ..Default::default()
        };

        let changes = cache.detect_workspace_changes(&state);
        assert!(!changes.has_changes());
        assert!(!changes.needs_full_reanalysis());
    }

    #[test]
    fn test_hash_stability() {
        // Same content should produce same hash
        let hash1 = {
            let mut hash: u64 = 0xcbf29ce484222325;
            for byte in b"test content" {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
            hash
        };

        let hash2 = {
            let mut hash: u64 = 0xcbf29ce484222325;
            for byte in b"test content" {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
            hash
        };

        assert_eq!(hash1, hash2);

        // Different content should produce different hash
        let hash3 = {
            let mut hash: u64 = 0xcbf29ce484222325;
            for byte in b"different content" {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
            hash
        };

        assert_ne!(hash1, hash3);
    }
}
