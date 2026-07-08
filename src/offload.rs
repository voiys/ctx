use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::storage::ensure_db;
use crate::util::{content_hash, stable_id, timestamp};

#[derive(Debug)]
pub(crate) struct OffloadNodeInput {
    pub(crate) project_root: String,
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) summary: Option<String>,
    pub(crate) content: Option<String>,
    pub(crate) media_type: Option<String>,
    pub(crate) parent_node_id: Option<String>,
    pub(crate) edge_kind: Option<String>,
}

pub(crate) fn create_offload_node(
    db_path: &Path,
    home: &Path,
    input: OffloadNodeInput,
) -> Result<Value> {
    ensure_db(db_path)?;
    if input.kind.trim().is_empty() {
        bail!("offload kind cannot be empty");
    }
    if input.title.trim().is_empty() {
        bail!("offload title cannot be empty");
    }
    let now = timestamp();
    let blob = match input.content {
        Some(content) => Some(write_payload_blob(
            db_path,
            home,
            &content,
            input.media_type.as_deref(),
            &now,
        )?),
        None => None,
    };
    let blob_id = blob
        .as_ref()
        .and_then(|blob| blob.get("id").and_then(Value::as_str))
        .map(str::to_string);
    let id = stable_id(&format!(
        "offload-node:{}:{}:{}:{}:{}",
        input.project_root,
        input.kind,
        input.title,
        now,
        blob_id.as_deref().unwrap_or("")
    ));
    let metadata_json = serde_json::to_string(&json!({"layer": "offload"}))?;
    let conn = Connection::open(db_path)?;
    conn.execute(
        "INSERT INTO offload_nodes(
            id, created_at, updated_at, project_root, kind, title, summary, blob_id, metadata_json
         )
         VALUES(?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            now,
            input.project_root,
            input.kind,
            input.title,
            input.summary,
            blob_id,
            metadata_json,
        ],
    )?;
    if let Some(parent_node_id) = input.parent_node_id {
        let edge_kind = input.edge_kind.unwrap_or_else(|| "relates_to".to_string());
        let edge_id = stable_id(&format!("offload-edge:{parent_node_id}:{id}:{edge_kind}"));
        conn.execute(
            "INSERT OR REPLACE INTO offload_edges(
                id, created_at, source_node_id, target_node_id, kind
             )
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![edge_id, now, parent_node_id, id, edge_kind],
        )?;
    }
    let node = find_offload_node(&conn, &input.project_root, &id)?;
    Ok(json!({
        "node": node,
        "blob": blob,
    }))
}

pub(crate) fn show_offload_node(db_path: &Path, project_root: &str, id: &str) -> Result<Value> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let node = find_offload_node(&conn, project_root, id)?;
    let blob = node
        .get("blob_id")
        .and_then(Value::as_str)
        .map(|blob_id| find_payload_blob(&conn, blob_id))
        .transpose()?;
    Ok(json!({
        "node": node,
        "blob": blob,
    }))
}

pub(crate) fn offload_graph(db_path: &Path, project_root: &str) -> Result<Value> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let nodes = offload_nodes(&conn, project_root)?;
    let edges = offload_edges(&conn, project_root)?;
    let mermaid = mermaid_graph(&nodes, &edges);
    Ok(json!({
        "nodes": nodes,
        "edges": edges,
        "mermaid": mermaid,
    }))
}

fn write_payload_blob(
    db_path: &Path,
    home: &Path,
    content: &str,
    media_type: Option<&str>,
    now: &str,
) -> Result<Value> {
    let hash = content_hash(content);
    let dir = home.join("blobs").join(&hash[..2]);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(&hash);
    fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
    let id = stable_id(&format!("payload-blob:{hash}"));
    let conn = Connection::open(db_path)?;
    conn.execute(
        "INSERT INTO payload_blobs(
            id, created_at, content_hash, media_type, size_bytes, path, redaction_version
         )
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, 1)
         ON CONFLICT(id) DO UPDATE SET
            path = excluded.path,
            media_type = excluded.media_type,
            size_bytes = excluded.size_bytes",
        params![
            id,
            now,
            hash,
            media_type,
            content.len() as i64,
            path.display().to_string(),
        ],
    )?;
    Ok(json!({
        "id": id,
        "content_hash": hash,
        "media_type": media_type,
        "size_bytes": content.len(),
        "path": path,
    }))
}

