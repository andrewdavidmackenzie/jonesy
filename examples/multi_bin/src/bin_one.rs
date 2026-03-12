//! Binary one - all panic types at unique line numbers for testing

fn main() {
    // jonesy: expect panic explicit panic call
    panic!("panic from bin_one");
}

#[allow(clippy::unnecessary_literal_unwrap)]
#[allow(dead_code)]
// jonesy: expect panic unwrap on None
fn cause_an_unwrap() {
    let _: () = None.unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
#[allow(dead_code)]
// jonesy: expect panic unwrap on Err
fn cause_unwrap_err() {
    let _: () = Err("error").unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
#[allow(dead_code)]
// jonesy: expect panic expect on None
fn cause_expect_none() {
    let _: () = None.expect("expected a value");
}

#[allow(clippy::unnecessary_literal_unwrap)]
#[allow(dead_code)]
// jonesy: expect panic expect on Err
fn cause_expect_err() {
    let _: () = Err("error").expect("expected ok");
}

#[allow(clippy::unnecessary_literal_unwrap)]
#[allow(dead_code)]
// jonesy: expect panic unwrap_err on Ok
fn cause_unwrap_err_on_ok() {
    let _: &str = Ok::<i32, &str>(42).unwrap_err();
}

#[allow(clippy::unnecessary_literal_unwrap)]
#[allow(dead_code)]
// jonesy: expect panic expect_err on Ok
fn cause_expect_err_on_ok() {
    let _: &str = Ok::<i32, &str>(42).expect_err("expected an error");
}

#[allow(clippy::assertions_on_constants)]
#[allow(dead_code)]
// jonesy: expect panic assert failed
fn cause_assert() {
    assert!(false);
}

#[allow(clippy::assertions_on_constants)]
#[allow(dead_code)]
// jonesy: expect panic assert_eq failed
fn cause_assert_eq() {
    assert_eq!(1, 2);
}

#[allow(clippy::assertions_on_constants, clippy::eq_op)]
#[allow(dead_code)]
// jonesy: expect panic assert_ne failed
fn cause_assert_ne() {
    assert_ne!(1, 1);
}

#[allow(dead_code)]
// jonesy: expect panic debug_assert failed (debug builds only)
fn cause_debug_assert() {
    debug_assert!(false);
}

#[allow(dead_code)]
// jonesy: expect panic debug_assert_eq failed (debug builds only)
fn cause_debug_assert_eq() {
    debug_assert_eq!(1, 2);
}

#[allow(dead_code, clippy::eq_op)]
// jonesy: expect panic debug_assert_ne failed (debug builds only)
fn cause_debug_assert_ne() {
    debug_assert_ne!(1, 1);
}

#[allow(dead_code)]
// jonesy: expect panic unreachable reached
fn cause_unreachable() {
    unreachable!();
}

#[allow(dead_code)]
// jonesy: expect panic unimplemented reached
fn cause_unimplemented() {
    unimplemented!();
}

#[allow(dead_code)]
// jonesy: expect panic todo reached
fn cause_todo() {
    todo!();
}

#[allow(unconditional_panic)]
#[allow(dead_code)]
// jonesy: expect panic division by zero
fn cause_divide_by_zero() {
    let _ = 1 / 0;
}

#[allow(arithmetic_overflow)]
#[allow(dead_code)]
fn cause_arithmetic_overflow() {
    let x: i32 = i32::MAX;
    // jonesy: expect panic arithmetic overflow (debug builds)
    let _ = x + 1;
}

#[allow(arithmetic_overflow)]
#[allow(dead_code)]
// jonesy: expect panic shift overflow
fn cause_shift_overflow() {
    let _ = 1u32 << 33;
}

#[allow(dead_code, clippy::useless_vec)]
// jonesy: expect panic slice index out of bounds
fn cause_slice_index_oob() {
    let v = vec![1, 2, 3];
    let _ = v[10];
}

#[allow(dead_code)]
// jonesy: expect panic string index not on UTF-8 boundary
fn cause_string_index_panic() {
    let s = "hello 世界";
    let _ = &s[0..7]; // panics - cuts through UTF-8 char
}
