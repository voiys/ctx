use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::util::make_executable;

pub(crate) fn install_hook_assets(project_root: &Path, host: &str) -> Result<serde_json::Value> {
    let host = normalize_host(host)?;
    let dir = project_root.join(".ctx").join("hooks");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let script_path = dir.join(format!("{host}-memory-hook.sh"));
    fs::write(&script_path, project_hook_script(host, project_root))
        .with_context(|| format!("failed to write {}", script_path.display()))?;
    make_executable(&script_path)?;
    let guidance_path = dir.join("guidance.md");
    if !guidance_path.exists() {
        fs::write(&guidance_path, DEFAULT_GROUNDING_GUIDANCE)
            .with_context(|| format!("failed to write {}", guidance_path.display()))?;
    }
    let plugin_dir = install_project_plugin_assets(project_root, host)?;
    let readme_path = dir.join("README.md");
    fs::write(&readme_path, hook_readme())
        .with_context(|| format!("failed to write {}", readme_path.display()))?;
    Ok(json!({
        "scope": "project",
        "host": host,
        "script_path": script_path,
        "guidance_path": guidance_path,
        "plugin_dir": plugin_dir,
        "readme_path": readme_path,
        "background_execution": false,
    }))
}

pub(crate) fn install_global_hook_assets(ctx_home: &Path, host: &str) -> Result<serde_json::Value> {
    let host = normalize_host(host)?;
    let dir = ctx_home.join("hooks");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let script_path = dir.join(format!("{host}-memory-hook.sh"));
    fs::write(&script_path, global_hook_script(host))
        .with_context(|| format!("failed to write {}", script_path.display()))?;
    make_executable(&script_path)?;
    let guidance_path = dir.join("guidance.md");
    if !guidance_path.exists() {
        fs::write(&guidance_path, DEFAULT_GROUNDING_GUIDANCE)
            .with_context(|| format!("failed to write {}", guidance_path.display()))?;
    }
    let marketplace = install_global_marketplace(ctx_home, host)?;
    let readme_path = dir.join("README.md");
    fs::write(&readme_path, hook_readme())
        .with_context(|| format!("failed to write {}", readme_path.display()))?;
    Ok(json!({
        "scope": "global",
        "host": host,
        "script_path": script_path,
        "guidance_path": guidance_path,
        "plugin_dir": marketplace.plugin_dir,
        "marketplace_root": marketplace.root,
        "marketplace_path": marketplace.manifest_path,
        "registration_command": marketplace.registration_command,
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
            let plugin_dir = project_root.join(".ctx").join("plugins").join(host);
            json!({
                "host": host,
                "script_path": script_path,
                "exists": script_path.exists(),
                "executable": is_executable(&script_path),
                "plugin_dir": plugin_dir,
                "plugin_exists": plugin_dir.exists(),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "scope": "project",
        "hooks_dir": dir,
        "guidance_path": project_root.join(".ctx").join("hooks").join("guidance.md"),
        "hosts": hosts,
        "background_execution": false,
    }))
}

pub(crate) fn doctor_global_hook_assets(ctx_home: &Path) -> Result<serde_json::Value> {
    let dir = ctx_home.join("hooks");
    let hosts = ["codex", "claude"]
        .into_iter()
        .map(|host| {
            let script_path = dir.join(format!("{host}-memory-hook.sh"));
            let marketplace = global_marketplace_paths(ctx_home, host);
            json!({
                "host": host,
                "script_path": script_path,
                "exists": script_path.exists(),
                "executable": is_executable(&script_path),
                "plugin_dir": marketplace.plugin_dir,
                "plugin_exists": marketplace.plugin_dir.exists(),
                "marketplace_root": marketplace.root,
                "marketplace_path": marketplace.manifest_path,
                "marketplace_exists": marketplace.manifest_path.exists(),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "scope": "global",
        "hooks_dir": dir,
        "guidance_path": ctx_home.join("hooks").join("guidance.md"),
        "hosts": hosts,
        "background_execution": false,
    }))
}

pub(crate) fn hook_context(
    project_root: &Path,
    ctx_home: &Path,
    recall_context: Option<&str>,
) -> Result<String> {
    let project_guidance_path = project_root.join(".ctx").join("hooks").join("guidance.md");
    let global_guidance_path = ctx_home.join("hooks").join("guidance.md");
    let guidance = if project_guidance_path.exists() {
        fs::read_to_string(&project_guidance_path)
            .with_context(|| format!("failed to read {}", project_guidance_path.display()))?
    } else if global_guidance_path.exists() {
        fs::read_to_string(&global_guidance_path)
            .with_context(|| format!("failed to read {}", global_guidance_path.display()))?
    } else {
        DEFAULT_GROUNDING_GUIDANCE.to_string()
    };
    let recall = recall_context
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("\n\nRelevant ctx memory recall:\n{value}"))
        .unwrap_or_default();
    Ok(format!("{}\n{}", guidance.trim(), recall)
        .trim()
        .to_string())
}

fn normalize_host(host: &str) -> Result<&'static str> {
    match host.to_ascii_lowercase().as_str() {
        "codex" => Ok("codex"),
        "claude" | "claude-code" | "claude_code" => Ok("claude"),
        other => bail!("unsupported hook host: {other}"),
    }
}

fn project_hook_script(host: &str, project_root: &Path) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

ctx hook handle \
  --host "{host}" \
  --cwd "{}"
"#,
        project_root.display()
    )
}

