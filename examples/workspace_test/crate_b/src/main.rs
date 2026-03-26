//! Crate B - binary component, with all panic types
//! Also calls crate_c and crate_b_lib functions to ensure they're linked

fn main() {
    // Dispatch based on argument to ensure all functions are linked
    match std::env::args().nth(1).as_deref() {
        // Call crate_c library functions (jonesy reports panic at call site)
        // jonesy: expect panic crate_c explicit panic
        Some("c_panic") => crate_c::lib_function(),
        // jonesy: expect panic crate_c unwrap on None
        Some("c_unwrap") => crate_c::unwrap_none(),
        // jonesy: expect panic crate_c unwrap on Err
        Some("c_unwrap_err") => crate_c::unwrap_err(),
        // jonesy: expect panic crate_c expect on None
        Some("c_expect_none") => crate_c::expect_none(),
        // jonesy: expect panic crate_c expect on Err
        Some("c_expect_err") => crate_c::expect_err(),
        // jonesy: expect panic crate_c unwrap_err on Ok
        Some("c_unwrap_err_ok") => crate_c::unwrap_err_on_ok(),
        // jonesy: expect panic crate_c expect_err on Ok
        Some("c_expect_err_ok") => crate_c::expect_err_on_ok(),
        // jonesy: expect panic crate_c assert failed
        Some("c_assert") => crate_c::assert_false(),
        // jonesy: expect panic crate_c debug_assert failed
        Some("c_debug_assert") => crate_c::debug_assert_false(),
        // jonesy: expect panic crate_c unreachable
        Some("c_unreachable") => crate_c::unreachable_code(),
        // jonesy: expect panic crate_c unimplemented
        Some("c_unimplemented") => crate_c::unimplemented_code(),
        // jonesy: expect panic crate_c todo
        Some("c_todo") => crate_c::todo_code(),
        // jonesy: expect panic crate_c division by zero
        Some("c_div_zero") => crate_c::divide_by_zero(),
        // jonesy: expect panic crate_c arithmetic overflow
        Some("c_overflow") => crate_c::arithmetic_overflow(),
        // jonesy: expect panic crate_c shift overflow
        Some("c_shift") => crate_c::shift_overflow(),
        // jonesy: expect panic crate_c string/slice error
        Some("c_string") => crate_c::string_index_panic(),

        // Call crate_b_lib functions (jonesy reports panic at call site)
        // jonesy: expect panic crate_b_lib explicit panic
        Some("b_lib_panic") => crate_b_lib::lib_function(),
        // jonesy: expect panic crate_b_lib unwrap on None
        Some("b_lib_unwrap") => crate_b_lib::lib_unwrap_none(),
        // jonesy: expect panic crate_b_lib unwrap on Err
        Some("b_lib_unwrap_err") => crate_b_lib::lib_unwrap_err(),
        // jonesy: expect panic crate_b_lib expect on None
        Some("b_lib_expect_none") => crate_b_lib::lib_expect_none(),
        // jonesy: expect panic crate_b_lib expect on Err
        Some("b_lib_expect_err") => crate_b_lib::lib_expect_err(),
        // jonesy: expect panic crate_b_lib unwrap_err on Ok
        Some("b_lib_unwrap_err_ok") => crate_b_lib::lib_unwrap_err_on_ok(),
        // jonesy: expect panic crate_b_lib expect_err on Ok
        Some("b_lib_expect_err_ok") => crate_b_lib::lib_expect_err_on_ok(),
        // jonesy: expect panic crate_b_lib assert failed
        Some("b_lib_assert") => crate_b_lib::lib_assert_false(),
        // jonesy: expect panic crate_b_lib debug_assert failed
        Some("b_lib_debug_assert") => crate_b_lib::lib_debug_assert_false(),
        // jonesy: expect panic crate_b_lib unreachable
        Some("b_lib_unreachable") => crate_b_lib::lib_unreachable_code(),
        // jonesy: expect panic crate_b_lib unimplemented
        Some("b_lib_unimplemented") => crate_b_lib::lib_unimplemented_code(),
        // jonesy: expect panic crate_b_lib todo
        Some("b_lib_todo") => crate_b_lib::lib_todo_code(),
        // jonesy: expect panic crate_b_lib division by zero
        Some("b_lib_div_zero") => crate_b_lib::lib_divide_by_zero(),
        // jonesy: expect panic crate_b_lib arithmetic overflow
        Some("b_lib_overflow") => crate_b_lib::lib_arithmetic_overflow(),
        // jonesy: expect panic crate_b_lib shift overflow
        Some("b_lib_shift") => crate_b_lib::lib_shift_overflow(),
        // jonesy: expect panic crate_b_lib string/slice error
        Some("b_lib_string") => crate_b_lib::lib_string_index_panic(),

        // Binary's own panic functions
        Some("unwrap") => bin_unwrap_none(),
        Some("unwrap_err") => bin_unwrap_err(),
        Some("expect_none") => bin_expect_none(),
        Some("expect_err") => bin_expect_err(),
        Some("unwrap_err_ok") => bin_unwrap_err_on_ok(),
        Some("expect_err_ok") => bin_expect_err_on_ok(),
        Some("assert") => bin_assert_false(),
        Some("assert_eq") => bin_assert_eq_fail(),
        Some("assert_ne") => bin_assert_ne_fail(),
        Some("debug_assert") => bin_debug_assert_false(),
        Some("debug_assert_eq") => bin_debug_assert_eq_fail(),
        Some("debug_assert_ne") => bin_debug_assert_ne_fail(),
        Some("unreachable") => bin_unreachable_code(),
        Some("unimplemented") => bin_unimplemented_code(),
        Some("todo") => bin_todo_code(),
        Some("div_zero") => bin_divide_by_zero(),
        Some("overflow") => bin_arithmetic_overflow(),
        Some("shift") => bin_shift_overflow(),
        Some("slice") => bin_slice_index_oob(),
        Some("string") => bin_string_index_panic(),
        _ => {
            // jonesy: expect panic explicit panic call
            panic!("panic from crate_b binary");
        }
    }
}

