---
layout: default
title: "JP013: Unimplemented"
---

# JP013: Unimplemented

**Severity**: High
**Category**: Explicit Panics

## Description

The `unimplemented!()` macro was executed. This indicates incomplete code that hasn't been written yet.

## Example

```rust
trait Storage {
    fn save(&self, data: &[u8]) -> Result<(), Error>;
    fn load(&self, id: &str) -> Result<Vec<u8>, Error>;
}

impl Storage for CloudStorage {
    fn save(&self, data: &[u8]) -> Result<(), Error> {
        // TODO: implement cloud upload
        unimplemented!()  // JP013
    }

    fn load(&self, id: &str) -> Result<Vec<u8>, Error> {
        unimplemented!("cloud download not yet implemented")  // JP013
    }
}
```

## Why It Happens

- Feature not yet developed
- Placeholder during prototyping
- Trait method not applicable to this implementation

## How to Avoid

### Implement the functionality

The obvious solution: write the actual code.

### Return an error for unsupported operations

```rust
fn load(&self, id: &str) -> Result<Vec<u8>, Error> {
    Err(Error::NotSupported("cloud storage"))
}
```

### Use a default implementation

```rust
fn load(&self, _id: &str) -> Result<Vec<u8>, Error> {
    Ok(Vec::new())  // Return empty for unsupported
}
```

### Mark as deprecated or remove

If the feature won't be implemented, remove the dead code.

## Jonesy Output

```text
 --> src/cloud.rs:8:9 [unimplemented!() reached]
     = help: Review if panic is intentional or add error handling
```

## Related

- [JP014 - Todo](/panics/JP014-todo): Similar placeholder
- [JP012 - Unreachable](/panics/JP012-unreachable): Code that shouldn't run
