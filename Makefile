.PHONY: all clippy build test

all: clippy test

clean:
	cargo clean

clippy:
	cargo clippy --tests --no-deps --all-features --all-targets

build:
	cargo build

test:
	cargo test -p jones