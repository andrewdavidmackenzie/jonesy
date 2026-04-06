# Linux aarch64 Phase 1: Compilation on Linux

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Get jonesy compiling, passing clippy, and running (with basic output) on Linux aarch64 via CI, without breaking macOS.

**Architecture:** Remove macOS-only `#[cfg]` gates, extend `SymbolTable` with an `Elf` variant, add `.so` to library discovery, and add Linux to the CI matrix. ELF analysis will initially return an "unsupported" error — real analysis comes in later phases.

**Tech Stack:** Rust, goblin (already supports ELF), GitHub Actions (ubuntu-latest)

---

### File Map

| File | Change | Purpose |
|------|--------|---------|
| `CLAUDE.md:45` | Modify | Update platform rule to "macOS and Linux aarch64 only" |
| `jonesy/src/lib.rs` | Modify | Remove `#[cfg(target_os = "macos")]` gates |
| `jonesy/src/main.rs:1-7` | Modify | Remove `#[cfg(target_os = "macos")]` on imports |
| `jonesy/src/sym.rs:19-46` | Modify | Add `Elf` variant to `SymbolTable`, accept ELF in `from()` |
| `jonesy/src/main.rs:200-211` | Modify | Handle `SymbolTable::Elf` in `analyze_binary` dispatch |
| `jonesy/src/cargo.rs:359-379` | Modify | Add `.so` to `find_library` |
| `jonesy/src/debug_info.rs:48-69` | Modify | Make `has_dwarf_sections` work for ELF (check `.debug_*` sections) |
| `.github/workflows/ci.yml` | Modify | Add OS matrix with `ubuntu-latest` |
| `.github/workflows/coverage.yml` | Keep macOS-only for now | Coverage stays on macOS until tests pass on Linux |

---

### Task 1: Update CLAUDE.md platform rule

**Files:**
- Modify: `CLAUDE.md:45`

- [ ] **Step 1: Update the platform rule**

Change line 45 from:
```
- **macOS aarch64 only** — jonesy only supports macOS aarch64. Don't add cross-platform
  concerns or conditional compilation for other targets.
```
to:
```
- **macOS and Linux aarch64 only** — jonesy supports macOS and Linux on aarch64.
  Don't add support for other architectures or operating systems.
```

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update platform rule to include Linux aarch64"
```

---

### Task 2: Remove `#[cfg(target_os = "macos")]` gates

**Files:**
- Modify: `jonesy/src/lib.rs`
- Modify: `jonesy/src/main.rs:1-7`

- [ ] **Step 1: Remove all `#[cfg(target_os = "macos")]` from lib.rs**

In `jonesy/src/lib.rs`, remove every `#[cfg(target_os = "macos")]` line (lines 6, 10, 15, 17, 20, 22, 26, 29, 35). The module declarations should be unconditional.

Result should be:
```rust
//! Jonesy library - analyze Rust binaries for panic paths
//!
//! This library provides the core functionality for analyzing Rust binaries
//! to find code paths that can lead to panics.

pub mod analysis;
pub mod analysis_cache;
pub mod args;
pub mod call_graph;
pub mod call_tree;
pub mod cargo;
pub mod config;
pub mod crate_line_table;
pub mod debug_info;
pub mod file_watcher;
pub mod full_line_table;
pub mod function_index;
pub mod heuristics;
pub mod inline_allows;
pub mod library_call_graph;
pub mod lsp;
pub(crate) mod object_line_table;
pub mod output;
pub mod panic_cause;
pub mod project_context;
pub mod string_tables;
pub mod sym;
```

- [ ] **Step 2: Remove `#[cfg(target_os = "macos")]` from main.rs imports**

In `jonesy/src/main.rs`, remove the three `#[cfg(target_os = "macos")]` lines (lines 2, 4, 6) so the imports are unconditional:

```rust
use goblin::mach::Mach::{Binary, Fat};
use jonesy::analysis::{BinaryAnalysisResult, analyze_archive, analyze_macho};
use jonesy::sym::SymbolTable;
```

