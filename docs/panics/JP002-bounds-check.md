---
layout: default
title: "JP002: Bounds Check"
---

# JP002: Bounds Check (Index Out of Bounds)

**Severity**: High
**Category**: Memory and Indexing

## Description

An attempt to access an array, slice, or vector element at an index that is outside its valid range.

## Example

```rust
fn get_third_element(data: &[i32]) -> i32 {
    data[2]  // JP002: panics if data has fewer than 3 elements
}

fn main() {
    let numbers = vec![1, 2];
    let third = get_third_element(&numbers);  // Panic!
}
```

## Why It Happens

- Index is larger than or equal to the collection length
- Negative index (when using `usize`, this wraps to a large positive number)
- Off-by-one errors in loop bounds
- Using an index from external/untrusted input

## How to Avoid

### Use `.get()` for safe access

```rust
fn get_third_element(data: &[i32]) -> Option<i32> {
    data.get(2).copied()
}
```

### Validate index before use

```rust
fn get_element(data: &[i32], index: usize) -> Result<i32, &'static str> {
    if index < data.len() {
        Ok(data[index])
    } else {
        Err("index out of bounds")
    }
}
```

### Use iterators instead of indexing

```rust
// Instead of manual indexing
for i in 0..data.len() {
    process(data[i]);
}

// Use iterators
for item in data.iter() {
    process(*item);
}
```

### Use `.first()`, `.last()`, etc.

```rust
let first = data.first();      // Option<&T>
let last = data.last();        // Option<&T>
let (head, tail) = data.split_first()?;
```

## Jonesy Output

```
 --> src/lib.rs:3:5 [index out of bounds]
     = help: Use .get() for safe access or validate index before use
```

## Related

- [JP020 - String/Slice Error](JP020-string-slice-error.md): String indexing panics
