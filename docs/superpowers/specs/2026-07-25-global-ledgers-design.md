# Global Ledgers Design

**Date:** 2026-07-25
**Status:** Approved for implementation planning

## Summary

Add a small, explicit ledger feature to CTX for durable coordination across agent threads,
repositories, and Git worktrees on one machine. A ledger is one globally stored Markdown
document with a stable identifier, title, and timestamps. CTX stores ledgers in its existing
global SQLite database and exposes direct create, list, show, and full-document write commands.

The feature is coordination state, not automatic agent memory. CTX will not infer ledger
content, recall it automatically, summarize it, or synchronize it between machines. A
coordinator gives workers a ledger identifier and tells them when to read or update it.

## Problem

Long-running work often spans several Codex threads, Herder panes, and Git worktrees. Markdown
ledgers kept inside individual worktrees become scattered, branch-specific, or unavailable to
other workers. Thread instructions can also lose the rule that workers must report through a
shared source of truth.

CTX already provides global machine-local storage through `CTX_HOME` or `~/.ctx`. A global
ledger in that storage gives all local workers one explicit document without project linking or
branch coordination.

## Goals

- Create globally available Markdown ledgers from any working directory.
- Identify each ledger with a readable, immutable identifier.
- List ledgers by most recent update.
- Read and replace the complete Markdown contents exactly.
- Keep every write atomic across independent CTX CLI processes.
- Provide clear optional guidance for coordinators and workers.
- Preserve CTX's structured output and current synchronous Rust architecture.

## Non-goals

- Project linking, unlinking, or per-project ledger membership.
- Revision history, event history, patches, merges, or conflict resolution.
- In-place collaborative editing.
- Ledger rename, archive, or delete commands.
- Full-text or vector search over ledger contents.
- HTML rendering or a local web interface.
- Automatic hooks, background jobs, recall, extraction, or summarization.
- Hosted, multi-device, or multi-machine synchronization.
- Secret detection or redaction.
- Replacing the live repository, runtime, or hosted service as factual proof.

## Storage

CTX will add the following table and index to the existing global `ctx.db`:

```sql
CREATE TABLE IF NOT EXISTS ledgers (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    contents TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS ledgers_updated_at
ON ledgers(updated_at DESC);
```

The implementation will continue to use `rusqlite`. `rusqlite` is the Rust binding for the
SQLite engine already bundled by CTX. Moving the whole database to Turso is out of scope because
CTX currently depends on SQLite FTS5, while Turso v0.7.1 provides a different Tantivy-based FTS
interface and marks SQLite FTS3, FTS4, and FTS5 unsupported.

Global database setup will enable SQLite WAL mode and a five-second busy timeout. Writes will
use short transactions. No application retry loop will run after the timeout.

The feature accepts last-completed-write-wins behavior. If two workers read the same document
and both replace it, CTX does not merge their changes. Each committed write remains atomic, so a
reader sees either the complete old document or the complete new document.

## Ledger identity

`ctx ledger create` derives a normalized slug from the supplied title and appends a compact UTC
creation timestamp:

```text
job-sync-review-20260725T214500Z
```

The identifier is immutable. The timestamp prevents ordinary title collisions and makes the
creation time visible without reading metadata. If two creates with the same title occur during
the same second, CTX adds stable random entropy rather than overwriting or rejecting an unrelated
ledger.

The title remains the user-facing label. Commands that address one ledger require its exact
identifier; title lookup is not supported in the first version because titles need not be unique.

## Command interface

### Create

```text
ctx ledger create "Job Sync review"
```

Creates an empty Markdown document and returns its identifier, title, creation time, and update
time. Creation does not require a CTX project or `--cwd`.

### List

```text
ctx ledger list
```

Lists all ledgers in descending `updated_at` order, with identifier, title, created time, updated
time, and content byte length. Empty results are successful.

### Show

```text
ctx ledger show job-sync-review-20260725T214500Z
```

Returns ledger metadata and the exact Markdown contents. A missing identifier produces a clear
not-found error and a non-zero exit status.

### Write

```text
ctx ledger write job-sync-review-20260725T214500Z --file ledger.md
ctx ledger write job-sync-review-20260725T214500Z --stdin
```

Replaces the complete document and updates `updated_at` in one transaction. `--file` and
`--stdin` are mutually exclusive. One input source is required. CTX preserves the supplied UTF-8
text, including line endings and a final newline. Empty Markdown is valid because clearing a
ledger is a legitimate full replacement.

The result returns the identifier, title, timestamps, and content byte length. It does not echo
the complete contents after a write.

## Output contract

All commands use CTX's existing structured TOON envelope:

```text
command: ledger_write
status: ok
result:
  id: job-sync-review-20260725T214500Z
  title: Job Sync review
  created_at: "2026-07-25T21:45:00Z"
  updated_at: "2026-07-25T22:02:10Z"
  content_bytes: 1842
```

