# Linux aarch64 Phase 2: Debug Info and Analysis Abstraction

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the core analysis pipeline work for ELF binaries by abstracting the Mach-O specific parts (section lookup, debug info loading, DWARF section naming) while keeping the existing code structure. Rename `analyze_macho` to `analyze_binary`.

**Architecture:** Rather than a trait abstraction, use a simple `BinaryRef` enum that wraps either `&MachO` or `&Elf` and provides format-aware helper methods for section lookup. This keeps the existing code structure intact while enabling ELF support. Functions that currently take `&MachO` will take `&BinaryRef` instead.

**Tech Stack:** Rust, goblin (MachO + ELF), gimli (DWARF parsing)

---

### Task 1: Create `BinaryRef` enum for format-aware section lookup

**Files:**
- Create: `jonesy/src/binary_format.rs`
- Modify: `jonesy/src/lib.rs` — add module

This is the central abstraction. A small enum that wraps either a MachO or ELF reference and provides unified access to sections and symbols.

- [ ] **Step 1: Create `jonesy/src/binary_format.rs`**

```rust
//! Binary format abstraction for Mach-O and ELF.
//!
//! Provides `BinaryRef` — a format-aware wrapper that gives uniform
//! access to sections and symbols across both binary formats.

use goblin::elf::Elf;
use goblin::mach::MachO;

/// A reference to either a Mach-O or ELF binary.
pub enum BinaryRef<'a> {
    MachO(&'a MachO<'a>),
    Elf(&'a Elf<'a>),
}

impl<'a> BinaryRef<'a> {
    /// Find a section by its platform-agnostic purpose.
    /// Handles naming differences: `__text` (MachO) vs `.text` (ELF).
    pub fn find_section(&self, buffer: &'a [u8], name: &str) -> Option<(u64, &'a [u8])> {
        match self {
            BinaryRef::MachO(macho) => find_macho_section(macho, buffer, name),
            BinaryRef::Elf(elf) => find_elf_section(elf, buffer, name),
        }
    }

    /// Get the text section name for this binary format.
    pub fn text_section_name(&self) -> &'static str {
        match self {
            BinaryRef::MachO(_) => "__text",
            BinaryRef::Elf(_) => ".text",
        }
    }

    /// Check if this binary has DWARF debug sections.
    pub fn has_dwarf(&self) -> bool {
        match self {
            BinaryRef::MachO(macho) => has_macho_dwarf(macho),
            BinaryRef::Elf(elf) => has_elf_dwarf(elf),
        }
    }

    /// Convert a gimli DWARF section name (e.g., ".debug_info") to the
    /// format-specific name used in this binary.
    pub fn dwarf_section_name(&self, gimli_name: &str) -> String {
        match self {
            BinaryRef::MachO(_) => {
                // ".debug_info" -> "__debug_info"
                format!("__{}", &gimli_name[1..])
            }
            BinaryRef::Elf(_) => {
                // ELF uses gimli names directly
                gimli_name.to_string()
            }
        }
    }

    /// Returns true if this is an ELF binary.
    pub fn is_elf(&self) -> bool {
        matches!(self, BinaryRef::Elf(_))
    }
}

fn find_macho_section<'a>(
    macho: &MachO<'a>,
    buffer: &'a [u8],
    name: &str,
) -> Option<(u64, &'a [u8])> {
    for segment in macho.segments.iter() {
        if let Ok(sections) = segment.sections() {
            for (section, _) in sections {
                if let Ok(section_name) = section.name() {
                    if section_name == name {
                        let offset = section.offset as usize;
                        let size = section.size as usize;
                        if offset + size <= buffer.len() {
                            return Some((section.addr, &buffer[offset..offset + size]));
                        }
                    }
                }
            }
        }
    }
    None
}

fn find_elf_section<'a>(
    elf: &Elf<'a>,
    buffer: &'a [u8],
    name: &str,
) -> Option<(u64, &'a [u8])> {
    for section_header in &elf.section_headers {
        if let Some(section_name) = elf.shdr_strtab.get_at(section_header.sh_name) {
            if section_name == name {
                let offset = section_header.sh_offset as usize;
                let size = section_header.sh_size as usize;
                if offset + size <= buffer.len() {
                    return Some((section_header.sh_addr, &buffer[offset..offset + size]));
                }
            }
        }
    }
    None
}

fn has_macho_dwarf(macho: &MachO) -> bool {
    for segment in macho.segments.iter() {
        if let Ok(name) = segment.name() {
            if name == "__DWARF" {
                return true;
            }
        }
        if let Ok(sections) = segment.sections() {
            for (section, _) in sections {
                if let Ok(name) = section.name() {
                    if name.starts_with("__debug_") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn has_elf_dwarf(elf: &Elf) -> bool {
    for section_header in &elf.section_headers {
        if let Some(name) = elf.shdr_strtab.get_at(section_header.sh_name) {
            if name.starts_with(".debug_") {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_section_name() {
        // We can't easily construct MachO/Elf in tests without real binaries,
        // but we can test the naming logic once we have a BinaryRef.
        // This test uses a real binary if available.
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(goblin::Object::Mach(goblin::mach::Mach::Binary(ref macho))) =
                goblin::Object::parse(&buffer)
            {
                let binary_ref = BinaryRef::MachO(macho);
                assert_eq!(binary_ref.text_section_name(), "__text");
                assert!(!binary_ref.is_elf());
            }
        }
    }

    #[test]
    fn test_dwarf_section_name_macho() {
        let binary_path = format!("{}/target/debug/jonesy", env!("CARGO_MANIFEST_DIR"));
        if let Ok(buffer) = std::fs::read(&binary_path) {
            if let Ok(goblin::Object::Mach(goblin::mach::Mach::Binary(ref macho))) =
                goblin::Object::parse(&buffer)
            {
                let binary_ref = BinaryRef::MachO(macho);
                assert_eq!(
                    binary_ref.dwarf_section_name(".debug_info"),
                    "__debug_info"
                );
            }
        }
    }
}
```

