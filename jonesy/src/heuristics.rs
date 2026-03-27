//! Detection heuristics for panic analysis.
//!
//! Jonesy uses a layered set of heuristics to achieve two core tasks:
//!
//! 1. **Source code ownership** — Distinguishing user/crate code from standard library
//!    and dependency code in DWARF debug info and symbol tables.
//! 2. **Panic cause classification** — Identifying _what kind_ of panic a call path
//!    leads to (e.g., `unwrap()` on `None` vs. index out of bounds).
//!
//! This module consolidates the pattern constants and classification functions used
//! throughout the crate, serving as the single reference for how detection works.
//!
//! # Source Code Ownership
//!
//! DWARF debug info includes file paths for every source line. When the compiler
//! inlines stdlib code (e.g., `Option::unwrap()`), the line table contains paths
//! from the Rust toolchain (like `/rustc/.../option.rs`). Jonesy must distinguish
//! these from user code to report only panic points the developer controls.
//!
//! Three complementary functions handle this:
//!
//! - [`is_dependency_path`] — Checks if a file path belongs to a dependency or
//!   the standard library. Uses known path prefixes (`.cargo/registry/`, `/rustc/`,
//!   `.rustup/toolchains/`, etc.).
//!
//! - [`is_stdlib_function`] — Checks if a demangled function name belongs to the
//!   standard library by namespace (`core::`, `std::`, `alloc::`), including trait
//!   impl forms like `<core::option::Option<T>>::unwrap`.
//!
//! - `matches_crate_pattern_validated` (in [`crate::sym`]) — Positive matching:
//!   checks if a path belongs to the user's crate based on `src/` patterns derived
//!   from `Cargo.toml`. For single-crate projects, validates against an allowlist
//!   of actual source files to prevent false positives from dependencies that use
//!   relative `src/` paths in their DWARF info.
//!
//! # Panic Entry Points
//!
//! Analysis begins by finding _entry points_ — the low-level functions that the
//! Rust panic runtime calls. The entry points differ between binary and library
//! analysis:
//!
//! - **Binary analysis** uses [`PANIC_SYMBOL_PATTERNS`] to find symbols like
//!   `rust_panic$` in the binary's symbol table, then traces backwards through
//!   the call graph to find user code that reaches them.
//!
//! - **Library analysis** (rlib/staticlib) cannot trace from a single entry point
//!   because library object files are not fully linked. Instead, it uses
//!   [`LIBRARY_PANIC_PATTERNS`] to match demangled relocation targets against
//!   known panic-related functions.
//!
//! - **Abort paths** — Some panics (like OOM via `alloc_error_handler`) go through
//!   `std::process::abort()` instead of the normal panic runtime. These are
//!   matched by [`ABORT_SYMBOL_PATTERNS`].
//!
//! # Panic Cause Classification
//!
//! Once a panic path is found, jonesy classifies _why_ it panics. This happens
//! in `detect_panic_cause` (in [`crate::panic_cause`]) by matching against the
//! function name at the panic site. The classification uses a priority order:
//!
//! 1. **Exact symbol match** — e.g., `panic_bounds_check` → bounds error
//! 2. **Domain detection** — e.g., `core::fmt::` prefix → formatting error
//! 3. **Contextual disambiguation** — e.g., `unwrap_failed` in `option.rs`
//!    vs. `result.rs` distinguishes `JP006` from `JP007`
//! 4. **Fallback** — `PanicCause::Unknown` when no heuristic matches
//!
//! # Direct vs. Indirect Panics
//!
//! [`is_panic_triggering_function`] determines whether a function in the call
//! chain _directly_ triggers a panic (like `unwrap()`, `assert!()`, `panic!()`)
//! or merely calls something that might panic internally. This distinction
//! controls the help message: direct panics suggest alternatives (e.g., "use
//! `if let`"), while indirect panics note the intermediate function.

// ---------------------------------------------------------------------------
// Panic entry point patterns
// ---------------------------------------------------------------------------

/// Symbol patterns for finding panic entry points in **binaries**.
///
/// These are searched in the binary's symbol table (via `nm`-style lookup).
/// The first match found becomes the root of the call-graph trace.
///
/// | Pattern              | Purpose                                     |
/// |----------------------|---------------------------------------------|
/// | `rust_panic$`        | Main Rust panic entry point                 |
/// | `panic_fmt$`         | Core panic formatting (fallback entry)      |
/// | `panic_display`      | Panic display helper                        |
/// | `slice_index_fail`   | Vec/slice index-out-of-bounds panics        |
/// | `str_index_overflow` | String slice boundary violation panics      |
///
/// The `$` suffix in some patterns is significant — it anchors the match to
/// the end of the symbol name to avoid matching functions that merely contain
/// the substring.
pub const PANIC_SYMBOL_PATTERNS: &[&str] = &[
    "rust_panic$",
    "panic_fmt$",
    "panic_display",
    "slice_index_fail",
    "str_index_overflow",
];

