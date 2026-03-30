//! Panic detection and classification.
//!
//! This module provides pattern matching and classification for panic analysis:
//!
//! # Panic Entry Points
//!
//! Analysis begins by finding _entry points_ — the low-level functions that the
//! Rust panic runtime calls:
//!
//! - **Binary analysis** uses [`PANIC_SYMBOL_PATTERNS`] to find symbols like
//!   `rust_panic$` in the binary's symbol table, then traces backwards through
//!   the call graph to find user code that reaches them.
//!
//! - **Library analysis** (rlib/staticlib) uses [`is_library_panic_symbol`] to
//!   match demangled relocation targets against known panic-related functions.
//!
//! - **Abort paths** — Some panics (like OOM via `alloc_error_handler`) go through
//!   `std::process::abort()` instead of the normal panic runtime. These are
//!   matched by [`ABORT_SYMBOL_PATTERNS`].
//!
//! # Panic Cause Classification
//!
//! Once a panic path is found, jonesy classifies _why_ it panics. This happens
//! in [`detect_panic_cause`] by matching against the fully qualified demangled
//! function name. The classification uses a priority order:
//!
//! 1. **Exact symbol match** — e.g., `panic_bounds_check` → bounds error
//! 2. **Domain detection** — e.g., `core::fmt::` prefix → formatting error
//! 3. **Fallback** — `None` when no pattern matches (caller assigns `Unknown`)
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
/// Checked with `contains()` matching, so `"core::panicking::"` matches all
/// functions in that module. `std::panicking::` excludes `set_hook`/`take_hook`
/// via [`is_library_panic_symbol`].
const LIBRARY_PANIC_PATTERNS: &[&str] = &[
    // All core panic functions (panic, panic_fmt, panic_const, assert_failed, etc.)
    "core::panicking::",
    // std panic entry points (begin_panic, begin_panic_fmt, etc.)
    "std::panicking::",
    // Option/Result unwrap/expect (appear as relocation targets in library analysis)
    "Option<T>::unwrap",
    "Option<T>::expect",
    "Result<T,E>::unwrap",
    "Result<T,E>::expect",
    "Result<T,E>::unwrap_err",
    "Result<T,E>::expect_err",
    "core::option::unwrap_failed",
    "core::option::expect_failed",
    "core::result::unwrap_failed",
];

/// Check if a demangled symbol name is a panic-related function for library analysis.
///
/// Uses [`LIBRARY_PANIC_PATTERNS`] with an exclusion for `set_hook`/`take_hook`
/// which configure the panic handler but don't trigger panics.
pub fn is_library_panic_symbol(name: &str) -> bool {
    if LIBRARY_PANIC_PATTERNS.iter().any(|p| name.contains(p)) {
        // Exclude panic handler configuration functions
        !name.contains("set_hook") && !name.contains("take_hook")
    } else {
        false
    }
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
        // Async function resumed after completion
        || func_name.contains("async_fn_resumed")
        // String/slice errors
        || func_name.contains("slice_error_fail")
        || func_name.contains("str_index_overflow_fail")
        // Index trait - direct bounds check
        // Matches both simple names ("index<T>") and fully qualified demangled
        // linkage names ("<impl Index<I> for str>::index")
        || func_name.starts_with("index<")
        || func_name.contains("::index<")
        || func_name.contains("Index::index")
        || func_name.contains(">::index")
}

// ---------------------------------------------------------------------------
// Panic cause classification
// ---------------------------------------------------------------------------

use crate::panic_cause::PanicCause;

