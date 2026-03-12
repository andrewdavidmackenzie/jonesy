# Jonesy Analysis Scenarios

This document defines all jonesy use cases, expected behavior for panic detection analysis, and their implementation status.

## What jonesy does

jonesy analyzes Rust binaries and libraries to find all sources of possible panics in user code, providing:
- Source file and line information
- Explanation of the panic cause
- Suggestions on how to avoid the panic

## Rust Library Types

Rust supports several library output types (crate-type), each with different characteristics that may affect analysis:

| Type | Description | Analysis Considerations |
|------|-------------|------------------------|
| **rlib** | Rust static library (default) | Contains Rust metadata + compiled code. Best for analysis - full symbol information available. |
| **dylib** | Rust dynamic library | Contains Rust metadata. Similar analysis capabilities to rlib. |
| **cdylib** | C-compatible dynamic library | No Rust metadata, only exported C-compatible symbols. Limited analysis - panic sources may be detected but Rust-specific context may be missing. |
| **staticlib** | C-compatible static library | No Rust metadata, designed for linking into non-Rust programs. Limited analysis similar to cdylib. |
| **proc-macro** | Procedural macro library | Runs at compile time, not in final binary. Panics in proc-macros manifest as compile errors, not runtime panics. May require different analysis approach or be out of scope. |

### Limitations by library type

- **rlib/dylib**: Full analysis capabilities when debug info is present
- **cdylib/staticlib**:
  - Rust metadata is stripped
  - Symbol names may be mangled differently (C-style)
  - Filtering to user crate code may be unreliable
  - May only detect panic symbols without full Rust context
- **proc-macro**:
  - Analysis may not be meaningful since panics occur at compile time
  - Consider excluding from analysis or flagging separately

## Detected Panic Types

