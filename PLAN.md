# Plan: Notes And Memory Markdown Sections

## Goal Prompt

Suggested `/goal` text:

```text
Implement the notes/memories Markdown sectioning plan in PLAN.md. Done when note snapshots are indexed with Markdown section metadata, manual memory commands store and recall scoped memories, recall can return grouped evidence, docs/research/source behavior remains unchanged, and focused tests plus make check pass.
```

This keeps the goal short enough for `/goal` and puts the detailed contract in this file. Treat each checkpoint below as resumable: finish the checkpoint, run its validation, then update this plan if the implementation discovers a better narrow path.

Goal practice applied:

- Keep the `/goal` objective short, specific, and measurable.
- Put longer context, constraints, and checkpoint details in this file.
- Give every checkpoint an observable "done when" gate.
- Keep validation close to each checkpoint, then run a broader final ladder before marking the goal complete.

## Scope

In scope:

- Markdown sectioning for notes only among existing resource kinds.
- A new manual memory layer that stores memory bodies as controlled Markdown.
- Section-aware retrieval for notes and memories.
- Grouped agent output for memory recall, modeled after the docs-search agent evidence shape.
- Tests that prove headings inside code fences do not create sections, lists/tables stay with their owning section, and unheaded Markdown has a stable fallback section.

Out of scope for this goal:

- AST sectioning for crawled docs, HTML pages, arXiv, or source repositories.
- Automatic background memory synthesis.
- Agent run logging as a full event journal.
- Promote-to-skill behavior.
- Hosted sync or multi-device memory.
- Human table output mode.

## Design Constraints

- Markdown is the internal prose format for notes and memories because those are the formats `ctx` controls.
- Use a real Markdown parser or event parser. Do not detect headings with regex over raw text.
- Keep the current docs/research indexing path unchanged.
- Keep `ctx query` compatible by default. Add richer evidence shapes behind explicit memory/agent commands or flags.
- Store enough section metadata to reconstruct evidence order without reparsing snapshot files during query.
- Keep SQLite migrations/idempotent schema setup inside the existing `ensure_db` path unless the repo grows a real migration runner.
- Treat embeddings as optional through the existing `EmbeddingBackend` behavior.
- Memory writes should be explicit and reviewable. No silent LLM-generated memories in this goal.

## Target Data Shape

Section metadata should be reusable for note chunks and memory records:

```text
section_index
heading_path
heading_level
parent_section_index
previous_section_index
next_section_index
anchor
markdown
plain_text
content_hash
```

Suggested chunk-table additions:

```sql
ALTER TABLE chunks ADD COLUMN section_index INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chunks ADD COLUMN heading_path TEXT NOT NULL DEFAULT '[]';
ALTER TABLE chunks ADD COLUMN heading_level INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chunks ADD COLUMN parent_section_index INTEGER;
ALTER TABLE chunks ADD COLUMN previous_section_index INTEGER;
ALTER TABLE chunks ADD COLUMN next_section_index INTEGER;
ALTER TABLE chunks ADD COLUMN anchor TEXT;
ALTER TABLE chunks ADD COLUMN plain_text TEXT NOT NULL DEFAULT '';
ALTER TABLE chunks ADD COLUMN content_hash TEXT NOT NULL DEFAULT '';
```

Suggested memory tables:

```sql
CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    scope TEXT NOT NULL,
    scope_key TEXT,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    subject TEXT NOT NULL,
    trigger TEXT,
    content TEXT NOT NULL,
    tags_json TEXT NOT NULL DEFAULT '[]',
    confidence TEXT NOT NULL DEFAULT 'observed',
    last_used_at TEXT,
    confirmed_at TEXT,
    expires_at TEXT,
    supersedes_id TEXT,
    embedding BLOB,
    metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS memory_sections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id TEXT NOT NULL,
    section_index INTEGER NOT NULL,
    heading_path TEXT NOT NULL DEFAULT '[]',
    heading_level INTEGER NOT NULL DEFAULT 0,
    parent_section_index INTEGER,
    previous_section_index INTEGER,
    next_section_index INTEGER,
    anchor TEXT,
    markdown TEXT NOT NULL,
    plain_text TEXT NOT NULL DEFAULT '',
    content_hash TEXT NOT NULL DEFAULT '',
    embedding BLOB
);

CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    memory_id UNINDEXED,
    section_index UNINDEXED,
    subject,
    trigger,
    content,
    tags
);

CREATE TABLE IF NOT EXISTS memory_evidence (
    id TEXT PRIMARY KEY,
    memory_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    source_type TEXT NOT NULL,
    source_id TEXT,
    uri TEXT,
    role TEXT NOT NULL,
    excerpt TEXT
);
```

