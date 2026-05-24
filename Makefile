.PHONY: build install-local test check

build:
	cargo build --release

install-local: build
	./target/release/ctx install --force

test:
	cargo test

check:
	cargo fmt --check
	cargo test
	cargo clippy -- -D warnings
