# Jonesy Features and Test Coverage

This document tracks all jonesy features and their integration test coverage.
Last updated: 2026-03-25

## Feature Hierarchy

### 1. Core Analysis Engine

| Feature                  | Description                       | Integration Test     | Unit Tests                                                                |
|--------------------------|-----------------------------------|----------------------|---------------------------------------------------------------------------|
| Binary analysis (Mach-O) | Analyze compiled binaries         | `test_panic_example` | `test_decode_branch_target_forward`, `test_decode_branch_target_backward` |
| FAT binary detection     | Detect multi-arch binaries        | —                    | —                                                                         |
| dSYM auto-generation     | Generate debug symbols if missing | —                    | —                                                                         |
| Stale dSYM detection     | Warn when dSYM is outdated        | —                    | —                                                                         |

### 2. Library Analysis

| Feature                      | Description                          | Integration Test             | Unit Tests |
|------------------------------|--------------------------------------|------------------------------|------------|
| rlib analysis                | Analyze Rust library archives        | `test_rlib_example`          | —          |
| staticlib analysis           | Analyze static libraries with DCE    | `test_staticlib_example`     | —          |
| cdylib analysis              | Analyze C-compatible dynamic libs    | `test_cdylib_example`        | —          |
| dylib analysis               | Analyze Rust dynamic libraries       | `test_dylib_example`         | —          |
| Line precision for libraries | Precise line/column info             | `test_rlib_line_precision`   | —          |

### 3. Workspace Support

| Feature             | Description                     | Integration Test              | Unit Tests                         |
|---------------------|---------------------------------|-------------------------------|------------------------------------|
| Workspace detection | Auto-detect workspace members   | `test_workspace_test_example` | `test_find_project_root_not_found` |
| Per-member analysis | Analyze each workspace crate    | `test_workspace_test_example` | —                                  |
| Workspace summary   | Aggregate results across crates | `test_workspace_test_example` | —                                  |

### 4. Multi-Binary Support

| Feature                    | Description                          | Integration Test                  | Unit Tests |
|----------------------------|--------------------------------------|-----------------------------------|------------|
| Multi-binary crate         | Analyze crates with multiple bins    | `test_multi_bin_example`          | —          |
| Specific binary (`--bin`)  | Analyze single binary                | `test_multi_bin_specific_binary`  | —          |
| Library only (`--lib`)     | Analyze library without binaries     | `test_multi_bin_lib_only`         | —          |

### 5. Output Formats

| Feature             | Description                  | Integration Test                                                           | Unit Tests                                                                                               |
|---------------------|------------------------------|----------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------|
| Text output         | Human-readable text format   | All tests                                                                  | `test_write_text_output_empty`, `test_write_text_output_flat_format`, `test_write_directory_tree_format` |
| JSON output         | Machine-readable JSON        | `test_inlined_function_names`, `test_indirect_panic_shows_called_function` | `test_generate_json_output_empty`, `test_generate_json_output_with_code_points`                          |
| HTML output         | Browser-viewable report      | —                                                                          | `test_generate_html_output_empty`, `test_generate_html_output_with_panic_points`                         |
| Terminal hyperlinks | Clickable file links (OSC 8) | —                                                                          | —                                                                                                        |
| `--no-hyperlinks`   | Disable terminal hyperlinks  | Used in all tests                                                          | —                                                                                                        |
| `--tree`            | Show full call tree          | `test_dwarf_specification_handling`                                        | `test_write_text_output_tree_with_children`, `test_generate_json_output_with_children`                   |
| `--summary-only`    | Summary statistics only      | —                                                                          | `test_write_text_output_summary_only`, `test_generate_json_output_summary_only`                          |
| `--quiet`           | Suppress progress messages   | —                                                                          | —                                                                                                        |

### 6. Configuration

