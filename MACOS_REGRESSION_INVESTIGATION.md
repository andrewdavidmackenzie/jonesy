# macOS Regression Investigation

**Date**: 2026-04-13  
**Branch**: x86_64_support_242  
**PR**: #245

## Problem Summary

After merging the `benchmark_x86_64` branch into `x86_64_support_242`, all macOS CI tests are failing while Linux tests pass.

### CI Results (Run #24344992911 with macos-26/macos-26-intel)

✅ **Linux x86_64 (ELF)**: PASSED  
✅ **Linux ARM (ELF)**: PASSED  
❌ **macOS ARM (MachO macos-26)**: FAILED - 1 test (test_workspace_test_example)  
❌ **macOS Intel x86_64 (MachO macos-26-intel)**: FAILED - 16+ tests including test_panic_example

### Failure Pattern

**test_panic_example on macos-26-intel**:
- Missing ALL 60 panic points (exit_code=0, markers=60)
- 0 panics detected when 60 expected
- This suggests **complete failure to build call graphs**, not just detection issues

**Other failing tests on macos-26-intel**:
- test_cdylib_example
- test_dylib_example
- test_simple_panic_line_detection
- test_multi_bin_example
- test_workspace_test_example
- test_async_fn_resumed_detection
- test_called_function_allow_distinguishes_modules
- test_dwarf_specification_handling
- test_config_allow_panic
- test_indirect_panic_shows_called_function
- test_inlined_function_names
- test_intermediate_functions_reported_as_roots
- test_oom_detection_via_abort
- test_problem_matcher_regex
- test_scoped_rules
- test_dsym_auto_generation

## Key Finding

**This is MachO-specific, NOT x86_64-specific**

Both macOS ARM and macOS Intel fail, but Linux x86_64 passes. The common factor is the **MachO binary format**.

## Hypothesis

The regression was introduced by commits from the `benchmark_x86_64` branch that modified:
1. DWARF inline information processing
2. Call graph building logic
3. Library call graph analysis (.o file processing)

## Merged Commits (Suspicious Ones)

Listed in reverse chronological order from merge point:

### Highest Suspicion

**c67476e** - "fix: Improve line precision using DWARF inline info and fix panic!() detection"
- **Files changed**: call_graph.rs, call_tree.rs, crate_line_table.rs, function_index.rs, heuristics.rs, inline_allows.rs
- **Impact**: Major DWARF changes, added inline info processing
- **Risk**: Could break MachO DWARF processing

**1dbcb38** - "fix: Only use DWARF inline info near function prologue"  
- **Files changed**: call_graph.rs
- **Impact**: Modified when DWARF inline info is used
- **Risk**: Could affect MachO differently than ELF

### Medium Suspicion

**893e891** - "fix: Prefer section name over symbol index for ELF .o file analysis"
- **Files changed**: library_call_graph.rs
- **Impact**: Changed priority of section_func_name vs symbol_index
- **Risk**: Says "ELF .o file" but the code path is used for MachO too
- **Note**: This changed logic in `build_from_elf_object`, not `build_from_macho_object`, so less likely

**388fba4** / **37beac8** - Parallel .o file processing (revert then un-revert)
- **Files changed**: analysis.rs
- **Impact**: Changed .rlib/.a archive processing
- **Risk**: Could affect .o file analysis flow

### Lower Suspicion

**ad06813** - "fix: Add register-indirect call tracking for x86_64"
- x86_64 specific, shouldn't affect ARM macOS

**Other commits** (dad4092, 9a0ef32, 5e144bc, 8091547, etc.)
- Mostly x86_64 ELF-specific fixes
- Less likely to affect MachO on either architecture

## Bisection Plan

