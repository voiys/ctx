use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::sanitize::sanitize_text;
use crate::storage::ensure_db;
use crate::util::{stable_id, timestamp};

pub(crate) const L2_SCENE_JOB_KIND: &str = "l2_scene";

const MAX_L2_MEMORIES: usize = 100;

pub(crate) fn l2_scene_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["scenes"],
        "properties": {
            "scenes": { "type": "array" }
        }
    })
}

pub(crate) fn l2_prompt_rules() -> &'static str {
    "L2 scene brief rules:
- Group related accepted L1 memories into compact scene briefs.
- Return scenes with scene_name, summary, heat, body_markdown, source_memory_ids, and action.
- Use action create, update, merge, or archive. Use create for new scene briefs.
- Keep body_markdown concise and source-backed; do not invent unsupported preferences or facts.
- Do not write files or storage directly; ctx will validate and materialize the scene briefs."
}

pub(crate) fn recent_l1_memory_evidence(
    db_path: &Path,
    project_root: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    ensure_db(db_path)?;
    if limit == 0 {
        bail!("l2 memory evidence limit must be greater than zero");
    }
    let limit = limit.min(MAX_L2_MEMORIES) as i64;
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, updated_at, kind, subject, trigger, content, tags_json, metadata_json
         FROM memories
         WHERE scope = 'project' AND scope_key = ?1 AND status = 'active'
         ORDER BY updated_at DESC, id DESC
         LIMIT ?2",
    )?;
    let mut rows = stmt
        .query_map(params![project_root, limit], row_l1_memory_evidence)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.reverse();
    Ok(rows)
}

pub(crate) fn apply_l2_scene_result(
    db_path: &Path,
    project_root: &str,
    job_id: &str,
    evidence: &Value,
    result: &Value,
) -> Result<Value> {
    ensure_db(db_path)?;
    let scenes = scene_array(result)?;
    let allowed_memory_ids = evidence_memory_ids(evidence);
    let mut materialized = Vec::new();
    let mut skipped = Vec::new();
    for (index, scene) in scenes.iter().enumerate() {
        let parsed = match L2Scene::parse(scene, &allowed_memory_ids) {
            Ok(parsed) => parsed,
            Err(error) => {
                return Err(error)
                    .map_err(|error| anyhow!("invalid L2 scene at index {index}: {error}"));
            }
        };
        if parsed.action == "archive" {
            let archived = archive_scene(db_path, project_root, &parsed.scene_name)?;
            skipped.push(json!({
                "scene_index": index,
                "decision": "archive",
                "scene_name": parsed.scene_name,
                "archived": archived,
            }));
            continue;
        }
        let scene = upsert_scene_brief(db_path, project_root, job_id, &parsed)?;
        materialized.push(json!({
            "scene_index": index,
            "decision": parsed.action,
            "scene": scene,
        }));
    }
    Ok(json!({
        "kind": L2_SCENE_JOB_KIND,
        "scene_count": scenes.len(),
        "materialized_count": materialized.len(),
        "skipped_count": skipped.len(),
        "scenes": materialized,
        "skipped": skipped,
    }))
}

fn row_l1_memory_evidence(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let tags_json: String = row.get(6)?;
    let metadata_json: String = row.get(7)?;
    Ok(json!({
        "memory_id": row.get::<_, String>(0)?,
        "updated_at": row.get::<_, String>(1)?,
        "kind": row.get::<_, String>(2)?,
        "subject": row.get::<_, String>(3)?,
        "trigger": row.get::<_, Option<String>>(4)?,
        "content": row.get::<_, String>(5)?,
        "tags": serde_json::from_str::<Value>(&tags_json).unwrap_or(Value::Array(Vec::new())),
        "metadata": serde_json::from_str::<Value>(&metadata_json).unwrap_or(Value::Object(Default::default())),
    }))
}

