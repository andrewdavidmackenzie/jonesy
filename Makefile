.PHONY: all clippy build test clean

all: clippy test

clean:
	cargo clean

clippy:
	cargo clippy --tests --no-deps --all-features --all-targets

build:
	cargo build

test: build
	cargo test -p jonesy