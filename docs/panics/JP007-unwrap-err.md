---
layout: default
title: "JP007: Unwrap Err"
---

# JP007: Unwrap Err

**Severity**: High
**Category**: Option/Result Handling

## Description

Calling `.unwrap()` on a `Result` that contains an `Err` variant.

## Example

```rust
fn read_config() -> Config {
    let content = std::fs::read_to_string("config.toml").unwrap();  // JP007
    parse_config(&content)
}
```

## Why It Happens

- I/O operation fails (file not found, permission denied)
- Network request fails
- Parse error on invalid input
- External service unavailable

## How to Avoid

### Propagate with `?`

```rust
fn read_config() -> Result<Config, Box<dyn Error>> {
    let content = std::fs::read_to_string("config.toml")?;
    Ok(parse_config(&content)?)
}
```

### Handle the error explicitly

```rust
fn read_config() -> Config {
    match std::fs::read_to_string("config.toml") {
        Ok(content) => parse_config(&content),
        Err(e) => {
            eprintln!("Warning: Could not read config: {e}");
            Config::default()
        }
    }
}
```

### Use `unwrap_or_else`

```rust
let content = std::fs::read_to_string("config.toml")
    .unwrap_or_else(|_| String::from("default = true"));
```

### Convert to Option

```rust
let content = std::fs::read_to_string("config.toml").ok();
```

## Jonesy Output

```text
 --> src/lib.rs:2:16 [unwrap() on Err]
     = help: Use if let, match, unwrap_or, or ? operator instead
```

## Related

- [JP006 - Unwrap None](/jonesy/panics/JP006-unwrap-none): `unwrap()` on `Option::None`
- [JP009 - Expect Err](/jonesy/panics/JP009-expect-err): `expect()` on `Err`
