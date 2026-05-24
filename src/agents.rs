use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Result;

use crate::constants::{AGENTS_BLOCK_END, AGENTS_BLOCK_START};

pub(crate) fn upsert_agents_block(project_root: &Path) -> Result<()> {
    let path = project_root.join("AGENTS.md");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let block = format!(
        r#"{AGENTS_BLOCK_START}
## ctx

Use `ctx` for this project's local context.

- `ctx query "<question>"` searches project docs and notes.
- `ctx query "<question>" --debug` includes ranking details.
- `ctx path <label>` prints the local path for pinned source repos.
- `ctx show` inspects the project manifest.
- `ctx list --project` shows linked resources.

Source repos are explored on disk. Docs and notes are returned as cited context blocks.
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
