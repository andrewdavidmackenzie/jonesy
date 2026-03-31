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
pub fn find_project_root(binary_path: &Path) -> Result<PathBuf, String> {
    let mut current = binary_path.parent();
    while let Some(dir) = current {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            return Ok(dir.to_path_buf());
        }
        current = dir.parent();
    }
    Err(format!(
        "Cannot find project root for {}",
        binary_path.display()
    ))
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
pub fn expand_workspace_members(workspace_root: &Path, member_pattern: &str) -> Vec<PathBuf> {
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
        let member_paths = expand_workspace_members(workspace_root, member_pattern);

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
        let member_paths = expand_workspace_members(workspace_root, member_pattern);

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
                    let member_paths = expand_workspace_members(dir, member_pattern);
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

/// Represents a workspace member crate with its binaries
#[derive(Debug)]
pub struct WorkspaceMember {
    /// Name of the member crate
    pub name: String,
    /// Path to the member crate directory
    pub path: PathBuf,
    /// Paths to binaries for this member
    pub binaries: Vec<PathBuf>,
}

/// Find target/debug directory, checking current directory and walking up to workspace root
pub(crate) fn find_target_dir() -> Result<PathBuf, String> {
    let mut current =
        std::env::current_dir().map_err(|e| format!("Cannot get current dir: {}", e))?;

    loop {
        let target_dir = current.join("target/debug");
        if target_dir.exists() {
            return Ok(target_dir);
        }

        // Check if this is a workspace root (has [workspace] in Cargo.toml)
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists()
            && let Ok(content) = fs::read_to_string(&cargo_toml)
            && content.contains("[workspace]")
        {
            // This is the workspace root but no target/debug
            return Err("target/debug/ directory not found. Run 'cargo build' first.".to_string());
        }

        // Move up one directory
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }

    Err("target/debug/ directory not found. Run 'cargo build' first.".to_string())
}

/// Check if the current directory is a workspace root (virtual or non-virtual).
/// Virtual workspace: has [workspace] but no [package]
/// Non-virtual workspace: has both [workspace] and [package]
/// Uses from_slice to avoid workspace inheritance resolution issues.
pub(crate) fn is_workspace_root() -> bool {
    let cargo_toml_path = PathBuf::from("Cargo.toml");
    if !cargo_toml_path.exists() {
        return false;
    }

    let Ok(content) = fs::read_to_string(&cargo_toml_path) else {
        return false;
    };

    // Use from_slice to avoid workspace inheritance resolution
    let Ok(manifest) = Manifest::from_slice(content.as_bytes()) else {
        return false;
    };

    manifest.workspace.is_some()
}

/// Check if running from a workspace root and return workspace members.
/// Handles both virtual workspaces (no [package]) and non-virtual workspaces
/// (has both [workspace] and [package]).
pub(crate) fn find_workspace_members() -> Result<Option<Vec<WorkspaceMember>>, String> {
    let cargo_toml_path = PathBuf::from("Cargo.toml");
    if !cargo_toml_path.exists() {
        return Ok(None);
    }

    let cargo_toml_content = fs::read_to_string(&cargo_toml_path)
        .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;

    // Use from_slice to avoid workspace inheritance resolution issues
    let manifest = Manifest::from_slice(cargo_toml_content.as_bytes())
        .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

    // Only proceed if this is a workspace root
    if manifest.workspace.is_none() {
        return Ok(None);
    }

    let workspace = manifest.workspace.as_ref().unwrap();
    let target_dir = PathBuf::from("target/debug");
    if !target_dir.exists() {
        return Err("target/debug/ directory not found. Run 'cargo build' first.".to_string());
    }

    let mut members = Vec::new();

    // For non-virtual workspaces, include the root package as a member
    if let Some(pkg) = &manifest.package {
        let pkg_name = pkg.name.clone();
        // Complete the manifest to discover implicit targets
        let mut root_manifest = manifest.clone();
        let _ = root_manifest.complete_from_path_and_workspace::<toml::Value>(
            &cargo_toml_path,
            None::<(&Manifest<toml::Value>, &Path)>, // No parent workspace for the root
        );
        let binaries = collect_binaries_from_manifest(&root_manifest, &pkg_name, &target_dir);
        if !binaries.is_empty() {
            members.push(WorkspaceMember {
                name: pkg_name,
                path: PathBuf::from("."),
                binaries,
            });
        }
    }

    // Iterate through workspace members
    let cwd = PathBuf::from(".");
    for member_pattern in &workspace.members {
        for member_path in expand_workspace_members(&cwd, member_pattern) {
            let member_cargo_toml = member_path.join("Cargo.toml");
            if !member_cargo_toml.exists() {
                continue;
            }

            // Parse manifest and complete it with workspace context for implicit target discovery
            if let Ok(content) = fs::read_to_string(&member_cargo_toml)
                && let Ok(mut member_manifest) = Manifest::from_slice(content.as_bytes())
                && let Some(pkg) = &member_manifest.package
            {
                let pkg_name = pkg.name.clone();

                // Complete the manifest to discover implicit targets (src/main.rs, src/lib.rs, etc.)
                // Pass the workspace manifest to avoid resolution errors
                let _ = member_manifest.complete_from_path_and_workspace(
                    &member_cargo_toml,
                    Some((&manifest, &cargo_toml_path)),
                );

                let binaries =
                    collect_binaries_from_manifest(&member_manifest, &pkg_name, &target_dir);

                // Only add a member if it has binaries
                if !binaries.is_empty() {
                    members.push(WorkspaceMember {
                        name: pkg_name,
                        path: member_path,
                        binaries,
                    });
                }
            }
        }
    }

    if members.is_empty() {
        return Err("No binary targets found in workspace. Run 'cargo build' first.".to_string());
    }

    Ok(Some(members))
}

/// Collect binaries from a parsed manifest
pub(crate) fn collect_binaries_from_manifest(
    manifest: &Manifest,
    pkg_name: &str,
    target_dir: &Path,
) -> Vec<PathBuf> {
    let mut binaries = Vec::new();

    // Check for [[bin]] targets (populated by complete_from_path_and_workspace)
    // No fallback probe needed - complete_from_path_and_workspace populates bin if there's a binary
    for bin in &manifest.bin {
        let bin_name = bin.name.as_deref().unwrap_or(pkg_name);
        if let Some(bin_path) = find_binary(target_dir, bin_name) {
            binaries.push(bin_path);
        }
    }

    // Check for library target
    if manifest.lib.is_some() {
        let lib_name = manifest
            .lib
            .as_ref()
            .and_then(|l| l.name.clone())
            .unwrap_or_else(|| pkg_name.replace('-', "_"));

        if let Some(lib_path) = find_library(target_dir, &lib_name) {
            binaries.push(lib_path);
        }
    }

    binaries
}

/// Find binaries for all workspace members
pub(crate) fn find_workspace_binaries(manifest: &Manifest) -> Result<Vec<PathBuf>, String> {
    let workspace = manifest
        .workspace
        .as_ref()
        .ok_or("No workspace section found")?;

    let target_dir = PathBuf::from("target/debug");
    if !target_dir.exists() {
        return Err("target/debug/ directory not found. Run 'cargo build' first.".to_string());
    }

    let mut binaries = Vec::new();

    // Iterate through workspace members
    let cwd = PathBuf::from(".");
    for member_pattern in &workspace.members {
        for member_path in expand_workspace_members(&cwd, member_pattern) {
            let member_cargo_toml = member_path.join("Cargo.toml");
            if !member_cargo_toml.exists() {
                continue;
            }

            if let Ok(content) = fs::read_to_string(&member_cargo_toml)
                && let Ok(member_manifest) = Manifest::from_slice(content.as_bytes())
                && let Some(pkg) = &member_manifest.package
            {
                let pkg_name = &pkg.name;

                // Check for explicit [[bin]] targets
                for bin in &member_manifest.bin {
                    let bin_name = bin.name.as_ref().unwrap_or(pkg_name);
                    let bin_path = target_dir.join(bin_name);
                    if bin_path.exists() {
                        binaries.push(bin_path);
                    }
                }

                // Check for default binary
                if member_manifest.bin.is_empty() {
                    let default_bin = target_dir.join(pkg_name);
                    if default_bin.exists() {
                        binaries.push(default_bin);
                    }
                }
            }
        }
    }

    if binaries.is_empty() {
        return Err("No binary targets found in workspace. Run 'cargo build' first.".to_string());
    }

    Ok(binaries)
}

/// Find binary targets by parsing Cargo.toml in the current directory
pub(crate) fn find_crate_binaries() -> Result<Vec<PathBuf>, String> {
    let cargo_toml_path = PathBuf::from("Cargo.toml");
    if !cargo_toml_path.exists() {
        return Err("No Cargo.toml found in current directory. \
                    Run jonesy from a crate root or use --bin <path>."
            .to_string());
    }

    // Read and parse without resolving workspace dependencies
    let cargo_toml_content = fs::read_to_string(&cargo_toml_path)
        .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;

    let manifest = Manifest::from_slice(cargo_toml_content.as_bytes())
        .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

    // Check if this is a workspace root
    if manifest.workspace.is_some() && manifest.package.is_none() {
        return find_workspace_binaries(&manifest);
    }

    let package = manifest
        .package
        .as_ref()
        .ok_or("Cargo.toml has no [package] section")?;

    let package_name = &package.name;

    // Look for the target/debug in the current directory or walk up to find the workspace root
    let target_dir = find_target_dir()?;

    let mut binaries = Vec::new();

    // Check for explicit [[bin]] targets
    for bin in &manifest.bin {
        let bin_name = bin.name.as_ref().unwrap_or(package_name);
        let bin_path = target_dir.join(bin_name);
        if bin_path.exists() {
            binaries.push(bin_path);
        }
    }

    // If no explicit [[bin]] targets, check for default binary (same name as package)
    // This happens when there's a src/main.rs
    if manifest.bin.is_empty() {
        let default_bin = target_dir.join(package_name);
        if default_bin.exists() {
            binaries.push(default_bin);
        }
    }

    // Check for library target
    if manifest.lib.is_some() {
        // Library name defaults to the package name with hyphens replaced by underscores
        let lib_name = manifest
            .lib
            .as_ref()
            .and_then(|l| l.name.clone())
            .unwrap_or_else(|| package_name.replace('-', "_"));

        if let Some(lib_path) = find_library(&target_dir, &lib_name) {
            binaries.push(lib_path);
        }
    }

    if binaries.is_empty() {
        return Err(format!(
            "No binary targets found in target/debug/ for package '{}'. \
             Run 'cargo build' first.",
            package_name
        ));
    }

    Ok(binaries)
}

/// Find a binary by name in a manifest's [[bin]] targets.
///
/// Returns the binary path if found, None otherwise.
/// Handles hyphen/underscore normalization (e.g., "my-bin" matches "my_bin").
pub(crate) fn find_bin_in_manifest(
    bin_name: &str,
    manifest: &Manifest,
    target_dir: &Path,
) -> Option<PathBuf> {
    // Check [[bin]] targets
    for bin in &manifest.bin {
        let manifest_bin_name = bin
            .name
            .as_ref()
            .or(manifest.package.as_ref().map(|p| &p.name));
        if let Some(name) = manifest_bin_name {
            if name == bin_name || name.replace('-', "_") == bin_name {
                let bin_path = target_dir.join(name);
                if bin_path.exists() {
                    return Some(bin_path);
                }
            }
        }
    }

    // Check the package name (default binary)
    if let Some(pkg) = &manifest.package {
        if pkg.name == bin_name || pkg.name.replace('-', "_") == bin_name {
            let bin_path = target_dir.join(&pkg.name);
            if bin_path.exists() {
                return Some(bin_path);
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
        assert!(result.is_err());
    }

    #[test]
    fn test_find_project_root_found() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        File::create(&cargo_toml).unwrap();

        let binary_path = temp_dir.path().join("target/debug/myapp");
        fs::create_dir_all(binary_path.parent().unwrap()).unwrap();

        let result = find_project_root(&binary_path);
        assert!(result.is_ok());
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
        let result = expand_workspace_members(temp_dir.path(), "crates/mylib");
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

        let result = expand_workspace_members(temp_dir.path(), "crates/*");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_expand_workspace_member_glob_no_matches() {
        let temp_dir = TempDir::new().unwrap();
        let result = expand_workspace_members(temp_dir.path(), "nonexistent/*");
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

    // ========================================================================
    // find_bin_in_manifest tests
    // ========================================================================

    #[test]
    fn test_find_bin_in_manifest_no_bins() {
        let content = r#"
            [package]
            name = "my-package"
            version = "0.1.0"
        "#;
        let manifest = Manifest::from_slice(content.as_bytes()).unwrap();
        let result = find_bin_in_manifest("nonexistent", &manifest, Path::new("/tmp"));
        assert!(result.is_none());
    }

    // ========================================================================
    // collect_binaries_from_manifest tests (with temp dir)
    // ========================================================================

    #[test]
    fn test_collect_binaries_no_bins() {
        // Create the manifest with the package but no [[bin]] or [lib] sections
        let content = r#"
            [package]
            name = "my-package"
            version = "0.1.0"
        "#;
        let manifest = Manifest::from_slice(content.as_bytes()).unwrap();
        let target_dir = PathBuf::from("/tmp");

        let binaries = collect_binaries_from_manifest(&manifest, "my-package", &target_dir);

        // No binaries exist in /tmp, so should be empty
        assert!(binaries.is_empty());
    }

    // ========================================================================
    // WorkspaceMember struct tests
    // ========================================================================

    #[test]
    fn test_workspace_member_debug() {
        let member = WorkspaceMember {
            name: "test-crate".to_string(),
            path: PathBuf::from("crates/test-crate"),
            binaries: vec![PathBuf::from("target/debug/test-crate")],
        };

        // Test Debug trait
        let debug_str = format!("{:?}", member);
        assert!(debug_str.contains("test-crate"));
        assert!(debug_str.contains("crates/test-crate"));
    }
}
