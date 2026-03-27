// This example tests the "simple" pattern where each function directly
// triggers a panic without using rand to prevent optimizer elimination.
// The compiler may inline or eliminate some of these calls, which tests
// that jonesy correctly detects source lines even with optimized code.

pub fn cause_a_panic() {
    // jonesy: expect panic explicit panic call
    panic!("panic");
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn cause_an_unwrap() {
    // jonesy: expect panic unwrap on None
    let _: () = None.unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn cause_unwrap_err() {
    // jonesy: expect panic unwrap on Err
    let _: () = Err("error").unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn cause_expect_none() {
    // jonesy: expect panic expect on None
    let _: () = None.expect("expected a value");
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn cause_expect_err() {
    // jonesy: expect panic expect on Err
    let _: () = Err("error").expect("expected ok");
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn cause_unwrap_err_on_ok() {
    // jonesy: expect panic unwrap_err on Ok
    let _: &str = Ok::<i32, &str>(42).unwrap_err();
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn cause_expect_err_on_ok() {
    // jonesy: expect panic expect_err on Ok
    let _: &str = Ok::<i32, &str>(42).expect_err("expected an error");
}

#[allow(clippy::assertions_on_constants)]
pub fn cause_assert() {
    // jonesy: expect panic assert failed
    assert!(false);
}

#[allow(clippy::assertions_on_constants)]
pub fn cause_assert_eq() {
    // jonesy: expect panic assert_eq failed
    assert_eq!(1, 2);
}

#[allow(clippy::assertions_on_constants, clippy::eq_op)]
pub fn cause_assert_ne() {
    // jonesy: expect panic assert_ne failed
    assert_ne!(1, 1);
}

pub fn cause_debug_assert() {
    // jonesy: expect panic assert failed (debug builds only)
    debug_assert!(false);
}

pub fn cause_debug_assert_eq() {
    // jonesy: expect panic assert_eq failed (debug builds only)
    debug_assert_eq!(1, 2);
}

#[allow(clippy::eq_op)]
pub fn cause_debug_assert_ne() {
    // jonesy: expect panic assert_ne failed (debug builds only)
    debug_assert_ne!(1, 1);
}

pub fn cause_unreachable() {
    // jonesy: expect panic unreachable reached
    unreachable!();
}

pub fn cause_unimplemented() {
    // jonesy: expect panic unimplemented reached
    unimplemented!();
}

pub fn cause_todo() {
    // jonesy: expect panic
    todo!();
}

#[allow(unconditional_panic)]
pub fn cause_divide_by_zero() {
    // jonesy: expect panic division by zero
    let _ = 1 / 0;
}

#[allow(arithmetic_overflow)]
pub fn cause_arithmetic_overflow() {
    let x: i32 = i32::MAX;
    // jonesy: expect panic arithmetic overflow (debug builds)
    let _ = x + 1;
}

#[allow(arithmetic_overflow)]
pub fn cause_shift_overflow() {
    // jonesy: expect panic shift overflow
    let _ = 1u32 << 33;
}

#[allow(clippy::useless_vec)]
pub fn cause_slice_index_oob() {
    let v = vec![1, 2, 3];
    // jonesy: expect panic index out of bounds
    let _ = v[10];
}

fn main() {
    cause_a_panic();
    cause_an_unwrap();
    cause_unwrap_err();
    cause_expect_none();
    cause_expect_err();
    cause_unwrap_err_on_ok();
    cause_expect_err_on_ok();
    cause_assert();
    cause_assert_eq();
    cause_assert_ne();
    cause_debug_assert();
    cause_debug_assert_eq();
    cause_debug_assert_ne();
    cause_unreachable();
    cause_unimplemented();
    cause_todo();
    cause_divide_by_zero();
    cause_arithmetic_overflow();
    cause_shift_overflow();
    cause_slice_index_oob();
}
