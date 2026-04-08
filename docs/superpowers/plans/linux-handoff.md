# Linux aarch64 Handoff — Issue #231, Phase 5

## Branch
`issue-231-linux-aarch64-phase5` — PR #236

## Current State

- **macOS**: all tests pass (404 unit + 27 integration + 15 LSP)
- **Linux**: 399 unit tests pass, 17/26 integration tests pass
- **9 failing tests**: all library/archive analysis (rlib, staticlib, dylib, workspace)
- **Binary analysis works** — panic, simple_panic, inlined, config tests all pass on Linux

## The Problem

Library analysis on ELF finds call graph edges (90-97 per object file) but reports "No panics in crate". The edges exist but the target symbol names don't match `is_library_panic_symbol()` in `jonesy/src/heuristics.rs`.

## Root Cause (suspected)

`is_library_panic_symbol` checks demangled target symbol names against:
- `LIBRARY_PANIC_MODULE_PREFIXES`: `["core::panicking::", "std::panicking::"]`
- `LIBRARY_PANIC_METHOD_SUFFIXES`: `[">::unwrap", ">::expect", etc.]`

On ELF, the demangled names might differ from MachO — different hash suffixes, module paths, or demangling output.

## How to Investigate

```bash
# 1. Build the rlib example
cd examples/rlib && cargo build && cd ../..

# 2. Run jonesy on it — expect "No panics in crate"
cargo run -- --lib --no-hyperlinks 2>&1

# 3. Temporarily add debug output to see what target symbols exist
# In jonesy/src/analysis.rs, in analyze_archive(), after building merged_graph,
# add before the "Search for callers" loop:
#
#   eprintln!("Target symbols in call graph:");
#   for sym in merged_graph.target_symbols() {
#       eprintln!("  {}", sym);
#   }
#
# Then rebuild and run:
cargo run -- --lib --no-hyperlinks 2>&1

# 4. Compare the printed target symbols against the patterns in
#    jonesy/src/heuristics.rs (is_library_panic_symbol function)

# 5. Fix the pattern matching or symbol name handling, then run:
make test
```

## Likely Fixes

1. **Demangling difference**: ELF might produce slightly different demangled output. Check if `core::panicking::panic_fmt` appears with a different module path on Linux.

2. **Symbol name format**: MachO symbols have leading `_` which gets stripped. ELF symbols don't. If somewhere in the library call graph the ELF path still has mangled names or incorrect demangling, fix it.

3. **Pattern matching**: The `starts_with`/`ends_with` checks in `is_library_panic_symbol` might need adjusting for ELF symbol format.

## Files to Check

- `jonesy/src/heuristics.rs:83-96` — `LIBRARY_PANIC_MODULE_PREFIXES` and `LIBRARY_PANIC_METHOD_SUFFIXES`
- `jonesy/src/heuristics.rs:103` — `is_library_panic_symbol()` function
- `jonesy/src/library_call_graph.rs:200+` — `build_from_elf_object`, specifically how target symbols are demangled (line ~310)
- `jonesy/src/analysis.rs:380+` — `analyze_archive`, where `is_library_panic_symbol` is called

## After Fixing

1. Run `make test` — all should pass
2. Run `cargo clippy --all-targets` — should be clean
3. Commit and push to the `issue-231-linux-aarch64-phase5` branch
4. The PR #236 will update automatically

## Remaining After Library Analysis Fix

Once the 9 library tests pass, the full Linux support should be complete for Phase 5. Verify with `make test` on both macOS and Linux CI.
