use std::path::Path;

use anyhow::{Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};

use crate::l1::{L1_EXTRACT_JOB_KIND, apply_l1_extract_result, l1_prompt_rules};
use crate::l2::{L2_SCENE_JOB_KIND, apply_l2_scene_result, l2_prompt_rules};
use crate::storage::ensure_db;
use crate::util::{content_hash, stable_id, timestamp};

#[derive(Debug)]
pub(crate) struct MemoryJobInput {
    pub(crate) project_root: String,
    pub(crate) session_id: Option<String>,
    pub(crate) kind: String,
    pub(crate) objective: String,
    pub(crate) evidence: Value,
    pub(crate) result_schema: Value,
}

#[derive(Clone, Debug)]
pub(crate) struct MemoryJobRecord {
    id: String,
    created_at: String,
    updated_at: String,
    project_root: String,
    session_id: Option<String>,
    kind: String,
    status: String,
    objective: String,
    evidence: Value,
    result_schema: Value,
    leased_at: Option<String>,
    lease_owner: Option<String>,
    attempts: i64,
    result: Option<Value>,
    error: Option<String>,
    metadata: Value,
}

pub(crate) fn enqueue_memory_job(
    db_path: &Path,
    input: MemoryJobInput,
) -> Result<serde_json::Value> {
    ensure_db(db_path)?;
    if input.kind.trim().is_empty() {
        bail!("memory job kind cannot be empty");
    }
    if input.objective.trim().is_empty() {
        bail!("memory job objective cannot be empty");
    }
    if !input.result_schema.is_object() {
        bail!("memory job result schema must be a JSON object");
    }
    let now = timestamp();
    let evidence_json = serde_json::to_string(&input.evidence)?;
    let result_schema_json = serde_json::to_string(&input.result_schema)?;
    let conn = Connection::open(db_path)?;
    let id_seed = format!(
        "memory-job:{}:{}:{}:{}:{}",
        input.project_root,
        input.kind,
        now,
        input.objective,
        content_hash(&format!("{evidence_json}:{result_schema_json}"))
    );
    let id = available_memory_job_id(&conn, &id_seed)?;
    conn.execute(
        "INSERT INTO memory_jobs(
            id, created_at, updated_at, project_root, session_id, kind, status, objective,
            evidence_json, result_schema_json
         )
         VALUES(?1, ?2, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8)",
        params![
            id,
            now,
            input.project_root,
            input.session_id,
            input.kind,
            input.objective,
            evidence_json,
            result_schema_json,
        ],
    )?;
    let job = find_memory_job(&conn, &input.project_root, &id)?;
    Ok(json!({
        "job": job_json(&job),
    }))
}

pub(crate) fn claim_next_memory_job(
    db_path: &Path,
    project_root: &str,
    lease_owner: Option<&str>,
) -> Result<Option<MemoryJobRecord>> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let Some(id) = conn
        .query_row(
            "SELECT id
             FROM memory_jobs
             WHERE project_root = ?1 AND status = 'pending'
             ORDER BY created_at ASC, id ASC
             LIMIT 1",
            params![project_root],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let now = timestamp();
    conn.execute(
        "UPDATE memory_jobs
         SET status = 'running', updated_at = ?1, leased_at = ?1, lease_owner = ?2,
             attempts = attempts + 1
         WHERE id = ?3 AND project_root = ?4 AND status = 'pending'",
        params![now, lease_owner, id, project_root],
    )?;
    Ok(Some(find_memory_job(&conn, project_root, &id)?))
}

pub(crate) fn memory_job_prompt(
    db_path: &Path,
    project_root: &str,
    id: &str,
) -> Result<serde_json::Value> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let job = find_memory_job(&conn, project_root, id)?;
    Ok(json!({
        "job": job_json(&job),
        "prompt": prompt_for_job(&job)?,
        "result_schema": job.result_schema,
    }))
}

