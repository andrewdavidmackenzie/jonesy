const CATCHPHRASES: [&str; 3] = [
    "Don't Panic!",
    "Permission to speak, sir?",
    "They don't like it up 'em!",
];

fn main() {
    for index in 0..5 {
        // How to index an array without a panic
        match CATCHPHRASES.get(index) {
            None => println!("Array index out of range of array"), // jonesy:allow(format)
            Some(phrase) => println!("{phrase}"),                  // jonesy:allow(format)
        }
    }

    // Demonstrate inline allow comments - these panics are intentionally suppressed
    // jonesy:allow(*)
    demonstrate_inline_allows();
}

/// Examples of using inline allow comments to suppress specific panic warnings.
/// These would normally be flagged by jonesy, but are suppressed with comments.
#[allow(clippy::unnecessary_literal_unwrap)]
fn demonstrate_inline_allows() {
    // Allow `unwrap` on a known-safe value
    let always_some: Option<i32> = Some(42);
    // jonesy:allow(unwrap)
    let value = always_some.unwrap();

    // Allow `format` in `println!()`
    // jonesy:allow(format)
    println!("Got value: {value}");

    // Allow `expect` with a descriptive message, and `format` for the message
    // jonesy:allow(expect,format)
    let config = std::env::var("PATH").expect("PATH must be set");

    // Allow `format` in `println!()`
    // jonesy:allow(format)
    println!("PATH length: {}", config.len());

    // Allow `oom` on constructing a Vec
    // jonesy:allow(oom)
    let data: Result<Vec<u8>, &str> = Ok(vec![1, 2, 3]);

    // Allow an explicit `unwrap`
    // jonesy:allow(unwrap)
    let bytes = data.unwrap();

    // Allow `format` in `println!()`
    // jonesy:allow(bounds,format)
    println!("First byte: {}", bytes[0]);

    // Allow `capacity` inside `len()`
    // jonesy:allow(capacity)
    if std::env::args().len() > 100 {
        // Allow explicit `panic`
        // jonesy:allow(panic)
        panic!("Too many arguments!");
    }
}
