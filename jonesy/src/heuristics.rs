//! Panic detection and classification.
//!
//! This module provides pattern matching and classification for panic analysis:
//!
//! # Panic Entry Points
//!
//! Analysis begins by finding _entry points_ — the low-level functions that the
//! Rust panic runtime calls:
//!
//! - **Binary analysis** uses [`find_entry_points`] to locate `rust_panic` and
//!   `std::process::abort` in the binary's symbol table, then traces backwards
//!   through the call graph to find user code that reaches them.
//!
//! - **Library analysis** (rlib/staticlib) uses [`is_library_panic_symbol`] to
//!   match demangled relocation targets against known panic-related functions.
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
// Panic entry point discovery
// ---------------------------------------------------------------------------

use crate::sym::SymbolTable;

/// Regex pattern for finding the main panic entry point in binaries.
/// The `$` anchors the match to the end of the symbol name.
const PANIC_SYMBOL: &str = "rust_panic$";

/// Regex pattern for the unwind entry point.
/// On x86_64, user code calls through `core::panicking::panic_fmt` →
/// `rust_begin_unwind` rather than reaching `rust_panic` directly.
/// Adding this as an entry point ensures the call graph traces back
/// from the point where user code enters the panic runtime.
const UNWIND_SYMBOL: &str = "rust_begin_unwind$";

/// Demangled symbol name for the abort-based error path.
/// OOM via `alloc_error_handler` goes through `std::process::abort()` instead
/// of the normal panic/unwind machinery.
const ABORT_SYMBOL: &str = "std::process::abort";

/// Panic entry point symbols, matched by regex on demangled names.
const PANIC_SYMBOLS: &[&str] = &[PANIC_SYMBOL, UNWIND_SYMBOL];

/// Module prefix for panic entry points in **binary** analysis.
///
/// On x86_64, user code calls `core::panicking::panic_fmt` and related
/// functions directly rather than going through `rust_panic` or
/// `rust_begin_unwind`. Only `core::panicking::` is used here — the
/// `std::panicking::` functions are internal panic runtime plumbing
/// and cause false positives if used as entry points.
const BINARY_PANIC_PREFIXES: &[&str] = &["core::panicking::"];

/// Find panic and abort entry point addresses in the binary's symbol table.
/// Returns `(mangled_name, demangled_name, address)` for each entry point.
pub fn find_entry_points(symbols: &SymbolTable) -> Vec<(String, String, u64)> {
    let mut entry_points = Vec::new();
    let mut seen_addrs = std::collections::HashSet::new();

    // Find panic entry points (rust_panic and rust_begin_unwind)
    for pattern in PANIC_SYMBOLS {
        if let Ok(Some((sym, dem))) = symbols.find_symbol_containing(pattern)
            && let Some(addr) = symbols.find_symbol_address(&sym)
            && seen_addrs.insert(addr)
        {
            entry_points.push((sym, dem, addr));
        }
    }

    // Find core::panicking:: functions as entry points.
    // On x86_64, these are the actual CALL targets from user code.
    // Use find_all_symbols_with_addresses to capture ALL monomorphised instances
    // (e.g., multiple panic_fmt at different addresses).
    if let Ok(panic_symbols) = symbols.find_all_symbols_with_addresses(BINARY_PANIC_PREFIXES) {
        for (sym, dem, addr) in panic_symbols {
            if seen_addrs.insert(addr) {
                entry_points.push((sym, dem, addr));
            }
        }
    }

    // Find abort entry points
    if let Ok(abort_symbols) = symbols.find_all_symbols_with_addresses(&[ABORT_SYMBOL]) {
        for (sym, dem, addr) in abort_symbols {
            if seen_addrs.insert(addr) {
                entry_points.push((sym, dem, addr));
            }
        }
    }

    entry_points
}

/// Module prefixes for panic-related symbols in **library** analysis.
///
/// Any symbol whose demangled name contains one of these prefixes is considered
/// a panic target. This catches `core::panicking::panic_fmt`,
/// `core::panicking::panic_const::panic_const_add_overflow`, etc.
const LIBRARY_PANIC_MODULE_PREFIXES: &[&str] = &["core::panicking::", "std::panicking::"];

