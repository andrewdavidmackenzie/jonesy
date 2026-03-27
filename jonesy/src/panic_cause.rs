//! Panic cause detection and explanations.
//!
//! This module identifies the source of potential panics by analyzing
//! function names in the call chain and provides helpful suggestions.
//!
//! # Debug vs Release Build Behavior
//!
//! In Rust, most panic causes occur in both debug and release builds. Only a few
//! are affected by Cargo profile settings:
//!
//! | Panic Cause | Debug Build (default) | Release Build (default) |
//! |-------------|----------------------|-------------------------|
//! | Arithmetic overflow | Panics | Wraps (configurable via `overflow-checks`) |
//! | Shift overflow | Panics | Wraps (configurable via `overflow-checks`) |
//! | Division by zero | Panics | Panics |
//! | Index out of bounds | Panics | Panics |
//! | All other causes | Panics | Panics |
//!
//! **Note**: The behaviors above are **defaults** that can be changed via Cargo profile
//! settings. For example, you can enable `overflow-checks` in release builds.
//!
//! **Important**: Safe Rust never has undefined behavior, regardless of build profile.
//! Bounds checking and division-by-zero checks are never removed in release builds.
//!
//! # References
//!
//! - [Cargo Profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) -
//!   Documents `overflow-checks` and `debug-assertions` settings
//! - [Behavior Considered Undefined](https://doc.rust-lang.org/reference/behavior-considered-undefined.html) -
//!   Confirms safe Rust never has UB

/// Known panic causes with explanations and suggestions
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(dead_code)] // Some variants reserved for future detection
pub enum PanicCause {
    /// Explicit panic!() macro
    ExplicitPanic,
    /// Array or slice index out of bounds
    BoundsCheck,
    /// Arithmetic overflow (add, sub, mul, div, rem, neg)
    ArithmeticOverflow(String),
    /// Shift overflow (shl, shr)
    ShiftOverflow(String),
    /// Division by zero
    DivisionByZero,
    /// Unwrap on None
    UnwrapNone,
    /// Unwrap on Err
    UnwrapErr,
    /// Expect on None
    ExpectNone,
    /// Expect on Err
    ExpectErr,
    /// Assert failed (includes both assert!() and debug_assert!())
    AssertFailed,
    /// Unreachable code reached
    Unreachable,
    /// Unimplemented code reached
    Unimplemented,
    /// Todo macro reached
    Todo,
    /// Panic during drop/cleanup
    PanicInDrop,
    /// Panic in no-unwind context (e.g., extern "C" function)
    CannotUnwind,
    /// Formatting error (format!, write!, Display/Debug impl panic)
    FormattingError,
    /// Capacity overflow (collection too large)
    CapacityOverflow,
    /// Out of memory (allocation failed)
    OutOfMemory,
    /// String/slice encoding or bounds error
    StringSliceError,
    /// Invalid enum discriminant (memory corruption or unsafe code)
    InvalidEnum,
    /// Misaligned pointer dereference
    MisalignedPointer,
    /// Unknown cause
    Unknown,
}

