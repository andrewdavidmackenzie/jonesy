//! Binary one - all panic types at unique line numbers for testing

fn main() {
    // Dispatch based on argument to ensure all functions are linked
    match std::env::args().nth(1).as_deref() {
        Some("unwrap") => cause_an_unwrap(),
        Some("unwrap_err") => cause_unwrap_err(),
        Some("expect_none") => cause_expect_none(),
        Some("expect_err") => cause_expect_err(),
        Some("unwrap_err_ok") => cause_unwrap_err_on_ok(),
        Some("expect_err_ok") => cause_expect_err_on_ok(),
        Some("assert") => cause_assert(),
        Some("assert_eq") => cause_assert_eq(),
        Some("assert_ne") => cause_assert_ne(),
        Some("debug_assert") => cause_debug_assert(),
        Some("debug_assert_eq") => cause_debug_assert_eq(),
        Some("debug_assert_ne") => cause_debug_assert_ne(),
        Some("unreachable") => cause_unreachable(),
        Some("unimplemented") => cause_unimplemented(),
        Some("todo") => cause_todo(),
        Some("div_zero") => cause_divide_by_zero(),
        Some("overflow") => cause_arithmetic_overflow(),
        Some("shift") => cause_shift_overflow(),
        Some("slice") => cause_slice_index_oob(),
        Some("string") => cause_string_index_panic(),
        _ => {
            // jonesy: expect panic explicit panic call
            panic!("panic from bin_one");
        }
    }
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic unwrap on None
fn cause_an_unwrap() {
    let _: () = None.unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic unwrap on Err
fn cause_unwrap_err() {
    let _: () = Err("error").unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic expect on None
fn cause_expect_none() {
    let _: () = None.expect("expected a value");
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic expect on Err
fn cause_expect_err() {
    let _: () = Err("error").expect("expected ok");
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic unwrap_err on Ok
fn cause_unwrap_err_on_ok() {
    let _: &str = Ok::<i32, &str>(42).unwrap_err();
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic expect_err on Ok
fn cause_expect_err_on_ok() {
    let _: &str = Ok::<i32, &str>(42).expect_err("expected an error");
}

#[allow(clippy::assertions_on_constants)]
// jonesy: expect panic assert failed
fn cause_assert() {
    assert!(false);
}

#[allow(clippy::assertions_on_constants)]
// TODO: jonesy does not detect assert_eq yet
fn cause_assert_eq() {
    assert_eq!(1, 2);
}

#[allow(clippy::assertions_on_constants, clippy::eq_op)]
// TODO: jonesy does not detect assert_ne yet
fn cause_assert_ne() {
    assert_ne!(1, 1);
}

// jonesy: expect panic debug_assert failed (debug builds only)
fn cause_debug_assert() {
    debug_assert!(false);
}

// TODO: jonesy does not detect debug_assert_eq yet (debug builds only)
fn cause_debug_assert_eq() {
    debug_assert_eq!(1, 2);
}

#[allow(clippy::eq_op)]
// TODO: jonesy does not detect debug_assert_ne yet (debug builds only)
fn cause_debug_assert_ne() {
    debug_assert_ne!(1, 1);
}

// jonesy: expect panic unreachable reached
fn cause_unreachable() {
    unreachable!();
}

// jonesy: expect panic unimplemented reached
fn cause_unimplemented() {
    unimplemented!();
}

// jonesy: expect panic todo reached
fn cause_todo() {
    todo!();
}

#[allow(unconditional_panic)]
// jonesy: expect panic division by zero
fn cause_divide_by_zero() {
    let _ = 1 / 0;
}

#[allow(arithmetic_overflow)]
fn cause_arithmetic_overflow() {
    let x: i32 = i32::MAX;
    // jonesy: expect panic arithmetic overflow (debug builds)
    let _ = x + 1;
}

#[allow(arithmetic_overflow)]
// jonesy: expect panic shift overflow
fn cause_shift_overflow() {
    let _ = 1u32 << 33;
}

#[allow(clippy::useless_vec)]
// TODO: jonesy slice index detection is platform-specific
fn cause_slice_index_oob() {
    let v = vec![1, 2, 3];
    let _ = v[10];
}

// TODO: jonesy does not detect string index panic yet
fn cause_string_index_panic() {
    let s = "hello 世界";
    let _ = &s[0..7]; // panics - cuts through UTF-8 char
}