Keep memory kinds small:

```text
preference
fact
decision
recipe
warning
```

Keep statuses small:

```text
suggested
active
dismissed
superseded
```

## CLI Contract

Human-facing:

```sh
ctx remember "In this repo, run cargo test before claiming parser fixes" \
  --kind preference \
  --subject test.workflow \
  --scope project

ctx recall "what should I know before changing parser tests?"

ctx memory list
ctx memory show <id>
ctx memory review
ctx memory forget <id>
```

Agent-friendly flags:

```sh
ctx recall "grouped search anchor selection" --agent --top-k 5
ctx remember "..." --suggested --trigger "..." --tag markdown --tag retrieval
```

Defer until a later goal:

```sh
ctx agent event ...
ctx agent learn ...
```

## Agent Recall Shape

`ctx recall --agent` should return grouped evidence, not loose flat snippets:

```json
{
  "query": "grouped search returning too many matches",
  "mode": "agent",
  "memories": [
    {
      "memory": {
        "id": "mem_123",
        "kind": "recipe",
        "subject": "search.agent-results",
        "trigger": "grouped search returns too many match sections",
        "content": "Prefer actual query-term overlap for anchor labels; cap anchors and include nearby sections as context.",
        "confidence": "validated",
        "status": "active"
      },
      "score": 0.91,
      "evidence": [
        {
          "kind": "match",
          "headingPath": ["Recipe: Grouped Search Anchor Selection", "Lesson"],
          "markdown": "## Lesson\nPrefer actual query-term overlap for anchor labels...",
          "plainText": "Prefer actual query-term overlap for anchor labels..."
        },
        {
          "kind": "context-before",
          "headingPath": ["Recipe: Grouped Search Anchor Selection", "Trigger"],
          "markdown": "## Trigger\nGrouped search returns too many match sections.",
          "plainText": "Grouped search returns too many match sections."
        }
      ]
    }
  ]
}
```

Evidence kinds:

```text
match
context-before
context-between
context-after
validation
```

## Checkpoints

### Checkpoint 0: Baseline And Parser Choice

Estimate: 30-45 minutes.

Tasks:

- Confirm current note indexing path and existing tests that cover notes.
- Pick the Rust Markdown parser/event parser for sectioning.
- Add the dependency only after a small parser spike proves it can distinguish real headings from fenced-code headings and can preserve tables/lists.

Done when:

- The chosen parser is recorded in this plan or a short docs note.
- No production behavior has changed yet.
- `cargo test` still passes before sectioning work begins.

### Checkpoint 1: Markdown Sectioner Module

Estimate: 60-90 minutes.

Tasks:

- Add a focused `src/markdown.rs` or `src/section.rs` module.
- Implement `section_markdown(markdown: &str) -> Vec<MarkdownSection>`.
- Port the useful `hc-docs-core` sectioner cases:
  - heading hierarchy and parent links
  - numbered workflow steps as sibling sections
  - headings inside fenced code blocks ignored
  - tables and lists stay inside the owning section
  - unheaded content becomes one fallback section
  - duplicate headings get stable unique anchors

Done when:

- Sectioner tests pass in isolation.
- The module has no dependency on storage, retrieval, or CLI code.

### Checkpoint 2: Section-Aware Note Indexing

Estimate: 90-120 minutes.

Tasks:

- Add section metadata columns to `chunks` idempotently.
- For `ResourceKind::Notes`, index Markdown sections instead of size-based paragraph chunks.
- Leave docs and research papers on the current `chunk_text` path.
- Store `heading_path` as JSON text to stay SQLite-simple.
- Use `plain_text` for search/embedding content when available; keep Markdown content for citation output.

Done when:

- Existing note add/query tests still pass.
- New tests prove notes return section metadata under `--debug` or a focused internal assertion.
- Docs and research-paper indexing output remains unchanged in focused regression tests.

### Checkpoint 3: Manual Memory Storage

Estimate: 90-120 minutes.

Tasks:

