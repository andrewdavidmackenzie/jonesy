# Phase 6: Fix Remaining ELF Detection Issues

## Objective

Fix the two remaining test failures on Linux aarch64 that were ignored in Phase 5:

1. **test_dylib_example** - 42 missing panics (ALL panics missing)
2. **test_rlib_example** - 2 missing panics (conditional value tracking)

## Current Status

Both tests are currently marked with `#[cfg_attr(target_os = "linux", ignore)]` to allow CI to pass. See PHASE6_HANDOFF.md for detailed analysis.

## Issue 1: test_dylib_example (42 missing panics)

**Problem:** Entry point analysis finds "2 entry points (panic + abort)" but no panic paths are traced.

**File Type:** `target/debug/libdylib_example.so: ELF 64-bit LSB shared object, ARM aarch64`

**Hypothesis:**
- Entry points are detected correctly
- But panic path tracing doesn't work for ELF shared objects (.so files)
- This is dylib-specific (cdylib, rlib, staticlib all work)
- Likely issue in `analyze_binary_target()` vs `analyze_archive()` code paths

**Investigation Plan:**
1. Compare entry point analysis between ELF binary and ELF shared object
2. Check if the issue is in entry point detection or panic path tracing
3. Debug what happens after "Found 2 entry points" message
4. Compare with how macOS dylib works (if it works there)

## Issue 2: test_rlib_example (2 missing panics)

**Problem:** Missing 2 unwrap calls that use `rand::Rng` for conditional values

**Missing Lines:**
- `examples/rlib/src/module/mod.rs:10` - `opt.unwrap()` after conditional Some/None
- `examples/rlib/src/module/mod.rs:22` - `result.unwrap()` after conditional Ok/Err

**Working Cases:**
- Direct `None.expect()` - ✅ detected
- Direct `Some(x).unwrap()` - ✅ detected
- All other unwrap/expect calls - ✅ detected

**Key Difference:**
```rust
// This WORKS (detected):
let _: () = None.expect("expected a value");

// This FAILS (missing):
let opt: Option<i32> = if rng.random_bool(0.0) { Some(42) } else { None };
opt.unwrap();
```

**Hypothesis:**
- Panic path tracking doesn't follow through rand conditional logic
- Might be a line number mapping issue (marker on line 10, actual panic on line 11)
- Or the unwrap path is missed because the Option value comes from a conditional

**Investigation Plan:**
1. Check if the issue is line number alignment
2. Test with simpler conditional: `let opt = if true { None } else { Some(42) };`
3. Debug why rand-based conditionals break detection
4. Check if it's specific to the rand library or any runtime-conditional values

## Success Criteria

- [ ] test_dylib_example passes (42 panics detected)
- [ ] test_rlib_example passes (2 missing unwraps detected)
- [ ] Remove `#[cfg_attr(target_os = "linux", ignore)]` from both tests
- [ ] No regressions in other tests (24 tests still passing)
- [ ] Understanding of why these cases failed and how the fix works

## Test Commands

```bash
# Run failing tests individually
cargo test --release test_dylib_example
cargo test --release test_rlib_example

# Check what jonesy detects
(cd examples/dylib && ../../target/release/jonesy --lib)
(cd examples/rlib && ../../target/release/jonesy --lib)

# Run all tests
make test
```

## Notes

- These are pre-existing issues in the ELF analysis path, unrelated to Phase 5 filtering
- Phase 5 proved archive analysis works when properly filtered (staticlib passes)
- Focus on understanding the root causes before implementing fixes
