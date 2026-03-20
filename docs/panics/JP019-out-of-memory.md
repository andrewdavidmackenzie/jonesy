---
layout: default
title: "JP019: Out of Memory"
---

# JP019: Out of Memory

**Severity**: Critical
**Category**: Resource Exhaustion

## Description

Memory allocation failed because the system ran out of available memory.

## Example

```rust
fn load_entire_file(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()  // JP019 if file is larger than RAM
}
```

## Why It Happens

- Allocating more memory than available
- Memory leak exhausting resources
- Many concurrent allocations
- Large file loaded entirely into memory

## How to Avoid

### Stream large data

```rust
fn process_file(path: &Path) -> io::Result<()> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        process_line(&line?)?;
    }
    Ok(())
}
```

### Use fallible allocation

```rust
fn try_allocate(size: usize) -> Result<Vec<u8>, TryReserveError> {
    let mut v = Vec::new();
    v.try_reserve(size)?;
    v.resize(size, 0);
    Ok(v)
}
```

### Set resource limits

```rust
const MAX_REQUEST_SIZE: usize = 10 * 1024 * 1024;  // 10 MB

fn read_request(stream: &mut TcpStream) -> Result<Vec<u8>, Error> {
    let size = read_size(stream)?;
    if size > MAX_REQUEST_SIZE {
        return Err(Error::RequestTooLarge);
    }
    // Now safe to allocate
    let mut buf = vec![0u8; size];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}
```

### Use memory-mapped files

```rust
use memmap2::Mmap;

fn process_large_file(path: &Path) -> io::Result<()> {
    let file = File::open(path)?;
    let mmap = unsafe { Mmap::map(&file)? };
    // Process without loading entire file
    for chunk in mmap.chunks(4096) {
        process_chunk(chunk)?;
    }
    Ok(())
}
```

## Jonesy Output

```text
 --> src/lib.rs:2:5 [out of memory]
     = help: Use streaming, fallible allocation, or resource limits
```

## Related

- [JP018 - Capacity Overflow](/jonesy/panics/JP018-capacity-overflow): Size calculation overflow
