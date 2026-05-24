use std::collections::{BTreeSet, HashMap};
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
    let matched_tokens = tokenize(question);
    let mut out = Vec::new();
    let mut used_tokens = 0usize;
    let mut seen = BTreeSet::new();
    for candidate in fused {
        if !seen.insert((
            candidate.base.resource_id.clone(),
            candidate.base.chunk_index,
        )) {
            continue;
        }
        let estimated = estimate_tokens(&candidate.base.content);
        if used_tokens + estimated > budget_tokens && !out.is_empty() {
            break;
        }
        used_tokens += estimated;
        let mut item = json!({
            "rank": out.len() + 1,
            "kind": candidate.base.kind,
            "label": candidate.base.label,
            "content": candidate.base.content,
            "citation": candidate.base.source_url,
            "snapshot_id": candidate.base.snapshot_id,
            "score": candidate.final_score,
        });
        if debug {
            item["debug"] = json!({
                "retrieval": if candidate.vector_rank.is_some() { "rrf_hybrid" } else { "fts5_bm25" },
                "chunk_id": candidate.base.chunk_id,
                "chunk_index": candidate.base.chunk_index,
                "matched_tokens": matched_tokens,
                "lexical_rank": candidate.lexical_rank,
                "lexical_score": candidate.lexical_score,
                "vector_rank": candidate.vector_rank,
                "vector_score": candidate.vector_score,
                "source_prior": candidate.source_prior,
                "source_prior_score": candidate.source_prior_score,
                "rrf_score": candidate.final_score,
            });
        }
        out.push(item);
        if out.len() >= top_k {
            break;
        }
    }
    Ok(out)
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
                c.content, -bm25(chunks_fts) AS score
         FROM chunks_fts
         JOIN chunks c ON c.id = chunks_fts.rowid
         WHERE chunks_fts MATCH ?1
         ORDER BY bm25(chunks_fts)
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
        Ok((
            CandidateBase {
                chunk_id: row.get(0)?,
                resource_id: row.get(1)?,
                snapshot_id: row.get(2)?,
                kind: row.get(3)?,
                label: row.get(4)?,
                source_url: row.get(5)?,
                chunk_index: row.get(6)?,
                content: row.get(7)?,
            },
            row.get::<_, f64>(8)?,
        ))
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
        "SELECT id, resource_id, snapshot_id, kind, label, source_url, chunk_index, content, embedding
         FROM chunks
         WHERE embedding IS NOT NULL AND LENGTH(embedding) > 0",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            CandidateBase {
                chunk_id: row.get(0)?,
                resource_id: row.get(1)?,
                snapshot_id: row.get(2)?,
                kind: row.get(3)?,
                label: row.get(4)?,
                source_url: row.get(5)?,
                chunk_index: row.get(6)?,
                content: row.get(7)?,
            },
            row.get::<_, Vec<u8>>(8)?,
        ))
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
        let ordinary = CandidateBase {
            chunk_id: 1,
            resource_id: "docs".to_string(),
            snapshot_id: "snapshot".to_string(),
            kind: "docs".to_string(),
            label: "docs".to_string(),
            source_url: "https://example.com/docs/page".to_string(),
            chunk_index: 0,
            content: "ordinary docs".to_string(),
        };
        let llms = CandidateBase {
            chunk_id: 2,
            resource_id: "docs".to_string(),
            snapshot_id: "snapshot".to_string(),
            kind: "docs".to_string(),
            label: "docs".to_string(),
            source_url: "https://example.com/docs/llms.txt".to_string(),
            chunk_index: 1,
            content: "llms docs".to_string(),
        };

        let fused = fuse_candidates(vec![(ordinary, 1.0), (llms, 0.9)], Vec::new());

        assert_eq!(
            fused[0].base.source_url,
            "https://example.com/docs/llms.txt"
        );
        assert_eq!(fused[0].source_prior.as_deref(), Some("llms_txt"));
        assert_eq!(fused[0].source_prior_score, LLMS_TXT_SOURCE_PRIOR_SCORE);
    }
}
