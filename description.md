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

## Parallel Analysis Architecture

Jones uses parallel processing to achieve ~8x speedup on multi-core systems. This section explains how the parallelization works.

### The Challenge

Analyzing a binary for panic paths involves two expensive operations:

1. **CallGraph Construction**: Scanning millions of ARM64 instructions to find all `bl` (branch-and-link) calls and mapping each call site to its containing function
2. **Call Tree Exploration**: Recursively finding all callers of the panic symbol, building a tree that can be hundreds of levels deep

Both operations are O(n) where n is the number of instructions or call edges, and for large binaries like Rust dylibs (which include the standard library), this can mean processing millions of instructions.

### Solution: Two-Phase Parallelization

#### Phase 1: Parallel CallGraph Construction

Instead of querying callers on-demand (which would scan all instructions for each query), Jones pre-computes a complete call graph:

```
┌─────────────────────────────────────────────────────────────┐
│                    Disassembly (Sequential)                  │
│  Capstone disassembles all instructions from __TEXT segment │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│               Extract BL Instructions (Sequential)          │
│  Filter to only branch-and-link instructions with targets   │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│              Parallel Instruction Processing                 │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐        │
│  │Thread 1 │  │Thread 2 │  │Thread 3 │  │Thread N │        │
│  │ BL @ A  │  │ BL @ B  │  │ BL @ C  │  │ BL @ Z  │        │
│  │   │     │  │   │     │  │   │     │  │   │     │        │
│  │   ▼     │  │   ▼     │  │   ▼     │  │   ▼     │        │
│  │ Lookup  │  │ Lookup  │  │ Lookup  │  │ Lookup  │        │
│  │Function │  │Function │  │Function │  │Function │        │
│  │+ DWARF  │  │+ DWARF  │  │+ DWARF  │  │+ DWARF  │        │
│  └────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘        │
│       │            │            │            │              │
│       └────────────┴────────────┴────────────┘              │
│                         │                                    │
│                         ▼                                    │
│              ┌─────────────────────┐                        │
│              │  DashMap (thread-   │                        │
│              │  safe concurrent    │                        │
│              │  hash map)          │                        │
│              │                     │                        │
│              │  target_addr →      │                        │
│              │    [CallerInfo...]  │                        │
│              └─────────────────────┘                        │
└─────────────────────────────────────────────────────────────┘
```

**Key insight**: The expensive work is looking up which function contains each call site (binary search through symbol table) and enriching with DWARF debug info (source file/line). These lookups are independent and parallelize perfectly.

**Data structures**:
- `DashMap<u64, Vec<CallerInfo>>`: A concurrent hash map from the `dashmap` crate that allows lock-free concurrent insertions
- Each entry maps a call target address to all the functions that call it

#### Phase 2: Parallel Call Tree Building

Once the CallGraph is built, finding callers is O(1). The tree building parallelizes the exploration of independent branches:

```
                        panic_function
                              │
              ┌───────────────┼───────────────┐
              │               │               │
           caller_A        caller_B        caller_C
              │               │               │
         ┌────┴────┐     ┌────┴────┐     ┌────┴────┐
         │         │     │         │     │         │
      (Thread 1) (T1)  (Thread 2)(T2)  (Thread 3)(T3)
         │         │     │         │     │         │
       (...recursive exploration within each branch...)
```

**Strategy**:
- Top-level callers of the panic function are explored **in parallel** using rayon's work-stealing thread pool
- Within each branch, recursion is **sequential** to ensure deterministic results
- A **DashSet** (concurrent hash set) tracks visited addresses to prevent cycles

**Why sequential within branches?**: The visited set must be consistent. If we allowed fully parallel recursion, different threads exploring different paths might reach the same node simultaneously, leading to race conditions and potentially different (but still correct) tree structures on each run. Sequential recursion within a branch ensures deterministic output.

### Thread Configuration

The `--max-threads N` option configures rayon's global thread pool:

```rust
rayon::ThreadPoolBuilder::new()
    .num_threads(max_threads)
    .build_global()
```

- **Default**: Number of logical CPUs (e.g., 10 on M1 Pro)
- **Minimum**: 1 (fully sequential execution)
- **Use case**: Limit parallelism when running alongside other processes

### Performance Characteristics

| Metric | Single-threaded | Parallel (10 cores) |
|--------|-----------------|---------------------|
| CPU utilization | ~100% (1 core) | ~1000% (all cores) |
| Wall-clock time | Baseline | ~8x faster |
| Memory | Baseline | ~1.2x (concurrent data structures) |

The speedup is nearly linear with core count because:
1. Instruction processing is embarrassingly parallel (no dependencies)
2. DashMap/DashSet have minimal contention (sharded locking)
3. Work-stealing balances load across threads

### Implementation Details

**Dependencies**:
- `rayon 1.10`: Work-stealing parallel iterators
- `dashmap 6`: Lock-free concurrent HashMap/HashSet