pub(crate) fn apply_memory_job_result(
    db_path: &Path,
    project_root: &str,
    id: &str,
    result: Value,
) -> Result<serde_json::Value> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let job = find_memory_job(&conn, project_root, id)?;
    if job.status == "done" {
        bail!("memory job already applied: {id}");
    }
    validate_result_against_schema(&job.result_schema, &result)?;
    let applied = match job.kind.as_str() {
        L1_EXTRACT_JOB_KIND => Some(apply_l1_extract_result(
            db_path,
            project_root,
            id,
            &job.evidence,
            &result,
        )?),
        L2_SCENE_JOB_KIND => Some(apply_l2_scene_result(
            db_path,
            project_root,
            id,
            &job.evidence,
            &result,
        )?),
        _ => None,
    };
    let now = timestamp();
    let stored_result = if let Some(applied) = &applied {
        json!({
            "submitted": result,
            "applied": applied,
        })
    } else {
        result
    };
    let result_json = serde_json::to_string(&stored_result)?;
    conn.execute(
        "UPDATE memory_jobs
         SET status = 'done', updated_at = ?1, result_json = ?2, error = NULL
         WHERE id = ?3 AND project_root = ?4",
        params![now, result_json, id, project_root],
    )?;
    let job = find_memory_job(&conn, project_root, id)?;
    Ok(json!({
        "job": job_json(&job),
        "applied": applied,
    }))
}

pub(crate) fn job_json(job: &MemoryJobRecord) -> serde_json::Value {
    json!({
        "id": job.id,
        "created_at": job.created_at,
        "updated_at": job.updated_at,
        "project_root": job.project_root,
        "session_id": job.session_id,
        "kind": job.kind,
        "status": job.status,
        "objective": job.objective,
        "evidence": job.evidence,
        "result_schema": job.result_schema,
        "leased_at": job.leased_at,
        "lease_owner": job.lease_owner,
        "attempts": job.attempts,
        "result": job.result,
        "error": job.error,
        "metadata": job.metadata,
    })
}

fn available_memory_job_id(conn: &Connection, seed: &str) -> Result<String> {
    for sequence in 0..1000 {
        let candidate = stable_id(&format!("{seed}:{sequence}"));
        let exists = conn
            .query_row(
                "SELECT 1 FROM memory_jobs WHERE id = ?1 LIMIT 1",
                params![candidate],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Ok(candidate);
        }
    }
    bail!("failed to allocate memory job id");
}

fn find_memory_job(conn: &Connection, project_root: &str, id: &str) -> Result<MemoryJobRecord> {
    conn.query_row(
        "SELECT id, created_at, updated_at, project_root, session_id, kind, status, objective,
                evidence_json, result_schema_json, leased_at, lease_owner, attempts,
                result_json, error, metadata_json
         FROM memory_jobs
         WHERE project_root = ?1 AND id = ?2",
        params![project_root, id],
        row_memory_job,
    )
    .optional()?
    .ok_or_else(|| anyhow!("memory job not found: {id}"))
}

fn row_memory_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryJobRecord> {
    let evidence_json = row.get::<_, String>(8)?;
    let result_schema_json = row.get::<_, String>(9)?;
    let result_json = row.get::<_, Option<String>>(13)?;
    let metadata_json = row.get::<_, String>(15)?;
    Ok(MemoryJobRecord {
        id: row.get(0)?,
        created_at: row.get(1)?,
        updated_at: row.get(2)?,
        project_root: row.get(3)?,
        session_id: row.get(4)?,
        kind: row.get(5)?,
        status: row.get(6)?,
        objective: row.get(7)?,
        evidence: serde_json::from_str(&evidence_json).unwrap_or(Value::Array(Vec::new())),
        result_schema: serde_json::from_str(&result_schema_json)
            .unwrap_or_else(|_| Value::Object(Map::new())),
        leased_at: row.get(10)?,
        lease_owner: row.get(11)?,
        attempts: row.get(12)?,
        result: result_json.and_then(|value| serde_json::from_str(&value).ok()),
        error: row.get(14)?,
        metadata: serde_json::from_str(&metadata_json)
            .unwrap_or_else(|_| Value::Object(Map::new())),
    })
}

fn prompt_for_job(job: &MemoryJobRecord) -> Result<String> {
    let evidence = serde_json::to_string_pretty(&job.evidence)?;
    let schema = serde_json::to_string_pretty(&job.result_schema)?;
    let layer_rules = match job.kind.as_str() {
        L1_EXTRACT_JOB_KIND => format!("\n{}\n", l1_prompt_rules()),
        L2_SCENE_JOB_KIND => format!("\n{}\n", l2_prompt_rules()),
        _ => String::new(),
    };
    Ok(format!(
        r#"You are processing a ctx memory job.

Job id: {id}
Kind: {kind}
Objective: {objective}

Rules:
- Use only the evidence supplied here or evidence you explicitly inspect with ctx commands.
- Return only JSON matching the result schema.
- Do not write ctx storage directly.
- Do not call a background harness or provider endpoint.
{layer_rules}

Evidence:
```json
{evidence}
```

Result schema:
```json
{schema}
```
"#,
        id = job.id,
        kind = job.kind,
        objective = job.objective,
        layer_rules = layer_rules,
    ))
}

