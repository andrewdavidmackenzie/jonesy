---
layout: default
title: "JP016: Cannot Unwind"
---

# JP016: Cannot Unwind

**Severity**: Critical
**Category**: Runtime Errors

## Description

A panic occurred in a context where unwinding is not allowed, such as across an `extern "C"` FFI boundary.

## Example

```rust
#[no_mangle]
pub extern "C" fn process_data(ptr: *const u8, len: usize) -> i32 {
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    parse(slice).unwrap()  // JP016: panic cannot cross FFI boundary!
}
```

## Why It's Dangerous

- Undefined behavior if panic unwinds through C code
- Will abort the entire process
- Can corrupt state in calling C/C++ code

## How to Avoid

### Catch panics at FFI boundary

```rust
use std::panic::{catch_unwind, AssertUnwindSafe};

#[no_mangle]
pub extern "C" fn process_data(ptr: *const u8, len: usize) -> i32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        parse(slice)
    }));

    match result {
        Ok(Ok(value)) => value,
        Ok(Err(_)) => -1,  // Parse error
        Err(_) => -2,      // Panic occurred
    }
}
```

### Use Result-based API

```rust
#[no_mangle]
pub extern "C" fn process_data(
    ptr: *const u8,
    len: usize,
    out_error: *mut i32,
) -> i32 {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        // ... implementation
    }));

    match result {
        Ok(Ok(v)) => { unsafe { *out_error = 0; } v }
        Ok(Err(e)) => { unsafe { *out_error = e.code(); } 0 }
        Err(_) => { unsafe { *out_error = -1; } 0 }
    }
}
```

### Use `extern "C-unwind"` (Rust 1.71+)

```rust
// Allows unwinding across FFI if the caller supports it
#[no_mangle]
pub extern "C-unwind" fn may_panic() {
    panic!("This can unwind through C++ code with exceptions");
}
```

## Jonesy Output

```text
 --> src/ffi.rs:5:5 [panic in no-unwind context]
     = help: Catch panics at FFI boundaries with catch_unwind
```

## Related

- [JP015 - Panic in Drop](JP015-panic-in-drop.md): Double panic issues
