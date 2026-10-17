---
name: ctx
version: 1.0.0
description: |
  Ground repository work in authoritative references using the local ctx CLI. Use for questions that depend on a repo's history, a dependency's exact installed version, external documentation, pinned upstream source, research papers, or project notes. Use when live project code is necessary but not sufficient. Do not invoke for every task.
---

# ctx

Use ctx to fetch, pin, label, and retrieve authoritative references for a project. The live repository and runtime behavior remain the primary evidence.

## Start from the project

1. Inspect the live code and the real callpath.
2. If the question involves a dependency, read the project's actual manifest or lockfile and resolve the exact installed version.
3. Run `ctx show --cwd <repo>` to see references already linked to the project.
4. Choose the smallest useful retrieval route below.

## Retrieve a known reference

- Pinned source: run `ctx path <label> --cwd <repo>`, then inspect that checkout with `rg` and file reads.
- Documentation, research, or notes: run `ctx query "<specific question>" --label <label> --cwd <repo>`.
- Add `--kind docs|research-paper|notes` when the resource kind is known.
- Use `--debug` only when ranking or selection needs diagnosis.

Prefer one label and one concrete question. An unscoped global query is discovery, not proof.

## Add an authoritative reference

When the project lacks the needed reference:

```sh
ctx add <official-docs-or-version-pinned-github-url> \
  --label <descriptive-label> \
  --reason "<why this is authoritative>" \
  --cwd <repo>
ctx link <descriptive-label> \
  --reason "<why this project needs it>" \
  --cwd <repo>
```

For dependencies, prefer the exact version's official documentation or a source URL pinned to the resolved version or commit. Do not infer a version from model knowledge or fetch a floating branch when version-sensitive behavior matters.

Run `ctx sync --cwd <repo>` to restore missing pinned source checkouts and verify or reindex snapshots already present locally. Use `ctx update <label> --cwd <repo>` only when a fresh docs, research, or notes snapshot is intentionally needed.

## Initialize or refresh project guidance

- `ctx init --cwd <repo>` creates `.ctx/ctx.json` and writes the generated root `AGENTS.md` block.
- `ctx agents --cwd <repo>` idempotently refreshes that block after ctx guidance changes.

## Evidence rules

- Live source and runtime behavior win over ctx snapshots, cached source, notes, and inference.
- Treat labels and reasons as routing metadata, not proof of authority.
- Verify drift-prone facts against current primary sources.
- Missing evidence means unknown.
- Stop retrieving when the question is answered with enough evidence for the decision. Do not turn ctx into mandatory setup or final-response ceremony.

## Verify grounding

Name the live files or runtime boundary inspected, the ctx label and pin or snapshot used, and any remaining proof gap. Do not claim a fetched reference proves behavior that was never exercised.