/// Detect panic cause from a function name in the call chain.
///
/// Uses fully qualified demangled function names (from DWARF linkage names)
/// for disambiguation (e.g., `core::result::unwrap_failed` vs
/// `core::option::unwrap_failed`).
pub fn detect_panic_cause(func_name: &str) -> Option<PanicCause> {
    // Check for async function resumed after completion
    if func_name.contains("async_fn_resumed") {
        return Some(PanicCause::AsyncFnResumed);
    }

    // Check for drop/cleanup panic paths first
    if func_name.contains("panic_in_cleanup") {
        return Some(PanicCause::PanicInDrop);
    }
    if func_name.contains("panic_cannot_unwind") || func_name.contains("panic_nounwind") {
        return Some(PanicCause::CannotUnwind);
    }

    // Check for specific panic functions
    if func_name.contains("panic_bounds_check") {
        return Some(PanicCause::BoundsCheck);
    }
    if func_name.contains("panic_const_add_overflow") {
        return Some(PanicCause::ArithmeticOverflow("addition".to_string()));
    }
    if func_name.contains("panic_const_sub_overflow") {
        return Some(PanicCause::ArithmeticOverflow("subtraction".to_string()));
    }
    if func_name.contains("panic_const_mul_overflow") {
        return Some(PanicCause::ArithmeticOverflow("multiplication".to_string()));
    }
    if func_name.contains("panic_const_div_overflow") {
        return Some(PanicCause::ArithmeticOverflow("division".to_string()));
    }
    if func_name.contains("panic_const_rem_overflow") {
        return Some(PanicCause::ArithmeticOverflow("remainder".to_string()));
    }
    if func_name.contains("panic_const_neg_overflow") {
        return Some(PanicCause::ArithmeticOverflow("negation".to_string()));
    }
    if func_name.contains("panic_const_shl_overflow") {
        return Some(PanicCause::ShiftOverflow("left".to_string()));
    }
    if func_name.contains("panic_const_shr_overflow") {
        return Some(PanicCause::ShiftOverflow("right".to_string()));
    }
    if func_name.contains("panic_const_div_by_zero") {
        return Some(PanicCause::DivisionByZero);
    }
    if func_name.contains("panic_const_rem_by_zero") {
        return Some(PanicCause::DivisionByZero);
    }
    // unwrap/expect detection
    if func_name.contains("unwrap_failed") {
        return Some(PanicCause::Unwrap);
    }
    if func_name.contains("expect_failed")
        || (func_name.contains("expect") && func_name.contains("Result"))
    {
        return Some(PanicCause::Expect);
    }
    // Assert macros - both assert!() and debug_assert!() compile to the same
    // assert_failed function, so we cannot distinguish them at the binary level.
    if func_name.contains("assert_failed") {
        return Some(PanicCause::AssertFailed);
    }
    // panic_display is explicit panic! with a simple message
    if func_name.contains("panic_display") {
        return Some(PanicCause::ExplicitPanic);
    }
    // Check for unreachable/unimplemented/todo patterns
    if func_name.contains("unreachable") && func_name.contains("panic") {
        return Some(PanicCause::Unreachable);
    }

    // ============================================================
    // Stdlib domain detection - detect panics from specific domains
    // ============================================================

    // Formatting domain (core::fmt::, alloc::fmt::)
    // These functions are in the call chain when format!/write!/Display/Debug panic
    if func_name.contains("core::fmt::") || func_name.contains("alloc::fmt::") {
        return Some(PanicCause::FormattingError);
    }
    if func_name.contains("format_inner") || func_name.contains("write_fmt") {
        return Some(PanicCause::FormattingError);
    }
    // Display/Debug trait formatting
    if func_name.contains("::fmt") && (func_name.contains("Display") || func_name.contains("Debug"))
    {
        return Some(PanicCause::FormattingError);
    }

    // Capacity/allocation domain
    if func_name.contains("capacity_overflow") {
        return Some(PanicCause::CapacityOverflow);
    }
    if func_name.contains("handle_alloc_error")
        || func_name.contains("alloc_error_handler")
        || func_name.contains("alloc_error_hook")
    {
        return Some(PanicCause::OutOfMemory);
    }
    if func_name.contains("raw_vec") && func_name.contains("grow") {
        return Some(PanicCause::CapacityOverflow);
    }

    // String/slice domain
    if func_name.contains("slice_error_fail") {
        return Some(PanicCause::StringSliceError);
    }
    if func_name.contains("str_index_overflow_fail") {
        return Some(PanicCause::StringSliceError);
    }
    if func_name.contains("slice_start_index_overflow")
        || func_name.contains("slice_end_index_overflow")
    {
        return Some(PanicCause::StringSliceError);
    }

    // Bounds checking domain - detect from Index trait implementations
    // These are called from user code when indexing slices/vecs
    // Matches both simple names ("index<T, usize>") and fully qualified demangled
    // linkage names ("<impl Index<I> for str>::index")
    if func_name.starts_with("index<")
        || func_name.contains("::index<")
        || func_name.contains("Index::index")
        || func_name.contains(">::index")
    {
        // Check if it's HashMap/BTreeMap indexing (key not found panic)
        let is_map_op = func_name.contains("HashMap")
            || func_name.contains("BTreeMap")
            || func_name.contains("hash::map")
            || func_name.contains("btree::map");
        if is_map_op {
            return Some(PanicCause::KeyNotFound);
        }

        // Check if it's for str (string slice) vs array/vec (bounds check)
        // Fully qualified names like "core::str::traits::...::index" contain "str::"
        if func_name.contains("str::") {
            return Some(PanicCause::StringSliceError);
        }
        return Some(PanicCause::BoundsCheck);
    }

    // Invalid enum discriminant - happens with unsafe enum transmutes or memory corruption
    if func_name.contains("panic_invalid_enum_construction") {
        return Some(PanicCause::InvalidEnum);
    }

    // Misaligned pointer dereference - unsafe code dereferencing misaligned pointers
    if func_name.contains("panic_misaligned_pointer_dereference") {
        return Some(PanicCause::MisalignedPointer);
    }

    // ============================================================
    // Collection internals - hashbrown raw table operations
    // ============================================================
    // hashbrown::raw:: contains the low-level hash table allocation/layout/capacity
    // functions. When these appear on a panic path, it indicates a capacity overflow
    // or allocation failure during HashMap/HashSet operations.
    // This is more specific than the CannotUnwind cause that gets detected earlier
    // from panic_nounwind_fmt on the allocator error path.
    if func_name.contains("hashbrown::raw::") {
        return Some(PanicCause::CapacityOverflow);
    }

    // std::collections::hash HashMap/HashSet creation and allocation functions
    // may panic through hasher initialization (thread-local storage) or internal
    // allocation. When no more specific cause is detected from the panic path,
    // classify as capacity overflow since that's the most actionable cause.
    if func_name.contains("std::collections::hash::") {
        return Some(PanicCause::CapacityOverflow);
    }

    // panic_fmt is the core panic function - if we reach here without a more
    // specific match, leave cause as None (unknown) to avoid incorrect labeling.
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panic_cause::PanicCause;

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

    // -- pattern constant tests --

    #[test]
    fn test_panic_symbol_patterns_cover_key_entry_points() {
        assert!(PANIC_SYMBOL_PATTERNS.iter().any(|p| p.contains("panic")));
        assert!(PANIC_SYMBOL_PATTERNS.iter().any(|p| p.contains("slice")));
    }

    #[test]
    fn test_is_library_panic_symbol() {
        assert!(is_library_panic_symbol("core::panicking::panic_fmt"));
        assert!(is_library_panic_symbol(
            "core::panicking::panic_const::panic_const_add_overflow"
        ));
        assert!(is_library_panic_symbol("std::panicking::begin_panic"));
        assert!(is_library_panic_symbol("core::option::unwrap_failed"));
        assert!(is_library_panic_symbol("core::result::unwrap_failed"));
        // set_hook/take_hook are NOT panic functions
        assert!(!is_library_panic_symbol("std::panicking::set_hook"));
        assert!(!is_library_panic_symbol("std::panicking::take_hook"));
        // User code is not a panic symbol
        assert!(!is_library_panic_symbol("my_crate::process_data"));
    }

    // -- detect_panic_cause tests --

    #[test]
    fn test_detect_panic_cause_bounds_check() {
        assert_eq!(
            detect_panic_cause("panic_bounds_check"),
            Some(PanicCause::BoundsCheck)
        );
    }

    #[test]
    fn test_detect_panic_cause_arithmetic_overflow() {
        assert_eq!(
            detect_panic_cause("panic_const_add_overflow"),
            Some(PanicCause::ArithmeticOverflow("addition".to_string()))
        );
        assert_eq!(
            detect_panic_cause("panic_const_sub_overflow"),
            Some(PanicCause::ArithmeticOverflow("subtraction".to_string()))
        );
        assert_eq!(
            detect_panic_cause("panic_const_mul_overflow"),
            Some(PanicCause::ArithmeticOverflow("multiplication".to_string()))
        );
    }

    #[test]
    fn test_detect_panic_cause_shift_overflow() {
        assert_eq!(
            detect_panic_cause("panic_const_shl_overflow"),
            Some(PanicCause::ShiftOverflow("left".to_string()))
        );
        assert_eq!(
            detect_panic_cause("panic_const_shr_overflow"),
            Some(PanicCause::ShiftOverflow("right".to_string()))
        );
    }

    #[test]
    fn test_detect_panic_cause_division_by_zero() {
        assert_eq!(
            detect_panic_cause("panic_const_div_by_zero"),
            Some(PanicCause::DivisionByZero)
        );
        assert_eq!(
            detect_panic_cause("panic_const_rem_by_zero"),
            Some(PanicCause::DivisionByZero)
        );
    }

    #[test]
    fn test_detect_panic_cause_unwrap_failed() {
        assert_eq!(
            detect_panic_cause("unwrap_failed"),
            Some(PanicCause::Unwrap)
        );
        assert_eq!(
            detect_panic_cause("core::option::unwrap_failed"),
            Some(PanicCause::Unwrap)
        );
        assert_eq!(
            detect_panic_cause("core::result::unwrap_failed"),
            Some(PanicCause::Unwrap)
        );
    }

    #[test]
    fn test_detect_panic_cause_expect_failed() {
        assert_eq!(
            detect_panic_cause("expect_failed"),
            Some(PanicCause::Expect)
        );
    }

    #[test]
    fn test_detect_panic_cause_result_expect() {
        assert_eq!(
            detect_panic_cause("Result::expect"),
            Some(PanicCause::Expect)
        );
    }

    #[test]
    fn test_detect_panic_cause_assert_failed() {
        assert_eq!(
            detect_panic_cause("assert_failed"),
            Some(PanicCause::AssertFailed)
        );
    }

    #[test]
    fn test_detect_panic_cause_panic_display() {
        assert_eq!(
            detect_panic_cause("panic_display"),
            Some(PanicCause::ExplicitPanic)
        );
    }

    #[test]
    fn test_detect_panic_cause_panic_in_cleanup() {
        assert_eq!(
            detect_panic_cause("panic_in_cleanup"),
            Some(PanicCause::PanicInDrop)
        );
    }

    #[test]
    fn test_detect_panic_cause_panic_cannot_unwind() {
        assert_eq!(
            detect_panic_cause("panic_cannot_unwind"),
            Some(PanicCause::CannotUnwind)
        );
        assert_eq!(
            detect_panic_cause("panic_nounwind"),
            Some(PanicCause::CannotUnwind)
        );
    }

    #[test]
    fn test_detect_panic_cause_formatting() {
        assert_eq!(
            detect_panic_cause("core::fmt::write"),
            Some(PanicCause::FormattingError)
        );
        assert_eq!(
            detect_panic_cause("write_fmt"),
            Some(PanicCause::FormattingError)
        );
    }

    #[test]
    fn test_detect_panic_cause_capacity_overflow() {
        assert_eq!(
            detect_panic_cause("capacity_overflow"),
            Some(PanicCause::CapacityOverflow)
        );
    }

    #[test]
    fn test_detect_panic_cause_out_of_memory() {
        assert_eq!(
            detect_panic_cause("handle_alloc_error"),
            Some(PanicCause::OutOfMemory)
        );
    }

    #[test]
    fn test_detect_panic_cause_string_slice_error() {
        assert_eq!(
            detect_panic_cause("slice_error_fail"),
            Some(PanicCause::StringSliceError)
        );
        assert_eq!(
            detect_panic_cause("str_index_overflow_fail"),
            Some(PanicCause::StringSliceError)
        );
    }

    #[test]
    fn test_detect_panic_cause_index_bounds() {
        assert_eq!(
            detect_panic_cause("index<T, usize>"),
            Some(PanicCause::BoundsCheck)
        );
        assert_eq!(
            detect_panic_cause("Index::index"),
            Some(PanicCause::BoundsCheck)
        );
    }

    #[test]
    fn test_detect_panic_cause_index_string() {
        // Fully qualified demangled names contain both Index trait pattern and "str::"
        assert_eq!(
            detect_panic_cause("core::str::traits::<impl Index<I> for str>::index"),
            Some(PanicCause::StringSliceError)
        );
        assert_eq!(
            detect_panic_cause("str::index<Range>"),
            Some(PanicCause::StringSliceError)
        );
    }

    #[test]
    fn test_detect_panic_cause_invalid_enum() {
        assert_eq!(
            detect_panic_cause("panic_invalid_enum_construction"),
            Some(PanicCause::InvalidEnum)
        );
    }

    #[test]
    fn test_detect_panic_cause_misaligned_pointer() {
        assert_eq!(
            detect_panic_cause("panic_misaligned_pointer_dereference"),
            Some(PanicCause::MisalignedPointer)
        );
    }

    #[test]
    fn test_detect_panic_cause_hashbrown_raw() {
        assert_eq!(
            detect_panic_cause("hashbrown::raw::TableLayout::calculate_layout_for"),
            Some(PanicCause::CapacityOverflow)
        );
        assert_eq!(
            detect_panic_cause("hashbrown::raw::RawTableInner::fallible_with_capacity"),
            Some(PanicCause::CapacityOverflow)
        );
        assert_eq!(
            detect_panic_cause("hashbrown::raw::RawTableInner::new_uninitialized"),
            Some(PanicCause::CapacityOverflow)
        );
    }

    #[test]
    fn test_detect_panic_cause_std_collections_hash() {
        assert_eq!(
            detect_panic_cause("std::collections::hash::map::HashMap<K,V>::new"),
            Some(PanicCause::CapacityOverflow)
        );
        assert_eq!(
            detect_panic_cause("std::collections::hash::set::HashSet<T>::with_capacity"),
            Some(PanicCause::CapacityOverflow)
        );
    }

    #[test]
    fn test_detect_hashbrown_capacity_overflow() {
        assert_eq!(
            detect_panic_cause("hashbrown::raw::Fallibility::capacity_overflow"),
            Some(PanicCause::CapacityOverflow)
        );
        // alloc_err also falls through to hashbrown::raw:: catch-all
        assert_eq!(
            detect_panic_cause("hashbrown::raw::Fallibility::alloc_err"),
            Some(PanicCause::CapacityOverflow)
        );
    }

    #[test]
    fn test_detect_panic_cause_unknown() {
        assert_eq!(detect_panic_cause("some_random_function"), None);
    }

    #[test]
    fn test_detect_panic_cause_unreachable() {
        assert_eq!(
            detect_panic_cause("unreachable_panic_handler"),
            Some(PanicCause::Unreachable)
        );
    }

    #[test]
    fn test_detect_panic_cause_raw_vec_grow() {
        assert_eq!(
            detect_panic_cause("raw_vec::grow"),
            Some(PanicCause::CapacityOverflow)
        );
    }

    #[test]
    fn test_detect_panic_cause_display_fmt() {
        assert_eq!(
            detect_panic_cause("Display::fmt"),
            Some(PanicCause::FormattingError)
        );
        assert_eq!(
            detect_panic_cause("Debug::fmt"),
            Some(PanicCause::FormattingError)
        );
    }

    #[test]
    fn test_detect_panic_cause_async_fn_resumed() {
        assert_eq!(
            detect_panic_cause("panic_const_async_fn_resumed"),
            Some(PanicCause::AsyncFnResumed)
        );
        assert_eq!(
            detect_panic_cause("core::panicking::panic_const::panic_const_async_fn_resumed"),
            Some(PanicCause::AsyncFnResumed)
        );
        // Also matches the "panic" variant
        assert_eq!(
            detect_panic_cause("panic_const_async_fn_resumed_panic"),
            Some(PanicCause::AsyncFnResumed)
        );
    }

    #[test]
    fn test_async_fn_resumed_is_panic_triggering() {
        assert!(is_panic_triggering_function("panic_const_async_fn_resumed"));
        assert!(is_panic_triggering_function(
            "core::panicking::panic_const::panic_const_async_fn_resumed"
        ));
    }
}
