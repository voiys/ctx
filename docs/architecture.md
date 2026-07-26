# Architecture

## Durable boundaries

ctx keeps four kinds of data:

- Project manifest: `.ctx/ctx.json`
- Global cache: `~/.ctx`
- SQLite retrieval index: `~/.ctx/ctx.db`
- Shared ledgers: the `ledgers` table in `~/.ctx/ctx.db`

The global cache holds downloaded references. A project manifest lists the references that matter to one project. It never stores local cache paths. CTX can rebuild the SQLite search index. Ledgers are shared notes stored on this computer; they do not belong to a project or the search index.

## Module shape

```text
main.rs       process entrypoint
app.rs        CLI parsing and command orchestration
context.rs    runtime paths and project readiness
ledger.rs     global ledger identity, validation, models, and transactions
models.rs     resource, manifest, snapshot, and retrieval types
manifest.rs   .ctx/ctx.json reads and edits
input.rs      absolute URL classification
source.rs     GitHub source clone, pin, and path behavior
crawl.rs      recursive bounded documentation crawl
arxiv.rs      arXiv research-paper behavior
markdown.rs   Markdown sectioning for controlled notes
snapshot.rs   immutable docs, paper, and notes snapshots
storage.rs    shared SQLite setup, resource metadata, and retrieval index
retrieve.rs   lexical/vector retrieval, fusion, and context packing
embeddings.rs swappable embedding backend
agents.rs     generated AGENTS.md block management
install.rs    local binary and bundled skill installation
output.rs     structured TOON stdout
util.rs       small shared helpers
```

The layering is:

```text
CLI -> AppContext -> manifest/cache/index services -> filesystem, Git, HTTP, SQLite
```

Ledger commands use the shorter global path: `CLI -> AppContext -> ledger service -> SQLite`. They do not read or change a project manifest or use FTS5 search.

## Source and query paths

Source repositories are pinned code trees, not retrieval chunks. Agents call `ctx path <label>` and inspect the checkout with normal code tools. This preserves repository structure and real callpaths.

Documentation, research papers, and notes form the query corpus. `ctx query` searches only those resource kinds and returns cited evidence blocks. Labels and kinds narrow retrieval before ranking.

Docs, papers, and notes are immutable snapshots. `ctx update <label>` creates a new snapshot when content changes; queries never refresh resources implicitly. `ctx sync` can restore source checkouts from a manifest pin and reindex snapshots that still exist locally.

## Grounding contract

ctx is supporting evidence. The live project and runtime behavior win when evidence conflicts.

For dependency-sensitive work, the agent reads the project's real manifest or lockfile, resolves the exact installed version, then links that version's official docs or pinned source. ctx deliberately does not implement a broad package-manager resolver.

The bundled `$ctx` skill and generated AGENTS.md block teach this workflow without requiring ctx on every task. `ctx agents` refreshes the generated block idempotently.

## Retrieval

Indexing stores parser-derived section metadata for controlled Markdown notes and paragraph chunks for crawled docs and papers. Lexical and optional vector candidates are fused with reciprocal rank fusion. `llms.txt` gets only a small transparent source prior after fusion.

Set `CTX_EMBEDDINGS=off` for tests or constrained environments. The default backend is local `fastembed`.

## SQLite and ledgers

All shared SQLite connections use WAL mode and wait up to five seconds for a lock. Setup adds the `ledgers` table and an index for listing the newest updates first. Repeated setup is safe and does not change FTS5 search.

Create and write each use one short database change. A write changes `contents` and `updated_at` together. SQLite lets one writer save at a time while readers continue. CTX does not retry, merge, keep old versions, search ledgers, archive or delete them, or link them to projects. If two workers write at once, ctx keeps the write that finishes last.

## Local install

`make install-local` builds a locked release and runs `ctx install --force`. The install command copies the executable to `~/.local/bin/ctx` by default and installs the bundled skill under the active Codex home.
