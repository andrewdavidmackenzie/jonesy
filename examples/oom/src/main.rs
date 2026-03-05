fn sub_foo() -> usize {
    63
}

fn foo() -> usize {
    sub_foo()
}

fn main() {
    vec![0; 1 << foo()];
}
