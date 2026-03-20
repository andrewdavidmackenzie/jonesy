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

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic expect on Err
pub fn cause_expect_err() {
    let _: () = Err("error").expect("expected ok");
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic unwrap_err on Ok
pub fn cause_unwrap_err_on_ok() {
    let _: &str = Ok::<i32, &str>(42).unwrap_err();
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic expect_err on Ok
pub fn cause_expect_err_on_ok() {
    let _: &str = Ok::<i32, &str>(42).expect_err("expected an error");
}

#[allow(clippy::assertions_on_constants)]
// jonesy: expect panic assert failed
pub fn cause_assert() {
    assert!(false);
}

#[allow(clippy::assertions_on_constants)]
// jonesy: expect panic assert_eq failed
pub fn cause_assert_eq() {
    assert_eq!(1, 2);
}

#[allow(clippy::assertions_on_constants, clippy::eq_op)]
// jonesy: expect panic assert_ne failed
pub fn cause_assert_ne() {
    assert_ne!(1, 1);
}

// jonesy: expect panic debug_assert failed (debug builds only)
pub fn cause_debug_assert() {
    debug_assert!(false);
}

// jonesy: expect panic debug_assert_eq failed (debug builds only)
pub fn cause_debug_assert_eq() {
    debug_assert_eq!(1, 2);
}

#[allow(clippy::eq_op)]
// jonesy: expect panic debug_assert_ne failed (debug builds only)
pub fn cause_debug_assert_ne() {
    debug_assert_ne!(1, 1);
}

// jonesy: expect panic unreachable reached
pub fn cause_unreachable() {
    unreachable!();
}

// jonesy: expect panic unimplemented reached
pub fn cause_unimplemented() {
    unimplemented!();
}

// jonesy: expect panic
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

// jonesy: expect panic index out of bounds
#[allow(clippy::useless_vec)]
pub fn cause_slice_index_oob() {
    let v = vec![1, 2, 3];
    let _ = v[10];
}

// Known limitation: string panic not detected in rlib/staticlib relocation-based analysis (no file path metadata)
pub fn cause_string_index_panic() {
    let s = "hello 世界";
    let _ = &s[0..7]; // panics - cuts through UTF-8 char
}