fn global_hook_script(host: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

ctx hook handle --host "{host}"
"#
    )
}

fn hook_readme() -> &'static str {
    r#"# ctx Hooks

These scripts ingest hook JSON from stdin into local ctx storage and may return bounded hook context for prompt-time grounding. They do not call model providers, spawn background harnesses, or apply memory jobs.

Edit `guidance.md` to tune the durable reminder injected by prompt/session hooks.

After wiring a host hook, run memory work visibly:

```sh
ctx memory l1 enqueue --cwd <repo>
ctx memory process --cwd <repo>
ctx memory job apply <job-id> <result.json> --cwd <repo>
```
"#
}

fn install_project_plugin_assets(project_root: &Path, host: &str) -> Result<PathBuf> {
    let plugin_dir = project_root.join(".ctx").join("plugins").join(host);
    install_plugin_assets(&plugin_dir, host)
}

fn install_plugin_assets(plugin_dir: &Path, host: &str) -> Result<PathBuf> {
    match host {
        "codex" => install_codex_plugin_assets(plugin_dir),
        "claude" => install_claude_plugin_assets(plugin_dir),
        other => bail!("unsupported hook host: {other}"),
    }
}

fn install_codex_plugin_assets(plugin_dir: &Path) -> Result<PathBuf> {
    let manifest_dir = plugin_dir.join(".codex-plugin");
    let bin_dir = plugin_dir.join("bin");
    let skill_dir = plugin_dir.join("skills").join("memory");
    fs::create_dir_all(&manifest_dir)?;
    fs::create_dir_all(&bin_dir)?;
    fs::create_dir_all(&skill_dir)?;

    write_json(
        &manifest_dir.join("plugin.json"),
        json!({
            "name": "ctx-memory",
            "version": "0.1.0",
            "description": "Local layered memory capture and grounding guidance for Codex.",
            "hooks": "./.codex-plugin/hooks.json",
            "skills": "./skills/",
            "interface": {
                "displayName": "ctx Memory",
                "shortDescription": "Local ctx memory capture and prompt grounding.",
                "developerName": "ctx",
                "category": "Developer Tools"
            }
        }),
    )?;
    write_json(&manifest_dir.join("hooks.json"), codex_hooks_json())?;
    let script_path = bin_dir.join("ctx-codex-hook.sh");
    fs::write(&script_path, plugin_hook_script("codex"))
        .with_context(|| format!("failed to write {}", script_path.display()))?;
    make_executable(&script_path)?;
    fs::write(skill_dir.join("SKILL.md"), plugin_skill("ctx-memory"))?;
    Ok(plugin_dir.to_path_buf())
}

