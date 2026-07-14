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

Run `ctx agents --cwd <repo>` to refresh this generated block after upgrading ctx.
<!-- ctx:end -->
