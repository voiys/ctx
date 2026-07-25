use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};

use crate::constants::{AGENTS_BLOCK_END, AGENTS_BLOCK_START};

pub(crate) fn upsert_agents_block(project_root: &Path) -> Result<()> {
    let path = project_root.join("AGENTS.md");
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        bail!("refusing to update symlinked AGENTS.md: {}", path.display());
    }
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let block = format!(
        r#"{AGENTS_BLOCK_START}
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

Global ledgers are optional explicit coordination state, separate from thread coordination and delegation:

- A coordinator creates a ledger and gives workers its exact identifier. Read it with `ctx ledger show <id>` at task start and after a handoff when shared state matters.
- Record confirmed decisions, ownership, blockers, evidence, and next actions. Do not store scratch reasoning, credentials, tokens, private content, or copied logs without a clear need.
- Before replacing a ledger after meaningful parallel work, read the current contents again, then use `ctx ledger write <id> --file <path>` or `--stdin`.
- Ledger contents can be stale. Live repository, runtime, database, and hosted state remain the authority.

Run `ctx agents --cwd <repo>` to refresh this generated block after upgrading ctx.
{AGENTS_BLOCK_END}"#
    );
    let updated = if let Some(start) = existing.find(AGENTS_BLOCK_START) {
        if let Some(end_rel) = existing[start..].find(AGENTS_BLOCK_END) {
            let end = start + end_rel + AGENTS_BLOCK_END.len();
            format!(
                "{}{}{}",
                existing[..start].trim_end(),
                if existing[..start].trim().is_empty() {
                    ""
                } else {
                    "\n\n"
                },
                block
            ) + if existing[end..].trim().is_empty() {
                "\n"
            } else {
                "\n\n"
            } + existing[end..].trim_start()
        } else {
            format!("{}\n\n{}\n", existing.trim_end(), block)
        }
    } else if existing.trim().is_empty() {
        format!("{block}\n")
    } else {
        format!("{}\n\n{}\n", existing.trim_end(), block)
    };
    let mut file = fs::File::create(path)?;
    file.write_all(updated.as_bytes())?;
    Ok(())
}
