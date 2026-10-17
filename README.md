# ctx

Pin upstream source and read it with `rg` before falling back to searchable docs and papers.
ctx pulls references into a global offline cache under `~/.ctx` (or `CTX_HOME`).
The live repository and its runtime behavior remain the primary source of truth.

ctx is not an agent memory system, hook framework, background service, or package manager.
Resolve exact dependency versions from the real manifest or lockfile; do not guess.

## Source-first workflow

```sh
ctx add https://github.com/clap-rs/clap/tree/v4.6.1 \
  --label clap-4.6.1-src --reason 'upstream source for the locked version'
SRC=$(ctx path clap-4.6.1-src)
rg -n 'ArgAction' "$SRC/clap_builder/src"
git -C "$SRC" rev-parse HEAD

# Documentation is a fallback when source is unavailable or insufficient.
ctx add https://docs.rs/clap/4.6.1/clap/ --label clap-docs --reason 'official API docs'
ctx query 'repeatable arguments' --label clap-docs
ctx guides
ctx guides pin
```

Cite `file:line @ SHA` for source evidence. Treat cached source as read-only; never edit or import from it.
Unscoped query is discovery, not proof. Prefer specific questions and known labels.

## Nine commands

| Command | Purpose |
| --- | --- |
| `add <url>` | Cache a source pin, documentation site, or arXiv paper; store optional `--reason`. |
| `list [target]` | List global resources/reasons; filter with `--kind`, inspect history with `--snapshots`. |
| `path <target>` | Print a cached source checkout path (sources only). |
| `query <question>` | Search docs/papers, optionally with repeated `--label`, `--kind`, or `--snapshot <id>`. |
| `update <target>` | Refresh docs/papers; sources report their existing pin. |
| `remove <target>` | Remove resource/index records; `--prune-cache` also removes current cached files. |
| `doctor` | Check global cache/index health and binary installation metadata. |
| `install` | Copy the binary and `ctx.install.json`; no workflow files are installed. |
| `guides [name]` | List or print eight embedded source-first Markdown workflows. |

Bare `ctx` and `ctx --help` show the short workflow. Use `ctx <command> --help` for examples.

## Stateless resources and restoration

All commands address the global cache by label, URL, or ID; query narrows resources by label/kind.
There is no project setup or project state. Legacy `.ctx/ctx.json` files are ignored, even invalid ones.
Use one label per version: different versions are different resources.

`add` looks up the label before any network access. The same stored URL and an existing cached path
return `existing: true`; an optional reason replaces the previous reason. Omitting reason preserves it.
A label already associated with a different URL is an error. Use `update` to refresh cached docs/papers.
GitHub `/tree/<tag-or-branch>` pins the resolved commit; `/tree/<40-lowercase-hex-sha>` restores an exact commit.
Source pins are code checkouts, not indexed retrieval chunks.

Keep your exact `ctx add` lines in a shell script or README to restore on another machine.
Use the URL displayed by `ctx list`: replacing a tag URL with its SHA URL under the same label is a conflict.
Docs/papers restore as a fresh crawl/fetch, not as their original snapshots.

## Snapshots and retrieval

Docs and papers are indexed on add. Updates create a new immutable snapshot when content changes
(or with `--force`); queries never refresh implicitly. Old snapshots remain cached and indexed.
By default, query searches only each resource's latest (`current`) snapshot. Repeated labels are OR;
kind and snapshot are AND filters. Every requested label must match a queryable resource.

```sh
ctx list clap-docs --snapshots
ctx query 'old API' --label clap-docs --snapshot <snapshot_id>
ctx query 'shared topic' --label docs-a --label docs-b --kind docs
```

An unknown or filtered-out snapshot is an error. Sources cannot be queried; use `path` and normal code tools.
Results are matched chunks with local context and citations, not whole-document summaries.
Lexical and optional local-vector retrieval are fused; `--debug` exposes ranking details.
Set `CTX_EMBEDDINGS=off` to disable embeddings.

`add` accepts absolute HTTP(S) URLs only: GitHub repositories, arXiv papers, and other URLs as docs.
Docs crawling defaults to at most 2048 pages and caps each page at 5 MiB; `--max-pages` and
`--concurrency` bound crawling, not completeness. Run `ctx guides` for the eight detailed workflows.

## Development

```sh
export PATH=$HOME/.cargo/bin:$PATH CTX_EMBEDDINGS=off
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo build --release --locked
CTX_HOME=$PWD/target/ctx-home ./target/release/ctx guides
```

Ignored tests retain optional embedding-download and live-network smoke coverage.
