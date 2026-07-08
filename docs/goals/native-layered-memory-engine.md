# Native Layered Memory Engine

## Success Criteria

This goal is complete when:

- `ctx` implements a native Rust memory engine inspired by TencentDB-Agent-Memory:
  L0 hook capture, review-gated L1 memories, L2 scene briefs, L3 profile
  synthesis, and short-term offload/Mermaid drill-down.
- Codex and Claude Code hooks can capture and inject bounded context without
  invisible paid harness execution.
- Harness-driven memory jobs can be claimed, prompted, validated, and applied
  through `ctx` commands.
- Applicable TencentDB reference tests are ported or explicitly mapped as
  non-applicable to this architecture.
- Generated `AGENTS.md` guidance tells agents how to use recall, memory jobs,
  evidence drill-down, and offload state.
- `make check` passes after the final checkpoint.

## Loop

1. Load relevant skills/instructions.
2. Run memory/context recall for ctx/TencentDB/layered-memory decisions.
3. Pick the first unchecked checkpoint that is not blocked.
4. Inspect the real callpath, tests, and TencentDB reference files for that
   checkpoint.
5. Make the smallest complete change for the checkpoint.
6. Run focused tests first, then broaden to `make check` when the checkpoint
   touches shared schema, dependencies, or CLI behavior.
7. Review the diff for accidental churn, failed attempts, and files outside the
   checkpoint.
8. Mark the checkpoint done in this document.
9. Commit the validated checkpoint before starting the next checkpoint.
10. Record only durable lessons, rejected patterns with reasons, repo-specific
    gotchas, or links that changed the goal.

## Decisions

- Native Rust implementation inside `ctx`; copy TencentDB-Agent-Memory's
  architecture, not its TypeScript/Node runtime boundary.
- SQLite is canonical for event, memory, job, offload, and review state.
- Materialized Markdown files are views unless a later checkpoint explicitly
  changes that contract.
- Hooks may capture, index, enqueue, and inject, but must not spawn Codex,
  Claude Code, or any paid/subscription-backed harness invisibly.
- V1 processing is visible conversation mode plus manual review mode.
- LLM-extracted memories are review-gated by default.
- Direct provider HTTP and Tencent/Hermes gateway process tests are out of
  scope for the default synthesis path.

## Pattern

```text
Codex/Claude hooks
  -> ctx hook ingest / ctx hook recall
  -> redacted L0 journal + blob/offload store
  -> inert memory_jobs
  -> visible harness run: job next -> prompt -> JSON result -> job apply
  -> review-gated L1, L2 scene briefs, L3 profile
  -> bounded recall/offload/Mermaid context injection
```

`ctx` validates and applies structured harness results. The harness does not
write SQLite rows or memory files directly.

## Inventory Commands

```bash
ctx path tencentdb-agent-memory-source --cwd /Users/voiys/Desktop/code/ctx
rg --files "$(ctx path tencentdb-agent-memory-source --cwd /Users/voiys/Desktop/code/ctx)" | rg '(test|spec)\.(ts|tsx|js|mjs|cjs)$|(__tests__|tests)/'
rg -n "L0|L1|L2|L3|offload|hook|memory job|harness" docs src tests
make check
```

## Checkpoints

- [x] C00: Reference/spec/bootstrap checkpoint
  - Download TencentDB-Agent-Memory through `ctx`.
  - Draft ADR/spec/glossary and document harness/subscription boundaries.
  - Port applicable utility reference tests.
  - Add initial L0 hook journal schema and `ctx hook ingest`.
  - Validate with `make check`.
- [ ] C01: Harness job queue CLI
  - Implement `ctx memory job next`, `ctx memory job prompt`,
    `ctx memory job apply`, and `ctx memory process` scaffolding.
  - Jobs remain inert until a visible agent run or explicit command handles
    them.
  - Validate with unit and CLI tests for claim, lease, schema validation, apply,
    retry/error states, and no invisible harness execution.
- [ ] C02: L1 extraction and dedup job path
  - Emit L1 extraction prompts/schemas from L0 evidence.
  - Apply structured candidates into review-gated memory rows.
  - Add conflict/dedup apply behavior using existing FTS/vector candidate recall.
  - Validate with Tencent-inspired fixtures and existing memory recall tests.
- [ ] C03: Review promotion and recall injection
  - Add accept/reject/update commands for candidates if needed.
  - Add `ctx hook recall` with bounded L1/L2/L3/offload context packing.
  - Validate with hook fixtures and recall budget tests.
- [ ] C04: L2 scene briefs
  - Add scene brief schema/materialization and structured scene update jobs.
  - Validate create/update/merge/archive operations with golden fixtures.
- [ ] C05: L3 profile
  - Add profile revision schema and changed-scene profile update jobs.
  - Validate profile provenance, budget behavior, and no override of live repo
    evidence.
- [ ] C06: Offload and Mermaid
  - Add offload node/blob storage, graph rendering, drill-down commands, and
    compact active task graph injection.
  - Validate large payload storage, node lookup, and Mermaid output.
- [ ] C07: Hook installers and generated agent guidance
  - Add Codex and Claude Code hook installer/doctor commands.
  - Update generated `AGENTS.md` block with memory-job/offload instructions.
  - Validate generated config and project guidance tests.
- [ ] C08: Final audit
  - Re-run Tencent reference inventory.
  - Confirm non-applicable tests remain documented.
  - Run `make check`.
  - Review diff for accidental direct-provider/gateway/background-worker paths.

## Setup Research Notes

- TencentDB-Agent-Memory current cached commit:
  `4339e63650920871eb0e8888083a1779d114e3ae`.
- Tencent's applicable reference tests for this architecture are currently
  `src/utils/sanitize.test.ts` and `src/utils/time.test.ts`.
- Tencent's direct provider request rewriting, OpenClaw auth-profile lookup, and
  Hermes gateway supervisor tests are intentionally non-applicable to the native
  `ctx` design.

## Working Notes

- C00 changed dependencies for `chrono-tz`, `iana-time-zone`, and `regex`.
- C00 also updated advisory-hit transitive/lock entries: `anyhow` and
  `crossbeam-epoch`.
