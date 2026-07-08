use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::util::make_executable;

pub(crate) fn install_hook_assets(project_root: &Path, host: &str) -> Result<serde_json::Value> {
    let host = normalize_host(host)?;
    let dir = project_root.join(".ctx").join("hooks");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let script_path = dir.join(format!("{host}-memory-hook.sh"));
    fs::write(&script_path, hook_script(host, project_root))
        .with_context(|| format!("failed to write {}", script_path.display()))?;
    make_executable(&script_path)?;
    let readme_path = dir.join("README.md");
    fs::write(&readme_path, hook_readme())
        .with_context(|| format!("failed to write {}", readme_path.display()))?;
    Ok(json!({
        "host": host,
        "script_path": script_path,
        "readme_path": readme_path,
        "background_execution": false,
    }))
}

pub(crate) fn doctor_hook_assets(project_root: &Path) -> Result<serde_json::Value> {
    let dir = project_root.join(".ctx").join("hooks");
    let hosts = ["codex", "claude"]
        .into_iter()
        .map(|host| {
            let script_path = dir.join(format!("{host}-memory-hook.sh"));
            json!({
                "host": host,
                "script_path": script_path,
                "exists": script_path.exists(),
                "executable": is_executable(&script_path),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "hooks_dir": dir,
        "hosts": hosts,
        "background_execution": false,
    }))
}

fn normalize_host(host: &str) -> Result<&'static str> {
    match host.to_ascii_lowercase().as_str() {
        "codex" => Ok("codex"),
        "claude" | "claude-code" | "claude_code" => Ok("claude"),
        other => bail!("unsupported hook host: {other}"),
    }
}

fn hook_script(host: &str, project_root: &Path) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

event="${{CTX_HOOK_EVENT:-${{CLAUDE_HOOK_EVENT_NAME:-${{CODEX_HOOK_EVENT_NAME:-unknown}}}}}}"
session_key="${{CTX_SESSION_KEY:-${{CLAUDE_SESSION_ID:-${{CODEX_SESSION_ID:-}}}}}}"

ctx hook ingest \
  --host "{host}" \
  --event "$event" \
  --session-key "$session_key" \
  --cwd "{}"
"#,
        project_root.display()
    )
}

fn hook_readme() -> &'static str {
    r#"# ctx Hooks

These scripts only ingest hook JSON from stdin into local ctx storage. They do not call model providers, spawn background harnesses, or apply memory jobs.

After wiring a host hook, run memory work visibly:

```sh
ctx memory l1 enqueue --cwd <repo>
ctx memory process --cwd <repo>
ctx memory job apply <job-id> <result.json> --cwd <repo>
```
"#
}

#[cfg(unix)]
fn is_executable(path: &PathBuf) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &PathBuf) -> bool {
    path.exists()
}
