---
layout: default
title: "JP006: Unwrap None"
---

# JP006: Unwrap None

**Severity**: High
**Category**: Option/Result Handling

## Description

Calling `.unwrap()` on an `Option` that contains `None`.

## Example

```rust
fn get_username(id: u32) -> String {
    let user = find_user(id);
    user.unwrap().name  // JP006: panics if user is None
}
```

## Why It Happens

- Assumption that a value will always be present
- Missing validation of input data
- Race condition or timing issue
- API contract violation

## How to Avoid

### Use `if let` or `match`

```rust
fn get_username(id: u32) -> Option<String> {
    if let Some(user) = find_user(id) {
        Some(user.name)
    } else {
        None
    }
}
```

### Use `?` operator

```rust
fn get_username(id: u32) -> Option<String> {
    let user = find_user(id)?;
    Some(user.name)
}
```

### Use `unwrap_or` / `unwrap_or_else`

```rust
let name = find_user(id)
    .map(|u| u.name)
    .unwrap_or_else(|| "Anonymous".to_string());
```

### Use `unwrap_or_default`

```rust
let count: i32 = maybe_count.unwrap_or_default();  // 0 if None
```

### Use `map` and combinators

```rust
let greeting = find_user(id)
    .map(|u| format!("Hello, {}!", u.name))
    .unwrap_or_else(|| "Hello, guest!".to_string());
```

## When Unwrap is Acceptable

```rust
// When you've just verified the value exists
if value.is_some() {
    process(value.unwrap());  // Safe, but prefer if let
}

// In tests
#[test]
fn test_parsing() {
    let result = parse("42").unwrap();  // Panic is the test failure
}

// When None is a programming error (document it)
/// Returns the cached value.
///
/// # Panics
/// Panics if called before `initialize()`.
fn get_cached() -> &Value {
    CACHE.get().unwrap()
}
```

## Jonesy Output

```text
 --> src/lib.rs:3:10 [unwrap() on None]
     = help: Use if let, match, unwrap_or, or ? operator instead
```

## Related

- [JP007 - Unwrap Err](/panics/JP007-unwrap-err): `unwrap()` on `Result::Err`
- [JP008 - Expect None](/panics/JP008-expect-none): `expect()` on `None`