Also remove the comment `// macOS-only imports for Mach-O binary analysis` on line 1.

- [ ] **Step 3: Verify macOS build**

```bash
cargo build 2>&1 | tail -3
```
Expected: builds successfully

- [ ] **Step 4: Commit**

```bash
git add jonesy/src/lib.rs jonesy/src/main.rs
git commit -m "refactor: remove macOS-only cfg gates from modules"
```

---

### Task 3: Add `Elf` variant to `SymbolTable`

**Files:**
- Modify: `jonesy/src/sym.rs:18-47`

- [ ] **Step 1: Add `Elf` variant and accept ELF in `from()`**

In `jonesy/src/sym.rs`, change the `SymbolTable` enum and its `from()` method:

```rust
#[allow(clippy::large_enum_variant)]
pub enum SymbolTable<'a> {
    MachO(goblin::mach::Mach<'a>),
    Elf(goblin::elf::Elf<'a>),
    Archive(goblin::archive::Archive<'a>),
}

impl<'a> SymbolTable<'a> {
    /// Parse a binary buffer into a SymbolTable.
    pub fn from(buffer: &'a [u8]) -> io::Result<Self> {
        match Object::parse(buffer).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))? {
            Object::Mach(mach) => Ok(SymbolTable::MachO(mach)),
            Object::Elf(elf) => Ok(SymbolTable::Elf(elf)),
            Object::Archive(archive) => Ok(SymbolTable::Archive(archive)),
            Object::PE(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PE format not supported",
            )),
            Object::COFF(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "COFF format not supported",
            )),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unknown binary format",
            )),
        }
    }
```

- [ ] **Step 2: Update `macho()` method doc comment**

The existing `macho()` method stays as-is (returns `None` for Elf). No code change needed — just verify ELF is handled by the `_ => None` fallthrough.

- [ ] **Step 3: Fix compilation — handle `SymbolTable::Elf` in all match statements**

Search for `match symbols` and `match self` on SymbolTable in `sym.rs`, `main.rs`, `analysis.rs`, and `lsp.rs`. Add `SymbolTable::Elf(_)` arms that return appropriate "not yet implemented" errors or `None`/empty results. The key locations:

In `jonesy/src/main.rs` `analyze_binary` (around line 200), add an arm:
```rust
SymbolTable::Elf(_) => Err("ELF analysis not yet implemented".to_string()),
```

In `jonesy/src/lsp.rs` `analyze_single_target` function, wherever `SymbolTable::MachO` or `SymbolTable::Archive` is matched, add:
```rust
SymbolTable::Elf(_) => Err("ELF analysis not yet implemented".to_string()),
```

In `jonesy/src/sym.rs` `find_symbol_containing`, `find_all_symbols_matching`, and `find_symbol_address`, handle the `Elf` variant by returning `Ok(None)` / `Ok(Vec::new())` / `None`.

- [ ] **Step 4: Verify macOS build and clippy**

```bash
cargo fmt && cargo clippy 2>&1 | grep "warning:" | grep -v "Node.js"
```
Expected: clean

- [ ] **Step 5: Run tests**

```bash
make test 2>&1 | grep -E "test result:|FAILED"
```
Expected: all pass

- [ ] **Step 6: Commit**

```bash
git add jonesy/src/sym.rs jonesy/src/main.rs jonesy/src/lsp.rs
git commit -m "feat: add SymbolTable::Elf variant, accept ELF binaries"
```

---

### Task 4: Add `.so` to library discovery

**Files:**
- Modify: `jonesy/src/cargo.rs:359-379`

- [ ] **Step 1: Add `.so` extension to `find_library`**

In `jonesy/src/cargo.rs`, add a `.so` check in `find_library` after the `.dylib` check:

