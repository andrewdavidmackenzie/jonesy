use rand::Rng;

const CATCHPHRASES: [&str; 3] = [
    "Don't Panic!",
    "Permission to speak, sir?",
    "They don't like it up 'em!",
];

fn main() {
    // this random number could be out of range of array indexes
    let index = rand::rng().random_range(0..4);

    // jones: this is a potential panic at runtime
    let phrase = CATCHPHRASES[index];
    println!("{}", phrase);

    // Here's a way to do it avoiding a potential panic
    match CATCHPHRASES.get(index) {
        None => println!("Array index out of range of array"),
        Some(phrase) => println!("{phrase}"),
    }
}
