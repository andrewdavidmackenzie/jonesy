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
    /// Project root path, used to resolve relative DWARF paths to absolute
    project_root: Option<String>,
}

impl ProjectContext {
    /// Build source directory prefixes from the project root.
    ///
    /// Reads Cargo.toml to find workspace members and their source directories.
    /// For single crates, uses `{project_root}/src/`.
    /// For workspaces, uses `{project_root}/{member}/src/` for each member.
    pub fn from_project_root(project_root: &Path) -> Self {
        let mut source_prefixes = Vec::new();

        // Canonicalize to get an absolute path for reliable matching
        let project_root = fs::canonicalize(project_root).unwrap_or(project_root.to_path_buf());
        let project_root = project_root.as_path();

        // TODO I think here we should use the Cargo crate to read and understand the
        // project (workspace or crate)

        let cargo_toml = project_root.join("Cargo.toml");
        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
            if let Ok(manifest) = cargo_toml::Manifest::from_slice(content.as_bytes()) {
                if let Some(workspace) = &manifest.workspace {
                    // Workspace: add each member's src directory
                    for member_pattern in &workspace.members {
                        let member_paths =
                            crate::cargo::expand_workspace_members(project_root, member_pattern);
                        for member_path in member_paths {
                            let src_dir = member_path.join("src");
                            if let Some(prefix) = src_dir.to_str() {
                                source_prefixes.push(format!("{}/", prefix));
                            }
                        }
                    }
                }

                if manifest.package.is_some() {
                    // Single crate (or workspace root with its own package)
                    let src_dir = project_root.join("src");
                    if let Some(prefix) = src_dir.to_str() {
                        source_prefixes.push(format!("{}/", prefix));
                    }
                }
            }
        }

        // Fallback: if no prefixes found, use project_root/src/
        if source_prefixes.is_empty() {
            let src_dir = project_root.join("src");
            if let Some(prefix) = src_dir.to_str() {
                source_prefixes.push(format!("{}/", prefix));
            }
        }

        // For resolving relative DWARF paths, we need the workspace root
        // (where cargo build actually runs), not just the member's project root.
        // Walk up from project_root to find the top-level workspace Cargo.toml.
        let workspace_root = Self::find_workspace_root(project_root);
        let root_str = workspace_root.to_str().map(|s| format!("{}/", s));

        Self {
            source_prefixes,
            project_root: root_str,
        }
    }

    /// Find the workspace root for a project.
    fn find_workspace_root(project_root: &Path) -> std::path::PathBuf {
        if Self::has_workspace_section(project_root) {
            return project_root.to_path_buf();
        }

        let mut current = project_root.parent();
        while let Some(dir) = current {
            if Self::has_workspace_section(dir) {
                return dir.to_path_buf();
            }
            current = dir.parent();
        }
        project_root.to_path_buf()
    }

    /// Check if a directory's Cargo.toml has a [workspace] section.
    fn has_workspace_section(dir: &Path) -> bool {
        let cargo_toml = dir.join("Cargo.toml");
        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
            if let Ok(manifest) = cargo_toml::Manifest::from_slice(content.as_bytes()) {
                return manifest.workspace.is_some();
            }
        }
        false
    }

    /// Check if a DWARF file path belongs to this project's source code.
    ///
    /// For absolute paths: checks if the path starts with any source directory prefix.
    /// For relative paths: resolves against workspace root and checks if the file exists.
    pub fn is_crate_source(&self, file_path: &str) -> bool {
        if file_path.starts_with('/') {
            return self
                .source_prefixes
                .iter()
                .any(|prefix| file_path.starts_with(prefix.as_str()));
        }

        if let Some(root) = &self.project_root {
            let absolute = format!("{}{}", root, file_path);
            if self
                .source_prefixes
                .iter()
                .any(|prefix| absolute.starts_with(prefix.as_str()))
            {
                let path = std::path::Path::new(&absolute);
                return path.exists();
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_absolute_path_matching() {
        let ctx = ProjectContext {
            source_prefixes: vec!["/Users/me/project/src/".to_string()],
            project_root: Some("/Users/me/project/".to_string()),
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
            project_root: Some("/Users/me/workspace/".to_string()),
        };

        assert!(ctx.is_crate_source("/Users/me/workspace/crate_a/src/lib.rs"));
        assert!(ctx.is_crate_source("/Users/me/workspace/crate_b/src/main.rs"));
        assert!(!ctx.is_crate_source("/Users/me/workspace/crate_c/src/lib.rs"));
    }

    #[test]
    fn test_relative_paths() {
        let project_root = env!("CARGO_MANIFEST_DIR");
        let ctx = ProjectContext {
            source_prefixes: vec![format!("{}/src/", project_root)],
            project_root: Some(format!("{}/", project_root)),
        };

        assert!(ctx.is_crate_source("src/lib.rs"));
        assert!(!ctx.is_crate_source("src/nonexistent_file.rs"));
        assert!(!ctx.is_crate_source("tests/test.rs"));
    }

    #[test]
    fn test_dependency_with_same_relative_path() {
        let ctx = ProjectContext {
            source_prefixes: vec!["/Users/me/meshchat/src/".to_string()],
            project_root: Some("/Users/me/meshchat/".to_string()),
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
            project_root: Some("/Users/me/meshchat/".to_string()),
        };

        let metal_device_rs =
            "/Users/me/.cargo/registry/src/index.crates.io-abc/metal-0.32.0/src/device.rs";
        assert!(!ctx.is_crate_source(metal_device_rs));
        assert!(ctx.is_crate_source("/Users/me/meshchat/src/device.rs"));
    }
}
