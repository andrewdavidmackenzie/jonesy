//! Crate B - library component, with all panic types

/// Library function that can panic
pub fn lib_function() {
    // jonesy: expect panic explicit panic call
    panic!("panic from crate_b library");
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic unwrap on None
pub fn lib_unwrap_none() {
    let _: () = None.unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic unwrap on Err
pub fn lib_unwrap_err() {
    let _: () = Err("error").unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic expect on None
pub fn lib_expect_none() {
    let _: () = None.expect("expected a value");
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic expect on Err
pub fn lib_expect_err() {
    let _: () = Err("error").expect("expected ok");
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic unwrap_err on Ok
pub fn lib_unwrap_err_on_ok() {
    let _: &str = Ok::<i32, &str>(42).unwrap_err();
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic expect_err on Ok
pub fn lib_expect_err_on_ok() {
    let _: &str = Ok::<i32, &str>(42).expect_err("expected an error");
}

#[allow(clippy::assertions_on_constants)]
// jonesy: expect panic assert failed
pub fn lib_assert_false() {
    assert!(false);
}

#[allow(clippy::assertions_on_constants)]
// jonesy: expect panic assert_eq failed
pub fn lib_assert_eq_fail() {
    assert_eq!(1, 2);
}

#[allow(clippy::assertions_on_constants, clippy::eq_op)]
// jonesy: expect panic assert_ne failed
pub fn lib_assert_ne_fail() {
    assert_ne!(1, 1);
}

// jonesy: expect panic debug_assert failed (debug builds only)
pub fn lib_debug_assert_false() {
    debug_assert!(false);
}

// jonesy: expect panic debug_assert_eq failed (debug builds only)
pub fn lib_debug_assert_eq_fail() {
    debug_assert_eq!(1, 2);
}

#[allow(clippy::eq_op)]
// jonesy: expect panic debug_assert_ne failed (debug builds only)
pub fn lib_debug_assert_ne_fail() {
    debug_assert_ne!(1, 1);
}

// jonesy: expect panic unreachable reached
pub fn lib_unreachable_code() {
    unreachable!();
}

// jonesy: expect panic unimplemented reached
pub fn lib_unimplemented_code() {
    unimplemented!();
}

// jonesy: expect panic todo reached
pub fn lib_todo_code() {
    todo!();
}

#[allow(unconditional_panic)]
// jonesy: expect panic division by zero
pub fn lib_divide_by_zero() {
    let _ = 1 / 0;
}

#[allow(arithmetic_overflow)]
pub fn lib_arithmetic_overflow() {
    let x: i32 = i32::MAX;
    // jonesy: expect panic arithmetic overflow (debug builds)
    let _ = x + 1;
}

#[allow(arithmetic_overflow)]
// jonesy: expect panic shift overflow
pub fn lib_shift_overflow() {
    let _ = 1u32 << 33;
}

#[allow(clippy::useless_vec)]
// jonesy: expect panic slice index out of bounds
pub fn lib_slice_index_oob() {
    let v = vec![1, 2, 3];
    let _ = v[10];
}

// jonesy: expect panic string index not on UTF-8 boundary
pub fn lib_string_index_panic() {
    let s = "hello 世界";
    let _ = &s[0..7];
}