/// Exact method suffixes for panic-related symbols in **library** analysis.
///
/// Matched with `ends_with()` to avoid false positives from safe variants
/// like `unwrap_or`, `unwrap_or_default`, `unwrap_or_else`.
const LIBRARY_PANIC_METHOD_SUFFIXES: &[&str] = &[
    ">::unwrap",
    ">::expect",
    ">::unwrap_err",
    ">::expect_err",
    "::unwrap_failed",
    "::expect_failed",
];

/// Check if a demangled symbol name is a panic-related function for library analysis.
///
/// Uses module prefix matching for `core::panicking::`/`std::panicking::` (excluding
/// `set_hook`/`take_hook`), and exact method suffix matching for `unwrap`/`expect`
/// variants to avoid false positives from safe alternatives like `unwrap_or`.
pub fn is_library_panic_symbol(name: &str) -> bool {
    // Check module prefixes (broad match for all core/std panic functions)
    if LIBRARY_PANIC_MODULE_PREFIXES
        .iter()
        .any(|p| name.contains(p))
    {
        return !name.contains("set_hook") && !name.contains("take_hook");
    }

    // Check exact method suffixes (avoids matching unwrap_or, unwrap_or_default, etc.)
    LIBRARY_PANIC_METHOD_SUFFIXES
        .iter()
        .any(|s| name.ends_with(s))
}

// ---------------------------------------------------------------------------
// Direct vs. indirect panic classification
// ---------------------------------------------------------------------------

/// Returns `true` if the function directly triggers a panic.
///
/// This includes all functions recognized by `detect_panic_cause`, plus
/// `panic_fmt` which is the entry point for `panic!("literal")` in Rust 1.78+.
/// `panic_fmt` is not in the rules table (it would cause false positives for
/// all panic paths) but IS a direct panic trigger when called from user code.
pub fn is_panic_triggering_function(func_name: &str) -> bool {
    detect_panic_cause(func_name).is_some() || func_name.contains("panic_fmt")
}

// ---------------------------------------------------------------------------
// Panic cause classification
// ---------------------------------------------------------------------------

use crate::panic_cause::PanicCause;

