use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

use crate::constants::RRF_K;
use crate::embeddings::{
    EmbeddingBackend, EmbeddingService, bytes_to_embedding, cosine_similarity,
};
use crate::markdown::{MarkdownSection, section_markdown};
use crate::models::{MemoryKind, MemoryScope, MemoryStatus};
use crate::storage::ensure_db;
use crate::util::{content_hash, stable_id, timestamp};

const AGENT_CONTEXT_WINDOW: i64 = 1;
const AGENT_LEXICAL_ANCHOR_LIMIT: usize = 3;
const AGENT_SEMANTIC_ANCHOR_LIMIT: usize = 2;

#[derive(Clone, Debug)]
pub(crate) struct ResolvedMemoryScope {
    pub(crate) scope: MemoryScope,
    pub(crate) scope_key: Option<String>,
}

#[derive(Debug)]
pub(crate) struct RememberInput {
    pub(crate) kind: MemoryKind,
    pub(crate) status: MemoryStatus,
    pub(crate) scope: ResolvedMemoryScope,
    pub(crate) subject: String,
    pub(crate) trigger: Option<String>,
    pub(crate) content: String,
    pub(crate) tags: Vec<String>,
}

#[derive(Clone, Debug)]
struct MemoryCandidate {
    memory: MemoryRecord,
    section: MemorySectionRecord,
}

#[derive(Clone, Debug)]
struct ScoredMemoryCandidate {
    candidate: MemoryCandidate,
    final_score: f64,
    lexical_rank: Option<usize>,
    lexical_score: Option<f64>,
    vector_rank: Option<usize>,
    vector_score: Option<f64>,
}

#[derive(Clone, Debug)]
struct MemoryRecord {
    id: String,
    created_at: String,
    updated_at: String,
    scope: String,
    scope_key: Option<String>,
    kind: String,
    status: String,
    subject: String,
    trigger: Option<String>,
    content: String,
    tags_json: String,
    confidence: String,
    last_used_at: Option<String>,
    confirmed_at: Option<String>,
    expires_at: Option<String>,
    supersedes_id: Option<String>,
    metadata_json: String,
}

#[derive(Clone, Debug)]
struct MemorySectionRecord {
    id: i64,
    memory_id: String,
    section_index: i64,
    heading_path: Vec<String>,
    heading_level: i64,
    parent_section_index: Option<i64>,
    previous_section_index: Option<i64>,
    next_section_index: Option<i64>,
    anchor: Option<String>,
    markdown: String,
    plain_text: String,
    content_hash: String,
}

