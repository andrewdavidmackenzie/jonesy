const CATCHPHRASES: [&str; 3] = [
    "Don't Panic!",
    "Permission to speak, sir?",
    "They don't like it up 'em!",
];

fn main() {
    for index in 0..5 {
        // How to index an array without a panic
        match CATCHPHRASES.get(index) {
            None => println!("Array index out of range of array"),
            Some(phrase) => println!("{phrase}"),
        }
    }

    // Demonstrate inline allow comments - these panics are intentionally suppressed
    // jonesy:allow(*)
    demonstrate_inline_allows();
}

/// Examples of using inline allow comments to suppress specific panic warnings.
/// These would normally be flagged by jonesy, but are suppressed with comments.
fn demonstrate_inline_allows() {
    // Example 1: Allow unwrap on a known-safe value
    let always_some: Option<i32> = Some(42);
    let value = always_some.unwrap(); // jonesy:allow(unwrap)
    println!("Got value: {value}");

    // Example 2: Allow expect with a descriptive message
    let config = std::env::var("PATH").expect("PATH must be set"); // jonesy:allow(expect)
    println!("PATH length: {}", config.len());

    // Example 3: Allow unwrap and bounds check
    let data: Result<Vec<u8>, &str> = Ok(vec![1, 2, 3]);
    let bytes = data.unwrap(); // jonesy:allow(unwrap)
    println!("First byte: {}", bytes[0]); // jonesy:allow(bounds)

    // Example 4: Allow panic in a known code path
    // jonesy:allow(capacity)
    if std::env::args().len() > 100 {
        panic!("Too many arguments!"); // jonesy:allow(panic, *)
    }
}
