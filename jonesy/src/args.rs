use cargo_toml::Manifest;
use std::path::PathBuf;

/// Represents a workspace member crate with its binaries
#[derive(Debug)]
pub(crate) struct WorkspaceMember {
    /// Name of the member crate
    pub name: String,
    /// Path to the member crate directory
    pub path: PathBuf,
    /// Paths to binaries for this member
    pub binaries: Vec<PathBuf>,
}

/// Parsed command line arguments
pub(crate) struct Args {
    /// Paths to binaries to analyze (for non-workspace mode)
    pub binaries: Vec<PathBuf>,
    /// Workspace members to analyze (for workspace mode)
    pub workspace_members: Option<Vec<WorkspaceMember>>,
    /// Whether to show the full call tree (--tree flag)
    pub show_tree: bool,
    /// Whether to only show summary output (--summary-only flag)
    pub summary_only: bool,
    /// Whether to show timing information (--show-timings flag)
    pub show_timings: bool,
    /// Whether to suppress progress messages (--quiet flag)
    pub quiet: bool,
    /// Maximum number of threads to use for parallel analysis
    pub max_threads: usize,
    /// Optional path to config file (--config flag)
    pub config_path: Option<PathBuf>,
    /// Whether to disable hyperlinks in output (--no-hyperlinks flag)
    pub no_hyperlinks: bool,
}

/// Parse command line arguments.
///
/// Modes:
/// 1) No arguments (run from crate root)
///    Parse Cargo.toml to find package name and binary targets,
///    then look for binaries in target/debug/
/// 2) --bin <path>
///    Analyze the specified binary file
/// 3) --lib <path>
///    Analyze the specified library object file
///
/// Optional flags:
/// --tree           Show the full call tree instead of just crate code points
/// --summary-only   Only show summary output, not detailed panic points
/// --max-threads N  Maximum threads for parallel analysis (default: number of CPUs)
/// --config <path>  Path to a TOML config file for allow/deny rules
pub(crate) fn parse_args(args: &[String]) -> Result<Args, String> {
    // Check for flags
    let show_tree = args.iter().any(|a| a == "--tree");
    let summary_only = args.iter().any(|a| a == "--summary-only");
    let show_timings = args.iter().any(|a| a == "--show-timings");
    let quiet = args.iter().any(|a| a == "--quiet");
    let no_hyperlinks = args.iter().any(|a| a == "--no-hyperlinks");

    // Parse --max-threads option
    let max_threads = parse_max_threads(args)?;

    // Parse --config option
    let config_path = parse_config_path(args)?;

    // Filter out standalone flags from args for path parsing
    // Keep --bin and --lib with their arguments for separate processing
    let filtered_args: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| {
            *a != "--tree"
                && *a != "--summary-only"
                && *a != "--show-timings"
                && *a != "--quiet"
                && *a != "--no-hyperlinks"
                && *a != "--max-threads"
                && *a != "--config"
                && !(*i > 0 && args.get(i - 1).is_some_and(|prev| prev == "--max-threads"))
                && !(*i > 0 && args.get(i - 1).is_some_and(|prev| prev == "--config"))
        })
        .map(|(_, a)| a)
        .collect();

    // Check for --bin or --lib flags
    let has_bin_flag = filtered_args.iter().any(|a| *a == "--bin");
    let has_lib_flag = filtered_args.iter().any(|a| *a == "--lib");

    if has_bin_flag && has_lib_flag {
        return Err("--bin and --lib are mutually exclusive".to_string());
    }

    // Check if running from a workspace root first
    let is_workspace_root = is_workspace_only_root();

    // Reject --bin and --lib at workspace level
    if is_workspace_root && (has_bin_flag || has_lib_flag) {
        return Err(
            "--bin and --lib are not supported at workspace level. \
             cd into a member crate directory for target-specific analysis."
                .to_string(),
        );
    }

    let (binaries, workspace_members) = if has_bin_flag {
        (parse_bin_args(&filtered_args)?, None)
    } else if has_lib_flag {
        (parse_lib_args(&filtered_args)?, None)
    } else if filtered_args.len() == 1 {
        // No arguments besides program name - try to find binaries from Cargo.toml
        // Check if this is a workspace root first
        if let Some(members) = find_workspace_members()? {
            (vec![], Some(members))
        } else {
            (find_crate_binaries()?, None)
        }
    } else {
        return Err(usage());
    };

    Ok(Args {
        binaries,
        workspace_members,
        show_tree,
        summary_only,
        show_timings,
        quiet,
        max_threads,
        config_path,
        no_hyperlinks,
    })
}

