---
layout: default
title: "JP021: Invalid Enum"
---

# JP021: Invalid Enum

**Severity**: Critical
**Category**: Runtime Errors

## Description

An enum has an invalid discriminant value. This typically indicates memory corruption or incorrect use of unsafe code.

## Example

```rust
#[repr(u8)]
enum Status {
    Active = 0,
    Inactive = 1,
    Pending = 2,
}

fn from_byte(b: u8) -> Status {
    unsafe { std::mem::transmute(b) }  // JP021 if b > 2!
}

fn main() {
    let status = from_byte(255);  // Undefined behavior -> panic
}
```

## Why It Happens

- Incorrect `transmute` from integer to enum
- Memory corruption (buffer overflow, use-after-free)
- Reading uninitialized memory
- Incorrect FFI data

## How to Avoid

### Use TryFrom for safe conversion

```rust
impl TryFrom<u8> for Status {
    type Error = InvalidStatusError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Status::Active),
            1 => Ok(Status::Inactive),
            2 => Ok(Status::Pending),
            _ => Err(InvalidStatusError(value)),
        }
    }
}
```

### Use num_enum crate

```rust
use num_enum::TryFromPrimitive;

#[derive(TryFromPrimitive)]
#[repr(u8)]
enum Status {
    Active = 0,
    Inactive = 1,
    Pending = 2,
}

let status = Status::try_from(byte)?;
```

### Validate FFI data

```rust
#[no_mangle]
pub extern "C" fn process_status(status_code: u8) -> i32 {
    let status = match Status::try_from(status_code) {
        Ok(s) => s,
        Err(_) => return -1,  // Invalid input
    };
    // Now safe to use status
    process(status)
}
```

## Jonesy Output

```text
 --> src/lib.rs:9:14 [invalid enum discriminant]
     = help: Use TryFrom instead of transmute for enum conversion
```

## Related

- [JP022 - Misaligned Pointer](/jonesy/panics/JP022-misaligned-pointer): Other unsafe memory issues
