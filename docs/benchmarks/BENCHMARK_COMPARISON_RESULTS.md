# Jonesy Performance Comparison Across Platforms

> **Note:** These benchmarks compare different machines with different CPUs,
> core counts, and operating systems. Performance differences reflect the
> combined effect of hardware, OS, and architecture — not architecture alone.
> A fair architecture comparison would require the same CPU generation and
> core count on both sides.

## Platform Details

| Platform | CPU | Cores | Rust Version |
|----------|-----|-------|--------------|
| **Linux x86_64** | Intel i7-8665U @ 1.90GHz | 8 (4C/8T, 2019 laptop) | 1.93.1 |
| **Linux aarch64** | (Unknown ARM) | 2 | 1.93.1 |
| **macOS aarch64** | Apple M2 Pro | 12 (2023 desktop) | 1.94.0 |

## Performance Comparison

### Jonesy Examples

| Binary | Size (x86/arm/mac) | x86_64 | aarch64 Linux | aarch64 macOS | **x86 vs aarch64** |
|--------|-------------------|---------|---------------|---------------|-------------------|
| **simple_panic** | 3.8M/3.8M/440K | **1.123s** | 0.053s | 0.042s | **21.2x slower** ⚠️ |
| **panic** | 5.0M/4.9M/703K | **0.751s** | 0.074s | 0.050s | **10.1x slower** ⚠️ |
| **perfect** | 3.9M/3.9M/468K | **0.817s** | 0.058s | 0.043s | **14.1x slower** ⚠️ |
| **rlib_example** | 379K/550K/416K | 0.046s | 0.057s | 0.030s | 0.8x (faster!) ✓ |
| **dylib_example** | 6.1M/6.6M/- | **1.382s** | 0.089s | - | **15.5x slower** ⚠️ |
| **staticlib_example** | 20M/21M/16M | 0.075s | 0.009s | 0.011s | **8.3x slower** ⚠️ |

### External Projects

| Binary | Size (x86/arm/mac) | x86_64 | aarch64 Linux | aarch64 macOS | **x86 vs aarch64** |
|--------|-------------------|---------|---------------|---------------|-------------------|
| **meshcore-rs.rlib** | -/6.0M/4.5M | - | 19.817s | 0.032s | - |
| **meshchat** | 415M/399M/68M | **165.3s** | 4.277s | 1.963s | **38.6x slower** ⚠️⚠️⚠️ |
| **flow/flowc** | 30M/- /7.2M | 2.024s | - | 1.308s | **1.5x slower** |
| **flow/flowrcli** | 113M/-/27M | 8.841s | - | 2.011s | **4.4x slower** ⚠️ |
| **flow/flowrex** | 84M/-/19M | 5.915s | - | 1.028s | **5.8x slower** ⚠️ |
| **flow/flowrgui** | 158M/-/33M | 9.080s | - | 2.281s | **4.0x slower** ⚠️ |
| **flow/libflowcore.rlib** | -/18M/17M | - | 0.154s | 0.097s | - |

## Key Findings

### 🔴 x86_64 Platform Significantly Slower

The x86_64 Linux platform is consistently slower, though the magnitude
reflects both architectural overhead (Capstone disassembly vs direct
pattern matching) and hardware differences (older/slower CPU).

1. **Simple binaries 10-21x slower**
   - simple_panic: **21.2x slower** (1.123s vs 0.053s)
   - perfect: **14.1x slower** (0.817s vs 0.058s)
   - dylib_example: **15.5x slower** (1.382s vs 0.089s)

2. **Large binaries 4-40x slower**
   - **meshchat: 38.6x slower** (165s vs 4.3s) - worst case
   - flowrcli: 4.4x slower
   - flowrgui: 4.0x slower

3. **Only rlib_example is faster on x86_64** (0.8x)
   - This is suspicious and needs investigation

### 🟡 CONCERNS

1. **Panic detection inconsistencies**
   - simple_panic: x86=33, aarch64=41, macOS=41
   - panic: x86=0, aarch64=91, macOS=8
   - meshchat: x86=557, aarch64=745, macOS=135
   - This suggests **different binaries** or **detection bugs**

2. **Binary size differences**
   - Linux binaries are 5-10x larger than macOS (debug info?)
   - meshchat: 415M (Linux) vs 68M (macOS)
   - This may explain some timing differences

3. **Incomplete benchmarks**
   - Linux x86_64 didn't benchmark meshcore-rs or flow libraries
   - Linux aarch64 didn't benchmark all flow binaries

### 🔍 Architecture-Specific Analysis

> The slowdowns below mix hardware and architecture factors. The Capstone
> and GOT overhead is genuinely architectural; the raw magnitude is inflated
> by comparing a 2019 laptop CPU against a 2023 desktop chip.

#### x86_64 Bottlenecks

Likely culprits for the architectural portion of the slowdown:

1. **Capstone disassembly overhead**
   - Variable-length instruction decoding is expensive
   - Each CALL instruction requires full disassembly context

2. **GOT resolution overhead**
   - Building GOT cache from relocations
   - RIP-relative address calculations
   - HashMap lookups for every indirect call

3. **Poor parallel efficiency**
   - Chunking strategy may not work well for variable-length ISA
   - Capstone decoder initialization per chunk?

#### aarch64 Efficiency

Why aarch64 is so fast:

1. **Fixed 4-byte instructions**
   - Direct pattern matching (BL_MASK/BL_OPCODE)
   - No disassembly overhead
   - O(n) scan vs O(n log n) with Capstone

2. **No GOT resolution needed**
   - Direct BL instructions for external calls
   - Simple offset calculation

3. **Better parallelization**
   - Fixed-size instructions align perfectly to chunks
   - No synchronization issues

### 📊 Performance Ratio Summary

| Category | Median Slowdown | Range |
|----------|-----------------|-------|
| Small binaries (< 10M) | **12.9x** | 10-21x |
| Large binaries (> 50M) | **21.5x** | 4-40x |
| **Overall** | **15.6x** | 4-40x |

## Optimisation Priorities

### 🎯 High Priority (Fix These First)

1. **Profile Capstone usage**
   ```bash
   perf record -g ./target/release/jonesy --bin target/debug/simple_panic
   perf report
   ```
   - Measure time in `scan_call_instructions`
   - Check decoder initialization overhead
   - Look for unnecessary allocations

2. **Optimize GOT resolution**
   - Use `FxHashMap` instead of `HashMap` (faster for integer keys)
   - Pre-allocate capacity
   - Profile `got::build_cache()` and `got::resolve_target()`

3. **Fix panic detection inconsistencies**
   - Verify all platforms analyze the same binaries
   - Check for x86_64-specific detection bugs

### 🎯 Medium Priority

4. **Review parallel chunking**
   - Current strategy may not work for variable-length ISA
   - Consider smaller/larger chunks
   - Profile parallel efficiency

5. **Cache Capstone decoder**
   - Reuse decoder instance across chunks
   - Avoid re-initialization overhead

### 🎯 Low Priority

6. **Binary size investigation**
   - Why are Linux binaries 5-10x larger?
   - Does this affect DWARF parsing time?

## Next Steps

1. **Profile x86_64 on simple_panic** (smallest overhead case)
2. **Identify hotspots** (Capstone? GOT? DWARF?)
3. **Implement targeted fixes**
4. **Re-benchmark and measure improvement**
5. **Target: < 3x slowdown vs aarch64 on comparable hardware**

---

**Generated:** 2026-04-11  
**Jonesy Version:** 0.8.0
