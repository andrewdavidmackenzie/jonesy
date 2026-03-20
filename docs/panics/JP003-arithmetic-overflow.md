---
layout: default
title: "JP003: Arithmetic Overflow"
---

# JP003: Arithmetic Overflow

**Severity**: Medium
**Category**: Numeric Operations

## Description

An arithmetic operation (addition, subtraction, multiplication, division, or negation) produces a result that cannot be represented in the target integer type.

**Note**: By default, overflow only panics in debug builds. Release builds wrap silently unless `overflow-checks = true` is set in Cargo.toml.

## Example

```rust
fn double(x: u8) -> u8 {
    x * 2  // JP003: panics in debug if x > 127
}

fn main() {
    let result = double(200);  // Panic in debug!
}
```

## Why It Happens

- Adding to a value near `MAX` for the type
- Subtracting from a value near `MIN` (or 0 for unsigned)
- Multiplying large numbers
- Negating `MIN` for signed types (e.g., `-i8::MIN` overflows)

## How to Avoid

### Use checked operations

```rust
fn safe_double(x: u8) -> Option<u8> {
    x.checked_mul(2)
}
```

### Use saturating operations

```rust
fn capped_double(x: u8) -> u8 {
    x.saturating_mul(2)  // Returns 255 if overflow
}
```

### Use wrapping operations (explicit wrap)

```rust
fn wrapping_double(x: u8) -> u8 {
    x.wrapping_mul(2)  // Wraps around on overflow
}
```

### Use a larger type

```rust
fn safe_double(x: u8) -> u16 {
    (x as u16) * 2  // Cannot overflow
}
```

### Enable overflow checks in release

```toml
# Cargo.toml
[profile.release]
overflow-checks = true
```

## Operations and Their Checked Variants

| Operation | Checked | Saturating | Wrapping |
|-----------|---------|------------|----------|
| `+` | `checked_add` | `saturating_add` | `wrapping_add` |
| `-` | `checked_sub` | `saturating_sub` | `wrapping_sub` |
| `*` | `checked_mul` | `saturating_mul` | `wrapping_mul` |
| `/` | `checked_div` | `saturating_div` | `wrapping_div` |
| `%` | `checked_rem` | - | `wrapping_rem` |
| `-x` | `checked_neg` | `saturating_neg` | `wrapping_neg` |

## Jonesy Output

```text
 --> src/lib.rs:3:5 [arithmetic overflow]
     = help: Use checked_*, saturating_*, or wrapping_* methods
     = warning: With default release settings (overflow-checks=false), this wraps silently
```

## Related

- [JP004 - Shift Overflow](JP004-shift-overflow.md): Bit shift overflow
- [JP005 - Division by Zero](JP005-division-by-zero.md): Division edge case
