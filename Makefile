.PHONY: all clippy build test clean build-examples coverage

all: clippy test

clean:
	cargo clean
	cargo clean --manifest-path examples/workspace_test/Cargo.toml

clippy:
	cargo clippy --tests --no-deps --all-features --all-targets -- -D warnings

build:
	CARGO_INCREMENTAL=0 cargo build

# Build the nested workspace_test example separately since it has its own workspace.
# CARGO_INCREMENTAL=0 ensures deterministic codegen matching CI (fresh builds).
build-examples: build
	CARGO_INCREMENTAL=0 cargo build --manifest-path examples/workspace_test/Cargo.toml

test:
	@# Remove stale dSYM bundles before building so fresh ones are created
	@find target/debug -name "*.dSYM" -type d -exec rm -rf {} + 2>/dev/null || true
	@$(MAKE) build-examples
	cargo test -p jonesy

coverage: build-examples
	cargo llvm-cov -p jonesy