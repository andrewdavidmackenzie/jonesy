---
layout: default
title: "JP018: Capacity Overflow"
---

# JP018: Capacity Overflow

**Severity**: High
**Category**: Resource Exhaustion

## Description

A collection's capacity calculation overflowed, typically when trying to allocate an impossibly large collection.

## Example

```rust
fn huge_vector() {
    let size: usize = usize::MAX;
    let mut v: Vec<u8> = Vec::with_capacity(size);  // JP018!
}
```

## Why It Happens

- Allocating too many elements (`size * element_size` overflows)
- Exponential growth hitting capacity limits
- Untrusted input controlling collection size

## How to Avoid

### Validate sizes from external input

```rust
fn create_buffer(requested_size: usize) -> Result<Vec<u8>, Error> {
    const MAX_BUFFER: usize = 1024 * 1024 * 100;  // 100 MB limit
    if requested_size > MAX_BUFFER {
        return Err(Error::BufferTooLarge);
    }
    Ok(Vec::with_capacity(requested_size))
}
```

### Use try_reserve

```rust
fn grow_buffer(buf: &mut Vec<u8>, additional: usize) -> Result<(), TryReserveError> {
    buf.try_reserve(additional)?;
    Ok(())
}
```

### Check multiplication before allocation

```rust
fn allocate_matrix(rows: usize, cols: usize) -> Result<Vec<Vec<i32>>, Error> {
    let total = rows.checked_mul(cols)
        .ok_or(Error::CapacityOverflow)?;
    if total > MAX_ELEMENTS {
        return Err(Error::TooLarge);
    }
    // Safe to allocate
    Ok(vec![vec![0; cols]; rows])
}
```

## Jonesy Output

```text
 --> src/lib.rs:3:5 [capacity overflow]
     = help: Validate collection sizes and use try_reserve
```

## Related

- [JP019 - Out of Memory](/jonesy/panics/JP019-out-of-memory): Allocation failure
