//! Benchmarks for jonesy analysis performance
//!
//! Run with: `cargo bench -p jonesy`
//!
//! Uses jonesy debug binary (33MB) as the primary target for significant measurements.
//! Also includes small example binaries for quick regression checks.
//!
//! Note: These benchmarks only run on macOS since jonesy analyzes Mach-O binaries.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;

// macOS-only imports for micro-benchmarks
#[cfg(target_os = "macos")]
use dashmap::DashSet;
#[cfg(target_os = "macos")]
use goblin::mach::Mach::Binary;
#[cfg(target_os = "macos")]
use jonesy::analysis::analyze_macho;
#[cfg(target_os = "macos")]
use jonesy::args::OutputFormat;
#[cfg(target_os = "macos")]
use jonesy::call_tree::{
    CallTreeNode, CodePointMap, build_call_tree_parallel_filtered,
    build_call_tree_sequential_filtered, build_shallow_callers_filtered,
    collect_crate_relationships, filter_allowed_causes,
};
#[cfg(target_os = "macos")]
use jonesy::config::Config;
#[cfg(target_os = "macos")]
use jonesy::heuristics::PANIC_SYMBOL_PATTERNS;
#[cfg(target_os = "macos")]
use jonesy::heuristics::detect_panic_cause;
#[cfg(target_os = "macos")]
use jonesy::inline_allows::check_inline_allow;
#[cfg(target_os = "macos")]
use jonesy::sym::{
    CallGraph, FunctionIndex, ProjectContext, SymbolIndex, SymbolTable, get_functions_from_dwarf,
    load_debug_info,
};
#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "macos")]
use std::sync::Arc;

/// Get the workspace root directory
fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Ensure jonesy debug binary is built
#[cfg(target_os = "macos")]
fn ensure_jonesy_debug_built() {
    let status = Command::new("cargo")
        .args(["build", "-p", "jonesy"])
        .current_dir(workspace_root())
        .status()
        .expect("Failed to build jonesy");

    if !status.success() {
        panic!("Failed to build jonesy debug binary");
    }
}

/// Ensure example binaries are built in debug mode (with DWARF info)
fn ensure_examples_built() {
    let status = Command::new("cargo")
        .args(["build", "-p", "panic", "-p", "perfect", "-p", "inlined"])
        .current_dir(workspace_root())
        .status()
        .expect("Failed to build examples");

    if !status.success() {
        panic!("Failed to build example binaries");
    }
}

