use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::memory::{RememberInput, ResolvedMemoryScope, recall, remember as remember_memory};
use crate::models::{MemoryKind, MemoryScope, MemoryStatus};
use crate::sanitize::{sanitize_text, should_extract_l1};
use crate::storage::ensure_db;
use crate::util::{stable_id, timestamp};

pub(crate) const L1_EXTRACT_JOB_KIND: &str = "l1_extract";

const MAX_L1_EVENTS: usize = 50;

pub(crate) fn l1_extract_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["candidates"],
        "properties": {
            "scene_name": { "type": "string" },
            "candidates": { "type": "array" }
        }
    })
}

pub(crate) fn l1_prompt_rules() -> &'static str {
    "L1 extraction rules:
- Extract only durable, reusable memories from the supplied redacted L0 hook events.
- Prefer atomic candidates: one preference, fact, decision, recipe, or warning per candidate.
- Every candidate should include content, kind, subject, trigger when useful, tags, and source_event_ids.
- Use kind values preference, fact, decision, recipe, or warning when possible.
- Use action store unless the evidence says to skip, update, or merge with a target_memory_id.
- Do not invent facts beyond the supplied evidence.
- Do not create active memories directly; ctx will save review-gated suggestions."
}

pub(crate) fn recent_l0_evidence(
    db_path: &Path,
    project_root: &str,
    session_id: Option<&str>,
    limit: usize,
) -> Result<Vec<Value>> {
    ensure_db(db_path)?;
    if limit == 0 {
        bail!("l1 evidence limit must be greater than zero");
    }
    let limit = limit.min(MAX_L1_EVENTS) as i64;
    let conn = Connection::open(db_path)?;
    let mut rows = if let Some(session_id) = session_id {
        let mut stmt = conn.prepare(
            "SELECT id, created_at, host, event_name, session_key, session_id, payload_json
             FROM hook_events
             WHERE project_root = ?1 AND session_id = ?2
             ORDER BY created_at DESC, id DESC
             LIMIT ?3",
        )?;
        stmt.query_map(params![project_root, session_id, limit], row_l0_evidence)?
            .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, created_at, host, event_name, session_key, session_id, payload_json
             FROM hook_events
             WHERE project_root = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT ?2",
        )?;
        stmt.query_map(params![project_root, limit], row_l0_evidence)?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    rows.retain(is_signal_l0_event);
    rows.reverse();
    Ok(rows)
}

pub(crate) fn apply_l1_extract_result(
    db_path: &Path,
    project_root: &str,
    job_id: &str,
    evidence: &Value,
    result: &Value,
) -> Result<Value> {
    ensure_db(db_path)?;
    let candidates = candidate_array(result)?;
    let allowed_event_ids = evidence_event_ids(evidence);
    let scope = ResolvedMemoryScope {
        scope: MemoryScope::Project,
        scope_key: Some(project_root.to_string()),
    };

    let mut stored = Vec::new();
    let mut skipped = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let parsed = match L1Candidate::parse(candidate, result, &allowed_event_ids) {
            Ok(Some(parsed)) => parsed,
            Ok(None) => {
                skipped.push(json!({
                    "candidate_index": index,
                    "decision": "skip",
                    "reason": "low_signal_or_empty",
                }));
                continue;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("invalid L1 candidate at index {index}"));
            }
        };
        if parsed.action == "skip" {
            skipped.push(json!({
                "candidate_index": index,
                "decision": "skip",
                "reason": "candidate_action_skip",
                "content": parsed.content,
            }));
            continue;
        }

        if let Some(duplicate) =
            exact_active_duplicate(db_path, project_root, parsed.kind, &parsed.content)?
        {
            skipped.push(json!({
                "candidate_index": index,
                "decision": "skip",
                "reason": "exact_active_duplicate",
                "duplicate": duplicate,
                "content": parsed.content,
            }));
            continue;
        }

        let similar = similar_active_memories(db_path, &scope, &parsed)?;
        let memory = remember_memory(
            db_path,
            RememberInput {
                kind: parsed.kind,
                status: MemoryStatus::Suggested,
                scope: scope.clone(),
                subject: parsed.subject.clone(),
                trigger: parsed.trigger.clone(),
                content: parsed.content.clone(),
                tags: parsed.tags.clone(),
            },
        )?;
        let memory_id = memory["id"]
            .as_str()
            .ok_or_else(|| anyhow!("suggested memory result has no id"))?;
        let metadata = json!({
            "layer": "L1",
            "job_id": job_id,
            "candidate_index": index,
            "action": parsed.action,
            "source_event_ids": parsed.source_event_ids,
            "target_memory_id": parsed.target_memory_id,
            "similar_memory_ids": similar
                .iter()
                .filter_map(|memory| memory.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>(),
        });
        update_memory_metadata(db_path, memory_id, &metadata)?;
        insert_memory_evidence(
            db_path,
            memory_id,
            &parsed.source_event_ids,
            &parsed.content,
        )?;
        stored.push(json!({
            "candidate_index": index,
            "decision": format!("{}_suggested", parsed.action),
            "memory": memory,
            "source_event_ids": parsed.source_event_ids,
            "similar_memories": similar,
        }));
    }

    Ok(json!({
        "kind": L1_EXTRACT_JOB_KIND,
        "candidate_count": candidates.len(),
        "stored_count": stored.len(),
        "skipped_count": skipped.len(),
        "stored": stored,
        "skipped": skipped,
    }))
}

