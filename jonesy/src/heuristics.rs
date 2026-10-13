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

    // Find core::panicking:: functions and abort entry points in a single pass.
    // These two calls were previously separate, each iterating all symbols and
    // demangling every one. Combining them into one pass saves ~30 MB of
    // transient allocations.
    let all_patterns: Vec<&str> = BINARY_PANIC_PREFIXES
        .iter()
        .copied()
        .chain(std::iter::once(ABORT_SYMBOL))
        .collect();

    if let Ok(matches) = symbols.find_all_symbols_with_addresses(&all_patterns) {
        for (sym, dem, addr) in matches {
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

/// Result of evaluating const arithmetic at a source line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConstArithmeticResult {
    /// The arithmetic is safe — the result does not overflow the type.
    Safe,
    /// The arithmetic overflows the type and will panic in debug mode.
    Overflow,
}

/// Evaluate whether a const arithmetic expression at a given source line overflows.
///
/// Reads the source file content, finds `const` declarations to resolve operand
/// values and types, then evaluates the arithmetic to determine if it overflows.
///
/// Returns:
/// - `Some(Safe)` if the line has const-only arithmetic that does not overflow
/// - `Some(Overflow)` if the line has const-only arithmetic that overflows
/// - `None` if the line is not const arithmetic or the values cannot be resolved
pub(crate) fn evaluate_const_arithmetic(
    source: &str,
    line_number: u16,
) -> Option<ConstArithmeticResult> {
    let lines: Vec<&str> = source.lines().collect();
    let target_line = lines.get((line_number as usize).saturating_sub(1))?;

    // Extract expression, split into operands, and verify they look like constants
    let expr = extract_expression(target_line)?;
    let tokens = split_arithmetic_operands(expr)?;
    if !tokens.iter().all(|t| is_const_token(t)) {
        return None;
    }

    // Parse the operator and operands from the expression.
    // split_arithmetic_operands already confirmed an operator exists, so
    // parse_binary_arithmetic will always find one — but we propagate None
    // for safety.
    let (lhs, op, rhs) = parse_binary_arithmetic(expr)?;

    // Resolve operand values and types from const declarations in the source.
    // is_const_token already verified the tokens look like constants, but
    // resolve_const_value additionally requires a matching `const` declaration
    // for SCREAMING_CASE identifiers (rejecting `let` bindings).
    let (lhs_val, lhs_type) = resolve_const_value(lhs, source)?;
    let (rhs_val, rhs_type) = resolve_const_value(rhs, source)?;

    // Determine the result type (use the more specific type, or default to i32)
    let result_type = if lhs_type != IntType::I32Default {
        lhs_type
    } else {
        rhs_type
    };

    // Evaluate whether the arithmetic overflows
    if check_overflow(lhs_val, op, rhs_val, result_type) {
        Some(ConstArithmeticResult::Overflow)
    } else {
        Some(ConstArithmeticResult::Safe)
    }
}

/// Supported integer types for overflow checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntType {
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    /// Default type when no explicit type annotation is given (Rust defaults to i32)
    I32Default,
}

impl IntType {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "u8" => Some(Self::U8),
            "u16" => Some(Self::U16),
            "u32" => Some(Self::U32),
            "u64" => Some(Self::U64),
            "u128" => Some(Self::U128),
            "usize" => Some(Self::Usize),
            "i8" => Some(Self::I8),
            "i16" => Some(Self::I16),
            "i32" => Some(Self::I32),
            "i64" => Some(Self::I64),
            "i128" => Some(Self::I128),
            "isize" => Some(Self::Isize),
            _ => None,
        }
    }

    /// Return the bit width of the type (used for shift overflow checks).
    fn bit_width(self) -> u32 {
        match self {
            Self::U8 | Self::I8 => 8,
            Self::U16 | Self::I16 => 16,
            Self::U32 | Self::I32 | Self::I32Default => 32,
            Self::U64 | Self::I64 => 64,
            Self::U128 | Self::I128 => 128,
            Self::Usize | Self::Isize => 64, // conservative: 64-bit platforms
        }
    }

    /// Check if a value overflows the type's range.
    fn overflows(self, value: i128) -> bool {
        match self {
            Self::U8 => value < 0 || value > u8::MAX as i128,
            Self::U16 => value < 0 || value > u16::MAX as i128,
            Self::U32 => value < 0 || value > u32::MAX as i128,
            Self::U64 => value < 0 || value > u64::MAX as i128,
            Self::U128 => value < 0, // can't overflow u128 max with i128
            Self::Usize => value < 0 || value > u64::MAX as i128, // conservative: 64-bit
            Self::I8 => value < i8::MIN as i128 || value > i8::MAX as i128,
            Self::I16 => value < i16::MIN as i128 || value > i16::MAX as i128,
            Self::I32 | Self::I32Default => value < i32::MIN as i128 || value > i32::MAX as i128,
            Self::I64 => value < i64::MIN as i128 || value > i64::MAX as i128,
            Self::I128 => false, // i128 is our evaluation type, can't overflow itself
            Self::Isize => value < i64::MIN as i128 || value > i64::MAX as i128, // conservative
        }
    }
}

