pub fn cause_a_panic() {
    // jonesy: expect panic explicit panic call
    panic!("panic");
}

pub fn cause_an_unwrap() {
    use rand::Rng;
    let mut rng = rand::rng();
    let opt: Option<i32> = if rng.random_bool(0.0) { Some(42) } else { None };
    // jonesy: expect panic unwrap on None
    opt.unwrap();
}

pub fn cause_unwrap_err() {
    use rand::Rng;
    let mut rng = rand::rng();
    let result: Result<i32, &str> = if rng.random_bool(0.0) { Ok(42) } else { Err("error") };
    // jonesy: expect panic unwrap on Err
    result.unwrap();
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
    // jonesy: expect panic debug_assert failed (debug builds only)
    debug_assert!(false);
}

pub fn cause_debug_assert_eq() {
    // jonesy: expect panic debug_assert_eq failed (debug builds only)
    debug_assert_eq!(1, 2);
}

#[allow(clippy::eq_op)]
pub fn cause_debug_assert_ne() {
    // jonesy: expect panic debug_assert_ne failed (debug builds only)
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
    // jonesy: expect panic capacity overflow from allocation
    let v = vec![1, 2, 3];
    // jonesy: expect panic index out of bounds
    let _ = v[10];
}

// Known limitation: string panic not detected in rlib/staticlib relocation-based analysis (no file path metadata)
pub fn cause_string_index_panic() {
    let s = "hello 世界";
    let _ = &s[0..7]; // panics - cuts through UTF-8 char
}
