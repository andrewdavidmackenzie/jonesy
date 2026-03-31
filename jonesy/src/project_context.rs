//! Project context for source file ownership.
//!
//! Determines whether a DWARF file path belongs to the user's crate or workspace
//! by checking if it falls under one of the project's source directories.
//! All DWARF paths are made absolute (via `comp_dir` prepending) at creation time,
//! so matching is done by absolute path prefix.

use std::fs;
use std::path::Path;

/// Absolute source directory prefixes for a project.
///
/// A file is "ours" if and only if its absolute path starts with a known
/// source directory prefix derived from the project's `Cargo.toml`.
#[derive(Debug, Default)]
pub struct ProjectContext {
    /// Absolute path prefixes for source directories (e.g., "/Users/me/project/src/")
    source_prefixes: Vec<String>,
}

impl ProjectContext {
    /// Build source directory prefixes from the project root.
    ///
    /// Uses `cargo_toml::Manifest::complete_from_path_and_workspace` to discover
    /// target source paths (bin, lib), then extracts their
    /// parent directories as source prefixes. This handles custom layouts like
    /// `[[bin]] path = "crates/core/main.rs"` automatically.
    pub fn from_project_root(project_root: &Path) -> Result<Self, String> {
        let mut source_prefixes = Vec::new();

        // Canonicalize to get an absolute path for reliable matching
        let project_root = fs::canonicalize(project_root).unwrap_or(project_root.to_path_buf());
        let project_root = project_root.as_path();

        let cargo_toml = project_root.join("Cargo.toml");
        let content = std::fs::read_to_string(&cargo_toml)
            .map_err(|e| format!("Cannot read {}: {}", cargo_toml.display(), e))?;
        let manifest = cargo_toml::Manifest::from_slice(content.as_bytes())
            .map_err(|e| format!("Cannot parse {}: {}", cargo_toml.display(), e))?;

        // Process workspace members
        if let Some(workspace) = &manifest.workspace {
            for member_pattern in &workspace.members {
                let member_paths =
                    crate::cargo::expand_workspace_members(project_root, member_pattern);
                for member_path in member_paths {
                    Self::collect_source_dirs_from_manifest(&member_path, &mut source_prefixes);
                }
            }
        }

        // Process the root package (single crate or workspace root with its own package)
        if manifest.package.is_some() {
            Self::collect_source_dirs_from_manifest(project_root, &mut source_prefixes);
        }

        if source_prefixes.is_empty() {
            return Err(format!(
                "No source targets found in {}",
                cargo_toml.display()
            ));
        }

        // Deduplicate prefixes
        source_prefixes.sort();
        source_prefixes.dedup();

        Ok(Self { source_prefixes })
    }

    /// Collect source directories from a crate's Cargo.toml bin and lib targets.
    ///
    /// Uses `complete_from_path_and_workspace` to resolve default target paths
    /// (e.g., `src/main.rs`, `src/lib.rs`), then extracts parent directories.
    /// Silently skips unreadable/unparseable manifests (workspace members may
    /// not all be present).
    fn collect_source_dirs_from_manifest(crate_root: &Path, prefixes: &mut Vec<String>) {
        let cargo_toml = crate_root.join("Cargo.toml");
        let Ok(content) = fs::read_to_string(&cargo_toml) else {
            return;
        };
        let Ok(mut manifest) = cargo_toml::Manifest::from_slice(content.as_bytes()) else {
            return;
        };

        // complete_from_path_and_workspace populates default paths for targets
        // (e.g., bin[].path = "src/main.rs" if src/main.rs exists)
        let _ = manifest.complete_from_path_and_workspace::<toml::Value>(
            &cargo_toml,
            None::<(&cargo_toml::Manifest<toml::Value>, &std::path::Path)>,
        );

        // Collect source file paths from lib and bin targets
        let mut target_paths: Vec<&str> = Vec::new();

        if let Some(lib) = &manifest.lib {
            if let Some(path) = &lib.path {
                target_paths.push(path);
            }
        }

        for bin in &manifest.bin {
            if let Some(path) = &bin.path {
                target_paths.push(path);
            }
        }

        // Convert target source file paths to directory prefixes
        for path in target_paths {
            let source_path = Path::new(path);
            if let Some(parent) = source_path.parent() {
                // Join handles empty parent (e.g., path = "main.rs") correctly —
                // crate_root.join("") returns crate_root
                let abs_dir = crate_root.join(parent);
                if let Some(prefix) = abs_dir.to_str() {
                    let prefix = prefix.trim_end_matches('/');
                    prefixes.push(format!("{}/", prefix));
                }
            }
        }
    }

