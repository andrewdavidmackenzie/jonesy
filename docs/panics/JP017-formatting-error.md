---
layout: default
title: "JP017: Formatting Error"
---

# JP017: Formatting Error

**Severity**: Medium
**Category**: Runtime Errors

## Description

A panic occurred during string formatting, typically in `format!()`, `write!()`, `println!()`, or a `Display`/`Debug` implementation.

## Example

```rust
impl Display for MyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // This panics if inner.fmt fails and we unwrap
        write!(f, "MyType: {}", self.inner)?;
        self.optional.as_ref().unwrap().fmt(f)  // JP017!
    }
}
```

## Why It Happens

- `Display` or `Debug` implementation panics
- Format string and arguments mismatch (rare in safe Rust)
- Recursive formatting causes stack overflow
- I/O error during `write!` to a `Write` impl that panics

## How to Avoid

### Never panic in Display/Debug

```rust
impl Display for MyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MyType: {}", self.inner)?;
        if let Some(opt) = &self.optional {
            write!(f, ", optional: {}", opt)?;
        }
        Ok(())
    }
}
```

### Handle write errors

```rust
fn log_value(value: &impl Display) {
    // Use if let instead of unwrap
    if let Err(e) = writeln!(std::io::stderr(), "{}", value) {
        // Fallback logging
    }
}
```

### Use format_args! carefully

```rust
// Safe: compile-time checked
println!("{} + {} = {}", 1, 2, 3);

// Avoid: runtime format strings
// let fmt = get_format_string();  // Could be invalid
```

## Jonesy Output

```text
 --> src/lib.rs:5:9 [formatting error]
     = help: Review Display/Debug implementations for panic paths
```

## Related

- [JP001 - Explicit Panic](/panics/JP001-explicit-panic): Direct panic in formatter
