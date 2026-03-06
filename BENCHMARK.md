# Jones Performance Benchmarks

## Baseline Timing (With CallGraph Pre-computation, Single-threaded)

Date: 2026-03-06
System: macOS ARM64 (Apple Silicon)
Build: Release

| Example      | Time (seconds) |
|--------------|----------------|
| panic        | 3.40           |
| array_access | 2.60           |
| oom          | 3.38           |
| perfect      | 3.50           |
| dylib        | 23.19          |
| cdylib       | 2.69           |

**Total**: ~38.76 seconds

## Parallel Implementation (Multi-threaded CallGraph + Tree Building)

Date: 2026-03-06
System: macOS ARM64 (Apple Silicon) with 10 cores
Build: Release

| Example      | Time (seconds) | Speedup  |
|--------------|----------------|----------|
| panic        | 0.46           | 7.4x     |
| array_access | 0.37           | 7.0x     |
| oom          | 0.43           | 7.9x     |
| perfect      | 0.46           | 7.6x     |
| dylib        | 2.78           | **8.3x** |
| cdylib       | 0.35           | 7.7x     |

**Total**: ~4.85 seconds (was 38.76 s - **8x overall speedup**)

## Implementation Details

1. **CallGraph Pre-computation**: Scans all instructions once upfront, enabling O(1) lookups
2. **Parallel Instruction Processing**: Uses rayon to process `bl` instructions in parallel
3. **Parallel Tree Building**: Top-level callers explored in parallel, sequential within branches
4. **Thread-safe Data Structures**: DashMap for concurrent CallGraph building, DashSet for visited tracking

## Parallel Disassembly (ARM64 only)

Date: 2026-03-06
System: macOS ARM64 (Apple Silicon) with 10 cores
Build: Release

In addition to parallel instruction processing, ARM64 binaries can benefit from parallel disassembly
by dividing the __TEXT section into chunks and disassembling each chunk on a separate thread.

| Example      | Without (s) | With Parallel Disasm (s) | Improvement |
|--------------|-------------|--------------------------|-------------|
| panic        | 0.46        | 0.44                     | ~4%         |
| array_access | 0.37        | 0.37                     | ~0%         |
| oom          | 0.43        | 0.44                     | ~0%         |
| perfect      | 0.46        | 0.45                     | ~2%         |
| **dylib**    | **2.78**    | **2.56**                 | **~8%**     |
| cdylib       | 0.35        | 0.37                     | ~0%         |

### Analysis

The parallel disassembly provides **modest improvement (~8%) on the largest binary (dylib)** but
negligible impact on smaller binaries. This is because:

1. **Disassembly is not the main bottleneck**, Capstone is already quite fast
2. **Overhead vs. benefit** - For smaller binaries, chunking/thread overhead exceeds the benefit
3. **Function lookup dominates** - Looking up containing functions (symbol table plus DWARF) is the
   real bottleneck, and that's already parallelized

### Implementation

- ARM64 has fixed 4-byte instructions, allowing chunking at any aligned boundary
- Each thread creates its own Capstone instance (not thread-safe)
- Minimum chunk size of 64KB prevents overhead on small sections
- Conditional compilation: `#[cfg(target_arch = "aarch64")]`

## Thread Scaling (--max-threads sweep)

Tested with the `dylib` example (largest binary) to show scaling behavior:

| Threads | Time (s) | Speedup vs 1 | CPU Utilization |
|---------|----------|--------------|-----------------|
| 1       | 22.40    | 1.0x         | 98%             |
| 2       | 11.42    | 2.0x         | 197%            |
| 4       | 5.65     | 4.0x         | 396%            |
| 6       | 3.84     | 5.8x         | 593%            |
| 8       | 2.93     | 7.6x         | 788%            |
| 10      | 2.88     | 7.8x         | 897%            |

**Observation**: Near-linear scaling up to 8 threads, with diminishing returns beyond that due to
coordination overhead and memory bandwidth limits.

### Benchmark Commands

```bash
# Build release binary
cargo build --release

# Run with specific thread count
time ./target/release/jones --lib target/debug/libdylib_example.dylib --max-threads N

# Default (uses all available cores)
time ./target/release/jones --lib target/debug/libdylib_example.dylib
```

## Notes

- The `dylib` example benefits most from parallelization due to its size (includes std library)
- CPU utilization reaches ~1000% (10 cores fully utilized) with default settings
- The `--max-threads N` option allows limiting parallelism if needed
