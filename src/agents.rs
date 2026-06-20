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

Use `ctx` for this project's local context and operational memory.

Before non-trivial work, check for prior project lessons:

- `ctx recall "<task, repo, or failure pattern>" --cwd <repo>` recalls scoped memories. Treat results as evidence-backed hints and verify drift-prone facts against the live repo.
- `ctx remember "<concise reusable lesson>" --kind preference|fact|decision|recipe|warning --subject <stable.topic> --scope project --cwd <repo>` stores confirmed durable lessons. Use `--suggested` for plausible but unconfirmed lessons. Do not store secrets, one-off noise, or unresolved guesses.

Use project context when source, docs, research, or notes evidence is needed:

- `ctx query "<question>" --cwd <repo>` searches project docs, research papers, and notes.
- `ctx query "<question>" --debug --cwd <repo>` includes ranking and section details.
- `ctx path <label>` prints the local path for pinned source repos.
- `ctx show` inspects the project manifest.
- `ctx list --project` shows linked resources.

Source repos are explored on disk. Docs, research papers, and notes are returned as cited context blocks. Memories are recalled separately through `ctx recall`.
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
