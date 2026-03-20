---
layout: default
title: Panic Reference
---

# Panic Reference

Jonesy detects and classifies panics using unique error codes (/jonesy/panics/JP001-JP022). Each panic type includes:

- **Description**: What causes this panic
- **Example**: Code that triggers the panic
- **Solution**: How to avoid or handle the panic
- **Related**: Similar panic types

## Panic Categories

### Explicit Panics
Panics intentionally triggered by the programmer:
- [JP001 - Explicit Panic](/jonesy/panics/JP001-explicit-panic): Direct `panic!()` call
- [JP010 - Assert Failed](/jonesy/panics/JP010-assert-failed): `assert!()` condition false
- [JP011 - Debug Assert Failed](/jonesy/panics/JP011-debug-assert-failed): `debug_assert!()` in debug builds
- [JP012 - Unreachable](/jonesy/panics/JP012-unreachable): Code marked as unreachable
- [JP013 - Unimplemented](/jonesy/panics/JP013-unimplemented): Unfinished implementation
- [JP014 - Todo](/jonesy/panics/JP014-todo): Placeholder for future code

### Option/Result Handling
Panics from improper error handling:
- [JP006 - Unwrap None](/jonesy/panics/JP006-unwrap-none): `unwrap()` on `None`
- [JP007 - Unwrap Err](/jonesy/panics/JP007-unwrap-err): `unwrap()` on `Err`
- [JP008 - Expect None](/jonesy/panics/JP008-expect-none): `expect()` on `None`
- [JP009 - Expect Err](/jonesy/panics/JP009-expect-err): `expect()` on `Err`

### Numeric Operations
Panics from arithmetic and bit operations:
- [JP003 - Arithmetic Overflow](/jonesy/panics/JP003-arithmetic-overflow): Integer overflow
- [JP004 - Shift Overflow](/jonesy/panics/JP004-shift-overflow): Invalid bit shift
- [JP005 - Division by Zero](/jonesy/panics/JP005-division-by-zero): Divide by zero

### Memory and Indexing
Panics from invalid memory access:
- [JP002 - Bounds Check](/jonesy/panics/JP002-bounds-check): Index out of bounds
- [JP020 - String/Slice Error](/jonesy/panics/JP020-string-slice-error): Invalid string indexing
- [JP022 - Misaligned Pointer](/jonesy/panics/JP022-misaligned-pointer): Alignment violation

### Resource Exhaustion
Panics from system limits:
- [JP018 - Capacity Overflow](/jonesy/panics/JP018-capacity-overflow): Collection too large
- [JP019 - Out of Memory](/jonesy/panics/JP019-out-of-memory): Allocation failed

### Runtime Errors
Other runtime panic conditions:
- [JP015 - Panic in Drop](/jonesy/panics/JP015-panic-in-drop): Destructor panic
- [JP016 - Cannot Unwind](/jonesy/panics/JP016-cannot-unwind): FFI boundary panic
- [JP017 - Formatting Error](/jonesy/panics/JP017-formatting-error): Format string error
- [JP021 - Invalid Enum](/jonesy/panics/JP021-invalid-enum): Corrupted enum value
