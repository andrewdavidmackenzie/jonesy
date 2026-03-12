//! Binary two - all panic types at unique line numbers for testing
//! (different from bin_one to verify independent analysis)

fn main() {
    println!("bin_two starting");

    // jonesy: expect panic explicit panic call
    panic!("panic from bin_two");
}

#[allow(clippy::unnecessary_literal_unwrap)]
#[allow(dead_code)]
// jonesy: expect panic unwrap on None
fn unwrap_none() {
    let _: () = None.unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
#[allow(dead_code)]
// jonesy: expect panic unwrap on Err
fn unwrap_err() {
    let _: () = Err("error").unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
#[allow(dead_code)]
// jonesy: expect panic expect on None
fn expect_none() {
    let _: () = None.expect("expected a value");
}

#[allow(clippy::unnecessary_literal_unwrap)]
#[allow(dead_code)]
// jonesy: expect panic expect on Err
fn expect_err() {
    let _: () = Err("error").expect("expected ok");
}

#[allow(clippy::unnecessary_literal_unwrap)]
#[allow(dead_code)]
// jonesy: expect panic unwrap_err on Ok
fn unwrap_err_on_ok() {
    let _: &str = Ok::<i32, &str>(42).unwrap_err();
}

#[allow(clippy::unnecessary_literal_unwrap)]
#[allow(dead_code)]
// jonesy: expect panic expect_err on Ok
fn expect_err_on_ok() {
    let _: &str = Ok::<i32, &str>(42).expect_err("expected an error");
}

#[allow(clippy::assertions_on_constants)]
#[allow(dead_code)]
// jonesy: expect panic assert failed
fn assert_false() {
    assert!(false);
}

#[allow(clippy::assertions_on_constants)]
#[allow(dead_code)]
// jonesy: expect panic assert_eq failed
fn assert_eq_fail() {
    assert_eq!(1, 2);
}

#[allow(clippy::assertions_on_constants, clippy::eq_op)]
#[allow(dead_code)]
// jonesy: expect panic assert_ne failed
fn assert_ne_fail() {
    assert_ne!(1, 1);
}

#[allow(dead_code)]
// jonesy: expect panic debug_assert failed (debug builds only)
fn debug_assert_false() {
    debug_assert!(false);
}

#[allow(dead_code)]
// jonesy: expect panic debug_assert_eq failed (debug builds only)
fn debug_assert_eq_fail() {
    debug_assert_eq!(1, 2);
}

#[allow(dead_code, clippy::eq_op)]
// jonesy: expect panic debug_assert_ne failed (debug builds only)
fn debug_assert_ne_fail() {
    debug_assert_ne!(1, 1);
}

#[allow(dead_code)]
// jonesy: expect panic unreachable reached
fn unreachable_code() {
    unreachable!();
}

#[allow(dead_code)]
// jonesy: expect panic unimplemented reached
fn unimplemented_code() {
    unimplemented!();
}

#[allow(dead_code)]
// jonesy: expect panic todo reached
fn todo_code() {
    todo!();
}

#[allow(unconditional_panic)]
#[allow(dead_code)]
// jonesy: expect panic division by zero
fn divide_by_zero() {
    let _ = 1 / 0;
}

#[allow(arithmetic_overflow)]
#[allow(dead_code)]
fn arithmetic_overflow() {
    let x: i32 = i32::MAX;
    // jonesy: expect panic arithmetic overflow (debug builds)
    let _ = x + 1;
}

#[allow(arithmetic_overflow)]
#[allow(dead_code)]
// jonesy: expect panic shift overflow
fn shift_overflow() {
    let _ = 1u32 << 33;
}

#[allow(dead_code, clippy::useless_vec)]
// jonesy: expect panic slice index out of bounds
fn slice_index_oob() {
    let v = vec![1, 2, 3];
    let _ = v[10];
}

#[allow(dead_code)]
// jonesy: expect panic string index not on UTF-8 boundary
fn string_index_panic() {
    let s = "hello 世界";
    let _ = &s[0..7]; // panics - cuts through UTF-8 char
}
