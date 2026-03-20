---
layout: default
title: "JP010: Assert Failed"
---

# JP010: Assert Failed

**Severity**: Medium
**Category**: Explicit Panics

## Description

An `assert!()` or `assert_eq!()`/`assert_ne!()` macro evaluates to false.

## Example

```rust
fn withdraw(balance: &mut i32, amount: i32) {
    assert!(amount > 0, "Amount must be positive");
    assert!(*balance >= amount, "Insufficient funds");  // JP010
    *balance -= amount;
}
```

## Why It Happens

- Precondition violation
- Invariant broken
- Input validation failure
- Logic error in code

## How to Avoid

### Return Result for recoverable errors

```rust
fn withdraw(balance: &mut i32, amount: i32) -> Result<(), WithdrawError> {
    if amount <= 0 {
        return Err(WithdrawError::InvalidAmount);
    }
    if *balance < amount {
        return Err(WithdrawError::InsufficientFunds);
    }
    *balance -= amount;
    Ok(())
}
```

### Use debug_assert for development-only checks

```rust
fn process(data: &[u8]) {
    debug_assert!(!data.is_empty());  // Only checked in debug builds
    // ...
}
```

## When Assert is Appropriate

- Checking invariants that indicate bugs (not user errors)
- Validating assumptions in unsafe code
- Test assertions

```rust
// Good: invariant that should never be violated
fn binary_search(sorted: &[i32], target: i32) -> Option<usize> {
    debug_assert!(sorted.windows(2).all(|w| w[0] <= w[1]), "slice must be sorted");
    // ...
}
```

## Jonesy Output

```
 --> src/lib.rs:3:5 [assertion failed]
     = help: Review assertion condition
```

## Related

- [JP011 - Debug Assert](JP011-debug-assert-failed.md): Debug-only assertions
- [JP001 - Explicit Panic](JP001-explicit-panic.md): Direct panic call