| Panic Type | Status | Notes |
|------------|--------|-------|
| `panic!()` | Supported | Explicit panic calls |
| `unwrap()` on None | Supported | |
| `unwrap()` on Err | Supported | |
| `expect()` on None | Supported | |
| `expect()` on Err | Supported | |
| `unwrap_err()` on Ok | Supported | |
| `expect_err()` on Ok | Supported | |
| `assert!()` | Supported | |
| `debug_assert!()` | Supported | Debug builds only |
| `unreachable!()` | Supported | |
| `unimplemented!()` | Supported | |
| `todo!()` | Supported | |
| Division by zero | Supported | |
| Arithmetic overflow | Supported | Debug builds only |
| Shift overflow | Supported | Debug builds only |
| `assert_eq!()` | Not yet | See [#45](https://github.com/andrewdavidmackenzie/jonesy/issues/45) |
| `assert_ne!()` | Not yet | See [#45](https://github.com/andrewdavidmackenzie/jonesy/issues/45) |
| `debug_assert_eq!()` | Not yet | See [#45](https://github.com/andrewdavidmackenzie/jonesy/issues/45) |
| `debug_assert_ne!()` | Not yet | See [#45](https://github.com/andrewdavidmackenzie/jonesy/issues/45) |
| Slice index OOB | Not yet | See [#45](https://github.com/andrewdavidmackenzie/jonesy/issues/45) |
| String index panic | Not yet | See [#45](https://github.com/andrewdavidmackenzie/jonesy/issues/45) |

## Analysis Scenarios

### Scenario A: Standalone Analysis (no source available)

These scenarios are for when only a compiled binary or library is available, without access to source code.

#### Scenario 1: Binary with debug info (dev build)

| | |
|---|---|
| **Status** | Supported |
| **Context** | Only a linked binary is available (no source code), built with debug info |
| **Command** | `jonesy --bin /path/to/binary` |
| **Expected output** | Panic sources with source file paths, line numbers, cause explanations, and avoidance suggestions |
| **Limitations** | No source verification possible |

#### Scenario 2: Binary without debug info (release build)

| | |
|---|---|
| **Status** | Supported (limited) |
| **Context** | Only a release-built linked binary, minimal/no debug info |
| **Command** | `jonesy --bin /path/to/binary` |
| **Expected output** | Panic source symbols detected |
| **Limitations** | May not be able to: filter to only user crate code, demangle function names to readable form, provide source file/line numbers |

#### Scenario 3: Library with debug info (dev build)

| | |
|---|---|
| **Status** | Not yet implemented - see [#42](https://github.com/andrewdavidmackenzie/jonesy/issues/42) |
| **Context** | Only a compiled (not linked) library file, built with debug info |
| **Command** | `jonesy --lib /path/to/library.rlib` |
| **Expected output** | Panic sources with source file paths, line numbers, cause explanations, and avoidance suggestions |
| **Limitations** | No source verification possible |

#### Scenario 4: Library without debug info (release build)

| | |
|---|---|
| **Status** | Not yet implemented - see [#42](https://github.com/andrewdavidmackenzie/jonesy/issues/42) |
| **Context** | Only a release-built library, minimal/no debug info |
| **Command** | `jonesy --lib /path/to/library.rlib` |
| **Expected output** | Panic source symbols detected |
| **Limitations** | May not be able to: filter to only user crate code, demangle function names to readable form, provide source file/line numbers |

### Scenario B: Crate Analysis (source available)

These scenarios are for when jonesy is run within a crate directory with source code available.

#### Scenario 5: Single crate analysis

| | |
|---|---|
| **Status** | Supported |
| **Context** | Running in a crate root directory with `Cargo.toml` |
| **Command** | `jonesy` (no arguments) |
| **Expected behavior** | 1. Detect crate from `Cargo.toml`<br>2. Check for existing dev build in `target/`; trigger `cargo build` if missing/stale<br>3. Analyze all `[[bin]]` entries and `[lib]` entry<br>4. Produce individual reports per binary/library + aggregate crate report |
| **Expected output** | Full panic analysis: source file/line, cause explanations, avoidance suggestions |

#### Scenario 5a: Analyze specific binary in crate

| | |
|---|---|
| **Status** | Not yet implemented - see [#43](https://github.com/andrewdavidmackenzie/jonesy/issues/43) |
| **Context** | Crate with multiple binaries, user wants to analyze only one |
| **Command** | `jonesy --bin bin_name` |
| **Expected behavior** | Analyze only the named binary from `Cargo.toml` |
| **Note** | `bin_name` is the name in Cargo.toml, NOT a file path |

#### Scenario 5b: Analyze library only in crate

| | |
|---|---|
| **Status** | Not yet implemented - see [#43](https://github.com/andrewdavidmackenzie/jonesy/issues/43) |
| **Context** | Crate with both binaries and library, user wants library only |
| **Command** | `jonesy --lib` |
| **Expected behavior** | Analyze only the `[lib]` entry |

### Scenario C: Workspace Analysis (source available)

#### Scenario 6: Workspace analysis

| | |
|---|---|
| **Status** | Not yet implemented - see [#44](https://github.com/andrewdavidmackenzie/jonesy/issues/44) |
| **Context** | Running in a workspace root with `Cargo.toml` containing `[workspace]` |
| **Command** | `jonesy` (no arguments) |
| **Expected behavior** | 1. Detect workspace from `Cargo.toml`<br>2. Enumerate all crates from `members` table<br>3. Run Scenario 5 analysis on each member crate<br>4. Produce per-crate reports + aggregate workspace report |
| **Expected output** | Full panic analysis for all crates + workspace summary |
| **Note** | `--bin` and `--lib` options are NOT supported at workspace level. To analyze a specific binary or library, cd into the crate directory first. |

### Scenario D: Error Handling

#### Scenario 7: Invalid binary/library path

| | |
|---|---|
| **Status** | Supported |
| **Command** | `jonesy --bin /nonexistent/path` |
| **Expected behavior** | Clear error message indicating file not found |

#### Scenario 8: Not in a crate/workspace directory

| | |
|---|---|
| **Status** | Supported |
| **Command** | `jonesy` (in a directory without Cargo.toml) |
| **Expected behavior** | Clear error message indicating no Cargo.toml found |

#### Scenario 9: Build failure

| | |
|---|---|
| **Status** | Supported |
| **Context** | Source has compilation errors |
| **Command** | `jonesy` |
| **Expected behavior** | Report cargo build failure with error output |

## Test Examples

The `examples/` directory contains test fixtures for each scenario:

| Example | Type | Purpose |
|---------|------|---------|
| `panic` | Binary | All supported panic types |
| `perfect` | Binary | No panics (clean example) |
| `dylib` | Library (dylib) | Dynamic library with all panic types |
| `cdylib` | Library (cdylib + rlib) | C-compatible dynamic library |
| `rlib` | Library (rlib) | Pure Rust static library |
| `staticlib` | Library (staticlib) | C-compatible static library |
| `multi_bin` | Binary crate | Multiple binaries + library |
| `workspace_test` | Workspace | Nested workspace with 3 crates |

## Related Issues

- [#40](https://github.com/andrewdavidmackenzie/jonesy/issues/40) - Original issue defining all scenarios
- [#42](https://github.com/andrewdavidmackenzie/jonesy/issues/42) - Library-only analysis (rlib/staticlib)
- [#43](https://github.com/andrewdavidmackenzie/jonesy/issues/43) - Multi-binary crate support
- [#44](https://github.com/andrewdavidmackenzie/jonesy/issues/44) - Workspace analysis
- [#45](https://github.com/andrewdavidmackenzie/jonesy/issues/45) - Additional panic types