| Feature                | Description                         | Integration Test          | Unit Tests                                                                                      |
|------------------------|-------------------------------------|---------------------------|-------------------------------------------------------------------------------------------------|
| Config cascade         | Defaults → Cargo.toml → jonesy.toml | `test_config_allow_panic` | `test_default_config`                                                                           |
| Global allow rules     | Allow specific panic causes         | `test_config_allow_panic` | `test_toml_config_allow_panic`                                                                  |
| Global deny rules      | Deny specific panic causes          | —                         | `test_toml_config_deny_drop`                                                                    |
| Scoped path rules      | Allow/deny by file path pattern     | `test_scoped_rules`       | `test_scoped_rule_path_matching`, `test_scoped_rule_wildcard_allow`                             |
| Scoped function rules  | Allow/deny by function pattern      | —                         | `test_scoped_rule_function_matching`, `test_function_rule_more_specific_than_path`              |
| Inline allow comments  | `// jonesy:allow(cause)`            | —                         | `test_parse_single_cause`, `test_parse_multiple_causes`, `test_check_inline_allow_with_file`    |
| Wildcard allow (`*`)   | Allow all causes                    | —                         | `test_parse_wildcard`, `test_check_inline_allow_wildcard`, `test_is_allowed_by_inline_wildcard` |
| `filter_phantom_async` | Filter phantom async panics         | —                         | `test_filter_phantom_async_default_true`, `test_filter_phantom_async_can_be_disabled`           |

### 7. Panic Cause Detection

| ID    | Cause            | Description               | Integration Test               | Unit Tests                                                                           |
|-------|------------------|---------------------------|--------------------------------|--------------------------------------------------------------------------------------|
| JP001 | `panic`          | Explicit panic!()         | `test_panic_example`           | `test_detect_panic_cause_panic_display`                                              |
| JP002 | `bounds`         | Index out of bounds       | `test_panic_example`           | `test_detect_panic_cause_bounds_check`, `test_detect_panic_cause_index_bounds`       |
| JP003 | `div_overflow`   | Division overflow         | —                              | `test_detect_panic_cause_arithmetic_overflow`                                        |
| JP004 | `rem_overflow`   | Remainder overflow        | —                              | `test_detect_panic_cause_arithmetic_overflow`                                        |
| JP005 | `shift_overflow` | Shift overflow            | —                              | `test_detect_panic_cause_shift_overflow`                                             |
| JP006 | `unwrap` (None)  | unwrap() on None          | `test_panic_example`           | `test_detect_panic_cause_unwrap_failed_option`                                       |
| JP007 | `unwrap` (Err)   | unwrap() on Err           | —                              | `test_detect_panic_cause_unwrap_failed_result`                                       |
| JP008 | `expect` (None)  | expect() on None          | `test_panic_example`           | `test_detect_panic_cause_expect_failed`                                              |
| JP009 | `expect` (Err)   | expect() on Err           | —                              | `test_detect_panic_cause_result_expect`                                              |
| JP010 | `assert`         | assert!() failure         | `test_panic_example`           | `test_detect_panic_cause_assert_failed`                                              |
| JP011 | `debug_assert`   | debug_assert!() failure   | `test_panic_example`           | `test_detect_panic_cause_assert_failed`                                              |
| JP012 | `unreachable`    | unreachable!() reached    | `test_panic_example`           | `test_detect_panic_cause_unreachable`                                                |
| JP013 | `unimplemented`  | unimplemented!() reached  | `test_panic_example`           | —                                                                                    |
| JP014 | `todo`           | todo!() reached           | `test_rlib_todo_detection`     | —                                                                                    |
| JP015 | `drop`           | Panic during drop         | —                              | `test_detect_panic_cause_panic_in_cleanup`                                           |
| JP016 | `unwind`         | Panic in no-unwind        | —                              | `test_detect_panic_cause_panic_cannot_unwind`                                        |
| JP017 | `format`         | Formatting error          | —                              | `test_detect_panic_cause_formatting`                                                 |
| JP018 | `capacity`       | Capacity overflow         | —                              | `test_detect_panic_cause_capacity_overflow`                                          |
| JP019 | `oom`            | Out of memory             | `test_oom_detection_via_abort` | `test_detect_panic_cause_out_of_memory`                                              |
| JP020 | `str_slice`      | String/slice error        | —                              | `test_detect_panic_cause_string_slice_error`, `test_detect_panic_cause_index_string` |
| JP021 | `invalid_enum`   | Invalid enum discriminant | —                              | `test_detect_panic_cause_invalid_enum`                                               |
| JP022 | `misaligned_ptr` | Misaligned pointer        | —                              | `test_detect_panic_cause_misaligned_pointer`                                         |
| —     | `div_zero`       | Division by zero          | —                              | `test_detect_panic_cause_division_by_zero`                                           |
| —     | `unknown`        | Unknown cause             | —                              | `test_detect_panic_cause_unknown`                                                    |

