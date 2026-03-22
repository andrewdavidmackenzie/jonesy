//! Benchmarks for jonesy analysis performance
//!
//! Run with: `cargo bench -p jonesy`
//!
//! Uses stable example binaries (debug builds with DWARF) as reference targets.
//! These don't change when optimizing jonesy, allowing meaningful comparisons.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;

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

/// Benchmark analysis on stable reference binaries
fn bench_analysis(c: &mut Criterion) {
    ensure_examples_built();

    let root = workspace_root();
    let mut group = c.benchmark_group("jonesy_analysis");

    // Stable reference binaries - debug builds with full DWARF info
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

criterion_group!(benches, bench_analysis);
criterion_main!(benches);
