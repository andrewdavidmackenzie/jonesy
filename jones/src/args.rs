use std::path::PathBuf;

/// Parsed command line arguments
pub(crate) struct Args {
    /// Paths to binaries to analyze
    pub binaries: Vec<PathBuf>,
    /// Whether to show the full call tree (--tree flag)
    pub show_tree: bool,
    /// Whether to show drop/cleanup panic paths (--drops flag)
    pub show_drops: bool,
}

/// parse the command line arguments into the three cases accepted:
/// 1) --example
///    Verify there is an example with that name and then find the compiled binary in ./target
///    depending on --debug or --release switch
/// 2) --examples
///    Generate a Vec of the paths to example binaries in ./target according to --debug or
///    --release switch
/// 3) --bin $path
///    Check that the binary file specified by the --bin option exists and is readable
///
/// Optional flags:
/// --tree  Show the full call tree instead of just crate code points
pub(crate) fn parse_args(args: &[String]) -> Result<Args, String> {
    // Check for flags
    let show_tree = args.iter().any(|a| a == "--tree");
    let show_drops = args.iter().any(|a| a == "--drops");

    // Filter out flags from args for path parsing
    let filtered_args: Vec<&String> = args
        .iter()
        .filter(|a| *a != "--tree" && *a != "--drops")
        .collect();

    if filtered_args.len() != 3 {
        return Err(usage());
    }

    let binaries = match filtered_args[1].as_str() {
        "--bin" => parse_bin_args(&filtered_args)?,
        "--lib" => parse_lib_args(&filtered_args)?,
        _ => return Err(usage()),
    };

    Ok(Args { binaries, show_tree, show_drops })
}

fn usage() -> String {
    "Usage:\n  \
     jones [--tree] [--drops] --bin <path_to_binary>\n  \
     jones [--tree] [--drops] --lib <path_to_lib_object>\n\n\
     Options:\n  \
     --tree   Show full call tree instead of just crate code points\n  \
     --drops  Include panic paths from drop/cleanup operations"
        .to_string()
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
