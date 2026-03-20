---
layout: default
title: "JP009: Expect Err"
---

# JP009: Expect Err

**Severity**: High
**Category**: Option/Result Handling

## Description

Calling `.expect()` on a `Result` that contains an `Err` variant.

## Example

```rust
fn load_config() -> Config {
    let file = File::open("app.conf")
        .expect("Failed to open config file");  // JP009
    serde_json::from_reader(file)
        .expect("Failed to parse config")        // JP009
}
```

## How to Avoid

See [JP007 - Unwrap Err](/jonesy/panics/JP007-unwrap-err) for detailed solutions.

### Quick fix: propagate with `?`

```rust
fn load_config() -> Result<Config, Box<dyn Error>> {
    let file = File::open("app.conf")?;
    let config = serde_json::from_reader(file)?;
    Ok(config)
}
```

## Jonesy Output

```text
 --> src/lib.rs:3:10 [expect() on Err]
     = help: Use if let, match, unwrap_or, or ? operator instead
```

## Related

- [JP007 - Unwrap Err](/jonesy/panics/JP007-unwrap-err): `unwrap()` without message
- [JP008 - Expect None](/jonesy/panics/JP008-expect-none): `expect()` on `Option::None`
