use goblin::mach::Mach::{Binary, Fat};
use jonesy::analysis::{BinaryAnalysisResult, analyze_archive, analyze_binary_target};
use jonesy::sym::SymbolTable;

// Cross-platform imports
use jonesy::args::{Args, VERSION, WorkspaceMember, parse_args};
use jonesy::call_tree::{AnalysisResult, AnalysisSummary, CrateCodePoint};
use jonesy::cargo::{detect_library_type, find_project_root, get_project_name};
use jonesy::output::OutputFormat;
use jonesy::project_context::ProjectContext;
use std::collections::HashSet;
use std::path::Path;

use jonesy::config::Config;
use jonesy::lsp;
use jonesy::output::html::{generate_html_output, generate_workspace_html_output};
use jonesy::output::json::{
    WorkspaceMemberResult, WorkspaceResult, generate_json_output, generate_workspace_json_output,
};
use jonesy::output::text::generate_text_output;
use rayon::prelude::*;
use std::error::Error;
use std::fs;
use std::io;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();

    let parsed_args = parse_args(&args).unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        std::process::exit(255);
    });

    // Handle LSP mode
    if parsed_args.lsp_mode {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(lsp::run_lsp_server());
        return Ok(());
    }

    // Configure rayon thread pool with user-specified max threads
    rayon::ThreadPoolBuilder::new()
        .num_threads(parsed_args.max_threads)
        .build_global()
        .ok(); // Ignore error if pool already initialized

    if let Some(ref workspace_members) = parsed_args.workspace_members {
        analyze_workspace(workspace_members, &parsed_args)
    } else {
        analyze_package(&parsed_args)
    }
}

/// Merge code points from an analysis result into an accumulator, deduplicating by (file, line).
/// When a duplicate is found, causes are merged into the existing entry.
fn merge_code_points(
    result: &mut BinaryAnalysisResult,
    seen: &mut HashSet<(String, u32)>,
    accumulator: &mut Vec<CrateCodePoint>,
) {
    for point in result.code_points.drain(..) {
        let key = (point.file.clone(), point.line);
        if seen.insert(key) {
            accumulator.push(point);
        } else if let Some(existing) = accumulator
            .iter_mut()
            .find(|p| p.file == point.file && p.line == point.line)
        {
            existing.causes.extend(point.causes);
        }
    }
}

