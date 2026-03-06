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
}
