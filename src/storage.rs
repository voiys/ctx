use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

use crate::embeddings::{EmbeddingBackend, EmbeddingService};
use crate::markdown::plain_text;
use crate::models::{Resource, ResourceKind, SnapshotMetadata, SnapshotPage};
use crate::util::{content_hash, kind_str, path_size_bytes};

pub(crate) const LEGACY_NOTES_KIND: &str = "notes";

pub(crate) fn ensure_db(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let conn = open_db(path)?;
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
    ensure_chunk_columns(&conn)?;
    let columns = conn
        .prepare("PRAGMA table_info(resources)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if !columns.iter().any(|column| column == "reason") {
        conn.execute("ALTER TABLE resources ADD COLUMN reason TEXT", [])?;
    }
    if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM resources WHERE kind = ?1)",
        params![LEGACY_NOTES_KIND],
        |row| row.get::<_, bool>(0),
    )? {
        conn.execute(
            "DELETE FROM chunks_fts WHERE resource_id IN (SELECT id FROM resources WHERE kind = ?1)",
            params![LEGACY_NOTES_KIND],
        )?;
        conn.execute(
            "DELETE FROM chunks WHERE resource_id IN (SELECT id FROM resources WHERE kind = ?1)",
            params![LEGACY_NOTES_KIND],
        )?;
        conn.execute(
            "DELETE FROM snapshots WHERE resource_id IN (SELECT id FROM resources WHERE kind = ?1)",
            params![LEGACY_NOTES_KIND],
        )?;
        conn.execute(
            "DELETE FROM resources WHERE kind = ?1",
            params![LEGACY_NOTES_KIND],
        )?;
    }
    if let Some(home) = path.parent() {
        let dir = home.join(LEGACY_NOTES_KIND);
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
    }
    conn.execute_batch("DROP TABLE IF EXISTS ledgers")?;
    Ok(())
}

pub(crate) fn open_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    Ok(conn)
}