/// Analyze a single package (non-workspace) with one or more binary targets.
fn analyze_package(parsed_args: &Args) -> Result<(), Box<dyn Error>> {
    let mut total_summary = AnalysisSummary::default();
    let mut all_code_points: Vec<CrateCodePoint> = Vec::new();
    let mut seen_code_points: HashSet<(String, u32)> = HashSet::new();
    let mut project_name: Option<String> = None;
    let mut project_root_path: Option<String> = None;

    // Build ProjectContext once from the first binary's project root
    let first_binary = parsed_args.binaries[0]
        .canonicalize()
        .unwrap_or_else(|_| parsed_args.binaries[0].clone());
    let project_root = find_project_root(&first_binary)?;
    let project_context = ProjectContext::from_project_root(&project_root)?;
    let config = Config::load_for_project(&project_root, parsed_args.config_path.as_deref())
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            std::process::exit(255);
        });

    for binary_path in &parsed_args.binaries {
        let binary_path = binary_path
            .canonicalize()
            .unwrap_or_else(|_| binary_path.clone());
        if parsed_args.output.show_progress() {
            println!("Processing {}", binary_path.display());
        }

        // Check if this is a library and detect its type
        let is_dylib = binary_path
            .extension()
            .is_some_and(|ext| ext == "dylib" || ext == "so");
        if parsed_args.output.show_progress()
            && is_dylib
            && let Some(lib_type) = detect_library_type(&binary_path)
        {
            println!("Library type: {}", lib_type);
            if lib_type == "dylib" {
                println!(
                    "Note: Rust dylib includes the standard library runtime. \
                     Analysis may take longer."
                );
            }
        }

        let binary_buffer = fs::read(&binary_path)?;
        let symbols = SymbolTable::from(&binary_buffer)?;

        // Capture project info from the first binary processed
        if project_name.is_none() {
            // Prefer project name from Cargo manifest, fall back to the binary filename
            project_name = get_project_name(&project_root).or_else(|| {
                binary_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
            });
            project_root_path = Some(project_root.to_string_lossy().to_string());
        }

        if let Some(mut result) = analyze_binary(
            &symbols,
            &binary_buffer,
            &binary_path,
            parsed_args.show_timings,
            &config,
            &parsed_args.output,
            &project_context,
            &project_root,
        )? {
            total_summary.add(&result.summary);
            merge_code_points(&mut result, &mut seen_code_points, &mut all_code_points);
        }

        if parsed_args.output.show_progress() {
            println!();
        }
    }

    // Create the unified analysis result
    let result = AnalysisResult::new(
        project_name.unwrap_or_else(|| "unknown".to_string()),
        project_root_path.unwrap_or_else(|| ".".to_string()),
        all_code_points,
    );

    // Output results based on format
    let tree = parsed_args.output.show_tree();
    let summary_only = parsed_args.output.is_summary_only();

    if parsed_args.output.is_json() {
        match generate_json_output(&result, tree, summary_only) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("Error serializing JSON: {}", e);
                std::process::exit(255);
            }
        }
    } else if parsed_args.output.is_html() {
        let html = generate_html_output(&result, tree, summary_only);
        println!("{}", html);
    } else {
        let no_hyperlinks = !parsed_args.output.use_hyperlinks();
        generate_text_output(&result, tree, summary_only, no_hyperlinks);
    }

    // Exit with the number of panic points found (0 = passed, >0 = found panics)
    // Note: Unix exit codes are 8-bit (0-255), the values above wrap around
    std::process::exit(result.panic_points() as i32);
}

/// Analyze a binary or archive based on its SymbolTable type.
/// Returns the analysis result, or None for unsupported formats (fat binaries).
#[allow(clippy::too_many_arguments)]
fn analyze_binary(
    symbols: &SymbolTable,
    buffer: &[u8],
    binary_path: &Path,
    show_timings: bool,
    config: &Config,
    output: &OutputFormat,
    project_context: &ProjectContext,
    workspace_root: &Path,
) -> Result<Option<BinaryAnalysisResult>, String> {
    match symbols {
        SymbolTable::MachO(Binary(_)) => analyze_binary_target(
            symbols,
            buffer,
            binary_path,
            show_timings,
            config,
            output,
            project_context,
        )
        .map(Some),
        SymbolTable::MachO(Fat(_)) => Ok(None),
        SymbolTable::Elf(_) => analyze_binary_target(
            symbols,
            buffer,
            binary_path,
            show_timings,
            config,
            output,
            project_context,
        )
        .map(Some),
        SymbolTable::Archive(archive) => analyze_archive(
            archive,
            buffer,
            show_timings,
            config,
            output,
            project_context,
            workspace_root,
        )
        .map(Some),
    }
}

