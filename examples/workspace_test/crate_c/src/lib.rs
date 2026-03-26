//! Crate C - library only (no binary), with all panic types

/// Library function that can panic
pub fn lib_function() {
    // jonesy: expect panic explicit panic call
    panic!("panic from crate_c library");
}

pub fn unwrap_none() {
    use rand::Rng;
    let mut rng = rand::rng();
    let opt: Option<i32> = if rng.random_bool(0.0) { Some(42) } else { None };
    // jonesy: expect panic unwrap on None
    opt.unwrap();
}

pub fn unwrap_err() {
    use rand::Rng;
    let mut rng = rand::rng();
    let result: Result<i32, &str> = if rng.random_bool(0.0) { Ok(42) } else { Err("error") };
    // jonesy: expect panic unwrap on Err
    result.unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn expect_none() {
    // jonesy: expect panic expect on None
    let _: () = None.expect("expected a value");
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn expect_err() {
    // jonesy: expect panic expect on Err
    let _: () = Err("error").expect("expected ok");
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn unwrap_err_on_ok() {
    // jonesy: expect panic unwrap_err on Ok
    let _: &str = Ok::<i32, &str>(42).unwrap_err();
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn expect_err_on_ok() {
    // jonesy: expect panic expect_err on Ok
    let _: &str = Ok::<i32, &str>(42).expect_err("expected an error");
}

#[allow(clippy::assertions_on_constants)]
pub fn assert_false() {
    // jonesy: expect panic assert failed
    assert!(false);
}

#[allow(clippy::assertions_on_constants)]
pub fn assert_eq_fail() {
    // jonesy: expect panic assert_eq failed
    assert_eq!(1, 2);
}

#[allow(clippy::assertions_on_constants, clippy::eq_op)]
pub fn assert_ne_fail() {
    // jonesy: expect panic assert_ne failed
    assert_ne!(1, 1);
}

pub fn debug_assert_false() {
    // jonesy: expect panic debug_assert failed (debug builds only)
    debug_assert!(false);
}

pub fn debug_assert_eq_fail() {
    // jonesy: expect panic debug_assert_eq failed
    debug_assert_eq!(1, 2);
}

#[allow(clippy::eq_op)]
pub fn debug_assert_ne_fail() {
    // jonesy: expect panic debug_assert_ne failed
    debug_assert_ne!(1, 1);
}

pub fn unreachable_code() {
    // jonesy: expect panic unreachable reached
    unreachable!();
}

pub fn unimplemented_code() {
    // jonesy: expect panic unimplemented reached
    unimplemented!();
}

pub fn todo_code() {
    // jonesy: expect panic todo reached
    todo!();
}

#[allow(unconditional_panic)]
pub fn divide_by_zero() {
    // jonesy: expect panic division by zero
    let _ = 1 / 0;
}

#[allow(arithmetic_overflow)]
pub fn arithmetic_overflow() {
    let x: i32 = i32::MAX;
    // jonesy: expect panic arithmetic overflow (debug builds)
    let _ = x + 1;
}

#[allow(arithmetic_overflow)]
pub fn shift_overflow() {
    // jonesy: expect panic shift overflow
    let _ = 1u32 << 33;
}

#[allow(clippy::useless_vec)]
// Known limitation: slice index detection is platform-specific (see issue #59)
pub fn slice_index_oob() {
    let v = vec![1, 2, 3];
    let _ = v[10];
}

pub fn string_index_panic() {
    let s = "hello 世界";
    // jonesy: expect panic string/slice error
    let _ = &s[0..7];
}
