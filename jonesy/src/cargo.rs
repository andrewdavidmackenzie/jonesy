//! Cargo project utilities.
//!
//! This module handles finding source files, binaries, and project structure
//! in Rust/Cargo projects.

use cargo_toml::Manifest;
use std::fs;
use std::path::{Path, PathBuf};

/// Get the project name from the Cargo.toml at the given project root.
/// For workspaces, returns the workspace package name if available, otherwise None.
/// For regular crates, returns the package name.
pub fn get_project_name(project_root: &Path) -> Option<String> {
    let cargo_toml = project_root.join("Cargo.toml");
    let content = fs::read_to_string(&cargo_toml).ok()?;
    let manifest = Manifest::from_slice(content.as_bytes()).ok()?;

    // Try package name first (works for both workspace root packages and regular crates)
    if let Some(package) = &manifest.package {
        return Some(package.name.clone());
    }

    // For workspace-only Cargo.toml (no [package]), return None
    // The caller can fall back to binary name
    None
}

/// Find the project root directory for a binary by walking up looking for Cargo.toml.
/// Returns the directory containing Cargo.toml, or None if not found.
pub fn find_project_root(binary_path: &Path) -> Option<PathBuf> {
    let mut current = binary_path.parent();
    while let Some(dir) = current {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// Try to derive the crate source path from the binary path.
/// For a binary at "target/panic/panic", looks for the source in common locations.
/// For libraries like "liblibrary.rlib", strips the "lib" prefix.
pub fn derive_crate_src_path(binary_path: &Path) -> Option<String> {
    // Get the binary name (e.g. "panic" from "target/panic/panic")
    let file_stem = binary_path.file_stem()?.to_str()?;

    // For libraries, strip "lib" prefix (e.g., "liblibrary" -> "library")
    // Only strip for actual library artifacts to avoid renaming binaries like "libtool"
    // Matches all library extensions that find_library can return
    let is_library_artifact = binary_path.extension().is_some_and(|ext| {
        ext == "dylib" || ext == "so" || ext == "dll" || ext == "rlib" || ext == "a" || ext == "lib"
    });
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
            // First, try manifest-driven resolution (more accurate than directory heuristics)
            if is_library_artifact {
                // For libraries, search workspace members by lib name
                if let Some(path) = find_lib_src_path(dir, binary_name) {
                    return Some(path);
                }
            } else {
                // For binaries, search workspace members by bin name
                if let Some(path) = find_bin_src_path(dir, binary_name) {
                    return Some(path);
                }
            }

            // Fallback: directory heuristics when manifest lookup fails
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

/// Expand a workspace member pattern to concrete directory paths.
/// Handles glob patterns like "examples/*" by enumerating directories.
pub fn expand_workspace_members(
    workspace_root: &Path,
    member_pattern: &str,
) -> Vec<std::path::PathBuf> {
    expand_workspace_member(workspace_root, member_pattern)
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

/// Search workspace members to find the source path for a binary by its name.
/// Returns the relative path to the src directory (e.g., "examples/multi_bin/src/").
pub fn find_bin_src_path(workspace_root: &Path, bin_name: &str) -> Option<String> {
    let cargo_toml = workspace_root.join("Cargo.toml");
    let content = fs::read_to_string(&cargo_toml).ok()?;
    let manifest = Manifest::from_slice(content.as_bytes()).ok()?;

    // First check if this manifest itself has the binary
    if let Some(path) = check_manifest_for_binary(&manifest, bin_name) {
        return Some(path);
    }

    // Also handle default single-binary crates (no [[bin]], src/main.rs)
    if manifest.bin.is_empty()
        && let Some(package) = &manifest.package
        && (package.name == bin_name || package.name.replace('-', "_") == bin_name)
        && workspace_root.join("src").join("main.rs").exists()
    {
        return Some("src/".to_string());
    }

    // Then search workspace members
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
            {
                // Check [[bin]] entries in this member
                for bin in &member_manifest.bin {
                    let manifest_bin_name = bin
                        .name
                        .clone()
                        .or_else(|| member_manifest.package.as_ref().map(|p| p.name.clone()))
                        .unwrap_or_default();

                    // Check if this bin matches the target name
                    if manifest_bin_name == bin_name
                        || manifest_bin_name.replace('-', "_") == bin_name
                    {
                        // Return relative path from workspace root, respecting [[bin]].path
                        if let Ok(rel_path) = member_path.strip_prefix(workspace_root) {
                            let src_dir = bin_source_dir_from_path(bin.path.as_ref());
                            return Some(format!("{}/{}/", rel_path.display(), src_dir.display()));
                        }
                    }
                }

                // Also check if it's a single-binary crate (no [[bin]] entries but has a main.rs)
                // In this case, the package name is the binary name
                if member_manifest.bin.is_empty()
                    && let Some(package) = &member_manifest.package
                {
                    let pkg_name = &package.name;
                    if pkg_name == bin_name || pkg_name.replace('-', "_") == bin_name {
                        // Check if there's a src/main.rs
                        if member_path.join("src").join("main.rs").exists() {
                            if let Ok(rel_path) = member_path.strip_prefix(workspace_root) {
                                return Some(format!("{}/src/", rel_path.display()));
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

/// Derive the source directory from a binary's path attribute.
/// Returns the parent directory of the path, or "src" as default.
fn bin_source_dir_from_path(bin_path: Option<&String>) -> PathBuf {
    bin_path
        .and_then(|p| Path::new(p).parent().map(|p| p.to_path_buf()))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("src"))
}

/// Check if a manifest has a binary with the given name and return src path.
fn check_manifest_for_binary(manifest: &Manifest, bin_name: &str) -> Option<String> {
    for bin in &manifest.bin {
        let manifest_bin_name = bin
            .name
            .clone()
            .or_else(|| manifest.package.as_ref().map(|p| p.name.clone()))
            .unwrap_or_default();

        if manifest_bin_name == bin_name || manifest_bin_name.replace('-', "_") == bin_name {
            let src_dir = bin_source_dir_from_path(bin.path.as_ref());
            return Some(format!("{}/", src_dir.display()));
        }
    }
    None
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

/// Find a binary artifact in the target directory.
/// Handles platform-specific extensions (.exe on Windows).
pub fn find_binary(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(name);
    if path.exists() {
        return Some(path);
    }
    #[cfg(windows)]
    {
        let exe_path = path.with_extension("exe");
        if exe_path.exists() {
            return Some(exe_path);
        }
    }
    None
}

/// Find a library artifact in the target directory.
/// Handles platform-specific extensions (.dylib on macOS, .so on Linux, .dll on Windows).
/// Falls back to .rlib if no dynamic library is found.
pub fn find_library(dir: &Path, name: &str) -> Option<PathBuf> {
    // Convert crate name to lib name (replace - with _)
    let lib_name = name.replace('-', "_");

    // Try platform-specific extensions
    #[cfg(target_os = "macos")]
    {
        let dylib = dir.join(format!("lib{}.dylib", lib_name));
        if dylib.exists() {
            return Some(dylib);
        }
    }
    #[cfg(target_os = "linux")]
    {
        let so = dir.join(format!("lib{}.so", lib_name));
        if so.exists() {
            return Some(so);
        }
    }
    #[cfg(windows)]
    {
        let dll = dir.join(format!("{}.dll", lib_name));
        if dll.exists() {
            return Some(dll);
        }
    }
    // Also try .rlib (Rust static library)
    let rlib = dir.join(format!("lib{}.rlib", lib_name));
    if rlib.exists() {
        return Some(rlib);
    }
    // Also try staticlib artifacts (.a on Unix, .lib on Windows)
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let staticlib = dir.join(format!("lib{}.a", lib_name));
        if staticlib.exists() {
            return Some(staticlib);
        }
    }
    #[cfg(windows)]
    {
        let staticlib = dir.join(format!("{}.lib", lib_name));
        if staticlib.exists() {
            return Some(staticlib);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;

    #[test]
    fn test_bin_source_dir_from_path_none() {
        let result = bin_source_dir_from_path(None);
        assert_eq!(result, PathBuf::from("src"));
    }

    #[test]
    fn test_bin_source_dir_from_path_with_path() {
        let result = bin_source_dir_from_path(Some(&"src/bin/app.rs".to_string()));
        assert_eq!(result, PathBuf::from("src/bin"));
    }

    #[test]
    fn test_bin_source_dir_from_path_empty_parent() {
        let result = bin_source_dir_from_path(Some(&"main.rs".to_string()));
        assert_eq!(result, PathBuf::from("src"));
    }

    #[test]
    fn test_find_project_root_not_found() {
        let result = find_project_root(Path::new("/nonexistent/path/to/binary"));
        assert!(result.is_none());
    }

    #[test]
    fn test_find_project_root_found() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        File::create(&cargo_toml).unwrap();

        let binary_path = temp_dir.path().join("target/debug/myapp");
        fs::create_dir_all(binary_path.parent().unwrap()).unwrap();

        let result = find_project_root(&binary_path);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp_dir.path());
    }

    #[test]
    fn test_get_project_name_not_found() {
        let result = get_project_name(Path::new("/nonexistent/path"));
        assert!(result.is_none());
    }

    #[test]
    fn test_get_project_name_workspace_only() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"
[workspace]
members = ["crate_a"]
"#,
        )
        .unwrap();

        let result = get_project_name(temp_dir.path());
        // Workspace-only has no package, returns None
        assert!(result.is_none());
    }

    #[test]
    fn test_get_project_name_with_package() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"
[package]
name = "my-project"
version = "0.1.0"
"#,
        )
        .unwrap();

        let result = get_project_name(temp_dir.path());
        assert_eq!(result, Some("my-project".to_string()));
    }

    #[test]
    fn test_find_binary_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let result = find_binary(temp_dir.path(), "nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_binary_found() {
        let temp_dir = TempDir::new().unwrap();
        let binary_path = temp_dir.path().join("myapp");
        File::create(&binary_path).unwrap();

        let result = find_binary(temp_dir.path(), "myapp");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), binary_path);
    }

    #[test]
    fn test_find_library_rlib() {
        let temp_dir = TempDir::new().unwrap();
        let lib_path = temp_dir.path().join("libmylib.rlib");
        File::create(&lib_path).unwrap();

        let result = find_library(temp_dir.path(), "mylib");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), lib_path);
    }

    #[test]
    fn test_find_library_with_dashes() {
        let temp_dir = TempDir::new().unwrap();
        // Cargo converts dashes to underscores in lib names
        let lib_path = temp_dir.path().join("libmy_lib.rlib");
        File::create(&lib_path).unwrap();

        let result = find_library(temp_dir.path(), "my-lib");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), lib_path);
    }

    #[test]
    fn test_find_library_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let result = find_library(temp_dir.path(), "nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_expand_workspace_member_no_glob() {
        let temp_dir = TempDir::new().unwrap();
        let result = expand_workspace_member(temp_dir.path(), "crates/mylib");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], temp_dir.path().join("crates/mylib"));
    }

    #[test]
    fn test_expand_workspace_member_glob() {
        let temp_dir = TempDir::new().unwrap();
        let crates_dir = temp_dir.path().join("crates");
        fs::create_dir(&crates_dir).unwrap();
        fs::create_dir(crates_dir.join("lib_a")).unwrap();
        fs::create_dir(crates_dir.join("lib_b")).unwrap();

        let result = expand_workspace_member(temp_dir.path(), "crates/*");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_expand_workspace_member_glob_no_matches() {
        let temp_dir = TempDir::new().unwrap();
        let result = expand_workspace_member(temp_dir.path(), "nonexistent/*");
        assert!(result.is_empty());
    }

    #[test]
    fn test_derive_crate_src_path_no_cargo_toml() {
        let temp_dir = TempDir::new().unwrap();
        let binary_path = temp_dir.path().join("target/debug/myapp");
        fs::create_dir_all(binary_path.parent().unwrap()).unwrap();

        let result = derive_crate_src_path(&binary_path);
        assert!(result.is_none());
    }

    #[test]
    fn test_derive_crate_src_path_simple_crate() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"
[package]
name = "myapp"
version = "0.1.0"
"#,
        )
        .unwrap();
        fs::create_dir(temp_dir.path().join("src")).unwrap();
        File::create(temp_dir.path().join("src/main.rs")).unwrap();

        let binary_path = temp_dir.path().join("target/debug/myapp");
        fs::create_dir_all(binary_path.parent().unwrap()).unwrap();

        let result = derive_crate_src_path(&binary_path);
        assert_eq!(result, Some("src/".to_string()));
    }

    #[test]
    fn test_derive_crate_src_path_library() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"