/// Parse a binary arithmetic expression into (left_operand, operator, right_operand).
fn parse_binary_arithmetic(expr: &str) -> Option<(&str, char, &str)> {
    // Try each operator, preferring longer ones first (<<, >>)
    for op_str in ["<<", ">>"] {
        if let Some((lhs, rhs)) = expr.split_once(op_str) {
            return Some((lhs.trim(), op_str.chars().next().unwrap(), rhs.trim()));
        }
    }
    for op in ['+', '-', '*', '/', '%'] {
        if let Some((lhs, rhs)) = expr.split_once(op) {
            let lhs = lhs.trim();
            let rhs = rhs.trim();
            if !lhs.is_empty() && !rhs.is_empty() {
                return Some((lhs, op, rhs));
            }
        }
    }
    None
}

/// Resolve a token to its numeric value and type by searching for `const` declarations
/// in the source file.
///
/// The caller must have already verified the token passes `is_const_token` (either a
/// numeric literal or a SCREAMING_CASE identifier).
fn resolve_const_value(token: &str, source: &str) -> Option<(i128, IntType)> {
    let token = token.trim();

    // Try to parse as a numeric literal first
    if token.starts_with(|c: char| c.is_ascii_digit()) {
        return parse_numeric_literal(token);
    }

    // Bail out if any `let` binding in the source uses the same name.
    // A `let OFFSET = input;` would shadow a file-level `const OFFSET`,
    // making it unsafe to assume the expression uses the const value.
    let let_pattern = format!("let {token}");
    let has_let_shadow = source.lines().any(|l| {
        let t = l.trim();
        t.starts_with(&let_pattern) && t[let_pattern.len()..].starts_with([' ', ':', '='])
    });
    if has_let_shadow {
        return None;
    }

    // Search for a matching `const NAME: TYPE = VALUE;` declaration.
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("const ") {
            if let Some((name_type, value_part)) = rest.split_once('=') {
                let name_type = name_type.trim();
                let value_str = value_part.trim().trim_end_matches(';').trim();

                if let Some((name, type_str)) = name_type.split_once(':') {
                    let name = name.trim();
                    let type_str = type_str.trim();

                    if name == token {
                        let int_type = IntType::from_str(type_str).unwrap_or(IntType::I32Default);
                        let (value, _) = parse_numeric_literal(value_str)?;
                        return Some((value, int_type));
                    }
                }
            }
        }
    }

    None
}

/// Parse a numeric literal (e.g., `7`, `0xFF`, `200u8`, `1_000i32`) into a value and type.
///
/// Handles decimal, hex (`0x`), octal (`0o`), and binary (`0b`) literals with optional
/// underscore separators and type suffixes (`u8`, `i32`, `usize`, etc.).
fn parse_numeric_literal(token: &str) -> Option<(i128, IntType)> {
    let token = token.trim();
    if token.is_empty() || !token.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }

    // Try to find a type suffix by checking known suffix strings at the end
    let type_suffixes = [
        "usize", "isize", "u128", "i128", "u64", "i64", "u32", "i32", "u16", "i16", "u8", "i8",
    ];

    let (num_str, int_type) = type_suffixes
        .iter()
        .find_map(|suffix| {
            token.strip_suffix(suffix).map(|num_part| {
                (
                    num_part,
                    IntType::from_str(suffix).unwrap_or(IntType::I32Default),
                )
            })
        })
        .unwrap_or((token, IntType::I32Default));

    let num_str = num_str.replace('_', "");

    let value = if let Some(hex) = num_str
        .strip_prefix("0x")
        .or_else(|| num_str.strip_prefix("0X"))
    {
        i128::from_str_radix(hex, 16).ok()?
    } else if let Some(oct) = num_str
        .strip_prefix("0o")
        .or_else(|| num_str.strip_prefix("0O"))
    {
        i128::from_str_radix(oct, 8).ok()?
    } else if let Some(bin) = num_str
        .strip_prefix("0b")
        .or_else(|| num_str.strip_prefix("0B"))
    {
        i128::from_str_radix(bin, 2).ok()?
    } else {
        num_str.parse::<i128>().ok()?
    };

    Some((value, int_type))
}

