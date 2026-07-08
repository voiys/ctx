use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::sanitize::sanitize_text;
use crate::storage::ensure_db;
use crate::util::{stable_id, timestamp};

pub(crate) const L3_PROFILE_JOB_KIND: &str = "l3_profile";

const MAX_L3_SCENES: usize = 50;

pub(crate) fn l3_profile_result_schema() -> Value {
    json!({
        "type": "object",
        "required": ["profile_markdown", "source_scene_ids"],
        "properties": {
            "summary": { "type": "string" },
            "profile_markdown": { "type": "string" },
            "source_scene_ids": { "type": "array" }
        }
    })
}

pub(crate) fn l3_prompt_rules() -> &'static str {
    "L3 profile rules:
- Build a compact profile from active L2 scene briefs only.
- Preserve uncertainty and provenance; include source_scene_ids for every profile revision.
- Summarize stable operating preferences, repeated workflows, warnings, and project-specific facts.
- Do not override live repository evidence or claim unverified current state.
- Do not write storage directly; ctx will validate and save the profile revision."
}

pub(crate) fn recent_l2_scene_evidence(
    db_path: &Path,
    project_root: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    ensure_db(db_path)?;
    if limit == 0 {
        bail!("l3 scene evidence limit must be greater than zero");
    }
    let limit = limit.min(MAX_L3_SCENES) as i64;
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, updated_at, scene_name, summary, heat, body_markdown, source_memory_ids_json
         FROM scene_briefs
         WHERE project_root = ?1 AND status = 'active'
         ORDER BY updated_at DESC, id DESC
         LIMIT ?2",
    )?;
    let mut rows = stmt
        .query_map(params![project_root, limit], row_l2_scene_evidence)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.reverse();
    Ok(rows)
}

pub(crate) fn apply_l3_profile_result(
    db_path: &Path,
    project_root: &str,
    job_id: &str,
    evidence: &Value,
    result: &Value,
) -> Result<Value> {
    ensure_db(db_path)?;
    let allowed_scene_ids = evidence_scene_ids(evidence);
    let source_scene_ids = source_scene_ids(result.get("source_scene_ids"), &allowed_scene_ids)?;
    let profile_markdown = result
        .get("profile_markdown")
        .and_then(Value::as_str)
        .map(sanitize_text)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("L3 result profile_markdown is required"))?;
    let summary = result
        .get("summary")
        .and_then(Value::as_str)
        .map(sanitize_text)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| first_line(&profile_markdown));
    let profile = insert_profile_revision(
        db_path,
        project_root,
        job_id,
        &summary,
        &profile_markdown,
        &source_scene_ids,
    )?;
    Ok(json!({
        "kind": L3_PROFILE_JOB_KIND,
        "profile": profile,
        "source_scene_ids": source_scene_ids,
    }))
}

fn row_l2_scene_evidence(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let source_memory_ids_json: String = row.get(6)?;
    Ok(json!({
        "scene_id": row.get::<_, String>(0)?,
        "updated_at": row.get::<_, String>(1)?,
        "scene_name": row.get::<_, String>(2)?,
        "summary": row.get::<_, String>(3)?,
        "heat": row.get::<_, f64>(4)?,
        "body_markdown": row.get::<_, String>(5)?,
        "source_memory_ids": serde_json::from_str::<Value>(&source_memory_ids_json).unwrap_or(Value::Array(Vec::new())),
    }))
}

fn evidence_scene_ids(evidence: &Value) -> BTreeSet<String> {
    evidence
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|scene| scene.get("scene_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn source_scene_ids(
    value: Option<&Value>,
    allowed_scene_ids: &BTreeSet<String>,
) -> Result<Vec<String>> {
    let ids = value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let ids = if ids.is_empty() {
        allowed_scene_ids.clone()
    } else {
        ids
    };
    let unknown = ids
        .iter()
        .filter(|id| !allowed_scene_ids.is_empty() && !allowed_scene_ids.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        bail!("L3 profile referenced unknown source_scene_ids: {unknown:?}");
    }
    Ok(ids.into_iter().collect())
}

fn insert_profile_revision(
    db_path: &Path,
    project_root: &str,
    job_id: &str,
    summary: &str,
    profile_markdown: &str,
    source_scene_ids: &[String],
) -> Result<Value> {
    let conn = Connection::open(db_path)?;
    let version = next_profile_version(&conn, project_root)?;
    let now = timestamp();
    let source_scene_ids_json = serde_json::to_string(source_scene_ids)?;
    let id = stable_id(&format!(
        "persona-profile:{project_root}:{version}:{now}:{}",
        stable_id(profile_markdown)
    ));
    let metadata_json = serde_json::to_string(&json!({
        "layer": "L3",
        "job_id": job_id,
    }))?;
    conn.execute(
        "INSERT INTO persona_profiles(
            id, created_at, updated_at, project_root, version, summary,
            profile_markdown, source_scene_ids_json, status, metadata_json
         )
         VALUES(?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, 'active', ?8)",
        params![
            id,
            now,
            project_root,
            version,
            summary,
            profile_markdown,
            source_scene_ids_json,
            metadata_json,
        ],
    )?;
    Ok(json!({
        "id": id,
        "version": version,
        "summary": summary,
        "status": "active",
    }))
}

fn next_profile_version(conn: &Connection, project_root: &str) -> Result<i64> {
    let version = conn
        .query_row(
            "SELECT MAX(version) FROM persona_profiles WHERE project_root = ?1",
            params![project_root],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten()
        .unwrap_or(0)
        + 1;
    Ok(version)
}

fn first_line(value: &str) -> String {
    value
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Profile revision")
        .trim()
        .to_string()
}
