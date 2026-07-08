# ADR 0001: Implement the layered memory engine natively in ctx

Status: Proposed

## Context

TencentDB-Agent-Memory has the behavior we want to borrow: L0 conversation
capture, L1 extracted memories with deduplication, L2 scene Markdown, L3 persona
Markdown, and separate short-term context offload with Mermaid task state.

The Tencent implementation is a TypeScript service with a host-neutral
`TdaiCore`, a Node HTTP gateway, and host adapters. It is not packaged as a
stable Rust library. Its gateway is useful as a reference boundary, but the
current gateway `/recall` response returns only stable system context and drops
dynamic L1 `prependContext`, so blindly adapting to the gateway would miss the
highest-value memory hits.

`ctx` already owns a local-first Rust storage and retrieval stack: SQLite, FTS5,
FastEmbed vectors, reciprocal-rank fusion, scoped explicit memories, Markdown
sectioning, and generated `AGENTS.md` instructions.

## Decision

Implement a native Rust layered-memory engine inside `ctx` rather than making
TencentDB-Agent-Memory the runtime dependency.

The implementation will copy the architecture, not the package boundary:

- Use `ctx` SQLite as the canonical store for L0-L3 metadata, searchable text,
  review status, vectors, and job state.
- Store bulky tool payloads as content-addressed blobs referenced by SQLite.
- Materialize L2 scene briefs and L3 profile files for human inspection.
- Add Codex and Claude Code hook adapters that call `ctx hook ...` commands.
- Add an optional `ctx daemon` for low-latency hooks and job coordination; hooks
  must still degrade to direct CLI writes when the daemon is unavailable.
- Run L1/L2/L3 reasoning through the user's agent harness, not through a
  `ctx`-owned LLM provider endpoint. `ctx` stores evidence, builds prompts,
  accepts structured job results, and applies them.
- Do not invoke paid or subscription-backed agent harnesses invisibly. Harness
  reasoning is user-visible or explicitly user-triggered by default.
- Keep LLM-extracted memories review-gated by default.

## Consequences

Benefits:

- One local binary owns storage, hooks, review, recall, job prompts, and project
  instructions.
- No Node sidecar is required for the core memory path.
- No direct provider HTTP client is required for memory synthesis; the host
  agent remains the execution environment for LLM work.
- Existing `ctx recall`, FTS, embeddings, import/export, and project scoping can
  be extended instead of duplicated.
- We can avoid copying the gateway recall omission and inject dynamic L1 hits
  explicitly.

Costs:

- We need to port the L1/L2/L3 pipeline state machine, prompts, structured
  result schemas, and validators to Rust.
- Hook support must handle Codex and Claude Code differences rather than relying
  on a single host runtime.
- Offload behavior will not be identical across hosts: Claude Code can replace
  tool output where its hook supports it; Codex can capture and inject context
  but currently should be treated as best-effort for true in-loop output
  replacement.

## Non-Goals

- Depend on Tencent Cloud VectorDB.
- Require TencentDB-Agent-Memory or Node to run the normal `ctx` memory engine.
- Contact OpenAI-compatible or other LLM endpoints directly for the default
  synthesis path.
- Run background agent-harness jobs without explicit opt-in, visible logs, and
  user-controlled limits.
- Make unreviewed LLM extraction active by default.
