all: clippy test

clippy:
	cargo clippy --tests --no-deps --all-features --all-targets

build:
	cargo build

test:
	cargo test -p jones