/// Symbol patterns for **abort-based** error paths.
///
/// Some error conditions (notably OOM via `alloc_error_handler`) go through
/// `std::process::abort()` instead of the normal panic/unwind machinery.
/// These are traced separately to catch panics that would otherwise be missed.
pub const ABORT_SYMBOL_PATTERNS: &[&str] = &["std::process::abort"];

/// Demangled symbol patterns for finding panic targets in **library** analysis.
///
/// Library analysis (rlib/staticlib) works differently from binary analysis:
/// object files contain relocations to external symbols but have no linked
/// call graph. This list defines the demangled names that indicate a
/// relocation target is panic-related.
///
/// The list is checked with `contains()` matching, so `"core::panicking::panic"`
/// also matches `core::panicking::panic_fmt` etc. Order does not matter.
///
/// # Categories
///
/// **Direct panic functions** — the low-level panic entry points:
/// - `core::panicking::panic`, `panic_fmt`, `panic_display`
/// - `panic_in_cleanup` (panic during drop), `panic_cannot_unwind`
/// - `panic_const` (compile-time overflow checks)
/// - `panic_bounds_check`, `panic_nounwind_fmt`
/// - `assert_failed` (assert/debug_assert macros)
/// - `std::panicking::begin_panic` / `begin_panic_fmt`
///
/// **Option panic functions** — called when unwrapping `None`:
/// - `core::option::Option<T>::unwrap`, `::expect`
/// - `core::option::unwrap_failed` (internal panic function)
///
/// **Result panic functions** — called when unwrapping `Err`:
/// - `core::result::Result<T, E>::unwrap`, `::expect`
/// - `core::result::Result<T, E>::unwrap_err`, `::expect_err`
/// - `core::result::unwrap_failed` (internal panic function)
///
/// # Additional dynamic patterns
///
/// At runtime, the library analysis also matches:
/// - Any symbol containing `core::panicking::` (catches future additions)
/// - `std::panicking::*` except `set_hook`/`take_hook` (which configures
///   the panic-handler, not trigger panics)
pub const LIBRARY_PANIC_PATTERNS: &[&str] = &[
    // Direct panic functions
    "core::panicking::panic",
    "core::panicking::panic_fmt",
    "core::panicking::panic_display",
    "core::panicking::panic_in_cleanup",
    "core::panicking::panic_const",
    "core::panicking::panic_bounds_check",
    "core::panicking::panic_nounwind_fmt",
    "core::panicking::panic_cannot_unwind",
    "core::panicking::assert_failed",
    "std::panicking::begin_panic",
    "std::panicking::begin_panic_fmt",
    // Option panic functions
    "core::option::Option<T>::unwrap",
    "core::option::Option<T>::expect",
    "core::option::unwrap_failed",
    // Result panic functions
    "core::result::Result<T,E>::unwrap",
    "core::result::Result<T,E>::expect",
    "core::result::Result<T,E>::unwrap_err",
    "core::result::Result<T,E>::expect_err",
    "core::result::unwrap_failed",
];

// ---------------------------------------------------------------------------
// Source code ownership heuristics
// ---------------------------------------------------------------------------

