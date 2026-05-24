# Architecture Notes

## Boundaries

`ctx` has three durable boundaries:

- Project manifest: `.ctx/ctx.json`
- Global cache: `~/.ctx`
- SQLite index: `~/.ctx/ctx.db`

The manifest is the source of project intent. The cache and index are rebuildable local state.

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

1. Load `.ctx/ctx.json`
2. Select docs/notes resources
3. Generate lexical candidates using code-aware tokens
4. Generate vector candidates when embeddings exist
5. Fuse candidates with reciprocal rank fusion
6. Deduplicate by parent/resource
7. Pack top results into the default budget
8. Emit structured stdout

Embeddings are generated during indexing with `fastembed` and stored directly on chunks in SQLite. Query-time vector scoring runs over embedded chunks, then lexical and vector candidates are fused with reciprocal rank fusion.

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
