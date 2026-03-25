# Jonesy Features and Test Coverage

This document tracks all jonesy features and their integration test coverage.
Last updated: 2026-03-25

## Feature Hierarchy

### 1. Core Analysis Engine

| Feature | Description | Integration Test | Unit Tests |
|---------|-------------|------------------|------------|
| Binary analysis (Mach-O) | Analyze compiled binaries | `test_panic_example` | `sym.rs`, `call_tree.rs` |
| FAT binary detection | Detect multi-arch binaries | — | `sym.rs` |
| dSYM auto-generation | Generate debug symbols if missing | — | — |
| Stale dSYM detection | Warn when dSYM is outdated | — | — |

### 2. Library Analysis

| Feature | Description | Integration Test | Unit Tests |
|---------|-------------|------------------|------------|
| rlib analysis | Analyze Rust library archives | `test_rlib_example` | — |
| staticlib analysis | Analyze static libraries with DCE | `test_staticlib_example` | — |
| cdylib analysis | Analyze C-compatible dynamic libraries | `test_cdylib_example` | — |
| dylib analysis | Analyze Rust dynamic libraries | `test_dylib_example` | — |
| Line precision for libraries | Precise line/column info | `test_rlib_line_precision` | — |

### 3. Workspace Support

| Feature | Description | Integration Test | Unit Tests |
|---------|-------------|------------------|------------|
| Workspace detection | Auto-detect workspace members | `test_workspace_test_example` | `cargo.rs` |
| Per-member analysis | Analyze each workspace crate | `test_workspace_test_example` | — |
| Workspace summary | Aggregate results across crates | `test_workspace_test_example` | — |

### 4. Multi-Binary Support

| Feature | Description | Integration Test | Unit Tests |
|---------|-------------|------------------|------------|
| Multi-binary crate | Analyze crates with multiple binaries | `test_multi_bin_example` | — |
| Specific binary (`--bin`) | Analyze single binary | `test_multi_bin_specific_binary` | — |
| Library only (`--lib`) | Analyze library without binaries | `test_multi_bin_lib_only` | — |

### 5. Output Formats

| Feature | Description | Integration Test | Unit Tests |
|---------|-------------|------------------|------------|
| Text output (default) | Human-readable text format | All tests | `text_output.rs` |
| JSON output | Machine-readable JSON | `test_inlined_function_names`, `test_indirect_panic_shows_called_function` | `json_output.rs` |
| HTML output | Browser-viewable report | — | `html_output.rs` |
| Terminal hyperlinks | Clickable file links (OSC 8) | — | `text_output.rs` |
| `--no-hyperlinks` | Disable terminal hyperlinks | Used in all tests | `text_output.rs` |
| `--tree` | Show full call tree | `test_dwarf_specification_handling` | `text_output.rs`, `json_output.rs` |
| `--summary-only` | Summary statistics only | — | `text_output.rs`, `json_output.rs` |
| `--quiet` | Suppress progress messages | — | — |

### 6. Configuration

| Feature | Description | Integration Test | Unit Tests |
|---------|-------------|------------------|------------|
| Config cascade | Defaults → Cargo.toml → jonesy.toml → --config | `test_config_allow_panic` | `config.rs` |
| Global allow rules | Allow specific panic causes | `test_config_allow_panic` | `config.rs` |
| Global deny rules | Deny specific panic causes | — | `config.rs` |
| Scoped path rules | Allow/deny by file path pattern | `test_scoped_rules` | `config.rs` |
| Scoped function rules | Allow/deny by function pattern | — | `config.rs` |
| Inline allow comments | `// jonesy:allow(cause)` | — | `inline_allows.rs` |
| Wildcard allow (`*`) | Allow all causes | — | `inline_allows.rs` |
| `filter_phantom_async` | Filter phantom async panics | — | `config.rs` |

### 7. Panic Cause Detection

