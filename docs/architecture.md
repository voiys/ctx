# Architecture

## Durable boundaries

ctx has four durable boundaries:

- Project manifest: `.ctx/ctx.json`
- Global cache: `~/.ctx`
- SQLite retrieval index: `~/.ctx/ctx.db`
- Global ledgers: the `ledgers` table in `~/.ctx/ctx.db`

The global cache owns stored references. A project manifest is an optional curated view with labels, reasons, kinds, and current pointers. It never stores machine-local cache paths. The SQLite index is rebuildable local state. Global ledgers are durable machine-local coordination documents and do not belong to a project or retrieval index.

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

Ledger commands take the shorter global path: `CLI -> AppContext -> ledger service -> SQLite`. They do not read or edit a project manifest and do not enter the FTS5 retrieval path.

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

All shared SQLite connections use WAL mode and a five-second busy timeout. Schema setup adds the `ledgers` table and its descending `updated_at` index with idempotent `CREATE IF NOT EXISTS` statements while leaving the FTS5 tables and query flow intact.

Ledger create and full-document write operations use short immediate transactions. A write changes `contents` and `updated_at` together. SQLite serializes concurrent writers; readers can continue during a commit. CTX applies no retry loop, revision check, merge, history, search, archive, delete, or project-link layer. The accepted conflict rule is last-completed-write-wins.

## Local install

`make install-local` builds a locked release and runs `ctx install --force`. The install command copies the executable to `~/.local/bin/ctx` by default and installs the bundled skill under the active Codex home.
