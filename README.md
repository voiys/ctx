# ctx

`ctx` is a local-first context manager for coding agents.

It pins source repositories, documentation snapshots, and notes globally, stores explicit operational memories, then lets projects opt into a curated `.ctx/ctx.json` view when they need one.

The guiding split is deliberate:

- Source repositories are pinned and cached on disk. Agents explore them with normal code tools such as `rg`, file reads, and callpath tracing.
- Documentation, notes, and research papers are snapshotted, indexed, and searched as LLM-ready context blocks. Notes are indexed as Markdown sections.
- Memories are explicit scoped operational knowledge. Use `ctx remember` to write them and `ctx recall` to retrieve them.
- Project state is optional. Without `.ctx/ctx.json`, commands use the global cache. With `.ctx/ctx.json`, queries default to that project's explicitly linked resources. Project manifests store portable resource intent and current pointers, not machine-local cache paths. Use `ctx link` and `ctx unlink` to edit the project view; `ctx add` always stores global references only.

## V1 Shape

```sh
ctx init
ctx add https://github.com/owner/repo
ctx add https://docs.example.com
ctx add https://arxiv.org/abs/1706.03762
ctx link <label>
ctx query "how do retries work?"
ctx query "how do retries work?" --label <docs-label>
ctx query "how do retries work?" --debug
ctx remember "Run cargo test before claiming parser fixes" --kind preference --subject test.workflow
ctx recall "parser test workflow"
ctx memory list
ctx memory review
ctx export ctx-personal.json
ctx import ctx-personal.json --cwd /path/to/repo
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
- `https://arxiv.org/abs/<id>` is a research paper from the arXiv registry.
- Any other `http` or `https` URL is documentation.
- `file:///absolute/path` may be used for notes.

Source repositories are pinned to a concrete ref and cached globally. `ctx sync` can rebuild a missing source checkout from a project manifest's GitHub URL and current pin. Documentation, research papers, and notes are captured as immutable snapshots with timestamps and content hashes; those snapshots are local cache entries, so a manifest alone cannot recreate them exactly on another machine.

Notes are treated as controlled Markdown and indexed by section headings. Crawled docs and research papers keep the existing text chunking path.

Memories are stored separately from resource snapshots. They support `global`, `project`, and `thread` scopes; project recall searches global memories plus the current project by default.

## Personal Export

Use `ctx export <file>` and `ctx import <file>` to move personal state between machines.
The export contains only memories and notes. It does not include linked project manifests, documentation crawls, cached source repositories, or embeddings.

Imported notes use `ctx-import://` source URLs so they cannot refresh from stale absolute paths. Imported memories and notes include a visible marker with the original export timestamp, import timestamp, and original scope or source path. Global memories stay global; project and thread memories are restored into the project passed with `--cwd` so they are immediately recallable on the new machine.

## Build and Link Locally

Install from a tagged checkout and build with the committed `Cargo.lock`.
There is intentionally no `curl | bash` installer.

```sh
git clone https://github.com/voiys/ctx.git
cd ctx
git checkout <version-tag>
cargo fetch --locked
make build
mkdir -p ~/.local/bin
ln -sf "$PWD/target/release/ctx" ~/.local/bin/ctx
ctx --version
```

Make sure `~/.local/bin` is on your `PATH`. If you prefer copying the binary
instead of linking it, run `make install-local` from the tagged checkout.

## Development Checks

```sh
make check
cargo nextest run
make bench-retrieval
```

`make bench-retrieval` runs the small and large corpus retrieval benchmark described in `docs/retrieval-benchmark.md`.

## Status

This repository is a fresh implementation with the v1 core in place: global resources, optional project manifests, GitHub source caching, recursive docs snapshots, research paper snapshots with arXiv as the first registry, sectioned notes snapshots, explicit scoped memories, SQLite FTS indexing, local embeddings, RRF hybrid retrieval, global listing, cache pruning, pointer validation, and a local install command.