/// End-to-end benchmark: full analysis via subprocess
fn bench_e2e_analysis(c: &mut Criterion) {
    ensure_examples_built();

    let root = workspace_root();
    let mut group = c.benchmark_group("e2e_analysis");

    let binaries = [
        ("panic", root.join("target/debug/panic")),
        ("perfect", root.join("target/debug/perfect")),
        ("inlined", root.join("target/debug/inlined")),
    ];

    for (name, binary_path) in binaries {
        if !binary_path.exists() {
            eprintln!("Skipping {}: binary not found at {:?}", name, binary_path);
            continue;
        }

        let size_kb = binary_path.metadata().map(|m| m.len() / 1024).unwrap_or(0);
        let binary_path_str = binary_path.to_string_lossy().to_string();

        group.bench_with_input(
            BenchmarkId::new(name, format!("{}KB", size_kb)),
            &binary_path_str,
            |b, path| {
                b.iter(|| {
                    let output = Command::new(env!("CARGO_BIN_EXE_jonesy"))
                        .args(["--bin", path, "--quiet"])
                        .output()
                        .expect("Failed to run jonesy");
                    black_box(output)
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// Hot Function #1: FunctionIndex::find_function_name (106 samples)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_find_function_name(c: &mut Criterion) {
    ensure_jonesy_debug_built();

    let root = workspace_root();
    let binary_path = root.join("target/debug/jonesy");

    if !binary_path.exists() {
        eprintln!("Skipping find_function_name benchmark: jonesy binary not found");
        return;
    }

    let buffer = fs::read(&binary_path).expect("Failed to read binary");
    let symbols = SymbolTable::from(&buffer).expect("Failed to read symbols");

    if let SymbolTable::MachO(Binary(ref macho)) = symbols {
        // Build function index from DWARF
        let (functions, inlined, strings) = match get_functions_from_dwarf(macho, &buffer) {
            Ok(result) => result,
            Err(_) => {
                eprintln!("Skipping find_function_name benchmark: no DWARF info");
                return;
            }
        };
        let func_index = FunctionIndex::new_with_inlined(functions, inlined, strings);

        // Get some sample addresses to look up
        let sample_addrs: Vec<u64> = macho
            .symbols
            .as_ref()
            .map(|s| {
                s.iter()
                    .flatten()
                    .filter_map(|(_, nlist)| {
                        if nlist.n_value != 0 && !nlist.is_undefined() {
                            Some(nlist.n_value)
                        } else {
                            None
                        }
                    })
                    .take(1000)
                    .collect()
            })
            .unwrap_or_default();

        c.bench_function("find_function_name_jonesy", |b| {
            b.iter(|| {
                for addr in &sample_addrs {
                    black_box(func_index.find_function_name(*addr));
                }
            })
        });
    }
}

// ============================================================================
// Hot Function #2: prune_call_tree - REMOVED (replaced by filtered tree building in PR #152)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_prune_call_tree(_c: &mut Criterion) {
    // prune_call_tree was removed in PR #152 - replaced by build_call_tree_parallel_filtered
    // which filters during construction instead of pruning after
}

// ============================================================================
// Hot Function #3: build_call_tree_sequential (92 samples)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_build_call_tree_sequential_filtered(c: &mut Criterion) {
    ensure_jonesy_debug_built();

    let root = workspace_root();
    let binary_path = root.join("target/debug/jonesy");

    if !binary_path.exists() {
        eprintln!("Skipping build_call_tree_sequential benchmark: jonesy binary not found");
        return;
    }

    let buffer = fs::read(&binary_path).expect("Failed to read binary");
    let symbols = SymbolTable::from(&buffer).expect("Failed to read symbols");

    if let SymbolTable::MachO(Binary(ref macho)) = symbols {
        let mut panic_addr = 0u64;
        for pattern in PANIC_SYMBOL_PATTERNS {
            if let Ok(Some((sym, _))) = symbols.find_symbol_containing(pattern)
                && let Some(addr) = symbols.find_symbol_address(&sym)
            {
                panic_addr = addr;
                break;
            }
        }

        if panic_addr == 0 {
            eprintln!("Skipping build_call_tree_sequential benchmark: no panic symbol found");
            return;
        }

        let project_context = ProjectContext::from_project_root(&root.join("jonesy"))
            .expect("Failed to create project context");
        let symbol_index = SymbolIndex::new(macho);
        let call_graph = CallGraph::build_with_debug_info(
            macho,
            &buffer,
            macho,
            &buffer,
            Some("jonesy/src/"),
            false,
            symbol_index.as_ref(),
            &project_context,
        )
        .expect("Failed to build call graph");

        let visited = Arc::new(DashSet::new());

        c.bench_function("build_call_tree_sequential_jonesy", |b| {
            b.iter(|| {
                visited.clear();
                let tree = build_call_tree_sequential_filtered(
                    &call_graph,
                    panic_addr,
                    &visited,
                    &project_context,
                );
                black_box(tree)
            })
        });
    }
}

// ============================================================================
// Hot Function #4: is_crate_source (76 samples)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_is_crate_source(c: &mut Criterion) {
    ensure_jonesy_debug_built();

    let root = workspace_root();
    let project_context = ProjectContext::from_project_root(&root.join("jonesy"))
        .expect("Failed to create project context");

    let test_paths = vec![
        "jonesy/src/main.rs",
        "jonesy/src/analysis.rs",
        "jonesy/src/call_tree.rs",
        "/rustc/xyz/library/core/src/option.rs",
        "/Users/test/.cargo/registry/src/crates.io/serde-1.0.0/src/lib.rs",
        "src/lib.rs",
        "examples/panic/src/main.rs",
    ];

    c.bench_function("is_crate_source", |b| {
        b.iter(|| {
            for path in &test_paths {
                for _ in 0..100 {
                    black_box(project_context.is_crate_source(path));
                }
            }
        })
    });
}

// ============================================================================
// Hot Function #5: collect_crate_relationships (69 samples)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_collect_crate_relationships(c: &mut Criterion) {
    ensure_jonesy_debug_built();

    let root = workspace_root();
    let binary_path = root.join("target/debug/jonesy");

    if !binary_path.exists() {
        eprintln!("Skipping collect_crate_relationships benchmark: jonesy binary not found");
        return;
    }

    let buffer = fs::read(&binary_path).expect("Failed to read binary");
    let symbols = SymbolTable::from(&buffer).expect("Failed to read symbols");

    if let SymbolTable::MachO(Binary(ref macho)) = symbols {
        let mut panic_addr = 0u64;
        let mut panic_name = "rust_panic".to_string();
        for pattern in PANIC_SYMBOL_PATTERNS {
            if let Ok(Some((sym, _))) = symbols.find_symbol_containing(pattern)
                && let Some(addr) = symbols.find_symbol_address(&sym)
            {
                panic_addr = addr;
                panic_name = sym;
                break;
            }
        }

        if panic_addr == 0 {
            return;
        }

        let project_context = ProjectContext::from_project_root(&root.join("jonesy"))
            .expect("Failed to create project context");
        let symbol_index = SymbolIndex::new(macho);
        let call_graph = CallGraph::build_with_debug_info(
            macho,
            &buffer,
            macho,
            &buffer,
            Some("jonesy/src/"),
            false,
            symbol_index.as_ref(),
            &project_context,
        )
        .expect("Failed to build call graph");

        let visited = Arc::new(DashSet::new());
        let mut root_node = CallTreeNode::new_root(panic_name);
        root_node.callers =
            build_call_tree_parallel_filtered(&call_graph, panic_addr, &visited, &project_context);

        c.bench_function("collect_crate_relationships_jonesy", |b| {
            b.iter(|| {
                let mut points: CodePointMap = std::collections::HashMap::new();
                collect_crate_relationships(
                    &root_node,
                    &mut points,
                    None,
                    None,
                    None,
                    &project_context,
                );
                black_box(points)
            })
        });
    }
}

// ============================================================================
// Hot Function #6: check_inline_allow (57 samples)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_check_inline_allow(c: &mut Criterion) {
    // Create a test file with some inline allows
    let test_file = "/tmp/jonesy_bench_test.rs";
    fs::write(
        test_file,
        r#"
fn main() {
    let x = Some(1);
    x.unwrap(); // jonesy:allow(unwrap)

    let y: Result<i32, &str> = Ok(1);
    y.expect("should work"); // jonesy:allow(expect)

    // Some code without allows
    let z = vec![1, 2, 3];
    z[0];
}
"#,
    )
    .expect("Failed to write test file");

    c.bench_function("check_inline_allow", |b| {
        b.iter(|| {
            // Check various lines
            for line in 1..12 {
                for cause in &["unwrap", "expect", "panic", "index"] {
                    black_box(check_inline_allow(test_file, line, cause, None));
                }
            }
        })
    });

    fs::remove_file(test_file).ok();
}

