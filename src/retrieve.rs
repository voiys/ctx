use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use anyhow::Result;
use rayon::prelude::*;
use rusqlite::{Connection, params};
use serde_json::json;

use crate::constants::{LLMS_TXT_SOURCE_PRIOR_SCORE, RRF_K};
use crate::embeddings::{
    EmbeddingBackend, EmbeddingService, bytes_to_embedding, cosine_similarity,
};
use crate::models::{CandidateBase, CandidateScore};

const AGENT_CONTEXT_WINDOW: i64 = 2;
const AGENT_LEXICAL_ANCHOR_LIMIT: usize = 3;
const AGENT_SEMANTIC_ANCHOR_LIMIT: usize = 3;

pub(crate) fn query_index(
    db_path: &Path,
    question: &str,
    allowed_resource_ids: &BTreeSet<String>,
    top_k: usize,
    budget_tokens: usize,
    debug: bool,
) -> Result<Vec<serde_json::Value>> {
    if allowed_resource_ids.is_empty() || top_k == 0 {
        return Ok(Vec::new());
    }
    let lexical = lexical_candidates(db_path, question, allowed_resource_ids, top_k.max(50) * 10)?;
    let vector = vector_candidates(db_path, question, allowed_resource_ids, top_k.max(50) * 10)?;
    let fused = fuse_candidates(lexical, vector);
    agent_results(db_path, fused, question, top_k, budget_tokens, debug)
}

