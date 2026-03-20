//! Static library example demonstrating DCE behavior with `#[no_mangle]`.
//!
//! Static libraries (.a files) are designed for C FFI. Only functions exported
//! with `#[no_mangle]` are preserved - other functions are eliminated by dead
//! code elimination (DCE) since C code cannot call mangled Rust symbols.
//!
//! This means jonesy correctly reports only panics in `#[no_mangle]` functions,
//! because those are the only reachable panic points from C code.

/// Exported function - preserved in staticlib, panics ARE detected.
///
/// This function uses `#[no_mangle]` so it can be called from C code.
/// The linker preserves it, and jonesy detects its panic points.
// jonesy: expect panic
#[no_mangle]
pub extern "C" fn exported_function() {
    panic!("exported panic - this WILL be detected");
}

/// Internal function - eliminated by DCE, panics NOT detected.
///
/// This function has no `#[no_mangle]`, so C code cannot call it.
/// The linker eliminates it as dead code, and jonesy correctly
/// reports no panics (they are unreachable).
pub fn internal_function() {
    panic!("internal panic - this will NOT be detected (DCE removes it)");
}