pub(crate) fn remember(db_path: &Path, input: RememberInput) -> Result<serde_json::Value> {
    ensure_db(db_path)?;
    if input.subject.trim().is_empty() {
        bail!("memory subject cannot be empty");
    }
    if input.content.trim().is_empty() {
        bail!("memory content cannot be empty");
    }

    let tags = normalized_tags(input.tags);
    let tags_json = serde_json::to_string(&tags)?;
    let now = timestamp();
    let content_hash = content_hash(&input.content);
    let id = stable_id(&format!(
        "memory:{}:{}:{}:{}:{}:{}",
        input.scope.scope.as_str(),
        input.scope.scope_key.as_deref().unwrap_or(""),
        input.kind.as_str(),
        input.subject,
        input.trigger.as_deref().unwrap_or(""),
        content_hash
    ));

    let sections = section_markdown(&input.content);
    if sections.is_empty() {
        bail!("memory content produced no indexable sections");
    }
    let search_contents = sections
        .iter()
        .map(|section| {
            memory_search_text(
                &input.subject,
                input.trigger.as_deref(),
                &tags,
                &section.plain_text,
            )
        })
        .collect::<Vec<_>>();
    let mut embedding_backend = EmbeddingService::from_env(db_path)?;
    let embeddings = embedding_backend.embed_passages(&search_contents)?;

    let mut conn = Connection::open(db_path)?;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO memories(
            id, created_at, updated_at, scope, scope_key, kind, status, subject, trigger,
            content, tags_json, confidence, metadata_json
         )
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'observed', '{}')
         ON CONFLICT(id) DO UPDATE SET
            updated_at = excluded.updated_at,
            scope = excluded.scope,
            scope_key = excluded.scope_key,
            kind = excluded.kind,
            status = excluded.status,
            subject = excluded.subject,
            trigger = excluded.trigger,
            content = excluded.content,
            tags_json = excluded.tags_json",
        params![
            id,
            now,
            now,
            input.scope.scope.as_str(),
            input.scope.scope_key,
            input.kind.as_str(),
            input.status.as_str(),
            input.subject,
            input.trigger,
            input.content,
            tags_json,
        ],
    )?;
    tx.execute("DELETE FROM memories_fts WHERE memory_id = ?1", params![id])?;
    tx.execute(
        "DELETE FROM memory_sections WHERE memory_id = ?1",
        params![id],
    )?;

    for (section, embedding) in sections.iter().zip(embeddings.iter()) {
        insert_memory_section(&tx, &id, section, embedding.as_ref())?;
        let rowid = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO memories_fts(rowid, memory_id, section_index, subject, trigger, content, tags)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                rowid,
                id,
                section.section_index as i64,
                input.subject,
                input.trigger,
                search_contents[section.section_index],
                tags.join(" "),
            ],
        )?;
    }
    tx.commit()?;

    Ok(json!({
        "id": id,
        "kind": input.kind.as_str(),
        "status": input.status.as_str(),
        "scope": input.scope.scope.as_str(),
        "scope_key": input.scope.scope_key,
        "subject": input.subject,
        "trigger": input.trigger,
        "tags": tags,
        "section_count": sections.len(),
    }))
}

fn insert_memory_section(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &str,
    section: &MarkdownSection,
    embedding: Option<&Vec<u8>>,
) -> Result<()> {
    let heading_path = serde_json::to_string(&section.heading_path)?;
    tx.execute(
        "INSERT INTO memory_sections(
            memory_id, section_index, heading_path, heading_level, parent_section_index,
            previous_section_index, next_section_index, anchor, markdown, plain_text,
            content_hash, embedding
         )
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            memory_id,
            section.section_index as i64,
            heading_path,
            section.heading_level as i64,
            section.parent_section_index.map(|value| value as i64),
            section.previous_section_index.map(|value| value as i64),
            section.next_section_index.map(|value| value as i64),
            section.anchor,
            section.markdown,
            section.plain_text,
            section.content_hash,
            embedding,
        ],
    )?;
    Ok(())
}