/// Parse --max-threads option, defaulting to number of available CPUs
fn parse_max_threads(args: &[String]) -> Result<usize, String> {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--max-threads" {
            let value = args
                .get(i + 1)
                .ok_or("--max-threads requires a number argument")?;
            let n: usize = value
                .parse()
                .map_err(|_| format!("Invalid --max-threads value: {}", value))?;
            if n == 0 {
                return Err("--max-threads must be at least 1".to_string());
            }
            return Ok(n);
        }
    }
    // Default to number of available CPUs
    Ok(std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1))
}

/// Parse --config option for custom config file path
fn parse_config_path(args: &[String]) -> Result<Option<PathBuf>, String> {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--config" {
            let value = args.get(i + 1).ok_or("--config requires a path argument")?;
            let path = PathBuf::from(value);
            if !path.exists() {
                return Err(format!("Config file not found: {}", path.display()));
            }
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn usage() -> String {
    "Usage:\n  \
     jonesy [OPTIONS]\n  \
     jonesy [OPTIONS] --bin <name_or_path>\n  \
     jonesy [OPTIONS] --lib [path_to_lib_object]\n\n\
     When run without --bin or --lib, jonesy looks for Cargo.toml in the current\n\
     directory and analyzes all binary targets found in target/debug/.\n\n\
     Options:\n  \
     --bin <name>       Analyze only the specified binary (by name or path)\n  \
     --lib              Analyze only the library target\n  \
     --tree             Show full call tree instead of just crate code points\n  \
     --summary-only     Only show summary, not detailed panic points\n  \
     --quiet            Suppress progress messages (keeps panic points and summary)\n  \
     --show-timings     Show timing information for each analysis step\n  \
     --max-threads N    Maximum threads for parallel analysis (default: CPU count)\n  \
     --config <path>    Path to TOML config file for allow/deny rules\n  \
     --no-hyperlinks    Disable terminal hyperlinks (use plain absolute paths)"
        .to_string()
}

/// Find target/debug directory, checking current directory and walking up to workspace root
fn find_target_dir() -> Result<PathBuf, String> {
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
            && let Ok(content) = std::fs::read_to_string(&cargo_toml)
            && content.contains("[workspace]")
        {
            // This is workspace root but no target/debug
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

/// Check if the current directory is a workspace-only root (has [workspace] but no [package])
fn is_workspace_only_root() -> bool {
    let cargo_toml_path = PathBuf::from("Cargo.toml");
    if !cargo_toml_path.exists() {
        return false;
    }

    let Ok(content) = std::fs::read_to_string(&cargo_toml_path) else {
        return false;
    };

    let Ok(manifest) = Manifest::from_slice(content.as_bytes()) else {
        return false;
    };

    manifest.workspace.is_some() && manifest.package.is_none()
}

/// Check if running from a workspace root and return workspace members
fn find_workspace_members() -> Result<Option<Vec<WorkspaceMember>>, String> {
    let cargo_toml_path = PathBuf::from("Cargo.toml");
    if !cargo_toml_path.exists() {
        return Ok(None);
    }

    let cargo_toml_content = std::fs::read_to_string(&cargo_toml_path)
        .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;

    let manifest = Manifest::from_slice(cargo_toml_content.as_bytes())
        .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

    // Only return Some if this is a workspace-only Cargo.toml (no [package])
    if manifest.workspace.is_none() || manifest.package.is_some() {
        return Ok(None);
    }

    let workspace = manifest.workspace.as_ref().unwrap();
    let target_dir = PathBuf::from("target/debug");
    if !target_dir.exists() {
        return Err("target/debug/ directory not found. Run 'cargo build' first.".to_string());
    }

    let mut members = Vec::new();

    // Iterate through workspace members
    for member_pattern in &workspace.members {
        // Handle glob patterns (e.g., "examples/*")
        let member_paths = if member_pattern.contains('*') {
            let base = member_pattern.trim_end_matches("/*").trim_end_matches("/*");
            let base_path = PathBuf::from(base);
            if base_path.is_dir() {
                std::fs::read_dir(&base_path)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .filter(|e| e.path().is_dir())
                            .map(|e| e.path())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            } else {
                vec![]
            }
        } else {
            vec![PathBuf::from(member_pattern)]
        };

        for member_path in member_paths {
            let member_cargo_toml = member_path.join("Cargo.toml");
            if !member_cargo_toml.exists() {
                continue;
            }

            if let Ok(content) = std::fs::read_to_string(&member_cargo_toml)
                && let Ok(member_manifest) = Manifest::from_slice(content.as_bytes())
                && let Some(pkg) = &member_manifest.package
            {
                let pkg_name = &pkg.name;
                let mut binaries = Vec::new();

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

                // Check for library target
                if member_manifest.lib.is_some() {
                    let lib_name = member_manifest
                        .lib
                        .as_ref()
                        .and_then(|l| l.name.clone())
                        .unwrap_or_else(|| pkg_name.replace('-', "_"));

                    let dylib_path = target_dir.join(format!("lib{}.dylib", lib_name));
                    let rlib_path = target_dir.join(format!("lib{}.rlib", lib_name));

                    if dylib_path.exists() {
                        binaries.push(dylib_path);
                    } else if rlib_path.exists() {
                        binaries.push(rlib_path);
                    }
                }

                // Only add member if it has binaries
                if !binaries.is_empty() {
                    members.push(WorkspaceMember {
                        name: pkg_name.clone(),
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

/// Find binaries for all workspace members
fn find_workspace_binaries(manifest: &Manifest) -> Result<Vec<PathBuf>, String> {
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
    for member_pattern in &workspace.members {
        // Handle glob patterns (e.g., "examples/*")
        let member_paths = if member_pattern.contains('*') {
            // Simple glob expansion for common patterns like "examples/*"
            let base = member_pattern.trim_end_matches("/*").trim_end_matches("/*");
            let base_path = PathBuf::from(base);
            if base_path.is_dir() {
                std::fs::read_dir(&base_path)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .filter(|e| e.path().is_dir())
                            .map(|e| e.path())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            } else {
                vec![]
            }
        } else {
            vec![PathBuf::from(member_pattern)]
        };

        for member_path in member_paths {
            let member_cargo_toml = member_path.join("Cargo.toml");
            if !member_cargo_toml.exists() {
                continue;
            }

            if let Ok(content) = std::fs::read_to_string(&member_cargo_toml)
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
fn find_crate_binaries() -> Result<Vec<PathBuf>, String> {
    let cargo_toml_path = PathBuf::from("Cargo.toml");
    if !cargo_toml_path.exists() {
        return Err("No Cargo.toml found in current directory. \
                    Run jonesy from a crate root or use --bin <path>."
            .to_string());
    }

    // Read and parse without resolving workspace dependencies
    let cargo_toml_content = std::fs::read_to_string(&cargo_toml_path)
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

    // Look for target/debug in current directory or walk up to find workspace root
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
        // Library name defaults to package name with hyphens replaced by underscores
        let lib_name = manifest
            .lib
            .as_ref()
            .and_then(|l| l.name.clone())
            .unwrap_or_else(|| package_name.replace('-', "_"));

        // On macOS, look for .dylib or .rlib
        let dylib_path = target_dir.join(format!("lib{}.dylib", lib_name));
        let rlib_path = target_dir.join(format!("lib{}.rlib", lib_name));

        if dylib_path.exists() {
            binaries.push(dylib_path);
        } else if rlib_path.exists() {
            binaries.push(rlib_path);
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

/// Parse --bin name_or_path
/// Can be either a path to a binary or a binary name to look up in Cargo.toml
fn parse_bin_args(args: &[&String]) -> Result<Vec<PathBuf>, String> {
    // Find the argument after --bin
    let bin_arg_idx = args
        .iter()
        .position(|a| *a == "--bin")
        .ok_or("--bin flag not found")?;
    let bin_name = args
        .get(bin_arg_idx + 1)
        .ok_or("--bin requires a binary name or path")?;

    // Reject unexpected trailing positional args
    if let Some(extra) = args.get(bin_arg_idx + 2) {
        if !extra.starts_with("--") {
            return Err(format!(
                "Unexpected extra argument '{}' after --bin <name_or_path>",
                extra
            ));
        }
    }

    let binary_path = PathBuf::from(bin_name.as_str());

    // First check if it's a path that exists
    if binary_path.exists() {
        std::fs::File::open(&binary_path)
            .map_err(|e| format!("Cannot read binary at {:?}: {}", binary_path, e))?;
        return Ok(vec![binary_path]);
    }

    // Otherwise, treat it as a binary name and look it up in Cargo.toml
    let cargo_toml_path = PathBuf::from("Cargo.toml");
    if !cargo_toml_path.exists() {
        return Err(format!(
            "Binary '{}' not found and no Cargo.toml in current directory",
            bin_name
        ));
    }

    let cargo_toml_content = std::fs::read_to_string(&cargo_toml_path)
        .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;

    let manifest = Manifest::from_slice(cargo_toml_content.as_bytes())
        .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

    // Look for target/debug directory
    let target_dir = find_target_dir()?;

    // Check if this binary name matches any [[bin]] target
    for bin in &manifest.bin {
        let manifest_bin_name = bin
            .name
            .as_ref()
            .or(manifest.package.as_ref().map(|p| &p.name));
        if let Some(name) = manifest_bin_name
            && (name == bin_name.as_str() || name.replace('-', "_") == bin_name.as_str())
        {
            let bin_path = target_dir.join(name);
            if bin_path.exists() {
                return Ok(vec![bin_path]);
            }
        }
    }

    // Check if it matches the package name (default binary)
    if let Some(pkg) = &manifest.package
        && (pkg.name == bin_name.as_str() || pkg.name.replace('-', "_") == bin_name.as_str())
    {
        let bin_path = target_dir.join(&pkg.name);
        if bin_path.exists() {
            return Ok(vec![bin_path]);
        }
    }

    // If this is a workspace, search workspace members
    if manifest.workspace.is_some() {
        if let Ok(workspace_binaries) = find_workspace_binaries(&manifest) {
            for bin_path in workspace_binaries {
                if let Some(name) = bin_path.file_name().and_then(|n| n.to_str()) {
                    if name == bin_name.as_str() || name.replace('-', "_") == bin_name.as_str() {
                        return Ok(vec![bin_path]);
                    }
                }
            }
        }
    }

    Err(format!(
        "Binary '{}' not found in Cargo.toml or target/debug/",
        bin_name
    ))
}

/// Parse --lib [path_to_library_object]
/// If a path is provided, use it directly.
/// Otherwise, find the library target from Cargo.toml
fn parse_lib_args(args: &[&String]) -> Result<Vec<PathBuf>, String> {
    // Find the --lib flag position
    let lib_arg_idx = args
        .iter()
        .position(|a| *a == "--lib")
        .ok_or("--lib flag not found")?;

    // Check if there's an argument after --lib that isn't another flag
    let lib_path_arg = args.get(lib_arg_idx + 1).filter(|a| !a.starts_with("--"));

    // Reject unexpected trailing positional args
    if lib_path_arg.is_some() {
        if let Some(extra) = args.get(lib_arg_idx + 2) {
            if !extra.starts_with("--") {
                return Err(format!(
                    "Unexpected extra argument '{}' after --lib [path_to_lib_object]",
                    extra
                ));
            }
        }
    }

    if let Some(path_str) = lib_path_arg {
        let binary_path = PathBuf::from(path_str.as_str());
        if !binary_path.exists() {
            return Err(format!(
                "Library shared object not found at {:?}",
                binary_path
            ));
        }
        std::fs::File::open(&binary_path).map_err(|e| {
            format!(
                "Cannot read Library shared object at {:?}: {}",
                binary_path, e
            )
        })?;
        return Ok(vec![binary_path]);
    }

    // No path provided - find the library from Cargo.toml
    let cargo_toml_path = PathBuf::from("Cargo.toml");
    if !cargo_toml_path.exists() {
        return Err("No Cargo.toml found. Use --lib <path> to specify library path.".to_string());
    }

    let cargo_toml_content = std::fs::read_to_string(&cargo_toml_path)
        .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;

    let manifest = Manifest::from_slice(cargo_toml_content.as_bytes())
        .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

    // Check for explicit [lib] or implicit library (src/lib.rs)
    let has_implicit_lib = PathBuf::from("src/lib.rs").exists();
    if manifest.lib.is_none() && !has_implicit_lib {
        return Err("No library target found in Cargo.toml or src/lib.rs".to_string());
    }

    let target_dir = find_target_dir()?;

    // Library name defaults to package name with hyphens replaced by underscores
    let lib_name = manifest
        .lib
        .as_ref()
        .and_then(|l| l.name.clone())
        .or_else(|| manifest.package.as_ref().map(|p| p.name.replace('-', "_")))
        .ok_or("Cannot determine library name")?;

    // On macOS, look for .dylib or .rlib
    let dylib_path = target_dir.join(format!("lib{}.dylib", lib_name));
    let rlib_path = target_dir.join(format!("lib{}.rlib", lib_name));

    if dylib_path.exists() {
        Ok(vec![dylib_path])
    } else if rlib_path.exists() {
        Ok(vec![rlib_path])
    } else {
        Err(format!(
            "Library 'lib{}' not found in target/debug/. Run 'cargo build' first.",
            lib_name
        ))
    }
}
