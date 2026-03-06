const PANIC_STR: &str = "panic";

mod module;

/// A library function that can panic
#[unsafe(no_mangle)]
pub fn library_function() {
    if std::env::args().len() > 1 {
        // jones: expect panic -
        panic!("{}", PANIC_STR);
    }

    // jones: expect panic -
    module::cause_a_panic();
}
