# Jones Technical Documentation

## macOS Debug Info Cases

On macOS, Rust/Cargo uses Apple's "lazy" DWARF scheme by default. This section documents the debug info configurations and how Jones handles them.

### The Problem

By default, macOS Rust builds do not embed DWARF debug info in the final binary:

1. **Object files (`.o`)** in `target/debug/deps/` contain full DWARF debug info
2. **Final binary** contains only a "debug map" - stab entries (`OSO`, `SO`) pointing to object files
3. **No embedded DWARF** in the final binary itself

This is why Jones requires a `.dSYM` bundle or the `dsymutil` step - the binary alone doesn't contain the debug information needed to map addresses to source locations.

### Debug Map Explained

Apple's solution segregates executable linking and debug info linking into two separate actions:

- The linker produces `OSO` stab entries (like `SO` but for object files) that point to where object files are located
- The `SO` stabs tell debuggers what source file corresponds to each object
- Object file paths include modification timestamps to detect stale debug info
- Every binary is stamped with a 128-bit UUID (`LC_UUID`) that's copied into the dSYM for verification

Debuggers like `lldb` can either:
1. Read DWARF from a `.dSYM` bundle (addresses already remapped by `dsymutil`)
2. Read DWARF from `.o` files and perform address translation on-the-fly

### Three Debug Info Configurations

| Configuration | Where debug info lives | `dsymutil` needed? | Build speed |
|--------------|----------------------|-------------------|-------------|
| `split-debuginfo = "unpacked"` (default) | Object files in `target/debug/deps/*.o` | Yes | Fast |
| `split-debuginfo = "packed"` | `.dSYM` bundle created automatically | No | Slower |
| Manual `dsymutil <binary>` step | `.dSYM` bundle | Already run | N/A |

### Configuration Options

#### Option 1: Use `split-debuginfo = "packed"`

Add to `Cargo.toml`:

```toml
[profile.dev]
split-debuginfo = "packed"
```

Or add to `~/.cargo/config.toml` for all projects:

```toml
[profile.dev]
split-debuginfo = "packed"

[profile.test]
split-debuginfo = "packed"
```

This automatically creates `.dSYM` bundles during build, eliminating the manual `dsymutil` step. However, it slows incremental builds because `dsymutil` runs on every build.

#### Option 2: Manual `dsymutil` Step

Run after building:

```bash
cargo build
dsymutil target/debug/my-binary -o target/debug/my-binary.dSYM
```

This is faster for development since you only create the dSYM when needed for analysis.

#### Option 3: Default (unpacked) with Debug Map Reading

Currently not supported by Jones, but debuggers like `lldb` can read debug info directly from object files using the debug map in the binary. This would eliminate all extra steps.

### Profile Options Affecting Debug Info

| Profile Setting | Effect |
|----------------|--------|
| `debug = true` | Include debug info (default for dev profile) |
| `debug = false` | No debug info generated |
| `debug = "line-tables-only"` | Minimal debug info (file/line only, no variables) |
| `debug = 2` | Full debug info (same as `true`) |
| `split-debuginfo = "off"` | Embed debug info in binary (not typical on macOS) |
| `split-debuginfo = "unpacked"` | Keep in object files (macOS default) |
| `split-debuginfo = "packed"` | Create `.dSYM` bundle automatically |

### Current Jones Behavior

Jones looks for debug info in this order:

1. **dSYM bundle** at `<binary>.dSYM/Contents/Resources/DWARF/<binary_name>`
2. **Embedded DWARF** in the binary itself (`.debug_info` section)
3. **Auto-generate dSYM** by running `dsymutil` automatically
4. **Debug map fallback** - if `dsymutil` is not available or fails, Jones reads DWARF directly from object files referenced in the binary's debug map
5. **Falls back** to symbol table only (no source locations)

The auto-generation of dSYM bundles means Jones "just works" with default Cargo builds - no manual `dsymutil` step required. If `dsymutil` is unavailable, the debug map fallback provides partial functionality (source locations may be less accurate).

### Future Improvements

Potential enhancements for Jones:

1. **Read debug map directly** - Parse `OSO`/`SO` stabs and read DWARF from object files, like `lldb` does. This would avoid the `dsymutil` step entirely, though it requires complex address translation and parsing of `.rlib` archives.

### Sources

- [Profiles - The Cargo Book](https://doc.rust-lang.org/cargo/reference/profiles.html)
- [Apple's "Lazy" DWARF Scheme - DWARF Wiki](https://wiki.dwarfstd.org/Apple's_%22Lazy%22_DWARF_Scheme.md)
- [dsymutil - LLVM Documentation](https://llvm.org/docs/CommandGuide/dsymutil.html)
- [Add split-debuginfo profile option - Cargo PR #9112](https://github.com/rust-lang/cargo/pull/9112)
- [Reducing Rust Incremental Compilation Times on macOS by 70%](https://jacobdeichert.ca/blog/reducing-rust-incremental-compilation-times-on-macos-by-70-percent/)
- [Missing dSYM on macos - Rust Forum](https://users.rust-lang.org/t/missing-dsym-on-macos-when-building-with-cargo/97543)