### 8. DWARF Handling

| Feature                     | Description                    | Integration Test                        | Unit Tests                                                               |
|-----------------------------|--------------------------------|-----------------------------------------|--------------------------------------------------------------------------|
| Line table parsing          | Extract source locations       | All example tests                       | `test_matches_crate_pattern_simple`, `test_is_dependency_path_user_code` |
| Function boundaries         | Determine function ranges      | All example tests                       | `test_valid_source_files_contains`                                       |
| `DW_AT_specification`       | Handle method references       | `test_dwarf_specification_handling`     | —                                                                        |
| Inlined function names      | Correct names for inlined code | `test_inlined_function_names`           | —                                                                        |
| Conditional panic detection | Panics inside if blocks        | `test_rlib_conditional_panic_detection` | —                                                                        |

### 9. Output Enhancements

| Feature                     | Description                      | Integration Test                                | Unit Tests                                                |
|-----------------------------|----------------------------------|-------------------------------------------------|-----------------------------------------------------------|
| Direct vs indirect panics   | Different help messages          | `test_indirect_panic_shows_called_function`     | `test_direct_suggestions`, `test_indirect_suggestions`    |
| Called function in messages | Show which function causes panic | `test_indirect_panic_shows_called_function`     | `test_panic_cause_format_suggestion_with_function`        |
| Error codes (JP001-JP022)   | Unique identifiers               | All tests (implicit)                            | `test_panic_cause_error_code`, `test_all_docs_slugs`      |
| Documentation URLs          | Links to docs site               | —                                               | `test_panic_cause_docs_url`, `test_panic_cause_docs_slug` |
| Root-level reporting        | Functions as root entries        | `test_intermediate_functions_reported_as_roots` | —                                                         |

### 10. LSP Server

| Feature                 | Description                | Integration Test | Unit Tests |
|-------------------------|----------------------------|------------------|------------|
| `jonesy lsp` subcommand | Start LSP server           | —                | —          |
| Diagnostics publishing  | Send warnings to IDE       | —                | —          |
| Quick fix actions       | Allow inline/file/function | —                | —          |
| Auto-refresh            | Analyze on binary change   | —                | —          |
| Progress indicators     | Show analysis progress     | —                | —          |
| File change watching    | Watch target/debug         | —                | —          |

### 11. CI Integration

| Feature                 | Description        | Integration Test       | Unit Tests |
|-------------------------|--------------------|------------------------|------------|
| Exit code = panic count | Non-zero on panics | `test_perfect_example` | —          |
| GitHub Action           | PR annotations     | —                      | —          |
| Problem matcher         | Inline warnings    | —                      | —          |

### 12. Performance

| Feature                     | Description               | Integration Test | Unit Tests |
|-----------------------------|---------------------------|------------------|------------|
| `--max-threads`             | Thread pool size          | —                | —          |
| Parallel workspace analysis | Concurrent crate analysis | —                | —          |
| Merged line table builds    | Single DWARF pass         | —                | —          |
| LTO release builds          | Optimized binary          | —                | —          |

