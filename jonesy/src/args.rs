use crate::cargo::{find_binary, find_library};
use cargo_toml::Manifest;
use std::path::{Path, PathBuf};

/// Output format and display configuration for analysis results.
/// Consolidates format selection with display options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable terminal output
    Text {
        /// Show full call tree instead of just crate code points
        tree: bool,
        /// Only show summary, not detailed panic points
        summary_only: bool,
        /// Suppress progress messages
        quiet: bool,
        /// Use terminal hyperlinks for file paths
        hyperlinks: bool,
    },
    /// Machine-readable JSON output (implies quiet)
    Json {
        /// Show full call tree (children) instead of flat list
        tree: bool,
        /// Only include summary, not detailed panic points
        summary_only: bool,
    },
    /// Self-contained HTML report (implies quiet)
    Html {
        /// Show full call tree instead of flat list
        tree: bool,
        /// Only include summary, not detailed panic points
        summary_only: bool,
    },
}

impl Default for OutputFormat {
    fn default() -> Self {
        OutputFormat::Text {
            tree: false,
            summary_only: false,
            quiet: false,
            hyperlinks: true,
        }
    }
}

impl OutputFormat {
    /// Create a text output format with the given options
    pub fn text(tree: bool, summary_only: bool, quiet: bool, hyperlinks: bool) -> Self {
        OutputFormat::Text {
            tree,
            summary_only,
            quiet,
            hyperlinks,
        }
    }

    /// Create a JSON output format with the given options
    pub fn json(tree: bool, summary_only: bool) -> Self {
        OutputFormat::Json { tree, summary_only }
    }

    /// Create an HTML output format with the given options
    pub fn html(tree: bool, summary_only: bool) -> Self {
        OutputFormat::Html { tree, summary_only }
    }

    /// Create a quiet text output format (for LSP/programmatic use)
    pub fn quiet() -> Self {
        OutputFormat::Text {
            tree: false,
            summary_only: false,
            quiet: true,
            hyperlinks: false,
        }
    }

    /// Returns true if this is JSON output
    pub fn is_json(&self) -> bool {
        matches!(self, OutputFormat::Json { .. })
    }

    /// Returns true if this is HTML output
    pub fn is_html(&self) -> bool {
        matches!(self, OutputFormat::Html { .. })
    }

    /// Returns true if this is text output
    pub fn is_text(&self) -> bool {
        matches!(self, OutputFormat::Text { .. })
    }

    /// Returns true if progress messages should be shown
    pub fn show_progress(&self) -> bool {
        match self {
            OutputFormat::Text {
                quiet,
                summary_only,
                ..
            } => !quiet && !summary_only,
            OutputFormat::Json { .. } | OutputFormat::Html { .. } => false,
        }
    }

    /// Returns true if only the summary should be shown (no panic point details)
    pub fn is_summary_only(&self) -> bool {
        match self {
            OutputFormat::Text { summary_only, .. }
            | OutputFormat::Json { summary_only, .. }
            | OutputFormat::Html { summary_only, .. } => *summary_only,
        }
    }

    /// Returns true if the full call tree should be shown
    pub fn show_tree(&self) -> bool {
        match self {
            OutputFormat::Text { tree, .. }
            | OutputFormat::Json { tree, .. }
            | OutputFormat::Html { tree, .. } => *tree,
        }
    }

    /// Returns true if hyperlinks should be used in output
    pub fn use_hyperlinks(&self) -> bool {
        match self {
            OutputFormat::Text { hyperlinks, .. } => *hyperlinks,
            OutputFormat::Json { .. } | OutputFormat::Html { .. } => false,
        }
    }
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

/// Parsed command line arguments
pub struct Args {
    /// Paths to binaries to analyze (for non-workspace mode)
    pub binaries: Vec<PathBuf>,
    /// Workspace members to analyze (for workspace mode)
    pub workspace_members: Option<Vec<WorkspaceMember>>,
    /// Whether to show timing information (--show-timings flag)
    pub show_timings: bool,
    /// Maximum number of threads to use for parallel analysis
    pub max_threads: usize,
    /// Optional path to config file (--config flag)
    pub config_path: Option<PathBuf>,
    /// Output format and display options
    pub output: OutputFormat,
    /// Run in LSP server mode
    pub lsp_mode: bool,
}

/// The version of jonesy, read from Cargo.toml at compile time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
/// --version        Print version and exit
pub fn parse_args(args: &[String]) -> Result<Args, String> {
    // Handle --version flag early
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("jonesy {}", VERSION);
        std::process::exit(0);
    }

