use rand::Rng;

const CATCHPHRASES: [&str; 3] = [
    "Don't Panic!",
    "Permission to speak, sir?",
    "They don't like it up 'em!",
];

fn main() {
    // this random number could be out of range of array indexes
    let num = rand::rng().random_range(0..4);
    let phrase = CATCHPHRASES[num];
    println!("{}", phrase);
}
