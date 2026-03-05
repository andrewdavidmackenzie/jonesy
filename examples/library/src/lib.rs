// to test that we only find the reference to panic code
const PANIC_STR: &str = "panic";

mod module;

/// A library function that can panic
pub fn library_function() {
    if std::env::args().len() > 1 {
        panic!("{}", PANIC_STR);
    }
    module::cause_a_panic();
}
