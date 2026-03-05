all: clippy examples run

clippy:
	cargo clippy --tests --no-deps --all-features --all-targets

build:
	cargo build

examples:
	cargo build -p panic -p perfect -p oom -p cdylib_example -p dylib_example
	dsymutil ./target/debug/panic -o ./target/debug/panic.dSYM 2>&1
	dsymutil ./target/debug/perfect -o ./target/debug/perfect.dSYM 2>&1
	dsymutil ./target/debug/oom -o ./target/debug/oom.dSYM 2>&1
	dsymutil ./target/debug/libcdylib_example.dylib -o ./target/debug/libcdylib_example.dSYM 2>&1
	dsymutil ./target/debug/libdylib_example.dylib -o ./target/debug/libdylib_example.dSYM 2>&1

run: examples
	cd examples/panic && cargo run -p jones || true
	cd examples/oom && cargo run -p jones || true
	cd examples/perfect && cargo run -p jones # perfect should exit with status = 0
	cd examples/cdylib && cargo run -p jones || true
	cd examples/dylib && cargo run -p jones || true