fn find_offload_node(conn: &Connection, project_root: &str, id: &str) -> Result<Value> {
    conn.query_row(
        "SELECT id, created_at, updated_at, project_root, kind, title, summary, blob_id, metadata_json
         FROM offload_nodes
         WHERE id = ?1 AND project_root = ?2",
        params![id, project_root],
        row_offload_node,
    )
    .optional()?
    .ok_or_else(|| anyhow!("offload node not found: {id}"))
}

fn find_payload_blob(conn: &Connection, id: &str) -> Result<Value> {
    conn.query_row(
        "SELECT id, created_at, content_hash, media_type, size_bytes, path, redaction_version, metadata_json
         FROM payload_blobs
         WHERE id = ?1",
        params![id],
        |row| {
            let metadata_json: String = row.get(7)?;
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "created_at": row.get::<_, String>(1)?,
                "content_hash": row.get::<_, String>(2)?,
                "media_type": row.get::<_, Option<String>>(3)?,
                "size_bytes": row.get::<_, i64>(4)?,
                "path": row.get::<_, String>(5)?,
                "redaction_version": row.get::<_, i64>(6)?,
                "metadata": serde_json::from_str::<Value>(&metadata_json).unwrap_or(Value::Object(Default::default())),
            }))
        },
    )
    .optional()?
    .ok_or_else(|| anyhow!("payload blob not found: {id}"))
}

fn offload_nodes(conn: &Connection, project_root: &str) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, created_at, updated_at, project_root, kind, title, summary, blob_id, metadata_json
         FROM offload_nodes
         WHERE project_root = ?1
         ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![project_root], row_offload_node)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn offload_edges(conn: &Connection, project_root: &str) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT e.id, e.created_at, e.source_node_id, e.target_node_id, e.kind
         FROM offload_edges e
         JOIN offload_nodes s ON s.id = e.source_node_id
         JOIN offload_nodes t ON t.id = e.target_node_id
         WHERE s.project_root = ?1 AND t.project_root = ?1
         ORDER BY e.created_at ASC, e.id ASC",
    )?;
    let rows = stmt.query_map(params![project_root], |row| {
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "created_at": row.get::<_, String>(1)?,
            "source_node_id": row.get::<_, String>(2)?,
            "target_node_id": row.get::<_, String>(3)?,
            "kind": row.get::<_, String>(4)?,
        }))
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn row_offload_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let metadata_json: String = row.get(8)?;
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "created_at": row.get::<_, String>(1)?,
        "updated_at": row.get::<_, String>(2)?,
        "project_root": row.get::<_, Option<String>>(3)?,
        "kind": row.get::<_, String>(4)?,
        "title": row.get::<_, String>(5)?,
        "summary": row.get::<_, Option<String>>(6)?,
        "blob_id": row.get::<_, Option<String>>(7)?,
        "metadata": serde_json::from_str::<Value>(&metadata_json).unwrap_or(Value::Object(Default::default())),
    }))
}

fn mermaid_graph(nodes: &[Value], edges: &[Value]) -> String {
    let mut lines = vec!["flowchart TD".to_string()];
    for node in nodes {
        let id = node.get("id").and_then(Value::as_str).unwrap_or("node");
        let title = node
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("offload node");
        lines.push(format!(
            "  {}[\"{}\"]",
            mermaid_id(id),
            escape_mermaid_label(title)
        ));
    }
    for edge in edges {
        let source = edge
            .get("source_node_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let target = edge
            .get("target_node_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let kind = edge.get("kind").and_then(Value::as_str).unwrap_or("edge");
        lines.push(format!(
            "  {} -- {} --> {}",
            mermaid_id(source),
            escape_mermaid_label(kind),
            mermaid_id(target)
        ));
    }
    lines.join("\n")
}

fn mermaid_id(id: &str) -> String {
    format!(
        "n{}",
        id.replace(|ch: char| !ch.is_ascii_alphanumeric(), "_")
    )
}

fn escape_mermaid_label(value: &str) -> String {
    value.replace('"', "'").replace('\n', " ")
}
