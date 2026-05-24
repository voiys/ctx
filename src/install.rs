use std::fs;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use directories::UserDirs;
use serde_json::json;

use crate::models::CommandStatus;
use crate::output::print_toon;
use crate::util::make_executable;

pub(crate) fn install(bin_dir: Option<PathBuf>, force: bool) -> Result<()> {
    let target_dir = match bin_dir {
        Some(path) => path,
        None => UserDirs::new()
            .ok_or_else(|| anyhow!("could not determine home directory"))?
            .home_dir()
            .join(".local")
            .join("bin"),
    };
    fs::create_dir_all(&target_dir)?;
    let target = target_dir.join("ctx");
    if target.exists() && !force {
        bail!(
            "{} already exists; pass --force to replace it",
            target.display()
        );
    }
    let current = std::env::current_exe()?;
    fs::copy(&current, &target)?;
    make_executable(&target)?;
    print_toon(CommandStatus {
        command: "install",
        status: "ok",
        result: json!({
            "source": current,
            "target": target,
        }),
    })
}