impl PanicCause {
    /// Get the configuration identifier for this panic cause.
    /// Used in allow/deny configuration files.
    ///
    /// More specific IDs are available for division/remainder operations:
    /// - `div_overflow` / `rem_overflow` for ArithmeticOverflow division/remainder
    /// - `shift_overflow` for ShiftOverflow
    ///
    /// The generic `overflow` ID matches all arithmetic overflow types.
    pub fn id(&self) -> &'static str {
        match self {
            PanicCause::ExplicitPanic => "panic",
            PanicCause::BoundsCheck => "bounds",
            // Use specific IDs for division/remainder to allow targeted suppression
            PanicCause::ArithmeticOverflow(op) => match op.as_str() {
                "division" => "div_overflow",
                "remainder" => "rem_overflow",
                _ => "overflow",
            },
            PanicCause::ShiftOverflow(_) => "shift_overflow",
            PanicCause::DivisionByZero => "div_zero",
            PanicCause::UnwrapNone => "unwrap",
            PanicCause::UnwrapErr => "unwrap",
            PanicCause::ExpectNone => "expect",
            PanicCause::ExpectErr => "expect",
            PanicCause::AssertFailed => "assert",
            PanicCause::Unreachable => "unreachable",
            PanicCause::Unimplemented => "unimplemented",
            PanicCause::Todo => "todo",
            PanicCause::PanicInDrop => "drop",
            PanicCause::CannotUnwind => "unwind",
            PanicCause::FormattingError => "format",
            PanicCause::CapacityOverflow => "capacity",
            PanicCause::OutOfMemory => "oom",
            PanicCause::StringSliceError => "str_slice",
            PanicCause::InvalidEnum => "invalid_enum",
            PanicCause::MisalignedPointer => "misaligned_ptr",
            PanicCause::Unknown => "unknown",
        }
    }

    /// Get the parent/generic configuration identifier, if any.
    /// This allows "overflow" to match specific types like "div_overflow",
    /// and "assert" to match specific overflow types.
    pub fn parent_id(&self) -> Option<&'static str> {
        match self {
            PanicCause::ArithmeticOverflow(_) => Some("overflow"),
            PanicCause::ShiftOverflow(_) => Some("overflow"),
            _ => None,
        }
    }

    /// Get all valid configuration identifiers
    pub fn all_ids() -> &'static [&'static str] {
        &[
            "panic",
            "bounds",
            "overflow",       // matches all arithmetic overflow
            "div_overflow",   // division overflow specifically
            "rem_overflow",   // remainder overflow specifically
            "shift_overflow", // shift overflow specifically
            "div_zero",
            "unwrap",
            "expect",
            "assert",
            "unreachable",
            "unimplemented",
            "todo",
            "drop",
            "unwind",
            "format",
            "capacity",
            "oom",
            "str_slice",
            "invalid_enum",
            "misaligned_ptr",
            "unknown",
        ]
    }

    /// Get a short description of the panic cause
    pub fn description(&self) -> &'static str {
        match self {
            PanicCause::ExplicitPanic => "explicit panic!() call",
            PanicCause::BoundsCheck => "index out of bounds",
            PanicCause::ArithmeticOverflow(_) => "arithmetic overflow",
            PanicCause::ShiftOverflow(_) => "shift overflow",
            PanicCause::DivisionByZero => "division by zero",
            PanicCause::UnwrapNone => "unwrap() on None",
            PanicCause::UnwrapErr => "unwrap() on Err",
            PanicCause::ExpectNone => "expect() on None",
            PanicCause::ExpectErr => "expect() on Err",
            PanicCause::AssertFailed => "assertion failed",
            PanicCause::Unreachable => "unreachable!() reached",
            PanicCause::Unimplemented => "unimplemented!() reached",
            PanicCause::Todo => "todo!() reached",
            PanicCause::PanicInDrop => "panic during drop",
            PanicCause::CannotUnwind => "panic in no-unwind context",
            PanicCause::FormattingError => "formatting error",
            PanicCause::CapacityOverflow => "capacity overflow",
            PanicCause::OutOfMemory => "out of memory",
            PanicCause::StringSliceError => "string/slice error",
            PanicCause::InvalidEnum => "invalid enum discriminant",
            PanicCause::MisalignedPointer => "misaligned pointer dereference",
            PanicCause::Unknown => "unknown cause",
        }
    }

    /// Get a suggestion for how to avoid this panic.
    ///
    /// # Arguments
    /// * `is_direct` - Whether the panic is direct (user code calls unwrap/expect directly)
    ///   or indirect (user code calls a function that eventually panics internally).
    ///
    /// For indirect panics, suggestions recommend using fallible alternatives when available,
    /// since the user cannot simply replace the call with `if let` or `match`.
    pub fn suggestion(&self, is_direct: bool) -> &'static str {
        if is_direct {
            self.direct_suggestion()
        } else {
            self.indirect_suggestion()
        }
    }

    /// Suggestion for direct panics (user code directly calls panic-triggering function)
    fn direct_suggestion(&self) -> &'static str {
        match self {
            PanicCause::ExplicitPanic => "Review if panic is intentional or add error handling",
            PanicCause::BoundsCheck => "Use .get() for safe access or validate index before use",
            PanicCause::ArithmeticOverflow(_) => {
                "Use checked_*, saturating_*, or wrapping_* methods"
            }
            PanicCause::ShiftOverflow(_) => "Validate shift amount is within valid range",
            PanicCause::DivisionByZero => "Check divisor is non-zero before division",
            PanicCause::UnwrapNone => "Use if let, match, unwrap_or, or ? operator instead",
            PanicCause::UnwrapErr => "Use if let, match, unwrap_or, or ? operator instead",
            PanicCause::ExpectNone => "Use if let, match, unwrap_or, or ? operator instead",
            PanicCause::ExpectErr => "Use if let, match, unwrap_or, or ? operator instead",
            PanicCause::AssertFailed => "Review assertion condition",
            PanicCause::Unreachable => "Ensure code path is truly unreachable",
            PanicCause::Unimplemented => "Implement the missing functionality",
            PanicCause::Todo => "Complete the TODO implementation",
            PanicCause::PanicInDrop => {
                "Avoid panicking in Drop implementations; use catch_unwind or log errors instead"
            }
            PanicCause::CannotUnwind => {
                "Avoid panicking in extern functions; use catch_unwind at FFI boundaries"
            }
            PanicCause::FormattingError => {
                "Ensure Display/Debug impls don't panic; validate format arguments"
            }
            PanicCause::CapacityOverflow => {
                "Check collection size before allocation; use try_reserve for fallible allocation"
            }
            PanicCause::OutOfMemory => {
                "Handle allocation failures; consider system memory limits or fallible allocation"
            }
            PanicCause::StringSliceError => {
                "Use str::get() for safe slicing; validate UTF-8 boundaries"
            }
            PanicCause::InvalidEnum => {
                "Check for memory corruption or unsafe enum transmutes; validate enum discriminants"
            }
            PanicCause::MisalignedPointer => {
                "Ensure pointer alignment requirements are met; review unsafe pointer casts"
            }
            PanicCause::Unknown => {
                "Jonesy detected a panic path but couldn't identify the cause. Use --tree to investigate"
            }
        }
    }

    /// Suggestion for indirect panics (user code calls a function that may panic internally)
    fn indirect_suggestion(&self) -> &'static str {
        match self {
            PanicCause::ExplicitPanic => {
                "This calls a function that may panic. Review the called function or handle errors"
            }
            PanicCause::BoundsCheck => {
                "This calls a function that may panic on bounds check. Validate inputs or use a fallible alternative"
            }
            PanicCause::ArithmeticOverflow(_) => {
                "This calls a function that may overflow. Validate inputs or use checked arithmetic"
            }
            PanicCause::ShiftOverflow(_) => {
                "This calls a function that may overflow on shift. Validate inputs"
            }
            PanicCause::DivisionByZero => {
                "This calls a function that may divide by zero. Validate inputs"
            }
            PanicCause::UnwrapNone | PanicCause::UnwrapErr => {
                "This calls a function that may call unwrap(). Consider a fallible alternative (e.g., try_*)"
            }
            PanicCause::ExpectNone | PanicCause::ExpectErr => {
                "This calls a function that may call expect(). Consider a fallible alternative (e.g., try_*)"
            }
            PanicCause::AssertFailed => {
                "This calls a function with an assertion. Review preconditions"
            }
            PanicCause::Unreachable => {
                "This calls a function that may reach unreachable code. Review control flow"
            }
            PanicCause::Unimplemented => "This calls a function with unimplemented!() code paths",
            PanicCause::Todo => "This calls a function with todo!() code paths",
            PanicCause::PanicInDrop => "This calls a function that may panic during drop",
            PanicCause::CannotUnwind => {
                "This calls a function that may panic in a no-unwind context"
            }
            PanicCause::FormattingError => "This calls a function that may panic during formatting",
            PanicCause::CapacityOverflow => {
                "This calls a function that may overflow capacity. Consider fallible allocation (try_reserve)"
            }
            PanicCause::OutOfMemory => "This calls a function that may fail to allocate memory",
            PanicCause::StringSliceError => {
                "This calls a function that may fail on string/slice operations"
            }
            PanicCause::InvalidEnum => {
                "This calls a function that may encounter an invalid enum discriminant"
            }
            PanicCause::MisalignedPointer => {
                "This calls a function that may dereference a misaligned pointer"
            }
            PanicCause::Unknown => {
                "This calls a function that may panic. Use --tree to investigate the call chain"
            }
        }
    }

    /// Format suggestion with the called function name for indirect panics.
    /// Returns a dynamically formatted string that includes the function name when available.
    pub fn format_suggestion(&self, is_direct: bool, called_function: Option<&str>) -> String {
        if is_direct || called_function.is_none() {
            // Direct panic or no function name - use static suggestion
            return self.suggestion(is_direct).to_string();
        }

        let func = called_function.unwrap();
        match self {
            PanicCause::ExplicitPanic => {
                format!("This calls `{func}` which may panic. Review `{func}` or handle errors")
            }
            PanicCause::BoundsCheck => {
                format!(
                    "This calls `{func}` which may panic on bounds check. Validate inputs or use a fallible alternative"
                )
            }
            PanicCause::ArithmeticOverflow(_) => {
                format!(
                    "This calls `{func}` which may overflow. Validate inputs or use checked arithmetic"
                )
            }
            PanicCause::ShiftOverflow(_) => {
                format!("This calls `{func}` which may overflow on shift. Validate inputs")
            }
            PanicCause::DivisionByZero => {
                format!("This calls `{func}` which may divide by zero. Validate inputs")
            }
            PanicCause::UnwrapNone | PanicCause::UnwrapErr => {
                format!(
                    "This calls `{func}` which may call unwrap(). Consider a fallible alternative (e.g., try_{func})"
                )
            }
            PanicCause::ExpectNone | PanicCause::ExpectErr => {
                format!(
                    "This calls `{func}` which may call expect(). Consider a fallible alternative (e.g., try_{func})"
                )
            }
            PanicCause::AssertFailed => {
                format!("This calls `{func}` which has an assertion. Review preconditions")
            }
            PanicCause::Unreachable => {
                format!("This calls `{func}` which may reach unreachable code. Review control flow")
            }
            PanicCause::Unimplemented => {
                format!("This calls `{func}` which has unimplemented!() code paths")
            }
            PanicCause::Todo => format!("This calls `{func}` which has todo!() code paths"),
            PanicCause::PanicInDrop => {
                format!("This calls `{func}` which may panic during drop")
            }
            PanicCause::CannotUnwind => {
                format!("This calls `{func}` which may panic in a no-unwind context")
            }
            PanicCause::FormattingError => {
                format!("This calls `{func}` which may panic during formatting")
            }
            PanicCause::CapacityOverflow => {
                format!(
                    "This calls `{func}` which may overflow capacity. Consider fallible allocation (try_reserve)"
                )
            }
            PanicCause::OutOfMemory => {
                format!(
                    "This calls `{func}` which may fail on allocation. Consider fallible allocation"
                )
            }
            PanicCause::StringSliceError => {
                format!("This calls `{func}` which may fail on string/slice operations")
            }
            PanicCause::InvalidEnum => {
                format!("This calls `{func}` which may encounter an invalid enum discriminant")
            }
            PanicCause::MisalignedPointer => {
                format!("This calls `{func}` which may dereference a misaligned pointer")
            }
            PanicCause::Unknown => {
                format!("This calls `{func}` which may panic. Use --tree to investigate")
            }
        }
    }

    /// Returns the unique error code for this panic type (e.g., "JP001").
    /// These codes correspond to documentation pages at the jonesy docs site.
    pub fn error_code(&self) -> &'static str {
        match self {
            PanicCause::ExplicitPanic => "JP001",
            PanicCause::BoundsCheck => "JP002",
            PanicCause::ArithmeticOverflow(_) => "JP003",
            PanicCause::ShiftOverflow(_) => "JP004",
            PanicCause::DivisionByZero => "JP005",
            PanicCause::UnwrapNone => "JP006",
            PanicCause::UnwrapErr => "JP007",
            PanicCause::ExpectNone => "JP008",
            PanicCause::ExpectErr => "JP009",
            PanicCause::AssertFailed => "JP010",
            PanicCause::Unreachable => "JP012",
            PanicCause::Unimplemented => "JP013",
            PanicCause::Todo => "JP014",
            PanicCause::PanicInDrop => "JP015",
            PanicCause::CannotUnwind => "JP016",
            PanicCause::FormattingError => "JP017",
            PanicCause::CapacityOverflow => "JP018",
            PanicCause::OutOfMemory => "JP019",
            PanicCause::StringSliceError => "JP020",
            PanicCause::InvalidEnum => "JP021",
            PanicCause::MisalignedPointer => "JP022",
            PanicCause::Unknown => "JP000",
        }
    }

    /// Returns the documentation URL slug for this panic type.
    /// The full URL is `https://jonesy.mackenzie-serres.net/panics/{slug}`
    pub fn docs_slug(&self) -> &'static str {
        match self {
            PanicCause::ExplicitPanic => "JP001-explicit-panic",
            PanicCause::BoundsCheck => "JP002-bounds-check",
            PanicCause::ArithmeticOverflow(_) => "JP003-arithmetic-overflow",
            PanicCause::ShiftOverflow(_) => "JP004-shift-overflow",
            PanicCause::DivisionByZero => "JP005-division-by-zero",
            PanicCause::UnwrapNone => "JP006-unwrap-none",
            PanicCause::UnwrapErr => "JP007-unwrap-err",
            PanicCause::ExpectNone => "JP008-expect-none",
            PanicCause::ExpectErr => "JP009-expect-err",
            PanicCause::AssertFailed => "JP010-assert-failed",
            PanicCause::Unreachable => "JP012-unreachable",
            PanicCause::Unimplemented => "JP013-unimplemented",
            PanicCause::Todo => "JP014-todo",
            PanicCause::PanicInDrop => "JP015-panic-in-drop",
            PanicCause::CannotUnwind => "JP016-cannot-unwind",
            PanicCause::FormattingError => "JP017-formatting-error",
            PanicCause::CapacityOverflow => "JP018-capacity-overflow",
            PanicCause::OutOfMemory => "JP019-out-of-memory",
            PanicCause::StringSliceError => "JP020-string-slice-error",
            PanicCause::InvalidEnum => "JP021-invalid-enum",
            PanicCause::MisalignedPointer => "JP022-misaligned-pointer",
            PanicCause::Unknown => "",
        }
    }

    /// Returns the full documentation URL for this panic type.
    pub fn docs_url(&self) -> String {
        const BASE_URL: &str = "https://jonesy.mackenzie-serres.net/panics";
        let slug = self.docs_slug();
        if slug.is_empty() {
            format!("{}/", BASE_URL)
        } else {
            format!("{}/{}", BASE_URL, slug)
        }
    }

    /// Returns true if this panic cause only occurs in debug builds (by default).
    /// In release builds, these conditions have different behavior (wrapping or omitted).
    ///
    /// # Profile-Dependent Behavior
    ///
    /// These causes have different behavior based on Cargo profile settings:
    /// - **Arithmetic/shift overflow**: Controlled by `overflow-checks`
    ///   - `true` (dev default): panics on overflow
    ///   - `false` (release default): wraps silently
    ///
    /// Division by zero and bounds checking panic in BOTH debug and release builds.
    /// Safe Rust never has undefined behavior.
    ///
    /// # References
    /// - Cargo profiles: <https://doc.rust-lang.org/cargo/reference/profiles.html>
    /// - Undefined behavior: <https://doc.rust-lang.org/reference/behavior-considered-undefined.html>
    #[allow(dead_code)] // May be useful for future filtering features
    pub fn is_debug_only(&self) -> bool {
        matches!(
            self,
            PanicCause::ArithmeticOverflow(_) | PanicCause::ShiftOverflow(_)
        )
    }

    /// Get a warning message for profile-dependent panics.
    /// Returns None if this panic occurs in both debug and release builds.
    ///
    /// # Profile-Dependent Behavior
    ///
    /// These causes have different behavior based on Cargo profile settings:
    /// - **Arithmetic/shift overflow**: Controlled by `overflow-checks` in Cargo profiles.
    ///   Panics when enabled (dev default), wraps when disabled (release default).
    ///
    /// The following panic in BOTH debug and release builds regardless of settings:
    /// - **Division by zero**: Always panics (safe Rust has no UB)
    /// - **Index out of bounds**: Always panics (bounds checks are never removed)
    ///
    /// # References
    /// - Cargo profiles: <https://doc.rust-lang.org/cargo/reference/profiles.html>
    /// - Undefined behavior: <https://doc.rust-lang.org/reference/behavior-considered-undefined.html>
    pub fn release_warning(&self) -> Option<&'static str> {
        match self {
            PanicCause::ArithmeticOverflow(_) | PanicCause::ShiftOverflow(_) => {
                Some("With default release settings (overflow-checks=false), this wraps silently")
            }
            _ => None,
        }
    }
}

