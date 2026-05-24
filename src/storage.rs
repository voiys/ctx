use std::fs;
use std::path::Path;

use std::collections::BTreeSet;

use anyhow::{Result, anyhow, bail};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

use crate::embeddings::{EmbeddingBackend, EmbeddingService};
use crate::models::{Resource, ResourceKind, SnapshotMetadata, SnapshotPage};
use crate::util::{kind_str, path_size_bytes};

pub(crate) fn ensure_db(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS resources (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            kind TEXT NOT NULL,
            url TEXT NOT NULL,
            current TEXT NOT NULL,
            local_path TEXT,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS snapshots (
            resource_id TEXT NOT NULL,
            snapshot_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            source_url TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            fetched_at TEXT NOT NULL,
            page_count INTEGER NOT NULL,
            path TEXT NOT NULL,
            PRIMARY KEY(resource_id, snapshot_id)
        );
        CREATE TABLE IF NOT EXISTS chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            resource_id TEXT NOT NULL,
            snapshot_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            label TEXT NOT NULL,
            source_url TEXT NOT NULL,
            chunk_index INTEGER NOT NULL,
            content TEXT NOT NULL,
            embedding BLOB
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
            content,
            resource_id UNINDEXED,
            snapshot_id UNINDEXED,
            label UNINDEXED
        );
        ",
    )?;
    ensure_embedding_column(&conn)?;
    Ok(())
}

