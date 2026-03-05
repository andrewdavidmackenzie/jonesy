# Jones: "Don't Panic!"

Jones analyzes Rust binaries to find all code paths that can lead to a panic, helping developers understand where panics can originate in their code.

## Installation

```bash
cargo install --path jones
```

## Usage

### From a Crate Directory

Run jones from the root of any Rust crate (where `Cargo.toml` is located):

```bash
cd my-crate
cargo build
jones
```

Jones will parse `Cargo.toml` to find the package name and binary targets, then analyze all binaries found in `target/debug/`.

### From a Workspace Root

When run from a workspace root, jones analyzes all workspace member binaries:

```bash
cd my-workspace
cargo build
jones
```

### Analyzing a Specific Binary

Use `--bin` to analyze a specific binary file:

```bash
jones --bin target/debug/my-binary
```

### Analyzing Libraries

Jones can analyze Rust libraries built as dynamic libraries (`.dylib`):

```bash
jones --lib target/debug/libmy_lib.dylib
```

**Library Setup Requirements:**

For jones to analyze a library, it must be built as a `cdylib` with exported symbols:

1. Add `cdylib` to your crate types in `Cargo.toml`:
   ```toml
   [lib]
   crate-type = ["rlib", "cdylib"]
   ```

2. Mark functions to export with `#[no_mangle]`:
   ```rust
   #[unsafe(no_mangle)]
   pub fn my_library_function() {
       // ...
   }
   ```

3. Build and create dSYM:
   ```bash
   cargo build
   dsymutil target/debug/libmy_lib.dylib -o target/debug/libmy_lib.dSYM
   ```

**Why `cdylib` + `#[no_mangle]`?**

There are two ways to build a Rust dynamic library:

| Type | Size | `pub fn` exported? | Analysis speed |
|------|------|-------------------|----------------|
| `cdylib` | ~16KB | No (needs `#[no_mangle]`) | Fast |
| `dylib` | ~1.4MB | Yes (automatic) | Very slow |

- **`cdylib`** creates a minimal C-compatible library. Only explicitly marked functions are exported; others are removed by dead code elimination. Analysis is fast because only your code is included.

- **`dylib`** creates a full Rust dynamic library including the standard library runtime. All `pub fn` are exported automatically, but the ~90x larger binary makes analysis impractical (minutes vs seconds).

**Other notes:**
- `.rlib` files (Rust static library archives) have limited support because panic symbols are unlinked references in object files
- The dSYM bundle provides debug symbols for source location information

## Command Line Options

```
Usage:
  jones [--tree] [--drops]
  jones [--tree] [--drops] --bin <path_to_binary>
  jones [--tree] [--drops] --lib <path_to_lib_object>

Options:
  --tree   Show full call tree instead of just crate code points
  --drops  Include panic paths from drop/cleanup operations
  --bin    Analyze a specific binary file
  --lib    Analyze a specific library object file
```

### `--tree`

By default, jones shows only the panic code points in your crate's source code. Use `--tree` to see the full call tree from `rust_panic` up to your code:

```bash
jones --tree
```

Example output with `--tree`:
```
Full call tree:
__rustc::rust_panic
Called from: 'panic_with_hook' (source: library/std/src/panicking.rs:796)
    Called from: '{closure#0}' (source: library/std/src/panicking.rs:698)
        ...
            Called from: 'panic_fmt' (source: library/core/src/panicking.rs:55)
                Called from: 'main' (source: src/main.rs:8)
```

### `--drops`

By default, jones excludes panic paths that occur during drop/cleanup operations (e.g., `panic_in_cleanup`, `panic_nounwind`). Use `--drops` to include these:

```bash
jones --drops
```

## Exit Status

Jones exits with the number of panic code points found:

- `0` - No panics found (code "passed")
- `N` - N panic code points found

This makes it easy to use jones in CI pipelines:

```bash
jones || echo "Found potential panics!"
```

## Example Output

For a crate with multiple panic paths:

```
Processing target/debug/my-app
Using .dSYM bundle for debug info

Panic code points in crate:
  src/main.rs:8 in 'main'
  src/main.rs:10 in 'main'
  src/module/mod.rs:2 in 'cause_a_panic'
```

For a panic-free crate:

```
Processing target/debug/perfect
Using .dSYM bundle for debug info

No panics in crate
```

## Requirements

- macOS with ARM64 (Apple Silicon) - currently the only supported platform
- Debug symbols (build with `cargo build`, not release mode without debug info)
- dSYM bundle recommended for best results (`dsymutil` creates these)

## Limitations

1. **ARM64 only**: Currently only supports ARM64 binaries (uses `bl` instruction detection)
2. **Direct calls only**: Only detects direct function calls, not indirect calls through function pointers
3. **macOS/Mach-O**: Currently only supports Mach-O binaries with dSYM or embedded DWARF
4. **Debug builds recommended**: Optimized builds may inline functions, affecting accuracy
