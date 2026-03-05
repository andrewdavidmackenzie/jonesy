all: clippy debug_info panic run

clippy:
	cargo clippy -p jones --tests --no-deps --all-features --all-targets

build:
	cargo build -p jones

panic:
	cargo build --profile=panic

debug_info:
	dsymutil ./target/panic/array_access -o ./target/panic/array_access.dSYM 2>&1
	dsymutil ./target/panic/oom -o ./target/panic/oom.dSYM 2>&1
	dsymutil ./target/panic/panic -o ./target/panic/panic.dSYM 2>&1

run:
	cargo run -p jones -- --bin target/panic/panic || true
	cargo run -p jones -- --bin target/panic/perfect