pub(crate) fn recall(
    db_path: &Path,
    query: &str,
    scopes: &[ResolvedMemoryScope],
    top_k: usize,
) -> Result<Vec<serde_json::Value>> {
    ensure_db(db_path)?;
    if top_k == 0 {
        return Ok(Vec::new());
    }
    let lexical = lexical_candidates(db_path, query, scopes, top_k.max(50) * 10)?;
    let vector = vector_candidates(db_path, query, scopes, top_k.max(50) * 10)?;
    let fused = fuse_candidates(lexical, vector);
    let results = grouped_agent_results(db_path, fused, top_k, query)?;
    let memory_ids = results
        .iter()
        .filter_map(|result| {
            result
                .get("memory")
                .and_then(|memory| memory.get("id"))
                .or_else(|| result.get("id"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .collect::<BTreeSet<_>>();
    mark_memories_used(db_path, &memory_ids)?;
    Ok(results)
}

fn lexical_candidates(
    db_path: &Path,
    query: &str,
    scopes: &[ResolvedMemoryScope],
    limit: usize,
) -> Result<Vec<(MemoryCandidate, f64)>> {
    let fts_query = fts_query(query);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT
            m.id, m.created_at, m.updated_at, m.scope, m.scope_key, m.kind, m.status,
            m.subject, m.trigger, m.content, m.tags_json, m.confidence, m.last_used_at,
            m.confirmed_at, m.expires_at, m.supersedes_id, m.metadata_json,
            s.id, s.memory_id, s.section_index, s.heading_path, s.heading_level,
            s.parent_section_index, s.previous_section_index, s.next_section_index,
            s.anchor, s.markdown, s.plain_text, s.content_hash,
            -bm25(memories_fts) AS score
         FROM memories_fts
         JOIN memory_sections s ON s.id = memories_fts.rowid
         JOIN memories m ON m.id = s.memory_id
         WHERE memories_fts MATCH ?1 AND m.status = 'active'
         ORDER BY bm25(memories_fts)
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![fts_query, limit as i64], row_candidate_with_score)?;
    let mut out = Vec::new();
    for row in rows {
        let (candidate, score) = row?;
        if scope_matches(&candidate.memory, scopes) {
            out.push((candidate, score));
        }
    }
    Ok(out)
}

fn vector_candidates(
    db_path: &Path,
    query: &str,
    scopes: &[ResolvedMemoryScope],
    limit: usize,
) -> Result<Vec<(MemoryCandidate, f64)>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT
            m.id, m.created_at, m.updated_at, m.scope, m.scope_key, m.kind, m.status,
            m.subject, m.trigger, m.content, m.tags_json, m.confidence, m.last_used_at,
            m.confirmed_at, m.expires_at, m.supersedes_id, m.metadata_json,
            s.id, s.memory_id, s.section_index, s.heading_path, s.heading_level,
            s.parent_section_index, s.previous_section_index, s.next_section_index,
            s.anchor, s.markdown, s.plain_text, s.content_hash, s.embedding
         FROM memory_sections s
         JOIN memories m ON m.id = s.memory_id
         WHERE s.embedding IS NOT NULL AND LENGTH(s.embedding) > 0 AND m.status = 'active'",
    )?;
    let rows = stmt.query_map([], row_candidate_with_embedding)?;
    let mut candidates = Vec::new();
    for row in rows {
        let (candidate, embedding) = row?;
        if scope_matches(&candidate.memory, scopes) {
            candidates.push((candidate, embedding));
        }
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let mut embedding_backend = EmbeddingService::from_env(db_path)?;
    let Some(query_embedding) = embedding_backend.embed_query(query)? else {
        return Ok(Vec::new());
    };
    let mut scored = candidates
        .into_par_iter()
        .map(|(candidate, bytes)| {
            let embedding = bytes_to_embedding(&bytes);
            let score = cosine_similarity(&query_embedding, &embedding);
            (candidate, score)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    Ok(scored)
}

fn fuse_candidates(
    lexical: Vec<(MemoryCandidate, f64)>,
    vector: Vec<(MemoryCandidate, f64)>,
) -> Vec<ScoredMemoryCandidate> {
    let mut by_section: HashMap<i64, ScoredMemoryCandidate> = HashMap::new();
    for (rank, (candidate, score)) in lexical.into_iter().enumerate() {
        let entry = by_section
            .entry(candidate.section.id)
            .or_insert(ScoredMemoryCandidate {
                candidate,
                final_score: 0.0,
                lexical_rank: None,
                lexical_score: None,
                vector_rank: None,
                vector_score: None,
            });
        entry.final_score += 1.0 / (RRF_K + rank as f64 + 1.0);
        entry.lexical_rank = Some(rank + 1);
        entry.lexical_score = Some(score);
    }
    for (rank, (candidate, score)) in vector.into_iter().enumerate() {
        let entry = by_section
            .entry(candidate.section.id)
            .or_insert(ScoredMemoryCandidate {
                candidate,
                final_score: 0.0,
                lexical_rank: None,
                lexical_score: None,
                vector_rank: None,
                vector_score: None,
            });
        entry.final_score += 1.0 / (RRF_K + rank as f64 + 1.0);
        entry.vector_rank = Some(rank + 1);
        entry.vector_score = Some(score);
    }
    let mut out = by_section.into_values().collect::<Vec<_>>();
    out.sort_by(|a, b| {
        b.final_score
            .partial_cmp(&a.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

fn grouped_agent_results(
    db_path: &Path,
    candidates: Vec<ScoredMemoryCandidate>,
    top_k: usize,
    query: &str,
) -> Result<Vec<serde_json::Value>> {
    let query_terms = tokenize(query);
    let mut grouped: HashMap<String, Vec<ScoredMemoryCandidate>> = HashMap::new();
    for candidate in candidates {
        grouped
            .entry(candidate.candidate.memory.id.clone())
            .or_default()
            .push(candidate);
    }
    let mut groups = grouped
        .into_values()
        .map(|mut group| {
            group.sort_by(|a, b| {
                b.final_score
                    .partial_cmp(&a.final_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            group
        })
        .collect::<Vec<_>>();
    groups.sort_by(|a, b| {
        let left = a.first().map(|item| item.final_score).unwrap_or_default();
        let right = b.first().map(|item| item.final_score).unwrap_or_default();
        right
            .partial_cmp(&left)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut out = Vec::new();
    for group in groups.into_iter().take(top_k) {
        let Some(first) = group.first() else {
            continue;
        };
        let lexical_matches = group
            .iter()
            .filter(|candidate| candidate.lexical_rank.is_some())
            .cloned()
            .collect::<Vec<_>>();
        let literal_lexical_matches = lexical_matches
            .iter()
            .filter(|candidate| query_term_overlap(&candidate.candidate.section, &query_terms) > 0)
            .cloned()
            .collect::<Vec<_>>();
        let ranked_lexical_matches = if literal_lexical_matches.is_empty() {
            lexical_matches
        } else {
            literal_lexical_matches
        };
        let anchors = if ranked_lexical_matches.is_empty() {
            group
                .iter()
                .take(AGENT_SEMANTIC_ANCHOR_LIMIT)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            ranked_lexical_matches
                .into_iter()
                .take(AGENT_LEXICAL_ANCHOR_LIMIT)
                .collect::<Vec<_>>()
        };
        let anchor_indexes = anchors
            .iter()
            .map(|candidate| candidate.candidate.section.section_index)
            .collect::<BTreeSet<_>>();
        let min_anchor = anchor_indexes
            .iter()
            .next()
            .copied()
            .unwrap_or(first.candidate.section.section_index);
        let max_anchor = anchor_indexes
            .iter()
            .next_back()
            .copied()
            .unwrap_or(first.candidate.section.section_index);
        let context_start = 0.max(min_anchor - AGENT_CONTEXT_WINDOW);
        let context_end = max_anchor + AGENT_CONTEXT_WINDOW;
        let sections = sections_for_memory(
            db_path,
            &first.candidate.memory.id,
            context_start,
            context_end,
        )?;
        let anchor_by_section = anchors
            .into_iter()
            .map(|candidate| (candidate.candidate.section.section_index, candidate))
            .collect::<HashMap<_, _>>();
        let evidence = sections
            .into_iter()
            .map(|section| {
                let anchor = anchor_by_section.get(&section.section_index);
                json!({
                    "kind": evidence_kind(section.section_index, min_anchor, max_anchor, &anchor_indexes),
                    "section_index": section.section_index,
                    "heading_path": section.heading_path,
                    "heading_level": section.heading_level,
                    "anchor": section.anchor,
                    "markdown": section.markdown,
                    "plain_text": section.plain_text,
                    "content_hash": section.content_hash,
                    "score": anchor.map(|candidate| candidate.final_score),
                    "lexical_rank": anchor.and_then(|candidate| candidate.lexical_rank),
                    "vector_rank": anchor.and_then(|candidate| candidate.vector_rank),
                })
            })
            .collect::<Vec<_>>();
        out.push(json!({
            "memory": memory_json(&first.candidate.memory),
            "score": group.iter().map(|candidate| candidate.final_score).sum::<f64>(),
            "matched_section_indexes": anchor_indexes.into_iter().collect::<Vec<_>>(),
            "evidence": evidence,
        }));
    }
    Ok(out)
}

pub(crate) fn list_memories(
    db_path: &Path,
    scopes: &[ResolvedMemoryScope],
    status: Option<MemoryStatus>,
) -> Result<Vec<serde_json::Value>> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, created_at, updated_at, scope, scope_key, kind, status, subject,
                trigger, content, tags_json, confidence, last_used_at, confirmed_at,
                expires_at, supersedes_id, metadata_json
         FROM memories
         ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], row_memory)?;
    let mut out = Vec::new();
    for row in rows {
        let memory = row?;
        if !scope_matches(&memory, scopes) {
            continue;
        }
        if let Some(status) = status
            && memory.status != status.as_str()
        {
            continue;
        }
        out.push(memory_json(&memory));
    }
    Ok(out)
}

pub(crate) fn export_memories(db_path: &Path) -> Result<Vec<serde_json::Value>> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, created_at, updated_at, scope, scope_key, kind, status, subject,
                trigger, content, tags_json, confidence, last_used_at, confirmed_at,
                expires_at, supersedes_id, metadata_json
         FROM memories
         ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], row_memory)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(memory_json(&row?));
    }
    Ok(out)
}

pub(crate) fn show_memory(
    db_path: &Path,
    id: &str,
    scopes: &[ResolvedMemoryScope],
) -> Result<serde_json::Value> {
    ensure_db(db_path)?;
    let conn = Connection::open(db_path)?;
    let memory = find_memory(&conn, id)?;
    if !scope_matches(&memory, scopes) {
        bail!("memory not found: {id}");
    }
    let sections = all_sections_for_memory(&conn, id)?;
    Ok(json!({
        "memory": memory_json(&memory),
        "sections": sections.into_iter().map(|section| section_json_plain(&section)).collect::<Vec<_>>(),
    }))
}

pub(crate) fn forget_memory(
    db_path: &Path,
    id: &str,
    scopes: &[ResolvedMemoryScope],
) -> Result<serde_json::Value> {
    ensure_db(db_path)?;
    let now = timestamp();
    let conn = Connection::open(db_path)?;
    let memory = find_memory(&conn, id)?;
    if !scope_matches(&memory, scopes) {
        bail!("memory not found: {id}");
    }
    let changed = conn.execute(
        "UPDATE memories SET status = 'dismissed', updated_at = ?1 WHERE id = ?2",
        params![now, id],
    )?;
    if changed == 0 {
        bail!("memory not found: {id}");
    }
    Ok(json!({
        "id": id,
        "status": "dismissed",
    }))
}

pub(crate) fn accept_memory(
    db_path: &Path,
    id: &str,
    scopes: &[ResolvedMemoryScope],
) -> Result<serde_json::Value> {
    transition_memory_status(db_path, id, scopes, "active", Some("suggested"), true)
}

pub(crate) fn reject_memory(
    db_path: &Path,
    id: &str,
    scopes: &[ResolvedMemoryScope],
) -> Result<serde_json::Value> {
    transition_memory_status(db_path, id, scopes, "dismissed", Some("suggested"), false)
}

fn transition_memory_status(
    db_path: &Path,
    id: &str,
    scopes: &[ResolvedMemoryScope],
    status: &str,
    expected_status: Option<&str>,
    confirm: bool,
) -> Result<serde_json::Value> {
    ensure_db(db_path)?;
    let now = timestamp();
    let conn = Connection::open(db_path)?;
    let memory = find_memory(&conn, id)?;
    if !scope_matches(&memory, scopes) {
        bail!("memory not found: {id}");
    }
    if let Some(expected_status) = expected_status
        && memory.status != expected_status
    {
        bail!(
            "memory {id} has status {}, expected {expected_status}",
            memory.status
        );
    }
    if confirm {
        conn.execute(
            "UPDATE memories
             SET status = ?1, updated_at = ?2, confirmed_at = COALESCE(confirmed_at, ?2)
             WHERE id = ?3",
            params![status, now, id],
        )?;
    } else {
        conn.execute(
            "UPDATE memories SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, now, id],
        )?;
    }
    let memory = find_memory(&conn, id)?;
    Ok(json!({
        "memory": memory_json(&memory),
    }))
}

fn find_memory(conn: &Connection, id: &str) -> Result<MemoryRecord> {
    conn.query_row(
        "SELECT id, created_at, updated_at, scope, scope_key, kind, status, subject,
                trigger, content, tags_json, confidence, last_used_at, confirmed_at,
                expires_at, supersedes_id, metadata_json
         FROM memories
         WHERE id = ?1",
        params![id],
        row_memory,
    )
    .optional()?
    .ok_or_else(|| anyhow!("memory not found: {id}"))
}

fn sections_for_memory(
    db_path: &Path,
    memory_id: &str,
    start: i64,
    end: i64,
) -> Result<Vec<MemorySectionRecord>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, memory_id, section_index, heading_path, heading_level,
                parent_section_index, previous_section_index, next_section_index,
                anchor, markdown, plain_text, content_hash
         FROM memory_sections
         WHERE memory_id = ?1 AND section_index BETWEEN ?2 AND ?3
         ORDER BY section_index ASC",
    )?;
    let rows = stmt.query_map(params![memory_id, start, end], row_section)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn all_sections_for_memory(conn: &Connection, memory_id: &str) -> Result<Vec<MemorySectionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, memory_id, section_index, heading_path, heading_level,
                parent_section_index, previous_section_index, next_section_index,
                anchor, markdown, plain_text, content_hash
         FROM memory_sections
         WHERE memory_id = ?1
         ORDER BY section_index ASC",
    )?;
    let rows = stmt.query_map(params![memory_id], row_section)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn mark_memories_used(db_path: &Path, memory_ids: &BTreeSet<String>) -> Result<()> {
    if memory_ids.is_empty() {
        return Ok(());
    }
    let now = timestamp();
    let conn = Connection::open(db_path)?;
    for id in memory_ids {
        conn.execute(
            "UPDATE memories SET last_used_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
    }
    Ok(())
}

fn row_candidate_with_score(row: &rusqlite::Row<'_>) -> rusqlite::Result<(MemoryCandidate, f64)> {
    Ok((
        MemoryCandidate {
            memory: row_memory_range(row, 0)?,
            section: row_section_range(row, 17)?,
        },
        row.get(29)?,
    ))
}

fn row_candidate_with_embedding(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(MemoryCandidate, Vec<u8>)> {
    Ok((
        MemoryCandidate {
            memory: row_memory_range(row, 0)?,
            section: row_section_range(row, 17)?,
        },
        row.get(29)?,
    ))
}

fn row_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRecord> {
    row_memory_range(row, 0)
}

fn row_memory_range(row: &rusqlite::Row<'_>, start: usize) -> rusqlite::Result<MemoryRecord> {
    Ok(MemoryRecord {
        id: row.get(start)?,
        created_at: row.get(start + 1)?,
        updated_at: row.get(start + 2)?,
        scope: row.get(start + 3)?,
        scope_key: row.get(start + 4)?,
        kind: row.get(start + 5)?,
        status: row.get(start + 6)?,
        subject: row.get(start + 7)?,
        trigger: row.get(start + 8)?,
        content: row.get(start + 9)?,
        tags_json: row.get(start + 10)?,
        confidence: row.get(start + 11)?,
        last_used_at: row.get(start + 12)?,
        confirmed_at: row.get(start + 13)?,
        expires_at: row.get(start + 14)?,
        supersedes_id: row.get(start + 15)?,
        metadata_json: row.get(start + 16)?,
    })
}

fn row_section(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemorySectionRecord> {
    row_section_range(row, 0)
}

fn row_section_range(
    row: &rusqlite::Row<'_>,
    start: usize,
) -> rusqlite::Result<MemorySectionRecord> {
    let heading_path: String = row.get(start + 3)?;
    Ok(MemorySectionRecord {
        id: row.get(start)?,
        memory_id: row.get(start + 1)?,
        section_index: row.get(start + 2)?,
        heading_path: serde_json::from_str(&heading_path).unwrap_or_default(),
        heading_level: row.get(start + 4)?,
        parent_section_index: row.get(start + 5)?,
        previous_section_index: row.get(start + 6)?,
        next_section_index: row.get(start + 7)?,
        anchor: row.get(start + 8)?,
        markdown: row.get(start + 9)?,
        plain_text: row.get(start + 10)?,
        content_hash: row.get(start + 11)?,
    })
}

fn memory_json(memory: &MemoryRecord) -> serde_json::Value {
    json!({
        "id": memory.id,
        "created_at": memory.created_at,
        "updated_at": memory.updated_at,
        "scope": memory.scope,
        "scope_key": memory.scope_key,
        "kind": memory.kind,
        "status": memory.status,
        "subject": memory.subject,
        "trigger": memory.trigger,
        "content": memory.content,
        "tags": tags_from_json(&memory.tags_json),
        "confidence": memory.confidence,
        "last_used_at": memory.last_used_at,
        "confirmed_at": memory.confirmed_at,
        "expires_at": memory.expires_at,
        "supersedes_id": memory.supersedes_id,
        "metadata": metadata_from_json(&memory.metadata_json),
    })
}

fn section_json_plain(section: &MemorySectionRecord) -> serde_json::Value {
    json!({
        "id": section.id,
        "memory_id": section.memory_id,
        "section_index": section.section_index,
        "heading_path": section.heading_path,
        "heading_level": section.heading_level,
        "parent_section_index": section.parent_section_index,
        "previous_section_index": section.previous_section_index,
        "next_section_index": section.next_section_index,
        "anchor": section.anchor,
        "markdown": section.markdown,
        "plain_text": section.plain_text,
        "content_hash": section.content_hash,
    })
}

fn evidence_kind(
    section_index: i64,
    min_anchor: i64,
    max_anchor: i64,
    anchor_indexes: &BTreeSet<i64>,
) -> &'static str {
    if anchor_indexes.contains(&section_index) {
        "match"
    } else if section_index < min_anchor {
        "context-before"
    } else if section_index > max_anchor {
        "context-after"
    } else {
        "context-between"
    }
}

fn scope_matches(memory: &MemoryRecord, scopes: &[ResolvedMemoryScope]) -> bool {
    scopes.iter().any(|scope| {
        memory.scope == scope.scope.as_str()
            && match (&scope.scope_key, &memory.scope_key) {
                (None, None) => true,
                (Some(expected), Some(actual)) => expected == actual,
                (None, Some(_)) => scope.scope == MemoryScope::Global,
                (Some(_), None) => false,
            }
    })
}

fn memory_search_text(
    subject: &str,
    trigger: Option<&str>,
    tags: &[String],
    plain_text: &str,
) -> String {
    [
        subject,
        trigger.unwrap_or_default(),
        &tags.join(" "),
        plain_text,
    ]
    .into_iter()
    .filter(|part| !part.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

fn normalized_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    tags.into_iter()
        .map(|tag| tag.trim().to_ascii_lowercase())
        .filter(|tag| !tag.is_empty())
        .filter(|tag| seen.insert(tag.clone()))
        .collect()
}

fn query_term_overlap(section: &MemorySectionRecord, query_terms: &[String]) -> usize {
    let content =
        format!("{} {}", section.heading_path.join(" "), section.plain_text).to_ascii_lowercase();
    query_terms
        .iter()
        .filter(|term| content.contains(term.as_str()))
        .count()
}

fn tokenize(input: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    input
        .split(|c: char| {
            !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '@' | '#'))
        })
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(|term| term.to_ascii_lowercase())
        .filter(|term| seen.insert(term.clone()))
        .collect()
}

fn fts_query(input: &str) -> String {
    tokenize(input)
        .into_iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn tags_from_json(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn metadata_from_json(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| json!({}))
}
