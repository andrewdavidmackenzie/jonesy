# Jones - Panic Call Tree Analyzer

Jones analyzes Rust binaries to find all code paths that can lead to a panic, helping developers understand where panics can originate in their code.

## How It Works

### 1. Symbol Discovery

Jones starts by finding the `rust_panic` symbol in the binary using a regex pattern match:

```
rust_panic$
```

This matches the core panic function that all Rust panics eventually call. Due to recent Rust ABI changes, this symbol may be mangled (e.g., `__rustc::rust_panic`), so regex matching is used instead of exact name matching.

### 2. Call Tree Construction

Starting from `rust_panic`, Jones builds a reverse call tree by:

1. **Disassembling the `__text` section** using Capstone (ARM64)
2. **Finding all `bl` (branch-link) instructions** that call the target address
3. **Looking up the containing function** for each call site using DWARF debug info
4. **Recursively repeating** for each caller function

The result is a tree where:
- The root is `rust_panic`
- Each node represents a function that eventually leads to a panic
- Leaf nodes are functions with no further callers in the binary

### 3. Cycle Detection

A `HashSet<u64>` tracks visited function addresses to prevent infinite recursion when there are cycles in the call graph (e.g., recursive functions, mutual recursion).

### 4. Source File Resolution

For each caller, Jones extracts the source file path from DWARF debug info:

1. **Function declaration file** (`DW_AT_decl_file`) - preferred but not always present
2. **Line info at function start** - fallback that provides the outer function's source file

This is important because inlined functions (like `panic!` macro expansions) have their own source locations. Using the function's START address ensures we get the outer function's file, not the inlined code's file.

### 5. Call Tree Pruning

After building the full tree, Jones prunes branches that don't lead to user code:

```rust
fn prune_call_tree(node: &mut CallTreeNode, crate_src_path: &str) -> bool {
    // Recursively prune children first
    node.callers.retain_mut(|caller| prune_call_tree(caller, crate_src_path));

    // Keep this node if:
    // 1. It's a leaf AND in the crate source, OR
    // 2. It still has children after pruning
    if node.callers.is_empty() {
        is_in_crate(node, crate_src_path)
    } else {
        true
    }
}
```

The algorithm:
1. Recursively prunes all children first (depth-first)
2. For leaf nodes: keeps only those whose source file contains the crate path
3. For non-leaf nodes: keeps them if they still have children after pruning
4. Removes entire branches that don't eventually reach user code

### 6. Crate Source Path Detection

The crate source path is derived from the binary path:

```
target/panic/panic → examples/panic/src/
```

Jones looks for:
- `examples/<name>/src/` for example crates
- `<name>/src/` for workspace members
- `src/` for the main crate

## Data Structures

### CallTreeNode

```rust
struct CallTreeNode {
    name: String,              // Function name (e.g., "main", "panic_fmt")
    file: Option<String>,      // Source file path
    line: Option<u32>,         // Line number at call site
    callers: Vec<CallTreeNode> // Functions that call this one
}
```

### CallerInfo (from sym.rs)

```rust
struct CallerInfo {
    caller: FunctionInfo,      // Calling function info
    call_site_addr: u64,       // Address of the call instruction
    file: Option<String>,      // Source file at call site
    line: Option<u32>,         // Line number at call site
}
```

## Example Output

For a simple panic example:

```rust
fn main() {
    panic!("panic");
}
```

Jones produces:

```
__rustc::rust_panic
Called from: 'panic_with_hook' (source: library/std/src/panicking.rs:250)
    Called from: '{closure#0}' (source: library/std/src/panicking.rs:250)
        Called from: '__rust_end_short_backtrace<...>'
            Called from: 'panic_handler'
                Called from: 'panic_fmt' (source: library/core/src/panicking.rs:75)
                    Called from: 'main' (source: examples/panic/src/main.rs:259)
```

Without pruning, this tree would include hundreds of branches for:
- Signal handlers
- Runtime initialization
- I/O operations that can panic
- Memory allocation failures
- And more...

## Limitations

1. **Inlined code line numbers**: The line numbers shown are from the call site, which may be inside inlined code (like `panic!` macro expansion)

2. **ARM64 only**: Currently only supports ARM64 binaries (uses `bl` instruction detection)

3. **Direct calls only**: Only detects direct function calls via `bl` instructions, not indirect calls through function pointers

4. **macOS/Mach-O**: Currently only supports Mach-O binaries with dSYM or embedded DWARF

## Key Files

- `jones/src/main.rs` - Entry point, tree building, pruning logic
- `jones/src/sym.rs` - Symbol resolution, DWARF parsing, caller detection
- `jones/src/args.rs` - Command line argument parsing
