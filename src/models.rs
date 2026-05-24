use std::path::PathBuf;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::constants::{DEFAULT_BUDGET_TOKENS, DEFAULT_TOP_K};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResourceKind {
    Source,
    Docs,
    Notes,
    Arxiv,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum QueryKind {
    Docs,
    Notes,
    Arxiv,
}

impl From<QueryKind> for ResourceKind {
    fn from(value: QueryKind) -> Self {
        match value {
            QueryKind::Docs => ResourceKind::Docs,
            QueryKind::Notes => ResourceKind::Notes,
            QueryKind::Arxiv => ResourceKind::Arxiv,
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub(crate) struct Manifest {
    pub(crate) version: u32,
    pub(crate) defaults: Defaults,
    pub(crate) resources: Vec<Resource>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Defaults {
    pub(crate) top_k: usize,
    pub(crate) budget_tokens: usize,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            top_k: DEFAULT_TOP_K,
            budget_tokens: DEFAULT_BUDGET_TOKENS,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Resource {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) kind: ResourceKind,
    pub(crate) url: String,
    pub(crate) reason: Option<String>,
    pub(crate) current: String,
    pub(crate) local_path: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommandStatus<T: Serialize> {
    pub(crate) command: &'static str,
    pub(crate) status: &'static str,
    pub(crate) result: T,
}

#[derive(Debug)]
pub(crate) struct RuntimePaths {
    pub(crate) project_root: PathBuf,
    pub(crate) manifest_path: PathBuf,
    pub(crate) ctx_dir: PathBuf,
    pub(crate) home: PathBuf,
    pub(crate) db_path: PathBuf,
}

#[derive(Debug)]
pub(crate) enum ResolvedInput {
    GithubSource {
        owner: String,
        repo: String,
        requested_ref: Option<String>,
        clone_url: String,
    },
    Docs {
        url: String,
    },
    Notes {
        url: String,
        path: PathBuf,
    },
    ArxivPaper {
        id: String,
        abs_url: String,
    },
}

#[derive(Debug, Serialize)]
pub(crate) struct SnapshotMetadata {
    pub(crate) snapshot_id: String,
    pub(crate) fetched_at: String,
    pub(crate) source_url: String,
    pub(crate) content_hash: String,
    pub(crate) page_count: usize,
    pub(crate) path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SnapshotPage {
    pub(crate) url: String,
    pub(crate) content: String,
}

#[derive(Clone, Debug)]
pub(crate) struct CandidateBase {
    pub(crate) chunk_id: i64,
    pub(crate) resource_id: String,
    pub(crate) snapshot_id: String,
    pub(crate) kind: String,
    pub(crate) label: String,
    pub(crate) source_url: String,
    pub(crate) chunk_index: i64,
    pub(crate) content: String,
}

#[derive(Clone, Debug)]
pub(crate) struct CandidateScore {
    pub(crate) base: CandidateBase,
    pub(crate) final_score: f64,
    pub(crate) source_prior: Option<String>,
    pub(crate) source_prior_score: f64,
    pub(crate) lexical_rank: Option<usize>,
    pub(crate) lexical_score: Option<f64>,
    pub(crate) vector_rank: Option<usize>,
    pub(crate) vector_score: Option<f64>,
}
