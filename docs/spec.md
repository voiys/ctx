# ctx specification

## Intent and non-goals

ctx pulls pinned upstream source, documentation sites, and arXiv research papers into a global offline cache.
Live code wins. Resolve exact installed versions, pin source, then use `path` and `rg` on source/tests/examples.
Docs and research queries are fallbacks. ctx is not an agent memory system, hook framework, background
service, package manager, dependency resolver, or source-code chunk search engine.

## Stateless addressing

The only durable state is the global cache and SQLite index under `~/.ctx` or `CTX_HOME`.
There is no project manifest or setup. Legacy `.ctx/ctx.json` files are ignored, including invalid files.
Resources are addressed by label, stored URL, or ID; use different labels for different versions.
Existing databases gain one nullable `resources.reason` column; chunk schema is unchanged.
Pre-existing duplicate labels resolve to the first matching resource row.

## Resources and add

Only absolute HTTP(S) URLs are accepted:

- GitHub repository URLs, optionally `/tree/<ref>`, create source pins (checkout plus resolved commit).
- arXiv URLs create research-paper snapshots, with canonical `https://arxiv.org/abs/<id>` URLs.
- Other URLs create documentation snapshots, storing the parsed URL's string form.

Source IDs remain based on clone URL and label, not ref; source stored URLs preserve raw input.
Before network access, resolve input and default label, then look up the label:

1. A different stored URL fails with `label '<label>' is already used by <url>; remove it or choose another label`.
2. The same URL with an existing local path returns the resource with `existing: true` and does not fetch.
3. Otherwise fetch/cache again. Exactly 40 lowercase hexadecimal refs use the explicit source-pointer fetch
   path; tags and branches use ordinary source cloning.

`--reason` is persisted and displayed by add/list; omitted reasons preserve existing values via COALESCE.
Docs/papers are always indexed on add. Refresh cached content explicitly with update.
Crawl limits default to 2048 pages and 5 MiB per page; `--max-pages` and `--concurrency` control crawling.

## Nine commands

Every subcommand has a short `Examples:` help block. Bare ctx and root help show a short source-first
add → path + rg → query-fallback loop and direct readers to guides and command help.

| Command | Contract |
| --- | --- |
| `add <url>` | Fetch globally; optional label/reason and docs crawl controls; idempotent as above. |
| `list [target] [--kind K] [--snapshots]` | List global state/reasons, optionally target/kind filtered, with optional snapshot history. |
| `path <target>` | Print the local source checkout path; reject docs/papers. |
| `query <question>` | Search docs/papers with top-k, budget, debug, repeated label, kind and single snapshot filters. |
| `update <target>` | Refresh docs/papers when content changes, or force a snapshot; source pins are reported unchanged. |
| `remove <target> [--prune-cache]` | Delete global resource, snapshots and indexed chunks; optionally prune current cached files. |
| `doctor` | Report global storage/index health and default binary install status. |
| `install [--bin-dir D] [--force]` | Copy executable and installation metadata only; protect existing executable unless forced. |
| `guides [name]` | List workflows or print an embedded Markdown guide verbatim. |

Structured command output is TOON; path and guides print plain text. Diagnostics use stderr and failures
return nonzero. Installation emits no skill path and installs no skills or repository guidance.

## Query and snapshot contract

Source pins are never indexed. Without labels/kind, query searches global docs/papers as discovery, not proof.
Repeated `--label` values are OR; each label must match a queryable resource under the kind filter, or fail
with `no queryable resource matched label`. Kind and snapshot filters intersect that resource set.

By default only `(resource_id, resources.current)` snapshot chunks are eligible. `--snapshot <id>` is one
optional string, selecting only that snapshot among allowed resources. If no allowed snapshot row exists,
fail with `snapshot not found: <id>`. Snapshot IDs are displayed by `list <target> --snapshots`.
Old snapshot files and indexed chunks remain intact. Ranking, fusion, budget and context expansion are unchanged;
context stays within its snapshot. Query output has no project or scope fields.

Updates compare content hashes; no-op updates keep current. Changed or forced updates write/index a new
immutable snapshot and move current. Queries never fetch or refresh resources implicitly.

## Guides and restoration

Eight plain Markdown guides are embedded with include_str!: pin, idiomatic-usage, copy-pattern, tests-as-spec,
trace-api, error-to-source, upgrade-impact, why-history. The index begins with three principles: live code
wins; resolve exact versions without guessing; cite file:line @ SHA. Each guide has a one-line index summary.
Unknown names fail with `unknown guide '<name>'; run \`ctx guides\` to list guides`.
Documentation-query fallback guidance lives in query help.

Keep exact `ctx add` lines in a script or README for restoration. Re-run them to restore missing source pins;
SHA URLs restore exact commits. Use the stored URL printed by list: a tag-to-SHA URL substitution under an
existing label is a conflict. Docs/papers restore through a fresh fetch/crawl, not the historical snapshot.
