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
    /// Unwrap on None or Err
    Unwrap,
    /// Expect on None or Err
    Expect,
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
    /// HashMap/BTreeMap key not found (Index trait on map)
    KeyNotFound,
    /// Async function polled after completion
    AsyncFnResumed,
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
            PanicCause::Unwrap => "unwrap",
            PanicCause::Expect => "expect",
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
            PanicCause::KeyNotFound => "key_not_found",
            PanicCause::AsyncFnResumed => "async_resumed",
            PanicCause::Unknown => "unknown",
        }
    }

    /// Get the parent/generic configuration identifier, if any.
    /// This allows "overflow" in config to match specific types like
    /// "div_overflow" and "shift_overflow".
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
            "key_not_found",
            "async_resumed",
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
            PanicCause::Unwrap => "unwrap() failed",
            PanicCause::Expect => "expect() failed",
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
            PanicCause::KeyNotFound => "key not found in map",
            PanicCause::AsyncFnResumed => "async function polled after completion",
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
            PanicCause::Unwrap => "Use if let, match, unwrap_or, or ? operator instead",
            PanicCause::Expect => "Use if let, match, unwrap_or, or ? operator instead",
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
            PanicCause::KeyNotFound => {
                "Use .get() to safely look up keys; returns None instead of panicking"
            }
            PanicCause::AsyncFnResumed => {
                "Ensure futures are not polled after returning Poll::Ready; check executor logic"
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
            PanicCause::Unwrap => {
                "This calls a function that may call unwrap(). Consider a fallible alternative (e.g., try_*)"
            }
            PanicCause::Expect => {
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
            PanicCause::KeyNotFound => {
                "This calls a function that indexes a map. Use .get() instead of [] for safe lookup"
            }
            PanicCause::AsyncFnResumed => {
                "This calls an async function that panics if polled after completion"
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
        // Extract just the last segment for try_ suggestions (e.g., "my_crate::foo" -> "foo")
        let short_func = func.rsplit("::").next().unwrap_or(func);
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
            PanicCause::Unwrap => {
                format!(
                    "This calls `{func}` which may call unwrap(). Consider a fallible alternative (e.g., try_{short_func})"
                )
            }
            PanicCause::Expect => {
                format!(
                    "This calls `{func}` which may call expect(). Consider a fallible alternative (e.g., try_{short_func})"
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
            PanicCause::KeyNotFound => {
                format!(
                    "This calls `{func}` which indexes a map. Use .get() instead of [] for safe lookup"
                )
            }
            PanicCause::AsyncFnResumed => {
                format!(
                    "This calls `{func}` which is an async function that panics if polled after completion"
                )
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
            PanicCause::Unwrap => "JP006",
            PanicCause::Expect => "JP008",
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
            PanicCause::KeyNotFound => "JP023",
            PanicCause::AsyncFnResumed => "JP024",
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
            PanicCause::Unwrap => "JP006-unwrap",
            PanicCause::Expect => "JP008-expect",
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
            PanicCause::KeyNotFound => "JP023-key-not-found",
            PanicCause::AsyncFnResumed => "JP024-async-fn-resumed",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panic_cause_id() {
        assert_eq!(PanicCause::ExplicitPanic.id(), "panic");
        assert_eq!(PanicCause::BoundsCheck.id(), "bounds");
        assert_eq!(PanicCause::Unwrap.id(), "unwrap");
        assert_eq!(PanicCause::Expect.id(), "expect");
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
        assert_eq!(PanicCause::Unwrap.description(), "unwrap() failed");
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
        assert_eq!(PanicCause::Unwrap.error_code(), "JP006");
        assert_eq!(PanicCause::Unknown.error_code(), "JP000");
    }

    #[test]
    fn test_panic_cause_docs_slug() {
        assert_eq!(
            PanicCause::ExplicitPanic.docs_slug(),
            "JP001-explicit-panic"
        );
        assert_eq!(PanicCause::BoundsCheck.docs_slug(), "JP002-bounds-check");
        assert_eq!(PanicCause::Unwrap.docs_slug(), "JP006-unwrap");
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
        assert!(!PanicCause::Unwrap.is_debug_only());
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
        assert!(PanicCause::Unwrap.release_warning().is_none());
    }

    #[test]
    fn test_panic_cause_suggestion_direct() {
        let suggestion = PanicCause::Unwrap.suggestion(true);
        assert!(suggestion.contains("if let") || suggestion.contains("match"));
    }

    #[test]
    fn test_panic_cause_suggestion_indirect() {
        let suggestion = PanicCause::Unwrap.suggestion(false);
        assert!(suggestion.contains("calls a function"));
    }

    #[test]
    fn test_panic_cause_format_suggestion_with_function() {
        let suggestion = PanicCause::Unwrap.format_suggestion(false, Some("parse_config"));
        assert!(suggestion.contains("parse_config"));
    }

    #[test]
    fn test_panic_cause_format_suggestion_direct() {
        let suggestion = PanicCause::Unwrap.format_suggestion(true, Some("ignored"));
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
            PanicCause::Unwrap,
            PanicCause::Unwrap,
            PanicCause::Expect,
            PanicCause::Expect,
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
            PanicCause::Unwrap
                .indirect_suggestion()
                .contains("unwrap()")
        );
        assert!(
            PanicCause::Unwrap
                .indirect_suggestion()
                .contains("unwrap()")
        );
        assert!(
            PanicCause::Expect
                .indirect_suggestion()
                .contains("expect()")
        );
        assert!(
            PanicCause::Expect
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
            PanicCause::Unwrap,
            PanicCause::Unwrap,
            PanicCause::Expect,
            PanicCause::Expect,
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
        assert_eq!(PanicCause::Expect.docs_slug(), "JP008-expect");
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
}
