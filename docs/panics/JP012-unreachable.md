---
layout: default
title: "JP012: Unreachable"
---

# JP012: Unreachable

**Severity**: High
**Category**: Explicit Panics

## Description

The `unreachable!()` macro was executed. This indicates code that was expected to never run has been reached.

## Example

```rust
fn sign(x: i32) -> &'static str {
    if x > 0 {
        "positive"
    } else if x < 0 {
        "negative"
    } else if x == 0 {
        "zero"
    } else {
        unreachable!()  // JP012: mathematically impossible
    }
}
```

## Why It Happens

- Logic error made the "impossible" possible
- Match arm expected to be unreachable was reached
- Code refactoring invalidated assumptions

## How to Avoid

### Use exhaustive patterns

```rust
fn sign(x: i32) -> &'static str {
    match x.cmp(&0) {
        Ordering::Greater => "positive",
        Ordering::Less => "negative",
        Ordering::Equal => "zero",
    }
}
```

### Return a default or error

```rust
fn process_status(code: u8) -> Result<Action, Error> {
    match code {
        0 => Ok(Action::Success),
        1 => Ok(Action::Retry),
        _ => Err(Error::UnknownStatus(code)),  // Handle unknown
    }
}
```

### Use `unreachable_unchecked` for performance-critical code

```rust
// SAFETY: We've verified this branch cannot be taken
unsafe { std::hint::unreachable_unchecked() }
```

## When Unreachable is Appropriate

```rust
// After exhaustive external validation
let digit = char.to_digit(10).unwrap();
match digit {
    0..=9 => process(digit),
    _ => unreachable!("to_digit(10) only returns 0-9"),
}
```

## Jonesy Output

```text
 --> src/lib.rs:10:9 [unreachable!() reached]
     = help: Review if panic is intentional or add error handling
```

## Related

- [JP013 - Unimplemented](/panics/JP013-unimplemented): Placeholder for missing code
- [JP014 - Todo](/panics/JP014-todo): Placeholder for future work
