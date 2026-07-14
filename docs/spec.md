# ctx specification

## Product intent

ctx is a Rust CLI for grounding coding agents in authoritative references. It manages:

- pinned source repositories inspected on disk;
- snapshotted documentation, research papers, and notes retrieved as cited context;
- an optional project manifest that records which global references matter and why;
- a bundled `$ctx` skill and generated root AGENTS.md guidance.

Commands emit compact structured stdout. Progress and diagnostics use stderr.

## Non-goals

- Agent memory, automatic recall, lifecycle hooks, or background synthesis.
- Package-manager or multi-ecosystem dependency resolution.
- Source-code chunk retrieval.
- Hosted storage or sync.
- Automatic resource refresh during query.
- Bare filesystem paths for `ctx add`.

Dependency grounding stays explicit: inspect the project's real manifest or lockfile, resolve the exact version, then add or link official docs or source pinned to that version.

## Resources

`ctx add` accepts absolute URLs:

- GitHub repository URLs, optionally with `/tree/<ref>`, become `source` resources pinned to a commit.
- arXiv URLs become `research_paper` resources with registry metadata and available full text.
- Other HTTP(S) URLs become recursively crawled `docs` resources.
- File URLs become snapshotted `notes` resources.

Source repositories are cached code trees. Docs, papers, and notes are immutable snapshots with stable citations and content hashes. Notes preserve Markdown section metadata.

## Project manifest and global cache

`.ctx/ctx.json` is an optional curated project view. Each entry records:

- id, label, kind, URL, and project-specific reason;
- current source commit or snapshot id;
- created and updated timestamps.

The manifest must not store machine-local cache paths. Global resource state and the SQLite index live under `~/.ctx` or `CTX_HOME`.

`ctx add` changes the global cache. `ctx link` and `ctx unlink` change the project manifest. A manifest is not required for global add, update, show, list, query, or remove operations.

## Commands

### `ctx` and `ctx --help`

Print the efficient grounding workflow before the generated command reference. The workflow must cover live-source precedence, `show`, labeled `query`, `path`, add/link with labels and reasons, exact dependency-version grounding, `sync`, `init`, and `agents`.

### `ctx init`

Create `.ctx/ctx.json` when missing and idempotently upsert the generated root AGENTS.md block.

- `--cwd <path>` selects the project root.
- `--no-agents` skips AGENTS.md generation.

### `ctx agents`

Idempotently upsert only the generated root AGENTS.md block. This is the explicit refresh path after upgrading ctx.

- `--cwd <path>` selects the project root.

The block says to inspect live code first, use ctx only when authoritative references matter, narrow by labels and kinds, resolve dependency versions from real lockfiles or manifests, and treat missing evidence as unknown. It contains no memory or hook instructions.

### `ctx add <absolute-url>`

Fetch and store a global resource.

- `--label <name>` supplies stable agent-visible routing metadata.
- `--reason <text>` records why the reference is authoritative.
- `--no-index` skips indexing for queryable resources.
- `--max-pages <n>` and `--concurrency <n>` bound docs crawling.
- `--cwd <path>` selects runtime context.

### `ctx link <target>` and `ctx unlink <target>`

Add or remove a global resource from the current project manifest. `link --reason` can replace the global reason with a project-specific one.

### `ctx update <target>`

Refresh docs, paper, or notes content and create a new immutable snapshot when changed. A source resource reports its current pin; changing source pins stays explicit.

### `ctx sync`

Ensure linked resources exist locally. Recreate missing GitHub source checkouts from the manifest URL and pin. Verify existing snapshots, and reindex them with `--reindex`. Missing docs, paper, or notes snapshot content cannot be recreated exactly from the manifest alone.

### `ctx query "<question>"`

Search docs, papers, and notes. With a manifest, search linked queryable resources; otherwise search global resources.

- `--label <name>` restricts to one agent-visible label.
- `--kind docs|research-paper|notes` restricts the resource kind.
- `--top-k`, `--budget`, and `--debug` control result packing and diagnostics.

Default results include matched evidence with nearby section context and stable citations. Debug output adds lexical/vector ranks and source-prior details.

### `ctx show [target]` and `ctx list`

Show the current project view or global resources, including labels, kinds, reasons, current pointers, and cache metadata. `show --snapshots` includes snapshot history. `list --project` limits output to linked resources.

### `ctx path <target>`

Print the local cache path for a pinned source repository. Resolve through the project manifest when linked, otherwise through the global cache.

### `ctx use <label> <pointer>`

Move a project manifest pointer to a cached source ref or known snapshot. Reject unknown pointers.

### `ctx remove <target>`

Remove a global resource entry. `--prune-cache` also removes its cached files. This does not edit project manifests.

### `ctx doctor`

Report manifest, database, cache, and local install health.

### `ctx install`

Copy the current binary to `~/.local/bin` or `--bin-dir`, write install metadata, and install the bundled skill under `$CODEX_HOME/skills/ctx` or `~/.codex/skills/ctx`.

## Retrieval behavior

Retrieval uses code-aware lexical candidates plus optional local embeddings, fused with reciprocal rank fusion. Labels are included in search metadata. `llms.txt` receives a small transparent source prior that may break close ties but must not override stronger matches.

Queries never refresh resources. Source trees are never presented as proof without live inspection. A fetched label or reason helps route evidence but does not establish authority by itself.
