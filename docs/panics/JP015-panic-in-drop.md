---
layout: default
title: "JP015: Panic in Drop"
---

# JP015: Panic in Drop

**Severity**: Critical
**Category**: Runtime Errors

## Description

A panic occurred during the execution of a `Drop` implementation (destructor). This is particularly dangerous because it can cause a "double panic" which aborts the program.

## Example

```rust
struct FileHandler {
    file: File,
}

impl Drop for FileHandler {
    fn drop(&mut self) {
        self.file.sync_all().unwrap();  // JP015: panics if sync fails!
    }
}
```

## Why It's Dangerous

If a panic is already in progress and another panic occurs in a destructor, Rust will abort the entire process (not just the thread).

```rust
fn dangerous() {
    let handler = FileHandler::new();
    panic!("first panic");
}   // Drop runs here during unwinding -> double panic -> ABORT
```

## How to Avoid

### Never panic in Drop

```rust
impl Drop for FileHandler {
    fn drop(&mut self) {
        // Log errors but don't panic
        if let Err(e) = self.file.sync_all() {
            eprintln!("Warning: failed to sync file: {e}");
        }
    }
}
```

### Use explicit cleanup methods

```rust
impl FileHandler {
    /// Explicitly close and sync the file.
    /// Call this when you need to handle errors.
    pub fn close(mut self) -> io::Result<()> {
        self.file.sync_all()?;
        // Prevent Drop from running
        std::mem::forget(self);
        Ok(())
    }
}

impl Drop for FileHandler {
    fn drop(&mut self) {
        // Best-effort cleanup only
        let _ = self.file.sync_all();
    }
}
```

### Use catch_unwind sparingly

```rust
impl Drop for Resource {
    fn drop(&mut self) {
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            self.cleanup();
        }));
    }
}
```

## Jonesy Output

```text
 --> src/lib.rs:8:9 [panic during drop]
     = help: Avoid panicking in Drop - log errors instead
```

## Related

- [JP016 - Cannot Unwind](/jonesy/panics/JP016-cannot-unwind): FFI panic boundary
