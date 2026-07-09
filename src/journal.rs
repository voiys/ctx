use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::sanitize::sanitize_text;
use crate::storage::ensure_db;
use crate::util::{content_hash, stable_id, timestamp};

const REDACTION_VERSION: i64 = 1;

#[derive(Debug)]
pub(crate) struct HookEventInput {
    pub(crate) host: String,
    pub(crate) event_name: String,
    pub(crate) project_root: String,
    pub(crate) session_key: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) payload: Value,
}

pub(crate) fn ingest_hook_event(
    db_path: &Path,
    input: HookEventInput,
) -> Result<serde_json::Value> {
    ensure_db(db_path)?;
    if input.host.trim().is_empty() {
        bail!("hook host cannot be empty");
    }
    if input.event_name.trim().is_empty() {
        bail!("hook event cannot be empty");
    }

    let now = timestamp();
    let payload = redact_payload(input.payload);
    let payload_json = serde_json::to_string(&payload).context("failed to encode hook payload")?;
    let payload_hash = content_hash(&payload_json);
    let session_row_id = session_row_id(
        &input.host,
        &input.project_root,
        input.session_key.as_deref(),
        input.session_id.as_deref(),
    );
    let event_id = stable_id(&format!(
        "hook-event:{}:{}:{}:{}:{}",
        input.host, input.event_name, input.project_root, now, payload_hash
    ));

    let conn = Connection::open(db_path)?;
    conn.execute(
        "INSERT INTO agent_sessions(
            id, created_at, updated_at, host, project_root, session_key, session_id, status
         )
         VALUES(?1, ?2, ?2, ?3, ?4, ?5, ?6, 'active')
         ON CONFLICT(id) DO UPDATE SET
            updated_at = excluded.updated_at,
            status = 'active'",
        params![
            session_row_id,
            now,
            input.host,
            input.project_root,
            input.session_key,
            input.session_id,
        ],
    )?;
    conn.execute(
        "INSERT INTO hook_events(
            id, created_at, host, event_name, project_root, session_key, session_id,
            payload_hash, payload_json, redaction_version
         )
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            event_id,
            now,
            input.host,
            input.event_name,
            input.project_root,
            input.session_key,
            session_row_id,
            payload_hash,
            payload_json,
            REDACTION_VERSION,
        ],
    )?;

    Ok(json!({
        "event_id": event_id,
        "session_id": session_row_id,
        "host": input.host,
        "event": input.event_name,
        "project_root": input.project_root,
        "payload_hash": payload_hash,
        "redaction_version": REDACTION_VERSION,
        "queued_jobs": 0,
    }))
}

fn session_row_id(
    host: &str,
    project_root: &str,
    session_key: Option<&str>,
    session_id: Option<&str>,
) -> String {
    stable_id(&format!(
        "agent-session:{host}:{project_root}:{}:{}",
        session_key.unwrap_or(""),
        session_id.unwrap_or("")
    ))
}

fn redact_payload(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let value = if is_secret_key(&key) {
                        Value::String("[redacted]".to_string())
                    } else {
                        redact_payload(value)
                    };
                    (key, value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(redact_payload).collect()),
        Value::String(value) => Value::String(sanitize_text(&value)),
        other => other,
    }
}

fn is_secret_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "authorization",
        "auth",
        "bearer",
        "password",
        "secret",
        "token",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn ingest_hook_event_redacts_secret_fields() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("ctx.db");
        let result = ingest_hook_event(
            &db_path,
            HookEventInput {
                host: "codex".to_string(),
                event_name: "PostToolUse".to_string(),
                project_root: "/repo".to_string(),
                session_key: Some("thread-1".to_string()),
                session_id: None,
                payload: json!({
                    "tool": "shell",
                    "api_key": "sk-live",
                    "nested": { "token": "secret-token", "text": "hello" }
                }),
            },
        )
        .unwrap();
        assert_eq!(result["host"], "codex");

        let conn = Connection::open(db_path).unwrap();
        let payload: String = conn
            .query_row("SELECT payload_json FROM hook_events", [], |row| row.get(0))
            .unwrap();
        assert!(payload.contains("[redacted]"));
        assert!(!payload.contains("sk-live"));
        assert!(!payload.contains("secret-token"));
    }
}
