---
layout: default
title: "JP014: Todo"
---

# JP014: Todo

**Severity**: High
**Category**: Explicit Panics

## Description

The `todo!()` macro was executed. This is a placeholder indicating work that needs to be done.

## Example

```rust
fn calculate_tax(income: f64, region: Region) -> f64 {
    match region {
        Region::US => income * 0.25,
        Region::EU => todo!("implement EU tax calculation"),  // JP014
        Region::Asia => todo!(),  // JP014
    }
}
```

## Why It Happens

- Development in progress
- Quick prototyping with planned follow-up
- Skeleton code awaiting implementation

## How to Avoid

### Complete the implementation

```rust
fn calculate_tax(income: f64, region: Region) -> f64 {
    match region {
        Region::US => income * 0.25,
        Region::EU => income * 0.20,  // Implemented!
        Region::Asia => income * 0.15,
    }
}
```

### Return a meaningful default or error

```rust
fn calculate_tax(income: f64, region: Region) -> Result<f64, TaxError> {
    match region {
        Region::US => Ok(income * 0.25),
        _ => Err(TaxError::UnsupportedRegion(region)),
    }
}
```

### Use feature flags

```rust
fn calculate_tax(income: f64, region: Region) -> f64 {
    match region {
        Region::US => income * 0.25,
        #[cfg(feature = "international")]
        Region::EU => income * 0.20,
        #[cfg(not(feature = "international"))]
        _ => panic!("Enable 'international' feature for non-US regions"),
    }
}
```

## Jonesy Output

```
 --> src/tax.rs:5:21 [todo!() reached]
     = help: Review if panic is intentional or add error handling
```

## Related

- [JP013 - Unimplemented](JP013-unimplemented.md): Similar placeholder
- [JP012 - Unreachable](JP012-unreachable.md): Code that shouldn't run
