// This example tests that jonesy correctly reports the function name
// for inlined functions. When `run()` is inlined into `main()`, the
// panic point should still report `run` as the function name, not `main`.

// jonesy: expect panic
#[allow(clippy::unnecessary_literal_unwrap)]
#[inline(always)]
fn run() {
    let x: Option<i32> = None;
    x.unwrap();
}

// jonesy: expect panic
#[allow(clippy::unnecessary_literal_unwrap)]
#[inline(always)]
fn helper() {
    let result: Result<i32, &str> = Err("error");
    result.expect("should not fail");
}

fn main() {
    // These calls will be inlined, but jonesy should report
    // the correct function names (run, helper) not main
    run();
    helper();
}