/// Check if a file path belongs to a dependency or the standard library.
///
/// Returns `true` if the path should **not** be reported as user code. This is
/// used in both binary and library analysis to filter DWARF line table entries
/// that point into inlined stdlib code.
///
/// # Path categories detected
///
/// | Pattern                      | Source                                    |
/// |------------------------------|-------------------------------------------|
/// | `.cargo/registry/`           | Crates.io dependencies (Unix)             |
/// | `.cargo\registry\`           | Crates.io dependencies (Windows)          |
/// | `/rustc/`                    | Compiler-generated paths (includes hash)  |
/// | `/.rustup/toolchains/`       | Toolchain stdlib source                   |
/// | `/rustlib/src/`              | Stdlib source in sysroot                  |
/// | `/rust/deps/` (prefix)       | Rust CI dependency paths                  |
/// | `library/` (prefix)          | Relative stdlib paths in DWARF            |
/// | `/__/`                       | Generated code boundaries (e.g., objc2)   |
/// | `__` (prefix)                | Generated code (macro-generated modules)  |
/// | `src/__` (prefix)            | Generated code in src directory            |
///
/// # Why both file paths and function names?
///
/// DWARF line tables use file paths while symbol tables use function names.
/// A call to `opt.unwrap()` may be inlined, leaving only the stdlib file path
/// (`option.rs`) in the line table — there is no function boundary to check.
/// Conversely, a function like `core::option::unwrap_failed` appears only in
/// the symbol table. Both checks are needed for complete coverage.
pub fn is_dependency_path(file_path: &str) -> bool {
    // Cargo registry dependencies (absolute paths)
    if file_path.contains(".cargo/registry/") || file_path.contains(".cargo\\registry\\") {
        return true;
    }

    // Rust stdlib and compiler-generated paths
    if file_path.contains("/rustc/")
        || file_path.contains("/.rustup/toolchains/")
        || file_path.contains("/rustlib/src/")
        || file_path.starts_with("/rust/deps/")
        || file_path.starts_with("library/")
    {
        return true;
    }

    // Internal/generated paths from dependencies (common patterns)
    // These use relative src/ paths that would match the "src/" pattern for single-crate projects
    // The __ prefixes are used by macro-generated code in crates like objc2
    // Use segment-boundary checks to avoid false positives on user dirs like /Users/__myuser__/
    if file_path.contains("/__/") || file_path.starts_with("__") || file_path.starts_with("src/__")
    {
        return true;
    }

    false
}

/// Check if a demangled function name belongs to the standard library.
///
/// Returns `true` for functions in the `core`, `std`, or `alloc` crates.
/// Handles multiple name formats that arise from Rust's name mangling:
///
/// | Format                       | Example                                  |
/// |------------------------------|------------------------------------------|
/// | Direct namespace             | `core::option::Option::unwrap`           |
/// | Generic bounds               | `<core::option::Option<T>>::unwrap`      |
/// | Trait impl with space        | `<Foo as core::fmt::Display>::fmt`       |
/// | Nested module reference      | `mycrate::core::panicking::panic`        |
///
/// This function is used during library analysis to skip stdlib callers
/// and report only user-code panic sources.
pub fn is_stdlib_function(name: &str) -> bool {
    name.starts_with("core::")
        || name.starts_with("std::")
        || name.starts_with("alloc::")
        || name.starts_with("<core::")
        || name.starts_with("<std::")
        || name.starts_with("<alloc::")
        || name.contains(" core::")
        || name.contains(" std::")
        || name.contains(" alloc::")
        || name.contains("::core::")
        || name.contains("::std::")
        || name.contains("::alloc::")
}

/// Paths in DWARF that indicate standard library source code.
///
/// Used to identify when a file path points to Rust standard library source.
///
/// These cover both the modern Rust source layout (`/library/core/src/`) and
/// the legacy layout (`/src/libcore/`).
pub const STDLIB_SOURCE_PREFIXES: &[&str] = &[
    "/rustc/",
    // Modern layout (absolute)
    "/library/core/src/",
    "/library/std/src/",
    "/library/alloc/src/",
    // Modern layout (relative — DWARF sometimes omits leading slash)
    "library/core/src/",
    "library/std/src/",
    "library/alloc/src/",
    // Legacy layout
    "/src/libstd/",
    "/src/libcore/",
    "/src/liballoc/",
];

/// Check if a file path points to standard library source code.
///
/// This is a narrower check than [`is_dependency_path`] — it specifically
/// identifies Rust stdlib source files, not all dependencies.
pub fn is_stdlib_source(file_path: &str) -> bool {
    STDLIB_SOURCE_PREFIXES
        .iter()
        .any(|prefix| file_path.contains(prefix))
}

// ---------------------------------------------------------------------------
// Direct vs. indirect panic classification
// ---------------------------------------------------------------------------

