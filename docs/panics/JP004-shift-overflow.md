---
layout: default
title: "JP004: Shift Overflow"
---

# JP004: Shift Overflow

**Severity**: Medium
**Category**: Numeric Operations

## Description

A bit shift operation (`<<` or `>>`) where the shift amount is greater than or equal to the number of bits in the type.

**Note**: Like arithmetic overflow, this only panics in debug builds by default.

## Example

```rust
fn shift_left(x: u32, amount: u32) -> u32 {
    x << amount  // JP004: panics if amount >= 32
}

fn main() {
    let result = shift_left(1, 32);  // Panic in debug!
}
```

## Why It Happens

- Shift amount comes from external input without validation
- Off-by-one error (shifting by bit width instead of bit width - 1)
- Type mismatch (e.g., shifting a u8 by a u32 value)

## How to Avoid

### Validate shift amount

```rust
fn safe_shift_left(x: u32, amount: u32) -> Option<u32> {
    if amount < 32 {
        Some(x << amount)
    } else {
        None
    }
}
```

### Use checked shifts

```rust
fn safe_shift(x: u32, amount: u32) -> Option<u32> {
    x.checked_shl(amount)
}
```

### Use wrapping or saturating shifts

```rust
// Wrapping: shifts by amount % bit_width
let result = x.wrapping_shl(amount);

// Overflowing: returns (result, did_overflow)
let (result, overflow) = x.overflowing_shl(amount);
```

## Jonesy Output

```text
 --> src/lib.rs:3:5 [shift overflow]
     = help: Validate shift amount is within valid range
     = warning: With default release settings (overflow-checks=false), this wraps silently
```

## Related

- [JP003 - Arithmetic Overflow](/panics/JP003-arithmetic-overflow): Other numeric overflow
