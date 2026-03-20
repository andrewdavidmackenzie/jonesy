---
layout: default
title: "JP022: Misaligned Pointer"
---

# JP022: Misaligned Pointer

**Severity**: Critical
**Category**: Memory and Indexing

## Description

A pointer dereference failed because the pointer was not properly aligned for the target type.

## Example

```rust
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    let ptr = bytes[offset..].as_ptr() as *const u32;
    unsafe { *ptr }  // JP022 if offset is not 4-byte aligned!
}
```

## Why It Happens

- Casting byte pointers to larger types without alignment check
- Packed structs accessed incorrectly
- Manual pointer arithmetic errors
- FFI with mismatched alignment requirements

## How to Avoid

### Use safe byte reading

```rust
fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}
```

### Use bytemuck or zerocopy crates

```rust
use bytemuck::{Pod, Zeroable, try_from_bytes};

#[derive(Copy, Clone, Pod, Zeroable)]
#[repr(C)]
struct Header {
    magic: u32,
    version: u32,
}

fn parse_header(bytes: &[u8]) -> Option<&Header> {
    try_from_bytes(&bytes[..8]).ok()
}
```

### Check alignment explicitly

```rust
fn read_aligned<T>(ptr: *const u8) -> Option<T>
where
    T: Copy,
{
    if (ptr as usize) % std::mem::align_of::<T>() != 0 {
        return None;
    }
    unsafe { Some(std::ptr::read(ptr as *const T)) }
}
```

### Use read_unaligned for unaligned access

```rust
fn read_u32_unaligned(bytes: &[u8], offset: usize) -> u32 {
    let ptr = bytes[offset..].as_ptr() as *const u32;
    unsafe { std::ptr::read_unaligned(ptr) }
}
```

### Use repr(packed) carefully

```rust
#[repr(C, packed)]
struct PackedData {
    byte: u8,
    word: u32,  // May be misaligned!
}

// Safe access to potentially misaligned field
fn get_word(data: &PackedData) -> u32 {
    // Use addr_of! to avoid creating misaligned reference
    unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(data.word)) }
}
```

## Jonesy Output

```
 --> src/lib.rs:4:14 [misaligned pointer dereference]
     = help: Ensure pointer alignment requirements are met; review unsafe pointer casts
```

## Related

- [JP021 - Invalid Enum](JP021-invalid-enum.md): Other unsafe memory issues
- [JP002 - Bounds Check](JP002-bounds-check.md): Memory access errors
