# Jonesy: "Don't Panic!"

Jonesy analyzes Rust binaries to find all code paths that can lead to a panic, helping developers understand where panics
can originate in their code.

Focus is currently on getting something useful working. I work on macOS and ARM64, so that's what implemented, but I
definitely want to make it
cross-platform and multi-architecture in the future, but will probably need help from others on Linux and Mac.

## Installation

```bash
cargo install --path jonesy
```

## Usage

### From a Crate Directory

Run jonesy from the root of any Rust crate (where `Cargo.toml` is located):

```bash
cd my-crate
cargo build
jonesy
```

Jonesy will parse `Cargo.toml` to find the package name and binary targets, then analyze all binaries found in
`target/debug/`.

### From a Workspace Root

When run from a workspace root, jonesy analyzes all workspace member binaries:

```bash
cd my-workspace
cargo build
jonesy
```

### Analyzing a Specific Binary

Use `--bin` to analyze a specific binary file:

```bash
jonesy --bin target/debug/my-binary
```

### Analyzing Libraries

Jonesy can analyze Rust libraries built as dynamic libraries (`.dylib`):

```bash
jonesy --lib target/debug/libmy_lib.dylib
```

**Library Setup Requirements:**

For jonesy to analyze a library, it must be built as a `cdylib` with exported symbols:

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

| Type     | Size   | `pub fn` exported?        | Analysis speed |
|----------|--------|---------------------------|----------------|
| `cdylib` | ~16KB  | No (needs `#[no_mangle]`) | Fast           |
| `dylib`  | ~1.4MB | Yes (automatic)           | Very slow      |

- **`cdylib`** creates a minimal C-compatible library. Only explicitly marked functions are exported; others are removed
  by dead code elimination. Analysis is fast because only your code is included.

- **`dylib`** creates a full Rust dynamic library including the standard library runtime. All `pub fn` are exported
  automatically, but the ~90x larger binary makes analysis impractical (minutes vs seconds).

**Other notes:**

- `.rlib` files (Rust static library archives) have limited support because panic symbols are unlinked references in
  object files
- The dSYM bundle provides debug symbols for source location information

## Command Line Options

```
Usage:
  jonesy[OPTIONS]
  jonesy[OPTIONS] --bin <path_to_binary>
  jonesy[OPTIONS] --lib <path_to_lib_object>

Options:
  --tree             Show full call tree instead of just crate code points
  --summary-only     Only show summary, not detailed panic points
  --config <path>    Path to a TOML config file for allow/deny rules
  --max-threads N    Maximum threads for parallel analysis (default: CPU count)
  --no-hyperlinks    Disable terminal hyperlinks (use plain absolute paths)
  --bin              Analyze a specific binary file
  --lib              Analyze a specific library object file
```

### `--tree`

By default, jonesy shows only the panic code points in your crate's source code. Use `--tree` to see the full call tree
from `rust_panic` up to your code:

```bash
jonesy --tree
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

### `--summary-only`

Show only the summary without detailed panic point locations. Useful for CI pipelines or quick checks:

```bash
jonesy --summary-only
```

Example output:

```text
Summary:
  Project: my-app
  Root: /path/to/project
  Panic points: 5 in 2 file(s)
```

### `--no-hyperlinks`

When stdout is a terminal, jonesy outputs source file locations
as [OSC 8 terminal hyperlinks](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda), making paths
clickable in supported terminals (iTerm2, Kitty, WezTerm, VS Code terminal, and others). The link points to the full
file path while displaying a shorter relative path.

When output is piped or redirected (e.g., `jonesy> file.txt`), plain absolute paths are used automatically to avoid
escape sequences in logs or files.

If your terminal doesn't support OSC 8 hyperlinks (e.g. macOS Terminal.app), the escape sequences will be invisible and
the output will still be readable. However, if you prefer plain absolute paths even in an interactive terminal, use this
flag:

```bash
jonesy --no-hyperlinks
```

This outputs paths like `/Users/me/project/src/main.rs:42:1` instead of clickable hyperlinks.

### `--config`

Specify a custom TOML configuration file for allow/deny rules:

```bash
jonesy --config my-config.toml
```

See the [Configuration](#configuration) section for details on the config file format.

## Configuration

Jonesy supports configuring which panic causes to report (deny) or suppress (allow). This is useful for:

- Suppressing known-acceptable panics in your codebase
- Enforcing stricter rules (e.g. reporting drop panics)
- Per-project customization

### Configuration Cascade

Configuration is loaded in order of precedence (later overrides earlier):

1. **Code defaults** - `drop` and `unwind` panics are allowed; all others are denied
2. **Cargo.toml** - `[package.metadata.jonesy]` section
3. **jonesy.toml** - Project root config file
4. **`--config`** - Command-line override

### Panic Cause Identifiers

| ID              | Description                               | Default     |
|-----------------|-------------------------------------------|-------------|
| `panic`         | Explicit `panic!()` calls                 | denied      |
| `bounds`        | Array/slice index out of bounds           | denied      |
| `overflow`      | Arithmetic overflow (add, sub, mul, etc.) | denied      |
| `div_zero`      | Division by zero                          | denied      |
| `unwrap`        | `unwrap()` on `None` or `Err`             | denied      |
| `expect`        | `expect()` on `None` or `Err`             | denied      |
| `assert`        | `assert!()` failures                      | denied      |
| `debug_assert`  | `debug_assert!()` failures                | denied      |
| `unreachable`   | `unreachable!()` reached                  | denied      |
| `unimplemented` | `unimplemented!()` reached                | denied      |
| `todo`          | `todo!()` reached                         | denied      |
| `drop`          | Panic during drop/cleanup                 | **allowed** |
| `unwind`        | Panic in no-unwind context                | **allowed** |
| `unknown`       | Unknown panic cause                       | denied      |

### jones.toml Format

Create a `jones.toml` file in your project root:

```toml
# Allow specific panic causes (suppress from output)
allow = ["drop", "unwind", "debug_assert"]

