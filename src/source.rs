use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::models::Resource;
use crate::util::run_command;

pub(crate) fn cache_github_source(
    home: &Path,
    owner: &str,
    repo: &str,
    requested_ref: Option<&str>,
    clone_url: &str,
) -> Result<(String, PathBuf)> {
    let tmp = home
        .join("tmp")
        .join(format!("{}-{}", repo, Uuid::new_v4()));
    fs::create_dir_all(tmp.parent().unwrap())?;
    let mut clone = Command::new("git");
    clone.arg("clone").arg("--depth").arg("1");
    if let Some(reference) = requested_ref {
        clone.arg("--branch").arg(reference);
    }
    clone.arg(clone_url).arg(&tmp);
    run_command(&mut clone, "git clone")?;

    let output = Command::new("git")
        .arg("-C")
        .arg(&tmp)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .context("failed to run git rev-parse")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let commit = String::from_utf8(output.stdout)?.trim().to_string();
    let final_path = home
        .join("sources")
        .join("github.com")
        .join(owner)
        .join(repo)
        .join(&commit);
    if final_path.exists() {
        fs::remove_dir_all(&tmp)?;
    } else {
        fs::create_dir_all(final_path.parent().unwrap())?;
        fs::rename(&tmp, &final_path)?;
    }
    Ok((commit, final_path))
}

pub(crate) fn validate_source_pointer(resource: &Resource, pointer: &str) -> Result<()> {
    if resource.current == pointer {
        return Ok(());
    }
    bail!("source pointer changes must be done by adding or caching an explicit GitHub ref first")
}