[package]
name = "mylib"
version = "0.1.0"

[lib]
name = "mylib"
"#,
        )
        .unwrap();
        fs::create_dir(temp_dir.path().join("src")).unwrap();
        File::create(temp_dir.path().join("src/lib.rs")).unwrap();

        // Library artifacts have lib prefix
        let lib_path = temp_dir.path().join("target/debug/libmylib.rlib");
        fs::create_dir_all(lib_path.parent().unwrap()).unwrap();

        let result = derive_crate_src_path(&lib_path);
        // Should find src/
        assert!(result.is_some());
    }

    #[test]
    fn test_check_member_lib_type_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let result = check_member_lib_type(temp_dir.path(), "mylib");
        assert!(result.is_none());
    }

    #[test]
    fn test_check_member_lib_type_cdylib() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"
[package]
name = "mylib"
version = "0.1.0"

[lib]
name = "mylib"
crate-type = ["cdylib"]
"#,
        )
        .unwrap();

        let result = check_member_lib_type(temp_dir.path(), "mylib");
        assert_eq!(result, Some("cdylib".to_string()));
    }

    #[test]
    fn test_check_member_lib_type_dylib() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"
[package]
name = "mylib"
version = "0.1.0"

[lib]
name = "mylib"
crate-type = ["dylib"]
"#,
        )
        .unwrap();

        let result = check_member_lib_type(temp_dir.path(), "mylib");
        assert_eq!(result, Some("dylib".to_string()));
    }

    #[test]
    fn test_detect_library_type_not_library() {
        // Binary without lib prefix
        let result = detect_library_type(Path::new("/target/debug/myapp"));
        assert!(result.is_none());
    }

    #[test]
    fn test_find_bin_src_path_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"
[package]
name = "other"
version = "0.1.0"
"#,
        )
        .unwrap();

        let result = find_bin_src_path(temp_dir.path(), "myapp");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_bin_src_path_simple_crate() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"
[package]
name = "myapp"
version = "0.1.0"
"#,
        )
        .unwrap();
        fs::create_dir(temp_dir.path().join("src")).unwrap();
        File::create(temp_dir.path().join("src/main.rs")).unwrap();

        let result = find_bin_src_path(temp_dir.path(), "myapp");
        assert_eq!(result, Some("src/".to_string()));
    }

    #[test]
    fn test_find_lib_src_path_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"
[package]
name = "other"
version = "0.1.0"
"#,
        )
        .unwrap();

        let result = find_lib_src_path(temp_dir.path(), "mylib");
        assert!(result.is_none());
    }
}