---

## Coverage Gaps

### High Priority (Core Features Missing Tests)

1. **HTML output format** — No integration test verifies HTML generation
2. **`--summary-only` flag** — No integration test for summary-only output
3. **LSP server** — No integration tests for LSP functionality
4. **`filter_phantom_async` config** — No integration test (has unit tests)
5. **Function-scoped rules** — No integration test (has unit tests)

### Medium Priority (Edge Cases)

6. **Division/remainder/shift overflow** — No integration test for JP003-JP005
7. **`expect()` on Err** (JP009) — Only None variant integration-tested
8. **`drop` panic** (JP015) — No integration test (has unit test)
9. **`unwind` panic** (JP016) — No integration test (has unit test)
10. **`format` panic** (JP017) — No integration test (has unit test)
11. **`capacity` panic** (JP018) — No integration test (has unit test)
12. **`str_slice` panic** (JP020) — No integration test (has unit tests)
13. **`invalid_enum` panic** (JP021) — No integration test (has unit test)
14. **`misaligned_ptr` panic** (JP022) — No integration test (has unit test)
15. **FAT binary handling** — No test for multi-arch binaries
16. **Stale dSYM detection** — No test for timestamp comparison

### Low Priority (Nice to Have)

17. **Terminal hyperlink output** — Hard to test OSC 8 sequences
18. **`--quiet` flag** — Suppresses output, hard to verify
19. **`--max-threads` performance** — Performance testing is complex
20. **GitHub Action workflow** — Tested in separate action repo

---

## Test File Locations

| Test Type                   | Location                            |
|-----------------------------|-------------------------------------|
| Integration tests           | `jonesy/tests/integration_tests.rs` |
| Unit tests - analysis_cache | `jonesy/src/analysis_cache.rs`      |
| Unit tests - cargo          | `jonesy/src/cargo.rs`               |
| Unit tests - config         | `jonesy/src/config.rs`              |
| Unit tests - inline_allows  | `jonesy/src/inline_allows.rs`       |
| Unit tests - panic_cause    | `jonesy/src/panic_cause.rs`         |
| Unit tests - text_output    | `jonesy/src/text_output.rs`         |
| Unit tests - json_output    | `jonesy/src/json_output.rs`         |
| Unit tests - html_output    | `jonesy/src/html_output.rs`         |
| Unit tests - sym            | `jonesy/src/sym.rs`                 |

---

## Example Projects

| Example                      | Purpose                 | Used By Tests                                   |
|------------------------------|-------------------------|-------------------------------------------------|
| `examples/panic`             | Various panic causes    | `test_panic_example`, `test_scoped_rules`, etc. |
| `examples/rlib`              | Library-only analysis   | `test_rlib_example`, `test_rlib_*`              |
| `examples/staticlib`         | Static library with DCE | `test_staticlib_example`                        |
| `examples/cdylib`            | C-compatible dylib      | `test_cdylib_example`                           |
| `examples/dylib`             | Rust dylib              | `test_dylib_example`                            |
| `examples/multi_bin`         | Multiple binaries + lib | `test_multi_bin_*`                              |
| `examples/perfect`           | No panics               | `test_perfect_example`                          |
| `examples/workspace_test`    | Nested workspace        | `test_workspace_test_example`                   |
| `examples/inlined`           | Inlined functions       | `test_inlined_function_names`                   |
| `examples/unwrap_or_default` | DW_AT_specification     | `test_dwarf_specification_handling`             |

---

## Adding New Tests

When adding a new feature:

1. Add `// jonesy: expect panic` or `// jonesy: expect panic(cause)` comments to example source files
2. The integration test framework automatically validates these markers against jonesy output
3. For features that don't produce panic points, add explicit assertions in the test function

When a test fails:
- **Missing panic** — Expected marker has no corresponding detection (likely a bug)
- **Unexpected panic** — Detection without marker (maybe platform-specific, generally OK)