- [ ] **Step 2: Add module to lib.rs**

Add `pub mod binary_format;` to `jonesy/src/lib.rs` (after `pub mod args;`).

- [ ] **Step 3: Verify build**

```bash
cargo fmt && cargo clippy && make test
```

- [ ] **Step 4: Commit**

```bash
git add jonesy/src/binary_format.rs jonesy/src/lib.rs
git commit -m "feat: add BinaryRef abstraction for format-aware section lookup"
```

---

### Task 2: Update `debug_info.rs` to use `BinaryRef`

**Files:**
- Modify: `jonesy/src/debug_info.rs`

The key change: `load_debug_info` currently takes `&MachO`. Change it to take `&BinaryRef` (plus the buffer). For ELF, skip dSYM/dsymutil/debug-map and just check for embedded DWARF.

- [ ] **Step 1: Update `load_debug_info` signature and body**

Change `load_debug_info(macho: &MachO, binary_path: &Path, quiet: bool) -> DebugInfo` to:
```rust
pub fn load_debug_info(binary: &BinaryRef, binary_path: &Path, quiet: bool) -> DebugInfo
```

For ELF: if `binary.has_dwarf()`, return `DebugInfo::Embedded`. Otherwise return `DebugInfo::None`. Skip dSYM, dsymutil, and debug map paths entirely (those are MachO-only).

For MachO: keep existing logic, extract the `&MachO` from `BinaryRef::MachO(macho)`.

- [ ] **Step 2: Remove `has_dwarf_sections` standalone function**

Replace callers with `binary.has_dwarf()` from BinaryRef. The implementation moves into `binary_format.rs`.

- [ ] **Step 3: Update `sym.rs` re-exports**

`sym.rs` re-exports `load_debug_info`. Update the import path if needed.

- [ ] **Step 4: Update call sites in `analysis.rs`**

Where `load_debug_info(macho, binary_path, ...)` is called, construct a `BinaryRef::MachO(macho)` and pass that instead.

- [ ] **Step 5: Verify build and tests**

```bash
cargo fmt && cargo clippy && make test
```

- [ ] **Step 6: Commit**

```bash
git commit -am "refactor: load_debug_info accepts BinaryRef for ELF support"
```

---

### Task 3: Update `call_graph.rs` to use `BinaryRef` for section access

**Files:**
- Modify: `jonesy/src/call_graph.rs`

The `get_section_by_name` function and its callers use MachO directly. Change to use `BinaryRef`.

- [ ] **Step 1: Replace `get_section_by_name` with `BinaryRef::find_section`**

The existing `get_section_by_name(macho, buffer, "__text")` calls should become `binary.find_section(buffer, binary.text_section_name())`.

