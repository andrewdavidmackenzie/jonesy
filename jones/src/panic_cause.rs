//! Panic cause detection and explanations.
//!
//! This module identifies the source of potential panics by analyzing
//! function names in the call chain and provides helpful suggestions.

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
    /// Assert failed
    AssertFailed,
    /// Debug assert failed
    DebugAssertFailed,
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
    /// Unknown cause
    Unknown,
}

impl PanicCause {
    /// Get the configuration identifier for this panic cause.
    /// Used in allow/deny configuration files.
    pub fn id(&self) -> &'static str {
        match self {
            PanicCause::ExplicitPanic => "panic",
            PanicCause::BoundsCheck => "bounds",
            PanicCause::ArithmeticOverflow(_) => "overflow",
            PanicCause::ShiftOverflow(_) => "overflow",
            PanicCause::DivisionByZero => "div_zero",
            PanicCause::UnwrapNone => "unwrap",
            PanicCause::UnwrapErr => "unwrap",
            PanicCause::ExpectNone => "expect",
            PanicCause::ExpectErr => "expect",
            PanicCause::AssertFailed => "assert",
            PanicCause::DebugAssertFailed => "debug_assert",
            PanicCause::Unreachable => "unreachable",
            PanicCause::Unimplemented => "unimplemented",
            PanicCause::Todo => "todo",
            PanicCause::PanicInDrop => "drop",
            PanicCause::CannotUnwind => "unwind",
            PanicCause::Unknown => "unknown",
        }
    }

    /// Get all valid configuration identifiers
    pub fn all_ids() -> &'static [&'static str] {
        &[
            "panic",
            "bounds",
            "overflow",
            "div_zero",
            "unwrap",
            "expect",
            "assert",
            "debug_assert",
            "unreachable",
            "unimplemented",
            "todo",
            "drop",
            "unwind",
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
            PanicCause::DebugAssertFailed => "debug assertion failed",
            PanicCause::Unreachable => "unreachable!() reached",
            PanicCause::Unimplemented => "unimplemented!() reached",
            PanicCause::Todo => "todo!() reached",
            PanicCause::PanicInDrop => "panic during drop",
            PanicCause::CannotUnwind => "panic in no-unwind context",
            PanicCause::Unknown => "unknown cause",
        }
    }

    /// Get a suggestion for how to avoid this panic
    pub fn suggestion(&self) -> &'static str {
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
            PanicCause::DebugAssertFailed => "Review debug assertion condition",
            PanicCause::Unreachable => "Ensure code path is truly unreachable",
            PanicCause::Unimplemented => "Implement the missing functionality",
            PanicCause::Todo => "Complete the TODO implementation",
            PanicCause::PanicInDrop => {
                "Avoid panicking in Drop implementations; use catch_unwind or log errors instead"
            }
            PanicCause::CannotUnwind => {
                "Avoid panicking in extern functions; use catch_unwind at FFI boundaries"
            }
            PanicCause::Unknown => "",
        }
    }
}

/// Detect panic cause from a function name in the call chain
pub fn detect_panic_cause(func_name: &str) -> Option<PanicCause> {
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
        // Could be Option or Result - context would tell us which
        return Some(PanicCause::UnwrapNone);
    }
    if func_name.contains("expect_failed") {
        return Some(PanicCause::ExpectNone);
    }
    // Assert macros
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
    // panic_fmt is the core panic function - if we reach here without a more
    // specific match, it's likely an explicit panic!() call
    if func_name.contains("panic_fmt") {
        return Some(PanicCause::ExplicitPanic);
    }

    None
}
