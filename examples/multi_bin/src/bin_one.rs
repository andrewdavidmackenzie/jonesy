//! Binary one - all panic types at unique line numbers for testing

fn main() {
    // Dispatch based on argument to ensure all functions are linked
    match std::env::args().nth(1).as_deref() {
        // Library function calls to ensure they're linked and analyzed
        Some("lib_panic") => multi_bin_lib::lib_function(),
        Some("lib_unwrap") => multi_bin_lib::lib_unwrap_none(),
        Some("lib_unwrap_err") => multi_bin_lib::lib_unwrap_err(),
        Some("lib_expect_none") => multi_bin_lib::lib_expect_none(),
        Some("lib_expect_err") => multi_bin_lib::lib_expect_err(),
        Some("lib_unwrap_err_ok") => multi_bin_lib::lib_unwrap_err_on_ok(),
        Some("lib_expect_err_ok") => multi_bin_lib::lib_expect_err_on_ok(),
        Some("lib_assert") => multi_bin_lib::lib_assert(),
        Some("lib_debug_assert") => multi_bin_lib::lib_debug_assert(),
        Some("lib_unreachable") => multi_bin_lib::lib_unreachable(),
        Some("lib_unimplemented") => multi_bin_lib::lib_unimplemented(),
        Some("lib_todo") => multi_bin_lib::lib_todo(),
        Some("lib_div_zero") => multi_bin_lib::lib_divide_by_zero(),
        Some("lib_overflow") => multi_bin_lib::lib_arithmetic_overflow(),
        Some("lib_shift") => multi_bin_lib::lib_shift_overflow(),
        Some("lib_string") => multi_bin_lib::lib_string_index_panic(),
        Some("unwrap") => cause_an_unwrap(),
        Some("unwrap_err") => cause_unwrap_err(),
        Some("expect_none") => cause_expect_none(),
        Some("expect_err") => cause_expect_err(),
        Some("unwrap_err_ok") => cause_unwrap_err_on_ok(),
        Some("expect_err_ok") => cause_expect_err_on_ok(),
        Some("assert") => cause_assert(),
        Some("assert_eq") => cause_assert_eq(),
        Some("assert_ne") => cause_assert_ne(),
        Some("debug_assert") => cause_debug_assert(),
        Some("debug_assert_eq") => cause_debug_assert_eq(),
        Some("debug_assert_ne") => cause_debug_assert_ne(),
        Some("unreachable") => cause_unreachable(),
        Some("unimplemented") => cause_unimplemented(),
        Some("todo") => cause_todo(),
        Some("div_zero") => cause_divide_by_zero(),
        Some("overflow") => cause_arithmetic_overflow(),
        Some("shift") => cause_shift_overflow(),
        Some("slice") => cause_slice_index_oob(),
        Some("string") => cause_string_index_panic(),
        _ => {
            // jonesy: expect panic explicit panic call
            panic!("panic from bin_one");
        }
    }
}

fn cause_an_unwrap() {
    use rand::Rng;
    let mut rng = rand::rng();
    let opt: Option<i32> = if rng.random_bool(0.0) { Some(42) } else { None };
    // jonesy: expect panic unwrap on None
    opt.unwrap();

    // Panic-free alternative: use if let or unwrap_or
    if let Some(value) = opt {
        println!("Got value: {value}");
    }
    let _value = opt.unwrap_or(0);
    let _value = opt.unwrap_or_default();
}

fn cause_unwrap_err() {
    use rand::Rng;
    let mut rng = rand::rng();
    let result: Result<i32, &str> = if rng.random_bool(0.0) {
        Ok(42)
    } else {
        Err("error")
    };
    // jonesy: expect panic unwrap on Err
    result.unwrap();

    // Panic-free alternative: use if let or unwrap_or
    if let Ok(value) = result {
        println!("Got value: {value}");
    }
    let _value = result.unwrap_or(0);
}

#[allow(clippy::unnecessary_literal_unwrap)]
fn cause_expect_none() {
    // jonesy: expect panic expect on None
    let _: () = None.expect("expected a value");

    // Panic-free alternative: use match
    let opt: Option<i32> = None;
    match opt {
        Some(value) => println!("Got value: {value}"),
        None => println!("No value present"),
    }
}

#[allow(clippy::unnecessary_literal_unwrap)]
fn cause_expect_err() {
    // jonesy: expect panic expect on Err
    let _: () = Err("error").expect("expected ok");

    // Panic-free alternative: use match
    let result: Result<i32, &str> = Err("error");
    match result {
        Ok(value) => println!("Got value: {value}"),
        Err(e) => println!("Error: {e}"),
    }
}

#[allow(clippy::unnecessary_literal_unwrap)]
fn cause_unwrap_err_on_ok() {
    // jonesy: expect panic unwrap_err on Ok
    let _: &str = Ok::<i32, &str>(42).unwrap_err();

    // Panic-free alternative: use match
    let result: Result<i32, &str> = Ok(42);
    match result {
        Ok(value) => println!("Got Ok: {value}"),
        Err(e) => println!("Got error: {e}"),
    }
}