fn lexical_candidates(
    db_path: &Path,
    question: &str,
    allowed_resource_ids: &BTreeSet<String>,
    limit: usize,
) -> Result<Vec<(CandidateBase, f64)>> {
    let fts_query = fts_query(question);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT c.id, c.resource_id, c.snapshot_id, c.kind, c.label, c.source_url, c.chunk_index,
                c.section_index, c.heading_path, c.heading_level, c.parent_section_index,
                c.previous_section_index, c.next_section_index, c.anchor, c.plain_text,
                c.content_hash, c.content, -bm25(chunks_fts) AS score
         FROM chunks_fts
         JOIN chunks c ON c.id = chunks_fts.rowid
         WHERE chunks_fts MATCH ?1
         ORDER BY bm25(chunks_fts)
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
        Ok((candidate_base_from_row(row)?, row.get::<_, f64>(17)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (base, score) = row?;
        if allowed_resource_ids.contains(&base.resource_id) {
            out.push((base, score));
        }
    }
    Ok(out)
}

fn vector_candidates(
    db_path: &Path,
    question: &str,
    allowed_resource_ids: &BTreeSet<String>,
    limit: usize,
) -> Result<Vec<(CandidateBase, f64)>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, resource_id, snapshot_id, kind, label, source_url, chunk_index,
                section_index, heading_path, heading_level, parent_section_index,
                previous_section_index, next_section_index, anchor, plain_text,
                content_hash, content, embedding
         FROM chunks
         WHERE embedding IS NOT NULL AND LENGTH(embedding) > 0",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((candidate_base_from_row(row)?, row.get::<_, Vec<u8>>(17)?))
    })?;
    let mut chunks = Vec::new();
    for row in rows {
        let (base, embedding) = row?;
        if allowed_resource_ids.contains(&base.resource_id) {
            chunks.push((base, embedding));
        }
    }
    if chunks.is_empty() {
        return Ok(Vec::new());
    }
    let mut embedding_backend = EmbeddingService::from_env(db_path)?;
    let Some(query_embedding) = embedding_backend.embed_query(question)? else {
        return Ok(Vec::new());
    };
    let mut scored = chunks
        .into_par_iter()
        .map(|(base, bytes)| {
            let embedding = bytes_to_embedding(&bytes);
            let score = cosine_similarity(&query_embedding, &embedding);
            (base, score)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    Ok(scored)
}

fn agent_results(
    db_path: &Path,
    candidates: Vec<CandidateScore>,
    question: &str,
    top_k: usize,
    budget_tokens: usize,
    debug: bool,
) -> Result<Vec<serde_json::Value>> {
    let mut grouped: HashMap<(String, String, String), Vec<CandidateScore>> = HashMap::new();
    for candidate in candidates {
        grouped
            .entry((
                candidate.base.resource_id.clone(),
                candidate.base.snapshot_id.clone(),
                candidate.base.source_url.clone(),
            ))
            .or_default()
            .push(candidate);
    }

    let query_terms = tokenize(question);
    let mut groups = grouped
        .into_values()
        .filter_map(|mut group| {
            group.sort_by(score_desc);
            let anchors = agent_anchor_candidates(&group, &query_terms);
            AgentGroup::from_anchors(anchors)
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut out = Vec::new();
    let mut used_tokens = 0usize;
    for group in groups.into_iter().take(top_k) {
        let rows = context_candidates(db_path, &group)?;
        let estimated = rows
            .iter()
            .map(|row| estimate_tokens(&row.content))
            .sum::<usize>();
        if used_tokens + estimated > budget_tokens && !out.is_empty() {
            break;
        }
        used_tokens += estimated;
        let anchor_by_chunk = group
            .anchors
            .iter()
            .map(|candidate| (candidate.base.chunk_id, candidate))
            .collect::<HashMap<_, _>>();
        let evidence = rows
            .into_iter()
            .map(|row| {
                let anchor = anchor_by_chunk.get(&row.chunk_id).copied();
                let mut item = json!({
                    "kind": evidence_kind(row.chunk_index, &group),
                    "content": row.content,
                    "citation": row.source_url,
                    "chunk_index": row.chunk_index,
                    "section_index": row.section_index,
                    "heading_path": row.heading_path,
                    "heading_level": row.heading_level,
                    "anchor": row.anchor,
                    "plain_text": row.plain_text,
                    "content_hash": row.content_hash,
                    "score": anchor.map(|candidate| candidate.final_score),
                    "lexical_rank": anchor.and_then(|candidate| candidate.lexical_rank),
                    "vector_rank": anchor.and_then(|candidate| candidate.vector_rank),
                });
                if debug {
                    item["debug"] = json!({
                        "chunk_id": row.chunk_id,
                        "parent_section_index": row.parent_section_index,
                        "previous_section_index": row.previous_section_index,
                        "next_section_index": row.next_section_index,
                        "lexical_score": anchor.and_then(|candidate| candidate.lexical_score),
                        "vector_score": anchor.and_then(|candidate| candidate.vector_score),
                        "source_prior": anchor.and_then(|candidate| candidate.source_prior.clone()),
                        "source_prior_score": anchor.map(|candidate| candidate.source_prior_score),
                    });
                }
                item
            })
            .collect::<Vec<_>>();
        out.push(json!({
            "rank": out.len() + 1,
            "kind": group.kind,
            "label": group.label,
            "citation": group.source_url,
            "snapshot_id": group.snapshot_id,
            "score": group.score,
            "matched_chunk_indexes": group.anchor_chunk_indexes.iter().copied().collect::<Vec<_>>(),
            "matched_section_indexes": group.anchor_section_indexes.iter().copied().collect::<Vec<_>>(),
            "evidence": evidence,
        }));
    }
    Ok(out)
}

#[derive(Debug)]
struct AgentGroup {
    resource_id: String,
    snapshot_id: String,
    kind: String,
    label: String,
    source_url: String,
    score: f64,
    anchors: Vec<CandidateScore>,
    anchor_chunk_indexes: BTreeSet<i64>,
    anchor_section_indexes: BTreeSet<i64>,
    min_anchor_chunk_index: i64,
    max_anchor_chunk_index: i64,
}

impl AgentGroup {
    fn from_anchors(anchors: Vec<CandidateScore>) -> Option<Self> {
        let first = anchors.first()?.base.clone();
        let anchor_chunk_indexes = anchors
            .iter()
            .map(|candidate| candidate.base.chunk_index)
            .collect::<BTreeSet<_>>();
        let anchor_section_indexes = anchors
            .iter()
            .map(|candidate| candidate.base.section_index)
            .collect::<BTreeSet<_>>();
        let min_chunk = anchor_chunk_indexes
            .iter()
            .next()
            .copied()
            .unwrap_or(first.chunk_index);
        let max_chunk = anchor_chunk_indexes
            .iter()
            .next_back()
            .copied()
            .unwrap_or(first.chunk_index);
        Some(Self {
            resource_id: first.resource_id,
            snapshot_id: first.snapshot_id,
            kind: first.kind,
            label: first.label,
            source_url: first.source_url,
            score: anchors.iter().map(|candidate| candidate.final_score).sum(),
            anchors,
            anchor_chunk_indexes,
            anchor_section_indexes,
            min_anchor_chunk_index: min_chunk,
            max_anchor_chunk_index: max_chunk,
        })
    }
}

fn agent_anchor_candidates(
    group: &[CandidateScore],
    query_terms: &[String],
) -> Vec<CandidateScore> {
    let lexical = group
        .iter()
        .filter(|candidate| candidate.lexical_rank.is_some())
        .cloned()
        .collect::<Vec<_>>();
    let mut literal_lexical = lexical
        .iter()
        .filter(|candidate| query_term_overlap(&candidate.base, query_terms) > 0)
        .cloned()
        .collect::<Vec<_>>();
    literal_lexical.sort_by(|left, right| {
        query_term_overlap(&right.base, query_terms)
            .cmp(&query_term_overlap(&left.base, query_terms))
            .then_with(|| {
                left.lexical_rank
                    .unwrap_or(usize::MAX)
                    .cmp(&right.lexical_rank.unwrap_or(usize::MAX))
            })
            .then_with(|| score_desc(left, right))
    });
    if !literal_lexical.is_empty() {
        literal_lexical.truncate(AGENT_LEXICAL_ANCHOR_LIMIT);
        return literal_lexical;
    }
    if !lexical.is_empty() {
        return lexical
            .into_iter()
            .take(AGENT_LEXICAL_ANCHOR_LIMIT)
            .collect();
    }
    group
        .iter()
        .take(AGENT_SEMANTIC_ANCHOR_LIMIT)
        .cloned()
        .collect()
}

fn context_candidates(db_path: &Path, group: &AgentGroup) -> Result<Vec<CandidateBase>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT id, resource_id, snapshot_id, kind, label, source_url, chunk_index,
                section_index, heading_path, heading_level, parent_section_index,
                previous_section_index, next_section_index, anchor, plain_text,
                content_hash, content
         FROM chunks
         WHERE resource_id = ?1
            AND snapshot_id = ?2
            AND source_url = ?3
            AND chunk_index BETWEEN ?4 AND ?5
         ORDER BY chunk_index ASC",
    )?;
    let mut by_chunk = BTreeMap::new();
    for (start, end) in context_ranges(group) {
        let rows = stmt.query_map(
            params![
                group.resource_id,
                group.snapshot_id,
                group.source_url,
                start,
                end
            ],
            candidate_base_from_row,
        )?;
        for row in rows {
            let row = row?;
            by_chunk.insert(row.chunk_id, row);
        }
    }
    Ok(by_chunk.into_values().collect())
}

fn context_ranges(group: &AgentGroup) -> Vec<(i64, i64)> {
    let mut ranges = group
        .anchor_chunk_indexes
        .iter()
        .map(|chunk| {
            (
                0.max(*chunk - AGENT_CONTEXT_WINDOW),
                *chunk + AGENT_CONTEXT_WINDOW,
            )
        })
        .collect::<Vec<_>>();
    ranges.sort_unstable();

    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (start, end) in ranges {
        if let Some((_, prev_end)) = merged.last_mut()
            && start <= *prev_end + 1
        {
            *prev_end = (*prev_end).max(end);
            continue;
        }
        merged.push((start, end));
    }
    merged
}

fn candidate_base_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CandidateBase> {
    Ok(CandidateBase {
        chunk_id: row.get(0)?,
        resource_id: row.get(1)?,
        snapshot_id: row.get(2)?,
        kind: row.get(3)?,
        label: row.get(4)?,
        source_url: row.get(5)?,
        chunk_index: row.get(6)?,
        section_index: row.get(7)?,
        heading_path: parse_heading_path(row.get(8)?),
        heading_level: row.get(9)?,
        parent_section_index: row.get(10)?,
        previous_section_index: row.get(11)?,
        next_section_index: row.get(12)?,
        anchor: row.get(13)?,
        plain_text: row.get(14)?,
        content_hash: row.get(15)?,
        content: row.get(16)?,
    })
}

fn fuse_candidates(
    lexical: Vec<(CandidateBase, f64)>,
    vector: Vec<(CandidateBase, f64)>,
) -> Vec<CandidateScore> {
    let mut by_chunk: HashMap<i64, CandidateScore> = HashMap::new();
    for (rank, (base, score)) in lexical.into_iter().enumerate() {
        let rank = rank + 1;
        let entry = by_chunk.entry(base.chunk_id).or_insert(CandidateScore {
            base,
            final_score: 0.0,
            source_prior: None,
            source_prior_score: 0.0,
            lexical_rank: None,
            lexical_score: None,
            vector_rank: None,
            vector_score: None,
        });
        entry.final_score += 1.0 / (RRF_K + rank as f64);
        entry.lexical_rank = Some(rank);
        entry.lexical_score = Some(score);
    }
    for (rank, (base, score)) in vector.into_iter().enumerate() {
        let rank = rank + 1;
        let entry = by_chunk.entry(base.chunk_id).or_insert(CandidateScore {
            base,
            final_score: 0.0,
            source_prior: None,
            source_prior_score: 0.0,
            lexical_rank: None,
            lexical_score: None,
            vector_rank: None,
            vector_score: None,
        });
        entry.final_score += 1.0 / (RRF_K + rank as f64);
        entry.vector_rank = Some(rank);
        entry.vector_score = Some(score);
    }
    let mut out = by_chunk
        .into_values()
        .map(|mut candidate| {
            if is_llms_txt_source(&candidate.base.source_url) {
                candidate.source_prior = Some("llms_txt".to_string());
                candidate.source_prior_score = LLMS_TXT_SOURCE_PRIOR_SCORE;
                candidate.final_score += LLMS_TXT_SOURCE_PRIOR_SCORE;
            }
            candidate
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| {
        b.final_score
            .partial_cmp(&a.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

fn is_llms_txt_source(source_url: &str) -> bool {
    source_url
        .split(['?', '#'])
        .next()
        .unwrap_or(source_url)
        .trim_end_matches('/')
        .ends_with("/llms.txt")
}

fn score_desc(left: &CandidateScore, right: &CandidateScore) -> std::cmp::Ordering {
    right
        .final_score
        .partial_cmp(&left.final_score)
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn evidence_kind(chunk_index: i64, group: &AgentGroup) -> &'static str {
    if group.anchor_chunk_indexes.contains(&chunk_index) {
        "match"
    } else if chunk_index < group.min_anchor_chunk_index {
        "context-before"
    } else if chunk_index > group.max_anchor_chunk_index {
        "context-after"
    } else {
        "context-between"
    }
}

fn query_term_overlap(candidate: &CandidateBase, query_terms: &[String]) -> usize {
    if query_terms.is_empty() {
        return 0;
    }
    let content = format!(
        "{} {} {}",
        candidate.label,
        candidate.heading_path.join(" "),
        candidate.plain_text
    )
    .to_ascii_lowercase();
    query_terms
        .iter()
        .filter(|term| content.contains(term.as_str()))
        .count()
}

fn parse_heading_path(raw: String) -> Vec<String> {
    serde_json::from_str(&raw).unwrap_or_default()
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

fn estimate_tokens(content: &str) -> usize {
    content.len().div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_code_shaped_terms() {
        assert_eq!(
            tokenize("ERR_MODULE_NOT_FOUND in @scope/pkg -- config.file"),
            vec![
                "err_module_not_found",
                "in",
                "@scope/pkg",
                "--",
                "config.file"
            ]
        );
    }

    #[test]
    fn llms_txt_prior_breaks_close_retrieval_ties() {
        let ordinary = test_candidate(1, "https://example.com/docs/page", 0, "ordinary docs");
        let llms = test_candidate(2, "https://example.com/docs/llms.txt", 1, "llms docs");

        let fused = fuse_candidates(vec![(ordinary, 1.0), (llms, 0.9)], Vec::new());

        assert_eq!(
            fused[0].base.source_url,
            "https://example.com/docs/llms.txt"
        );
        assert_eq!(fused[0].source_prior.as_deref(), Some("llms_txt"));
        assert_eq!(fused[0].source_prior_score, LLMS_TXT_SOURCE_PRIOR_SCORE);
    }

    fn test_candidate(
        chunk_id: i64,
        source_url: &str,
        chunk_index: i64,
        content: &str,
    ) -> CandidateBase {
        let content = content.to_string();
        CandidateBase {
            chunk_id,
            resource_id: "docs".to_string(),
            snapshot_id: "snapshot".to_string(),
            kind: "docs".to_string(),
            label: "docs".to_string(),
            source_url: source_url.to_string(),
            chunk_index,
            section_index: chunk_index,
            heading_path: Vec::new(),
            heading_level: 0,
            parent_section_index: None,
            previous_section_index: None,
            next_section_index: None,
            anchor: None,
            plain_text: content.clone(),
            content_hash: String::new(),
            content,
        }
    }
}
