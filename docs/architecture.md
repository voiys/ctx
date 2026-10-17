# Architecture

## Global boundaries

ctx has a global filesystem cache (`~/.ctx` or `CTX_HOME`) and its SQLite index (`ctx.db`).
No command reads repository state or initializes a project. Legacy `.ctx/ctx.json` files are ignored.
Resources are addressed globally by label, URL or ID; labels distinguish versions and carry optional reasons.
ctx is not an agent memory system.

## Modules

```text
main.rs       process entrypoint
app.rs        nine-command CLI orchestration and embedded guide dispatch
context.rs    global cache/index paths and readiness
models.rs     resource, snapshot and retrieval types
input.rs      absolute URL classification
source.rs     GitHub clone, exact-pointer fetch and checkout caching
crawl.rs      bounded recursive documentation crawl
arxiv.rs      arXiv metadata/full-text fetching
markdown.rs   plain-text extraction
snapshot.rs   immutable documentation/paper snapshots
storage.rs    SQLite migration, resources, snapshots and chunk indexing
retrieve.rs   lexical/vector candidates, fusion and context packing
embeddings.rs embedding backend
install.rs    executable and installation metadata only
output.rs     TOON serialization
util.rs       shared helpers
```

The callpath is CLI → AppContext → cache/index services → filesystem, Git, HTTP and SQLite.
Commands: add, list, path, query, update, remove, doctor, install, guides.
Use command help for examples; guides are eight Markdown files embedded with `include_str!` and printed verbatim.
No guide installation or generated repository guidance is needed.

## Source-first path

Resolve the installed version from the live repository's manifest/lockfile before adding upstream source.
Source add preserves the input URL, with an ID based on clone URL plus label. Resolve tags/branches via shallow
clone; a 40-lowercase-hex SHA uses the existing explicit-pointer cache/fetch path. Source pins have no snapshots
or chunks. `path` prints the checkout for normal code tools; cite file:line @ SHA. Never edit cached references.

Add classifies input and looks up the label before any network operation. A conflicting URL fails. An existing
matching path returns `existing: true`; otherwise it is fetched again. Reasons are stored in resources using
an ALTER-if-missing migration following the existing column migration pattern. COALESCE preserves omitted reasons.
Existing databases and chunk schemas remain compatible. Pre-existing duplicate labels resolve to the first row.

## Docs, papers and retrieval

Add always snapshots/indexes docs and papers. Update compares hashes; no-op updates preserve current, while changed
or forced updates write/index a new snapshot. Sources report their existing pin. Queries never refresh implicitly.

Storage validates each repeated query label and builds the OR union, intersected with kind. It returns eligible
`(resource_id, snapshot_id)` pairs: current snapshots by default, or a single explicitly requested snapshot ID.
An explicit ID must exist on an allowed resource, otherwise selection fails. Lexical/vector candidate membership
checks compare both IDs. Historical chunks remain indexed but cannot leak into the default latest-snapshot search.

Retrieval preserves paragraph chunking, optional local-vector search, reciprocal rank fusion, transparent llms.txt
source prior, budget and per-snapshot context expansion. Results are evidence blocks with citations and local context,
not whole-document summaries. `--debug` exposes ranking details; `CTX_EMBEDDINGS=off` disables vectors.

## Restoration and installation

Keep exact `ctx add` lines in a script or README. Re-running cached label/URL pairs is idempotent; explicit SHA URLs
restore exact source commits. Use URLs shown by list: a tag URL and its corresponding SHA URL are different stored URLs.
Docs and papers restore as fresh fetches, not historical snapshots. List can display retained snapshot history;
query can target an older snapshot without moving current.

Remove deletes resource/index records and can prune current cached files. Doctor checks global storage and installation
metadata. Install copies only the executable and ctx.install.json; it does not install workflows or modify repositories.
