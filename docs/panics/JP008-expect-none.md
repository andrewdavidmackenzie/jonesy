---
layout: default
title: "JP008: Expect None"
---

# JP008: Expect None

**Severity**: High
**Category**: Option/Result Handling

## Description

Calling `.expect()` on an `Option` that contains `None`. Similar to `unwrap()`, but includes a custom panic message.

## Example

```rust
fn get_env_port() -> u16 {
    std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .expect("PORT must be set")  // JP008: panics if None
}
```

## Why It Happens

Same reasons as [JP006 - Unwrap None](JP006-unwrap-none.md), but the programmer added a descriptive message expecting the panic to be "informative."

## How to Avoid

### Return Result with context

```rust
fn get_env_port() -> Result<u16, ConfigError> {
    let port_str = std::env::var("PORT")
        .map_err(|_| ConfigError::MissingEnvVar("PORT"))?;
    port_str.parse()
        .map_err(|_| ConfigError::InvalidPort(port_str))
}
```

### Use default value

```rust
fn get_env_port() -> u16 {
    std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080)  // Default to 8080
}
```

## When Expect is Acceptable

`expect()` is preferred over `unwrap()` when the panic message helps debugging:

```rust
// In initialization code where missing value is fatal
let db_url = std::env::var("DATABASE_URL")
    .expect("DATABASE_URL environment variable must be set");

// Document the invariant
/// # Panics
/// Panics if DATABASE_URL is not set.
```

## Jonesy Output

```text
 --> src/lib.rs:5:10 [expect() on None]
     = help: Use if let, match, unwrap_or, or ? operator instead
```

## Related

- [JP006 - Unwrap None](JP006-unwrap-none.md): `unwrap()` without message
- [JP009 - Expect Err](JP009-expect-err.md): `expect()` on `Result::Err`