fn ensure_chunk_columns(conn: &Connection) -> Result<()> {
    let columns = conn
        .prepare("PRAGMA table_info(chunks)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let has_column = |name: &str| columns.iter().any(|column| column == name);
    for (name, definition) in [
        ("embedding", "embedding BLOB"),
        ("section_index", "section_index INTEGER NOT NULL DEFAULT 0"),
        ("heading_path", "heading_path TEXT NOT NULL DEFAULT '[]'"),
        ("heading_level", "heading_level INTEGER NOT NULL DEFAULT 0"),
        ("parent_section_index", "parent_section_index INTEGER"),
        ("previous_section_index", "previous_section_index INTEGER"),
        ("next_section_index", "next_section_index INTEGER"),
        ("anchor", "anchor TEXT"),
        ("plain_text", "plain_text TEXT NOT NULL DEFAULT ''"),
        ("content_hash", "content_hash TEXT NOT NULL DEFAULT ''"),
    ] {
        if !has_column(name) {
            conn.execute(&format!("ALTER TABLE chunks ADD COLUMN {definition}"), [])?;
        }
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
    let mut conn = open_db(db_path)?;
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
            page_chunks(page)
                .into_iter()
                .map(move |chunk| (page_index, page.url.clone(), chunk))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    chunks.sort_by_key(|(page_index, _, chunk)| (*page_index, chunk.chunk_index));

    let search_contents = chunks
        .iter()
        .map(|(_, source_url, chunk)| chunk_search_content(resource, source_url, chunk))
        .collect::<Vec<_>>();
    let mut embedding_backend = EmbeddingService::from_env(db_path)?;
    let embeddings = embedding_backend.embed_passages(&search_contents)?;

    for (global_index, (((_, source_url, chunk), search_content), embedding)) in chunks
        .iter()
        .zip(search_contents.iter())
        .zip(embeddings.iter())
        .enumerate()
    {
        tx.execute(
            "INSERT INTO chunks(
                resource_id, snapshot_id, kind, label, source_url, chunk_index, content, embedding,
                section_index, heading_path, heading_level, parent_section_index, previous_section_index,
                next_section_index, anchor, plain_text, content_hash
             )
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                resource.id,
                snapshot_id,
                kind_str(resource.kind),
                resource.label,
                source_url,
                global_index as i64,
                chunk.content,
                embedding,
                chunk.section_index,
                chunk.heading_path,
                chunk.heading_level,
                chunk.parent_section_index,
                chunk.previous_section_index,
                chunk.next_section_index,
                chunk.anchor,
                chunk.plain_text,
                chunk.content_hash,
            ],
        )?;
        let rowid = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO chunks_fts(rowid, content, resource_id, snapshot_id, label)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![
                rowid,
                search_content,
                resource.id,
                snapshot_id,
                resource.label
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn chunk_search_content(resource: &Resource, source_url: &str, chunk: &IndexedChunk) -> String {
    [
        resource.label.as_str(),
        source_url,
        &heading_path_search_text(&chunk.heading_path),
        chunk.plain_text.as_str(),
        chunk.content.as_str(),
    ]
    .into_iter()
    .filter(|part| !part.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

fn heading_path_search_text(heading_path: &str) -> String {
    serde_json::from_str::<Vec<String>>(heading_path)
        .unwrap_or_default()
        .join(" > ")
}

#[derive(Debug)]
struct IndexedChunk {
    chunk_index: usize,
    section_index: i64,
    heading_path: String,
    heading_level: i64,
    parent_section_index: Option<i64>,
    previous_section_index: Option<i64>,
    next_section_index: Option<i64>,
    anchor: Option<String>,
    content: String,
    plain_text: String,
    content_hash: String,
}

fn page_chunks(page: &SnapshotPage) -> Vec<IndexedChunk> {
    chunk_text(&page.content, 2_400)
        .into_iter()
        .enumerate()
        .map(|(index, content)| {
            let plain_text = plain_text(&content);
            IndexedChunk {
                chunk_index: index,
                section_index: 0,
                heading_path: "[]".to_string(),
                heading_level: 0,
                parent_section_index: None,
                previous_section_index: index.checked_sub(1).map(|value| value as i64),
                next_section_index: None,
                anchor: None,
                content_hash: content_hash(&content),
                plain_text,
                content,
            }
        })
        .collect::<Vec<_>>()
}

pub(crate) fn upsert_global_resource(
    db_path: &Path,
    resource: &Resource,
    snapshot: Option<&SnapshotMetadata>,
) -> Result<()> {
    ensure_db(db_path)?;
    let conn = open_db(db_path)?;
    conn.execute(
        "INSERT INTO resources(id, label, kind, url, current, local_path, updated_at, reason)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
            label = excluded.label,
            kind = excluded.kind,
            url = excluded.url,
            current = excluded.current,
            local_path = excluded.local_path,
            updated_at = excluded.updated_at,
            reason = COALESCE(excluded.reason, resources.reason)",
        params![
            resource.id,
            resource.label,
            kind_str(resource.kind),
            resource.url,
            resource.current,
            resource.local_path,
            resource.updated_at,
            resource.reason
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
    let conn = open_db(db_path)?;
    let resource = conn
        .query_row(
            "SELECT id, label, kind, url, current, local_path, updated_at, reason
             FROM resources
             WHERE label = ?1 OR url = ?1 OR id = ?1
             ORDER BY rowid ASC LIMIT 1",
            params![target],
            resource_from_row,
        )
        .optional()?
        .ok_or_else(|| anyhow!("resource not found: {target}"))?;
    Ok(resource)
}

pub(crate) fn allowed_global_resource_ids(
    db_path: &Path,
    labels: &[String],
    kind: Option<ResourceKind>,
    snapshot: Option<&str>,
) -> Result<BTreeSet<(String, String)>> {
    let resources = list_global_resource_models(db_path, kind)?
        .into_iter()
        .filter(|resource| resource.kind != ResourceKind::Source)
        .collect::<Vec<_>>();
    for label in labels {
        if !resources.iter().any(|resource| &resource.label == label) {
            bail!("no queryable resource matched label: {label}");
        }
    }
    let conn = open_db(db_path)?;
    let mut out = BTreeSet::new();
    for resource in resources {
        if !labels.is_empty() && !labels.contains(&resource.label) {
            continue;
        }
        let pointer = snapshot.unwrap_or(&resource.current);
        let exists = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM snapshots WHERE resource_id = ?1 AND snapshot_id = ?2)",
            params![resource.id, pointer],
            |row| row.get::<_, bool>(0),
        )?;
        if exists {
            out.insert((resource.id, pointer.to_string()));
        }
    }
    if let Some(snapshot) = snapshot
        && out.is_empty()
    {
        bail!("snapshot not found: {snapshot}");
    }
    Ok(out)
}

pub(crate) fn list_global_resources(
    db_path: &Path,
    kind: Option<ResourceKind>,
) -> Result<Vec<serde_json::Value>> {
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT r.id, r.label, r.kind, r.url, r.current, r.local_path, r.updated_at,
                COUNT(s.snapshot_id) AS snapshot_count,
                MAX(CASE WHEN s.snapshot_id = r.current THEN s.page_count END) AS current_page_count,
                MAX(CASE WHEN s.snapshot_id = r.current THEN s.content_hash END) AS current_content_hash,
                r.reason
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
            "reason": row.get::<_, Option<String>>(10)?,
        }))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let row = row?;
        if let Some(kind) = kind
            && row["kind"]
                .as_str()
                .and_then(parse_kind)
                .is_none_or(|parsed_kind| parsed_kind != kind)
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
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, label, kind, url, current, local_path, updated_at, reason
         FROM resources
         ORDER BY rowid ASC",
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
    let conn = open_db(db_path)?;
    let mut out = Vec::new();
    for resource in resources {
        let mut stmt = conn.prepare(
            "SELECT snapshot_id, kind, source_url, content_hash, fetched_at, page_count, path
             FROM snapshots
             WHERE resource_id = ?1
             ORDER BY fetched_at DESC",
        )?;
        let rows = stmt.query_map(params![resource.id], |row| {
            let path = row.get::<_, String>(6)?;
            let extra = snapshot_extra(Path::new(&path)).ok().flatten();
            Ok(json!({
                "resource_id": resource.id,
                "label": resource.label,
                "snapshot_id": row.get::<_, String>(0)?,
                "kind": row.get::<_, String>(1)?,
                "source_url": row.get::<_, String>(2)?,
                "content_hash": row.get::<_, String>(3)?,
                "fetched_at": row.get::<_, String>(4)?,
                "page_count": row.get::<_, i64>(5)?,
                "path": path,
                "extra": extra,
            }))
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

fn snapshot_extra(path: &Path) -> Result<Option<serde_json::Value>> {
    let metadata_path = path.join("snapshot.json");
    if !metadata_path.exists() {
        return Ok(None);
    }
    let value = serde_json::from_str::<serde_json::Value>(&fs::read_to_string(metadata_path)?)?;
    Ok(value.get("extra").cloned())
}

pub(crate) fn current_content_hash(
    db_path: &Path,
    resource_id: &str,
    snapshot_id: &str,
) -> Result<Option<String>> {
    ensure_db(db_path)?;
    let conn = open_db(db_path)?;
    let hash = conn
        .query_row(
            "SELECT content_hash FROM snapshots WHERE resource_id = ?1 AND snapshot_id = ?2",
            params![resource_id, snapshot_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(hash)
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
    let conn = open_db(db_path)?;
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
        reason: row.get(7)?,
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
        "research_paper" | "arxiv" => Some(ResourceKind::ResearchPaper),
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

#[cfg(test)]
mod tests {
    use super::open_db;

    #[test]
    fn shared_connections_use_wal_and_five_second_busy_timeout() {
        let home = tempfile::tempdir().unwrap();
        let conn = open_db(&home.path().join("ctx.db")).unwrap();
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode, "wal");
        assert_eq!(busy_timeout, 5000);
    }
}