```rust
pub fn find_library(dir: &Path, name: &str) -> Option<PathBuf> {
    let lib_name = name.replace('-', "_");

    // Try platform-specific dynamic library extensions
    let dylib = dir.join(format!("lib{}.dylib", lib_name));
    if dylib.exists() {
        return Some(dylib);
    }
    let so = dir.join(format!("lib{}.so", lib_name));
    if so.exists() {
        return Some(so);
    }
    // Also try .rlib (Rust static library)
    let rlib = dir.join(format!("lib{}.rlib", lib_name));
    if rlib.exists() {
        return Some(rlib);
    }
    // Also try staticlib artifacts (.a)
    let staticlib = dir.join(format!("lib{}.a", lib_name));
    if staticlib.exists() {
        return Some(staticlib);
    }
    None
}
```

- [ ] **Step 2: Update `is_dylib` check in main.rs to also detect `.so`**

In `jonesy/src/main.rs` around line 108, change:
```rust
let is_dylib = binary_path.extension().is_some_and(|ext| ext == "dylib");
```
to:
```rust
let is_dylib = binary_path.extension().is_some_and(|ext| ext == "dylib" || ext == "so");
```

- [ ] **Step 3: Verify build and tests**

```bash
cargo fmt && cargo clippy 2>&1 | grep "warning:" | grep -v "Node.js"
make test 2>&1 | grep -E "test result:|FAILED"
```
Expected: clean build, all tests pass

- [ ] **Step 4: Commit**

```bash
git add jonesy/src/cargo.rs jonesy/src/main.rs
git commit -m "feat: add .so to library discovery for Linux support"
```

---

### Task 5: Add Linux to CI matrix

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add OS matrix to CI workflow**

Replace the CI workflow with a matrix strategy:

```yaml
name: CI

on:
  push:
    branches: [master]
  pull_request:
    branches: [master]

jobs:
  lint:
    strategy:
      matrix:
        os: [macos-14, ubuntu-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy

      - name: Check formatting
        run: cargo fmt --all -- --check

      - name: Build
        run: cargo build --workspace

      - name: Run clippy
        run: make clippy
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add Linux to CI matrix (build + clippy)"
```

---

### Task 6: Fix any Linux compilation issues

This task handles compilation errors that appear on Linux CI. Since we can't run Linux locally, we push and iterate based on CI results.

**Files:**
- Potentially any file with macOS-specific code that wasn't caught above

- [ ] **Step 1: Push branch and check CI**

```bash
git push -u origin issue-231-linux-aarch64-phase1
```

Monitor CI for both macOS and Linux jobs.

- [ ] **Step 2: Fix any Linux-specific compilation errors**

Common issues to expect and fix:
- `debug_info.rs`: `has_dwarf_sections` uses Mach-O segment iteration — needs ELF section check path
- `call_graph.rs`: imports Mach-O types — needs conditional or abstracted
- `analysis.rs`: references `MachO` type directly
- Any `use goblin::mach::*` imports that aren't behind the right conditions

For each error: fix, commit, push, check CI again.

- [ ] **Step 3: Verify both macOS and Linux CI pass**

```bash
gh run list --branch issue-231-linux-aarch64-phase1 --limit 2
```

Both jobs should show `success`.

- [ ] **Step 4: Run local macOS tests**

```bash
make test 2>&1 | grep -E "test result:|FAILED"
```
Expected: all pass (no macOS regression)

---

### Task 7: Create PR

- [ ] **Step 1: Create PR linking to issue #231**

```bash
gh pr create --title "feat: Linux aarch64 Phase 1 — compilation and CI" --body "$(cat <<'EOF'
## Summary

Phase 1 of #231 (Linux aarch64 support):

- Removed macOS-only `#[cfg]` gates from all modules
- Added `SymbolTable::Elf` variant (accepts ELF, analysis not yet implemented)
- Added `.so` to library discovery
- Added Linux (`ubuntu-latest`) to CI matrix (build + clippy)
- Updated CLAUDE.md platform rule

ELF analysis returns "not yet implemented" — real analysis comes in Phase 2+.

## Test plan

- [x] macOS: `make test` all pass
- [x] macOS: `cargo clippy` clean
- [x] Linux CI: build passes
- [x] Linux CI: clippy passes

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 2: Monitor CI and address review comments**
