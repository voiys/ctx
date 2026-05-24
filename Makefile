.PHONY: audit build check install-local lint nextest test unused-deps

build:
	cargo build --release

install-local: build
	./target/release/ctx install --force

test:
	cargo test

nextest:
	cargo nextest run

lint:
	cargo fmt --check
	cargo clippy --all-targets --all-features -- -D warnings

audit:
	cargo deny check

unused-deps:
	cargo machete

check: lint test audit unused-deps