Master (commit 2d6ac72 - PR #243) works on macOS. Test these commits in order on macOS:

```bash
# Known good - this is what's in master
git checkout 2d6ac72
make clean && make build-examples
cargo test -p jonesy test_panic_example -- --nocapture

# First DWARF change
git checkout c67476e
make clean && make build-examples
cargo test -p jonesy test_panic_example -- --nocapture

# Second DWARF change
git checkout 1dbcb38  
make clean && make build-examples
cargo test -p jonesy test_panic_example -- --nocapture

# Section name priority change
git checkout 893e891
make clean && make build-examples
cargo test -p jonesy test_panic_example -- --nocapture

# Current HEAD
git checkout x86_64_support_242
make clean && make build-examples
cargo test -p jonesy test_panic_example -- --nocapture
```

## Quick Validation Test

On macOS, run this to see if ANY panics are detected:

```bash
cd examples/panic
cargo build
../../target/debug/jonesy --bin panic
```

Expected: Should find ~60 panic points  
If broken: Will show "No panics in crate" or very few panics

## Investigation Areas

### 1. DWARF Processing (Most Likely)

Check if MachO DWARF sections are being loaded correctly:

**Files to examine**:
- `jonesy/src/call_graph.rs` - `process_instruction_data_with_crate_table()`
- `jonesy/src/function_index.rs` - `get_inlined_call_site()`, inline info parsing
- `jonesy/src/crate_line_table.rs` - Line table building

**What to look for**:
- MachO vs ELF differences in DWARF section loading
- Inline info parsing that assumes ELF format
- Function index building that breaks on MachO dSYM

### 2. Library Call Graph (Less Likely)

The section name priority change in 893e891:

**File**: `jonesy/src/library_call_graph.rs` lines 337-350

**Current code**:
```rust
let (func_addr, func_name) = if let Some(ref name) = section_func_name {
    // Per-function section - use section name
    (text_addr, name.clone())
} else if let Some((addr, name)) = symbol_index
    .as_ref()
    .and_then(|idx| idx.find_containing(call_site_addr))
{
    // Use symbol index for non-per-function sections
    (addr, name.to_string())
} else {
    continue;
};
```

**What changed**: Now prefers `section_func_name` over `symbol_index`

**Why it might break MachO**: 
- MachO .o files might have different section naming conventions
- `section_func_name` is extracted from `.text.func_name` prefix
- If MachO uses different prefixes, this could fail

**Note**: This code is in `build_from_elf_object()`, not `build_from_macho_object()`, so impact should be limited to ELF .rlib files only. But worth checking if there's similar logic elsewhere.

### 3. Call Graph Building

Check if macOS-specific code paths changed:

**File**: `jonesy/src/call_graph.rs`

Look for:
- MachO-specific branches that might have been broken
- DWARF loading that differs between platforms
- Symbol resolution that works on Linux but not macOS

## Debugging Commands

Enable debug output to see what's happening:

```bash
# Run with verbose output
RUST_LOG=debug cargo run -p jonesy -- --bin panic 2>&1 | tee debug.log

# Check if DWARF is being loaded
grep -i "dwarf\|debug" debug.log

# Check if symbols are being found
grep -i "symbol\|function" debug.log

# Check if call graph is being built
grep -i "edge\|call" debug.log
```

## Expected Fix

Once the breaking commit is identified:

1. **If c67476e or 1dbcb38**: Revert the DWARF changes and investigate MachO-specific fixes
2. **If 893e891**: Adjust section name logic to work with MachO or make it ELF-only
3. **If 388fba4**: Investigate parallel processing interaction with MachO

## Current Status (as of commit 525f09f)

- Reverted CI matrix from macos-26/macos-26-intel to macos-14 to test if issue is runner-specific
- CI run #24344992911 in progress with macos-14
- If macos-14 also fails → confirms code regression
- If macos-14 passes → indicates macos-26 runner environment issue

## Notes

- Linux (ELF) works perfectly on both x86_64 and ARM
- This proves the x86_64 architecture support itself is correct
- The issue is specifically with MachO binary format
- Both ARM and x86_64 macOS fail, so it's not architecture-specific within MachO
