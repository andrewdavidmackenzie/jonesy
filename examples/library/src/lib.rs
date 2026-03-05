// to test that we only find the reference to panic coe
const PANIC_STR: &str = "panic";

mod module;

pub fn library_function() {
    if std::env::args().len() > 1 {
        panic!("{}", PANIC_STR);
    }
    module::cause_a_panic();
}
