<!-- ctx:start -->
## ctx

Use `ctx` when this task needs authoritative external or version-pinned references. It is a grounding tool, not mandatory per-task ceremony.

- Inspect the live repository and real callpath first. Live source and runtime behavior win over indexed references.
- `ctx show --cwd <repo>` lists references linked to this project, including their labels, kinds, reasons, and current pins.
- `ctx path <source-label> --cwd <repo>` prints a pinned source checkout. Inspect it with normal code tools such as `rg`.
- `ctx query "<specific question>" --label <label> --cwd <repo>` searches fetched documentation, research papers, or notes. Add `--kind docs|research-paper|notes` when useful; use `--debug` only to inspect ranking.
- When an authoritative reference is missing, add it globally with `ctx add <url> --label <descriptive-label> --reason "<why authoritative>" --cwd <repo>`, then link it with `ctx link <label> --reason "<why this project needs it>" --cwd <repo>`.
- For dependency questions, inspect the project's real manifest or lockfile, resolve the exact installed version, then add or link that version's official documentation or version-pinned source. Do not guess versions.
- `ctx sync --cwd <repo>` restores missing pinned source checkouts and verifies or reindexes linked snapshots already present locally.
- Keep retrieval bounded: prefer one known label and a specific question over an unscoped global search. Treat missing evidence as unknown, and verify drift-prone claims against current primary sources.

Global ledgers are shared notes for work that spans threads or workers. They do not manage those threads or workers:

- A coordinator creates a ledger and gives workers its exact identifier. Read it with `ctx ledger show <id>` at task start and after a handoff when shared state matters.
- Record confirmed decisions, owners, blockers, evidence, and next steps. Do not store scratch reasoning, credentials, tokens, private content, or copied logs without a clear need.
- Before replacing a ledger when others may have changed it, read it again. Then use `ctx ledger write <id> --file <path>` or `--stdin`.
- A ledger may be out of date. Check live code, databases, and services before acting on it.

Run `ctx agents --cwd <repo>` to refresh this generated block after upgrading ctx.
<!-- ctx:end -->