fn bin_unwrap_none() {
    use rand::Rng;
    let mut rng = rand::rng();
    let opt: Option<i32> = if rng.random_bool(0.0) { Some(42) } else { None };
    // jonesy: expect panic unwrap on None
    opt.unwrap();
}

fn bin_unwrap_err() {
    use rand::Rng;
    let mut rng = rand::rng();
    let result: Result<i32, &str> = if rng.random_bool(0.0) { Ok(42) } else { Err("error") };
    // jonesy: expect panic unwrap on Err
    result.unwrap();
}

#[allow(clippy::unnecessary_literal_unwrap)]
fn bin_expect_none() {
    // jonesy: expect panic expect on None
    let _: () = None.expect("expected a value");
}

#[allow(clippy::unnecessary_literal_unwrap)]
fn bin_expect_err() {
    // jonesy: expect panic expect on Err
    let _: () = Err("error").expect("expected ok");
}

#[allow(clippy::unnecessary_literal_unwrap)]
fn bin_unwrap_err_on_ok() {
    // jonesy: expect panic unwrap_err on Ok
    let _: &str = Ok::<i32, &str>(42).unwrap_err();
}

#[allow(clippy::unnecessary_literal_unwrap)]
fn bin_expect_err_on_ok() {
    // jonesy: expect panic expect_err on Ok
    let _: &str = Ok::<i32, &str>(42).expect_err("expected an error");
}

#[allow(clippy::assertions_on_constants)]
fn bin_assert_false() {
    // jonesy: expect panic assert failed
    assert!(false);
}

#[allow(clippy::assertions_on_constants)]
fn bin_assert_eq_fail() {
    // jonesy: expect panic assert_eq failed
    assert_eq!(1, 2);
}

#[allow(clippy::assertions_on_constants, clippy::eq_op)]
fn bin_assert_ne_fail() {
    // jonesy: expect panic assert_ne failed
    assert_ne!(1, 1);
}

fn bin_debug_assert_false() {
    // jonesy: expect panic debug_assert failed (debug builds only)
    debug_assert!(false);
}

fn bin_debug_assert_eq_fail() {
    // jonesy: expect panic debug_assert_eq failed
    debug_assert_eq!(1, 2);
}

#[allow(clippy::eq_op)]
fn bin_debug_assert_ne_fail() {
    // jonesy: expect panic debug_assert_ne failed
    debug_assert_ne!(1, 1);
}

fn bin_unreachable_code() {
    // jonesy: expect panic unreachable reached
    unreachable!();
}

fn bin_unimplemented_code() {
    // jonesy: expect panic unimplemented reached
    unimplemented!();
}

fn bin_todo_code() {
    // jonesy: expect panic todo reached
    todo!();
}

#[allow(unconditional_panic)]
fn bin_divide_by_zero() {
    // jonesy: expect panic division by zero
    let _ = 1 / 0;
}

#[allow(arithmetic_overflow)]
fn bin_arithmetic_overflow() {
    let x: i32 = i32::MAX;
    // jonesy: expect panic arithmetic overflow (debug builds)
    let _ = x + 1;
}

#[allow(arithmetic_overflow)]
fn bin_shift_overflow() {
    // jonesy: expect panic shift overflow
    let _ = 1u32 << 33;
}

#[allow(clippy::useless_vec)]
// Known limitation: slice index detection is platform-specific (see issue #59)
fn bin_slice_index_oob() {
    let v = vec![1, 2, 3];
    let _ = v[10];
}

fn bin_string_index_panic() {
    let s = "hello 世界";
    // jonesy: expect panic string/slice error
    let _ = &s[0..7];
}
