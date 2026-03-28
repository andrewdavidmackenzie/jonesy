// to test that we only find the reference to panic code and not constants
const PANIC_STR: &str = "panic";

mod module;
mod module2;

fn main() {
    if std::env::args().len() > 1 {
        // jonesy: expect panic This is a potential panic at runtime, depending on args
        panic!("{}", PANIC_STR);
    }

    // jonesy: expect panic This obviously causes a panic
    module::cause_a_panic();

    // jonesy: expect panic This obviously causes a panic
    module::cause_an_unwrap();

    // jonesy: expect panic unwrap on Err
    module::cause_unwrap_err();

    // jonesy: expect panic expect on None
    module::cause_expect_none();

    // jonesy: expect panic expect on Err
    module::cause_expect_err();

    // jonesy: expect panic assert failed
    module::cause_assert();

    // jonesy: expect panic assert failed
    module::cause_debug_assert();

    // jonesy: expect panic unreachable
    module::cause_unreachable();

    // jonesy: expect panic unimplemented
    module::cause_unimplemented();

    // jonesy: expect panic todo
    module::cause_todo();

    // jonesy: expect panic division by zero
    module::cause_divide_by_zero();

    // jonesy: expect panic arithmetic overflow
    module::cause_arithmetic_overflow();

    // jonesy: expect panic shift overflow
    module::cause_shift_overflow();

    // jonesy: expect panic unwrap_err on Ok
    module::cause_unwrap_err_on_ok();

    // jonesy: expect panic expect_err on Ok
    module::cause_expect_err_on_ok();

    // jonesy: expect panic assert_eq! failed
    module::cause_assert_eq();

    // jonesy: expect panic assert_ne! failed
    module::cause_assert_ne();

    // jonesy: expect panic assert_eq! failed
    module::cause_debug_assert_eq();

    // jonesy: expect panic assert_ne! failed
    module::cause_debug_assert_ne();

    // jonesy: expect panic index out of bounds
    module::cause_slice_index_oob();

    // jonesy: expect panic string slice boundary error
    module::cause_string_index_panic();

    // jonesy: expect panic(overflow)
    module::cause_division_overflow();

    // jonesy: expect panic(overflow)
    module::cause_remainder_overflow();

    // jonesy: expect panic(capacity)
    module::cause_capacity_overflow();

    // jonesy: expect panic(invalid_enum)
    module::cause_invalid_enum();

    // jonesy: expect panic(misaligned_ptr)
    module::cause_misaligned_pointer();

    // jonesy: expect panic(key_not_found)
    module::cause_hashmap_index_panic();

    // jonesy: expect panic(capacity)
    module::cause_hashset_capacity_overflow();

    // jonesy: expect panic expect on None
    module2::cause_expect_none();
}
