fn sub_foo() -> usize {
    63
}

fn foo() -> usize {
    sub_foo()
}

fn main() {
    // jones: expect panic - allocation too large, will panic on OOM
    vec![0; 1 << foo()];
}