#[allow(clippy::unnecessary_literal_unwrap)]
fn cause_expect_err_on_ok() {
    // jonesy: expect panic expect_err on Ok
    let _: &str = Ok::<i32, &str>(42).expect_err("expected an error");

    // Panic-free alternative: use if let
    let result: Result<i32, &str> = Ok(42);
    if let Err(e) = result {
        println!("Got expected error: {e}");
    } else {
        println!("Got Ok, but expected Err");
    }
}

#[allow(clippy::assertions_on_constants)]
fn cause_assert() {
    // jonesy: expect panic assert failed
    assert!(false);

    // Panic-free alternative: use if
    let condition = false;
    if !condition {
        println!("Condition was false");
    }
}

#[allow(clippy::assertions_on_constants)]
fn cause_assert_eq() {
    // jonesy: expect panic assert_eq failed
    assert_eq!(1, 2);

    // Panic-free alternative: use if
    let a = 1;
    let b = 2;
    if a != b {
        println!("Values differ: {a} != {b}");
    } else {
        println!("Values are equal");
    }
}

#[allow(clippy::assertions_on_constants, clippy::eq_op)]
fn cause_assert_ne() {
    // jonesy: expect panic assert_ne failed
    assert_ne!(1, 1);

    // Panic-free alternative: use if
    let a = 1;
    let b = 1;
    if a == b {
        println!("Values are equal: {a} == {b}");
    } else {
        println!("Values differ");
    }
}

fn cause_debug_assert() {
    // jonesy: expect panic assert failed (debug builds only)
    debug_assert!(false);

    // Panic-free alternative: use if
    let condition = false;
    if !condition {
        println!("Debug: condition was false");
    }
}

fn cause_debug_assert_eq() {
    // jonesy: expect panic assert_eq failed (debug builds only)
    debug_assert_eq!(1, 2);

    // Panic-free alternative: use if
    let a = 1;
    let b = 2;
    if a != b {
        println!("Debug: values differ: {a} != {b}");
    } else {
        println!("Debug: values are equal");
    }
}

#[allow(clippy::eq_op)]
fn cause_debug_assert_ne() {
    // jonesy: expect panic assert_ne failed (debug builds only)
    debug_assert_ne!(1, 1);

    // Panic-free alternative: use if
    let a = 1;
    let b = 1;
    if a == b {
        println!("Debug: values are equal: {a} == {b}");
    } else {
        println!("Debug: values differ");
    }
}

fn cause_unreachable() {
    // jonesy: expect panic unreachable reached
    unreachable!();
}

fn cause_unimplemented() {
    // jonesy: expect panic unimplemented reached
    unimplemented!();
}

fn cause_todo() {
    // jonesy: expect panic todo reached
    todo!();
}

#[allow(unconditional_panic)]
fn cause_divide_by_zero() {
    // jonesy: expect panic division by zero
    let _ = 1 / 0;

    // Panic-free alternative: check divisor or use checked_div
    let dividend: i32 = 1;
    let divisor: i32 = 0;
    if divisor != 0 {
        let _result = dividend / divisor;
    } else {
        println!("Cannot divide by zero");
    }
    let _result = dividend.checked_div(divisor); // Returns None
}

#[allow(arithmetic_overflow)]
fn cause_arithmetic_overflow() {
    let x: i32 = i32::MAX;
    // jonesy: expect panic arithmetic overflow (debug builds)
    let _ = x + 1;

    // Panic-free alternatives: use checked, saturating, or wrapping arithmetic
    let _checked = x.checked_add(1); // Returns None on overflow
    let _saturating = x.saturating_add(1); // Returns i32::MAX
    let _wrapping = x.wrapping_add(1); // Wraps to i32::MIN
}

#[allow(arithmetic_overflow)]
fn cause_shift_overflow() {
    // jonesy: expect panic shift overflow
    let _ = 1u32 << 33;

    // Panic-free alternative: validate shift amount or use checked_shl
    let value = 1u32;
    let shift = 33;
    if shift < 32 {
        let _result = value << shift;
    }
    let _result = value.checked_shl(shift); // Returns None
}

#[allow(clippy::useless_vec)]
// Known limitation: slice index detection is platform-specific (see issue #59)
fn cause_slice_index_oob() {
    // jonesy: expect panic capacity overflow from allocation
    let v = vec![1, 2, 3];
    let _ = v[10];

    // Panic-free alternative: use .get() which returns Option
    if let Some(value) = v.get(10) {
        println!("Got value: {value}");
    } else {
        println!("Index out of bounds");
    }
}

fn cause_string_index_panic() {
    let s = "hello 世界";
    // jonesy: expect panic string/slice error
    let _ = &s[0..7]; // panics - cuts through UTF-8 char

    // Panic-free alternative: use .get() which returns Option
    if let Some(slice) = s.get(0..7) {
        println!("Got slice: {slice}");
    } else {
        println!("Invalid string slice boundary");
    }
    // Or use char_indices to find valid boundaries
    for (i, c) in s.char_indices() {
        println!("Char '{c}' starts at byte {i}");
    }
}