Diagnostics and progress remain on stderr. Errors return a non-zero exit status and do not alter
the existing document.

## Concurrency and failure behavior

- WAL mode allows readers while a writer commits.
- SQLite serializes the small write transactions.
- A writer waits for at most five seconds when another writer holds the database lock.
- A timeout returns a clear busy error. CTX does not loop or retry indefinitely.
- Replacing contents and updating `updated_at` occur in the same transaction.
- A failed read, invalid input source, missing ledger, or failed commit leaves the ledger
  unchanged.
- Last-completed-write-wins is an explicit product rule. CTX provides no revision check.

This contract targets a small number of local agent processes, such as one coordinator and two
workers. It does not promise efficient high-contention collaborative editing.

## Agent workflow and guidance

The bundled CTX skill and generated `AGENTS.md` block will describe ledgers as optional explicit
coordination state:

1. A coordinator creates a ledger and includes its exact identifier in the task or goal.
2. Workers read the named ledger at task start and after a handoff when current shared state
   matters.
3. Workers record confirmed decisions, ownership, blockers, evidence, and next actions.
4. Workers do not store scratch reasoning, credentials, tokens, private content, or copied logs
   without a clear need.
5. Before replacing a shared ledger after meaningful parallel work, a worker reads the current
   contents again. This reduces accidental last-write-wins loss but is guidance, not enforcement.
6. Ledger contents can be stale. Live repository, runtime, database, and hosted state remain the
   authority.

Ledger use will not become mandatory for ordinary CTX grounding tasks. The skill must keep
visible thread coordination and delegation mechanics separate from ledger storage.

## Code boundaries

- `src/app.rs`: nested Clap commands and command dispatch.
- `src/ledger.rs`: ledger validation, identifiers, models, and storage operations.
- `src/storage.rs`: shared SQLite connection initialization, WAL, timeout, and schema setup.
- `src/models.rs`: output data structures if they are shared outside `ledger.rs`.
- `src/agents.rs`: generated project guidance.
- `skills/ctx/SKILL.md`: installed agent workflow.
- `skills/ctx/agents/openai.yaml`: description or default prompt updates if ledger discovery needs
  them.
- `tests/cli.rs`: command, persistence, isolation, and multi-process tests.
- `README.md`, `docs/spec.md`, and `docs/architecture.md`: public contract and boundaries.

Ledger behavior should remain isolated from resource indexing and retrieval. The current FTS5
tables and query flow will not change.

## Verification

### Command behavior

- Create returns a unique readable identifier and empty contents.
- List sorts by `updated_at` descending.
- Show returns exact UTF-8 Markdown.
- Write accepts file and stdin input and rejects missing or conflicting input sources.
- Write preserves multiline content, Unicode, line endings, and final newlines.
- Empty writes clear the contents.
- Missing identifiers fail without creating data.
- Commands work without a project manifest and from unrelated working directories that share
  `CTX_HOME`.

### Storage and migration

- Schema initialization upgrades an existing pre-ledger CTX database without altering resource,
  snapshot, chunk, or FTS data.
- Repeated initialization is idempotent.
- WAL and the busy timeout are set on relevant connections.
- A failed transaction cannot update only the contents or only the timestamp.

### Concurrency

- Start three real OS processes against one isolated `CTX_HOME`.
- Have each process replace one shared ledger with a distinct complete Markdown marker.
- Assert that every successful command exits cleanly.
- Assert that the final document exactly equals one complete submitted marker and contains no
  torn or combined bytes.
- Keep one reader active during writes and assert successful complete reads.

The test proves the declared atomic last-completed-write-wins contract; it does not assert a
particular winning worker.

### Guidance and packaging

- Generated `AGENTS.md` updates remain idempotent and preserve user-authored content.
- The installed CTX skill contains the ledger workflow after `ctx install`.
- Root and nested help expose the commands accurately.
- Formatting, linting, unit tests, CLI integration tests, and a clean release build pass.
- The rebuilt local CLI works against a temporary `CTX_HOME` before any real ledger is created.

## Delivery sequence

1. Add the isolated ledger schema and storage module.
2. Add shared SQLite WAL and busy-timeout setup without changing FTS behavior.
3. Add the command interface and structured outputs.
4. Add functional, migration, and three-process concurrency tests.
5. Update skill, generated guidance, help, and public documentation.
6. Run the full verification suite and release build.
7. Reinstall the local CLI and perform a temporary-home smoke test.
8. Commit the implementation to `main` only after proof is green.

## Future options

Later work may add archive, rename, export, history, search, HTML rendering, hosted sync, or a
Turso storage experiment. Each requires a separate design. Turso should not become the durable
backend until its cross-process path is no longer experimental and CTX has a migration plan for
its FTS5 retrieval index.
