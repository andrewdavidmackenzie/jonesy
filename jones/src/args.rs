use cargo_toml::Manifest;
use std::path::PathBuf;

/// Parsed command line arguments
pub(crate) struct Args {
    /// Paths to binaries to analyze
    pub binaries: Vec<PathBuf>,
    /// Whether to show the full call tree (--tree flag)
    pub show_tree: bool,
    /// Whether to only show summary output (--summary-only flag)
    pub summary_only: bool,
    /// Maximum number of threads to use for parallel analysis
    pub max_threads: usize,
    /// Optional path to config file (--config flag)
    pub config_path: Option<PathBuf>,
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

    // Parse --max-threads option
    let max_threads = parse_max_threads(args)?;

    // Parse --config option
    let config_path = parse_config_path(args)?;

    // Filter out flags and options with values from args for path parsing
    let filtered_args: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| {
            *a != "--tree"
                && *a != "--summary-only"
                && *a != "--max-threads"
                && *a != "--config"
                && !(*i > 0 && args.get(i - 1).is_some_and(|prev| prev == "--max-threads"))
                && !(*i > 0 && args.get(i - 1).is_some_and(|prev| prev == "--config"))
        })
        .map(|(_, a)| a)
        .collect();

    let binaries = if filtered_args.len() == 1 {
        // No arguments besides program name - try to find binaries from Cargo.toml
        find_crate_binaries()?
    } else if filtered_args.len() == 3 {
        match filtered_args[1].as_str() {
            "--bin" => parse_bin_args(&filtered_args)?,
            "--lib" => parse_lib_args(&filtered_args)?,
            _ => return Err(usage()),
        }
    } else {
        return Err(usage());
    };

    Ok(Args {
        binaries,
        show_tree,
        summary_only,
        max_threads,
        config_path,
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
     jones [OPTIONS]\n  \
     jones [OPTIONS] --bin <path_to_binary>\n  \
     jones [OPTIONS] --lib <path_to_lib_object>\n\n\
     When run without --bin or --lib, jones looks for Cargo.toml in the current\n\
     directory and analyzes all binary targets found in target/debug/.\n\n\
     Options:\n  \
     --tree             Show full call tree instead of just crate code points\n  \
     --summary-only     Only show summary, not detailed panic points\n  \
     --max-threads N    Maximum threads for parallel analysis (default: CPU count)\n  \
     --config <path>    Path to TOML config file for allow/deny rules"
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
                    Run jones from a crate root or use --bin <path>."
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

/// Parse --bin path_to_binary
fn parse_bin_args(args: &[&String]) -> Result<Vec<PathBuf>, String> {
    let binary_path = PathBuf::from(args[2].as_str());

    // Check that the file exists
    if !binary_path.exists() {
        return Err(format!("Binary not found at {:?}", binary_path));
    }

    // Check that the file is readable by attempting to open it
    std::fs::File::open(&binary_path)
        .map_err(|e| format!("Cannot read binary at {:?}: {}", binary_path, e))?;

    Ok(vec![binary_path])
}

/// Parse --lib path_to_library_object
fn parse_lib_args(args: &[&String]) -> Result<Vec<PathBuf>, String> {
    let binary_path = PathBuf::from(args[2].as_str());

    // Check that the file exists
    if !binary_path.exists() {
        return Err(format!(
            "Library shared object not found at {:?}",
            binary_path
        ));
    }

    // Check that the file is readable by attempting to open it
    std::fs::File::open(&binary_path).map_err(|e| {
        format!(
            "Cannot read Library shared object at {:?}: {}",
            binary_path, e
        )
    })?;

    Ok(vec![binary_path])
}
