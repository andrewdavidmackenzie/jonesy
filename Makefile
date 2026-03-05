all: clippy examples run

clippy:
	cargo clippy --tests --no-deps --all-features --all-targets

build:
	cargo build

examples:
	cargo build -p panic -p perfect -p oom -p library
	dsymutil ./target/debug/panic -o ./target/debug/panic.dSYM 2>&1
	dsymutil ./target/debug/perfect -o ./target/debug/perfect.dSYM 2>&1
	dsymutil ./target/debug/oom -o ./target/debug/oom.dSYM 2>&1
	dsymutil ./target/debug/liblibrary.dylib -o ./target/debug/liblibrary.dSYM 2>&1

run: examples
	cargo run -p jones -- --bin target/debug/panic || true
	cargo run -p jones -- --bin target/debug/oom || true
	cargo run -p jones -- --bin target/debug/perfect || true
	cargo run -p jones -- --lib target/debug/liblibrary.dylib || true