fn validate_result_against_schema(schema: &Value, result: &Value) -> Result<()> {
    if schema.get("type").and_then(Value::as_str) == Some("object") && !result.is_object() {
        bail!("memory job result must be a JSON object");
    }
    let Some(result_object) = result.as_object() else {
        return Ok(());
    };
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for field in required.iter().filter_map(Value::as_str) {
            if !result_object.contains_key(field) {
                bail!("memory job result missing required field: {field}");
            }
        }
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (field, property_schema) in properties {
            let Some(value) = result_object.get(field) else {
                continue;
            };
            if let Some(expected_type) = property_schema.get("type").and_then(Value::as_str) {
                validate_json_type(field, expected_type, value)?;
            }
        }
    }
    Ok(())
}

fn validate_json_type(field: &str, expected_type: &str, value: &Value) -> Result<()> {
    let valid = match expected_type {
        "array" => value.is_array(),
        "boolean" => value.is_boolean(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "object" => value.is_object(),
        "string" => value.is_string(),
        _ => true,
    };
    if !valid {
        bail!("memory job result field `{field}` must be {expected_type}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn schema() -> Value {
        json!({
            "type": "object",
            "required": ["candidates"],
            "properties": {
                "candidates": { "type": "array" }
            }
        })
    }

    #[test]
    fn job_lifecycle_claim_prompt_and_apply() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("ctx.db");
        let root = "/repo".to_string();
        let created = enqueue_memory_job(
            &db_path,
            MemoryJobInput {
                project_root: root.clone(),
                session_id: None,
                kind: "test_job".to_string(),
                objective: "Extract memory candidates".to_string(),
                evidence: json!([{"event_id": "evt1"}]),
                result_schema: schema(),
            },
        )
        .unwrap();
        let id = created["job"]["id"].as_str().unwrap().to_string();
        let job = claim_next_memory_job(&db_path, &root, Some("visible-agent"))
            .unwrap()
            .unwrap();
        assert_eq!(job.id, id);
        assert_eq!(job.status, "running");
        assert_eq!(job.attempts, 1);

        let prompt = memory_job_prompt(&db_path, &root, &id).unwrap();
        assert!(
            prompt["prompt"]
                .as_str()
                .unwrap()
                .contains("Return only JSON")
        );

        let applied = apply_memory_job_result(
            &db_path,
            &root,
            &id,
            json!({"candidates": [{"content": "Use cargo test"}]}),
        )
        .unwrap();
        assert_eq!(applied["job"]["status"], "done");
    }

    #[test]
    fn apply_rejects_missing_required_fields() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("ctx.db");
        let root = "/repo".to_string();
        let created = enqueue_memory_job(
            &db_path,
            MemoryJobInput {
                project_root: root.clone(),
                session_id: None,
                kind: "test_job".to_string(),
                objective: "Extract memory candidates".to_string(),
                evidence: json!([]),
                result_schema: schema(),
            },
        )
        .unwrap();
        let id = created["job"]["id"].as_str().unwrap();
        let error = apply_memory_job_result(&db_path, &root, id, json!({})).unwrap_err();
        assert!(error.to_string().contains("missing required field"));
    }

    #[test]
    fn enqueue_allocates_distinct_ids_for_duplicate_jobs() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("ctx.db");
        let root = "/repo".to_string();
        let input = || MemoryJobInput {
            project_root: root.clone(),
            session_id: None,
            kind: "test_job".to_string(),
            objective: "Extract memory candidates".to_string(),
            evidence: json!([]),
            result_schema: schema(),
        };

        let first = enqueue_memory_job(&db_path, input()).unwrap();
        let second = enqueue_memory_job(&db_path, input()).unwrap();

        assert_ne!(first["job"]["id"], second["job"]["id"]);
    }
}
