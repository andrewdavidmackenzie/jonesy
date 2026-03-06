all: clippy examples run

clippy:
	cargo clippy --tests --no-deps --all-features --all-targets

build:
	cargo build

run:
	cd examples/array_access && cargo run -p jones || true
	cd examples/panic && cargo run -p jones || true
	cd examples/oom && cargo run -p jones || true
	cd examples/perfect && cargo run -p jones # perfect should exit with status = 0
	cd examples/cdylib && cargo run -p jones || true
	cd examples/dylib && cargo run -p jones || true