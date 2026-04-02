//! Analysis cache for incremental re-analysis.
//!
//! Stores analysis state in `target/jonesy/` to avoid unnecessary re-analysis.
//! Tracks:
//! - Binary/library modification times
//! - Cargo.toml content hashes (for target/member changes)
//! - jonesy.toml content hash (for rule changes)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Cache file location within the target directory.
const CACHE_FILE: &str = "jonesy/cache.json";

/// Cached state for a single target (binary or library).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TargetState {
    /// Path to the target file.
    path: PathBuf,
    /// Last modification time (as milliseconds since epoch for subsecond precision).
    mtime: u128,
    /// Number of panic points found in the last analysis.
    pub(crate) panic_count: usize,
}

/// Cached state for a config file (Cargo.toml or jonesy.toml).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigState {
    /// Path to the config file.
    path: PathBuf,
    /// Hash of relevant content (for change detection).
    content_hash: u64,
}

/// Workspace structure snapshot for detecting membership changes.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct WorkspaceState {
    /// Workspace members (crate names).
    members: Vec<String>,
    /// Binary targets (name -> path).
    binaries: HashMap<String, PathBuf>,
    /// Library targets (name -> path).
    libraries: HashMap<String, PathBuf>,
}

impl WorkspaceState {
    /// Whether this is a single-package crate (no workspace members).
    pub fn is_single_package(&self) -> bool {
        self.members.is_empty()
    }
}

