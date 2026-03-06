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
}
