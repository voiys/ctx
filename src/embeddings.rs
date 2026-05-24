use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

use crate::constants::EMBEDDING_BATCH_SIZE;

pub(crate) trait EmbeddingBackend {
    fn embed_passages(&mut self, contents: &[String]) -> Result<Vec<Option<Vec<u8>>>>;
    fn embed_query(&mut self, question: &str) -> Result<Option<Vec<f32>>>;
}

pub(crate) enum EmbeddingService {
    Disabled,
    FastEmbed { models_dir: PathBuf },
}

impl EmbeddingService {
    pub(crate) fn from_env(db_path: &Path) -> Result<Self> {
        if embeddings_disabled() {
            return Ok(Self::Disabled);
        }
        let models_dir = db_path
            .parent()
            .ok_or_else(|| anyhow!("database path has no parent"))?
            .join("models");
        Ok(Self::FastEmbed { models_dir })
    }
}

impl EmbeddingBackend for EmbeddingService {
    fn embed_passages(&mut self, contents: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        match self {
            Self::Disabled => Ok(vec![None; contents.len()]),
            Self::FastEmbed { models_dir } => {
                if contents.is_empty() {
                    return Ok(Vec::new());
                }
                let mut model = fastembed_model(models_dir)?;
                let mut out = Vec::with_capacity(contents.len());
                for batch in contents.chunks(EMBEDDING_BATCH_SIZE) {
                    let inputs = batch
                        .iter()
                        .map(|content| format!("passage: {content}"))
                        .collect::<Vec<_>>();
                    let embeddings = model.embed(inputs, None)?;
                    out.extend(
                        embeddings
                            .into_iter()
                            .map(|embedding| Some(embedding_to_bytes(&embedding))),
                    );
                }
                Ok(out)
            }
        }
    }

    fn embed_query(&mut self, question: &str) -> Result<Option<Vec<f32>>> {
        match self {
            Self::Disabled => Ok(None),
            Self::FastEmbed { models_dir } => {
                let mut model = fastembed_model(models_dir)?;
                let embeddings = model.embed(vec![format!("query: {question}")], None)?;
                embeddings
                    .into_iter()
                    .next()
                    .map(Some)
                    .ok_or_else(|| anyhow!("embedding model returned no query embedding"))
            }
        }
    }
}

fn embeddings_disabled() -> bool {
    std::env::var("CTX_EMBEDDINGS")
        .map(|value| matches!(value.as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(false)
}

fn fastembed_model(models_dir: &Path) -> Result<TextEmbedding> {
    std::fs::create_dir_all(models_dir)?;
    let options = TextInitOptions::new(EmbeddingModel::AllMiniLML6V2Q)
        .with_cache_dir(models_dir.to_path_buf())
        .with_show_download_progress(false);
    TextEmbedding::try_new(options)
}

fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

pub(crate) fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    let mut dot = 0.0f64;
    let mut a_norm = 0.0f64;
    let mut b_norm = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let x = *x as f64;
        let y = *y as f64;
        dot += x * y;
        a_norm += x * x;
        b_norm += y * y;
    }
    if a_norm == 0.0 || b_norm == 0.0 {
        0.0
    } else {
        dot / (a_norm.sqrt() * b_norm.sqrt())
    }
}
