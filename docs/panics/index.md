---
layout: default
title: Panic Reference
---

# Panic Reference

Jonesy detects and classifies panics using unique error codes (JP001-JP022). Each panic type includes:

- **Description**: What causes this panic
- **Example**: Code that triggers the panic
- **Solution**: How to avoid or handle the panic
- **Related**: Similar panic types

## Panic Categories

### Explicit Panics
Panics intentionally triggered by the programmer:
- [JP001 - Explicit Panic](JP001-explicit-panic.md): Direct `panic!()` call
- [JP010 - Assert Failed](JP010-assert-failed.md): `assert!()` condition false
- [JP011 - Debug Assert Failed](JP011-debug-assert-failed.md): `debug_assert!()` in debug builds
- [JP012 - Unreachable](JP012-unreachable.md): Code marked as unreachable
- [JP013 - Unimplemented](JP013-unimplemented.md): Unfinished implementation
- [JP014 - Todo](JP014-todo.md): Placeholder for future code

### Option/Result Handling
Panics from improper error handling:
- [JP006 - Unwrap None](JP006-unwrap-none.md): `unwrap()` on `None`
- [JP007 - Unwrap Err](JP007-unwrap-err.md): `unwrap()` on `Err`
- [JP008 - Expect None](JP008-expect-none.md): `expect()` on `None`
- [JP009 - Expect Err](JP009-expect-err.md): `expect()` on `Err`

### Numeric Operations
Panics from arithmetic and bit operations:
- [JP003 - Arithmetic Overflow](JP003-arithmetic-overflow.md): Integer overflow
- [JP004 - Shift Overflow](JP004-shift-overflow.md): Invalid bit shift
- [JP005 - Division by Zero](JP005-division-by-zero.md): Divide by zero

### Memory and Indexing
Panics from invalid memory access:
- [JP002 - Bounds Check](JP002-bounds-check.md): Index out of bounds
- [JP020 - String/Slice Error](JP020-string-slice-error.md): Invalid string indexing
- [JP022 - Misaligned Pointer](JP022-misaligned-pointer.md): Alignment violation

### Resource Exhaustion
Panics from system limits:
- [JP018 - Capacity Overflow](JP018-capacity-overflow.md): Collection too large
- [JP019 - Out of Memory](JP019-out-of-memory.md): Allocation failed

### Runtime Errors
Other runtime panic conditions:
- [JP015 - Panic in Drop](JP015-panic-in-drop.md): Destructor panic
- [JP016 - Cannot Unwind](JP016-cannot-unwind.md): FFI boundary panic
- [JP017 - Formatting Error](JP017-formatting-error.md): Format string error
- [JP021 - Invalid Enum](JP021-invalid-enum.md): Corrupted enum value
