//! Benchmarks for jonesy hot path functions
//!
//! These benchmarks measure the performance of the most time-consuming functions
//! identified through profiling. Run with: `cargo bench`

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;

/// Ensure the panic example binary is built before benchmarking
fn ensure_example_built() -> std::path::PathBuf {
    let output = Command::new("cargo")
        .args(["build", "--package", "panic"])
        .output()
        .expect("Failed to build panic example");

    if !output.status.success() {
        panic!(
            "Failed to build panic example: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Return path to the built binary
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/debug/panic")
}

/// Benchmark the full analysis pipeline on a small binary
fn bench_full_analysis(c: &mut Criterion) {
    let binary_path = ensure_example_built();
    let binary_path_str = binary_path.to_string_lossy();

    c.bench_function("full_analysis_panic_example", |b| {
        b.iter(|| {
            let output = Command::new(env!("CARGO_BIN_EXE_jonesy"))
                .args(["--bin", &binary_path_str, "--quiet"])
                .output()
                .expect("Failed to run jonesy");
            black_box(output)
        })
    });
}

/// Benchmark with different binary sizes by running on workspace examples
fn bench_analysis_scaling(c: &mut Criterion) {
    // Build all example binaries
    let _ = Command::new("cargo")
        .args(["build", "--examples"])
        .output();

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    let mut group = c.benchmark_group("analysis_scaling");

    // Test different example binaries if they exist
    let examples = [
        ("panic", "examples/panic/target/debug/panic"),
        ("perfect", "examples/perfect/target/debug/perfect"),
        ("inlined", "examples/inlined/target/debug/inlined"),
    ];

    for (name, rel_path) in examples {
        let binary_path = manifest_dir.parent().unwrap().join(rel_path);
        if binary_path.exists() {
            let binary_path_str = binary_path.to_string_lossy().to_string();
            group.bench_with_input(BenchmarkId::new("binary", name), &binary_path_str, |b, path| {
                b.iter(|| {
                    let output = Command::new(env!("CARGO_BIN_EXE_jonesy"))
                        .args(["--bin", path, "--quiet"])
                        .output()
                        .expect("Failed to run jonesy");
                    black_box(output)
                })
            });
        }
    }

    group.finish();
}

criterion_group!(benches, bench_full_analysis, bench_analysis_scaling);
criterion_main!(benches);