| ID | Cause | Description | Integration Test | Unit Tests |
|----|-------|-------------|------------------|------------|
| JP001 | `panic` | Explicit panic!() | `test_panic_example` | `panic_cause.rs` |
| JP002 | `bounds` | Index out of bounds | `test_panic_example` | `panic_cause.rs` |
| JP003 | `div_overflow` | Division overflow | — | `panic_cause.rs` |
| JP004 | `rem_overflow` | Remainder overflow | — | `panic_cause.rs` |
| JP005 | `shift_overflow` | Shift overflow | — | `panic_cause.rs` |
| JP006 | `unwrap` (None) | unwrap() on None | `test_panic_example` | `panic_cause.rs` |
| JP007 | `unwrap` (Err) | unwrap() on Err | — | `panic_cause.rs` |
| JP008 | `expect` (None) | expect() on None | `test_panic_example` | `panic_cause.rs` |
| JP009 | `expect` (Err) | expect() on Err | — | `panic_cause.rs` |
| JP010 | `assert` | assert!() failure | `test_panic_example` | `panic_cause.rs` |
| JP011 | `debug_assert` | debug_assert!() failure | `test_panic_example` | `panic_cause.rs` |
| JP012 | `unreachable` | unreachable!() reached | `test_panic_example` | `panic_cause.rs` |
| JP013 | `unimplemented` | unimplemented!() reached | `test_panic_example` | `panic_cause.rs` |
| JP014 | `todo` | todo!() reached | `test_rlib_todo_detection` | `panic_cause.rs` |
| JP015 | `drop` | Panic during drop | — | `panic_cause.rs` |
| JP016 | `unwind` | Panic in no-unwind context | — | `panic_cause.rs` |
| JP017 | `format` | Formatting error | — | `panic_cause.rs` |
| JP018 | `capacity` | Capacity overflow | — | `panic_cause.rs` |
| JP019 | `oom` | Out of memory | `test_oom_detection_via_abort` | `panic_cause.rs` |
| JP020 | `str_slice` | String/slice error | — | `panic_cause.rs` |
| JP021 | `invalid_enum` | Invalid enum discriminant | — | `panic_cause.rs` |
| JP022 | `misaligned_ptr` | Misaligned pointer | — | `panic_cause.rs` |
| — | `div_zero` | Division by zero | — | `panic_cause.rs` |
| — | `unknown` | Unknown cause | — | `panic_cause.rs` |

### 8. DWARF Handling

| Feature | Description | Integration Test | Unit Tests |
|---------|-------------|------------------|------------|
| Line table parsing | Extract source locations | All example tests | `sym.rs` |
| Function boundaries | Determine function ranges | All example tests | `sym.rs` |
| `DW_AT_specification` | Handle method references | `test_dwarf_specification_handling` | — |
| Inlined function names | Correct names for inlined code | `test_inlined_function_names` | — |
| Conditional panic detection | Panics inside if blocks | `test_rlib_conditional_panic_detection` | — |

### 9. Output Enhancements

| Feature | Description | Integration Test | Unit Tests |
|---------|-------------|------------------|------------|
| Direct vs indirect panics | Different help messages | `test_indirect_panic_shows_called_function` | `panic_cause.rs` |
| Called function in messages | Show which function causes panic | `test_indirect_panic_shows_called_function` | — |
| Error codes (JP001-JP022) | Unique identifiers | All tests (implicit) | `panic_cause.rs` |
| Documentation URLs | Links to docs site | — | `panic_cause.rs` |
| Root-level reporting | Functions as root entries | `test_intermediate_functions_reported_as_roots` | — |

### 10. LSP Server

| Feature | Description | Integration Test | Unit Tests |
|---------|-------------|------------------|------------|
| `jonesy lsp` subcommand | Start LSP server | — | — |
| Diagnostics publishing | Send warnings to IDE | — | `lsp.rs` |
| Quick fix actions | Allow inline/file/function | — | — |
| Auto-refresh | Analyze on binary change | — | — |
| Progress indicators | Show analysis progress | — | — |
| File change watching | Watch target/debug | — | — |

### 11. CI Integration

| Feature | Description | Integration Test | Unit Tests |
|---------|-------------|------------------|------------|
| Exit code = panic count | Non-zero on panics | `test_perfect_example` | — |
| GitHub Action | PR annotations | — | — |
| Problem matcher | Inline warnings | — | — |