/// How to match a pattern against a function name.
#[derive(Clone, Copy)]
enum Match {
    /// `func_name.contains(pattern)`
    Contains,
    /// `func_name.ends_with(pattern)`
    EndsWith,
    /// `func_name.contains(pattern)` AND `func_name.contains(second)`
    ContainsBoth(&'static str),
}

/// Rule table for panic cause classification.
///
/// Each entry is `(pattern, match_kind, cause)`. Rules are checked in order;
/// the first match wins. Uses `ends_with` for method names (avoids `unwrap_or*`
/// false positives) and `contains` for internal runtime function names.
const PANIC_CAUSE_RULES: &[(&str, Match, PanicCause)] = &[
    // Async
    (
        "async_fn_resumed",
        Match::Contains,
        PanicCause::AsyncFnResumed,
    ),
    // Cleanup/unwind
    ("panic_in_cleanup", Match::Contains, PanicCause::PanicInDrop),
    (
        "panic_cannot_unwind",
        Match::Contains,
        PanicCause::CannotUnwind,
    ),
    ("panic_nounwind", Match::Contains, PanicCause::CannotUnwind),
    // Bounds check
    (
        "panic_bounds_check",
        Match::Contains,
        PanicCause::BoundsCheck,
    ),
    ("slice_index_fail", Match::Contains, PanicCause::BoundsCheck),
    // Arithmetic overflow (panic_const_* functions)
    (
        "panic_const_add_overflow",
        Match::Contains,
        PanicCause::ArithmeticOverflow("addition"),
    ),
    (
        "panic_const_sub_overflow",
        Match::Contains,
        PanicCause::ArithmeticOverflow("subtraction"),
    ),
    (
        "panic_const_mul_overflow",
        Match::Contains,
        PanicCause::ArithmeticOverflow("multiplication"),
    ),
    (
        "panic_const_div_overflow",
        Match::Contains,
        PanicCause::ArithmeticOverflow("division"),
    ),
    (
        "panic_const_rem_overflow",
        Match::Contains,
        PanicCause::ArithmeticOverflow("remainder"),
    ),
    (
        "panic_const_neg_overflow",
        Match::Contains,
        PanicCause::ArithmeticOverflow("negation"),
    ),
    // Shift overflow
    (
        "panic_const_shl_overflow",
        Match::Contains,
        PanicCause::ShiftOverflow("left"),
    ),
    (
        "panic_const_shr_overflow",
        Match::Contains,
        PanicCause::ShiftOverflow("right"),
    ),
    // Division by zero
    (
        "panic_const_div_by_zero",
        Match::Contains,
        PanicCause::DivisionByZero,
    ),
    (
        "panic_const_rem_by_zero",
        Match::Contains,
        PanicCause::DivisionByZero,
    ),
    // Unwrap (ends_with to avoid matching unwrap_or*)
    ("unwrap_failed", Match::Contains, PanicCause::Unwrap),
    (">::unwrap", Match::EndsWith, PanicCause::Unwrap),
    (">::unwrap_err", Match::EndsWith, PanicCause::Unwrap),
    // Expect (ends_with to avoid matching safe variants)
    ("expect_failed", Match::Contains, PanicCause::Expect),
    (">::expect", Match::EndsWith, PanicCause::Expect),
    (">::expect_err", Match::EndsWith, PanicCause::Expect),
    // Assert
    ("assert_failed", Match::Contains, PanicCause::AssertFailed),
    // Explicit panic!()
    ("panic_display", Match::Contains, PanicCause::ExplicitPanic),
    // Unsafe pointer errors
    (
        "panic_misaligned_pointer_dereference",
        Match::Contains,
        PanicCause::MisalignedPointer,
    ),
    (
        "panic_invalid_enum_construction",
        Match::Contains,
        PanicCause::InvalidEnum,
    ),
    // Formatting domain
    ("core::fmt::", Match::Contains, PanicCause::FormattingError),
    ("alloc::fmt::", Match::Contains, PanicCause::FormattingError),
    ("format_inner", Match::Contains, PanicCause::FormattingError),
    ("write_fmt", Match::Contains, PanicCause::FormattingError),
    // Capacity/allocation
    (
        "capacity_overflow",
        Match::Contains,
        PanicCause::CapacityOverflow,
    ),
    (
        "handle_alloc_error",
        Match::Contains,
        PanicCause::OutOfMemory,
    ),
    (
        "alloc_error_handler",
        Match::Contains,
        PanicCause::OutOfMemory,
    ),
    ("alloc_error_hook", Match::Contains, PanicCause::OutOfMemory),
    // String/slice errors
    (
        "slice_error_fail",
        Match::Contains,
        PanicCause::StringSliceError,
    ),
    (
        "str_index_overflow_fail",
        Match::Contains,
        PanicCause::StringSliceError,
    ),
    (
        "slice_start_index_overflow",
        Match::Contains,
        PanicCause::StringSliceError,
    ),
    (
        "slice_end_index_overflow",
        Match::Contains,
        PanicCause::StringSliceError,
    ),
    // Compound rules (ContainsBoth — both patterns must match)
    (
        "unreachable",
        Match::ContainsBoth("panic"),
        PanicCause::Unreachable,
    ),
    (
        "::fmt",
        Match::ContainsBoth("Display"),
        PanicCause::FormattingError,
    ),
    (
        "::fmt",
        Match::ContainsBoth("Debug"),
        PanicCause::FormattingError,
    ),
    (
        "raw_vec",
        Match::ContainsBoth("grow"),
        PanicCause::CapacityOverflow,
    ),
    // Index trait — subclassify by collection type (before generic Index entries)
    (
        ">::index",
        Match::ContainsBoth("HashMap"),
        PanicCause::KeyNotFound,
    ),
    (
        ">::index",
        Match::ContainsBoth("BTreeMap"),
        PanicCause::KeyNotFound,
    ),
    (
        ">::index",
        Match::ContainsBoth("hash::map"),
        PanicCause::KeyNotFound,
    ),
    (
        ">::index",
        Match::ContainsBoth("btree::map"),
        PanicCause::KeyNotFound,
    ),
    (
        ">::index",
        Match::ContainsBoth("str::"),
        PanicCause::StringSliceError,
    ),
    // Index trait — str subclassification for index< patterns
    (
        "index<",
        Match::ContainsBoth("str::"),
        PanicCause::StringSliceError,
    ),
    // Index trait — generic (after subclassifications)
    (">::index", Match::Contains, PanicCause::BoundsCheck),
    ("Index::index", Match::Contains, PanicCause::BoundsCheck),
    ("index<", Match::Contains, PanicCause::BoundsCheck),
    // Collection internals (after Index trait so HashMap gets KeyNotFound)
    (
        "hashbrown::raw::",
        Match::Contains,
        PanicCause::CapacityOverflow,
    ),
    (
        "std::collections::hash::",
        Match::Contains,
        PanicCause::CapacityOverflow,
    ),
];

/// Detect panic cause from a function name in the call chain.
///
/// Iterates [`PANIC_CAUSE_RULES`] in order; first match wins.
pub fn detect_panic_cause(func_name: &str) -> Option<PanicCause> {
    for (pattern, kind, cause) in PANIC_CAUSE_RULES {
        let matched = match kind {
            Match::Contains => func_name.contains(pattern),
            Match::EndsWith => func_name.ends_with(pattern),
            Match::ContainsBoth(second) => {
                func_name.contains(pattern) && func_name.contains(second)
            }
        };
        if matched {
            return Some(cause.clone());
        }
    }
    None
}

/// Check whether a source line contains arithmetic where all operands are
/// compile-time constants.
///
/// Returns `true` if the line has an arithmetic operator (`+`, `-`, `*`, `/`, `%`)
/// and every operand is either a numeric literal or a `SCREAMING_CASE` identifier
/// (the Rust convention for constants). Such arithmetic cannot overflow at runtime
/// because the compiler evaluates and validates it at compile time — if the result
/// overflowed, the code would not compile.
///
/// This is used to suppress `ArithmeticOverflow` and `ShiftOverflow` detections
/// for const-only expressions (see issue #266).
pub(crate) fn is_const_arithmetic(line: &str) -> bool {
    let trimmed = line.trim();

    // Strip common prefixes: `let _ = ...;`, `let name = ...;`, `let name: Type = ...;`
    let expr = if let Some(rest) = trimmed.strip_prefix("let ") {
        // Skip past the binding pattern and `=`
        rest.split_once('=')
            .map(|(_, rhs)| rhs.trim())
            .unwrap_or("")
    } else {
        trimmed
    };

    // Strip trailing semicolons and comments
    let expr = expr.split("//").next().unwrap_or(expr).trim();
    let expr = expr.trim_end_matches(';').trim();

    if expr.is_empty() {
        return false;
    }

    // Find an arithmetic operator in the expression
    let arithmetic_ops = [" + ", " - ", " * ", " / ", " % ", " << ", " >> "];
    let has_arithmetic = arithmetic_ops.iter().any(|op| expr.contains(op));
    if !has_arithmetic {
        return false;
    }

    // Split on arithmetic operators and check every token is a constant
    let tokens: Vec<&str> = expr
        .split(['+', '-', '*', '/', '%'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();

    if tokens.len() < 2 {
        return false;
    }

    tokens.iter().all(|token| is_const_token(token))
}

/// Check whether a token looks like a compile-time constant.
///
/// A token is considered a constant if it is:
/// - A numeric literal (decimal, hex `0x`, octal `0o`, binary `0b`, with optional `_` separators
///   and optional type suffix like `usize`, `i32`)
/// - A `SCREAMING_SNAKE_CASE` identifier (the Rust convention for `const` and `static` values)
fn is_const_token(token: &str) -> bool {
    let token = token.trim();
    if token.is_empty() {
        return false;
    }

    // Numeric literal: starts with a digit
    if token.starts_with(|c: char| c.is_ascii_digit()) {
        // Strip type suffix (e.g., `1usize`, `0xFFi32`)
        let num_part = token.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '_');
        if num_part.is_empty() {
            return false;
        }
        // Validate the remaining part looks like a number
        let num_part = num_part.replace('_', "");
        return if num_part.starts_with("0x") || num_part.starts_with("0X") {
            num_part[2..].chars().all(|c| c.is_ascii_hexdigit())
        } else if num_part.starts_with("0o") || num_part.starts_with("0O") {
            num_part[2..].chars().all(|c| ('0'..='7').contains(&c))
        } else if num_part.starts_with("0b") || num_part.starts_with("0B") {
            num_part[2..].chars().all(|c| c == '0' || c == '1')
        } else {
            num_part.chars().all(|c| c.is_ascii_digit())
        };
    }

    // SCREAMING_SNAKE_CASE identifier: all uppercase letters, digits, and underscores,
    // must contain at least one uppercase letter, must not start with a digit
    if token.starts_with(|c: char| c.is_ascii_uppercase()) {
        return token
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    }

    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panic_cause::PanicCause;

    // -- pattern constant tests --

    #[test]
    fn test_panic_symbol_patterns_cover_key_entry_points() {
        assert!(PANIC_SYMBOL.contains("panic"));
        assert!(ABORT_SYMBOL.contains("abort"));
    }

    #[test]
    fn test_is_library_panic_symbol() {
        // Module prefix matches
        assert!(is_library_panic_symbol("core::panicking::panic_fmt"));
        assert!(is_library_panic_symbol(
            "core::panicking::panic_const::panic_const_add_overflow"
        ));
        assert!(is_library_panic_symbol("std::panicking::begin_panic"));
        // set_hook/take_hook are NOT panic functions
        assert!(!is_library_panic_symbol("std::panicking::set_hook"));
        assert!(!is_library_panic_symbol("std::panicking::take_hook"));
        // User code is not a panic symbol
        assert!(!is_library_panic_symbol("my_crate::process_data"));

        // Exact method suffix matches (with full demangled paths)
        assert!(is_library_panic_symbol("core::option::Option<T>::unwrap"));
        assert!(is_library_panic_symbol("core::option::Option<T>::expect"));
        assert!(is_library_panic_symbol("core::result::Result<T,E>::unwrap"));
        assert!(is_library_panic_symbol(
            "core::result::Result<T,E>::unwrap_err"
        ));
        assert!(is_library_panic_symbol(
            "core::result::Result<T,E>::expect_err"
        ));
        assert!(is_library_panic_symbol("core::option::unwrap_failed"));
        assert!(is_library_panic_symbol("core::option::expect_failed"));
        assert!(is_library_panic_symbol("core::result::unwrap_failed"));

        // Safe variants must NOT match
        assert!(!is_library_panic_symbol(
            "core::option::Option<T>::unwrap_or"
        ));
        assert!(!is_library_panic_symbol(
            "core::option::Option<T>::unwrap_or_default"
        ));
        assert!(!is_library_panic_symbol(
            "core::option::Option<T>::unwrap_or_else"
        ));
        assert!(!is_library_panic_symbol(
            "core::result::Result<T,E>::unwrap_or"
        ));
        assert!(!is_library_panic_symbol(
            "core::result::Result<T,E>::unwrap_or_default"
        ));
        assert!(!is_library_panic_symbol(
            "core::result::Result<T,E>::unwrap_or_else"
        ));
    }

    #[test]
    fn test_is_library_panic_symbol_unwrap_err_expect_err() {
        // Short names (as seen in some demangled output)
        assert!(is_library_panic_symbol("Result<T,E>::unwrap_err"));
        assert!(is_library_panic_symbol("Result<T,E>::expect_err"));
    }

    #[test]
    fn test_is_library_panic_symbol_expect_failed() {
        assert!(is_library_panic_symbol("core::option::expect_failed"));
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
            Some(PanicCause::ArithmeticOverflow("addition"))
        );
        assert_eq!(
            detect_panic_cause("panic_const_sub_overflow"),
            Some(PanicCause::ArithmeticOverflow("subtraction"))
        );
        assert_eq!(
            detect_panic_cause("panic_const_mul_overflow"),
            Some(PanicCause::ArithmeticOverflow("multiplication"))
        );
    }

    #[test]
    fn test_detect_panic_cause_shift_overflow() {
        assert_eq!(
            detect_panic_cause("panic_const_shl_overflow"),
            Some(PanicCause::ShiftOverflow("left"))
        );
        assert_eq!(
            detect_panic_cause("panic_const_shr_overflow"),
            Some(PanicCause::ShiftOverflow("right"))
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
    fn test_detect_panic_cause_unwrap_method() {
        assert_eq!(
            detect_panic_cause("core::option::Option<T>::unwrap"),
            Some(PanicCause::Unwrap)
        );
        assert_eq!(
            detect_panic_cause("core::result::Result<T,E>::unwrap"),
            Some(PanicCause::Unwrap)
        );
        assert_eq!(
            detect_panic_cause("core::result::Result<T,E>::unwrap_err"),
            Some(PanicCause::Unwrap)
        );
        // Safe variants must NOT return Unwrap
        assert_ne!(
            detect_panic_cause("core::option::Option<T>::unwrap_or"),
            Some(PanicCause::Unwrap)
        );
        assert_ne!(
            detect_panic_cause("core::option::Option<T>::unwrap_or_default"),
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
    fn test_detect_panic_cause_expect_method() {
        assert_eq!(
            detect_panic_cause("core::result::Result<T,E>::expect"),
            Some(PanicCause::Expect)
        );
        assert_eq!(
            detect_panic_cause("core::result::Result<T,E>::expect_err"),
            Some(PanicCause::Expect)
        );
        assert_eq!(
            detect_panic_cause("core::option::Option<T>::expect"),
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
    fn test_detect_panic_cause_explicit_panic() {
        assert_eq!(
            detect_panic_cause("panic_display"),
            Some(PanicCause::ExplicitPanic)
        );
        // panic_fmt is NOT classified via detect_panic_cause (it's the generic
        // runtime entry). Instead, is_panic_triggering_function handles it
        // separately so that direct panic!() calls get ExplicitPanic via
        // assign_unknown_causes.
        assert_eq!(detect_panic_cause("panic_fmt"), None);
    }

    #[test]
    fn test_detect_panic_cause_safe_variants_return_none() {
        assert_eq!(detect_panic_cause("unwrap_or_default"), None);
        assert_eq!(
            detect_panic_cause("core::option::Option<T>::unwrap_or"),
            None
        );
        assert_eq!(
            detect_panic_cause("core::option::Option<T>::unwrap_or_else"),
            None
        );
        assert_eq!(detect_panic_cause("my_function"), None);
        assert_eq!(detect_panic_cause("process_data"), None);
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
        // Fully qualified demangled names contain both the Index trait pattern and "str::"
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

    /// Issue #266: const arithmetic (e.g., `CONST + 1`) cannot overflow at runtime
    /// because the compiler would reject it at compile time. Jonesy should suppress
    /// arithmetic overflow detections when the source line only involves constants.
    #[test]
    fn test_const_arithmetic_not_reported_as_overflow() {
        // Lines where both operands are compile-time constants
        assert!(is_const_arithmetic("    let _result = OFFSET + 1;"));
        assert!(is_const_arithmetic(
            "    let x = PATH_OFFSET + HEADER_SIZE;"
        ));
        assert!(is_const_arithmetic("    let _ = 7 + 1;"));
        assert!(is_const_arithmetic("    let _ = CONST_A - CONST_B;"));
        assert!(is_const_arithmetic("    let _ = SIZE * 2;"));
        assert!(is_const_arithmetic("    let _ = TOTAL / 4;"));

        // Lines where at least one operand is a runtime variable — NOT const arithmetic
        assert!(!is_const_arithmetic("    let _ = x + 1;"));
        assert!(!is_const_arithmetic("    let _ = input * 2;"));
        assert!(!is_const_arithmetic("    let _ = vec.len() + offset;"));
        assert!(!is_const_arithmetic("    opt.unwrap();"));
        assert!(!is_const_arithmetic("    let _ = a.checked_add(b);"));
    }
}