# Deny specific panic causes (report in output)
deny = ["todo", "unimplemented"]
```

### Cargo.toml Format

Add configuration to your `Cargo.toml` under `[package.metadata.jonesy]`:

```toml
[package]
name = "my-crate"
version = "0.1.0"

[package.metadata.jonesy]
allow = ["drop", "unwind"]
deny = ["todo"]
```

### Example: Strict Mode

To report all panic causes including drops:

```toml
# jonesy.toml
deny = ["drop", "unwind"]
```

### Example: Lenient Development Mode

To allow common development panics:

```toml
# jonesy.toml
allow = ["todo", "unimplemented", "debug_assert"]
```

## Exit Status

Jonesy exits with the number of panic code points found:

- `0` - No panics found (code "passed")
- `N` - N panic code points found

This makes it easy to use jonesy in CI pipelines:

```bash
jonesy || echo "Found potential panics!"
```

## Example Output

For a crate with multiple panic paths:

```text
Processing /path/to/target/debug/my-app
Using .dSYM bundle for debug info

Panic code points in crate:
 --> /path/to/src/main.rs:9:1 [explicit panic!() call]
     = help: Review if panic is intentional or add error handling
 --> /path/to/src/main.rs:13:1
     └──  --> /path/to/src/module/mod.rs:3:1
 --> /path/to/src/main.rs:16:1
     └──  --> /path/to/src/module/mod.rs:7:1 [unwrap() on None]
          = help: Use if let, match, unwrap_or, or ? operator instead

Summary:
  Project: my-app
  Root: /path/to
  Panic points: 5 in 2 file(s)
```

For a panic-free crate:

```text
Processing /path/to/target/debug/perfect
Using .dSYM bundle for debug info

No panics in crate

Summary:
  Project: perfect
  Root: /path/to
  Panic points: 0 in 0 file(s)
```

## Requirements

- macOS with ARM64 (Apple Silicon)—currently the only supported platform
- Debug symbols (build with `cargo build`, not release mode without debug info)

## Using on macOS

Jonesy needs DWARF debug information to map code addresses to source file locations. On macOS, Jonesy automatically
handles this for you:

### Automatic dSYM Generation

When no `.dSYM` bundle exists, Jonesy automatically runs `dsymutil` (if it is present) to generate one, if not it will
attempt (on macOS) to fall back to the "Debug Map" method.

in your project run:

```bash
cargo build
jonesy
```

Jonesy will output "Generated .dSYM bundle for debug info" when it creates one.

### Why is this needed?

By default, macOS Rust builds use Apple's "lazy" DWARF scheme:

- Debug info stays in object files (`target/debug/deps/*.o`)
- The final binary only contains a "debug map" pointing to those files
- `dsymutil` combines everything into a `.dSYM` bundle

Jonesy automatically runs `dsymutil` when needed, so you don't have to.

### Optional: Pre-generate dSYM in Cargo

If you want Cargo to create dSYM bundles during build (avoiding Jonesy's auto-generation), add to `Cargo.toml`:

```toml
[profile.dev]
split-debuginfo = "packed"
```

**Trade-off:** This slightly slows incremental builds because `dsymutil` runs on every build.

See [description.md](description.md) for detailed technical documentation.

## Limitations

1. **ARM64 only**: Currently only supports ARM64 binaries (uses `bl` instruction detection)
2. **Direct calls only**: Only detects direct function calls, not indirect calls through function pointers
3. **macOS/Mach-O**: Currently only supports Mach-O binaries with dSYM or embedded DWARF
4. **Debug builds recommended**: Optimized builds may inline functions, affecting accuracy

See [SCENARIOS.md](SCENARIOS.md) for detailed documentation of all analysis scenarios, supported panic types, and implementation status.