/// Check if an arithmetic operation overflows for the given type.
fn check_overflow(lhs: i128, op: char, rhs: i128, int_type: IntType) -> bool {
    // For shift operations, check the shift amount against the type's bit width.
    // Rust panics when the shift amount >= the bit width, regardless of the operand values.
    if op == '<' || op == '>' {
        let bit_width = int_type.bit_width();
        if rhs < 0 || rhs >= bit_width as i128 {
            return true; // Shift amount out of range
        }
        // Shift is within range — evaluate the result for overflow
        let result = if op == '<' {
            lhs.checked_shl(rhs as u32)
        } else {
            lhs.checked_shr(rhs as u32)
        };
        return match result {
            Some(value) => int_type.overflows(value),
            None => true,
        };
    }

    let result = match op {
        '+' => lhs.checked_add(rhs),
        '-' => lhs.checked_sub(rhs),
        '*' => lhs.checked_mul(rhs),
        '/' => {
            if rhs == 0 {
                return true; // Division by zero
            }
            lhs.checked_div(rhs)
        }
        '%' => {
            if rhs == 0 {
                return true; // Remainder by zero
            }
            lhs.checked_rem(rhs)
        }
        _ => return false,
    };

    match result {
        Some(value) => int_type.overflows(value),
        None => true, // The operation itself overflowed i128
    }
}

/// Safe arithmetic method prefixes that cannot panic on overflow.
/// These methods explicitly handle overflow by returning `Option`, `(T, bool)`,
/// or a clamped/wrapped result instead of panicking.
const SAFE_ARITHMETIC_PREFIXES: &[&str] =
    &[".checked_", ".saturating_", ".wrapping_", ".overflowing_"];

/// Check whether a source line contains ONLY safe arithmetic methods
/// (`checked_*`, `saturating_*`, `wrapping_*`, or `overflowing_*`)
/// and no plain arithmetic operators that could independently overflow.
///
/// These methods are the explicit safe alternatives to unchecked arithmetic.
/// They cannot panic on overflow and should not be flagged as overflow panic points.
///
/// Returns `false` if the line also contains plain arithmetic operators (`+`, `-`,
/// `*`, `/`, `%`, `<<`, `>>`), since the overflow detection might be from those
/// operators rather than the safe method.
///
/// See issue #267.
pub(crate) fn is_safe_arithmetic_method(line: &str) -> bool {
    let trimmed = line.trim();

    // Must contain at least one safe arithmetic method call
    if !SAFE_ARITHMETIC_PREFIXES
        .iter()
        .any(|prefix| trimmed.contains(prefix))
    {
        return false;
    }

    // Strip the line down to just the expression (remove `let x = ...;` wrapper)
    let expr = if let Some(rest) = trimmed.strip_prefix("let ") {
        rest.split_once('=')
            .map(|(_, rhs)| rhs.trim())
            .unwrap_or(trimmed)
    } else {
        trimmed
    };
    let expr = expr.split("//").next().unwrap_or(expr).trim();
    let expr = expr.trim_end_matches(';').trim();

    // Check that the expression does NOT also contain plain arithmetic operators
    // that could independently overflow. We look for operators surrounded by
    // non-operator characters (to avoid matching inside method names or strings).
    let plain_operators = [" + ", " - ", " * ", " / ", " % ", " << ", " >> "];
    !plain_operators.iter().any(|op| expr.contains(op))
}

/// Extract the arithmetic expression from a source line, stripping `let` bindings,
/// trailing semicolons, and comments. Returns `None` if the line has no expression.
fn extract_expression(line: &str) -> Option<&str> {
    let trimmed = line.trim();

    // Strip common prefixes: `let _ = ...;`, `let name = ...;`, `let name: Type = ...;`
    let expr = if let Some(rest) = trimmed.strip_prefix("let ") {
        rest.split_once('=').map(|(_, rhs)| rhs.trim())?
    } else {
        trimmed
    };

    // Strip trailing semicolons and comments
    let expr = expr.split("//").next().unwrap_or(expr).trim();
    let expr = expr.trim_end_matches(';').trim();

    if expr.is_empty() {
        return None;
    }

    Some(expr)
}

