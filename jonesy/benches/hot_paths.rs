//! Benchmarks for jonesy analysis performance
//!
//! Run with: `cargo bench -p jonesy`
//!
//! Uses stable example binaries (debug builds with DWARF) as reference targets.
//! These don't change when optimizing jonesy, allowing meaningful comparisons.
//!
//! Note: These benchmarks only run on macOS since jonesy analyzes Mach-O binaries.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;

// macOS-only imports for micro-benchmarks
#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "macos")]
use jonesy::analysis::{analyze_macho, PANIC_SYMBOL_PATTERNS};
#[cfg(target_os = "macos")]
use jonesy::args::OutputFormat;
#[cfg(target_os = "macos")]
use jonesy::call_tree::{CallTreeNode, build_call_tree_parallel, prune_call_tree};
#[cfg(target_os = "macos")]
use jonesy::config::Config;
#[cfg(target_os = "macos")]
use jonesy::sym::{CallGraph, SymbolIndex, SymbolTable, find_symbol_address, find_symbol_containing, read_symbols};
#[cfg(target_os = "macos")]
use dashmap::DashSet;
#[cfg(target_os = "macos")]
use goblin::mach::Mach::Binary;
#[cfg(target_os = "macos")]
use std::sync::Arc;

/// Get the workspace root directory
fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
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

/// Micro-benchmark: analyze_macho function
#[cfg(target_os = "macos")]
fn bench_analyze_macho(c: &mut Criterion) {
    ensure_examples_built();

    let root = workspace_root();
    let binary_path = root.join("target/debug/panic");

    if !binary_path.exists() {
        eprintln!("Skipping analyze_macho benchmark: panic binary not found");
        return;
    }

    let buffer = fs::read(&binary_path).expect("Failed to read binary");
    let config = Config::with_defaults();
    let output = OutputFormat::quiet();

    c.bench_function("analyze_macho_panic", |b| {
        b.iter(|| {
            let symbols = read_symbols(&buffer).expect("Failed to read symbols");
            match symbols {
                SymbolTable::MachO(Binary(ref macho)) => {
                    let result = analyze_macho(
                        macho,
                        &buffer,
                        &binary_path,
                        Some("examples/panic/src/"),
                        false,
                        &config,
                        &output,
                    );
                    black_box(result);
                }
                _ => {}
            }
        })
    });
}

/// Micro-benchmark: CallGraph::build_with_debug_info
#[cfg(target_os = "macos")]
fn bench_call_graph_build(c: &mut Criterion) {
    ensure_examples_built();

    let root = workspace_root();
    let binary_path = root.join("target/debug/panic");

    if !binary_path.exists() {
        eprintln!("Skipping call_graph benchmark: panic binary not found");
        return;
    }

    let buffer = fs::read(&binary_path).expect("Failed to read binary");
    let symbols = read_symbols(&buffer).expect("Failed to read symbols");

    if let SymbolTable::MachO(Binary(ref macho)) = symbols {
        let symbol_index = SymbolIndex::new(macho);

        c.bench_function("call_graph_build_panic", |b| {
            b.iter(|| {
                let graph = CallGraph::build_with_debug_info(
                    macho,
                    &buffer,
                    macho,
                    &buffer,
                    Some("examples/panic/src/"),
                    false,
                    symbol_index.as_ref(),
                );
                black_box(graph)
            })
        });
    }
}

/// Micro-benchmark: prune_call_tree
#[cfg(target_os = "macos")]
fn bench_prune_call_tree(c: &mut Criterion) {
    ensure_examples_built();

    let root = workspace_root();
    let binary_path = root.join("target/debug/panic");

    if !binary_path.exists() {
        eprintln!("Skipping prune_call_tree benchmark: panic binary not found");
        return;
    }

    let buffer = fs::read(&binary_path).expect("Failed to read binary");
    let symbols = read_symbols(&buffer).expect("Failed to read symbols");

    if let SymbolTable::MachO(Binary(ref macho)) = symbols {
        // Find the actual panic symbol address
        let mut panic_addr = 0u64;
        let mut panic_name = "rust_panic".to_string();
        for pattern in PANIC_SYMBOL_PATTERNS {
            if let Ok(Some((sym, _))) = find_symbol_containing(macho, pattern)
                && let Some(addr) = find_symbol_address(macho, &sym)
            {
                panic_addr = addr;
                panic_name = sym;
                break;
            }
        }

        if panic_addr == 0 {
            eprintln!("Skipping prune_call_tree benchmark: no panic symbol found");
            return;
        }

        let symbol_index = SymbolIndex::new(macho);
        let call_graph = CallGraph::build_with_debug_info(
            macho,
            &buffer,
            macho,
            &buffer,
            Some("examples/panic/src/"),
            false,
            symbol_index.as_ref(),
        )
        .expect("Failed to build call graph");

        // Build an initial call tree to prune
        let visited = Arc::new(DashSet::new());

        c.bench_function("prune_call_tree_panic", |b| {
            b.iter(|| {
                // Build fresh tree each iteration since prune modifies in place
                let mut root_node = CallTreeNode::new_root(panic_name.clone());
                visited.clear();
                root_node.callers = build_call_tree_parallel(&call_graph, panic_addr, &visited);
                prune_call_tree(&mut root_node, "examples/panic/src/", None);
                black_box(root_node)
            })
        });
    }
}

// Stub functions for non-macOS platforms
#[cfg(not(target_os = "macos"))]
fn bench_analyze_macho(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_call_graph_build(_c: &mut Criterion) {}
#[cfg(not(target_os = "macos"))]
fn bench_prune_call_tree(_c: &mut Criterion) {}

criterion_group!(
    benches,
    bench_e2e_analysis,
    bench_analyze_macho,
    bench_call_graph_build,
    bench_prune_call_tree,
);
criterion_main!(benches);