    // Check for lsp subcommand
    if args.get(1).is_some_and(|a| a == "lsp") {
        return Ok(Args {
            binaries: Vec::new(),
            workspace_members: None,
            show_timings: false,
            max_threads: 1,
            config_path: None,
            output: OutputFormat::default(),
            lsp_mode: true,
        });
    }

    // Check for flags
    let show_tree = args.iter().any(|a| a == "--tree");
    let summary_only = args.iter().any(|a| a == "--summary-only");
    let show_timings = args.iter().any(|a| a == "--show-timings");
    let quiet = args.iter().any(|a| a == "--quiet");
    let no_hyperlinks = args.iter().any(|a| a == "--no-hyperlinks");

    // Parse --format option with validation
    let output = parse_output_format(args, show_tree, summary_only, quiet, no_hyperlinks)?;

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
                && *a != "--format"
                && !(*i > 0 && args.get(i - 1).is_some_and(|prev| prev == "--max-threads"))
                && !(*i > 0 && args.get(i - 1).is_some_and(|prev| prev == "--config"))
                && !(*i > 0 && args.get(i - 1).is_some_and(|prev| prev == "--format"))
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
    let at_workspace_root = is_workspace_root();

    // Reject --bin and --lib at workspace level
    if at_workspace_root && (has_bin_flag || has_lib_flag) {
        return Err("--bin and --lib are not supported at workspace level. \
             cd into a member crate directory for target-specific analysis."
            .to_string());
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
        show_timings,
        max_threads,
        config_path,
        output,
        lsp_mode: false,
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

/// Parse --format option and build OutputFormat with proper validation
fn parse_output_format(
    args: &[String],
    show_tree: bool,
    summary_only: bool,
    quiet: bool,
    no_hyperlinks: bool,
) -> Result<OutputFormat, String> {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--format" {
            let value = args
                .get(i + 1)
                .ok_or("--format requires an argument (text, json, or html)")?;
            return match value.to_lowercase().as_str() {
                "text" => Ok(OutputFormat::text(
                    show_tree,
                    summary_only,
                    quiet,
                    !no_hyperlinks,
                )),
                "json" => Ok(OutputFormat::json(show_tree, summary_only)),
                "html" => Ok(OutputFormat::html(show_tree, summary_only)),
                _ => Err(format!(
                    "Invalid format '{}'. Valid options: text, json, html",
                    value
                )),
            };
        }
    }
    // No --format flag, default to text
    Ok(OutputFormat::text(
        show_tree,
        summary_only,
        quiet,
        !no_hyperlinks,
    ))
}