### 12. Performance

| Feature | Description | Integration Test | Unit Tests |
|---------|-------------|------------------|------------|
| `--max-threads` | Thread pool size | — | — |
| Parallel workspace analysis | Concurrent crate analysis | — | — |
| Merged line table builds | Single DWARF pass | — | — |
| LTO release builds | Optimized binary | — | — |

---

## Coverage Gaps

### High Priority (Core Features Missing Tests)

1. **HTML output format** — No integration test verifies HTML generation
2. **`--summary-only` flag** — No integration test for summary-only output
3. **LSP server** — No integration tests for LSP functionality
4. **`filter_phantom_async` config** — No test for phantom async filtering
5. **Function-scoped rules** — Scoped rules test only covers path patterns

### Medium Priority (Edge Cases)

6. **Division/remainder/shift overflow** — No specific test for JP003-JP005
7. **`expect()` on Err** (JP009) — Only None variant tested
8. **`drop` panic** (JP015) — No test for panic during drop
9. **`unwind` panic** (JP016) — No test for no-unwind context
10. **`format` panic** (JP017) — No test for formatting errors
11. **`capacity` panic** (JP018) — No test for capacity overflow
12. **`str_slice` panic** (JP020) — No test for string slice errors
13. **`invalid_enum` panic** (JP021) — No test for invalid enum
14. **`misaligned_ptr` panic** (JP022) — No test for misaligned pointer
15. **FAT binary handling** — No test for multi-arch binaries
16. **Stale dSYM detection** — No test for timestamp comparison

### Low Priority (Nice to Have)

17. **Terminal hyperlink output** — Hard to test OSC 8 sequences
18. **`--quiet` flag** — Suppresses output, hard to verify
19. **`--max-threads` performance** — Performance testing is complex
20. **GitHub Action workflow** — Tested in separate action repo

---

## Test File Locations

| Test Type | Location |
|-----------|----------|
| Integration tests | `jonesy/tests/integration_tests.rs` |
| Unit tests - analysis_cache | `jonesy/src/analysis_cache.rs` |
| Unit tests - cargo | `jonesy/src/cargo.rs` |
| Unit tests - config | `jonesy/src/config.rs` |
| Unit tests - inline_allows | `jonesy/src/inline_allows.rs` |
| Unit tests - panic_cause | `jonesy/src/panic_cause.rs` |
| Unit tests - text_output | `jonesy/src/text_output.rs` |
| Unit tests - json_output | `jonesy/src/json_output.rs` |
| Unit tests - html_output | `jonesy/src/html_output.rs` |
| Unit tests - sym | `jonesy/src/sym.rs` |

---

## Example Projects

| Example | Purpose | Used By Tests |
|---------|---------|---------------|
| `examples/panic` | Various panic causes | `test_panic_example`, `test_scoped_rules`, etc. |
| `examples/rlib` | Library-only analysis | `test_rlib_example`, `test_rlib_*` |
| `examples/staticlib` | Static library with DCE | `test_staticlib_example` |
| `examples/cdylib` | C-compatible dylib | `test_cdylib_example` |
| `examples/dylib` | Rust dylib | `test_dylib_example` |
| `examples/multi_bin` | Multiple binaries + lib | `test_multi_bin_*` |
| `examples/perfect` | No panics | `test_perfect_example` |
| `examples/workspace_test` | Nested workspace | `test_workspace_test_example` |
| `examples/inlined` | Inlined functions | `test_inlined_function_names` |
| `examples/unwrap_or_default` | DW_AT_specification | `test_dwarf_specification_handling` |

---

## Adding New Tests

When adding a new feature:

1. Add `// jonesy: expect panic` or `// jonesy: expect panic(cause)` comments to example source files
2. The integration test framework automatically validates these markers against jonesy output
3. For features that don't produce panic points, add explicit assertions in the test function

When a test fails:
- **Missing panic** — Expected marker has no corresponding detection (likely a bug)
- **Unexpected panic** — Detection without marker (may be platform-specific, generally OK)
