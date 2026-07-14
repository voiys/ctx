use std::fs;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use directories::UserDirs;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::models::CommandStatus;
use crate::output::print_toon;
use crate::util::{make_executable, timestamp};

const INSTALL_METADATA_FILE: &str = "ctx.install.json";
const CTX_SKILL: &str = include_str!("../skills/ctx/SKILL.md");
const CTX_SKILL_METADATA: &str = include_str!("../skills/ctx/agents/openai.yaml");

#[derive(Debug, Deserialize, Serialize)]
struct InstallMetadata {
    version: String,
    installed_at: String,
    source: String,
    target: String,
}

pub(crate) fn install(bin_dir: Option<PathBuf>, force: bool) -> Result<()> {
    let target_dir = match bin_dir {
        Some(path) => path,
        None => default_bin_dir()?,
    };
    fs::create_dir_all(&target_dir)?;
    let target = target_dir.join("ctx");
    if target.exists() && !force {
        bail!(
            "{} already exists; pass --force to replace it",
            target.display()
        );
    }
    let metadata_path = install_metadata_path(&target_dir);
    let previous = read_install_metadata(&metadata_path).ok().flatten();
    let target_existed = target.exists();
    let current = std::env::current_exe()?;
    fs::copy(&current, &target)?;
    make_executable(&target)?;
    let metadata = InstallMetadata {
        version: env!("CARGO_PKG_VERSION").to_string(),
        installed_at: timestamp(),
        source: current.display().to_string(),
        target: target.display().to_string(),
    };
    write_install_metadata(&metadata_path, &metadata)?;
    let skill_path = install_ctx_skill()?;
    let status = install_version_status(target_existed, previous.as_ref(), &metadata.version);
    let notice = install_version_notice(status, previous.as_ref(), &metadata.version);
    print_toon(CommandStatus {
        command: "install",
        status: "ok",
        result: json!({
            "source": current,
            "target": target,
            "metadata_path": metadata_path,
            "version": metadata.version,
            "previous_version": previous.map(|metadata| metadata.version),
            "install_version_status": status,
            "skill_path": skill_path,
            "notice": notice,
        }),
    })
}

fn install_ctx_skill() -> Result<PathBuf> {
    let codex_home = if let Some(path) = std::env::var_os("CODEX_HOME") {
        PathBuf::from(path)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".codex")
    } else {
        UserDirs::new()
            .ok_or_else(|| anyhow!("could not determine Codex home directory"))?
            .home_dir()
            .join(".codex")
    };
    let skill_dir = codex_home.join("skills").join("ctx");
    let agents_dir = skill_dir.join("agents");
    fs::create_dir_all(&agents_dir)?;
    fs::write(skill_dir.join("SKILL.md"), CTX_SKILL)?;
    fs::write(agents_dir.join("openai.yaml"), CTX_SKILL_METADATA)?;
    Ok(skill_dir)
}

pub(crate) fn default_install_status() -> Result<serde_json::Value> {
    let target_dir = default_bin_dir()?;
    let target = target_dir.join("ctx");
    let metadata_path = install_metadata_path(&target_dir);
    let metadata_result = read_install_metadata(&metadata_path);
    let metadata_error = metadata_result
        .as_ref()
        .err()
        .map(|error| format!("{error:#}"));
    let metadata = metadata_result.ok().flatten();
    let running_version = env!("CARGO_PKG_VERSION");
    let installed_version = metadata.as_ref().map(|metadata| metadata.version.clone());
    let status = if !target.exists() {
        "not_installed"
    } else if metadata_error.is_some() {
        "metadata_invalid"
    } else if metadata.is_none() {
        "metadata_missing"
    } else if installed_version.as_deref() == Some(running_version) {
        "current"
    } else if installed_version
        .as_deref()
        .and_then(|version| compare_versions(version, running_version))
        == Some(std::cmp::Ordering::Less)
    {
        "outdated"
    } else {
        "version_mismatch"
    };
    let notice = match status {
        "metadata_missing" => Some(
            "default ctx install has no version metadata; run `make install-local` from this checkout to refresh it",
        ),
        "metadata_invalid" => Some(
            "default ctx install metadata is unreadable; run `make install-local` from this checkout to refresh it",
        ),
        "outdated" => Some(
            "default ctx install is older than this checkout; run `make install-local` to refresh it",
        ),
        "version_mismatch" => Some(
            "default ctx install version differs from this checkout; run `make install-local` if this checkout should be active",
        ),
        _ => None,
    };
    Ok(json!({
        "target": target,
        "target_exists": target.exists(),
        "metadata_path": metadata_path,
        "metadata_exists": metadata.is_some(),
        "metadata_error": metadata_error,
        "running_version": running_version,
        "installed_version": installed_version,
        "status": status,
        "notice": notice,
    }))
}

fn default_bin_dir() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".local").join("bin"));
    }
    Ok(UserDirs::new()
        .ok_or_else(|| anyhow!("could not determine home directory"))?
        .home_dir()
        .join(".local")
        .join("bin"))
}

fn install_metadata_path(target_dir: &std::path::Path) -> PathBuf {
    target_dir.join(INSTALL_METADATA_FILE)
}

fn read_install_metadata(path: &std::path::Path) -> Result<Option<InstallMetadata>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

fn write_install_metadata(path: &std::path::Path, metadata: &InstallMetadata) -> Result<()> {
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(metadata)?),
    )?;
    Ok(())
}

fn install_version_status(
    target_existed: bool,
    previous: Option<&InstallMetadata>,
    current_version: &str,
) -> &'static str {
    if !target_existed {
        return "new_install";
    }
    let Some(previous) = previous else {
        return "replaced_untracked_install";
    };
    if previous.version == current_version {
        return "reinstalled_same_version";
    }
    match compare_versions(&previous.version, current_version) {
        Some(std::cmp::Ordering::Less) => "updated_outdated_install",
        Some(std::cmp::Ordering::Greater) => "replaced_newer_install",
        _ => "replaced_different_version",
    }
}

fn install_version_notice(
    status: &str,
    previous: Option<&InstallMetadata>,
    current_version: &str,
) -> Option<String> {
    match status {
        "replaced_untracked_install" => Some(format!(
            "existing ctx install had no version metadata; installed {current_version}"
        )),
        "updated_outdated_install" => Some(format!(
            "updated ctx install from {} to {current_version}",
            previous
                .map(|metadata| metadata.version.as_str())
                .unwrap_or("unknown")
        )),
        "replaced_newer_install" => Some(format!(
            "replaced newer recorded ctx install {} with {current_version}",
            previous
                .map(|metadata| metadata.version.as_str())
                .unwrap_or("unknown")
        )),
        "replaced_different_version" => Some(format!(
            "replaced ctx install {} with {current_version}",
            previous
                .map(|metadata| metadata.version.as_str())
                .unwrap_or("unknown")
        )),
        _ => None,
    }
}

fn compare_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    Some(parse_version(left)?.cmp(&parse_version(right)?))
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let core = value.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}
