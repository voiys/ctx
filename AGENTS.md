# Contributor notes

- Live source and runtime behavior win; inspect the real callpath before changing it.
- Build/check with `PATH=$HOME/.cargo/bin:$PATH CTX_EMBEDDINGS=off`.
- Run `cargo fmt --check`.
- Run `cargo clippy --locked --all-targets --all-features -- -D warnings`.
- Run `cargo test --locked`; network/model tests remain ignored by default.
- Build with `cargo build --release --locked`.
- For local CLI runs use `CTX_HOME=$PWD/target/ctx-home ./target/release/ctx`.
- Embedded workflow guides live in `guides/`; keep examples stateless.
- Reuse existing helpers; avoid speculative abstractions and dependencies.
