pub fn cause_a_panic() {
    // jonesy: expect panic explicit panic call
    panic!("panic");
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic unwrap on None
pub fn cause_an_unwrap() {
    let _: () = None.unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic unwrap on Err
pub fn cause_unwrap_err() {
    let _: () = Err("error").unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic expect on None
pub fn cause_expect_none() {
    let _: () = None.expect("expected a value");
}

#[allow(clippy::assertions_on_constants)]
// jonesy: expect panic assert failed
pub fn cause_assert() {
    assert!(false);
}

// jonesy: expect panic debug_assert failed (debug builds only)
pub fn cause_debug_assert() {
    debug_assert!(false);
}

// jonesy: expect panic unreachable reached
pub fn cause_unreachable() {
    unreachable!();
}

// jonesy: expect panic unimplemented reached
pub fn cause_unimplemented() {
    unimplemented!();
}

// jonesy: expect panic todo reached
pub fn cause_todo() {
    todo!();
}

#[allow(unconditional_panic)]
// jonesy: expect panic division by zero
pub fn cause_divide_by_zero() {
    let _ = 1 / 0;
}

#[allow(arithmetic_overflow)]
pub fn cause_arithmetic_overflow() {
    let x: i32 = i32::MAX;
    // jonesy: expect panic arithmetic overflow (debug builds)
    let _ = x + 1;
}

#[allow(arithmetic_overflow)]
// jonesy: expect panic shift overflow
pub fn cause_shift_overflow() {
    let _ = 1u32 << 33;
}