/// The full analysis cache.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AnalysisCache {
    /// Cache format version (for migration).
    version: u32,
    /// Cached target states.
    pub(crate) targets: HashMap<PathBuf, TargetState>,
    /// Cached config file states.
    configs: HashMap<PathBuf, ConfigState>,
    /// Last known workspace structure.
    workspace: WorkspaceState,
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

        // Return with the current version
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

        // Check if the file has been modified
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
            return true; // Not in the cache
        };

        let current_hash = hash_file_content(config_path).unwrap_or(0);
        current_hash != cached.content_hash
    }

    /// Update cache for a config file using a pre-computed hash.
    pub fn update_config_with_hash(&mut self, config_path: &Path, content_hash: u64) {
        self.configs.insert(
            config_path.to_path_buf(),
            ConfigState {
                path: config_path.to_path_buf(),
                content_hash,
            },
        );
    }

    /// Check if a config file is tracked in the cache.
    pub fn has_config(&self, config_path: &Path) -> bool {
        self.configs.contains_key(config_path)
    }

    /// Remove a config file from the cache (e.g., when deleted).
    pub fn remove_config(&mut self, config_path: &Path) {
        self.configs.remove(config_path);
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
pub(crate) struct WorkspaceChanges {
    added_members: Vec<String>,
    removed_members: Vec<String>,
    added_binaries: Vec<String>,
    removed_binaries: Vec<String>,
    changed_binaries: Vec<String>,
    added_libraries: Vec<String>,
    removed_libraries: Vec<String>,
    changed_libraries: Vec<String>,
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

    /// Returns (members_affected, binaries_affected, libraries_affected) counts.
    pub fn change_counts(&self) -> (usize, usize, usize) {
        (
            self.added_members.len() + self.removed_members.len(),
            self.added_binaries.len() + self.removed_binaries.len() + self.changed_binaries.len(),
            self.added_libraries.len()
                + self.removed_libraries.len()
                + self.changed_libraries.len(),
        )
    }

    /// Check if a specific target path is affected by workspace changes.
    ///
    /// This checks if the target's name (derived from its file path) matches
    /// any added or changed binaries/libraries. Names are normalized to handle
    /// Cargo's dash-to-underscore conversion in artifact filenames.
    pub fn affects_target(&self, target_path: &Path) -> bool {
        let stem = target_path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        let normalize = |s: &str| s.replace('-', "_");

        // For binaries: use the stem as-is (a binary named "libtool" stays "libtool")
        let binary_name = normalize(stem);

        // For libraries: strip "lib" prefix (libfoo.rlib -> foo)
        let library_name = normalize(stem.strip_prefix("lib").unwrap_or(stem));

        self.added_binaries
            .iter()
            .any(|n| normalize(n) == binary_name)
            || self
                .changed_binaries
                .iter()
                .any(|n| normalize(n) == binary_name)
            || self
                .added_libraries
                .iter()
                .any(|n| normalize(n) == library_name)
            || self
                .changed_libraries
                .iter()
                .any(|n| normalize(n) == library_name)
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
pub(crate) fn hash_file_content(path: &Path) -> Option<u64> {
    let content = fs::read(path).ok()?;
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    Some(hasher.finish())
}

/// Build current workspace state from Cargo.toml files.
pub(crate) fn build_workspace_state(workspace_root: &Path) -> WorkspaceState {
    let mut state = WorkspaceState::default();

    let cargo_toml = workspace_root.join("Cargo.toml");
    let Ok(content) = fs::read_to_string(&cargo_toml) else {
        return state;
    };
    let Ok(manifest) = cargo_toml::Manifest::from_slice(content.as_bytes()) else {
        return state;
    };

    // Collect workspace members (use relative paths for unique IDs)
    if let Some(workspace) = &manifest.workspace {
        for member in &workspace.members {
            if member.contains('*') {
                // Expand glob - use the path relative to the workspace root for uniqueness
                if let Ok(paths) = glob::glob(&workspace_root.join(member).to_string_lossy()) {
                    for path in paths.flatten() {
                        if path.is_dir() && path.join("Cargo.toml").exists() {
                            // Use relative path to avoid collisions (crates/foo vs. examples/foo)
                            if let Ok(rel_path) = path.strip_prefix(workspace_root) {
                                state.members.push(rel_path.to_string_lossy().to_string());
                            }
                        }
                    }
                }
            } else {
                state.members.push(member.clone());
            }
        }
    }

    // Collect targets from the root manifest
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

    // Collect explicit binaries first (they take precedence)
    for bin in &manifest.bin {
        let name = bin.name.as_deref().unwrap_or(&pkg.name);
        let path = bin
            .path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| crate_root.join("src/main.rs"));
        state.binaries.insert(name.to_string(), path);
    }

    // Always check for implicit/auto-discovered binaries (Rust 2024 merges them with explicit)
    // Use entry().or_insert() to avoid overwriting explicit [[bin]] entries

    // src/main.rs -> binary named after package
    if crate_root.join("src/main.rs").exists() {
        state
            .binaries
            .entry(pkg.name.clone())
            .or_insert_with(|| crate_root.join("src/main.rs"));
    }

    // src/bin/*.rs -> binary named after file stem
    // src/bin/*/main.rs -> binary named after directory
    let bin_dir = crate_root.join("src/bin");
    if let Ok(entries) = fs::read_dir(&bin_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "rs") {
                if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
                    state.binaries.entry(name.to_string()).or_insert(path);
                }
            } else if path.is_dir() {
                let main_rs = path.join("main.rs");
                if main_rs.exists() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        state.binaries.entry(name.to_string()).or_insert(main_rs);
                    }
                }
            }
        }
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
                ("app".to_string(), PathBuf::from("src/bin/app.rs")), // Changed the path
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
    fn test_is_single_package() {
        // No members = single package
        let state = WorkspaceState::default();
        assert!(state.is_single_package());

        // With members = workspace
        let state = WorkspaceState {
            members: vec!["crate_a".to_string()],
            ..Default::default()
        };
        assert!(!state.is_single_package());
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

    #[test]
    fn test_target_needs_analysis_not_in_cache() {
        let cache = AnalysisCache::default();
        assert!(cache.target_needs_analysis(Path::new("/nonexistent/path")));
    }

    #[test]
    fn test_update_target() {
        let mut cache = AnalysisCache::default();
        let path = PathBuf::from("/test/target");
        cache.update_target(&path, 5);

        assert!(cache.targets.contains_key(&path));
        let state = cache.targets.get(&path).unwrap();
        assert_eq!(state.panic_count, 5);
    }

    #[test]
    fn test_config_changed_not_in_cache() {
        let cache = AnalysisCache::default();
        assert!(cache.config_changed(Path::new("/nonexistent/config")));
    }

    #[test]
    fn test_update_config_with_hash() {
        let mut cache = AnalysisCache::default();
        let path = PathBuf::from("/test/config.toml");
        cache.update_config_with_hash(&path, 12345);

        assert!(cache.configs.contains_key(&path));
    }

    #[test]
    fn test_update_workspace() {
        let mut cache = AnalysisCache::default();
        let state = WorkspaceState {
            members: vec!["member1".to_string()],
            binaries: HashMap::new(),
            libraries: HashMap::new(),
        };
        cache.update_workspace(state.clone());

        assert_eq!(cache.workspace.members, vec!["member1"]);
    }

    #[test]
    fn test_prune_stale_targets() {
        let mut cache = AnalysisCache::default();
        // Add a non-existent target
        cache.targets.insert(
            PathBuf::from("/nonexistent/target"),
            TargetState {
                path: PathBuf::from("/nonexistent/target"),
                mtime: 0,
                panic_count: 0,
            },
        );
        assert_eq!(cache.targets.len(), 1);

        cache.prune_stale_targets();
        assert!(cache.targets.is_empty());
    }

    #[test]
    fn test_workspace_changes_affects_target_binary() {
        let changes = WorkspaceChanges {
            added_binaries: vec!["my-app".to_string()],
            ..Default::default()
        };

        // Binary with dashes -> underscore normalization
        assert!(changes.affects_target(Path::new("/target/debug/my_app")));
        assert!(changes.affects_target(Path::new("/target/debug/my-app")));
        assert!(!changes.affects_target(Path::new("/target/debug/other")));
    }

    #[test]
    fn test_workspace_changes_affects_target_library() {
        let changes = WorkspaceChanges {
            added_libraries: vec!["mylib".to_string()],
            ..Default::default()
        };

        // Library with lib prefix
        assert!(changes.affects_target(Path::new("/target/debug/libmylib.rlib")));
        assert!(changes.affects_target(Path::new("/target/debug/libmylib.dylib")));
        assert!(!changes.affects_target(Path::new("/target/debug/libother.rlib")));
    }

    #[test]
    fn test_workspace_changes_affects_target_changed() {
        let changes = WorkspaceChanges {
            changed_binaries: vec!["cli".to_string()],
            changed_libraries: vec!["core".to_string()],
            ..Default::default()
        };

        assert!(changes.affects_target(Path::new("/target/debug/cli")));
        assert!(changes.affects_target(Path::new("/target/debug/libcore.rlib")));
    }

    #[test]
    fn test_cache_version() {
        let cache = AnalysisCache::default();
        // Default should have version 0
        assert_eq!(cache.version, 0);

        // After loading, the version should be set to current
        // (we can't easily test this without filesystem)
    }

    #[test]
    fn test_config_changed_detects_content_change() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("jonesy.toml");

        // Write initial config
        fs::write(&config_path, "deny = [\"unwrap\"]").unwrap();

        let mut cache = AnalysisCache::default();

        // First check: not in cache, so reports changed
        assert!(cache.config_changed(&config_path));

        // Cache the config
        cache.update_config_with_hash(&config_path, hash_file_content(&config_path).unwrap_or(0));

        // Same content: no change detected
        assert!(!cache.config_changed(&config_path));

        // Modify the config (e.g., the user adds an "allow" rule via quick fix)
        fs::write(&config_path, "allow = [\"capacity\"]\ndeny = [\"unwrap\"]").unwrap();

        // Should detect the content change
        assert!(
            cache.config_changed(&config_path),
            "config_changed should detect modified jonesy.toml content"
        );

        // Update cache, then verify no further change
        cache.update_config_with_hash(&config_path, hash_file_content(&config_path).unwrap_or(0));
        assert!(!cache.config_changed(&config_path));
    }

    #[test]
    fn test_workspace_changes_no_full_reanalysis_without_member_changes() {
        let changes = WorkspaceChanges {
            added_binaries: vec!["bin1".to_string()],
            changed_libraries: vec!["lib1".to_string()],
            ..Default::default()
        };

        assert!(changes.has_changes());
        assert!(!changes.needs_full_reanalysis());
    }

    // ========================================================================
    // Cache load/save tests with temp directories
    // ========================================================================

    #[test]
    fn test_cache_load_nonexistent() {
        let temp_dir = std::env::temp_dir().join(format!("jonesy_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous run

        let cache = AnalysisCache::load(&temp_dir);

        // Should return default with the current version
        assert_eq!(cache.version, AnalysisCache::VERSION);
        assert!(cache.targets.is_empty());
        assert!(cache.configs.is_empty());
    }

    #[test]
    fn test_cache_save_and_load() {
        let temp_dir =
            std::env::temp_dir().join(format!("jonesy_test_save_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Create a cache with some data
        let mut cache = AnalysisCache {
            version: AnalysisCache::VERSION,
            ..Default::default()
        };
        cache.update_workspace(WorkspaceState {
            members: vec!["test_member".to_string()],
            binaries: HashMap::new(),
            libraries: HashMap::new(),
        });

        // Save it
        cache.save(&temp_dir).unwrap();

        // Verify file exists
        let cache_file = temp_dir.join("target/jonesy/cache.json");
        assert!(cache_file.exists());

        // Load it back
        let loaded = AnalysisCache::load(&temp_dir);
        assert_eq!(loaded.version, AnalysisCache::VERSION);
        assert_eq!(loaded.workspace.members, vec!["test_member"]);

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cache_load_invalid_json() {
        let temp_dir =
            std::env::temp_dir().join(format!("jonesy_test_invalid_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        let cache_dir = temp_dir.join("target/jonesy");
        fs::create_dir_all(&cache_dir).unwrap();

        // Write invalid JSON
        fs::write(cache_dir.join("cache.json"), "not valid json").unwrap();

        // Should return default cache
        let cache = AnalysisCache::load(&temp_dir);
        assert_eq!(cache.version, AnalysisCache::VERSION);
        assert!(cache.targets.is_empty());

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cache_load_wrong_version() {
        let temp_dir =
            std::env::temp_dir().join(format!("jonesy_test_version_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        let cache_dir = temp_dir.join("target/jonesy");
        fs::create_dir_all(&cache_dir).unwrap();

        // Write cache with the old version
        let old_cache = r#"{"version": 0, "targets": {}, "configs": {}, "workspace": {"members": [], "binaries": {}, "libraries": {}}}"#;
        fs::write(cache_dir.join("cache.json"), old_cache).unwrap();

        // Should return the fresh cache since the version doesn't match
        let cache = AnalysisCache::load(&temp_dir);
        assert_eq!(cache.version, AnalysisCache::VERSION);

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    // ========================================================================
    // Target analysis with real files
    // ========================================================================

    #[test]
    fn test_target_needs_analysis_with_real_file() {
        let temp_dir =
            std::env::temp_dir().join(format!("jonesy_test_target_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let target_file = temp_dir.join("test_binary");
        fs::write(&target_file, "test content").unwrap();

        let mut cache = AnalysisCache::default();

        // First time - needs analysis
        assert!(cache.target_needs_analysis(&target_file));

        // Update cache
        cache.update_target(&target_file, 5);

        // Should not need analysis now (mtime matches)
        assert!(!cache.target_needs_analysis(&target_file));

        // Modify the file
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(&target_file, "modified content").unwrap();

        // Now needs analysis again
        assert!(cache.target_needs_analysis(&target_file));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_config_changed_with_real_file() {
        let temp_dir =
            std::env::temp_dir().join(format!("jonesy_test_config_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let config_file = temp_dir.join("jonesy.toml");
        fs::write(&config_file, "allow = [\"unwrap\"]").unwrap();

        let mut cache = AnalysisCache::default();

        // First time - changed (not in cache)
        assert!(cache.config_changed(&config_file));

        // Update cache
        cache.update_config_with_hash(&config_file, hash_file_content(&config_file).unwrap_or(0));

        // Should not be changed now
        assert!(!cache.config_changed(&config_file));

        // Modify content
        fs::write(&config_file, "allow = [\"panic\"]").unwrap();

        // Now changed
        assert!(cache.config_changed(&config_file));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    // ========================================================================
    // Build workspace state tests
    // ========================================================================

    #[test]
    fn test_build_workspace_state_no_cargo_toml() {
        let temp_dir =
            std::env::temp_dir().join(format!("jonesy_test_no_cargo_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let state = build_workspace_state(&temp_dir);

        assert!(state.members.is_empty());
        assert!(state.binaries.is_empty());
        assert!(state.libraries.is_empty());

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_build_workspace_state_simple_crate() {
        let temp_dir =
            std::env::temp_dir().join(format!("jonesy_test_simple_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Create a simple Cargo.toml
        let cargo_toml = r#"
[package]
name = "my-app"
version = "0.1.0"
"#;
        fs::write(temp_dir.join("Cargo.toml"), cargo_toml).unwrap();

        // Create src/main.rs
        fs::create_dir_all(temp_dir.join("src")).unwrap();
        fs::write(temp_dir.join("src/main.rs"), "fn main() {}").unwrap();

        let state = build_workspace_state(&temp_dir);

        // Should find the binary
        assert!(state.binaries.contains_key("my-app"));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_build_workspace_state_with_lib() {
        let temp_dir = std::env::temp_dir().join(format!("jonesy_test_lib_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Create a library Cargo.toml
        let cargo_toml = r#"
[package]
name = "my-lib"
version = "0.1.0"
"#;
        fs::write(temp_dir.join("Cargo.toml"), cargo_toml).unwrap();

        // Create src/lib.rs
        fs::create_dir_all(temp_dir.join("src")).unwrap();
        fs::write(temp_dir.join("src/lib.rs"), "pub fn hello() {}").unwrap();

        let state = build_workspace_state(&temp_dir);

        // Should find the library
        assert!(state.libraries.contains_key("my-lib"));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_build_workspace_state_with_bin_dir() {
        let temp_dir =
            std::env::temp_dir().join(format!("jonesy_test_bin_dir_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Create Cargo.toml
        let cargo_toml = r#"
[package]
name = "multi-bin"
version = "0.1.0"
"#;
        fs::write(temp_dir.join("Cargo.toml"), cargo_toml).unwrap();

        // Create src/bin/foo.rs and src/bin/bar/main.rs
        fs::create_dir_all(temp_dir.join("src/bin/bar")).unwrap();
        fs::write(temp_dir.join("src/bin/foo.rs"), "fn main() {}").unwrap();
        fs::write(temp_dir.join("src/bin/bar/main.rs"), "fn main() {}").unwrap();

        let state = build_workspace_state(&temp_dir);

        // Should find both binaries
        assert!(state.binaries.contains_key("foo"));
        assert!(state.binaries.contains_key("bar"));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    // ========================================================================
    // Helper function tests
    // ========================================================================

    #[test]
    fn test_get_mtime_nonexistent() {
        let result = get_mtime(Path::new("/nonexistent/path/to/file"));
        assert!(result.is_none());
    }

    #[test]
    fn test_hash_file_content_nonexistent() {
        let result = hash_file_content(Path::new("/nonexistent/path/to/file"));
        assert!(result.is_none());
    }

    #[test]
    fn test_hash_file_content_consistent() {
        let temp_dir =
            std::env::temp_dir().join(format!("jonesy_test_hash_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let test_file = temp_dir.join("test.txt");
        fs::write(&test_file, "hello world").unwrap();

        let hash1 = hash_file_content(&test_file);
        let hash2 = hash_file_content(&test_file);

        assert!(hash1.is_some());
        assert_eq!(hash1, hash2);

        // Different content should have different hash
        fs::write(&test_file, "goodbye world").unwrap();
        let hash3 = hash_file_content(&test_file);

        assert_ne!(hash1, hash3);

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