- Add `memories`, `memories_fts`, and `memory_evidence` tables.
- Add `ctx remember`.
- Implement scopes:
  - `global`
  - `project`, resolved from `--cwd` and current `.ctx`
  - `thread`, accepted as an explicit `--scope-key` only for now
- Store memory content as Markdown, section it with the new sectioner, and index it for FTS plus embeddings when enabled.

Done when:

- `ctx remember` emits structured stdout with the memory id and status.
- Duplicate content can be detected by hash or stable id and handled deterministically.
- Tests cover global and project-scoped memory writes.

### Checkpoint 4: Recall Search

Estimate: 90-120 minutes.

Tasks:

- Add `ctx recall`.
- Search active memories by default.
- Fuse lexical and vector results using the existing retrieval approach where practical.
- Return compact default results for humans/agents that do not request grouped evidence.
- Mark `last_used_at` after a successful recall hit.

Done when:

- Tests prove project-scoped recall excludes unrelated project memories.
- Tests prove dismissed/superseded memories are not returned by default.
- `CTX_EMBEDDINGS=off` still yields lexical recall.

### Checkpoint 5: Grouped Agent Evidence

Estimate: 60-90 minutes.

Tasks:

- Add `ctx recall --agent`.
- Group matches by memory id.
- Return matched sections plus nearby sections and between sections.
- Label evidence as `match`, `context-before`, `context-between`, or `context-after`.
- Cap anchor matches so broad lexical ranking does not turn every section into a match.

Done when:

- A test with hits in section 1 and section 4 includes sections 2 and 3 as `context-between`.
- A test with weak broad matches still returns a small number of anchors.
- The response includes enough ids/citations for an agent to explain why a memory was used.

### Checkpoint 6: Memory Review And Forget

Estimate: 45-75 minutes.

Tasks:

- Add `ctx memory list`.
- Add `ctx memory show <id>`.
- Add `ctx memory forget <id>` as a status update to `dismissed`, not a destructive delete.
- Add `ctx memory review` for `suggested` memories.

Done when:

- Review/list/show/forget all emit structured stdout.
- Tests prove forgotten memories stop appearing in recall.
- No destructive delete is needed for normal user control.

### Checkpoint 7: Cutover Validation

Estimate: 60-90 minutes.

Tasks:

- Run the full validation ladder:
  - `cargo fmt --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test`
  - `make check`
- Run at least one local end-to-end note flow:
  - add a Markdown note
  - query it
  - confirm heading-scoped citation content
- Run at least one local memory flow:
  - remember a scoped recipe
  - recall it
  - recall it with `--agent`
  - forget it
  - confirm it no longer appears

Done when:

- Validation commands pass or any skipped command has a concrete reason.
- `PLAN.md` is updated with completion notes and any deferred follow-ups.
- The final answer reports the implemented CLI, schema changes, and validation evidence.

## Risk Register

- Parser crate mismatch: if no Rust Markdown parser gives enough structure cleanly, use an event parser and build sections from heading events.
- Large notes: if one section is too large for retrieval, split into section parts while preserving the same heading path and section index.
- Schema drift: keep all new columns idempotent because this repo currently initializes schema through `ensure_db`.
- Memory overreach: do not add automatic synthesis in this goal. Manual `remember` plus `suggested` status is enough.
- Retrieval confusion: do not blend docs and memories by default. Use `ctx query` for external resources and `ctx recall` for operational memory.

## Final Handoff Checklist

- [x] Parser choice recorded: `pulldown-cmark` 0.13.4.
- [x] Sectioner tests added.
- [x] Notes section indexing added.
- [x] Memory tables added.
- [x] `ctx remember` added.
- [x] `ctx recall` added.
- [x] `ctx recall --agent` grouped output added.
- [x] Memory review/list/show/forget added.
- [x] Docs/research/source behavior unchanged by default.
- [x] Validation ladder run.

## Completion Notes

- Implemented a Rust Markdown sectioner for controlled Markdown prose using `pulldown-cmark`.
- Notes now index Markdown sections while docs and research papers keep the existing paragraph chunking path.
- Added explicit scoped memories with `remember`, `recall`, and `memory list/show/review/forget`.
- Added grouped `recall --agent` output with match/context-before/context-between/context-after evidence.
- Added CLI tests for note section metadata, memory recall, grouped evidence, scope isolation, suggested review, and forget behavior.
- Validation passed with `make check`.
