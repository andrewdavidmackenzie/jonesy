//! Binary two - all panic types at unique line numbers for testing
//! (different from bin_one to verify independent analysis)

fn main() {
    // jonesy: expect panic(format)
    println!("bin_two starting");

    // Dispatch based on argument to ensure all functions are linked
    // jonesy: expect panic
    match std::env::args().nth(1).as_deref() {
        // jonesy: expect panic(unwrap)
        Some("unwrap") => unwrap_none(),
        // jonesy: expect panic(unwrap)
        Some("unwrap_err") => unwrap_err(),
        // jonesy: expect panic(expect)
        Some("expect_none") => expect_none(),
        // jonesy: expect panic(expect)
        Some("expect_err") => expect_err(),
        // jonesy: expect panic(unwrap)
        Some("unwrap_err_ok") => unwrap_err_on_ok(),
        // jonesy: expect panic(expect)
        Some("expect_err_ok") => expect_err_on_ok(),
        // jonesy: expect panic(unknown)
        Some("assert") => assert_false(),
        // jonesy: expect panic(assert)
        Some("assert_eq") => assert_eq_fail(),
        // jonesy: expect panic(assert)
        Some("assert_ne") => assert_ne_fail(),
        // jonesy: expect panic(unknown)
        Some("debug_assert") => debug_assert_false(),
        // jonesy: expect panic(assert)
        Some("debug_assert_eq") => debug_assert_eq_fail(),
        // jonesy: expect panic(assert)
        Some("debug_assert_ne") => debug_assert_ne_fail(),
        // jonesy: expect panic(unknown)
        Some("unreachable") => unreachable_code(),
        // jonesy: expect panic(unknown)
        Some("unimplemented") => unimplemented_code(),
        // jonesy: expect panic(unknown)
        Some("todo") => todo_code(),
        // jonesy: expect panic(div_zero)
        Some("div_zero") => divide_by_zero(),
        // jonesy: expect panic(overflow)
        Some("overflow") => arithmetic_overflow(),
        // jonesy: expect panic(shift_overflow)
        Some("shift") => shift_overflow(),
        // jonesy: expect panic
        Some("slice") => slice_index_oob(),
        // jonesy: expect panic(bounds)
        Some("string") => string_index_panic(),
        _ => {
            // jonesy: expect panic explicit panic call
            panic!("panic from bin_two");
        }
    }
}

fn unwrap_none() {
    use rand::Rng;
    let mut rng = rand::rng();
    let opt: Option<i32> = if rng.random_bool(0.0) { Some(42) } else { None };
    // jonesy: expect panic unwrap on None
    opt.unwrap();
}

fn unwrap_err() {
    use rand::Rng;
    let mut rng = rand::rng();
    let result: Result<i32, &str> = if rng.random_bool(0.0) {
        Ok(42)
    } else {
        Err("error")
    };
    // jonesy: expect panic unwrap on Err
    result.unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
fn expect_none() {
    // jonesy: expect panic expect on None
    let _: () = None.expect("expected a value");
}

#[allow(clippy::unnecessary_literal_unwrap)]
fn expect_err() {
    // jonesy: expect panic expect on Err
    let _: () = Err("error").expect("expected ok");
}

#[allow(clippy::unnecessary_literal_unwrap)]
fn unwrap_err_on_ok() {
    // jonesy: expect panic unwrap_err on Ok
    let _: &str = Ok::<i32, &str>(42).unwrap_err();
}

#[allow(clippy::unnecessary_literal_unwrap)]
fn expect_err_on_ok() {
    // jonesy: expect panic expect_err on Ok
    let _: &str = Ok::<i32, &str>(42).expect_err("expected an error");
}

#[allow(clippy::assertions_on_constants)]
fn assert_false() {
    // jonesy: expect panic assert failed
    assert!(false);
}

#[allow(clippy::assertions_on_constants)]
fn assert_eq_fail() {
    // jonesy: expect panic assert_eq failed
    assert_eq!(1, 2);
}

#[allow(clippy::assertions_on_constants, clippy::eq_op)]
fn assert_ne_fail() {
    // jonesy: expect panic assert_ne failed
    assert_ne!(1, 1);
}

fn debug_assert_false() {
    // jonesy: expect panic assert failed (debug builds only)
    debug_assert!(false);
}

fn debug_assert_eq_fail() {
    // jonesy: expect panic assert_eq failed (debug builds only)
    debug_assert_eq!(1, 2);
}

#[allow(clippy::eq_op)]
fn debug_assert_ne_fail() {
    // jonesy: expect panic assert_ne failed (debug builds only)
    debug_assert_ne!(1, 1);
}

fn unreachable_code() {
    // jonesy: expect panic unreachable reached
    unreachable!();
}

fn unimplemented_code() {
    // jonesy: expect panic unimplemented reached
    unimplemented!();
}

fn todo_code() {
    // jonesy: expect panic todo reached
    todo!();
}

#[allow(unconditional_panic)]
fn divide_by_zero() {
    // jonesy: expect panic division by zero
    let _ = 1 / 0;
}

#[allow(arithmetic_overflow)]
fn arithmetic_overflow() {
    let x: i32 = i32::MAX;
    // jonesy: expect panic arithmetic overflow (debug builds)
    let _ = x + 1;
}

#[allow(arithmetic_overflow)]
fn shift_overflow() {
    // jonesy: expect panic shift overflow
    let _ = 1u32 << 33;
}

#[allow(clippy::useless_vec)]
fn slice_index_oob() {
    // jonesy: expect panic
    let v = vec![1, 2, 3];
    // jonesy: expect panic(bounds)
    let _ = v[10];
}

fn string_index_panic() {
    let s = "hello 世界";
    // jonesy: expect panic string/slice error
    let _ = &s[0..7]; // panics - cuts through UTF-8 char
}