/// Split an expression on arithmetic operators, returning the operand tokens.
/// Handles `<<`, `>>` as well as `+`, `-`, `*`, `/`, `%`, with or without spaces.
fn split_arithmetic_operands(expr: &str) -> Option<Vec<&str>> {
    // Check for shift operators first (must check before single-char operators)
    if expr.contains("<<") || expr.contains(">>") {
        let parts: Vec<&str> = if expr.contains("<<") {
            expr.split("<<").map(str::trim).collect()
        } else {
            expr.split(">>").map(str::trim).collect()
        };
        if parts.len() >= 2 && parts.iter().all(|p| !p.is_empty()) {
            return Some(parts);
        }
    }

    // Try single-character arithmetic operators
    for op in ['+', '-', '*', '/', '%'] {
        if expr.contains(op) {
            let parts: Vec<&str> = expr
                .split(op)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            if parts.len() >= 2 {
                return Some(parts);
            }
        }
    }

    None
}

/// Check whether a source line contains arithmetic where all operands are
/// compile-time constants (numeric literals or `SCREAMING_SNAKE_CASE` identifiers).
///
/// **Note**: This is a syntactic check only. It does not verify that SCREAMING_CASE
/// identifiers are actually declared as `const`. The caller should use
/// [`evaluate_const_arithmetic`] for a full semantic check that resolves values.
///
/// This is used as a fast pre-filter before the more expensive source-file scan
/// in `evaluate_const_arithmetic` (see issue #266).
#[cfg(test)]
fn is_const_arithmetic(line: &str) -> bool {
    let expr = match extract_expression(line) {
        Some(e) => e,
        None => return false,
    };

    let tokens = match split_arithmetic_operands(expr) {
        Some(t) => t,
        None => return false,
    };

    tokens.iter().all(|token| is_const_token(token))
}