fn usage() -> String {
    format!(
        "jonesy {} - Find panic points in Rust binaries\n\n\
         Usage:\n  \
         jonesy [OPTIONS]\n  \
         jonesy [OPTIONS] --bin <name_or_path>\n  \
         jonesy [OPTIONS] --lib [path_to_lib_object]\n  \
         jonesy lsp\n\n\
         When run without --bin or --lib, jonesy looks for Cargo.toml in the current\n\
         directory and analyzes all binary targets found in target/debug/.\n\n\
         Subcommands:\n  \
         lsp                Start LSP server for IDE integration\n\n\
         Options:\n  \
         --bin <name>       Analyze only the specified binary (by name or path)\n  \
         --lib              Analyze only the library target\n  \
         --tree             Show full call tree instead of just crate code points\n  \
         --summary-only     Only show summary, not detailed panic points\n  \
         --quiet            Suppress progress messages (keeps panic points and summary)\n  \
         --show-timings     Show timing information for each analysis step\n  \
         --max-threads N    Maximum threads for parallel analysis (default: CPU count)\n  \
         --config <path>    Path to TOML config file for allow/deny rules\n  \
         --no-hyperlinks    Disable terminal hyperlinks (use plain absolute paths)\n  \
         --format <fmt>     Output format: text (default), json, html\n  \
         --version, -V      Print version and exit",
        VERSION
    )
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

/// Check if the current directory is a workspace root (virtual or non-virtual).
/// Virtual workspace: has [workspace] but no [package]
/// Non-virtual workspace: has both [workspace] and [package]
/// Uses from_slice to avoid workspace inheritance resolution issues.
fn is_workspace_root() -> bool {
    let cargo_toml_path = PathBuf::from("Cargo.toml");
    if !cargo_toml_path.exists() {
        return false;
    }

    let Ok(content) = std::fs::read_to_string(&cargo_toml_path) else {
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
fn find_workspace_members() -> Result<Option<Vec<WorkspaceMember>>, String> {
    let cargo_toml_path = PathBuf::from("Cargo.toml");
    if !cargo_toml_path.exists() {
        return Ok(None);
    }

    let cargo_toml_content = std::fs::read_to_string(&cargo_toml_path)
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
            None::<(&Manifest<toml::Value>, &std::path::Path)>, // No parent workspace for the root
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

            // Parse manifest and complete it with workspace context for implicit target discovery
            if let Ok(content) = std::fs::read_to_string(&member_cargo_toml)
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

                // Only add member if it has binaries
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
fn collect_binaries_from_manifest(
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

/// Extract the binary name/path argument from --bin flag.
///
/// Returns the argument value after --bin, or an error if:
/// - --bin flag is not found
/// - No value follows --bin
/// - There are unexpected trailing positional arguments
///
/// This is a pure function that only examines the args slice.
fn extract_bin_arg<'a>(args: &[&'a String]) -> Result<&'a str, String> {
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

    Ok(bin_name.as_str())
}

/// Find a binary by name in a manifest's [[bin]] targets.
///
/// Returns the binary path if found, None otherwise.
/// Handles hyphen/underscore normalization (e.g., "my-bin" matches "my_bin").
fn find_bin_in_manifest(
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

    // Check package name (default binary)
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

/// Parse --bin name_or_path
/// Can be either a path to a binary or a binary name to look up in Cargo.toml
fn parse_bin_args(args: &[&String]) -> Result<Vec<PathBuf>, String> {
    let bin_name = extract_bin_arg(args)?;
    let binary_path = PathBuf::from(bin_name);

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

    // Check if this binary name matches any [[bin]] target or package name
    if let Some(bin_path) = find_bin_in_manifest(bin_name, &manifest, &target_dir) {
        return Ok(vec![bin_path]);
    }

    // If this is a workspace, search workspace members
    if manifest.workspace.is_some() {
        if let Ok(workspace_binaries) = find_workspace_binaries(&manifest) {
            for bin_path in workspace_binaries {
                if let Some(name) = bin_path.file_name().and_then(|n| n.to_str()) {
                    if name == bin_name || name.replace('-', "_") == bin_name {
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

/// Extract the optional library path argument from --lib flag.
///
/// Returns Ok(Some(path)) if --lib has a path argument
/// Returns Ok(None) if --lib is used without a path (use Cargo.toml lookup)
/// Returns Err if --lib flag not found or there are unexpected trailing args
///
/// This is a pure function that only examines the args slice.
fn extract_lib_arg<'a>(args: &[&'a String]) -> Result<Option<&'a str>, String> {
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

    Ok(lib_path_arg.map(|s| s.as_str()))
}

/// Determine the library name from a manifest.
///
/// Returns the library name from [lib] section if present,
/// otherwise derives it from package name (replacing hyphens with underscores).
fn get_lib_name(manifest: &Manifest) -> Option<String> {
    manifest
        .lib
        .as_ref()
        .and_then(|l| l.name.clone())
        .or_else(|| manifest.package.as_ref().map(|p| p.name.replace('-', "_")))
}

/// Find a library file by name in the target directory.
///
/// Checks for .dylib, .rlib, and .a files in order.
/// Returns the first existing library path, or None if not found.
fn find_lib_in_target(lib_name: &str, target_dir: &Path) -> Option<PathBuf> {
    let dylib_path = target_dir.join(format!("lib{}.dylib", lib_name));
    let rlib_path = target_dir.join(format!("lib{}.rlib", lib_name));
    let staticlib_path = target_dir.join(format!("lib{}.a", lib_name));

    if dylib_path.exists() {
        Some(dylib_path)
    } else if rlib_path.exists() {
        Some(rlib_path)
    } else if staticlib_path.exists() {
        Some(staticlib_path)
    } else {
        None
    }
}

/// Parse --lib [path_to_library_object]
/// If a path is provided, use it directly.
/// Otherwise, find the library target from Cargo.toml
fn parse_lib_args(args: &[&String]) -> Result<Vec<PathBuf>, String> {
    let lib_path_arg = extract_lib_arg(args)?;

    if let Some(path_str) = lib_path_arg {
        let binary_path = PathBuf::from(path_str);
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

    let lib_name = get_lib_name(&manifest).ok_or("Cannot determine library name")?;

    find_lib_in_target(&lib_name, &target_dir).map(|p| vec![p]).ok_or_else(|| {
        format!(
            "Library 'lib{}' not found in target/debug/. Run 'cargo build' first.",
            lib_name
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // OutputFormat tests
    // ========================================================================

    #[test]
    fn test_output_format_default() {
        let format = OutputFormat::default();
        assert!(format.is_text());
        assert!(!format.is_json());
        assert!(!format.is_html());
        assert!(format.show_progress());
        assert!(!format.is_summary_only());
        assert!(!format.show_tree());
        assert!(format.use_hyperlinks());
    }

    #[test]
    fn test_output_format_text_constructor() {
        let format = OutputFormat::text(true, true, true, false);
        assert!(format.is_text());
        assert!(format.show_tree());
        assert!(format.is_summary_only());
        assert!(!format.show_progress()); // quiet=true means no progress
        assert!(!format.use_hyperlinks());
    }

    #[test]
    fn test_output_format_json_constructor() {
        let format = OutputFormat::json(true, false);
        assert!(format.is_json());
        assert!(!format.is_text());
        assert!(!format.is_html());
        assert!(format.show_tree());
        assert!(!format.is_summary_only());
        assert!(!format.show_progress()); // JSON never shows progress
        assert!(!format.use_hyperlinks()); // JSON never uses hyperlinks
    }

    #[test]
    fn test_output_format_html_constructor() {
        let format = OutputFormat::html(false, true);
        assert!(format.is_html());
        assert!(!format.is_text());
        assert!(!format.is_json());
        assert!(!format.show_tree());
        assert!(format.is_summary_only());
        assert!(!format.show_progress()); // HTML never shows progress
        assert!(!format.use_hyperlinks()); // HTML never uses hyperlinks
    }

    #[test]
    fn test_output_format_quiet() {
        let format = OutputFormat::quiet();
        assert!(format.is_text());
        assert!(!format.show_progress());
        assert!(!format.use_hyperlinks());
    }

    #[test]
    fn test_output_format_show_progress_logic() {
        // Progress shown when text, not quiet, not summary_only
        let format = OutputFormat::text(false, false, false, true);
        assert!(format.show_progress());

        // No progress when quiet
        let format = OutputFormat::text(false, false, true, true);
        assert!(!format.show_progress());

        // No progress when summary_only
        let format = OutputFormat::text(false, true, false, true);
        assert!(!format.show_progress());

        // No progress when both
        let format = OutputFormat::text(false, true, true, true);
        assert!(!format.show_progress());
    }

    // ========================================================================
    // parse_max_threads tests
    // ========================================================================

    #[test]
    fn test_parse_max_threads_default() {
        let args = vec!["jonesy".to_string()];
        let result = parse_max_threads(&args).unwrap();
        // Default should be at least 1
        assert!(result >= 1);
    }

    #[test]
    fn test_parse_max_threads_explicit() {
        let args = vec![
            "jonesy".to_string(),
            "--max-threads".to_string(),
            "4".to_string(),
        ];
        let result = parse_max_threads(&args).unwrap();
        assert_eq!(result, 4);
    }

    #[test]
    fn test_parse_max_threads_one() {
        let args = vec![
            "jonesy".to_string(),
            "--max-threads".to_string(),
            "1".to_string(),
        ];
        let result = parse_max_threads(&args).unwrap();
        assert_eq!(result, 1);
    }

    #[test]
    fn test_parse_max_threads_zero_error() {
        let args = vec![
            "jonesy".to_string(),
            "--max-threads".to_string(),
            "0".to_string(),
        ];
        let result = parse_max_threads(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at least 1"));
    }

    #[test]
    fn test_parse_max_threads_missing_value() {
        let args = vec!["jonesy".to_string(), "--max-threads".to_string()];
        let result = parse_max_threads(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires a number"));
    }

    #[test]
    fn test_parse_max_threads_invalid_value() {
        let args = vec![
            "jonesy".to_string(),
            "--max-threads".to_string(),
            "abc".to_string(),
        ];
        let result = parse_max_threads(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid"));
    }

    // ========================================================================
    // parse_output_format tests
    // ========================================================================

    #[test]
    fn test_parse_output_format_default() {
        let args = vec!["jonesy".to_string()];
        let result = parse_output_format(&args, false, false, false, false).unwrap();
        assert!(result.is_text());
        assert!(result.use_hyperlinks());
    }

    #[test]
    fn test_parse_output_format_text_explicit() {
        let args = vec![
            "jonesy".to_string(),
            "--format".to_string(),
            "text".to_string(),
        ];
        let result = parse_output_format(&args, false, false, false, false).unwrap();
        assert!(result.is_text());
    }

    #[test]
    fn test_parse_output_format_json() {
        let args = vec![
            "jonesy".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ];
        let result = parse_output_format(&args, true, false, false, false).unwrap();
        assert!(result.is_json());
        assert!(result.show_tree());
    }

    #[test]
    fn test_parse_output_format_html() {
        let args = vec![
            "jonesy".to_string(),
            "--format".to_string(),
            "html".to_string(),
        ];
        let result = parse_output_format(&args, false, true, false, false).unwrap();
        assert!(result.is_html());
        assert!(result.is_summary_only());
    }

    #[test]
    fn test_parse_output_format_case_insensitive() {
        let args = vec![
            "jonesy".to_string(),
            "--format".to_string(),
            "JSON".to_string(),
        ];
        let result = parse_output_format(&args, false, false, false, false).unwrap();
        assert!(result.is_json());
    }

    #[test]
    fn test_parse_output_format_invalid() {
        let args = vec![
            "jonesy".to_string(),
            "--format".to_string(),
            "xml".to_string(),
        ];
        let result = parse_output_format(&args, false, false, false, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid format"));
    }

    #[test]
    fn test_parse_output_format_missing_value() {
        let args = vec!["jonesy".to_string(), "--format".to_string()];
        let result = parse_output_format(&args, false, false, false, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires an argument"));
    }

    #[test]
    fn test_parse_output_format_no_hyperlinks() {
        let args = vec!["jonesy".to_string()];
        let result = parse_output_format(&args, false, false, false, true).unwrap();
        assert!(result.is_text());
        assert!(!result.use_hyperlinks());
    }

    #[test]
    fn test_parse_output_format_with_flags() {
        let args = vec!["jonesy".to_string()];
        let result = parse_output_format(&args, true, true, true, true).unwrap();
        assert!(result.is_text());
        assert!(result.show_tree());
        assert!(result.is_summary_only());
        assert!(!result.show_progress()); // quiet + summary_only
        assert!(!result.use_hyperlinks());
    }

    // ========================================================================
    // usage tests
    // ========================================================================

    #[test]
    fn test_usage_contains_version() {
        let help = usage();
        assert!(help.contains(VERSION));
    }

    #[test]
    fn test_usage_contains_key_options() {
        let help = usage();
        assert!(help.contains("--bin"));
        assert!(help.contains("--lib"));
        assert!(help.contains("--tree"));
        assert!(help.contains("--quiet"));
        assert!(help.contains("--format"));
        assert!(help.contains("--config"));
        assert!(help.contains("--max-threads"));
        assert!(help.contains("lsp"));
    }

    #[test]
    fn test_usage_contains_format_options() {
        let help = usage();
        assert!(help.contains("text"));
        assert!(help.contains("json"));
        assert!(help.contains("html"));
    }

    // ========================================================================
    // extract_bin_arg tests
    // ========================================================================

    #[test]
    fn test_extract_bin_arg_valid() {
        let args = vec![
            "jonesy".to_string(),
            "--bin".to_string(),
            "my-binary".to_string(),
        ];
        let refs: Vec<&String> = args.iter().collect();
        let result = extract_bin_arg(&refs).unwrap();
        assert_eq!(result, "my-binary");
    }

    #[test]
    fn test_extract_bin_arg_with_path() {
        let args = vec![
            "jonesy".to_string(),
            "--bin".to_string(),
            "/path/to/binary".to_string(),
        ];
        let refs: Vec<&String> = args.iter().collect();
        let result = extract_bin_arg(&refs).unwrap();
        assert_eq!(result, "/path/to/binary");
    }

    #[test]
    fn test_extract_bin_arg_missing_value() {
        let args = vec!["jonesy".to_string(), "--bin".to_string()];
        let refs: Vec<&String> = args.iter().collect();
        let result = extract_bin_arg(&refs);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires a binary name"));
    }

    #[test]
    fn test_extract_bin_arg_no_flag() {
        let args = vec!["jonesy".to_string()];
        let refs: Vec<&String> = args.iter().collect();
        let result = extract_bin_arg(&refs);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("flag not found"));
    }

    #[test]
    fn test_extract_bin_arg_extra_positional() {
        let args = vec![
            "jonesy".to_string(),
            "--bin".to_string(),
            "my-binary".to_string(),
            "extra-arg".to_string(),
        ];
        let refs: Vec<&String> = args.iter().collect();
        let result = extract_bin_arg(&refs);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unexpected extra argument"));
    }

    #[test]
    fn test_extract_bin_arg_allows_trailing_flags() {
        let args = vec![
            "jonesy".to_string(),
            "--bin".to_string(),
            "my-binary".to_string(),
            "--quiet".to_string(),
        ];
        let refs: Vec<&String> = args.iter().collect();
        let result = extract_bin_arg(&refs).unwrap();
        assert_eq!(result, "my-binary");
    }

    // ========================================================================
    // extract_lib_arg tests
    // ========================================================================

    #[test]
    fn test_extract_lib_arg_with_path() {
        let args = vec![
            "jonesy".to_string(),
            "--lib".to_string(),
            "/path/to/lib.rlib".to_string(),
        ];
        let refs: Vec<&String> = args.iter().collect();
        let result = extract_lib_arg(&refs).unwrap();
        assert_eq!(result, Some("/path/to/lib.rlib"));
    }

    #[test]
    fn test_extract_lib_arg_without_path() {
        let args = vec!["jonesy".to_string(), "--lib".to_string()];
        let refs: Vec<&String> = args.iter().collect();
        let result = extract_lib_arg(&refs).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_lib_arg_followed_by_flag() {
        let args = vec![
            "jonesy".to_string(),
            "--lib".to_string(),
            "--quiet".to_string(),
        ];
        let refs: Vec<&String> = args.iter().collect();
        let result = extract_lib_arg(&refs).unwrap();
        assert_eq!(result, None); // --quiet is not a path
    }

    #[test]
    fn test_extract_lib_arg_no_flag() {
        let args = vec!["jonesy".to_string()];
        let refs: Vec<&String> = args.iter().collect();
        let result = extract_lib_arg(&refs);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("flag not found"));
    }

    #[test]
    fn test_extract_lib_arg_extra_positional() {
        let args = vec![
            "jonesy".to_string(),
            "--lib".to_string(),
            "/path/to/lib.rlib".to_string(),
            "extra-arg".to_string(),
        ];
        let refs: Vec<&String> = args.iter().collect();
        let result = extract_lib_arg(&refs);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unexpected extra argument"));
    }

    // ========================================================================
    // get_lib_name tests
    // ========================================================================

    #[test]
    fn test_get_lib_name_from_lib_section() {
        let content = r#"
            [package]
            name = "my-package"
            version = "0.1.0"

            [lib]
            name = "custom_lib_name"
        "#;
        let manifest = Manifest::from_slice(content.as_bytes()).unwrap();
        let result = get_lib_name(&manifest);
        assert_eq!(result, Some("custom_lib_name".to_string()));
    }

    #[test]
    fn test_get_lib_name_from_package() {
        let content = r#"
            [package]
            name = "my-package"
            version = "0.1.0"
        "#;
        let manifest = Manifest::from_slice(content.as_bytes()).unwrap();
        let result = get_lib_name(&manifest);
        assert_eq!(result, Some("my_package".to_string())); // hyphen -> underscore
    }

    #[test]
    fn test_get_lib_name_no_package() {
        let content = r#"
            [workspace]
            members = ["crate_a"]
        "#;
        let manifest = Manifest::from_slice(content.as_bytes()).unwrap();
        let result = get_lib_name(&manifest);
        assert_eq!(result, None);
    }

    // ========================================================================
    // find_lib_in_target tests
    // ========================================================================

    #[test]
    fn test_find_lib_in_target_nonexistent() {
        let result = find_lib_in_target("nonexistent", Path::new("/tmp"));
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
}