Update `CallGraph::build` and `CallGraph::build_with_debug_info` to accept `&BinaryRef` instead of `&MachO`.

- [ ] **Step 2: Update callers in `analysis.rs`**

Where `CallGraph::build(macho, buffer, ...)` is called, pass `&BinaryRef::MachO(macho)` instead.

- [ ] **Step 3: Verify build and tests**

```bash
cargo fmt && cargo clippy && make test
```

- [ ] **Step 4: Commit**

```bash
git commit -am "refactor: call_graph uses BinaryRef for section access"
```

---

### Task 4: Update `function_index.rs` DWARF section name mapping

**Files:**
- Modify: `jonesy/src/function_index.rs`

The `load_dwarf_sections` function has hardcoded MachO name conversion (`.debug_info` → `__debug_info`). Use `BinaryRef::dwarf_section_name()` instead.

- [ ] **Step 1: Update `load_dwarf_sections` and `get_functions_from_dwarf`**

These functions currently take `&MachO`. Change to take `&BinaryRef` (plus buffer). Use `binary.dwarf_section_name(gimli_name)` for the name conversion and `binary.find_section(buffer, &converted_name)` for section lookup.

- [ ] **Step 2: Update callers**

Update `call_graph.rs` where these functions are called.

- [ ] **Step 3: Verify build and tests**

```bash
cargo fmt && cargo clippy && make test
```

- [ ] **Step 4: Commit**

```bash
git commit -am "refactor: function_index uses BinaryRef for DWARF section names"
```

---

### Task 5: Rename `analyze_macho` → `analyze_binary` and wire ELF path

**Files:**
- Modify: `jonesy/src/analysis.rs`
- Modify: `jonesy/src/main.rs`
- Modify: `jonesy/src/lsp.rs`

- [ ] **Step 1: Rename `analyze_macho` to `analyze_binary_target` in analysis.rs**

(Using `analyze_binary_target` since `analyze_binary` already exists as the dispatch function in `main.rs`.)

Update the function to construct a `BinaryRef` from the SymbolTable and pass it to downstream functions that now accept `BinaryRef`.

For the ELF case: construct `BinaryRef::Elf(elf)` and follow the same code path as MachO. The `DebugInfo::Embedded` path should work since ELF has embedded DWARF.

- [ ] **Step 2: Update callers in `main.rs`**

Change `analyze_macho(...)` to `analyze_binary_target(...)`. Update the `analyze_binary` dispatch function to route `SymbolTable::Elf` to `analyze_binary_target` instead of returning an error.

- [ ] **Step 3: Update callers in `lsp.rs`**

Same rename. Route `SymbolTable::Elf` to `analyze_binary_target`.

- [ ] **Step 4: Verify build and tests**

```bash
cargo fmt && cargo clippy && make test
```

- [ ] **Step 5: Commit**

```bash
git commit -am "feat: rename analyze_macho to analyze_binary_target, wire ELF path"
```

---

### Task 6: Update `SymbolIndex` for ELF

**Files:**
- Modify: `jonesy/src/sym.rs`

`SymbolIndex::new` currently takes `&MachO`. It needs to also work with ELF.

- [ ] **Step 1: Add `SymbolIndex::from_elf` or generalize `new`**

Add a method that builds a SymbolIndex from an ELF binary's symbol table. ELF symbols use `elf.syms` iterator. Symbol names don't have the leading underscore that MachO has.

- [ ] **Step 2: Update callers in analysis.rs**

Where `SymbolIndex::new(macho)` is called, branch on the binary format to call the right constructor.

- [ ] **Step 3: Verify build and tests**

```bash
cargo fmt && cargo clippy && make test
```

- [ ] **Step 4: Commit**

```bash
git commit -am "feat: SymbolIndex supports ELF symbol tables"
```

---

### Task 7: Push and verify Linux CI

- [ ] **Step 1: Run full local test suite**

```bash
make test
```

- [ ] **Step 2: Push and create PR**

```bash
git push -u origin issue-231-linux-aarch64-phase2
gh pr create --title "feat: Linux aarch64 Phase 2 — analysis abstraction" --body "..."
```

- [ ] **Step 3: Verify both macOS and Linux CI pass**

Monitor CI for compilation on both platforms.

- [ ] **Step 4: Fix any Linux-specific issues from CI feedback**
