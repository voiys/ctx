# ctx

`ctx` is a local-first project context manager for coding agents.

It pins the source repositories, documentation snapshots, and notes that matter to a project, then gives agents cited local context on demand.

The guiding split is deliberate:

- Source repositories are pinned and cached on disk. Agents explore them with normal code tools such as `rg`, file reads, and callpath tracing.
- Documentation and notes are snapshotted, indexed, and searched as LLM-ready context blocks.
- Project state lives in `.ctx/ctx.json`; `ctx init` also upserts a concise usage block into the repository root `AGENTS.md`.

## V1 Shape

```sh
ctx init
ctx add https://github.com/owner/repo
ctx add https://docs.example.com
ctx query "how do retries work?"
ctx query "how do retries work?" --debug
ctx list
ctx show
ctx path <label>
ctx update <label>
ctx sync
ctx remove <label>
ctx doctor
ctx install
```

All command output is optimized for agent consumption. Human progress and diagnostics belong on stderr; structured results belong on stdout.

## Resource Model

`ctx add` accepts absolute URLs only.

- `https://github.com/owner/repo` is a source repository.
- Any other `http` or `https` URL is documentation.
- `file:///absolute/path` may be used for notes.

Source repositories are pinned to a concrete ref and cached globally. Documentation and notes are captured as immutable snapshots with timestamps and content hashes.

## Install Locally

From this checkout:

```sh
make install-local
```

That builds the release binary and copies it to `~/.local/bin/ctx`. Make sure `~/.local/bin` is on your `PATH`.

## Status

This repository is a fresh implementation with the v1 core in place: project manifests, GitHub source caching, recursive docs snapshots, notes snapshots, SQLite FTS indexing, local embeddings, RRF hybrid retrieval, global listing, cache pruning, pointer validation, and a local install command.