fn install_claude_plugin_assets(plugin_dir: &Path) -> Result<PathBuf> {
    let manifest_dir = plugin_dir.join(".claude-plugin");
    let hooks_dir = plugin_dir.join("hooks");
    let bin_dir = plugin_dir.join("bin");
    let skill_dir = plugin_dir.join("skills").join("memory");
    fs::create_dir_all(&manifest_dir)?;
    fs::create_dir_all(&hooks_dir)?;
    fs::create_dir_all(&bin_dir)?;
    fs::create_dir_all(&skill_dir)?;

    write_json(
        &manifest_dir.join("plugin.json"),
        json!({
            "name": "ctx-memory",
            "description": "Local layered memory capture and grounding guidance for Claude Code.",
            "version": "0.1.0",
            "author": {
                "name": "ctx"
            }
        }),
    )?;
    write_json(&hooks_dir.join("hooks.json"), claude_hooks_json())?;
    let script_path = bin_dir.join("ctx-claude-hook.sh");
    fs::write(&script_path, plugin_hook_script("claude"))
        .with_context(|| format!("failed to write {}", script_path.display()))?;
    make_executable(&script_path)?;
    fs::write(skill_dir.join("SKILL.md"), plugin_skill("ctx-memory"))?;
    Ok(plugin_dir.to_path_buf())
}

struct MarketplacePaths {
    root: PathBuf,
    plugin_dir: PathBuf,
    manifest_path: PathBuf,
    registration_command: String,
}

fn install_global_marketplace(ctx_home: &Path, host: &str) -> Result<MarketplacePaths> {
    let paths = global_marketplace_paths(ctx_home, host);
    fs::create_dir_all(&paths.root)?;
    install_plugin_assets(&paths.plugin_dir, host)?;
    match host {
        "codex" => write_codex_marketplace(&paths.root)?,
        "claude" => write_claude_marketplace(&paths.root)?,
        other => bail!("unsupported hook host: {other}"),
    }
    Ok(paths)
}

fn global_marketplace_paths(ctx_home: &Path, host: &str) -> MarketplacePaths {
    let root = ctx_home.join("plugin-marketplaces").join(host);
    let plugin_dir = root.join("plugins").join("ctx-memory");
    let manifest_path = match host {
        "claude" => root.join(".claude-plugin").join("marketplace.json"),
        _ => root.join("marketplace.json"),
    };
    let registration_command = match host {
        "claude" => format!("/plugin marketplace add {}", root.display()),
        _ => format!("codex plugin marketplace add {}", root.display()),
    };
    MarketplacePaths {
        root,
        plugin_dir,
        manifest_path,
        registration_command,
    }
}

fn write_codex_marketplace(root: &Path) -> Result<()> {
    write_json(
        &root.join("marketplace.json"),
        json!({
            "name": "ctx-memory",
            "plugins": [
                {
                    "name": "ctx-memory",
                    "source": {
                        "source": "local",
                        "path": "./plugins/ctx-memory"
                    },
                    "policy": {
                        "installation": "AVAILABLE",
                        "authentication": "ON_INSTALL"
                    },
                    "category": "Developer Tools"
                }
            ]
        }),
    )
}

fn write_claude_marketplace(root: &Path) -> Result<()> {
    let manifest_dir = root.join(".claude-plugin");
    fs::create_dir_all(&manifest_dir)?;
    write_json(
        &manifest_dir.join("marketplace.json"),
        json!({
            "name": "ctx-memory",
            "owner": {
                "name": "ctx"
            },
            "plugins": [
                {
                    "name": "ctx-memory",
                    "description": "Local ctx memory capture and prompt grounding.",
                    "source": "./plugins/ctx-memory"
                }
            ]
        }),
    )
}

