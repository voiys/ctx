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

    finalize_source_checkout(home, owner, repo, &tmp)
}

pub(crate) fn cache_github_source_pointer(
    home: &Path,
    owner: &str,
    repo: &str,
    pointer: &str,
    clone_url: &str,
) -> Result<(String, PathBuf)> {
    let existing = source_checkout_path(home, owner, repo, pointer);
    if existing.exists() {
        return Ok((pointer.to_string(), existing));
    }

    cache_github_source_from_fetch(home, owner, repo, pointer, clone_url).or_else(|fetch_error| {
        cache_github_source_from_clone(home, owner, repo, pointer, clone_url).with_context(|| {
            format!("failed shallow fetch for source pointer {pointer}: {fetch_error}")
        })
    })
}

pub(crate) fn validate_source_pointer(resource: &Resource, pointer: &str) -> Result<()> {
    if resource.current == pointer {
        return Ok(());
    }
    bail!("source pointer changes must be done by adding or caching an explicit GitHub ref first")
}

fn cache_github_source_from_fetch(
    home: &Path,
    owner: &str,
    repo: &str,
    pointer: &str,
    clone_url: &str,
) -> Result<(String, PathBuf)> {
    let tmp = source_tmp_path(home, repo);
    let result = (|| {
        fs::create_dir_all(tmp.parent().unwrap())?;
        let mut init = Command::new("git");
        init.arg("init").arg(&tmp);
        run_command(&mut init, "git init")?;

        let mut remote = Command::new("git");
        remote
            .arg("-C")
            .arg(&tmp)
            .arg("remote")
            .arg("add")
            .arg("origin")
            .arg(clone_url);
        run_command(&mut remote, "git remote add")?;

        let mut fetch = Command::new("git");
        fetch
            .arg("-C")
            .arg(&tmp)
            .arg("fetch")
            .arg("--depth")
            .arg("1")
            .arg("origin")
            .arg(pointer);
        run_command(&mut fetch, "git fetch")?;

        let mut checkout = Command::new("git");
        checkout
            .arg("-C")
            .arg(&tmp)
            .arg("checkout")
            .arg("--detach")
            .arg("FETCH_HEAD");
        run_command(&mut checkout, "git checkout")?;

        finalize_source_checkout(home, owner, repo, &tmp)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&tmp);
    }
    result
}

fn cache_github_source_from_clone(
    home: &Path,
    owner: &str,
    repo: &str,
    pointer: &str,
    clone_url: &str,
) -> Result<(String, PathBuf)> {
    let tmp = source_tmp_path(home, repo);
    let result = (|| {
        fs::create_dir_all(tmp.parent().unwrap())?;
        let mut clone = Command::new("git");
        clone
            .arg("clone")
            .arg("--filter")
            .arg("blob:none")
            .arg("--no-checkout")
            .arg(clone_url)
            .arg(&tmp);
        run_command(&mut clone, "git clone")?;

        let mut checkout = Command::new("git");
        checkout
            .arg("-C")
            .arg(&tmp)
            .arg("checkout")
            .arg("--detach")
            .arg(pointer);
        run_command(&mut checkout, "git checkout")?;

        finalize_source_checkout(home, owner, repo, &tmp)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&tmp);
    }
    result
}

fn source_tmp_path(home: &Path, repo: &str) -> PathBuf {
    home.join("tmp")
        .join(format!("{}-{}", repo, Uuid::new_v4()))
}

fn source_checkout_path(home: &Path, owner: &str, repo: &str, commit: &str) -> PathBuf {
    home.join("sources")
        .join("github.com")
        .join(owner)
        .join(repo)
        .join(commit)
}

fn finalize_source_checkout(
    home: &Path,
    owner: &str,
    repo: &str,
    tmp: &Path,
) -> Result<(String, PathBuf)> {
    let output = Command::new("git")
        .arg("-C")
        .arg(tmp)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .context("failed to run git rev-parse")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let commit = String::from_utf8(output.stdout)?.trim().to_string();
    let final_path = source_checkout_path(home, owner, repo, &commit);
    if final_path.exists() {
        fs::remove_dir_all(tmp)?;
    } else {
        fs::create_dir_all(final_path.parent().unwrap())?;
        fs::rename(tmp, &final_path)?;
    }
    Ok((commit, final_path))
}
