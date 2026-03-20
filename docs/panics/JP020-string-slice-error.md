---
layout: default
title: "JP020: String/Slice Error"
---

# JP020: String/Slice Error

**Severity**: High
**Category**: Memory and Indexing

## Description

An invalid operation on a string or slice, typically indexing into the middle of a UTF-8 character or invalid byte range.

## Example

```rust
fn first_three_bytes(s: &str) -> &str {
    &s[0..3]  // JP020: panics if 3rd byte is mid-character!
}

fn main() {
    let emoji = "Hello!";
    first_three_bytes(emoji);  // Panic!
}
```

## Why It Happens

- Indexing a `&str` at non-character boundaries
- Assuming ASCII when string contains UTF-8
- Using byte indices with multi-byte characters

## How to Avoid

### Use character iterators

```rust
fn first_n_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
```

### Use char_indices for safe slicing

```rust
fn truncate_at_char(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}
```

### Check boundaries with is_char_boundary

```rust
fn safe_slice(s: &str, start: usize, end: usize) -> Option<&str> {
    if s.is_char_boundary(start) && s.is_char_boundary(end) && start <= end {
        Some(&s[start..end])
    } else {
        None
    }
}
```

### Use get() for safe indexing

```rust
fn safe_slice(s: &str, range: std::ops::Range<usize>) -> Option<&str> {
    s.get(range)
}
```

### Work with bytes explicitly when needed

```rust
fn process_ascii(s: &str) -> Result<(), Error> {
    if !s.is_ascii() {
        return Err(Error::NonAsciiInput);
    }
    // Now safe to work with bytes
    for byte in s.as_bytes() {
        process_byte(*byte)?;
    }
    Ok(())
}
```

## Jonesy Output

```
 --> src/lib.rs:2:5 [string/slice error]
     = help: Use .get() or .chars() for safe string indexing
```

## Related

- [JP002 - Bounds Check](JP002-bounds-check.md): Array/vector indexing