fn codex_hooks_json() -> Value {
    json!({
        "hooks": {
            "SessionStart": [hook_group("${PLUGIN_ROOT}/bin/ctx-codex-hook.sh", "Loading ctx grounding")],
            "UserPromptSubmit": [hook_group("${PLUGIN_ROOT}/bin/ctx-codex-hook.sh", "Loading ctx memory")],
            "PostToolUse": [{
                "matcher": "Bash|apply_patch|Edit|Write|mcp__.*",
                "hooks": [hook_handler("${PLUGIN_ROOT}/bin/ctx-codex-hook.sh", "Recording ctx hook evidence")]
            }],
            "PreCompact": [hook_group("${PLUGIN_ROOT}/bin/ctx-codex-hook.sh", "Recording ctx compaction")],
            "Stop": [hook_group("${PLUGIN_ROOT}/bin/ctx-codex-hook.sh", "Recording ctx turn")]
        }
    })
}

fn claude_hooks_json() -> Value {
    json!({
        "hooks": {
            "SessionStart": [hook_group("${CLAUDE_PLUGIN_ROOT}/bin/ctx-claude-hook.sh", "Loading ctx grounding")],
            "UserPromptSubmit": [hook_group("${CLAUDE_PLUGIN_ROOT}/bin/ctx-claude-hook.sh", "Loading ctx memory")],
            "PostToolUse": [{
                "matcher": "Bash|Edit|Write|mcp__.*",
                "hooks": [hook_handler("${CLAUDE_PLUGIN_ROOT}/bin/ctx-claude-hook.sh", "Recording ctx hook evidence")]
            }],
            "PreCompact": [hook_group("${CLAUDE_PLUGIN_ROOT}/bin/ctx-claude-hook.sh", "Recording ctx compaction")],
            "Stop": [hook_group("${CLAUDE_PLUGIN_ROOT}/bin/ctx-claude-hook.sh", "Recording ctx turn")]
        }
    })
}

fn hook_group(command: &str, status_message: &str) -> Value {
    json!({
        "hooks": [hook_handler(command, status_message)]
    })
}

fn hook_handler(command: &str, status_message: &str) -> Value {
    json!({
        "type": "command",
        "command": command,
        "statusMessage": status_message,
        "timeout": 30
    })
}

fn plugin_hook_script(host: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

ctx hook handle --host "{host}"
"#
    )
}

fn plugin_skill(name: &str) -> String {
    format!(
        r#"---
name: memory
description: Use local ctx memory, linked docs, and linked source before answering repo-specific questions.
---

Use `ctx show --cwd <repo>` to inspect linked ctx resources for non-trivial repo work, then use `ctx query`, `ctx path`, or `ctx sync` when those resources can establish relevant facts. Treat ctx and memory as supporting evidence; verify behavior against live source code before making claims or edits.

This skill is bundled with the `{name}` hooks, which inject bounded grounding guidance at prompt time.
"#
    )
}

fn write_json(path: &Path, value: Value) -> Result<()> {
    let encoded = serde_json::to_string_pretty(&value)?;
    fs::write(path, format!("{encoded}\n"))
        .with_context(|| format!("failed to write {}", path.display()))
}

const DEFAULT_GROUNDING_GUIDANCE: &str = r#"# ctx Grounding Guidance

For non-trivial repo work, ground yourself before answering or editing:
- Inspect live source at the real callpath; live source wins over memory, docs, and guesses.
- Use project-linked ctx resources when they can establish relevant context: start with `ctx show --cwd <repo>`, then use `ctx query`, `ctx path`, or `ctx sync` as needed.
- Treat ctx, memories, linked docs, and prior notes as guidance, not proof; verify drift-prone facts against current source, runtime output, or official docs.
- If a claim is not established, check it or state the uncertainty instead of making it up.
"#;

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
