// This example tests that jonesy correctly reports the function name
// for inlined functions. When `run()` is inlined into `main()`, the
// panic point should still report `run` as the function name, not `main`.

#[inline(always)]
fn run() {
    use rand::Rng;
    let mut rng = rand::rng();
    let x: Option<i32> = if rng.random_bool(0.0) { Some(42) } else { None };
    // jonesy: expect panic
    x.unwrap();
}

#[inline(always)]
fn helper() {
    use rand::Rng;
    let mut rng = rand::rng();
    let result: Result<i32, &str> = if rng.random_bool(0.0) {
        Ok(42)
    } else {
        Err("error")
    };
    // jonesy: expect panic
    result.expect("should not fail");
}

fn main() {
    // These calls will be inlined, but jonesy should report
    // the correct function names (run, helper) not main
    run();
    helper();
}
