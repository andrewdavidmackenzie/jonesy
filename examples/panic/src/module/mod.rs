pub fn cause_a_panic() {
    // jonesy: expect panic explicit panic call
    panic!("panic");
    // No panic-free alternative: explicit panic is intentional
}

pub fn cause_an_unwrap() {
    use rand::Rng;
    let mut rng = rand::rng();
    let opt: Option<i32> = if rng.random_bool(0.0) { Some(42) } else { None };
    // jonesy: expect panic unwrap on None
    opt.unwrap();

    // Panic-free alternative: use if let, match, or unwrap_or
    if let Some(value) = opt {
        println!("Got value: {value}");
    }
    let _value = opt.unwrap_or(0);
    let _value = opt.unwrap_or_default();
}

pub fn cause_unwrap_err() {
    use rand::Rng;
    let mut rng = rand::rng();
    let result: Result<i32, &str> = if rng.random_bool(0.0) { Ok(42) } else { Err("error") };
    // jonesy: expect panic unwrap on Err
    result.unwrap();

    // Panic-free alternative: use if let, match, or the ? operator
    if let Ok(value) = result {
        println!("Got value: {value}");
    }
    let _value = result.unwrap_or(0);
    let _value = result.unwrap_or_default();
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn cause_expect_none() {
    // jonesy: expect panic expect on None
    let _: () = None.expect("expected a value");

    // Panic-free alternative: use if let, match, or unwrap_or
    let opt: Option<i32> = None;
    match opt {
        Some(value) => println!("Got value: {value}"),
        None => println!("No value present"),
    }
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn cause_expect_err() {
    // jonesy: expect panic expect on Err
    let _: () = Err("error").expect("expected ok");

    // Panic-free alternative: use match or the ? operator in functions returning Result
    let result: Result<i32, &str> = Err("error");
    match result {
        Ok(value) => println!("Got value: {value}"),
        Err(e) => println!("Error occurred: {e}"),
    }
}

#[allow(clippy::assertions_on_constants)]
pub fn cause_assert() {
    // jonesy: expect panic assert failed
    assert!(false);

    // Panic-free alternative: use if/else to handle the condition
    let condition = false;
    if !condition {
        println!("Condition was false, handling gracefully");
    }
}

pub fn cause_debug_assert() {
    // jonesy: expect panic debug_assert failed (debug builds only)
    debug_assert!(false);

    // Panic-free alternative: use if to check condition (same as assert)
    let condition = false;
    if !condition {
        println!("Debug: condition was false");
    }
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
    // jonesy: expect panic todo reached
    todo!();
}

#[allow(unconditional_panic)]
pub fn cause_divide_by_zero() {
    // jonesy: expect panic division by zero
    let _ = 1 / 0;

    // Panic-free alternative: check divisor before dividing
    let dividend: i32 = 1;
    let divisor: i32 = 0;
    if divisor != 0 {
        let _result = dividend / divisor;
    } else {
        println!("Cannot divide by zero");
    }
    // Or use checked_div which returns None on division by zero
    let _result = dividend.checked_div(divisor);
}

#[allow(arithmetic_overflow)]
pub fn cause_arithmetic_overflow() {
    let x: i32 = i32::MAX;
    // jonesy: expect panic arithmetic overflow (debug builds)
    let _ = x + 1;

    // Panic-free alternatives: use checked, saturating, or wrapping arithmetic
    let _checked = x.checked_add(1); // Returns None on overflow
    let _saturating = x.saturating_add(1); // Returns i32::MAX on overflow
    let _wrapping = x.wrapping_add(1); // Wraps around to i32::MIN
}

#[allow(arithmetic_overflow)]
pub fn cause_shift_overflow() {
    // jonesy: expect panic shift overflow
    let _ = 1u32 << 33;

    // Panic-free alternative: validate shift amount or use checked_shl
    let value = 1u32;
    let shift = 33;
    if shift < 32 {
        let _result = value << shift;
    }
    // Or use checked_shl which returns None on overflow
    let _result = value.checked_shl(shift);
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn cause_unwrap_err_on_ok() {
    // jonesy: expect panic unwrap_err on Ok
    let _: &str = Ok::<i32, &str>(42).unwrap_err();

    // Panic-free alternative: use match to handle both cases
    let result: Result<i32, &str> = Ok(42);
    match result {
        Ok(value) => println!("Got Ok value: {value}"),
        Err(e) => println!("Got error: {e}"),
    }
}

#[allow(clippy::unnecessary_literal_unwrap)]
pub fn cause_expect_err_on_ok() {
    // jonesy: expect panic expect_err on Ok
    let _: &str = Ok::<i32, &str>(42).expect_err("expected an error");

    // Panic-free alternative: use if let or match
    let result: Result<i32, &str> = Ok(42);
    if let Err(e) = result {
        println!("Got expected error: {e}");
    } else {
        println!("Got Ok, but expected Err");
    }
}

#[allow(clippy::assertions_on_constants)]
pub fn cause_assert_eq() {
    // jonesy: expect panic assert_eq! failed
    assert_eq!(1, 2);

    // Panic-free alternative: use if to check equality
    let a = 1;
    let b = 2;
    if a == b {
        println!("Values are equal");
    } else {
        println!("Values differ: {a} != {b}");
    }
}

#[allow(clippy::assertions_on_constants, clippy::eq_op)]
pub fn cause_assert_ne() {
    // jonesy: expect panic assert_ne! failed
    assert_ne!(1, 1);

    // Panic-free alternative: use if to check inequality
    let a = 1;
    let b = 1;
    if a != b {
        println!("Values differ");
    } else {
        println!("Values are equal: {a} == {b}");
    }
}

pub fn cause_debug_assert_eq() {
    // jonesy: expect panic debug_assert_eq! failed
    debug_assert_eq!(1, 2);

    // Panic-free alternative: use if to check equality (same as assert_eq)
    let a = 1;
    let b = 2;
    if a != b {
        println!("Debug: values differ: {a} != {b}");
    } else {
        println!("Debug: values are equal");
    }
}

#[allow(clippy::eq_op)]
pub fn cause_debug_assert_ne() {
    // jonesy: expect panic debug_assert_ne! failed
    debug_assert_ne!(1, 1);

    // Panic-free alternative: use if to check inequality (same as assert_ne)
    let a = 1;
    let b = 1;
    if a == b {
        println!("Debug: values are equal: {a} == {b}");
    } else {
        println!("Debug: values differ");
    }
}

#[allow(clippy::useless_vec)]
pub fn cause_slice_index_oob() {
    // jonesy: expect panic capacity overflow from allocation
    let v = vec![1, 2, 3];
    // jonesy: expect panic index out of bounds
    let _ = v[10];

    // Panic-free alternative: use .get() which returns Option
    if let Some(value) = v.get(10) {
        println!("Got value: {value}");
    } else {
        println!("Index out of bounds");
    }
}

pub fn cause_string_index_panic() {
    let s = "hello 世界";
    // jonesy: expect panic(str_slice)
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

#[allow(arithmetic_overflow, unconditional_panic)]
pub fn cause_division_overflow() {
    // Division overflow: i32::MIN / -1 overflows because the result
    // would be i32::MAX + 1 which doesn't fit in i32
    // jonesy: expect panic(overflow)
    let _ = i32::MIN / -1;

    // Panic-free alternative: use checked_div
    let _result = i32::MIN.checked_div(-1); // Returns None
}

#[allow(arithmetic_overflow, unconditional_panic, clippy::modulo_one)]
pub fn cause_remainder_overflow() {
    // Remainder overflow: i32::MIN % -1 can overflow on some platforms
    // jonesy: expect panic(overflow)
    let _ = i32::MIN % -1;

    // Panic-free alternative: use checked_rem
    let _result = i32::MIN.checked_rem(-1); // Returns None
}

pub fn cause_capacity_overflow() {
    // Attempting to allocate an impossibly large vector
    // jonesy: expect panic(capacity)
    let _v: Vec<u8> = Vec::with_capacity(usize::MAX);

    // Panic-free alternative: use try_reserve
    // (not shown here to avoid additional panic points from println!)
}

#[repr(u8)]
#[derive(Debug)]
#[allow(dead_code)]
pub enum SmallEnum {
    A = 0,
    B = 1,
}

pub fn cause_invalid_enum() {
    // SAFETY: This is intentionally unsafe and will panic
    // when Rust checks the enum discriminant
    // jonesy: expect panic(invalid_enum)
    let _invalid: SmallEnum = unsafe { std::mem::transmute::<u8, SmallEnum>(42) };

    // Panic-free alternative: validate before transmute
    // or use TryFrom trait for safe conversion
}

pub fn cause_misaligned_pointer() {
    let data: [u8; 8] = [0; 8];
    // Create a misaligned pointer to u32 (requires 4-byte alignment)
    let ptr = &data[1] as *const u8 as *const u32;
    // SAFETY: This is intentionally unsafe - dereferencing misaligned pointer
    // jonesy: expect panic(misaligned_ptr)
    let _ = unsafe { *ptr };

    // Panic-free alternative: use read_unaligned for potentially misaligned data
    // let value = unsafe { ptr.read_unaligned() };
}
