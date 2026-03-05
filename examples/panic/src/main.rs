// to test that we only find the reference to panic coe
const PANIC_STR: &str = "panic";

fn main() {
    panic!("{}", PANIC_STR);
}