/// Detect panic cause from a function name in the call chain
/// The optional file path helps distinguish Option vs Result for unwrap/expect
pub fn detect_panic_cause(func_name: &str, file_path: Option<&str>) -> Option<PanicCause> {
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
    // unwrap/expect detection - distinguish Option vs Result by file path or function name
    // Check for Result::expect first (it calls unwrap_failed internally)
    if func_name.contains("Result") && func_name.contains("expect") {
        return Some(PanicCause::ExpectErr);
    }
    if func_name.contains("unwrap_failed") {
        // Check file path first (most reliable), then fall back to function name
        // Note: file_path may be the crate source file (not stdlib) when called from
        // rlib analysis, so always check func_name as fallback
        let is_result = file_path
            .filter(|f| {
                f.contains("result.rs")
                    || f.contains("core/result")
                    || f.contains("option.rs")
                    || f.contains("core/option")
            })
            .map(|f| f.contains("result.rs") || f.contains("core/result"))
            .unwrap_or_else(|| func_name.contains("result"));
        if is_result {
            // core::result::unwrap_failed - used by Result::unwrap()
            // (Result::expect is detected above via the caller)
            return Some(PanicCause::UnwrapErr);
        } else {
            // core::option::unwrap_failed
            return Some(PanicCause::UnwrapNone);
        }
    }
    if func_name.contains("expect_failed") {
        // Only Option has expect_failed; Result::expect() uses unwrap_failed
        return Some(PanicCause::ExpectNone);
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
    // Function names like "index<T, usize>" or "Index::index"
    // Note: function name might be just "index<...>" without module prefix
    if func_name.starts_with("index<")
        || func_name.contains("::index<")
        || func_name.contains("Index::index")
    {
        // Check if it's for str (string slice) vs array/vec (bounds check)
        // String slicing can be detected via:
        // 1. Function name containing str:: or core::str::
        // 2. File path matching known stdlib string module paths
        let is_string_op = func_name.contains("str::") || func_name.contains("core::str::");
        let is_string_file = file_path
            .map(|f| {
                // Normalize path separators for cross-platform matching
                let normalized = f.replace('\\', "/");
                // Only match known stdlib string module paths to avoid false positives
                // from user directories named "str"
                normalized.contains("/library/core/src/str/")
                    || normalized.contains("/library/std/src/str/")
                    || normalized.contains("/src/libcore/str/")
            })
            .unwrap_or(false);
        if is_string_op || is_string_file {
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

    // panic_fmt is the core panic function - if we reach here without a more
    // specific match, leave cause as None (unknown) to avoid incorrect labeling.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panic_cause_id() {
        assert_eq!(PanicCause::ExplicitPanic.id(), "panic");
        assert_eq!(PanicCause::BoundsCheck.id(), "bounds");
        assert_eq!(PanicCause::UnwrapNone.id(), "unwrap");
        assert_eq!(PanicCause::UnwrapErr.id(), "unwrap");
        assert_eq!(PanicCause::ExpectNone.id(), "expect");
        assert_eq!(PanicCause::ExpectErr.id(), "expect");
        assert_eq!(PanicCause::DivisionByZero.id(), "div_zero");
        assert_eq!(
            PanicCause::ArithmeticOverflow("addition".to_string()).id(),
            "overflow"
        );
        assert_eq!(
            PanicCause::ArithmeticOverflow("division".to_string()).id(),
            "div_overflow"
        );
        assert_eq!(
            PanicCause::ArithmeticOverflow("remainder".to_string()).id(),
            "rem_overflow"
        );
        assert_eq!(
            PanicCause::ShiftOverflow("left".to_string()).id(),
            "shift_overflow"
        );
    }

    #[test]
    fn test_panic_cause_parent_id() {
        assert_eq!(
            PanicCause::ArithmeticOverflow("add".to_string()).parent_id(),
            Some("overflow")
        );
        assert_eq!(
            PanicCause::ShiftOverflow("left".to_string()).parent_id(),
            Some("overflow")
        );
        assert_eq!(PanicCause::AssertFailed.parent_id(), None);
        assert_eq!(PanicCause::ExplicitPanic.parent_id(), None);
        assert_eq!(PanicCause::BoundsCheck.parent_id(), None);
    }

    #[test]
    fn test_panic_cause_description() {
        assert_eq!(
            PanicCause::ExplicitPanic.description(),
            "explicit panic!() call"
        );
        assert_eq!(PanicCause::BoundsCheck.description(), "index out of bounds");
        assert_eq!(PanicCause::UnwrapNone.description(), "unwrap() on None");
        assert_eq!(PanicCause::UnwrapErr.description(), "unwrap() on Err");
        assert_eq!(PanicCause::Todo.description(), "todo!() reached");
        assert_eq!(
            PanicCause::Unreachable.description(),
            "unreachable!() reached"
        );
    }

    #[test]
    fn test_panic_cause_error_code() {
        assert_eq!(PanicCause::ExplicitPanic.error_code(), "JP001");
        assert_eq!(PanicCause::BoundsCheck.error_code(), "JP002");
        assert_eq!(
            PanicCause::ArithmeticOverflow("add".to_string()).error_code(),
            "JP003"
        );
        assert_eq!(PanicCause::UnwrapNone.error_code(), "JP006");
        assert_eq!(PanicCause::UnwrapErr.error_code(), "JP007");
        assert_eq!(PanicCause::Unknown.error_code(), "JP000");
    }

    #[test]
    fn test_panic_cause_docs_slug() {
        assert_eq!(
            PanicCause::ExplicitPanic.docs_slug(),
            "JP001-explicit-panic"
        );
        assert_eq!(PanicCause::BoundsCheck.docs_slug(), "JP002-bounds-check");
        assert_eq!(PanicCause::UnwrapNone.docs_slug(), "JP006-unwrap-none");
        assert_eq!(PanicCause::Unknown.docs_slug(), "");
    }

    #[test]
    fn test_panic_cause_docs_url() {
        assert_eq!(
            PanicCause::ExplicitPanic.docs_url(),
            "https://jonesy.mackenzie-serres.net/panics/JP001-explicit-panic"
        );
        assert_eq!(
            PanicCause::Unknown.docs_url(),
            "https://jonesy.mackenzie-serres.net/panics/"
        );
    }

    #[test]
    fn test_panic_cause_is_debug_only() {
        assert!(PanicCause::ArithmeticOverflow("add".to_string()).is_debug_only());
        assert!(PanicCause::ShiftOverflow("left".to_string()).is_debug_only());
        assert!(!PanicCause::AssertFailed.is_debug_only());
        assert!(!PanicCause::BoundsCheck.is_debug_only());
        assert!(!PanicCause::DivisionByZero.is_debug_only());
        assert!(!PanicCause::UnwrapNone.is_debug_only());
    }

    #[test]
    fn test_panic_cause_release_warning() {
        assert!(
            PanicCause::ArithmeticOverflow("add".to_string())
                .release_warning()
                .is_some()
        );
        assert!(
            PanicCause::ShiftOverflow("left".to_string())
                .release_warning()
                .is_some()
        );
        assert!(PanicCause::AssertFailed.release_warning().is_none());
        assert!(PanicCause::BoundsCheck.release_warning().is_none());
        assert!(PanicCause::UnwrapNone.release_warning().is_none());
    }

    #[test]
    fn test_panic_cause_suggestion_direct() {
        let suggestion = PanicCause::UnwrapNone.suggestion(true);
        assert!(suggestion.contains("if let") || suggestion.contains("match"));
    }

    #[test]
    fn test_panic_cause_suggestion_indirect() {
        let suggestion = PanicCause::UnwrapNone.suggestion(false);
        assert!(suggestion.contains("calls a function"));
    }

    #[test]
    fn test_panic_cause_format_suggestion_with_function() {
        let suggestion = PanicCause::UnwrapNone.format_suggestion(false, Some("parse_config"));
        assert!(suggestion.contains("parse_config"));
    }

    #[test]
    fn test_panic_cause_format_suggestion_direct() {
        let suggestion = PanicCause::UnwrapNone.format_suggestion(true, Some("ignored"));
        // Direct suggestions don't include function name
        assert!(!suggestion.contains("ignored"));
    }

    #[test]
    fn test_all_ids_contains_expected() {
        let ids = PanicCause::all_ids();
        assert!(ids.contains(&"panic"));
        assert!(ids.contains(&"bounds"));
        assert!(ids.contains(&"overflow"));
        assert!(ids.contains(&"div_overflow"));
        assert!(ids.contains(&"rem_overflow"));
        assert!(ids.contains(&"unwrap"));
        assert!(ids.contains(&"expect"));
    }

    #[test]
    fn test_detect_panic_cause_bounds_check() {
        assert_eq!(
            detect_panic_cause("panic_bounds_check", None),
            Some(PanicCause::BoundsCheck)
        );
    }

    #[test]
    fn test_detect_panic_cause_arithmetic_overflow() {
        assert_eq!(
            detect_panic_cause("panic_const_add_overflow", None),
            Some(PanicCause::ArithmeticOverflow("addition".to_string()))
        );
        assert_eq!(
            detect_panic_cause("panic_const_sub_overflow", None),
            Some(PanicCause::ArithmeticOverflow("subtraction".to_string()))
        );
        assert_eq!(
            detect_panic_cause("panic_const_mul_overflow", None),
            Some(PanicCause::ArithmeticOverflow("multiplication".to_string()))
        );
    }

    #[test]
    fn test_detect_panic_cause_shift_overflow() {
        assert_eq!(
            detect_panic_cause("panic_const_shl_overflow", None),
            Some(PanicCause::ShiftOverflow("left".to_string()))
        );
        assert_eq!(
            detect_panic_cause("panic_const_shr_overflow", None),
            Some(PanicCause::ShiftOverflow("right".to_string()))
        );
    }

    #[test]
    fn test_detect_panic_cause_division_by_zero() {
        assert_eq!(
            detect_panic_cause("panic_const_div_by_zero", None),
            Some(PanicCause::DivisionByZero)
        );
        assert_eq!(
            detect_panic_cause("panic_const_rem_by_zero", None),
            Some(PanicCause::DivisionByZero)
        );
    }

    #[test]
    fn test_detect_panic_cause_unwrap_failed_option() {
        // Option unwrap - detected via file path
        assert_eq!(
            detect_panic_cause("unwrap_failed", Some("option.rs")),
            Some(PanicCause::UnwrapNone)
        );
    }

    #[test]
    fn test_detect_panic_cause_unwrap_failed_result() {
        // Result unwrap - detected via file path
        assert_eq!(
            detect_panic_cause("unwrap_failed", Some("result.rs")),
            Some(PanicCause::UnwrapErr)
        );
        assert_eq!(
            detect_panic_cause("unwrap_failed", Some("core/result/mod.rs")),
            Some(PanicCause::UnwrapErr)
        );
    }

    #[test]
    fn test_detect_panic_cause_expect_failed() {
        assert_eq!(
            detect_panic_cause("expect_failed", None),
            Some(PanicCause::ExpectNone)
        );
    }

    #[test]
    fn test_detect_panic_cause_result_expect() {
        assert_eq!(
            detect_panic_cause("Result::expect", None),
            Some(PanicCause::ExpectErr)
        );
    }

    #[test]
    fn test_detect_panic_cause_assert_failed() {
        // Assert from user code (no file path) -> AssertFailed
        assert_eq!(
            detect_panic_cause("assert_failed", None),
            Some(PanicCause::AssertFailed)
        );
        // Assert from user code (non-stdlib path) -> AssertFailed
        assert_eq!(
            detect_panic_cause("assert_failed", Some("src/main.rs")),
            Some(PanicCause::AssertFailed)
        );
    }

    #[test]
    fn test_assert_failed_always_assert_regardless_of_path() {
        // assert_failed is always classified as AssertFailed regardless of source path,
        // since we cannot distinguish assert!() from debug_assert!() at the binary level
        assert_eq!(
            detect_panic_cause(
                "assert_failed",
                Some("/rustc/abc123/library/std/src/time.rs")
            ),
            Some(PanicCause::AssertFailed)
        );
        assert_eq!(
            detect_panic_cause("assert_failed", Some("/library/core/src/num/mod.rs")),
            Some(PanicCause::AssertFailed)
        );
        assert_eq!(
            detect_panic_cause("assert_failed", Some("src/main.rs")),
            Some(PanicCause::AssertFailed)
        );
    }

    #[test]
    fn test_assert_in_user_path_with_library() {
        assert_eq!(
            detect_panic_cause("assert_failed", Some("/home/me/library/app/src/main.rs")),
            Some(PanicCause::AssertFailed)
        );
        assert_eq!(
            detect_panic_cause("assert_failed", Some("/projects/library/core/lib.rs")),
            Some(PanicCause::AssertFailed)
        );
    }

    #[test]
    fn test_detect_panic_cause_panic_display() {
        assert_eq!(
            detect_panic_cause("panic_display", None),
            Some(PanicCause::ExplicitPanic)
        );
    }

    #[test]
    fn test_detect_panic_cause_panic_in_cleanup() {
        assert_eq!(
            detect_panic_cause("panic_in_cleanup", None),
            Some(PanicCause::PanicInDrop)
        );
    }

    #[test]
    fn test_detect_panic_cause_panic_cannot_unwind() {
        assert_eq!(
            detect_panic_cause("panic_cannot_unwind", None),
            Some(PanicCause::CannotUnwind)
        );
        assert_eq!(
            detect_panic_cause("panic_nounwind", None),
            Some(PanicCause::CannotUnwind)
        );
    }

    #[test]
    fn test_detect_panic_cause_formatting() {
        assert_eq!(
            detect_panic_cause("core::fmt::write", None),
            Some(PanicCause::FormattingError)
        );
        assert_eq!(
            detect_panic_cause("write_fmt", None),
            Some(PanicCause::FormattingError)
        );
    }

    #[test]
    fn test_detect_panic_cause_capacity_overflow() {
        assert_eq!(
            detect_panic_cause("capacity_overflow", None),
            Some(PanicCause::CapacityOverflow)
        );
    }

    #[test]
    fn test_detect_panic_cause_out_of_memory() {
        assert_eq!(
            detect_panic_cause("handle_alloc_error", None),
            Some(PanicCause::OutOfMemory)
        );
    }

    #[test]
    fn test_detect_panic_cause_string_slice_error() {
        assert_eq!(
            detect_panic_cause("slice_error_fail", None),
            Some(PanicCause::StringSliceError)
        );
        assert_eq!(
            detect_panic_cause("str_index_overflow_fail", None),
            Some(PanicCause::StringSliceError)
        );
    }

    #[test]
    fn test_detect_panic_cause_index_bounds() {
        assert_eq!(
            detect_panic_cause("index<T, usize>", None),
            Some(PanicCause::BoundsCheck)
        );
        assert_eq!(
            detect_panic_cause("Index::index", None),
            Some(PanicCause::BoundsCheck)
        );
    }

    #[test]
    fn test_detect_panic_cause_index_string() {
        // String index detected via file path
        assert_eq!(
            detect_panic_cause("index<Range>", Some("/library/core/src/str/mod.rs")),
            Some(PanicCause::StringSliceError)
        );
        // Or via str:: in function name
        assert_eq!(
            detect_panic_cause("str::index<Range>", None),
            Some(PanicCause::StringSliceError)
        );
    }

    #[test]
    fn test_detect_panic_cause_invalid_enum() {
        assert_eq!(
            detect_panic_cause("panic_invalid_enum_construction", None),
            Some(PanicCause::InvalidEnum)
        );
    }

    #[test]
    fn test_detect_panic_cause_misaligned_pointer() {
        assert_eq!(
            detect_panic_cause("panic_misaligned_pointer_dereference", None),
            Some(PanicCause::MisalignedPointer)
        );
    }

    #[test]
    fn test_detect_panic_cause_unknown() {
        // Generic panic function should return None (unknown)
        assert_eq!(detect_panic_cause("some_random_function", None), None);
    }

    // Test all descriptions
    #[test]
    fn test_all_panic_cause_descriptions() {
        // Ensure every variant has a non-empty description
        let variants = vec![
            PanicCause::ExplicitPanic,
            PanicCause::BoundsCheck,
            PanicCause::ArithmeticOverflow("add".to_string()),
            PanicCause::ShiftOverflow("left".to_string()),
            PanicCause::DivisionByZero,
            PanicCause::UnwrapNone,
            PanicCause::UnwrapErr,
            PanicCause::ExpectNone,
            PanicCause::ExpectErr,
            PanicCause::AssertFailed,
            PanicCause::Unreachable,
            PanicCause::Unimplemented,
            PanicCause::Todo,
            PanicCause::PanicInDrop,
            PanicCause::CannotUnwind,
            PanicCause::FormattingError,
            PanicCause::CapacityOverflow,
            PanicCause::OutOfMemory,
            PanicCause::StringSliceError,
            PanicCause::InvalidEnum,
            PanicCause::MisalignedPointer,
            PanicCause::Unknown,
        ];

        for variant in &variants {
            assert!(
                !variant.description().is_empty(),
                "{:?} has empty description",
                variant
            );
            assert!(
                !variant.error_code().is_empty(),
                "{:?} has empty error code",
                variant
            );
            assert!(
                !variant.suggestion(true).is_empty(),
                "{:?} has empty direct suggestion",
                variant
            );
            assert!(
                !variant.suggestion(false).is_empty(),
                "{:?} has empty indirect suggestion",
                variant
            );
        }
    }

    // Test all direct suggestions
    #[test]
    fn test_direct_suggestions() {
        assert!(
            PanicCause::ExplicitPanic
                .direct_suggestion()
                .contains("Review")
        );
        assert!(
            PanicCause::BoundsCheck
                .direct_suggestion()
                .contains(".get()")
        );
        assert!(
            PanicCause::ArithmeticOverflow("add".to_string())
                .direct_suggestion()
                .contains("checked_")
        );
        assert!(
            PanicCause::ShiftOverflow("left".to_string())
                .direct_suggestion()
                .contains("Validate")
        );
        assert!(
            PanicCause::DivisionByZero
                .direct_suggestion()
                .contains("divisor")
        );
        assert!(
            PanicCause::AssertFailed
                .direct_suggestion()
                .contains("assertion")
        );
        assert!(
            PanicCause::Unreachable
                .direct_suggestion()
                .contains("unreachable")
        );
        assert!(
            PanicCause::Unimplemented
                .direct_suggestion()
                .contains("Implement")
        );
        assert!(PanicCause::Todo.direct_suggestion().contains("TODO"));
        assert!(PanicCause::PanicInDrop.direct_suggestion().contains("Drop"));
        assert!(
            PanicCause::CannotUnwind
                .direct_suggestion()
                .contains("extern")
        );
        assert!(
            PanicCause::FormattingError
                .direct_suggestion()
                .contains("Display")
        );
        assert!(
            PanicCause::CapacityOverflow
                .direct_suggestion()
                .contains("try_reserve")
        );
        assert!(
            PanicCause::OutOfMemory
                .direct_suggestion()
                .contains("allocation")
        );
        assert!(
            PanicCause::StringSliceError
                .direct_suggestion()
                .contains("str::get()")
        );
        assert!(PanicCause::InvalidEnum.direct_suggestion().contains("enum"));
        assert!(
            PanicCause::MisalignedPointer
                .direct_suggestion()
                .contains("alignment")
        );
        assert!(PanicCause::Unknown.direct_suggestion().contains("--tree"));
    }

    // Test all indirect suggestions
    #[test]
    fn test_indirect_suggestions() {
        assert!(
            PanicCause::ExplicitPanic
                .indirect_suggestion()
                .contains("calls a function")
        );
        assert!(
            PanicCause::BoundsCheck
                .indirect_suggestion()
                .contains("bounds check")
        );
        assert!(
            PanicCause::ArithmeticOverflow("add".to_string())
                .indirect_suggestion()
                .contains("overflow")
        );
        assert!(
            PanicCause::ShiftOverflow("left".to_string())
                .indirect_suggestion()
                .contains("shift")
        );
        assert!(
            PanicCause::DivisionByZero
                .indirect_suggestion()
                .contains("divide by zero")
        );
        assert!(
            PanicCause::UnwrapNone
                .indirect_suggestion()
                .contains("unwrap()")
        );
        assert!(
            PanicCause::UnwrapErr
                .indirect_suggestion()
                .contains("unwrap()")
        );
        assert!(
            PanicCause::ExpectNone
                .indirect_suggestion()
                .contains("expect()")
        );
        assert!(
            PanicCause::ExpectErr
                .indirect_suggestion()
                .contains("expect()")
        );
        assert!(
            PanicCause::AssertFailed
                .indirect_suggestion()
                .contains("assertion")
        );
        assert!(
            PanicCause::Unreachable
                .indirect_suggestion()
                .contains("unreachable")
        );
        assert!(
            PanicCause::Unimplemented
                .indirect_suggestion()
                .contains("unimplemented!()")
        );
        assert!(PanicCause::Todo.indirect_suggestion().contains("todo!()"));
        assert!(
            PanicCause::PanicInDrop
                .indirect_suggestion()
                .contains("drop")
        );
        assert!(
            PanicCause::CannotUnwind
                .indirect_suggestion()
                .contains("no-unwind")
        );
        assert!(
            PanicCause::FormattingError
                .indirect_suggestion()
                .contains("formatting")
        );
        assert!(
            PanicCause::CapacityOverflow
                .indirect_suggestion()
                .contains("capacity")
        );
        assert!(
            PanicCause::OutOfMemory
                .indirect_suggestion()
                .contains("allocate")
        );
        assert!(
            PanicCause::StringSliceError
                .indirect_suggestion()
                .contains("string/slice")
        );
        assert!(
            PanicCause::InvalidEnum
                .indirect_suggestion()
                .contains("enum")
        );
        assert!(
            PanicCause::MisalignedPointer
                .indirect_suggestion()
                .contains("misaligned")
        );
        assert!(PanicCause::Unknown.indirect_suggestion().contains("--tree"));
    }

    // Test format_suggestion for all variants
    #[test]
    fn test_format_suggestion_all_variants() {
        let variants = vec![
            PanicCause::ExplicitPanic,
            PanicCause::BoundsCheck,
            PanicCause::ArithmeticOverflow("add".to_string()),
            PanicCause::ShiftOverflow("left".to_string()),
            PanicCause::DivisionByZero,
            PanicCause::UnwrapNone,
            PanicCause::UnwrapErr,
            PanicCause::ExpectNone,
            PanicCause::ExpectErr,
            PanicCause::AssertFailed,
            PanicCause::Unreachable,
            PanicCause::Unimplemented,
            PanicCause::Todo,
            PanicCause::PanicInDrop,
            PanicCause::CannotUnwind,
            PanicCause::FormattingError,
            PanicCause::CapacityOverflow,
            PanicCause::OutOfMemory,
            PanicCause::StringSliceError,
            PanicCause::InvalidEnum,
            PanicCause::MisalignedPointer,
            PanicCause::Unknown,
        ];

        for variant in &variants {
            // Test with function name (indirect)
            let with_func = variant.format_suggestion(false, Some("test_func"));
            assert!(
                with_func.contains("test_func"),
                "{:?} format_suggestion doesn't include function name: {}",
                variant,
                with_func
            );
        }
    }

    // Test all docs slugs
    #[test]
    fn test_all_docs_slugs() {
        assert_eq!(PanicCause::BoundsCheck.docs_slug(), "JP002-bounds-check");
        assert_eq!(
            PanicCause::ArithmeticOverflow("add".to_string()).docs_slug(),
            "JP003-arithmetic-overflow"
        );
        assert_eq!(
            PanicCause::ShiftOverflow("left".to_string()).docs_slug(),
            "JP004-shift-overflow"
        );
        assert_eq!(
            PanicCause::DivisionByZero.docs_slug(),
            "JP005-division-by-zero"
        );
        assert_eq!(PanicCause::ExpectNone.docs_slug(), "JP008-expect-none");
        assert_eq!(PanicCause::ExpectErr.docs_slug(), "JP009-expect-err");
        assert_eq!(PanicCause::AssertFailed.docs_slug(), "JP010-assert-failed");
        assert_eq!(PanicCause::Unreachable.docs_slug(), "JP012-unreachable");
        assert_eq!(PanicCause::Unimplemented.docs_slug(), "JP013-unimplemented");
        assert_eq!(PanicCause::Todo.docs_slug(), "JP014-todo");
        assert_eq!(PanicCause::PanicInDrop.docs_slug(), "JP015-panic-in-drop");
        assert_eq!(PanicCause::CannotUnwind.docs_slug(), "JP016-cannot-unwind");
        assert_eq!(
            PanicCause::FormattingError.docs_slug(),
            "JP017-formatting-error"
        );
        assert_eq!(
            PanicCause::CapacityOverflow.docs_slug(),
            "JP018-capacity-overflow"
        );
        assert_eq!(PanicCause::OutOfMemory.docs_slug(), "JP019-out-of-memory");
        assert_eq!(
            PanicCause::StringSliceError.docs_slug(),
            "JP020-string-slice-error"
        );
        assert_eq!(PanicCause::InvalidEnum.docs_slug(), "JP021-invalid-enum");
        assert_eq!(
            PanicCause::MisalignedPointer.docs_slug(),
            "JP022-misaligned-pointer"
        );
    }

    // Test unreachable detection
    #[test]
    fn test_detect_panic_cause_unreachable() {
        assert_eq!(
            detect_panic_cause("unreachable_panic_handler", None),
            Some(PanicCause::Unreachable)
        );
    }

    // Test raw_vec capacity detection
    #[test]
    fn test_detect_panic_cause_raw_vec_grow() {
        assert_eq!(
            detect_panic_cause("raw_vec::grow", None),
            Some(PanicCause::CapacityOverflow)
        );
    }

    // Test Display/Debug trait formatting
    #[test]
    fn test_detect_panic_cause_display_fmt() {
        assert_eq!(
            detect_panic_cause("Display::fmt", None),
            Some(PanicCause::FormattingError)
        );
        assert_eq!(
            detect_panic_cause("Debug::fmt", None),
            Some(PanicCause::FormattingError)
        );
    }
}
