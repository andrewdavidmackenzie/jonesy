---
layout: default
title: "JP005: Division by Zero"
---

# JP005: Division by Zero

**Severity**: High
**Category**: Numeric Operations

## Description

An attempt to divide an integer by zero, or compute the remainder (modulo) with a zero divisor.

## Example

```rust
fn calculate_average(sum: i32, count: i32) -> i32 {
    sum / count  // JP005: panics if count == 0
}

fn main() {
    let avg = calculate_average(100, 0);  // Panic!
}
```

## Why It Happens

- Divisor comes from user input or external source
- Counter that hasn't been incremented yet
- Edge case in algorithm not handled
- Empty collection (length = 0)

## How to Avoid

### Check for zero before dividing

```rust
fn safe_average(sum: i32, count: i32) -> Option<i32> {
    if count != 0 {
        Some(sum / count)
    } else {
        None
    }
}
```

### Use checked division

```rust
fn safe_divide(a: i32, b: i32) -> Option<i32> {
    a.checked_div(b)  // Returns None if b == 0 or overflow
}
```

### Use NonZero types

```rust
use std::num::NonZeroI32;

fn divide(a: i32, b: NonZeroI32) -> i32 {
    a / b.get()  // Cannot be zero by construction
}
```

### Handle empty collections

```rust
fn average(numbers: &[i32]) -> Option<f64> {
    if numbers.is_empty() {
        return None;
    }
    let sum: i32 = numbers.iter().sum();
    Some(sum as f64 / numbers.len() as f64)
}
```

## Jonesy Output

```text
 --> src/lib.rs:3:5 [division by zero]
     = help: Check divisor is non-zero before division
```

## Related

- [JP003 - Arithmetic Overflow](/panics/JP003-arithmetic-overflow): `i32::MIN / -1` also overflows