// ============================================================================
// Hot Function #7: detect_panic_cause (38 samples)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_detect_panic_cause(c: &mut Criterion) {
    let test_funcs = vec![
        "core::option::Option<T>::unwrap",
        "core::result::Result<T,E>::expect",
        "std::panicking::begin_panic",
        "alloc::vec::Vec<T>::index",
        "core::panicking::panic_bounds_check",
        "myapp::process_data",
        "core::slice::index::slice_index_fail",
    ];

    c.bench_function("detect_panic_cause", |b| {
        b.iter(|| {
            for _ in 0..100 {
                for func in &test_funcs {
                    black_box(detect_panic_cause(func));
                }
            }
        })
    });
}

// ============================================================================
// Hot Function #8: CallGraph::build_with_debug_info (32 samples)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_call_graph_build(c: &mut Criterion) {
    ensure_jonesy_debug_built();

    let root = workspace_root();
    let binary_path = root.join("target/debug/jonesy");

    if !binary_path.exists() {
        eprintln!("Skipping call_graph benchmark: jonesy binary not found");
        return;
    }

    let buffer = fs::read(&binary_path).expect("Failed to read binary");
    let symbols = SymbolTable::from(&buffer).expect("Failed to read symbols");

    if let SymbolTable::MachO(Binary(ref macho)) = symbols {
        let project_context = ProjectContext::from_project_root(&root.join("jonesy"))
            .expect("Failed to create project context");
        let symbol_index = SymbolIndex::new(macho);

        c.bench_function("call_graph_build_jonesy", |b| {
            b.iter(|| {
                let graph = CallGraph::build_with_debug_info(
                    macho,
                    &buffer,
                    macho,
                    &buffer,
                    Some("jonesy/src/"),
                    false,
                    symbol_index.as_ref(),
                    &project_context,
                );
                black_box(graph)
            })
        });
    }
}

