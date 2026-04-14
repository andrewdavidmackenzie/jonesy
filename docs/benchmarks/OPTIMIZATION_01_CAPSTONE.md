# Optimization #1: Fix Capstone Instance Creation

## Problem Identified

The x86_64 implementation was creating a **new Capstone disassembler instance per chunk** during parallel processing. On an 8-core system, this meant creating 8 instances. On an M2 Pro (12 cores), this was creating 12 instances!

**Root cause:** `scan_call_instructions()` was initializing Capstone at line 42, and this function was called once per chunk in the parallel iterator.

## The Fix

**Before:**
```rust
// parallel_disassemble splits into chunks
chunks.par_iter().map(|chunk| {
    scan_call_instructions(chunk, ...)  // Creates Capstone here! (8x)
})

fn scan_call_instructions(...) {
    let cs = Capstone::new()...build(); // ❌ Called 8 times!
    ...
}
```

**After:**
```rust
// parallel_disassemble creates Capstone once
let cs = Capstone::new()...build(); // ✅ Called once!

// Note: Capstone is not Sync, so we use sequential scanning
scan_call_instructions(&cs, text_data, ...)

fn scan_call_instructions(cs: &Capstone, ...) {
    // Use provided instance
}
```

**Trade-off:** We lost parallelization, but gained much more by eliminating redundant Capstone initialization. Capstone's initialization overhead far exceeds the benefit of parallel scanning.

## Results

### simple_panic (3.8M binary)

| Metric | Before | After | Improvement |
|--------|--------|-------|-------------|
| Median time | 1.123s | **0.197s** | **5.7x faster** |
| vs aarch64 | 21.2x slower | **3.7x slower** | **5.7x improvement** |
| Panic points | 33 | 33 | ✓ Correct |

### meshchat (415M binary)

| Metric | Before | After | Improvement |
|--------|--------|-------|-------------|
| Time | 165.3s | **118.7s** | **1.4x faster** |
| vs aarch64 | 38.6x slower | **27.7x slower** | **1.4x improvement** |
| Panic points | 557 | 557 | ✓ Correct |

## Analysis

1. **Small binaries see huge gains** (5.7x) - Capstone initialization was dominating the time
2. **Large binaries see moderate gains** (1.4x) - Other factors (DWARF parsing, memory) dominate
3. **Still 3-27x slower than aarch64** - More optimizations needed

## Remaining Bottlenecks

Based on the results:

1. **Capstone disassembly itself** is still slower than aarch64's direct pattern matching
2. **GOT cache** may benefit from FxHashMap (next optimization)
3. **DWARF parsing** likely dominates on large binaries
4. **Memory allocations** during instruction iteration

## Next Steps

- [ ] Profile with `perf` to identify next bottleneck
- [ ] Try FxHashMap for GOT cache
- [ ] Consider iced-x86 as Capstone alternative (might be faster)
- [ ] Look at memory allocation patterns

---

**Date:** 2026-04-11  
**Commit:** TBD  
**Status:** ✅ Implemented and verified