/// Analyze a workspace with multiple member crates.
/// Produces per-crate reports and an aggregate workspace summary.
fn analyze_workspace(members: &[WorkspaceMember], args: &Args) -> Result<(), Box<dyn Error>> {
    let workspace_root = std::env::current_dir()?;

    if args.output.show_progress() {
        println!(
            "Analyzing workspace with {} member crate(s)...\n",
            members.len()
        );
    }

    let mut workspace_summary = AnalysisSummary::default();
    let mut member_results: Vec<WorkspaceMemberResult> = Vec::new();

    // Build ProjectContext once for the workspace root
    let project_context = ProjectContext::from_project_root(&workspace_root)?;

    for member in members {
        if args.output.show_progress() {
            println!("=== {} ===", member.name);
        }

        // Load configuration once for this member crate (same for all binaries)
        // If the user explicitly provided --config, fail fast on errors
        let config = match Config::load_for_project(&member.path, args.config_path.as_deref()) {
            Ok(c) => c,
            Err(e) if args.config_path.is_some() => {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Failed to load config for {}: {}", member.name, e),
                )));
            }
            Err(e) => {
                eprintln!("Warning: Failed to load config for {}: {}", member.name, e);
                Config::with_defaults()
            }
        };

        // Analyze binaries in parallel for better performance
        if args.output.show_progress() && member.binaries.len() > 1 {
            println!(
                "Analyzing {} binaries in parallel...",
                member.binaries.len()
            );
        }

        // Parallel analysis of all binaries in this member
        let binary_results: Vec<(PathBuf, BinaryAnalysisResult)> = member
            .binaries
            .par_iter()
            .filter_map(|binary_path| {
                let binary_path = binary_path
                    .canonicalize()
                    .unwrap_or_else(|_| binary_path.clone());
                let binary_buffer = fs::read(&binary_path).ok()?;
                let symbols = SymbolTable::from(&binary_buffer).ok()?;

                let result = analyze_binary(
                    &symbols,
                    &binary_buffer,
                    &binary_path,
                    args.show_timings,
                    &config,
                    &args.output,
                    &project_context,
                    &workspace_root,
                )
                .ok()??;
                Some((binary_path, result))
            })
            .collect();

        // Merge results sequentially
        let mut member_summary = AnalysisSummary::default();
        let mut member_code_points: Vec<CrateCodePoint> = Vec::new();
        let mut seen_code_points: HashSet<(String, u32)> = HashSet::new();

        for (binary_path, mut result) in binary_results {
            if args.output.show_progress() {
                println!("Processed {}", binary_path.display());
            }
            member_summary.add(&result.summary);
            merge_code_points(&mut result, &mut seen_code_points, &mut member_code_points);
        }

        // For text output, print immediately; for JSON/HTML, collect for later
        if args.output.is_text() {
            if !args.output.is_summary_only() {
                let member_result = AnalysisResult::new(
                    member.name.clone(),
                    workspace_root.to_string_lossy().to_string(),
                    member_code_points.clone(),
                );
                let no_hyperlinks = !args.output.use_hyperlinks();
                generate_text_output(
                    &member_result,
                    args.output.show_tree(),
                    false,
                    no_hyperlinks,
                );
            } else if args.output.show_progress() {
                println!(
                    "Panic points: {} in {} file(s)\n",
                    member_summary.panic_points(),
                    member_summary.files_affected()
                );
            }
        }

        // Store member results for workspace output
        member_results.push(WorkspaceMemberResult {
            name: member.name.clone(),
            path: member.path.to_string_lossy().to_string(),
            summary: member_summary.clone(),
            code_points: member_code_points,
        });
        workspace_summary.add(&member_summary);
    }

    // Build workspace result
    let workspace_result = WorkspaceResult {
        root: workspace_root.to_string_lossy().to_string(),
        members: member_results,
        total_summary: workspace_summary.clone(),
    };

    let tree = args.output.show_tree();
    let summary_only = args.output.is_summary_only();

    // Output based on format
    if args.output.is_json() {
        match generate_workspace_json_output(&workspace_result, tree, summary_only) {
            Ok(json) => println!("{}", json),
            Err(e) => {
                eprintln!("Error serializing JSON: {}", e);
                std::process::exit(255);
            }
        }
    } else if args.output.is_html() {
        let html = generate_workspace_html_output(&workspace_result, tree, summary_only);
        println!("{}", html);
    } else {
        // Text output: print workspace summary
        println!("=== Workspace Summary (jonesy v{}) ===", VERSION);
        println!("  Root: {}", workspace_root.display());
        println!("  Members analyzed: {}", workspace_result.members.len());
        for member in &workspace_result.members {
            println!(
                "    {}: {} panic point(s) in {} file(s)",
                member.name,
                member.summary.panic_points(),
                member.summary.files_affected()
            );
        }
        println!(
            "  Total panic points: {} across {} crate(s)",
            workspace_summary.panic_points(),
            members.len()
        );
    }

    // Exit with the number of panic points found
    std::process::exit(workspace_summary.panic_points() as i32);
}
