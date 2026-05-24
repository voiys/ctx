use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};
use url::Url;

use crate::models::ResourceKind;

pub(crate) fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(crate) fn stable_id(input: &str) -> String {
    let hash = content_hash(input);
    hash[..16].to_string()
}

pub(crate) fn content_hash(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

pub(crate) fn default_label_for_url(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host.trim_start_matches("www.").replace(['.', ':'], "-"))
        })
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "resource".to_string())
}

pub(crate) fn kind_str(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Source => "source",
        ResourceKind::Docs => "docs",
        ResourceKind::Notes => "notes",
    }
}

pub(crate) fn run_command(command: &mut Command, label: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("failed to run {label}"))?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

pub(crate) fn path_size_bytes(path: &Path) -> Result<u64> {
    if path.is_file() {
        return Ok(std::fs::metadata(path)?.len());
    }
    if !path.is_dir() {
        return Ok(0);
    }
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        total += path_size_bytes(&entry?.path())?;
    }
    Ok(total)
}

#[cfg(unix)]
pub(crate) fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}