/// Check whether a token looks like a compile-time constant.
///
/// A token is considered a constant if it is:
/// - A numeric literal (decimal, hex `0x`, octal `0o`, binary `0b`, with optional `_` separators
///   and optional type suffix like `usize`, `i32`)
/// - A `SCREAMING_SNAKE_CASE` identifier (the Rust convention for `const` and `static` values).
///   **Note**: this is a naming convention heuristic — `evaluate_const_arithmetic` verifies
///   that a matching `const` declaration exists in the source file.
fn is_const_token(token: &str) -> bool {
    let token = token.trim();
    if token.is_empty() {
        return false;
    }

    // Numeric literal: starts with a digit
    if token.starts_with(|c: char| c.is_ascii_digit()) {
        return parse_numeric_literal(token).is_some();
    }

    // SCREAMING_SNAKE_CASE identifier: all uppercase letters, digits, and underscores,
    // must contain at least one uppercase letter, must not start with a digit.
    // This is a heuristic — the caller must verify against actual const declarations.
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

    /// Issue #266: `is_const_arithmetic` should detect lines where all operands
    /// are compile-time constants.
    #[test]
    fn test_is_const_arithmetic() {
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

    /// Issue #266: `evaluate_const_arithmetic` should resolve constant values from
    /// the source file and evaluate whether the arithmetic overflows.
    #[test]
    fn test_evaluate_const_arithmetic_safe() {
        // Source file with a const declaration and safe arithmetic
        let source = "const OFFSET: usize = 7;\n\
                      \n\
                      fn example() {\n\
                          let _result = OFFSET + 1;\n\
                      }";
        // Line 4 has `let _result = OFFSET + 1;` — OFFSET=7, 7+1=8, no overflow
        assert_eq!(
            evaluate_const_arithmetic(source, 4),
            Some(ConstArithmeticResult::Safe)
        );
    }

    #[test]
    fn test_evaluate_const_arithmetic_overflow() {
        // Source file with a const declaration and overflowing arithmetic
        let source = "const A: u8 = 200;\n\
                      const B: u8 = 200;\n\
                      \n\
                      fn example() {\n\
                          let _ = A + B;\n\
                      }";
        // Line 5 has `let _ = A + B;` — A=200, B=200, 200+200=400 overflows u8
        assert_eq!(
            evaluate_const_arithmetic(source, 5),
            Some(ConstArithmeticResult::Overflow)
        );
    }

    #[test]
    fn test_evaluate_const_arithmetic_literals_safe() {
        let source = "fn example() {\n\
                          let _ = 7 + 1;\n\
                      }";
        // Pure literals: 7+1=8, no overflow (default integer type is i32)
        assert_eq!(
            evaluate_const_arithmetic(source, 2),
            Some(ConstArithmeticResult::Safe)
        );
    }

    #[test]
    fn test_evaluate_const_arithmetic_not_const() {
        let source = "fn example() {\n\
                          let x: i32 = 42;\n\
                          let _ = x + 1;\n\
                      }";
        // `x` is a runtime variable, not const arithmetic
        assert_eq!(evaluate_const_arithmetic(source, 3), None);
    }

    #[test]
    fn test_evaluate_const_arithmetic_typed_literals() {
        // Literals with type suffixes: 1u8 + 2u8 = 3, no overflow
        let source = "fn example() {\n\
                          let _ = 1u8 + 2;\n\
                      }";
        assert_eq!(
            evaluate_const_arithmetic(source, 2),
            Some(ConstArithmeticResult::Safe)
        );
    }

    #[test]
    fn test_evaluate_const_arithmetic_typed_literal_overflow() {
        // 250u8 + 10u8 = 260, overflows u8
        let source = "fn example() {\n\
                          let _ = 250u8 + 10;\n\
                      }";
        assert_eq!(
            evaluate_const_arithmetic(source, 2),
            Some(ConstArithmeticResult::Overflow)
        );
    }

    #[test]
    fn test_is_const_arithmetic_no_spaces() {
        // Operators without spaces should also be detected
        assert!(is_const_arithmetic("    let _ = OFFSET+1;"));
        assert!(is_const_arithmetic("    let _ = SIZE*2;"));
    }

    #[test]
    fn test_is_const_arithmetic_shift() {
        // Shift operators
        assert!(is_const_arithmetic("    let _ = SIZE << 2;"));
        assert!(is_const_arithmetic("    let _ = VALUE >> 3;"));
    }

    #[test]
    fn test_evaluate_uppercase_let_binding_not_const() {
        // An uppercase let binding is NOT a const — should not be treated as const arithmetic
        let source = "fn example(input: usize) {\n\
                          let OFFSET = input;\n\
                          let _ = OFFSET + 1;\n\
                      }";
        // OFFSET is a let binding, not a const declaration — resolve_const_value should fail
        assert_eq!(evaluate_const_arithmetic(source, 3), None);
    }

    #[test]
    fn test_evaluate_const_shadowed_by_let_binding() {
        // A file-level const is shadowed by a let binding in a function.
        // We should NOT trust the const value because the let binding may hold
        // a different runtime value.
        let source = "const OFFSET: u8 = 1;\n\
                      \n\
                      fn example(input: u8) {\n\
                          let OFFSET = input;\n\
                          let _ = OFFSET + 1;\n\
                      }";
        // Even though const OFFSET = 1 and 1 + 1 = 2 (safe), the let binding
        // shadows it, so we can't be sure — should return None
        assert_eq!(evaluate_const_arithmetic(source, 5), None);
    }

    #[test]
    fn test_evaluate_shift_overflow_bit_width() {
        // Shift by exactly the bit width of the type should overflow
        let source = "const A: u8 = 0;\n\
                      const N: u8 = 8;\n\
                      fn f() {\n\
                          let _ = A << N;\n\
                      }";
        // u8 has 8 bits, so shifting by 8 overflows regardless of the value
        assert_eq!(
            evaluate_const_arithmetic(source, 4),
            Some(ConstArithmeticResult::Overflow)
        );
    }

    #[test]
    fn test_evaluate_shift_safe_within_bit_width() {
        let source = "const A: u8 = 1;\n\
                      const N: u8 = 7;\n\
                      fn f() {\n\
                          let _ = A << N;\n\
                      }";
        // 1 << 7 = 128, fits in u8 (0..=255)
        assert_eq!(
            evaluate_const_arithmetic(source, 4),
            Some(ConstArithmeticResult::Safe)
        );
    }

    // -- parse_numeric_literal coverage --

    #[test]
    fn test_parse_numeric_literal_decimal() {
        assert_eq!(parse_numeric_literal("42"), Some((42, IntType::I32Default)));
        assert_eq!(parse_numeric_literal("0"), Some((0, IntType::I32Default)));
    }

    #[test]
    fn test_parse_numeric_literal_hex() {
        assert_eq!(
            parse_numeric_literal("0xFF"),
            Some((255, IntType::I32Default))
        );
        assert_eq!(
            parse_numeric_literal("0XAB"),
            Some((171, IntType::I32Default))
        );
    }

    #[test]
    fn test_parse_numeric_literal_octal() {
        assert_eq!(
            parse_numeric_literal("0o77"),
            Some((63, IntType::I32Default))
        );
    }

    #[test]
    fn test_parse_numeric_literal_binary() {
        assert_eq!(
            parse_numeric_literal("0b1010"),
            Some((10, IntType::I32Default))
        );
    }

    #[test]
    fn test_parse_numeric_literal_with_underscores() {
        assert_eq!(
            parse_numeric_literal("1_000_000"),
            Some((1_000_000, IntType::I32Default))
        );
    }

    #[test]
    fn test_parse_numeric_literal_with_type_suffix() {
        assert_eq!(parse_numeric_literal("1u8"), Some((1, IntType::U8)));
        assert_eq!(parse_numeric_literal("200u8"), Some((200, IntType::U8)));
        assert_eq!(parse_numeric_literal("42i32"), Some((42, IntType::I32)));
        assert_eq!(
            parse_numeric_literal("100usize"),
            Some((100, IntType::Usize))
        );
        assert_eq!(parse_numeric_literal("0xFFu16"), Some((255, IntType::U16)));
    }

    #[test]
    fn test_parse_numeric_literal_invalid() {
        assert_eq!(parse_numeric_literal(""), None);
        assert_eq!(parse_numeric_literal("abc"), None);
    }

    // -- check_overflow coverage for all operations --

    #[test]
    fn test_check_overflow_subtraction() {
        // u8: 0 - 1 underflows
        assert!(check_overflow(0, '-', 1, IntType::U8));
        // i32: 100 - 50 = 50, safe
        assert!(!check_overflow(100, '-', 50, IntType::I32Default));
    }

    #[test]
    fn test_check_overflow_multiplication() {
        // u8: 20 * 20 = 400, overflows u8
        assert!(check_overflow(20, '*', 20, IntType::U8));
        // i32: 100 * 100 = 10000, safe
        assert!(!check_overflow(100, '*', 100, IntType::I32Default));
    }

    #[test]
    fn test_check_overflow_division() {
        // division by zero
        assert!(check_overflow(10, '/', 0, IntType::I32Default));
        // i32: 100 / 10 = 10, safe
        assert!(!check_overflow(100, '/', 10, IntType::I32Default));
    }

    #[test]
    fn test_check_overflow_remainder() {
        // remainder by zero
        assert!(check_overflow(10, '%', 0, IntType::I32Default));
        // i32: 100 % 7 = 2, safe
        assert!(!check_overflow(100, '%', 7, IntType::I32Default));
    }

    #[test]
    fn test_check_overflow_shift() {
        // left shift: 1 << 4 = 16, safe for u8
        assert!(!check_overflow(1, '<', 4, IntType::U8));
        // left shift: 1 << 8 overflows u8 — shift amount >= bit width (8)
        assert!(check_overflow(1, '<', 8, IntType::U8));
        // 0 << 8 also overflows u8 — shift amount >= bit width regardless of value
        assert!(check_overflow(0, '<', 8, IntType::U8));
        // right shift: 255 >> 4 = 15, safe for u8
        assert!(!check_overflow(255, '>', 4, IntType::U8));
        // right shift: shift amount >= bit width overflows
        assert!(check_overflow(255, '>', 8, IntType::U8));
        // negative shift amount is invalid
        assert!(check_overflow(1, '<', -1, IntType::U8));
        // 2u8 << 7 = 256, overflows u8 range (but shift amount is valid)
        assert!(check_overflow(2, '<', 7, IntType::U8));
        // 1u8 << 7 = 128, safe for u8
        assert!(!check_overflow(1, '<', 7, IntType::U8));
        // left shift by >= 128: checked_shl returns None (i128 overflow)
        assert!(check_overflow(1, '<', 200, IntType::I32Default));
    }

    #[test]
    fn test_check_overflow_unknown_operator() {
        // Unknown operator returns false (no overflow detected)
        assert!(!check_overflow(1, '?', 1, IntType::I32Default));
    }

    // -- IntType::overflows coverage for various types --

    #[test]
    fn test_overflows_unsigned_types() {
        assert!(IntType::U16.overflows(70000));
        assert!(!IntType::U16.overflows(60000));
        assert!(IntType::U32.overflows(u32::MAX as i128 + 1));
        assert!(!IntType::U32.overflows(u32::MAX as i128));
        assert!(IntType::U64.overflows(-1));
        assert!(!IntType::U64.overflows(0));
        assert!(IntType::U128.overflows(-1));
        assert!(!IntType::U128.overflows(0));
        assert!(IntType::Usize.overflows(-1));
        assert!(!IntType::Usize.overflows(1000));
    }

    #[test]
    fn test_overflows_signed_types() {
        assert!(IntType::I8.overflows(128));
        assert!(!IntType::I8.overflows(127));
        assert!(IntType::I8.overflows(-129));
        assert!(!IntType::I8.overflows(-128));
        assert!(IntType::I16.overflows(40000));
        assert!(!IntType::I16.overflows(30000));
        assert!(!IntType::I64.overflows(i64::MAX as i128));
        assert!(IntType::I64.overflows(i64::MAX as i128 + 1));
        assert!(!IntType::I128.overflows(i128::MAX));
        assert!(!IntType::Isize.overflows(0));
        assert!(IntType::Isize.overflows(i64::MAX as i128 + 1));
    }

    // -- evaluate_const_arithmetic coverage for more operations --

    #[test]
    fn test_evaluate_const_arithmetic_subtraction_safe() {
        let source = "const A: u8 = 10;\n\
                      const B: u8 = 5;\n\
                      fn f() {\n\
                          let _ = A - B;\n\
                      }";
        assert_eq!(
            evaluate_const_arithmetic(source, 4),
            Some(ConstArithmeticResult::Safe)
        );
    }

    #[test]
    fn test_evaluate_const_arithmetic_subtraction_underflow() {
        let source = "const A: u8 = 5;\n\
                      const B: u8 = 10;\n\
                      fn f() {\n\
                          let _ = A - B;\n\
                      }";
        assert_eq!(
            evaluate_const_arithmetic(source, 4),
            Some(ConstArithmeticResult::Overflow)
        );
    }

    #[test]
    fn test_evaluate_const_arithmetic_multiplication() {
        let source = "const A: u8 = 20;\n\
                      const B: u8 = 20;\n\
                      fn f() {\n\
                          let _ = A * B;\n\
                      }";
        assert_eq!(
            evaluate_const_arithmetic(source, 4),
            Some(ConstArithmeticResult::Overflow)
        );
    }

    #[test]
    fn test_evaluate_const_arithmetic_division_safe() {
        let source = "const A: i32 = 100;\n\
                      const B: i32 = 10;\n\
                      fn f() {\n\
                          let _ = A / B;\n\
                      }";
        assert_eq!(
            evaluate_const_arithmetic(source, 4),
            Some(ConstArithmeticResult::Safe)
        );
    }

    #[test]
    fn test_evaluate_const_arithmetic_remainder() {
        let source = "const A: i32 = 100;\n\
                      const B: i32 = 7;\n\
                      fn f() {\n\
                          let _ = A % B;\n\
                      }";
        assert_eq!(
            evaluate_const_arithmetic(source, 4),
            Some(ConstArithmeticResult::Safe)
        );
    }

    #[test]
    fn test_evaluate_const_arithmetic_shift_safe() {
        let source = "const A: u32 = 1;\n\
                      fn f() {\n\
                          let _ = A << 4;\n\
                      }";
        assert_eq!(
            evaluate_const_arithmetic(source, 3),
            Some(ConstArithmeticResult::Safe)
        );
    }

    #[test]
    fn test_evaluate_const_arithmetic_hex_literal() {
        let source = "fn f() {\n\
                          let _ = 0xFF + 1;\n\
                      }";
        // 0xFF + 1 = 256, safe for i32 default
        assert_eq!(
            evaluate_const_arithmetic(source, 2),
            Some(ConstArithmeticResult::Safe)
        );
    }

    #[test]
    fn test_evaluate_const_arithmetic_line_out_of_range() {
        let source = "fn f() {}";
        // Line 100 is out of range
        assert_eq!(evaluate_const_arithmetic(source, 100), None);
    }

    #[test]
    fn test_evaluate_const_arithmetic_no_expression() {
        let source = "fn f() {\n\
                          // just a comment\n\
                      }";
        assert_eq!(evaluate_const_arithmetic(source, 2), None);
    }

    #[test]
    fn test_evaluate_const_arithmetic_bare_expression() {
        // Expression without `let` prefix — e.g., in a return position
        let source = "const A: i32 = 10;\n\
                      fn f() -> i32 {\n\
                          A + 5\n\
                      }";
        assert_eq!(
            evaluate_const_arithmetic(source, 3),
            Some(ConstArithmeticResult::Safe)
        );
    }

    // -- IntType::from_str coverage --

    #[test]
    fn test_int_type_from_str() {
        assert_eq!(IntType::from_str("u8"), Some(IntType::U8));
        assert_eq!(IntType::from_str("u16"), Some(IntType::U16));
        assert_eq!(IntType::from_str("u32"), Some(IntType::U32));
        assert_eq!(IntType::from_str("u64"), Some(IntType::U64));
        assert_eq!(IntType::from_str("u128"), Some(IntType::U128));
        assert_eq!(IntType::from_str("usize"), Some(IntType::Usize));
        assert_eq!(IntType::from_str("i8"), Some(IntType::I8));
        assert_eq!(IntType::from_str("i16"), Some(IntType::I16));
        assert_eq!(IntType::from_str("i32"), Some(IntType::I32));
        assert_eq!(IntType::from_str("i64"), Some(IntType::I64));
        assert_eq!(IntType::from_str("i128"), Some(IntType::I128));
        assert_eq!(IntType::from_str("isize"), Some(IntType::Isize));
        assert_eq!(IntType::from_str("f64"), None);
        assert_eq!(IntType::from_str("string"), None);
    }

    /// Issue #267: `checked_add`, `saturating_mul`, `wrapping_sub`, and similar
    /// safe arithmetic methods should not be flagged as overflow panic points.
    #[test]
    fn test_is_safe_arithmetic_method() {
        // checked_* methods return Option — cannot panic
        assert!(is_safe_arithmetic_method(
            "    let result = x.checked_add(y).ok_or(Error::Overflow)?;"
        ));
        assert!(is_safe_arithmetic_method(
            "    out_path_start.checked_add(out_path_bytes)"
        ));
        assert!(is_safe_arithmetic_method("    x.checked_mul(2)"));
        assert!(is_safe_arithmetic_method("    a.checked_sub(b)"));
        assert!(is_safe_arithmetic_method("    a.checked_div(b)"));
        assert!(is_safe_arithmetic_method("    a.checked_rem(b)"));
        assert!(is_safe_arithmetic_method("    a.checked_shl(4)"));
        assert!(is_safe_arithmetic_method("    a.checked_shr(4)"));
        assert!(is_safe_arithmetic_method("    a.checked_neg()"));
        assert!(is_safe_arithmetic_method("    a.checked_pow(3)"));
        assert!(is_safe_arithmetic_method("    a.checked_abs()"));

        // saturating_* methods clamp on overflow — cannot panic
        assert!(is_safe_arithmetic_method("    x.saturating_add(1)"));
        assert!(is_safe_arithmetic_method("    x.saturating_mul(2)"));
        assert!(is_safe_arithmetic_method("    x.saturating_sub(1)"));

        // wrapping_* methods wrap on overflow — cannot panic
        assert!(is_safe_arithmetic_method("    x.wrapping_add(1)"));
        assert!(is_safe_arithmetic_method("    x.wrapping_mul(2)"));

        // overflowing_* methods return (result, bool) — cannot panic
        assert!(is_safe_arithmetic_method("    x.overflowing_add(1)"));
        assert!(is_safe_arithmetic_method("    x.overflowing_mul(2)"));

        // Plain arithmetic operators — NOT safe
        assert!(!is_safe_arithmetic_method("    let _ = x + 1;"));
        assert!(!is_safe_arithmetic_method("    let _ = a * b;"));
        assert!(!is_safe_arithmetic_method("    let _ = x << 4;"));

        // Unrelated method calls — NOT safe arithmetic
        assert!(!is_safe_arithmetic_method("    x.unwrap()"));
        assert!(!is_safe_arithmetic_method("    v.push(1)"));

        // Mixed: safe method AND a plain operator — NOT safe (the + could overflow)
        assert!(!is_safe_arithmetic_method(
            "    let result = x.checked_add(1).unwrap_or(0) + y;"
        ));
        assert!(!is_safe_arithmetic_method(
            "    let _ = x.saturating_add(1) * z;"
        ));

        // Safe method chained with non-arithmetic methods — still safe
        assert!(is_safe_arithmetic_method(
            "    x.checked_add(y).ok_or_else(|| Error::Overflow)?;"
        ));
        assert!(is_safe_arithmetic_method(
            "    x.checked_mul(2).unwrap_or(0)"
        ));
    }
}
