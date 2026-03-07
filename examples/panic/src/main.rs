// to test that we only find the reference to panic code and not constants
const PANIC_STR: &str = "panic";

mod module;

fn main() {
    if std::env::args().len() > 1 {
        // jones: expect panic - This is a potential panic at runtime, depending on args
        panic!("{}", PANIC_STR);
    }

    // jones: expect panic - This obviously causes a panic
    module::cause_a_panic();

    // jones: expect panic - This obviously causes a panic
    module::cause_an_unwrap();

    // jones: expect panic - unwrap on Err
    module::cause_unwrap_err();

    // jones: expect panic - assert failed
    module::cause_assert();

    // jones: expect panic - debug_assert failed
    module::cause_debug_assert();

    // jones: expect panic - unreachable
    module::cause_unreachable();

    // jones: expect panic - unimplemented
    module::cause_unimplemented();

    // jones: expect panic - todo
    module::cause_todo();

    // jones: expect panic - division by zero
    module::cause_divide_by_zero();

    // jones: expect panic - arithmetic overflow
    module::cause_arithmetic_overflow();

    // jones: expect panic - shift overflow
    module::cause_shift_overflow();
}