// ============================================================================
// Hot Function #9: filter_allowed_causes (29 samples)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_filter_allowed_causes(c: &mut Criterion) {
    use jonesy::call_tree::CrateCodePoint;
    use jonesy::panic_cause::PanicCause;
    use std::collections::HashSet;

    let config = Config::with_defaults();

    // Create sample code points
    let sample_points: Vec<CrateCodePoint> = (0..100)
        .map(|i| {
            let mut causes = HashSet::new();
            causes.insert(PanicCause::Unwrap);
            if i % 2 == 0 {
                causes.insert(PanicCause::Expect);
            }
            CrateCodePoint {
                name: format!("function_{}", i),
                file: format!("src/file_{}.rs", i % 10),
                line: (i * 10 + 1) as u32,
                column: Some(5),
                causes,
                children: Vec::new(),
                is_direct_panic: i % 3 == 0,
                called_function: None,
            }
        })
        .collect();

    c.bench_function("filter_allowed_causes", |b| {
        b.iter(|| {
            let mut points = sample_points.clone();
            filter_allowed_causes(&mut points, &config, None);
            black_box(points)
        })
    });
}

// ============================================================================
// Hot Function #10: build_shallow_callers (15 samples)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_build_shallow_callers_filtered(c: &mut Criterion) {
    ensure_jonesy_debug_built();

    let root = workspace_root();
    let binary_path = root.join("target/debug/jonesy");

    if !binary_path.exists() {
        eprintln!("Skipping build_shallow_callers benchmark: jonesy binary not found");
        return;
    }

    let buffer = fs::read(&binary_path).expect("Failed to read binary");
    let symbols = SymbolTable::from(&buffer).expect("Failed to read symbols");

    if let SymbolTable::MachO(Binary(ref macho)) = symbols {
        let mut panic_addr = 0u64;
        for pattern in PANIC_SYMBOL_PATTERNS {
            if let Ok(Some((sym, _))) = symbols.find_symbol_containing(pattern)
                && let Some(addr) = symbols.find_symbol_address(&sym)
            {
                panic_addr = addr;
                break;
            }
        }

        if panic_addr == 0 {
            return;
        }

        let project_context = ProjectContext::from_project_root(&root.join("jonesy"))
            .expect("Failed to create project context");
        let symbol_index = SymbolIndex::new(macho);
        let call_graph = CallGraph::build_with_debug_info(
            macho,
            &buffer,
            macho,
            &buffer,
            Some("jonesy/src/"),
            false,
            symbol_index.as_ref(),
            &project_context,
        )
        .expect("Failed to build call graph");

        c.bench_function("build_shallow_callers_jonesy", |b| {
            b.iter(|| {
                let callers =
                    build_shallow_callers_filtered(&call_graph, panic_addr, &project_context);
                black_box(callers)
            })
        });
    }
}

// ============================================================================
// Hot Function #13: SymbolIndex::new (6 samples)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_symbol_index_new(c: &mut Criterion) {
    ensure_jonesy_debug_built();

    let root = workspace_root();
    let binary_path = root.join("target/debug/jonesy");

    if !binary_path.exists() {
        eprintln!("Skipping SymbolIndex::new benchmark: jonesy binary not found");
        return;
    }

    let buffer = fs::read(&binary_path).expect("Failed to read binary");
    let symbols = SymbolTable::from(&buffer).expect("Failed to read symbols");

    if let SymbolTable::MachO(Binary(ref macho)) = symbols {
        c.bench_function("symbol_index_new_jonesy", |b| {
            b.iter(|| {
                let index = SymbolIndex::new(macho);
                black_box(index)
            })
        });
    }
}