fn row_l0_evidence(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let payload_json: String = row.get(6)?;
    let payload = serde_json::from_str::<Value>(&payload_json).unwrap_or(Value::Null);
    Ok(json!({
        "event_id": row.get::<_, String>(0)?,
        "created_at": row.get::<_, String>(1)?,
        "host": row.get::<_, String>(2)?,
        "event": row.get::<_, String>(3)?,
        "session_key": row.get::<_, Option<String>>(4)?,
        "session_id": row.get::<_, Option<String>>(5)?,
        "payload": payload,
    }))
}

fn is_signal_l0_event(event: &Value) -> bool {
    event
        .get("payload")
        .and_then(|payload| serde_json::to_string(payload).ok())
        .is_some_and(|payload| payload.trim().len() > 2)
}

fn candidate_array(result: &Value) -> Result<&Vec<Value>> {
    result
        .get("candidates")
        .or_else(|| result.get("memories"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("L1 result must include a candidates array"))
}

fn evidence_event_ids(evidence: &Value) -> BTreeSet<String> {
    evidence
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|event| event.get("event_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

#[derive(Debug)]
struct L1Candidate {
    kind: MemoryKind,
    subject: String,
    trigger: Option<String>,
    content: String,
    tags: Vec<String>,
    source_event_ids: Vec<String>,
    action: String,
    target_memory_id: Option<String>,
}

impl L1Candidate {
    fn parse(
        value: &Value,
        result: &Value,
        allowed_event_ids: &BTreeSet<String>,
    ) -> Result<Option<Self>> {
        let Some(object) = value.as_object() else {
            bail!("candidate must be an object");
        };
        let Some(raw_content) = object.get("content").and_then(Value::as_str) else {
            bail!("candidate content is required");
        };
        let content = sanitize_text(raw_content);
        if !should_extract_l1(&content) {
            return Ok(None);
        }
        let kind = object
            .get("kind")
            .or_else(|| object.get("type"))
            .and_then(Value::as_str)
            .map(parse_l1_kind)
            .transpose()?
            .unwrap_or(MemoryKind::Fact);
        let subject = object
            .get("subject")
            .and_then(Value::as_str)
            .map(sanitize_text)
            .filter(|subject| !subject.trim().is_empty())
            .or_else(|| {
                result
                    .get("scene_name")
                    .and_then(Value::as_str)
                    .map(sanitize_text)
                    .filter(|subject| !subject.trim().is_empty())
            })
            .unwrap_or_else(|| format!("l1.{}", stable_id(&content)));
        let trigger = object
            .get("trigger")
            .and_then(Value::as_str)
            .map(sanitize_text)
            .filter(|trigger| !trigger.trim().is_empty());
        let source_event_ids = source_event_ids(object.get("source_event_ids"), allowed_event_ids)?;
        let mut tags = object
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(sanitize_text)
            .filter(|tag| !tag.trim().is_empty())
            .collect::<BTreeSet<_>>();
        tags.insert("l1".to_string());
        let action = object
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("store")
            .to_ascii_lowercase();
        if !matches!(action.as_str(), "store" | "skip" | "update" | "merge") {
            bail!("unsupported L1 candidate action: {action}");
        }
        let target_memory_id = object
            .get("target_memory_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|id| !id.trim().is_empty());
        Ok(Some(Self {
            kind,
            subject,
            trigger,
            content,
            tags: tags.into_iter().collect(),
            source_event_ids,
            action,
            target_memory_id,
        }))
    }
}

fn parse_l1_kind(value: &str) -> Result<MemoryKind> {
    match value.to_ascii_lowercase().as_str() {
        "preference" | "persona" => Ok(MemoryKind::Preference),
        "fact" | "episodic" => Ok(MemoryKind::Fact),
        "decision" => Ok(MemoryKind::Decision),
        "recipe" | "instruction" => Ok(MemoryKind::Recipe),
        "warning" => Ok(MemoryKind::Warning),
        other => bail!("unsupported L1 memory kind: {other}"),
    }
}

fn source_event_ids(
    value: Option<&Value>,
    allowed_event_ids: &BTreeSet<String>,
) -> Result<Vec<String>> {
    let ids = value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let ids = if ids.is_empty() {
        allowed_event_ids.clone()
    } else {
        ids
    };
    let unknown = ids
        .iter()
        .filter(|id| !allowed_event_ids.is_empty() && !allowed_event_ids.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        bail!("L1 candidate referenced unknown source_event_ids: {unknown:?}");
    }
    Ok(ids.into_iter().collect())
}

fn exact_active_duplicate(
    db_path: &Path,
    project_root: &str,
    kind: MemoryKind,
    content: &str,
) -> Result<Option<Value>> {
    let conn = Connection::open(db_path)?;
    conn.query_row(
        "SELECT id, subject
         FROM memories
         WHERE scope = 'project'
           AND scope_key = ?1
           AND kind = ?2
           AND status = 'active'
           AND content = ?3
         LIMIT 1",
        params![project_root, kind.as_str(), content],
        |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "subject": row.get::<_, String>(1)?,
            }))
        },
    )
    .optional()
    .map_err(Into::into)
}

