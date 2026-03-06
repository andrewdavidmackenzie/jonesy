//! Cargo project utilities.
//!
//! This module handles finding source files, binaries, and project structure
//! in Rust/Cargo projects.

use cargo_toml::Manifest;
use std::fs;
use std::path::Path;

/// Try to derive the crate source path from the binary path.
/// For a binary at "target/panic/panic", looks for the source in common locations.
/// For libraries like "liblibrary.rlib", strips the "lib" prefix.
pub fn derive_crate_src_path(binary_path: &Path) -> Option<String> {
    // Get the binary name (e.g. "panic" from "target/panic/panic")
    let file_stem = binary_path.file_stem()?.to_str()?;

    // For libraries, strip "lib" prefix (e.g., "liblibrary" -> "library")
    // Only strip for actual library artifacts to avoid renaming binaries like "libtool"
    let is_library_artifact = binary_path
        .extension()
        .is_some_and(|ext| ext == "dylib" || ext == "rlib" || ext == "a");
    let binary_name = if is_library_artifact {
        file_stem.strip_prefix("lib").unwrap_or(file_stem)
    } else {
        file_stem
    };

    // Common patterns:
    // 1. examples/<name>/src/ for crates in examples
    // 2. <name>/src/ for workspace members
    // 3. src/ for the main crate

    // Try to find the workspace root by looking for Cargo.toml
    let mut current = binary_path.parent();
    while let Some(dir) = current {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            // Check for examples/<binary_name>/src/
            let example_src = dir.join("examples").join(binary_name).join("src");
            if example_src.exists() {
                return Some(format!("examples/{}/src/", binary_name));
            }

            // Check for <binary_name>/src/
            let member_src = dir.join(binary_name).join("src");
            if member_src.exists() {
                return Some(format!("{}/src/", binary_name));
            }

            // For libraries, the directory name may not match the lib name.
            // Search workspace members to find the matching lib.
            if let Some(path) = find_lib_src_path(dir, binary_name) {
                return Some(path);
            }

            // Check for src/ in the workspace root
            let root_src = dir.join("src");
            if root_src.exists() {
                return Some("src/".to_string());
            }
        }
        current = dir.parent();
    }

    None
}

/// Expand a workspace member pattern to concrete paths.
/// Handles glob patterns like "examples/*" by enumerating directories.
fn expand_workspace_member(workspace_root: &Path, member_pattern: &str) -> Vec<std::path::PathBuf> {
    if member_pattern.contains('*') {
        let base = member_pattern.trim_end_matches("/*");
        let base_path = workspace_root.join(base);
        if base_path.is_dir() {
            fs::read_dir(&base_path)
                .ok()
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().is_dir())
                        .map(|e| e.path())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            vec![]
        }
    } else {
        vec![workspace_root.join(member_pattern)]
    }
}

/// Search workspace members to find the source path for a library by its name.
/// Returns the relative path to the src directory (e.g., "examples/cdylib/src/").
pub fn find_lib_src_path(workspace_root: &Path, lib_name: &str) -> Option<String> {
    let cargo_toml = workspace_root.join("Cargo.toml");
    let content = fs::read_to_string(&cargo_toml).ok()?;
    let manifest = Manifest::from_slice(content.as_bytes()).ok()?;

    let workspace = manifest.workspace.as_ref()?;

    for member_pattern in &workspace.members {
        let member_paths = expand_workspace_member(workspace_root, member_pattern);

        for member_path in member_paths {
            let member_cargo_toml = member_path.join("Cargo.toml");
            if !member_cargo_toml.exists() {
                continue;
            }

            if let Ok(member_content) = fs::read_to_string(&member_cargo_toml)
                && let Ok(member_manifest) = Manifest::from_slice(member_content.as_bytes())
                && let Some(lib) = &member_manifest.lib
            {
                let manifest_lib_name = lib
                    .name
                    .clone()
                    .or_else(|| member_manifest.package.as_ref().map(|p| p.name.clone()))
                    .unwrap_or_default();

                // Check if this lib matches the target name
                if manifest_lib_name == lib_name || manifest_lib_name.replace('-', "_") == lib_name
                {
                    // Return relative path from workspace root
                    if let Ok(rel_path) = member_path.strip_prefix(workspace_root) {
                        return Some(format!("{}/src/", rel_path.display()));
                    }
                }
            }
        }
    }

    None
}

/// Detect if a library is a cdylib or dylib by checking Cargo.toml
/// Returns Some("cdylib"), Some("dylib"), or None if not determinable
pub fn detect_library_type(binary_path: &Path) -> Option<String> {
    // Extract library name from path (e.g., "liblibrary.dylib" -> "library")
    let file_stem = binary_path.file_stem()?.to_str()?;
    let lib_name = file_stem.strip_prefix("lib")?;

    // Walk up to find Cargo.toml
    let mut current = binary_path.parent();
    while let Some(dir) = current {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists()
            && let Ok(content) = fs::read_to_string(&cargo_toml)
            && let Ok(manifest) = Manifest::from_slice(content.as_bytes())
        {
            // Check if this is a workspace, look for member with matching lib name
            if let Some(workspace) = &manifest.workspace {
                for member_pattern in &workspace.members {
                    // Handle glob patterns (e.g., "examples/*")
                    let member_paths = expand_workspace_member(dir, member_pattern);
                    for member_path in member_paths {
                        if let Some(lib_type) = check_member_lib_type(&member_path, lib_name) {
                            return Some(lib_type);
                        }
                    }
                }
            }

            // Check if this manifest has a matching lib
            if let Some(lib) = &manifest.lib {
                let manifest_lib_name = lib
                    .name
                    .clone()
                    .or_else(|| manifest.package.as_ref().map(|p| p.name.clone()))
                    .unwrap_or_default();

                if manifest_lib_name == lib_name || manifest_lib_name.replace('-', "_") == lib_name
                {
                    // Check crate types
                    for crate_type in &lib.crate_type {
                        if crate_type == "cdylib" {
                            return Some("cdylib".to_string());
                        }
                        if crate_type == "dylib" {
                            return Some("dylib".to_string());
                        }
                    }
                }
            }
        }
        current = dir.parent();
    }
    None
}

/// Check a workspace member for matching library type
fn check_member_lib_type(member_path: &Path, lib_name: &str) -> Option<String> {
    let cargo_toml = member_path.join("Cargo.toml");
    if !cargo_toml.exists() {
        return None;
    }

    let content = fs::read_to_string(&cargo_toml).ok()?;
    let manifest = Manifest::from_slice(content.as_bytes()).ok()?;

    if let Some(lib) = &manifest.lib {
        let manifest_lib_name = lib
            .name
            .clone()
            .or_else(|| manifest.package.as_ref().map(|p| p.name.clone()))
            .unwrap_or_default();

        if manifest_lib_name == lib_name || manifest_lib_name.replace('-', "_") == lib_name {
            for crate_type in &lib.crate_type {
                if crate_type == "cdylib" {
                    return Some("cdylib".to_string());
                }
                if crate_type == "dylib" {
                    return Some("dylib".to_string());
                }
            }
        }
    }
    None
}
