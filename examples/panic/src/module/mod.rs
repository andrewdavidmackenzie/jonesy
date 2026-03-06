pub fn cause_a_panic() {
    // jones: expect panic - explicit panic call
    panic!("panic");
}

// jones: expect panic - unwrap on None
pub fn cause_an_unwrap() {
    None.unwrap()
}
