# Architecture Notes

## Boundaries

`ctx` has three durable boundaries:

- Project manifest: `.ctx/ctx.json`
- Global cache: `~/.ctx`
- SQLite index: `~/.ctx/ctx.db`

The global cache is the source of stored references. A project manifest is an optional linked view that records project intent and current pointers. `ctx add` mutates global state; `ctx link` and `ctx unlink` mutate project state. The index is rebuildable local state.

## Module Shape

`ctx` uses small Rust modules instead of a framework-level dependency injection system.

```text
main.rs       process entrypoint
app.rs        CLI command orchestration
context.rs    AppContext, runtime paths, project readiness
models.rs     shared durable and output types
manifest.rs   .ctx/ctx.json reads, writes, selection helpers
input.rs      absolute URL classification
source.rs     GitHub source clone/pin/path behavior
crawl.rs      recursive bounded docs crawl
snapshot.rs   immutable docs/notes snapshot writing
storage.rs    SQLite schema, global resources, indexing, cache metadata
retrieve.rs   lexical/vector retrieval, RRF, context packing
embeddings.rs swappable embedding backend boundary
agents.rs     AGENTS.md block management
install.rs    local binary install command
output.rs     TOON stdout encoding
util.rs       small shared helpers
```

The intended layering is:

```text
CLI -> AppContext -> manifest/cache/index services -> concrete implementations
```

Only boundaries that are expected to change get a trait-like seam. Embeddings use `EmbeddingBackend` because local engines, external APIs, and disabled/BM25-only mode should be swappable without touching storage or retrieval command code.

## Source vs Retrieval

Source repositories are not RAG chunks in V1. They are pinned code trees.

Agents should use:

```sh
ctx path <label>
rg "symbol" "$(ctx path <label>)"
```

Docs and notes are the searchable retrieval corpus.

This keeps code exploration structural and lets `ctx query` focus on high-recall prose/context retrieval.

## Docs Snapshots

Docs are mutable on the internet, so `ctx` treats each crawl as an immutable local snapshot.

```text
snapshot id = fetched timestamp + content fingerprint
```

`ctx update <label>` creates a new snapshot when content changes and moves the manifest's `current` pointer.

Queries should never silently update a docs snapshot.

## Retrieval

Target retrieval flow:

1. Load `.ctx/ctx.json` when present
2. Select project docs/notes resources, or global docs/notes resources when no project exists
3. Generate lexical candidates using code-aware tokens
4. Generate vector candidates when embeddings exist
5. Fuse candidates with reciprocal rank fusion
6. Deduplicate by parent/resource
7. Pack top results into the default budget
8. Emit structured stdout

Embeddings are generated during indexing through `EmbeddingBackend` and stored directly on chunks in SQLite. The default backend is `fastembed`; `CTX_EMBEDDINGS=off` selects the disabled backend. Query-time vector scoring runs over embedded chunks, then lexical and vector candidates are fused with reciprocal rank fusion.

## Local Install

Use:

```sh
make install-local
```

This runs a release build and then calls:

```sh
./target/release/ctx install --force
```

The install command copies the current executable to `~/.local/bin/ctx` by default.
