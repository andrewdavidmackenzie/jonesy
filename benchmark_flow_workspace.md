# Jonesy Benchmark: flow Workspace

**Date:** 2026-03-17
**Jonesy Version:** 0.5.0
**Workspace:** /Users/amackenz/workspace/flow
**Platform:** macOS ARM64 (Apple Silicon)

## Summary

| Metric | Before | After | Speedup |
|--------|--------|-------|---------|
| Total workspace time | ~6 minutes | **11.5 seconds** | **31x** |
| flowrgui (largest) | 175.4s | 3.89s | 45x |
| flowrcli | 125s | 3.27s | 38x |
| flowc | 5.54s | 0.62s | 9x |

### Results by Member

| Member | Panic Points | Files |
|--------|--------------|-------|
| flowc | 485 | 37 |
| flowstdlib | 4 | 2 |
| flowr | 682 | 92 |
| flowcore | 23 | 11 |
| flowmacro | 25 | 4 |
| **Total** | **1,140** | - |

---

## Optimization: Pre-built Full Line Table

The key optimization was replacing O(n) DWARF line table traversals with O(log n) binary search lookups.

### The Problem

The `get_source_location()` function was called for each unique function address during instruction processing. Each call traversed ALL DWARF line table rows sequentially - O(n) per lookup.

For flowrgui with 14,573 unique function lookups and ~1.4M line entries:
- 14,573 × O(1.4M) ≈ **20 billion row comparisons**

### The Solution

Build a `FullLineTable` once upfront (single O(n) traversal), then use binary search for all lookups:
- 14,573 × O(log 1.4M) ≈ **300,000 comparisons**

### Results by Phase (flowrgui)

| Phase | Before | After | Speedup |
|-------|--------|-------|---------|
| build full line table | - | 902ms | (new) |
| process instructions | **171.8s** | **283ms** | **607x** |
| Build call graph total | 174.4s | 3.28s | 53x |
| **TOTAL** | **175.4s** | **3.89s** | **45x** |

---

## Detailed Timing: Before Optimization

### flowc (Binary)
**Target:** `target/debug/flowc`
**Total:** 5.54s

| Phase | Time | Details |
|-------|------|---------|
| Find panic symbol | 30ms | |
| Load debug info | 13ms | Using .dSYM bundle |
| **Build call graph** | **5.05s** | **(91% of total)** |
| ├─ get_functions_from_dwarf | 162ms | 4,754 functions |
| ├─ scan for branch instructions | 3ms | 83,747 found |
| ├─ build crate line table | 295ms | 8,087 entries |
| └─ process instructions | 4.59s | 2,655 source_cache entries |
| Build call tree | 55ms | |
| Prune call tree | 33ms | |
| Collect/output | 353ms | |

### flowr - flowrcli (Binary)
**Target:** `target/debug/flowrcli`
**Total:** 125s (2m 5s)

| Phase | Time | Details |
|-------|------|---------|
| Find panic symbol | 95ms | |
| Load debug info | 116ms | Using .dSYM bundle |
| **Build call graph** | **124.2s** | **(99% of total)** |
| ├─ get_functions_from_dwarf | 836ms | 17,186 functions |
| ├─ scan for branch instructions | 12ms | 352,138 found |
| ├─ build crate line table | 1.26s | 10,401 entries |
| └─ process instructions | 122.1s | 10,827 source_cache entries |
| Build call tree | 167ms | |
| Prune call tree | 360ms | |
| Collect/output | 51ms | |

### flowr - flowrgui (Binary) — SLOWEST
**Target:** `target/debug/flowrgui`
**Total:** 175.4s (2m 55s)

| Phase | Time | Details |
|-------|------|---------|
| Find panic symbol | 119ms | |
| Load debug info | 97ms | Using .dSYM bundle |
| **Build call graph** | **174.4s** | **(99% of total)** |
| ├─ get_functions_from_dwarf | 1.03s | 22,245 functions |
| ├─ scan for branch instructions | 14ms | 428,716 found |
| ├─ build crate line table | 1.55s | 10,110 entries |
| └─ process instructions | 171.8s | 14,573 source_cache entries |
| Build call tree | 239ms | |
| Prune call tree | 476ms | |
| Collect/output | 60ms | |

---

## Detailed Timing: After Optimization

### flowc (Binary)
**Target:** `target/debug/flowc`
**Total:** 618ms

| Phase | Time | Details |
|-------|------|---------|
| Find panic symbol | 29ms | |
| Load debug info | 13ms | Using .dSYM bundle |
| **Build call graph** | **490ms** | |
| ├─ get_functions_from_dwarf | 141ms | 4,754 functions |
| ├─ scan for branch instructions | 2ms | 83,747 found |
| ├─ build crate line table | 154ms | 1,087 entries |
| ├─ build full line table | 134ms | 271,548 entries |
| └─ process instructions | 49ms | |
| Build call tree | 49ms | |
| Prune call tree | 25ms | |
| Collect/output | 13ms | |

### flowr - flowrcli (Binary)
**Target:** `target/debug/flowrcli`
**Total:** 3.27s

| Phase | Time | Details |
|-------|------|---------|
| Find panic symbol | 93ms | |
| Load debug info | 56ms | Using .dSYM bundle |
| **Build call graph** | **2.74s** | |
| ├─ get_functions_from_dwarf | 854ms | 17,186 functions |
| ├─ scan for branch instructions | 16ms | 352,138 found |
| ├─ build crate line table | 935ms | 2,649 entries |
| ├─ build full line table | 666ms | 1,141,476 entries |
| └─ process instructions | 223ms | |
| Build call tree | 200ms | |
| Prune call tree | 151ms | |
| Collect/output | 28ms | |

### flowr - flowrgui (Binary)
**Target:** `target/debug/flowrgui`
**Total:** 3.89s

| Phase | Time | Details |
|-------|------|---------|
| Find panic symbol | 112ms | |
| Load debug info | 71ms | Using .dSYM bundle |
| **Build call graph** | **3.28s** | |
| ├─ get_functions_from_dwarf | 949ms | 22,245 functions |
| ├─ scan for branch instructions | 15ms | 428,716 found |
| ├─ build crate line table | 1.07s | 2,841 entries |
| ├─ build full line table | 902ms | 1,428,583 entries |
| └─ process instructions | 283ms | |
| Build call tree | 246ms | |
| Prune call tree | 159ms | |
| Collect/output | 21ms | |

---

## Library Analysis (unchanged - already fast)

rlib analysis via relocations remains fast:
- libflowrlib.rlib: 26ms
- libflowcore.rlib: 38ms
- libflowstdlib.rlib: 5.6ms
- libflowrclib.rlib: 40ms

---

## Future Optimization Opportunities

1. **Skip already-analyzed shared code** - flowrcli, flowrex, flowrgui all share flowr library code. Analysis could be cached/shared.

2. **Incremental analysis** - Only re-analyze functions that changed since last run.

3. **Early termination** - Stop analyzing a call path once we've found a panic point (if user only wants existence, not full tree).