/// Check if a function name represents a **direct** panic-triggering function.
///
/// Direct panic functions are those that _immediately_ cause a panic when called
/// (e.g., `unwrap()`, `assert!()`, `panic!()`). Indirect functions are user code
/// that calls something that eventually panics.
///
/// This distinction is used for help messages:
/// - **Direct**: "Use `if let`, `match`, or `unwrap_or` instead"
/// - **Indirect**: "This calls `foo()` which may panic internally"
///
/// # Patterns matched
///
/// **Unwrap/expect variants:**
/// - `unwrap_failed`, `expect_failed` — internal panic functions
/// - `unwrap` (excluding `unwrap_or*`) — `Option::unwrap()` / `Result::unwrap()`
/// - `expect` with `Option` or `Result` — `.expect("msg")`
///
/// **Panic runtime functions:**
/// - `panic_fmt`, `panic_display` — explicit `panic!()` macro
/// - `panic_bounds_check` — array/slice index out of bounds
/// - `panic_const_*` — compile-time overflow/division checks
/// - `panic_in_cleanup`, `panic_cannot_unwind`, `panic_nounwind`
/// - `panic_misaligned_pointer`, `panic_invalid_enum`
///
/// **Assert macros:**
/// - `assert_failed` — `assert!()`, `assert_eq!()`, `assert_ne!()`
///
/// **Capacity/allocation:**
/// - `capacity_overflow` — `Vec::with_capacity(usize::MAX)`
/// - `handle_alloc_error` — OOM handler
///
/// **String/slice errors:**
/// - `slice_error_fail`, `str_index_overflow_fail`
/// - `index<`, `::index<`, `Index::index` — Index trait implementations
pub fn is_panic_triggering_function(func_name: &str) -> bool {
    // Unwrap/expect variants
    func_name.contains("unwrap_failed")
        || func_name.contains("expect_failed")
        // Direct unwrap/expect calls (before they reach _failed)
        || (func_name.contains("unwrap") && !func_name.contains("unwrap_or"))
        || (func_name.contains("expect") && func_name.contains("Option"))
        || (func_name.contains("expect") && func_name.contains("Result"))
        // Panic functions
        || func_name.contains("panic_fmt")
        || func_name.contains("panic_display")
        || func_name.contains("panic_bounds_check")
        || func_name.contains("panic_const_")
        || func_name.contains("panic_in_cleanup")
        || func_name.contains("panic_cannot_unwind")
        || func_name.contains("panic_nounwind")
        || func_name.contains("panic_misaligned_pointer")
        || func_name.contains("panic_invalid_enum")
        // Assert
        || func_name.contains("assert_failed")
        // Capacity/allocation
        || func_name.contains("capacity_overflow")
        || func_name.contains("handle_alloc_error")
        // String/slice errors
        || func_name.contains("slice_error_fail")
        || func_name.contains("str_index_overflow_fail")
        // Index trait - direct bounds check
        || func_name.starts_with("index<")
        || func_name.contains("::index<")
        || func_name.contains("Index::index")
}

