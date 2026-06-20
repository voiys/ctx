use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use fastembed::{
    InitOptionsUserDefined, QuantizationMode, TextEmbedding, TokenizerFiles,
    UserDefinedEmbeddingModel,
};
use hf_hub::{Cache, Repo, RepoType, api::sync::ApiBuilder};
use sha2::{Digest, Sha256};

use crate::constants::EMBEDDING_BATCH_SIZE;

const FASTEMBED_REPO: &str = "Xenova/all-MiniLM-L6-v2";
const FASTEMBED_REVISION: &str = "751bff37182d3f1213fa05d7196b954e230abad9";
const FASTEMBED_FILES: &[PinnedModelFile] = &[
    PinnedModelFile {
        path: "onnx/model_quantized.onnx",
        sha256: "afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1",
    },
    PinnedModelFile {
        path: "tokenizer.json",
        sha256: "da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0",
    },
    PinnedModelFile {
        path: "config.json",
        sha256: "7135149f7cffa1a573466c6e4d8423ed73b62fd2332c575bf738a0d033f70df7",
    },
    PinnedModelFile {
        path: "special_tokens_map.json",
        sha256: "b6d346be366a7d1d48332dbc9fdf3bf8960b5d879522b7799ddba59e76237ee3",
    },
    PinnedModelFile {
        path: "tokenizer_config.json",
        sha256: "9261e7d79b44c8195c1cada2b453e55b00aeb81e907a6664974b4d7776172ab3",
    },
];

struct PinnedModelFile {
    path: &'static str,
    sha256: &'static str,
}

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
    let files = pinned_fastembed_files(models_dir)?;
    let model = UserDefinedEmbeddingModel::new(
        files.onnx_file,
        TokenizerFiles {
            tokenizer_file: files.tokenizer_file,
            config_file: files.config_file,
            special_tokens_map_file: files.special_tokens_map_file,
            tokenizer_config_file: files.tokenizer_config_file,
        },
    )
    .with_quantization(QuantizationMode::Dynamic)
    .with_pooling(fastembed::Pooling::Mean);
    TextEmbedding::try_new_from_user_defined(model, InitOptionsUserDefined::new())
}

struct PinnedFastEmbedFiles {
    onnx_file: Vec<u8>,
    tokenizer_file: Vec<u8>,
    config_file: Vec<u8>,
    special_tokens_map_file: Vec<u8>,
    tokenizer_config_file: Vec<u8>,
}

fn pinned_fastembed_files(models_dir: &Path) -> Result<PinnedFastEmbedFiles> {
    let repo = pinned_fastembed_repo(models_dir)?;
    Ok(PinnedFastEmbedFiles {
        onnx_file: pinned_file(&repo, &FASTEMBED_FILES[0])?,
        tokenizer_file: pinned_file(&repo, &FASTEMBED_FILES[1])?,
        config_file: pinned_file(&repo, &FASTEMBED_FILES[2])?,
        special_tokens_map_file: pinned_file(&repo, &FASTEMBED_FILES[3])?,
        tokenizer_config_file: pinned_file(&repo, &FASTEMBED_FILES[4])?,
    })
}

fn pinned_fastembed_repo(models_dir: &Path) -> Result<hf_hub::api::sync::ApiRepo> {
    let cache = Cache::new(models_dir.to_path_buf());
    let api = ApiBuilder::from_cache(cache).with_progress(false).build()?;
    Ok(api.repo(Repo::with_revision(
        FASTEMBED_REPO.to_string(),
        RepoType::Model,
        FASTEMBED_REVISION.to_string(),
    )))
}

fn pinned_file(repo: &hf_hub::api::sync::ApiRepo, file: &PinnedModelFile) -> Result<Vec<u8>> {
    let path = repo
        .get(file.path)
        .with_context(|| format!("failed to retrieve pinned FastEmbed file {}", file.path))?;
    verified_file_bytes(&path, file)
}

fn verified_file_bytes(path: &Path, file: &PinnedModelFile) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read pinned FastEmbed file {}", path.display()))?;
    let digest = hex::encode(Sha256::digest(&bytes));
    if digest != file.sha256 {
        bail!(
            "pinned FastEmbed file {} hash mismatch: expected {}, got {}",
            file.path,
            file.sha256,
            digest
        );
    }
    Ok(bytes)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_file_hash_mismatch_is_rejected() {
        let path = std::env::temp_dir().join(format!("ctx-hash-test-{}", std::process::id()));
        std::fs::write(&path, b"model bytes").unwrap();
        let file = PinnedModelFile {
            path: "model.onnx",
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        };

        assert!(verified_file_bytes(&path, &file).is_err());
        let _ = std::fs::remove_file(path);
    }
}
