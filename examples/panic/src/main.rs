// to test that we only find the reference to panic code and not constants
const PANIC_STR: &str = "panic";

mod module;

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

    // jonesy: expect panic debug_assert failed
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
}
