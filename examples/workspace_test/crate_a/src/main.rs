//! Crate A - binary only, with all panic types

fn main() {
    // Dispatch based on argument to ensure all functions are linked
    match std::env::args().nth(1).as_deref() {
        Some("unwrap") => unwrap_none(),
        Some("unwrap_err") => unwrap_err(),
        Some("expect_none") => expect_none(),
        Some("expect_err") => expect_err(),
        Some("unwrap_err_ok") => unwrap_err_on_ok(),
        Some("expect_err_ok") => expect_err_on_ok(),
        Some("assert") => assert_false(),
        Some("assert_eq") => assert_eq_fail(),
        Some("assert_ne") => assert_ne_fail(),
        Some("debug_assert") => debug_assert_false(),
        Some("debug_assert_eq") => debug_assert_eq_fail(),
        Some("debug_assert_ne") => debug_assert_ne_fail(),
        Some("unreachable") => unreachable_code(),
        Some("unimplemented") => unimplemented_code(),
        Some("todo") => todo_code(),
        Some("div_zero") => divide_by_zero(),
        Some("overflow") => arithmetic_overflow(),
        Some("shift") => shift_overflow(),
        Some("slice") => slice_index_oob(),
        Some("string") => string_index_panic(),
        _ => {
            // jonesy: expect panic explicit panic call
            panic!("panic from crate_a");
        }
    }
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic unwrap on None
fn unwrap_none() {
    let _: () = None.unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic unwrap on Err
fn unwrap_err() {
    let _: () = Err("error").unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic expect on None
fn expect_none() {
    let _: () = None.expect("expected a value");
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic expect on Err
fn expect_err() {
    let _: () = Err("error").expect("expected ok");
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic unwrap_err on Ok
fn unwrap_err_on_ok() {
    let _: &str = Ok::<i32, &str>(42).unwrap_err();
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jonesy: expect panic expect_err on Ok
fn expect_err_on_ok() {
    let _: &str = Ok::<i32, &str>(42).expect_err("expected an error");
}

#[allow(clippy::assertions_on_constants)]
// jonesy: expect panic assert failed
fn assert_false() {
    assert!(false);
}

#[allow(clippy::assertions_on_constants)]
// jonesy: expect panic assert_eq failed
fn assert_eq_fail() {
    assert_eq!(1, 2);
}

#[allow(clippy::assertions_on_constants, clippy::eq_op)]
// jonesy: expect panic assert_ne failed
fn assert_ne_fail() {
    assert_ne!(1, 1);
}

// jonesy: expect panic debug_assert failed (debug builds only)
fn debug_assert_false() {
    debug_assert!(false);
}

// jonesy: expect panic debug_assert_eq failed
fn debug_assert_eq_fail() {
    debug_assert_eq!(1, 2);
}

#[allow(clippy::eq_op)]
// jonesy: expect panic debug_assert_ne failed
fn debug_assert_ne_fail() {
    debug_assert_ne!(1, 1);
}

// jonesy: expect panic unreachable reached
fn unreachable_code() {
    unreachable!();
}

// jonesy: expect panic unimplemented reached
fn unimplemented_code() {
    unimplemented!();
}

// jonesy: expect panic todo reached
fn todo_code() {
    todo!();
}

#[allow(unconditional_panic)]
// jonesy: expect panic division by zero
fn divide_by_zero() {
    let _ = 1 / 0;
}

#[allow(arithmetic_overflow)]
fn arithmetic_overflow() {
    let x: i32 = i32::MAX;
    // jonesy: expect panic arithmetic overflow (debug builds)
    let _ = x + 1;
}

#[allow(arithmetic_overflow)]
// jonesy: expect panic shift overflow
fn shift_overflow() {
    let _ = 1u32 << 33;
}

#[allow(clippy::useless_vec)]
// Known limitation: slice index detection is platform-specific (see issue #59)
fn slice_index_oob() {
    let v = vec![1, 2, 3];
    let _ = v[10];
}

fn string_index_panic() {
    let s = "hello 世界";
    // jonesy: expect panic string/slice error
    let _ = &s[0..7];
}