**Key functions**:
- `CallGraph::build()` / `CallGraph::build_with_debug_info()`: Parallel instruction processing
- `build_call_tree_parallel()`: Parallel top-level exploration
- `build_call_tree_sequential()`: Sequential recursion within branches

### Sources

- [Profiles - The Cargo Book](https://doc.rust-lang.org/cargo/reference/profiles.html)
- [Apple's "Lazy" DWARF Scheme - DWARF Wiki](https://wiki.dwarfstd.org/Apple's_%22Lazy%22_DWARF_Scheme.md)
- [dsymutil - LLVM Documentation](https://llvm.org/docs/CommandGuide/dsymutil.html)
- [Add split-debuginfo profile option - Cargo PR #9112](https://github.com/rust-lang/cargo/pull/9112)
- [Reducing Rust Incremental Compilation Times on macOS by 70%](https://jacobdeichert.ca/blog/reducing-rust-incremental-compilation-times-on-macos-by-70-percent/)
- [Missing dSYM on macos - Rust Forum](https://users.rust-lang.org/t/missing-dsym-on-macos-when-building-with-cargo/97543)

## Panic Cause Detection

Jones identifies the **cause** of potential panics by analyzing function names in the call chain between the panic symbol and user code. This allows it to provide helpful descriptions and suggestions.

### How It Works

When Rust code panics, it follows a well-defined call path through the standard library:

```
User code (e.g., array[index])
    ↓
Panic helper (e.g., panic_bounds_check)
    ↓
panic_fmt
    ↓
rust_panic
```

Jones walks this call tree from `rust_panic` upward, examining each function name for known patterns that indicate specific panic causes.

### Pattern Matching

The `detect_panic_cause()` function in `panic_cause.rs` matches function names against known patterns:

| Function Pattern | Detected Cause | Description |
|-----------------|----------------|-------------|
| `panic_bounds_check` | BoundsCheck | Index out of bounds |
| `panic_const_add_overflow` | ArithmeticOverflow | Addition overflow |
| `panic_const_sub_overflow` | ArithmeticOverflow | Subtraction overflow |
| `panic_const_mul_overflow` | ArithmeticOverflow | Multiplication overflow |
| `panic_const_div_overflow` | ArithmeticOverflow | Division overflow |
| `panic_const_shl_overflow` | ShiftOverflow | Left shift overflow |
| `panic_const_shr_overflow` | ShiftOverflow | Right shift overflow |
| `panic_const_div_by_zero` | DivisionByZero | Division by zero |
| `panic_const_rem_by_zero` | DivisionByZero | Remainder by zero |
| `unwrap_failed` | UnwrapNone | unwrap() on None |
| `expect_failed` | ExpectNone | expect() on None |
| `assert_failed` | AssertFailed | Assertion failed |
| `panic_display` | ExplicitPanic | Explicit panic!() call |
| `panic_fmt` (fallback) | ExplicitPanic | Explicit panic!() call |

### Cause Propagation

Once a cause is detected at a node in the call tree, it propagates up through the `collect_crate_relationships()` function:

```rust
let detected_cause = detect_panic_cause(&node.name).or(current_cause);
```

This means:
1. If the current node's function name matches a pattern, use that cause
2. Otherwise, inherit the cause from the child node (closer to panic)
3. The cause propagates until it reaches user code (leaf nodes)

### Output Display

Causes are displayed only on **leaf nodes** (the actual user code lines that lead to panics):

```
Panic code points in crate:
 --> examples/array_access/src/main.rs:14:1 [index out of bounds]
     = help: Use .get() for safe access or validate index before use
```

Intermediate nodes in the call hierarchy don't show causes since the actual panic source is at the leaf.

### Suggestions

Each panic cause includes a help suggestion:

| Cause | Suggestion |
|-------|------------|
| Explicit panic | Review if panic is intentional or add error handling |
| Index out of bounds | Use .get() for safe access or validate index before use |
| Arithmetic overflow | Use checked_*, saturating_*, or wrapping_* methods |
| Division by zero | Check divisor is non-zero before division |
| unwrap()/expect() on None | Use if let, match, unwrap_or, or ? operator instead |
| Assertion failed | Review assertion condition |

### Implementation

The panic cause detection is implemented in `jones/src/panic_cause.rs`:

- `PanicCause` enum: Defines all known panic causes
- `detect_panic_cause()`: Pattern matching on function names
- `PanicCause::description()`: Human-readable cause description
- `PanicCause::suggestion()`: Actionable help text

### Limitations

- **Symbol availability**: Cause detection requires function names to be present in the binary. Release builds with stripped symbols may not have these helper function names visible.
- **Inlining**: Aggressive inlining in release builds may eliminate the panic helper functions, making cause detection less accurate.
- **Option vs Result**: Currently `unwrap_failed` is reported as "unwrap() on None" even though it could be from `Result::unwrap()`. Both use the same internal helper.
