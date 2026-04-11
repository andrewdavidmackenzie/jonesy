# Proposed Tests for Parallel .rlib Processing

## 1. Determinism Test (CRITICAL)

Verify that parallel processing produces identical results regardless of thread count:

```rust
#[test]
fn test_rlib_parallel_determinism() {
    setup();
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("rlib");

    // Run with 1 thread (sequential)
    let output_1_thread = run_jonesy_raw_output(
        &example_dir, 
        &["--no-hyperlinks", "--lib", "--max-threads", "1"]
    );
    
    // Run with 4 threads (parallel)
    let output_4_threads = run_jonesy_raw_output(
        &example_dir, 
        &["--no-hyperlinks", "--lib", "--max-threads", "4"]
    );

    // Parse both outputs
    let panics_1 = parse_jones_output(&output_1_thread);
    let panics_4 = parse_jones_output(&output_4_threads);

    // Should detect the same panic points
    assert_eq!(panics_1.len(), panics_4.len(), 
        "Thread count affects panic detection count");
    
    // Convert to sets for order-independent comparison
    let set_1: HashSet<_> = panics_1.into_iter().collect();
    let set_4: HashSet<_> = panics_4.into_iter().collect();
    
    assert_eq!(set_1, set_4, 
        "Parallel processing produces different results than sequential");
}
```

## 2. Large .rlib Correctness Test

Test with a larger .rlib (like meshcore-rs if available) to ensure parallel processing handles many .o files correctly:

```rust
#[test]
#[ignore] // Optional - only if external library is available
fn test_large_rlib_parallel_correctness() {
    setup();
    
    // This requires meshcore-rs to be built in a sibling directory
    let meshcore_rlib = PathBuf::from("../meshcore-rs/target/debug/libmeshcore_rs.rlib");
    
    if !meshcore_rlib.exists() {
        eprintln!("Skipping: meshcore-rs not available");
        return;
    }

    // Run analysis with different thread counts
    let results: Vec<_> = [1, 2, 4].iter().map(|&threads| {
        let output = Command::new(cargo_bin("jonesy"))
            .current_dir(workspace_root())
            .args(&["--lib", meshcore_rlib.to_str().unwrap()])
            .args(&["--max-threads", &threads.to_string()])
            .args(&["--no-hyperlinks", "--quiet"])
            .output()
            .expect("Failed to run jonesy");
        
        String::from_utf8_lossy(&output.stdout).to_string()
    }).collect();

    // All results should be identical
    for i in 1..results.len() {
        assert_eq!(results[0], results[i], 
            "Thread count {} produces different results", i + 1);
    }
}
```

## 3. Empty Archive Handling

Test edge case of archive with no user .o files:

```rust
#[test]
fn test_rlib_empty_after_filtering() {
    // This would require creating a test .rlib with only stdlib .o files
    // Skip for now unless we create a dedicated test fixture
}
```

## 4. Concurrent Safety Test (Optional)

Run the same analysis multiple times concurrently to ensure thread safety:

```rust
#[test]
fn test_rlib_concurrent_analysis() {
    use std::thread;
    
    setup();
    let workspace_root = find_workspace_root();
    let example_dir = workspace_root.join("examples").join("rlib");

    // Run the same analysis in parallel threads
    let handles: Vec<_> = (0..4).map(|_| {
        let dir = example_dir.clone();
        thread::spawn(move || {
            run_jonesy_raw_output(&dir, &["--no-hyperlinks", "--lib"])
        })
    }).collect();

    // All should succeed and produce the same results
    let results: Vec<_> = handles.into_iter()
        .map(|h| h.join().expect("Thread panicked"))
        .collect();

    let first = parse_jones_output(&results[0]);
    for result in &results[1..] {
        let parsed = parse_jones_output(result);
        let set_first: HashSet<_> = first.iter().collect();
        let set_parsed: HashSet<_> = parsed.iter().collect();
        assert_eq!(set_first, set_parsed, "Concurrent runs produce different results");
    }
}
```

## Recommendation

**Priority 1 (Must Have):**
- ✅ Test #1: Determinism Test - Verifies parallel and sequential produce identical results

**Priority 2 (Should Have):**
- Test #2: Large .rlib test (if meshcore-rs is available in CI)

**Priority 3 (Nice to Have):**
- Test #4: Concurrent safety test

## Why These Tests Matter

1. **Determinism Test**: The most important test. Ensures that our parallel implementation doesn't introduce non-determinism or race conditions that could cause different results on different runs.

2. **Large .rlib Test**: Stresses the parallel implementation with many .o files (96 in meshcore-rs case) to ensure scalability.

3. **Concurrent Safety**: Verifies that multiple jonesy instances can run simultaneously without interfering with each other.

## Current Test Coverage

The existing tests already verify:
- ✅ Correctness of .rlib analysis (test_rlib_example)
- ✅ Line precision (test_rlib_line_precision)
- ✅ Edge cases (todo!() detection, conditional panics)
- ✅ Inline allow comments (test_rlib_inline_allow)

What's missing is verification that **parallelization doesn't change the results**.