    /// Check if a DWARF file path belongs to this project's source code.
    ///
    /// All DWARF paths are absolute (comp_dir prepended at creation time),
    /// so this is a simple prefix check.
    pub fn is_crate_source(&self, file_path: &str) -> bool {
        self.source_prefixes
            .iter()
            .any(|prefix| file_path.starts_with(prefix.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_absolute_path_matching() {
        let ctx = ProjectContext {
            source_prefixes: vec!["/Users/me/project/src/".to_string()],
        };

        assert!(ctx.is_crate_source("/Users/me/project/src/main.rs"));
        assert!(ctx.is_crate_source("/Users/me/project/src/module/mod.rs"));
        assert!(!ctx.is_crate_source(
            "/Users/me/.cargo/registry/src/index.crates.io-abc/metal-0.32.0/src/device.rs"
        ));
        assert!(!ctx.is_crate_source(
            "/Users/me/.cargo/registry/src/index.crates.io-abc/wgpu-hal-27.0.4/src/metal/device.rs"
        ));
        assert!(!ctx.is_crate_source("/Users/me/other_project/src/main.rs"));
    }

    #[test]
    fn test_workspace() {
        let ctx = ProjectContext {
            source_prefixes: vec![
                "/Users/me/workspace/crate_a/src/".to_string(),
                "/Users/me/workspace/crate_b/src/".to_string(),
            ],
        };

        assert!(ctx.is_crate_source("/Users/me/workspace/crate_a/src/lib.rs"));
        assert!(ctx.is_crate_source("/Users/me/workspace/crate_b/src/main.rs"));
        assert!(!ctx.is_crate_source("/Users/me/workspace/crate_c/src/lib.rs"));
    }

    #[test]
    fn test_dependency_with_same_relative_path() {
        let ctx = ProjectContext {
            source_prefixes: vec!["/Users/me/meshchat/src/".to_string()],
        };

        assert!(ctx.is_crate_source("/Users/me/meshchat/src/device.rs"));
        assert!(!ctx.is_crate_source(
            "/Users/me/.cargo/registry/src/index.crates.io-abc/metal-0.32.0/src/device.rs"
        ));
    }

    #[test]
    fn test_absolute_path_from_comp_dir() {
        let ctx = ProjectContext {
            source_prefixes: vec!["/Users/me/meshchat/src/".to_string()],
        };

        let metal_device_rs =
            "/Users/me/.cargo/registry/src/index.crates.io-abc/metal-0.32.0/src/device.rs";
        assert!(!ctx.is_crate_source(metal_device_rs));
        assert!(ctx.is_crate_source("/Users/me/meshchat/src/device.rs"));
    }

    // Additional test for NEW logic in simplify_heuristics branch

    #[test]
    fn test_from_project_root_with_jonesy_project() {
        // Test using the actual jonesy project root
        let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let ctx = ProjectContext::from_project_root(project_root)
            .expect("Should create context for jonesy project");

        // Should have at least one source prefix
        assert!(!ctx.source_prefixes.is_empty());

        // Should correctly identify jonesy's own source files
        // Build absolute path to a known file
        let src_lib = project_root.join("src/lib.rs");
        if let Some(src_lib_str) = src_lib.to_str() {
            assert!(
                ctx.is_crate_source(src_lib_str),
                "Expected {} to be recognized as crate source",
                src_lib_str
            );
        }

        // Should NOT recognize random absolute paths as crate source
        assert!(!ctx.is_crate_source("/tmp/random_file.rs"));
        assert!(!ctx.is_crate_source(
            "/Users/someone/.cargo/registry/src/index.crates.io-abc/serde-1.0.0/src/lib.rs"
        ));
    }
}
