# ctx Memory Domain Glossary

This glossary names the memory-system terms used by the Rust layered-memory
spec. It is intentionally small; expand it only when an implementation decision
needs stable language.

## Terms

- **Hook event**: A normalized lifecycle payload received from Codex, Claude
  Code, or another agent host. Hook events are raw evidence, not memories.
- **L0 journal**: The append-only record of user, assistant, and tool events
  captured from hooks. L0 is the audit trail and source of later extraction.
- **L1 memory candidate**: A small structured memory extracted from L0 by an
  LLM. L1 candidates start review-gated unless configured otherwise.
- **L1 active memory**: A reviewed or trusted memory that is eligible for
  recall injection and memory search.
- **L2 scene brief**: A Markdown summary of related L1 memories, organized by
  recurring project, preference, task, or relationship context.
- **L3 persona/profile**: A compact, durable profile synthesized from changed
  L2 scene briefs. It should change slowly and be injected only within budget.
- **Offload node**: A stored tool call, tool result, file snapshot, or summary
  referenced by a stable node id instead of repeated verbatim in context.
- **Active task graph**: A small Mermaid graph describing the current task's
  state, derived from offload nodes and injected during active work.
- **Recall injection**: Bounded context emitted by a hook before the agent
  responds, combining active L1 memories, relevant L2 scene pointers, L3 profile
  snippets, and drill-down commands.
- **Harness job**: A queued memory task where `ctx` provides evidence, a prompt,
  and a result schema, then the user's agent harness performs the reasoning and
  returns structured JSON for `ctx` to validate and apply.
- **Review gate**: The promotion boundary between extracted memory candidates
  and active memories. Review-gated extraction preserves quality by default.
- **Materialized view**: A human-readable file, such as a scene Markdown file,
  generated from canonical storage. It is convenient to inspect but not the
  source of truth unless the spec says otherwise.
