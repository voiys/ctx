UNAME_S := $(shell uname -s)

ifeq ($(UNAME_S),Darwin)
MACOS_CLANG_RUNTIME_DIR := $(shell clang_bin="$$(xcrun --find clang 2>/dev/null || command -v clang)"; \
	if [ -n "$$clang_bin" ]; then \
		clang_root="$$(dirname "$$(dirname "$$clang_bin")")"; \
		clang_major="$$("$$clang_bin" --version | sed -n 's/.*clang version \([0-9][0-9]*\).*/\1/p;q')"; \
		if [ -n "$$clang_major" ] && [ -d "$$clang_root/lib/clang/$$clang_major/lib/darwin" ]; then \
			printf '%s\n' "$$clang_root/lib/clang/$$clang_major/lib/darwin"; \
		else \
			find "$$clang_root/lib/clang" -path '*/lib/darwin' -type d 2>/dev/null | sort | tail -n 1; \
		fi; \
	fi)
endif

CARGO_RELEASE_RUSTFLAGS := $(strip $(RUSTFLAGS) $(if $(MACOS_CLANG_RUNTIME_DIR),-L native=$(MACOS_CLANG_RUNTIME_DIR)))
CARGO_RELEASE_ENV := $(if $(CARGO_RELEASE_RUSTFLAGS),RUSTFLAGS="$(CARGO_RELEASE_RUSTFLAGS)")

.PHONY: audit bench-retrieval build check install-local lint nextest test unused-deps

build:
	$(CARGO_RELEASE_ENV) cargo build --release --locked

bench-retrieval: build
	python3 scripts/retrieval_bench.py --mode both --embeddings on

install-local: build
	./target/release/ctx install --force

test:
	cargo test --locked

nextest:
	cargo nextest run --locked

lint:
	cargo fmt --check
	cargo clippy --locked --all-targets --all-features -- -D warnings

audit:
	cargo deny check

unused-deps:
	cargo machete

check: lint test audit unused-deps