fn ensure_embedding_column(conn: &Connection) -> Result<()> {
    let has_embedding = conn
        .prepare("PRAGMA table_info(chunks)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == "embedding");
    if !has_embedding {
        conn.execute("ALTER TABLE chunks ADD COLUMN embedding BLOB", [])?;
    }
    Ok(())
}

pub(crate) fn index_snapshot(
    db_path: &Path,
    resource: &Resource,
    snapshot: &SnapshotMetadata,
) -> Result<()> {
    let pages_path = Path::new(&snapshot.path).join("pages.json");
    let pages = if pages_path.exists() {
        serde_json::from_str::<Vec<SnapshotPage>>(&fs::read_to_string(pages_path)?)?
    } else {
        vec![SnapshotPage {
            url: snapshot.source_url.clone(),
            content: fs::read_to_string(Path::new(&snapshot.path).join("content.txt"))?,
        }]
    };
    index_pages(db_path, resource, &snapshot.snapshot_id, &pages)
}

fn index_pages(
    db_path: &Path,
    resource: &Resource,
    snapshot_id: &str,
    pages: &[SnapshotPage],
) -> Result<()> {
    ensure_db(db_path)?;
    let mut conn = Connection::open(db_path)?;
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM chunks WHERE resource_id = ?1 AND snapshot_id = ?2",
        params![resource.id, snapshot_id],
    )?;
    tx.execute(
        "DELETE FROM chunks_fts WHERE resource_id = ?1 AND snapshot_id = ?2",
        params![resource.id, snapshot_id],
    )?;

    let mut chunks = pages
        .par_iter()
        .enumerate()
        .flat_map(|(page_index, page)| {
            chunk_text(&page.content, 2_400)
                .into_iter()
                .enumerate()
                .map(move |(chunk_index, content)| {
                    (page_index, chunk_index, page.url.clone(), content)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    chunks.sort_by_key(|(page_index, chunk_index, _, _)| (*page_index, *chunk_index));

    let contents = chunks
        .iter()
        .map(|(_, _, _, content)| content.clone())
        .collect::<Vec<_>>();
    let mut embedding_backend = EmbeddingService::from_env(db_path)?;
    let embeddings = embedding_backend.embed_passages(&contents)?;

    for (global_index, ((_, _, source_url, content), embedding)) in
        chunks.iter().zip(embeddings.iter()).enumerate()
    {
        tx.execute(
            "INSERT INTO chunks(resource_id, snapshot_id, kind, label, source_url, chunk_index, content, embedding)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                resource.id,
                snapshot_id,
                kind_str(resource.kind),
                resource.label,
                source_url,
                global_index as i64,
                content,
                embedding
            ],
        )?;
        let rowid = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO chunks_fts(rowid, content, resource_id, snapshot_id, label)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![rowid, content, resource.id, snapshot_id, resource.label],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn upsert_global_resource(
    db_path: &Path,
    resource: &Resource,
    snapshot: Option<&SnapshotMetadata>,
) -> Result<()> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    conn.execute(
        "INSERT INTO resources(id, label, kind, url, current, local_path, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            label = excluded.label,
            kind = excluded.kind,
            url = excluded.url,
            current = excluded.current,
            local_path = excluded.local_path,
            updated_at = excluded.updated_at",
        params![
            resource.id,
            resource.label,
            kind_str(resource.kind),
            resource.url,
            resource.current,
            resource.local_path,
            resource.updated_at
        ],
    )?;
    if let Some(snapshot) = snapshot {
        conn.execute(
            "INSERT OR REPLACE INTO snapshots(resource_id, snapshot_id, kind, source_url, content_hash, fetched_at, page_count, path)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                resource.id,
                snapshot.snapshot_id,
                kind_str(resource.kind),
                snapshot.source_url,
                snapshot.content_hash,
                snapshot.fetched_at,
                snapshot.page_count as i64,
                snapshot.path,
            ],
        )?;
    }
    Ok(())
}

pub(crate) fn find_global_resource(db_path: &Path, target: &str) -> Result<Resource> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let resource = conn
        .query_row(
            "SELECT id, label, kind, url, current, local_path, updated_at
             FROM resources
             WHERE label = ?1 OR url = ?1 OR id = ?1",
            params![target],
            resource_from_row,
        )
        .optional()?
        .ok_or_else(|| anyhow!("resource not found: {target}"))?;
    Ok(resource)
}

pub(crate) fn allowed_global_resource_ids(
    db_path: &Path,
    label: Option<&str>,
    kind: Option<ResourceKind>,
) -> Result<BTreeSet<String>> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare("SELECT id, label, kind FROM resources")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut out = BTreeSet::new();
    for row in rows {
        let (id, resource_label, resource_kind) = row?;
        let Some(parsed_kind) = parse_kind(&resource_kind) else {
            continue;
        };
        if parsed_kind == ResourceKind::Source {
            continue;
        }
        if let Some(label) = label
            && resource_label != label
        {
            continue;
        }
        if let Some(kind) = kind
            && parsed_kind != kind
        {
            continue;
        }
        out.insert(id);
    }
    if label.is_some() && out.is_empty() {
        bail!("no queryable docs/notes resource matched label");
    }
    Ok(out)
}

pub(crate) fn list_global_resources(
    db_path: &Path,
    kind: Option<ResourceKind>,
) -> Result<Vec<serde_json::Value>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT r.id, r.label, r.kind, r.url, r.current, r.local_path, r.updated_at,
                COUNT(s.snapshot_id) AS snapshot_count,
                MAX(CASE WHEN s.snapshot_id = r.current THEN s.page_count END) AS current_page_count,
                MAX(CASE WHEN s.snapshot_id = r.current THEN s.content_hash END) AS current_content_hash
         FROM resources r
         LEFT JOIN snapshots s ON s.resource_id = r.id
         GROUP BY r.id
         ORDER BY r.kind ASC, r.label ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "label": row.get::<_, String>(1)?,
            "kind": row.get::<_, String>(2)?,
            "url": row.get::<_, String>(3)?,
            "current": row.get::<_, String>(4)?,
            "local_path": row.get::<_, Option<String>>(5)?,
            "updated_at": row.get::<_, String>(6)?,
            "snapshot_count": row.get::<_, i64>(7)?,
            "current_page_count": row.get::<_, Option<i64>>(8)?,
            "current_content_hash": row.get::<_, Option<String>>(9)?,
        }))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let row = row?;
        if let Some(kind) = kind
            && row["kind"].as_str() != Some(kind_str(kind))
        {
            continue;
        }
        let mut row = row;
        if let Some(object) = row.as_object_mut() {
            let size = object
                .get("local_path")
                .and_then(|value| value.as_str())
                .map(Path::new)
                .and_then(|path| path_size_bytes(path).ok());
            object.insert("size_bytes".to_string(), json!(size));
        }
        out.push(row);
    }
    Ok(out)
}