/// File path filter for library analysis DWARF entries.
///
/// Used in library (rlib/staticlib) analysis to filter out DWARF file entries
/// that point to stdlib or dependency code. This is applied when processing
/// `CallerInfo` from the `LibraryCallGraph` to ensure only user code paths
/// are reported.
///
/// This is a superset of the checks in [`is_dependency_path`], also covering
/// additional paths that appear in library DWARF info (e.g., `/rust/` CI paths,
/// `/deps/` subdirectories).
pub fn is_library_dependency_path(file_path: &str) -> bool {
    is_dependency_path(file_path) || file_path.starts_with("/rust/") || file_path.contains("/deps/")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- is_dependency_path tests --

    #[test]
    fn test_cargo_registry_unix() {
        assert!(is_dependency_path(
            "/home/user/.cargo/registry/src/crates.io/serde-1.0/src/lib.rs"
        ));
    }

    #[test]
    fn test_cargo_registry_windows() {
        assert!(is_dependency_path(
            "C:\\Users\\user\\.cargo\\registry\\src\\crates.io\\serde-1.0\\src\\lib.rs"
        ));
    }

    #[test]
    fn test_rustc_path() {
        assert!(is_dependency_path(
            "/rustc/abc123/library/core/src/option.rs"
        ));
    }

    #[test]
    fn test_rustup_toolchain() {
        assert!(is_dependency_path(
            "/Users/user/.rustup/toolchains/stable-aarch64-apple-darwin/lib/rustlib/src/rust/library/core/src/option.rs"
        ));
    }

    #[test]
    fn test_rustlib_src() {
        assert!(is_dependency_path("/usr/lib/rustlib/src/rust/core.rs"));
    }

    #[test]
    fn test_relative_library_path() {
        assert!(is_dependency_path("library/core/src/option.rs"));
    }

    #[test]
    fn test_generated_code_boundary() {
        assert!(is_dependency_path("src/__generated/bindings.rs"));
        assert!(is_dependency_path("__objc2/src/lib.rs"));
        assert!(is_dependency_path("some/path/__/generated.rs"));
    }

    #[test]
    fn test_user_code_not_dependency() {
        assert!(!is_dependency_path("src/main.rs"));
        assert!(!is_dependency_path("src/lib.rs"));
        assert!(!is_dependency_path("/Users/user/project/src/module/mod.rs"));
        assert!(!is_dependency_path("examples/demo/src/main.rs"));
    }

    // -- is_stdlib_function tests --

    #[test]
    fn test_stdlib_direct_namespace() {
        assert!(is_stdlib_function("core::option::Option::unwrap"));
        assert!(is_stdlib_function("std::io::Read::read"));
        assert!(is_stdlib_function("alloc::vec::Vec::push"));
    }

    #[test]
    fn test_stdlib_generic_bounds() {
        assert!(is_stdlib_function("<core::option::Option<T>>::unwrap"));
        assert!(is_stdlib_function("<std::vec::Vec<T>>::push"));
    }

    #[test]
    fn test_stdlib_trait_impl() {
        assert!(is_stdlib_function("<MyStruct as core::fmt::Display>::fmt"));
    }

    #[test]
    fn test_user_function_not_stdlib() {
        assert!(!is_stdlib_function("my_crate::module::function"));
        assert!(!is_stdlib_function("cause_an_unwrap"));
    }

    // -- is_panic_triggering_function tests --

    #[test]
    fn test_unwrap_variants() {
        assert!(is_panic_triggering_function("unwrap_failed"));
        assert!(is_panic_triggering_function("expect_failed"));
        assert!(is_panic_triggering_function(
            "core::option::Option<i32>::unwrap"
        ));
        assert!(!is_panic_triggering_function("unwrap_or_default"));
    }

    #[test]
    fn test_panic_functions() {
        assert!(is_panic_triggering_function("panic_fmt"));
        assert!(is_panic_triggering_function("panic_bounds_check"));
        assert!(is_panic_triggering_function("panic_const_add_overflow"));
        assert!(is_panic_triggering_function("panic_misaligned_pointer"));
    }

    #[test]
    fn test_user_function_not_triggering() {
        assert!(!is_panic_triggering_function("my_function"));
        assert!(!is_panic_triggering_function("process_data"));
    }

    // -- is_stdlib_source tests --

    #[test]
    fn test_stdlib_source_paths() {
        assert!(is_stdlib_source("/rustc/abc123/library/core/src/option.rs"));
        assert!(is_stdlib_source("/library/core/src/panicking.rs"));
        assert!(is_stdlib_source("/library/std/src/io/mod.rs"));
        // Relative paths (DWARF sometimes omits leading slash)
        assert!(is_stdlib_source("library/core/src/panicking.rs"));
        assert!(is_stdlib_source("library/std/src/io/mod.rs"));
        assert!(is_stdlib_source("library/alloc/src/vec/mod.rs"));
        // User code and dependencies should not match
        assert!(!is_stdlib_source("src/main.rs"));
        assert!(!is_stdlib_source(
            "/Users/user/.cargo/registry/src/serde/lib.rs"
        ));
    }

    // -- is_library_dependency_path tests --

    #[test]
    fn test_library_dependency_paths() {
        // Inherits all is_dependency_path checks
        assert!(is_library_dependency_path("/rustc/abc/core.rs"));
        assert!(is_library_dependency_path("library/core/src/option.rs"));
        assert!(is_library_dependency_path(
            "/home/user/.cargo/registry/serde.rs"
        ));
        assert!(is_library_dependency_path(
            "/home/user/.rustup/toolchains/stable/lib.rs"
        ));
        // Additional library-specific checks
        assert!(is_library_dependency_path("/rust/deps/std/src/lib.rs"));
        assert!(is_library_dependency_path("target/debug/deps/serde.rs"));
        // User code should not be filtered
        assert!(!is_library_dependency_path("src/main.rs"));
        assert!(!is_library_dependency_path("src/arch/arm64/mod.rs"));
        assert!(!is_library_dependency_path("src/raw/mod.rs"));
    }

    // -- pattern constant tests --

    #[test]
    fn test_panic_symbol_patterns_cover_key_entry_points() {
        assert!(PANIC_SYMBOL_PATTERNS.iter().any(|p| p.contains("panic")));
        assert!(PANIC_SYMBOL_PATTERNS.iter().any(|p| p.contains("slice")));
    }

    #[test]
    fn test_library_panic_patterns_comprehensive() {
        assert!(LIBRARY_PANIC_PATTERNS.iter().any(|p| p.contains("panic")));
        assert!(LIBRARY_PANIC_PATTERNS.iter().any(|p| p.contains("unwrap")));
        assert!(LIBRARY_PANIC_PATTERNS.iter().any(|p| p.contains("expect")));
        assert!(LIBRARY_PANIC_PATTERNS.iter().any(|p| p.contains("option")));
        assert!(LIBRARY_PANIC_PATTERNS.iter().any(|p| p.contains("result")));
    }
}
