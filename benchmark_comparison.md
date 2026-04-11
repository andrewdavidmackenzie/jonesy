# Benchmark Results Comparison

## How to Compare Results

1. Run `./benchmark.sh` on each platform:
   - Linux x86_64
   - Linux aarch64
   - macOS aarch64

2. This generates files like:
   - `benchmark_results_Linux-x86_64_YYYYMMDD_HHMMSS.txt`
   - `benchmark_results_Linux-aarch64_YYYYMMDD_HHMMSS.txt`
   - `benchmark_results_Darwin-arm64_YYYYMMDD_HHMMSS.txt`

3. Compare median times across platforms for each binary

## Performance Ratio Calculation

For each binary, calculate the performance ratio:

```
Ratio = Time_x86_64 / Time_aarch64
```

- **Ratio > 1.0**: x86_64 is slower (needs optimization)
- **Ratio = 1.0**: Equal performance
- **Ratio < 1.0**: x86_64 is faster

## Key Metrics to Compare

### 1. Disassembly Performance

Look at binaries with large `.text` sections:
- **x86_64**: Uses Capstone (variable-length instructions)
- **aarch64**: Direct pattern matching (fixed 4-byte instructions)

Expected: aarch64 should be faster due to simpler instruction scanning.

### 2. GOT Resolution Overhead (x86_64 only)

x86_64 builds GOT cache from ELF relocations. Monitor:
- Time to build GOT cache
- Number of GOT entries resolved
- Impact on overall analysis time

### 3. DWARF Processing

Should be similar across architectures unless there are differences in:
- Line table size/complexity
- Debug info organization

### 4. Overall Throughput

Measure in terms of:
- **Instructions/second**: Text section size / time
- **Panics detected/second**: Panic count / time

## Bottleneck Identification

If x86_64 is significantly slower, check:

1. **Capstone overhead**: Compare time spent in disassembly
2. **GOT resolution**: Is cache building efficient?
3. **Memory allocation**: Are we creating unnecessary temporary structures?
4. **Parallel efficiency**: Is chunking strategy optimal for variable-length ISA?

## Optimization Opportunities

Based on ratio analysis:

### If disassembly is slow (x86_64):
- [ ] Profile Capstone calls
- [ ] Optimize chunk size for parallel processing
- [ ] Cache instruction decoder instance
- [ ] Pre-filter obvious non-CALL instructions

### If GOT resolution is slow (x86_64):
- [ ] Use faster HashMap implementation (e.g., FxHashMap)
- [ ] Pre-allocate HashMap capacity
- [ ] Lazy GOT cache building (only for indirect calls)

### If overall throughput is poor:
- [ ] Review parallel processing efficiency
- [ ] Check for unnecessary allocations
- [ ] Profile with `perf` or `instruments`

## Example Comparison Table

| Binary | x86_64 (median) | aarch64 (median) | Ratio | Notes |
|--------|-----------------|------------------|-------|-------|
| simple_panic | 1.2s | 0.8s | 1.5x | Disassembly overhead |
| panic | 3.5s | 2.1s | 1.67x | GOT resolution impact |
| perfect | 1.0s | 0.7s | 1.43x | Baseline overhead |
| meshcore-rs | 8.2s | 5.5s | 1.49x | Large binary |

## Profiling Commands

### Linux (perf)
```bash
perf record -g ./target/release/jonesy --bin target/debug/panic
perf report
```

### macOS (Instruments)
```bash
xcrun xctrace record --template 'Time Profiler' \
  --launch ./target/release/jonesy -- --bin target/debug/panic
```

## Action Items Template

After comparing results:

- [ ] Record performance ratios for all binaries
- [ ] Identify top 3 bottlenecks
- [ ] Profile slowest operations
- [ ] Implement targeted optimizations
- [ ] Re-benchmark and measure improvement
