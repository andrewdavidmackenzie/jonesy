.PHONY: all clippy build test clean build-examples coverage

all: clippy test

clean:
	cargo clean
	cargo clean --manifest-path examples/workspace_test/Cargo.toml

clippy:
	cargo clippy --tests --no-deps --all-features --all-targets -- -D warnings

build:
	cargo build

# Build the nested workspace_test example separately since it has its own workspace
build-examples: build
	cargo build --manifest-path examples/workspace_test/Cargo.toml

test: build-examples
	@# Remove stale dSYM bundles so jonesy regenerates them from the freshly built binaries
	@find target/debug -name "*.dSYM" -type d -exec rm -rf {} + 2>/dev/null || true
	@# Integration tests run single-threaded to avoid dSYM generation race conditions
	@# when multiple tests analyse the same binary concurrently
	cargo test -p jonesy -- --test-threads=1

coverage: build-examples
	cargo llvm-cov -p jonesy