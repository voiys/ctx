# AGENTS.md

**ctx** pins upstream source, docs and papers into a global offline cache (`~/.ctx` or
`CTX_HOME`) with a SQLite index. Behaviour: `README.md`, `docs/spec.md`,
`docs/architecture.md`.

## Code principles

> I value robustness, minimizing total lines of code, avoiding bloat, avoiding duplication.
> I hate fallbacks, useless if-else statements (things like "elif code is broken then repair it"
> should not exist, it should fail loudly), and I love asserts, which allow to catch early bugs.
> Fallbacks hide bugs and bloat the code.

Matej: rust (and Effect in TS) make this work well: typed errors, `?`, exhaustive matches,
debug_assert!/assert!, no silent defaults. Prefer a clear loud error over a silent fallback.

## Environment

- `export PATH=$HOME/.cargo/bin:$PATH CTX_EMBEDDINGS=off` for builds and tests.
- No pinned toolchain; edition 2024.
- Git identity comes from repo config: plain `git commit`, never `-c user.*`.
- Local CLI runs: `CTX_HOME=$PWD/target/ctx-home ./target/release/ctx` (never the real
  `~/.ctx`).

## Build, test, gate

- Gate: `cargo fmt --check && cargo clippy --locked --all-targets --all-features -- -D warnings && cargo test --locked`
  (`make lint test`). Network/model tests are `#[ignore]`d.
- Full: `make check` (adds `cargo deny check`, `cargo machete`; neither installed on this box).
- Build: `cargo build --release --locked` (`make build`; sets macOS clang runtime flags).
- Version bump: `make version VERSION=x.y.z`.
- Retrieval benchmark: `make bench-retrieval`.

## Layout

| Path | What |
| --- | --- |
| `src/main.rs` | entrypoint |
| `src/app.rs` | CLI (add, list, path, query, update, remove, doctor, install, guides), guide dispatch |
| `src/context.rs` | cache/index paths, readiness |
| `src/source.rs` | GitHub clone, exact-SHA fetch, checkout cache |
| `src/crawl.rs`, `src/arxiv.rs`, `src/markdown.rs`, `src/snapshot.rs` | docs crawl, arXiv, text extraction, immutable snapshots |
| `src/storage.rs` | SQLite migrations, resources, snapshots, chunks |
| `src/retrieve.rs`, `src/embeddings.rs` | lexical/vector retrieval, fusion, packing; embedding backend |
| `src/install.rs`, `src/output.rs`, `src/models.rs`, `src/input.rs`, `src/util.rs` | install metadata, TOON output, types, URL classification, helpers |
| `guides/` | eight guides embedded via `include_str!` in `src/app.rs` |
| `tests/cli.rs` | CLI integration tests |
| `scripts/` | `bump_version.py`, `retrieval_bench.py`, `docs_stress.py` |

## Product spec

Spec, not policy.
- Stateless: no command reads repository state or initializes a project.
- Guides are printed verbatim; their examples stay stateless.
- ctx is not an agent memory system, hook framework, background service or package manager.
