# ctx

`ctx` is a small, local-first grounding CLI for coding agents. It keeps authoritative references close to the project: pinned upstream source, fetched documentation, research papers, and controlled notes.

The live repository remains the primary source of truth. ctx supplies external and historical evidence when live code alone is not enough.

## What ctx is good at

- Pinning a GitHub repository to a concrete commit and exposing its local checkout.
- Fetching and indexing official documentation with stable snapshots and citations.
- Capturing research papers and local Markdown notes as queryable references.
- Giving references descriptive labels and project-specific reasons so agents can retrieve the right source quickly.
- Linking a curated set of global references to a project through `.ctx/ctx.json`.
- Restoring missing pinned source checkouts and reindexing snapshots with `ctx sync`.
- Installing a `$ctx` skill and generating focused root `AGENTS.md` guidance.

ctx is not an agent memory system, hook framework, background service, or package manager. For dependency questions, inspect the project's real manifest or lockfile, resolve the exact installed version, then add or link that version's official docs or version-pinned source.

## Efficient workflow

```sh
# Initialize the project manifest and generated AGENTS.md block.
ctx init --cwd /path/to/repo

# See what authoritative references are already linked.
ctx show --cwd /path/to/repo

# Add references globally with useful routing metadata.
ctx add https://docs.example.com/v2 \
  --label example-v2-docs \
  --reason "official docs for the locked v2 dependency" \
  --cwd /path/to/repo
ctx add https://github.com/owner/repo/tree/v2.4.1 \
  --label example-v2.4.1-source \
  --reason "upstream source for the exact locked version" \
  --cwd /path/to/repo

# Curate them into the project view.
ctx link example-v2-docs --reason "API behavior reference" --cwd /path/to/repo
ctx link example-v2.4.1-source --reason "implementation reference" --cwd /path/to/repo

# Query prose or inspect pinned source.
ctx query "how are retries bounded?" --label example-v2-docs --cwd /path/to/repo
ctx path example-v2.4.1-source --cwd /path/to/repo

# Restore or verify linked references and refresh generated guidance.
ctx sync --cwd /path/to/repo
ctx agents --cwd /path/to/repo
```

Bare `ctx` and `ctx --help` print this workflow before the command reference.

## Resource model

`ctx add` accepts absolute URLs only.

- `https://github.com/owner/repo[/tree/ref]` is a pinned source repository.
- `https://arxiv.org/...` is a research paper.
- Other `http` or `https` URLs are documentation.
- `file:///absolute/path` is a controlled local note.

Source repositories are cached globally and explored with normal code tools such as `rg`. Documentation, papers, and notes are stored as immutable snapshots and searched as cited context blocks. A project manifest stores portable resource intent and current pointers, never machine-local cache paths.

`ctx add` writes the global cache. `ctx link` and `ctx unlink` edit the project view. Without a project manifest, list, show, update, and query can operate on global resources.

## Bundled `$ctx` skill

`ctx install` copies the binary and installs the bundled skill at `$CODEX_HOME/skills/ctx` or `~/.codex/skills/ctx`. Invoke it as `$ctx` when repository work depends on pinned upstream source, exact dependency versions, external documentation, research, or project notes. It intentionally does not trigger for every task.

## Build and install locally

Build from a tagged checkout with the committed lockfile. There is no `curl | bash` installer.

```sh
cargo fetch --locked
make build
make install-local
ctx --version
```

`ctx install` writes `ctx.install.json` beside the installed binary. `ctx doctor` reports whether the default `~/.local/bin/ctx` install is missing or older than the running checkout.

## Development checks

```sh
make check
cargo nextest run --locked
make bench-retrieval
```

See [docs/spec.md](docs/spec.md) for the command contract and [docs/architecture.md](docs/architecture.md) for module boundaries.
