# ctx V1 Specification

## Product Intent

`ctx` is a Rust CLI that manages global and project-specific context for coding agents.

It combines three ideas:

- pinned source repositories that agents can inspect on disk
- snapshotted docs, research papers, and notes that can be retrieved as cited context
- an optional project manifest that says which global resources matter here and why

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

Any non-GitHub, non-arXiv `http` or `https` URL is a docs resource.

Docs are crawled, normalized, snapshotted, indexed, and queried. A docs snapshot is immutable and identified by timestamp plus content fingerprint.

### Research Papers

Research papers are queryable resources. V1 supports arXiv as the first paper registry:

```text
https://arxiv.org/abs/1706.03762
https://arxiv.org/pdf/1706.03762.pdf
https://arxiv.org/html/1706.03762
```

V1 normalizes arXiv paper URLs to the canonical abstract URL, snapshots citation metadata and abstract text, and indexes the arXiv HTML full text when arXiv provides it. Future registries should plug into the same `research_paper` resource kind.

Research paper registries can optionally expose version metadata. arXiv does this through paper versions such as `v1` or `v7`; registries without version semantics do not need to implement that capability.

### Notes

`file://` URLs may represent notes.

Notes are snapshotted like docs. Local bare paths are intentionally not accepted by `ctx add`.

Notes are controlled Markdown. Indexing splits notes into parser-derived sections and stores heading path, heading level, parent/previous/next section indexes, anchors, Markdown content, plain text, and content hashes. Headings inside fenced code blocks must not create sections.

### Memories

Memories are explicit operational knowledge records stored separately from resource snapshots. They are not crawled resources and are not linked through `.ctx/ctx.json`.

Memory scopes:

- `global`: visible everywhere
- `project`: keyed by canonical project root
- `thread`: keyed by an explicit thread id

Memory kinds:

- `preference`
- `fact`
- `decision`
- `recipe`
- `warning`

Memory statuses:

- `suggested`
- `active`
- `dismissed`
- `superseded`

Default recall searches active global memories plus active memories for the current project. Suggested memories appear in `ctx memory review` but are not recalled by default.

## Project Manifest

Each project has:

```text
.ctx/ctx.json
```

The manifest records project intent and current pointers. It must not store machine-local cache paths such as `local_path`; global cache contents live outside the project. A manifest is not required to collect, update, show, query, or remove global references.

Manifest entries should include:

- `id`
- `label`
- `kind`: `source`, `docs`, `research_paper`, or `notes`
- `url`
- `reason`
- `current`: source ref or queryable snapshot id
- `created_at`
- `updated_at`

## Global Cache

Suggested layout:

```text
~/.ctx/
  ctx.db
  sources/github.com/<owner>/<repo>/<ref>/
  docs/<resource-id>/<snapshot-id>/
  research_papers/<resource-id>/<snapshot-id>/
  notes/<resource-id>/<snapshot-id>/
```

Docs, research papers, and notes snapshots should store enough metadata to make citations stable:

- `snapshot_id`
- `fetched_at`
- `source_url`
- `content_hash`
- `page_count`
- `path`
- optional `extra` metadata, such as a research paper registry name and registry version

## CLI Surface

### `ctx init`

Create `.ctx/ctx.json` and upsert a generated block into root `AGENTS.md`.
The generated block tells agents to use `ctx recall` before non-trivial work, `ctx remember` for durable confirmed lessons, and `ctx query`/`ctx path` for project evidence.

Flags:

- `--cwd <path>`: project root override
- `--no-agents`: create `.ctx` only

### `ctx add <absolute-url>`

Add a resource globally and prepare it. This command does not edit `.ctx/ctx.json`.

Behavior:

- GitHub repo URL: resolve, pin, clone/cache globally
- Docs URL: crawl, snapshot, index globally
- Docs URL: include nearby `llms.txt` when present, then ordinary link crawling
- research paper URL: resolve through the matching registry, snapshot paper metadata and available full text, index globally
- Notes file URL: snapshot, index globally
Flags:

- `--label <name>`: stable resource name
- `--reason <text>`: why this resource belongs in the global cache
- `--no-index`: fetch/snapshot only
- `--max-pages <n>`: maximum docs pages to crawl, default `256`
- `--concurrency <n>`: docs crawl worker count, default `16`
- `--cwd <path>`: project root override

### `ctx link <label-or-url-or-id>`

Link an existing global resource into the current project manifest.

Flags:

- `--reason <text>`: project-specific reason for linking this resource
- `--cwd <path>`: project root override

### `ctx update <label-or-url>`

Refresh a docs, research paper, or notes resource from the project manifest when present, otherwise from global resources. Create a new immutable snapshot if content changed.

For source resources, report the current pin. Changing source refs should be explicit.

Flags:

- `--force`: create a new snapshot even if the content hash did not change
- `--max-pages <n>`: maximum docs pages to crawl, default `256`
- `--concurrency <n>`: docs crawl worker count, default `16`
- `--cwd <path>`: project root override

### `ctx sync`

