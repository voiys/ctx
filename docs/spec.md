# ctx V1 Specification

## Product Intent

`ctx` is a Rust CLI that manages project-specific context for coding agents.

It combines three ideas:

- pinned source repositories that agents can inspect on disk
- snapshotted docs and notes that can be retrieved as cited context
- a project manifest that says which resources matter and why

The CLI is agent-first. It does not have a separate human table output mode. Commands should emit structured, compact stdout and keep progress or diagnostics on stderr.

## Non-Goals For V1

- No npm, PyPI, crates.io, package-manager, or lockfile resolution.
- No bare paths for `ctx add`.
- No source-code chunk retrieval by default.
- No hosted service.
- No automatic background update during query.
- No GitLab, Codeberg, or Bitbucket resolver yet, though resolver boundaries should allow them later.

## Resources

`ctx add` accepts absolute URLs only.

### Source

GitHub repository URLs are source resources:

```text
https://github.com/owner/repo
https://github.com/owner/repo/tree/ref
```

V1 should resolve these to a concrete pinned ref, cache the repository globally, and expose the local path through `ctx path`.

Source repositories are not indexed for RAG retrieval by default.

### Docs

Any other `http` or `https` URL is a docs resource.

Docs are crawled, normalized, snapshotted, indexed, and queried. A docs snapshot is immutable and identified by timestamp plus content fingerprint.

### Notes

`file://` URLs may represent notes.

Notes are snapshotted and indexed like docs. Local bare paths are intentionally not accepted by `ctx add`.

## Project Manifest

Each project has:

```text
.ctx/ctx.json
```

The manifest records project intent and current pointers. Global cache contents live outside the project.

Manifest entries should include:

- `id`
- `label`
- `kind`: `source`, `docs`, or `notes`
- `url`
- `reason`
- `current`: source ref or docs/notes snapshot id
- `created_at`
- `updated_at`

## Global Cache

Suggested layout:

```text
~/.ctx/
  ctx.db
  sources/github.com/<owner>/<repo>/<ref>/
  docs/<resource-id>/<snapshot-id>/
  notes/<resource-id>/<snapshot-id>/
```

Docs and notes snapshots should store enough metadata to make citations stable:

- `snapshot_id`
- `fetched_at`
- `source_url`
- `content_hash`
- `page_count`
- `path`

## CLI Surface

### `ctx init`

Create `.ctx/ctx.json` and upsert a generated block into root `AGENTS.md`.

Flags:

- `--cwd <path>`: project root override
- `--no-agents`: create `.ctx` only

### `ctx add <absolute-url>`

Add a resource to the current project and prepare it.

Behavior:

- GitHub repo URL: resolve, pin, clone/cache, write manifest entry
- Docs URL: crawl, snapshot, index, write manifest entry
- Notes file URL: snapshot, index, write manifest entry

Flags:

- `--label <name>`: stable project-local name
- `--reason <text>`: why this resource belongs to the project
- `--no-index`: fetch/snapshot only
- `--cwd <path>`: project root override

### `ctx update <label-or-url>`

Refresh a docs or notes resource. Create a new immutable snapshot if content changed.

For source resources, report the current pin. Changing source refs should be explicit.

Flags:

- `--force`: create a new snapshot even if the content hash did not change
- `--cwd <path>`: project root override

### `ctx sync`

Ensure every resource in `.ctx/ctx.json` exists locally and docs/notes are query-ready.

Flags:

- `--reindex`: rebuild docs/notes indexes
- `--cwd <path>`: project root override

### `ctx query "<question>"`

Search current project docs and notes only. Return several cited context blocks.

Flags:

- `--top-k <n>`: cited block count, default `5`
- `--budget <tokens>`: context budget, default `20000`
- `--debug`: include ranking details
- `--label <name>`: restrict to one resource
- `--kind docs|notes`: restrict by kind
- `--cwd <path>`: project root override

Retrieval should use code-aware lexical search plus semantic search, fused with reciprocal rank fusion. V1 may start with lexical search if embedding support is not yet wired.

### `ctx show [label-or-url]`

Show current project state or one resource.

Flags:

- `--snapshots`: include docs/notes snapshot history
- `--cwd <path>`: project root override

### `ctx list`

Show all globally cached resources with useful metadata.

Flags:

- `--project`: only resources linked by the current project
- `--kind source|docs|notes`: filter by kind
- `--cwd <path>`: project root override for project-link annotations

### `ctx path <label-or-github-url>`

Print the local cache path for a pinned source repository.

Flags:

- `--cwd <path>`: project root override

### `ctx use <label> <snapshot-or-ref>`

Move the project manifest pointer for a resource.

Use cases:

- docs: set current snapshot
- notes: set current snapshot
- source: switch pinned ref only when already cached/resolved

Flags:

- `--cwd <path>`: project root override

### `ctx remove <label-or-url>`

Remove a resource from the project manifest.

Flags:

- `--prune-cache`: also delete cached data if unused
- `--cwd <path>`: project root override

### `ctx doctor`

Check manifest, cache paths, database access, and index readiness.

Flags:

- `--cwd <path>`: project root override

## Retrieval Output

Default query output should include:

- question
- top-k
- budget
- project root
- results

Each result should include:

- rank
- kind
- label
- content
- citation
- url or file path
- snapshot id for docs/notes
- source ref for source references when relevant
- score

`--debug` should add:

- lexical rank and score
- vector rank and score when available
- fused rank and score
- matched tokens
- chunk id
- parent id

## Architecture

Suggested Rust modules:

```text
cli        command parsing
output     structured stdout encoding
paths      project and global path resolution
manifest   .ctx/ctx.json model and edits
resolver   URL classification and resource resolution
cache      global cache writes and reads
github     GitHub source clone/pin behavior
docs       docs crawl and snapshot behavior
notes      file URL snapshot behavior
index      SQLite schema and indexing
query      retrieval, fusion, packing
agent      AGENTS.md block upsert
doctor     health checks
```

Keep source/docs/notes as distinct resource kinds even if they share storage helpers.
