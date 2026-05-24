# ctx

`ctx` is a local-first context manager for coding agents.

It pins source repositories, documentation snapshots, and notes globally, then lets projects opt into a curated `.ctx/ctx.json` view when they need one.

The guiding split is deliberate:

- Source repositories are pinned and cached on disk. Agents explore them with normal code tools such as `rg`, file reads, and callpath tracing.
- Documentation and notes are snapshotted, indexed, and searched as LLM-ready context blocks.
- Project state is optional. Without `.ctx/ctx.json`, commands use the global cache. With `.ctx/ctx.json`, queries default to that project's explicitly linked resources. Use `ctx link` and `ctx unlink` to edit the project view; `ctx add` always stores global references only.

## V1 Shape

```sh
ctx init
ctx add https://github.com/owner/repo
ctx add https://docs.example.com
ctx link <label>
ctx query "how do retries work?"
ctx query "how do retries work?" --debug
ctx list
ctx show
ctx path <label>
ctx update <label>
ctx sync
ctx unlink <label>
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

## Development Checks

```sh
make check
cargo nextest run
make bench-retrieval
```

`make bench-retrieval` runs the small and large corpus retrieval benchmark described in `docs/retrieval-benchmark.md`.

## Status

This repository is a fresh implementation with the v1 core in place: global resources, optional project manifests, GitHub source caching, recursive docs snapshots, notes snapshots, SQLite FTS indexing, local embeddings, RRF hybrid retrieval, global listing, cache pruning, pointer validation, and a local install command.