Ensure every resource in `.ctx/ctx.json` exists locally and queryable resources are indexed. Missing GitHub source checkouts can be rebuilt from the manifest URL and current source pin. Docs, research-paper, and notes snapshots are timestamped cache artifacts; `ctx sync` verifies and can reindex them when present, but it does not recreate exact missing snapshots from the manifest alone.

Flags:

- `--reindex`: rebuild docs/research-paper/notes indexes
- `--cwd <path>`: project root override

### `ctx query "<question>"`

Search docs, research papers, and notes. If a project manifest exists, search that project's linked queryable resources. Otherwise, search global queryable resources. Return several cited context blocks.

Flags:

- `--top-k <n>`: cited block count, default `5`
- `--budget <tokens>`: context budget, default `20000`
- `--debug`: include ranking details
- `--label <name>`: restrict to one resource
- `--kind docs|research-paper|notes`: restrict by kind
- `--cwd <path>`: project root override

Retrieval uses code-aware lexical search plus semantic search, fused with reciprocal rank fusion. Chunks sourced from `llms.txt` get a small transparent retrieval prior after fusion so curated LLM context can break close ties without overriding stronger matches. Set `CTX_EMBEDDINGS=off` only for tests or constrained environments.

### `ctx remember <content>`

Store an explicit operational memory.

Flags:

- `--kind preference|fact|decision|recipe|warning`: memory kind
- `--subject <text>`: stable subject or namespace
- `--scope global|project|thread`: memory scope, default `project`
- `--scope-key <text>`: explicit key, required for `thread`
- `--trigger <text>`: optional condition that should surface this memory
- `--suggested`: store as review-needed instead of active
- `--tag <tag>`: repeatable tag
- `--cwd <path>`: project root override

Memory content is stored as Markdown and indexed by section.

### `ctx recall "<question>"`

Search active operational memories.

Flags:

- `--top-k <n>`: memory result count, default `5`
- `--agent`: return grouped evidence sections for each memory
- `--scope global|project|thread`: restrict recall to one scope
- `--scope-key <text>`: explicit key for scoped recall
- `--cwd <path>`: project root override

Default recall includes global memories and the current project scope.

### `ctx memory`

Inspect or manage memories.

Subcommands:

- `ctx memory list`: list visible memories
- `ctx memory show <id>`: show one memory and its sections
- `ctx memory review`: list suggested memories
- `ctx memory forget <id>`: mark a memory dismissed without deleting it

### `ctx show [label-or-url]`

Show current project state or global state. If a project manifest exists, no-target output shows that project view. Without a manifest, no-target output shows global resources.

Flags:

- `--snapshots`: include docs/research-paper/notes snapshot history
- `--cwd <path>`: project root override

### `ctx list`

Show all globally cached resources with useful metadata.

Flags:

- `--project`: only resources linked by the current project
- `--kind source|docs|research-paper|notes`: filter by kind
- `--cwd <path>`: project root override for project-link annotations

### `ctx path <label-or-github-url>`

Print the local cache path for a pinned source repository. Resolves through the project manifest when present, otherwise through global resources.

Flags:

- `--cwd <path>`: project root override

### `ctx use <label> <snapshot-or-ref>`

Move the project manifest pointer for a resource.

Use cases:

- docs: set current snapshot
- research paper: set current snapshot
- notes: set current snapshot
- source: switch pinned ref only when already cached/resolved

Flags:

- `--cwd <path>`: project root override

### `ctx unlink <label-or-url-or-id>`

Remove a resource from the current project manifest without deleting the global resource or cached files.

Flags:

- `--cwd <path>`: project root override

### `ctx remove <label-or-url>`

Remove a global resource entry. This does not edit any project manifest. Use `ctx unlink` for project manifests.

Flags:

- `--prune-cache`: also delete cached files
- `--cwd <path>`: project root override

### `ctx doctor`

Check manifest, cache paths, database access, and index readiness.

Flags:

- `--cwd <path>`: project root override

### `ctx install`

Copy the current `ctx` executable into a user-local binary directory.

Flags:

- `--bin-dir <path>`: override install directory; defaults to `~/.local/bin`
- `--force`: replace an existing `ctx` binary

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
- snapshot id for docs/research-paper/notes
- source ref for source references when relevant
- score

`--debug` should add:

- lexical rank and score
- vector rank and score when available
- fused rank and score
- source prior and prior score when applied
- matched tokens
- chunk id
- parent id

## Docs Crawling

Docs crawling is recursive and parallel. It follows same-origin links under the seed path, strips fragments and query strings, skips common static assets, and stores page-level citations in the snapshot.

Default crawl limits:

- max pages: `256`
- concurrency: `16`

Queries never update docs implicitly. `ctx update <label>` creates a new immutable snapshot when content changes.

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
arxiv      arXiv research paper registry behavior
notes      file URL snapshot behavior
index      SQLite schema and indexing
query      retrieval, fusion, packing
agent      AGENTS.md block upsert
doctor     health checks
```

Keep source/docs/research-paper/notes as distinct resource kinds even if they share storage helpers.