// ============================================================================
// Hot Function #14: get_functions_from_dwarf (4 samples)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_get_functions_from_dwarf(c: &mut Criterion) {
    ensure_jonesy_debug_built();

    let root = workspace_root();
    let binary_path = root.join("target/debug/jonesy");

    if !binary_path.exists() {
        eprintln!("Skipping get_functions_from_dwarf benchmark: jonesy binary not found");
        return;
    }

    let buffer = fs::read(&binary_path).expect("Failed to read binary");
    let symbols = SymbolTable::from(&buffer).expect("Failed to read symbols");

    if let SymbolTable::MachO(Binary(ref macho)) = symbols {
        c.bench_function("get_functions_from_dwarf_jonesy", |b| {
            b.iter(|| {
                let funcs = get_functions_from_dwarf(macho, &buffer);
                black_box(funcs)
            })
        });
    }
}

// ============================================================================
// Hot Function #19: load_debug_info (2 samples)
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_load_debug_info(c: &mut Criterion) {
    ensure_jonesy_debug_built();

    let root = workspace_root();
    let binary_path = root.join("target/debug/jonesy");

    if !binary_path.exists() {
        eprintln!("Skipping load_debug_info benchmark: jonesy binary not found");
        return;
    }

    let buffer = fs::read(&binary_path).expect("Failed to read binary");
    let symbols = SymbolTable::from(&buffer).expect("Failed to read symbols");

    if let SymbolTable::MachO(Binary(ref macho)) = symbols {
        c.bench_function("load_debug_info_jonesy", |b| {
            b.iter(|| {
                let info = load_debug_info(macho, &binary_path, true);
                black_box(info)
            })
        });
    }
}

// ============================================================================
// Hot Function #12: analyze_macho (9 samples) - full pipeline
// ============================================================================
#[cfg(target_os = "macos")]
fn bench_analyze_macho(c: &mut Criterion) {
    ensure_jonesy_debug_built();

    let root = workspace_root();
    let binary_path = root.join("target/debug/jonesy");

    if !binary_path.exists() {
        eprintln!("Skipping analyze_macho benchmark: jonesy binary not found");
        return;
    }

    let buffer = fs::read(&binary_path).expect("Failed to read binary");
    let config = Config::with_defaults();
    let output = OutputFormat::quiet();

    c.bench_function("analyze_macho_jonesy", |b| {
        b.iter(|| {
            let symbols = SymbolTable::from(&buffer).expect("Failed to read symbols");
            if let SymbolTable::MachO(Binary(ref macho)) = symbols {
                let result = analyze_macho(
                    macho,
                    &buffer,
                    &binary_path,
                    Some("jonesy/src/"),
                    false,
                    &config,
                    &output,
                );
                black_box(result.ok());
            }
        })
    });
}

// Stub functions for non-macOS platforms
#[cfg(not(target_os = "macos"))]
fn bench_find_function_name(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_prune_call_tree(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_build_call_tree_sequential_filtered(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_is_crate_source(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_collect_crate_relationships(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_check_inline_allow(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_detect_panic_cause(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_call_graph_build(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_filter_allowed_causes(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_build_shallow_callers_filtered(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_symbol_index_new(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_get_functions_from_dwarf(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_load_debug_info(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_analyze_macho(_c: &mut Criterion) {}

criterion_group!(
    benches,
    // End-to-end (small binaries for quick regression)
    bench_e2e_analysis,
    // Top 20 hot functions (using jonesy 33MB binary)
    bench_find_function_name,                  // #1: 106 samples
    bench_prune_call_tree,                     // #2: REMOVED (PR #152)
    bench_build_call_tree_sequential_filtered, // #3: 92 samples
    bench_is_crate_source,                     // #4: 76 samples
    bench_collect_crate_relationships,         // #5: 69 samples
    bench_check_inline_allow,                  // #6: 57 samples
    bench_detect_panic_cause,                  // #7: 38 samples
    bench_call_graph_build,                    // #8: 32 samples
    bench_filter_allowed_causes,               // #9: 29 samples
    bench_build_shallow_callers_filtered,      // #10: 15 samples
    bench_symbol_index_new,                    // #13: 6 samples
    bench_get_functions_from_dwarf,            // #14: 4 samples
    bench_load_debug_info,                     // #19: 2 samples
    bench_analyze_macho,                       // #12: 9 samples (full pipeline)
);
criterion_main!(benches);
