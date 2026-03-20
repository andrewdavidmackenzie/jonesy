---
layout: default
title: "JP001: Explicit Panic"
---

# JP001: Explicit Panic

**Severity**: High
**Category**: Explicit Panics

## Description

A direct call to the `panic!()` macro. This immediately terminates the current thread with an error message.

## Example

```rust
fn process_data(data: &[u8]) {
    if data.is_empty() {
        panic!("Cannot process empty data");  // JP001
    }
    // ...
}
```

## Why It Happens

- Programmer explicitly calls `panic!()` to signal an unrecoverable error
- Often used as a placeholder during development
- May indicate a logic error that "should never happen"

## How to Avoid

### Return a Result instead

```rust
fn process_data(data: &[u8]) -> Result<(), ProcessError> {
    if data.is_empty() {
        return Err(ProcessError::EmptyData);
    }
    // ...
    Ok(())
}
```

### Use Option for missing values

```rust
fn find_item(items: &HashMap<u32, Item>, id: u32) -> Option<Item> {
    // Return None instead of panicking
    items.get(&id).cloned()
}
```

### Document panic conditions

If panic is intentional (e.g., invariant violation), document it:

```rust
/// Processes the data buffer.
///
/// # Panics
///
/// Panics if `data` is empty.
fn process_data(data: &[u8]) {
    assert!(!data.is_empty(), "data must not be empty");
    // ...
}
```

## Jonesy Output

```text
 --> src/lib.rs:5:9 [explicit panic!() call]
     = help: Review if panic is intentional or add error handling
```

## Related

- [JP010 - Assert Failed](JP010-assert-failed.md): Conditional panic via `assert!()`
- [JP012 - Unreachable](JP012-unreachable.md): Panic for impossible code paths
