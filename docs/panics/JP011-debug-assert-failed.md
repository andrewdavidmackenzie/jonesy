---
layout: default
title: "JP011: Debug Assert Failed"
---

# JP011: Debug Assert Failed

**Severity**: Low (debug only)
**Category**: Explicit Panics

## Description

A `debug_assert!()`, `debug_assert_eq!()`, or `debug_assert_ne!()` macro evaluates to false. These only run in debug builds.

## Example

```rust
fn get_cached_value(cache: &Cache, key: &str) -> &Value {
    debug_assert!(cache.contains(key), "Key should be pre-loaded");  // JP011
    cache.get(key).unwrap()
}
```

## Behavior

| Build Mode | Assertion Checked? |
|------------|-------------------|
| Debug (`cargo build`) | Yes |
| Release (`cargo build --release`) | No |
| Test (`cargo test`) | Yes |

## Why It Happens

- Precondition violated during development
- Helps catch bugs early without runtime cost in production

## How to Avoid

### Verify the condition is always true

Debug assertions should check invariants, not handle expected cases.

### Use regular assert for critical checks

```rust
// If this must be checked in production too:
assert!(index < len, "index out of bounds");
```

### Remove if the check is obsolete

If the assertion no longer makes sense, remove it rather than leaving dead code.

## Jonesy Output

```text
 --> src/lib.rs:3:5 [debug assertion failed]
     = help: Review assertion condition
```

## Related

- [JP010 - Assert Failed](/panics/JP010-assert-failed): Always-on assertions
