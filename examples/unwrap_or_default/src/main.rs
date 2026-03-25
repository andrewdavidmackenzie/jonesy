//! Test case for issue #181: DW_AT_specification handling in DWARF parsing.
//!
//! This example tests that jonesy correctly parses functions with DW_AT_specification
//! references (where the definition DIE references a separate declaration DIE for
//! name/file/line info). Without this handling, TimeStamp::now() would be missing
//! from the FunctionIndex and panic paths would incorrectly skip over it.
//!
//! The key verification is that TimeStamp::now (line 19) appears in the call tree
//! as a child of its callers (lines 35 and 40), creating a consistent hierarchy.

use std::time::{SystemTime, UNIX_EPOCH};

/// A timestamp wrapper similar to the one in meshchat.
pub struct TimeStamp(u128);

impl TimeStamp {
    /// Get the current time.
    /// The duration_since() call at line 22 leads to an internal stdlib expect().
    // jonesy: expect panic(expect)
    pub fn now() -> Self {
        Self(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
    }
}

/// This function calls TimeStamp::now().
fn get_current_time() -> TimeStamp {
    // jonesy: expect panic(format)
    println!("Getting current time...");
    // jonesy: expect panic(expect)
    TimeStamp::now()
}

/// Another function that calls now() and prints.
fn log_event(msg: &str) {
    // jonesy: expect panic(expect)
    let ts = TimeStamp::now();
    // jonesy: expect panic(format)
    println!("[{}] {}", ts.0, msg);
}

fn main() {
    // jonesy: expect panic(format, expect)
    let ts = get_current_time();
    // jonesy: expect panic(format)
    println!("Current timestamp: {}", ts.0);
    // jonesy: expect panic(format, expect)
    log_event("Application started");
}
