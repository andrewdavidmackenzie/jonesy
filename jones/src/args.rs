use std::path::PathBuf;

/// parse the command line arguments into the three cases accepted:
/// 1) --example
///    Verify there is an example with that name and then find the compiled binary in ./target
///    depending on --debug or --release switch
/// 2) --examples
///    Generate a Vec of the paths to example binaries in ./target according to --debug or
///    --release switch
/// 3) --bin $path
///    Check that the binary file specified by the --bin option exists and is readable
pub(crate) fn parse_args(args: &[String]) -> Result<Vec<PathBuf>, String> {
    if args.len() != 3 {
        return Err(usage());
    }

    match args[1].as_str() {
        "--bin" => parse_bin_args(args),
        "--lib" => parse_lib_args(args),
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "Usage:\n  \
     jones --bin <path_to_binary>
     jones --lib <path_to_lib_object>"
        .to_string()
}

/// Parse --bin path_to_binary
fn parse_bin_args(args: &[String]) -> Result<Vec<PathBuf>, String> {
    if args.len() != 3 {
        return Err("--bin requires a path to a binary".to_string());
    }

    let binary_path = PathBuf::from(&args[2]);

    // Check that the file exists
    if !binary_path.exists() {
        return Err(format!("Binary not found at {:?}", binary_path));
    }

    // Check that the file is readable by attempting to open it
    std::fs::File::open(&binary_path)
        .map_err(|e| format!("Cannot read binary at {:?}: {}", binary_path, e))?;

    Ok(vec![binary_path])
}

/// Parse --bin path_to_library_object
fn parse_lib_args(args: &[String]) -> Result<Vec<PathBuf>, String> {
    if args.len() != 3 {
        return Err("--lib requires a path to a library object file".to_string());
    }

    let binary_path = PathBuf::from(&args[2]);

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