pub(crate) fn list_global_resource_models(
    db_path: &Path,
    kind: Option<ResourceKind>,
) -> Result<Vec<Resource>> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, label, kind, url, current, local_path, updated_at
         FROM resources
         ORDER BY kind ASC, label ASC",
    )?;
    let rows = stmt.query_map([], resource_from_row)?;
    let mut out = Vec::new();
    for row in rows {
        let resource = row?;
        if let Some(kind) = kind
            && resource.kind != kind
        {
            continue;
        }
        out.push(resource);
    }
    Ok(out)
}

pub(crate) fn snapshots_for_resources(
    db_path: &Path,
    resources: &[Resource],
) -> Result<Vec<serde_json::Value>> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let mut out = Vec::new();
    for resource in resources {
        let mut stmt = conn.prepare(
            "SELECT snapshot_id, kind, source_url, content_hash, fetched_at, page_count, path
             FROM snapshots
             WHERE resource_id = ?1
             ORDER BY fetched_at DESC",
        )?;
        let rows = stmt.query_map(params![resource.id], |row| {
            Ok(json!({
                "resource_id": resource.id,
                "label": resource.label,
                "snapshot_id": row.get::<_, String>(0)?,
                "kind": row.get::<_, String>(1)?,
                "source_url": row.get::<_, String>(2)?,
                "content_hash": row.get::<_, String>(3)?,
                "fetched_at": row.get::<_, String>(4)?,
                "page_count": row.get::<_, i64>(5)?,
                "path": row.get::<_, String>(6)?,
            }))
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

pub(crate) fn current_content_hash(
    db_path: &Path,
    resource_id: &str,
    snapshot_id: &str,
) -> Result<Option<String>> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let hash = conn
        .query_row(
            "SELECT content_hash FROM snapshots WHERE resource_id = ?1 AND snapshot_id = ?2",
            params![resource_id, snapshot_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(hash)
}

pub(crate) fn snapshot_path_for_pointer(
    db_path: &Path,
    resource_id: &str,
    snapshot_id: &str,
) -> Result<Option<String>> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let path = conn
        .query_row(
            "SELECT path FROM snapshots WHERE resource_id = ?1 AND snapshot_id = ?2",
            params![resource_id, snapshot_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(path)
}

pub(crate) fn prune_resource_cache(db_path: &Path, resource: &Resource) -> Result<bool> {
    remove_global_resource(db_path, resource, true)
}

pub(crate) fn remove_global_resource(
    db_path: &Path,
    resource: &Resource,
    prune_files: bool,
) -> Result<bool> {
    ensure_db(db_path)?;
    if prune_files && let Some(path) = &resource.local_path {
        let path = Path::new(path);
        if path.exists() {
            let _ = fs::remove_dir_all(path);
        }
        if let Some(parent) = path.parent()
            && parent.exists()
            && fs::read_dir(parent)?.next().is_none()
        {
            let _ = fs::remove_dir(parent);
        }
    }
    let conn = Connection::open(db_path)?;
    conn.execute(
        "DELETE FROM chunks_fts WHERE resource_id = ?1",
        params![resource.id],
    )?;
    conn.execute(
        "DELETE FROM chunks WHERE resource_id = ?1",
        params![resource.id],
    )?;
    conn.execute(
        "DELETE FROM snapshots WHERE resource_id = ?1",
        params![resource.id],
    )?;
    conn.execute("DELETE FROM resources WHERE id = ?1", params![resource.id])?;
    Ok(true)
}

fn resource_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Resource> {
    let kind = parse_kind(&row.get::<_, String>(2)?).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(2, "kind".to_string(), rusqlite::types::Type::Text)
    })?;
    let updated_at = row.get::<_, String>(6)?;
    Ok(Resource {
        id: row.get(0)?,
        label: row.get(1)?,
        kind,
        url: row.get(3)?,
        reason: None,
        current: row.get(4)?,
        local_path: row.get(5)?,
        created_at: updated_at.clone(),
        updated_at,
    })
}

fn parse_kind(value: &str) -> Option<ResourceKind> {
    match value {
        "source" => Some(ResourceKind::Source),
        "docs" => Some(ResourceKind::Docs),
        "notes" => Some(ResourceKind::Notes),
        _ => None,
    }
}

fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for paragraph in text.split("\n\n") {
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        if current.len() + paragraph.len() + 2 > max_chars && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current.clear();
        }
        current.push_str(paragraph);
        current.push_str("\n\n");
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }
    chunks
}
