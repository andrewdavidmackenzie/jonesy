.PHONY: all clippy build test clean build-examples coverage

all: clippy test

clean:
	cargo clean
	cargo clean --manifest-path examples/workspace_test/Cargo.toml

clippy:
	cargo clippy --tests --no-deps --all-features --all-targets

build:
	cargo build

# Build the nested workspace_test example separately since it has its own workspace
build-examples: build
	cargo build --manifest-path examples/workspace_test/Cargo.toml

test: build-examples
	cargo test -p jonesy

coverage: build-examples
	cargo llvm-cov -p jonesy