fn scene_array(result: &Value) -> Result<&Vec<Value>> {
    result
        .get("scenes")
        .or_else(|| result.get("scene_briefs"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("L2 result must include a scenes array"))
}

fn evidence_memory_ids(evidence: &Value) -> BTreeSet<String> {
    evidence
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|memory| memory.get("memory_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

#[derive(Debug)]
struct L2Scene {
    scene_name: String,
    summary: String,
    heat: f64,
    body_markdown: String,
    source_memory_ids: Vec<String>,
    action: String,
}

impl L2Scene {
    fn parse(value: &Value, allowed_memory_ids: &BTreeSet<String>) -> Result<Self> {
        let Some(object) = value.as_object() else {
            bail!("scene must be an object");
        };
        let scene_name = required_string(object.get("scene_name"), "scene_name")?;
        let summary = required_string(object.get("summary"), "summary")?;
        let body_markdown = object
            .get("body_markdown")
            .or_else(|| object.get("markdown"))
            .and_then(Value::as_str)
            .map(sanitize_text)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| summary.clone());
        let heat = object.get("heat").and_then(Value::as_f64).unwrap_or(0.0);
        let action = object
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("create")
            .to_ascii_lowercase();
        if !matches!(action.as_str(), "create" | "update" | "merge" | "archive") {
            bail!("unsupported L2 scene action: {action}");
        }
        let source_memory_ids =
            source_memory_ids(object.get("source_memory_ids"), allowed_memory_ids)?;
        Ok(Self {
            scene_name,
            summary,
            heat,
            body_markdown,
            source_memory_ids,
            action,
        })
    }
}

fn required_string(value: Option<&Value>, field: &str) -> Result<String> {
    value
        .and_then(Value::as_str)
        .map(sanitize_text)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("scene {field} is required"))
}

fn source_memory_ids(
    value: Option<&Value>,
    allowed_memory_ids: &BTreeSet<String>,
) -> Result<Vec<String>> {
    let ids = value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let ids = if ids.is_empty() {
        allowed_memory_ids.clone()
    } else {
        ids
    };
    let unknown = ids
        .iter()
        .filter(|id| !allowed_memory_ids.is_empty() && !allowed_memory_ids.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        bail!("L2 scene referenced unknown source_memory_ids: {unknown:?}");
    }
    Ok(ids.into_iter().collect())
}

fn upsert_scene_brief(
    db_path: &Path,
    project_root: &str,
    job_id: &str,
    scene: &L2Scene,
) -> Result<Value> {
    let conn = Connection::open(db_path)?;
    let now = timestamp();
    let id = stable_id(&format!(
        "scene-brief:{project_root}:{}",
        scene.scene_name.to_ascii_lowercase()
    ));
    let source_memory_ids_json = serde_json::to_string(&scene.source_memory_ids)?;
    let metadata_json = serde_json::to_string(&json!({
        "layer": "L2",
        "job_id": job_id,
        "action": scene.action,
    }))?;
    conn.execute(
        "INSERT INTO scene_briefs(
            id, created_at, updated_at, project_root, scene_name, summary, heat,
            body_markdown, source_memory_ids_json, status, metadata_json
         )
         VALUES(?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'active', ?9)
         ON CONFLICT(id) DO UPDATE SET
            updated_at = excluded.updated_at,
            scene_name = excluded.scene_name,
            summary = excluded.summary,
            heat = excluded.heat,
            body_markdown = excluded.body_markdown,
            source_memory_ids_json = excluded.source_memory_ids_json,
            status = 'active',
            metadata_json = excluded.metadata_json",
        params![
            id,
            now,
            project_root,
            scene.scene_name,
            scene.summary,
            scene.heat,
            scene.body_markdown,
            source_memory_ids_json,
            metadata_json,
        ],
    )?;
    Ok(json!({
        "id": id,
        "scene_name": scene.scene_name,
        "summary": scene.summary,
        "heat": scene.heat,
        "source_memory_ids": scene.source_memory_ids,
        "status": "active",
    }))
}

fn archive_scene(db_path: &Path, project_root: &str, scene_name: &str) -> Result<bool> {
    let conn = Connection::open(db_path)?;
    let id = stable_id(&format!(
        "scene-brief:{project_root}:{}",
        scene_name.to_ascii_lowercase()
    ));
    let changed = conn.execute(
        "UPDATE scene_briefs
         SET status = 'archived', updated_at = ?1
         WHERE id = ?2 AND project_root = ?3",
        params![timestamp(), id, project_root],
    )?;
    Ok(changed > 0)
}