fn similar_active_memories(
    db_path: &Path,
    scope: &ResolvedMemoryScope,
    candidate: &L1Candidate,
) -> Result<Vec<Value>> {
    let query = format!("{}\n{}", candidate.subject, candidate.content);
    let results = recall(db_path, &query, std::slice::from_ref(scope), 3).unwrap_or_default();
    Ok(results
        .into_iter()
        .filter_map(|item| item.get("memory").cloned())
        .map(|memory| {
            json!({
                "id": memory.get("id"),
                "kind": memory.get("kind"),
                "subject": memory.get("subject"),
                "status": memory.get("status"),
            })
        })
        .collect())
}

fn update_memory_metadata(db_path: &Path, memory_id: &str, metadata: &Value) -> Result<()> {
    let conn = Connection::open(db_path)?;
    let metadata_json = serde_json::to_string(metadata)?;
    conn.execute(
        "UPDATE memories SET metadata_json = ?1, updated_at = ?2 WHERE id = ?3",
        params![metadata_json, timestamp(), memory_id],
    )?;
    Ok(())
}

fn insert_memory_evidence(
    db_path: &Path,
    memory_id: &str,
    source_event_ids: &[String],
    content: &str,
) -> Result<()> {
    let conn = Connection::open(db_path)?;
    let now = timestamp();
    let excerpt = truncate_chars(content, 512);
    for source_id in source_event_ids {
        let id = stable_id(&format!(
            "memory-evidence:{memory_id}:hook_event:{source_id}"
        ));
        conn.execute(
            "INSERT OR REPLACE INTO memory_evidence(
                id, memory_id, created_at, source_type, source_id, role, excerpt
             )
             VALUES(?1, ?2, ?3, 'hook_event', ?4, 'source', ?5)",
            params![id, memory_id, now, source_id, excerpt],
        )?;
    }
    Ok(())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}
