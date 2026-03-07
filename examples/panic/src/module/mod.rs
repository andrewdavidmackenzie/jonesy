pub fn cause_a_panic() {
    // jones: expect panic - explicit panic call
    panic!("panic");
}

#[allow(clippy::unnecessary_literal_unwrap)]
// jones: expect panic - unwrap on None
pub fn cause_an_unwrap() {
    let _: () = None.unwrap();
}
