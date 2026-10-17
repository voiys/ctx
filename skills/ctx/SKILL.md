---
name: ctx
version: 1.0.0
description: |
  Read pinned upstream source and tests before writing code against a dependency. Resolve the exact installed version, pin its upstream tree, and inspect examples, implementations, and contracts with ctx path and rg. Use when live project code is necessary but not sufficient; documentation and research retrieval are fallbacks. Do not invoke for every task.
---

# ctx

Inspect the live repository and real callpath first. For dependency questions:
**exact installed version → pin → ctx path → rg source/tests/examples → cite file:line @ SHA**.
Pass the cached path explicitly to code tools; it lives outside the project.
Treat cached source as read-only reference code: never edit its files or import from it.

## Evidence rules

- Live source and runtime behavior win over snapshots, cached source, and inference.
- Resolve versions from the project's actual manifest or lockfile; do not guess versions or use a floating branch for version-sensitive behavior.
- Labels and reasons route retrieval; they do not prove authority.
- Missing evidence means unknown. Verify drift-prone facts against current primary sources.
- Stop when the decision has enough evidence; ctx is not mandatory setup or final-response ceremony.

## 0. pin

Read the lockfile and map the exact installed version to its official upstream tag or commit.
Inspect existing links with `ctx show --cwd <repo>`. If the needed pin is missing:

```sh
ctx add https://github.com/<owner>/<repo>/tree/<locked-tag> \
  --label <dep>-<ver>-src --reason "upstream source for locked <ver>" --cwd <repo>
SRC=$(ctx path <dep>-<ver>-src --cwd <repo>)
git -C "$SRC" rev-parse HEAD
```

When project writes are allowed, link with `ctx link <label> --reason "<why needed>" --cwd <repo>`.
Skip `link` and `init` when scope forbids repository writes; the global cache still works.
Use `ctx sync --cwd <repo>` to restore missing linked pins and verify local snapshots.

## 1. idiomatic-usage

Search existing upstream examples and tests before implementation details:

```sh
rg -n '<Api>' "$SRC" -g '**/examples/**' -g '**/docs/**' -g '**/test/**' -g '**/tests/**' -g '**/__tests__/**'
rg -n '<Api>\(' "$SRC" -g '!**/dist/**' -g '!**/*.d.ts'
rg -n '<Api>' <repo>
```

Prefer repeated patterns from the pinned version, not a remembered API shape.
The project's established convention wins when both are valid. Return the pattern plus 1–3 citations.

## 2. copy-pattern

Find how upstream composes the abstraction internally, for example:
`rg -l '<Abstraction>' "$SRC" -g '**/src/**'`.
Read a full call site, then mirror its error handling, resource scoping, and configuration.
Reuse the project's existing helpers instead of hand-rolling another layer. Cite the upstream file.

## 3. tests-as-spec

```sh
rg -n '<symbol>' "$SRC" -g '*test*' -g '*spec*' -g '**/tests/**'
```

Read assertions for inputs, errors, edge cases, and defaults as the pinned contract.
When behavior matters, write the project's own test from that contract; do not mock away the behavior under test.

## 4. trace-api

Find the definition, then trace re-exports from `index.ts`, `lib.rs`, or `__init__.py`:

```sh
rg -n 'export (async )?(function|const|class|type|interface) <X>\b|pub (async )?fn <x>\b|^def <x>\(' "$SRC"
```

Read types, defaults, and real callers. Never answer from model knowledge when pinned source is available.

## 5. error-to-source

Run `rg -n -F '<exact error text>' "$SRC"`.
Read the throw site and its guard conditions, then map them to what the project actually passes in.
Trace wrappers when the error text is generated indirectly; verify the proposed fix at the real boundary.

## 6. upgrade-impact

Pin old and new refs as separate labels using recipe 0, then resolve both paths:

```sh
OLD=$(ctx path <old-label> --cwd <repo>)
NEW=$(ctx path <new-label> --cwd <repo>)
git diff --no-index --stat "$OLD/src" "$NEW/src"
git diff --no-index "$OLD/src" "$NEW/src"
```

Diff existing public-entry subpaths instead if upstream has no `src` directory.
Never diff whole pins: they include `.git`. A diff exit status of 1 means differences, not a broken command.
Search existing changelog, migration, and docs paths under `$NEW` for breaking changes:
`rg -n -i 'breaking|removed|renamed|deprecat' "$NEW" -g '*CHANGELOG*' -g '*MIGRATION*' -g '**/docs/**'`.
List changed/removed exports, then map **every** project use with `rg -n -w '<name1>|<name2>' <repo>`.
Include wrappers, re-exports, types, mocks, fixtures, and snapshots; exclude vendored code as appropriate.
Done means build and full tests pass, and searches for each removed/renamed symbol return zero project hits.

## 7. why-history (optional)

Cached clones are shallow. Only when history is needed and network access is permitted:

```sh
git -C "$SRC" fetch --deepen=500
git -C "$SRC" log -S '<symbol>' --oneline
git -C "$SRC" log -L :<function>:<file>
```

Deepening changes only cached history, never the pinned HEAD; never edit `$SRC` files.
Use `--unshallow` instead of `--deepen` if complete history is necessary. History explains intent, not current behavior.

## Documentation fallback

When source is unavailable or insufficient, add an authoritative docs URL or research paper with a descriptive label and reason.
Run `ctx query "<specific question>" --label <label> --cwd <repo>`; narrow with `--kind docs|research-paper` when useful.
Prefer one known label and a concrete question. Unscoped global search is discovery, not proof.
Use `--debug` only to diagnose ranking. Refresh snapshots with `ctx update <label>` only intentionally.

## Verify grounding

Name the live files or runtime boundary inspected, the ctx label and pin or snapshot used, and any remaining proof gap.
Cite upstream evidence as `file:line @ SHA`. Do not claim a fetched reference proves behavior that was never exercised.
