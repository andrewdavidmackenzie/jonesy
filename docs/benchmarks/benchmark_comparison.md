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

## Linux aarch64 Scaling Results

### Before Parallel .rlib Processing
**benchmark_results_Linux-aarch64_20260411_202239.txt (2 cores, 4GB RAM)**  
**benchmark_results_Linux-aarch64_20260411_211517.txt (4 cores, 8GB RAM)**

| Binary | 2 Cores (median) | 4 Cores (median) | Improvement | Notes |
|--------|------------------|------------------|-------------|-------|
| simple_panic | 0.053s | 0.054s | -2% | Noise |
| panic | 0.074s | 0.064s | **14%** | Good scaling |
| perfect | 0.058s | 0.057s | 2% | Noise |
| rlib_example | 0.057s | 0.057s | 0% | No change |
| dylib_example | 0.089s | 0.083s | 7% | Moderate scaling |
| staticlib_example | 0.009s | 0.009s | 0% | Too fast |
| **meshcore-rs rlib** | **19.817s** | **19.790s** | **0%** | ⚠️ No scaling |
| **meshchat** | **4.277s** | **3.736s** | **13%** | Good scaling |
| flowcore rlib | 0.154s | 0.153s | 0% | No change |

**Key Findings:**
- **Regular binaries (5MB+)**: Show 7-14% improvement with 4 cores
- **Large binaries (meshchat 399M)**: 13% faster - demonstrates parallel benefit  
- **⚠️ .rlib files**: NO improvement - single-threaded archive processing bottleneck
- **Small binaries (<5MB)**: Too fast for parallelization overhead to be worthwhile

### After Parallel .rlib Processing (2026-04-11)
**benchmark_results_Linux-aarch64_20260411_214418.txt (4 cores, 8GB RAM)**

| Binary | Before (4 cores) | After (4 cores) | Improvement | Notes |
|--------|------------------|-----------------|-------------|-------|
| meshcore-rs rlib | 19.790s | **19.185s** | **3%** | Small improvement |
| flowcore rlib | 0.153s | **0.081s** | **47%** ✅ | Great scaling! |

**Thread scaling for flowcore.rlib (18M .rlib file):**

| Threads | Time (median) | Speedup | CPU Efficiency |
|---------|---------------|---------|----------------|
| 1       | 0.178s        | 1.00x   | 100%           |
| 2       | 0.103s        | 1.73x   | 87%            |
| 4       | 0.081s        | **2.20x** ✅ | 55%        |

**✅ Parallel .o file processing is now working!** flowcore.rlib shows **47% improvement** (0.153s → 0.081s).

**Note on meshcore-rs**: Shows minimal improvement (3%) despite parallelization working correctly. This appears specific to meshcore-rs's DWARF processing characteristics rather than a parallelization issue.

## Example Comparison Table (Template for cross-platform comparison)

| Binary | x86_64 (median) | aarch64 (median) | Ratio | Notes |
|--------|-----------------|------------------|-------|-------|
| simple_panic | TBD | 0.054s | - | Disassembly overhead |
| panic | TBD | 0.064s | - | GOT resolution impact |
| perfect | TBD | 0.057s | - | Baseline overhead |
| meshcore-rs | TBD | 19.790s | - | Large .rlib file |

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
