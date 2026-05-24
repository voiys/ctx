use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use directories::UserDirs;
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

const DEFAULT_TOP_K: usize = 5;
const DEFAULT_BUDGET_TOKENS: usize = 20_000;
const DEFAULT_MAX_PAGES: usize = 256;
const DEFAULT_CRAWL_CONCURRENCY: usize = 16;
const EMBEDDING_BATCH_SIZE: usize = 64;
const RRF_K: f64 = 60.0;
const AGENTS_BLOCK_START: &str = "<!-- ctx:start -->";
const AGENTS_BLOCK_END: &str = "<!-- ctx:end -->";

#[derive(Parser)]
#[command(name = "ctx")]
#[command(about = "Project context for coding agents")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize .ctx/ctx.json and update AGENTS.md.
    Init {
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        no_agents: bool,
    },
    /// Add a source, docs, or notes URL to this project.
    Add {
        url: String,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        no_index: bool,
        #[arg(long, default_value_t = DEFAULT_MAX_PAGES)]
        max_pages: usize,
        #[arg(long, default_value_t = DEFAULT_CRAWL_CONCURRENCY)]
        concurrency: usize,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Refresh a docs/notes snapshot or report a source pin.
    Update {
        target: String,
        #[arg(long)]
        force: bool,
        #[arg(long, default_value_t = DEFAULT_MAX_PAGES)]
        max_pages: usize,
        #[arg(long, default_value_t = DEFAULT_CRAWL_CONCURRENCY)]
        concurrency: usize,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Ensure manifest resources exist locally and are indexed.
    Sync {
        #[arg(long)]
        reindex: bool,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Query project docs and notes.
    Query {
        question: String,
        #[arg(long, default_value_t = DEFAULT_TOP_K)]
        top_k: usize,
        #[arg(long, default_value_t = DEFAULT_BUDGET_TOKENS)]
        budget: usize,
        #[arg(long)]
        debug: bool,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        kind: Option<QueryKind>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Show project or resource state.
    Show {
        target: Option<String>,
        #[arg(long)]
        snapshots: bool,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// List globally cached resources.
    List {
        #[arg(long)]
        project: bool,
        #[arg(long)]
        kind: Option<ResourceKind>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Print the local path for a cached GitHub source.
    Path {
        target: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Move the current manifest pointer for a resource.
    Use {
        label: String,
        pointer: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Remove a resource from the project manifest.
    Remove {
        target: String,
        #[arg(long)]
        prune_cache: bool,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Check manifest, cache, and index health.
    Doctor {
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Copy the current ctx binary into ~/.local/bin or a requested directory.
    Install {
        #[arg(long)]
        bin_dir: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum ResourceKind {
    Source,
    Docs,
    Notes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum QueryKind {
    Docs,
    Notes,
}

impl From<QueryKind> for ResourceKind {
    fn from(value: QueryKind) -> Self {
        match value {
            QueryKind::Docs => ResourceKind::Docs,
            QueryKind::Notes => ResourceKind::Notes,
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Manifest {
    version: u32,
    defaults: Defaults,
    resources: Vec<Resource>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Defaults {
    top_k: usize,
    budget_tokens: usize,
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
struct Resource {
    id: String,
    label: String,
    kind: ResourceKind,
    url: String,
    reason: Option<String>,
    current: String,
    local_path: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct CommandStatus<T: Serialize> {
    command: &'static str,
    status: &'static str,
    result: T,
}

#[derive(Debug)]
struct RuntimePaths {
    project_root: PathBuf,
    manifest_path: PathBuf,
    ctx_dir: PathBuf,
    home: PathBuf,
    db_path: PathBuf,
}

#[derive(Debug)]
enum ResolvedInput {
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
}

#[derive(Debug, Serialize)]
struct SnapshotMetadata {
    snapshot_id: String,
    fetched_at: String,
    source_url: String,
    content_hash: String,
    page_count: usize,
    path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SnapshotPage {
    url: String,
    content: String,
}

#[derive(Clone, Debug)]
struct CandidateBase {
    chunk_id: i64,
    resource_id: String,
    snapshot_id: String,
    kind: String,
    label: String,
    source_url: String,
    chunk_index: i64,
    content: String,
}

#[derive(Clone, Debug)]
struct CandidateScore {
    base: CandidateBase,
    final_score: f64,
    lexical_rank: Option<usize>,
    lexical_score: Option<f64>,
    vector_rank: Option<usize>,
    vector_score: Option<f64>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init { cwd, no_agents } => init(cwd, !no_agents),
        Commands::Add {
            url,
            label,
            reason,
            no_index,
            max_pages,
            concurrency,
            cwd,
        } => add(cwd, &url, label, reason, !no_index, max_pages, concurrency),
        Commands::Update {
            target,
            force,
            max_pages,
            concurrency,
            cwd,
        } => update(cwd, &target, force, max_pages, concurrency),
        Commands::Sync { reindex, cwd } => sync(cwd, reindex),
        Commands::Query {
            question,
            top_k,
            budget,
            debug,
            label,
            kind,
            cwd,
        } => query(cwd, &question, top_k, budget, debug, label, kind),
        Commands::Show {
            target,
            snapshots,
            cwd,
        } => show(cwd, target, snapshots),
        Commands::List { project, kind, cwd } => list(cwd, project, kind),
        Commands::Path { target, cwd } => print_source_path(cwd, &target),
        Commands::Use {
            label,
            pointer,
            cwd,
        } => use_pointer(cwd, &label, &pointer),
        Commands::Remove {
            target,
            prune_cache,
            cwd,
        } => remove(cwd, &target, prune_cache),
        Commands::Doctor { cwd } => doctor(cwd),
        Commands::Install { bin_dir, force } => install(bin_dir, force),
    }
}

fn init(cwd: Option<PathBuf>, write_agents: bool) -> Result<()> {
    let paths = runtime_paths(cwd)?;
    fs::create_dir_all(&paths.ctx_dir)?;
    fs::create_dir_all(&paths.home)?;
    ensure_db(&paths.db_path)?;

    if !paths.manifest_path.exists() {
        write_manifest(
            &paths.manifest_path,
            &Manifest {
                version: 1,
                defaults: Defaults::default(),
                resources: Vec::new(),
            },
        )?;
    }

    if write_agents {
        upsert_agents_block(&paths.project_root)?;
    }

    print_toon(CommandStatus {
        command: "init",
        status: "ok",
        result: json!({
            "project_root": paths.project_root,
            "manifest_path": paths.manifest_path,
            "agents_updated": write_agents,
        }),
    })
}

fn add(
    cwd: Option<PathBuf>,
    input_url: &str,
    label: Option<String>,
    reason: Option<String>,
    should_index: bool,
    max_pages: usize,
    concurrency: usize,
) -> Result<()> {
    let paths = runtime_paths(cwd)?;
    ensure_project(&paths)?;
    let mut manifest = read_manifest(&paths.manifest_path)?;
    let resolved = resolve_input(input_url)?;
    let now = timestamp();

    let (resource, extra) = match resolved {
        ResolvedInput::GithubSource {
            owner,
            repo,
            requested_ref,
            clone_url,
        } => {
            let label = label.unwrap_or_else(|| repo.clone());
            let id = stable_id(&format!("source:{clone_url}:{label}"));
            let (commit, path) = cache_github_source(
                &paths.home,
                &owner,
                &repo,
                requested_ref.as_deref(),
                &clone_url,
            )?;
            let resource = Resource {
                id: id.clone(),
                label,
                kind: ResourceKind::Source,
                url: input_url.to_string(),
                reason,
                current: commit.clone(),
                local_path: Some(path.display().to_string()),
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            upsert_global_resource(&paths.db_path, &resource, None)?;
            (resource, json!({"commit": commit, "path": path}))
        }
        ResolvedInput::Docs { url } => {
            let label = label.unwrap_or_else(|| default_label_for_url(&url));
            let id = stable_id(&format!("docs:{url}:{label}"));
            let snapshot = snapshot_docs(
                &paths.home,
                &id,
                &url,
                should_index,
                &paths.db_path,
                max_pages,
                concurrency,
            )?;
            let resource = Resource {
                id,
                label,
                kind: ResourceKind::Docs,
                url,
                reason,
                current: snapshot.snapshot_id.clone(),
                local_path: Some(snapshot.path.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            upsert_global_resource(&paths.db_path, &resource, Some(&snapshot))?;
            if should_index {
                index_snapshot(&paths.db_path, &resource, &snapshot)?;
            }
            (resource, json!({"snapshot": snapshot}))
        }
        ResolvedInput::Notes { url, path } => {
            let label = label.unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("notes")
                    .to_string()
            });
            let id = stable_id(&format!("notes:{url}:{label}"));
            let snapshot =
                snapshot_notes(&paths.home, &id, &url, &path, should_index, &paths.db_path)?;
            let resource = Resource {
                id,
                label,
                kind: ResourceKind::Notes,
                url,
                reason,
                current: snapshot.snapshot_id.clone(),
                local_path: Some(snapshot.path.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            upsert_global_resource(&paths.db_path, &resource, Some(&snapshot))?;
            if should_index {
                index_snapshot(&paths.db_path, &resource, &snapshot)?;
            }
            (resource, json!({"snapshot": snapshot}))
        }
    };

    upsert_manifest_resource(&mut manifest, resource.clone());
    write_manifest(&paths.manifest_path, &manifest)?;

    print_toon(CommandStatus {
        command: "add",
        status: "ok",
        result: json!({
            "resource": resource,
            "extra": extra,
        }),
    })
}

fn update(
    cwd: Option<PathBuf>,
    target: &str,
    force: bool,
    max_pages: usize,
    concurrency: usize,
) -> Result<()> {
    let paths = runtime_paths(cwd)?;
    ensure_project(&paths)?;
    let mut manifest = read_manifest(&paths.manifest_path)?;
    let index = find_manifest_resource_index(&manifest, target)?;
    let mut resource = manifest.resources[index].clone();

    match resource.kind {
        ResourceKind::Source => {
            print_toon(CommandStatus {
                command: "update",
                status: "ok",
                result: json!({
                    "message": "source resources are pinned; add or use an explicit ref to change them",
                    "resource": resource,
                }),
            })?;
        }
        ResourceKind::Docs => {
            let snapshot = snapshot_docs(
                &paths.home,
                &resource.id,
                &resource.url,
                true,
                &paths.db_path,
                max_pages,
                concurrency,
            )?;
            let changed = current_content_hash(&paths.db_path, &resource.id, &resource.current)?
                .is_none_or(|hash| hash != snapshot.content_hash);
            if changed || force {
                resource.current = snapshot.snapshot_id.clone();
                resource.local_path = Some(snapshot.path.clone());
                resource.updated_at = timestamp();
                manifest.resources[index] = resource.clone();
                write_manifest(&paths.manifest_path, &manifest)?;
                upsert_global_resource(&paths.db_path, &resource, Some(&snapshot))?;
                index_snapshot(&paths.db_path, &resource, &snapshot)?;
            } else {
                let _ = fs::remove_dir_all(&snapshot.path);
            }
            print_toon(CommandStatus {
                command: "update",
                status: "ok",
                result: json!({
                    "changed": changed,
                    "resource": resource,
                    "snapshot": snapshot,
                }),
            })?;
        }
        ResourceKind::Notes => {
            let file_path = Url::parse(&resource.url)
                .ok()
                .and_then(|url| url.to_file_path().ok())
                .ok_or_else(|| anyhow!("notes URL is not a valid file URL"))?;
            let snapshot = snapshot_notes(
                &paths.home,
                &resource.id,
                &resource.url,
                &file_path,
                true,
                &paths.db_path,
            )?;
            let changed = current_content_hash(&paths.db_path, &resource.id, &resource.current)?
                .is_none_or(|hash| hash != snapshot.content_hash);
            if changed || force {
                resource.current = snapshot.snapshot_id.clone();
                resource.local_path = Some(snapshot.path.clone());
                resource.updated_at = timestamp();
                manifest.resources[index] = resource.clone();
                write_manifest(&paths.manifest_path, &manifest)?;
                upsert_global_resource(&paths.db_path, &resource, Some(&snapshot))?;
                index_snapshot(&paths.db_path, &resource, &snapshot)?;
            } else {
                let _ = fs::remove_dir_all(&snapshot.path);
            }
            print_toon(CommandStatus {
                command: "update",
                status: "ok",
                result: json!({
                    "changed": changed,
                    "resource": resource,
                    "snapshot": snapshot,
                }),
            })?;
        }
    }
    Ok(())
}

fn sync(cwd: Option<PathBuf>, reindex: bool) -> Result<()> {
    let paths = runtime_paths(cwd)?;
    ensure_project(&paths)?;
    ensure_db(&paths.db_path)?;
    let manifest = read_manifest(&paths.manifest_path)?;
    let mut checked = Vec::new();

    for resource in &manifest.resources {
        let ready = match resource.kind {
            ResourceKind::Source => resource
                .local_path
                .as_deref()
                .map(Path::new)
                .is_some_and(Path::exists),
            ResourceKind::Docs | ResourceKind::Notes => {
                let path_ready = resource
                    .local_path
                    .as_deref()
                    .map(Path::new)
                    .is_some_and(Path::exists);
                if reindex
                    && path_ready
                    && let Some(path) = &resource.local_path
                {
                    let snapshot = SnapshotMetadata {
                        snapshot_id: resource.current.clone(),
                        fetched_at: resource.updated_at.clone(),
                        source_url: resource.url.clone(),
                        content_hash: current_content_hash(
                            &paths.db_path,
                            &resource.id,
                            &resource.current,
                        )?
                        .unwrap_or_default(),
                        page_count: 0,
                        path: path.clone(),
                    };
                    index_snapshot(&paths.db_path, resource, &snapshot)?;
                }
                path_ready
            }
        };
        checked.push(json!({
            "label": resource.label,
            "kind": resource.kind,
            "ready": ready,
        }));
    }

    print_toon(CommandStatus {
        command: "sync",
        status: "ok",
        result: json!({
            "project_root": paths.project_root,
            "reindexed": reindex,
            "resources": checked,
        }),
    })
}

fn query(
    cwd: Option<PathBuf>,
    question: &str,
    top_k: usize,
    budget_tokens: usize,
    debug: bool,
    label: Option<String>,
    kind: Option<QueryKind>,
) -> Result<()> {
    let paths = runtime_paths(cwd)?;
    ensure_project(&paths)?;
    ensure_db(&paths.db_path)?;
    let manifest = read_manifest(&paths.manifest_path)?;
    let allowed = allowed_resource_ids(&manifest, label.as_deref(), kind.map(Into::into))?;
    let results = query_index(
        &paths.db_path,
        question,
        &allowed,
        top_k,
        budget_tokens,
        debug,
    )?;
    print_toon(CommandStatus {
        command: "query",
        status: "ok",
        result: json!({
            "question": question,
            "top_k": top_k,
            "budget_tokens": budget_tokens,
            "project_root": paths.project_root,
            "debug": debug,
            "results": results,
        }),
    })
}

fn show(cwd: Option<PathBuf>, target: Option<String>, snapshots: bool) -> Result<()> {
    let paths = runtime_paths(cwd)?;
    ensure_project(&paths)?;
    let manifest = read_manifest(&paths.manifest_path)?;
    let resources = if let Some(target) = target {
        vec![find_manifest_resource(&manifest, &target)?.clone()]
    } else {
        manifest.resources.clone()
    };
    let snapshot_rows = if snapshots {
        snapshots_for_resources(&paths.db_path, &resources)?
    } else {
        Vec::new()
    };
    print_toon(CommandStatus {
        command: "show",
        status: "ok",
        result: json!({
            "project_root": paths.project_root,
            "manifest_path": paths.manifest_path,
            "resources": resources,
            "snapshots": snapshot_rows,
        }),
    })
}

fn list(cwd: Option<PathBuf>, project_only: bool, kind: Option<ResourceKind>) -> Result<()> {
    let paths = runtime_paths(cwd)?;
    ensure_db(&paths.db_path)?;
    let project_resources = if project_only || paths.manifest_path.exists() {
        read_manifest(&paths.manifest_path)
            .map(|manifest| manifest.resources)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let project_ids = project_resources
        .iter()
        .map(|resource| resource.id.clone())
        .collect::<BTreeSet<_>>();
    let rows = list_global_resources(&paths.db_path, kind)?;
    let filtered = rows
        .into_iter()
        .filter(|row| !project_only || project_ids.contains(row["id"].as_str().unwrap_or_default()))
        .map(|mut row| {
            if let Some(object) = row.as_object_mut() {
                let linked = object
                    .get("id")
                    .and_then(|id| id.as_str())
                    .is_some_and(|id| project_ids.contains(id));
                object.insert("linked_to_current_project".to_string(), json!(linked));
            }
            row
        })
        .collect::<Vec<_>>();
    print_toon(CommandStatus {
        command: "list",
        status: "ok",
        result: json!({
            "home": paths.home,
            "resources": filtered,
        }),
    })
}

fn print_source_path(cwd: Option<PathBuf>, target: &str) -> Result<()> {
    let paths = runtime_paths(cwd)?;
    ensure_project(&paths)?;
    let manifest = read_manifest(&paths.manifest_path)?;
    let resource = find_manifest_resource(&manifest, target)?;
    if resource.kind != ResourceKind::Source {
        bail!("ctx path only supports source resources");
    }
    let Some(path) = &resource.local_path else {
        bail!("source resource has no cached path");
    };
    println!("{path}");
    Ok(())
}

fn use_pointer(cwd: Option<PathBuf>, label: &str, pointer: &str) -> Result<()> {
    let paths = runtime_paths(cwd)?;
    ensure_project(&paths)?;
    let mut manifest = read_manifest(&paths.manifest_path)?;
    let index = find_manifest_resource_index(&manifest, label)?;
    match manifest.resources[index].kind {
        ResourceKind::Source => validate_source_pointer(&manifest.resources[index], pointer)?,
        ResourceKind::Docs | ResourceKind::Notes => {
            let snapshot_path =
                snapshot_path_for_pointer(&paths.db_path, &manifest.resources[index].id, pointer)?
                    .ok_or_else(|| anyhow!("snapshot not found for {}: {pointer}", label))?;
            manifest.resources[index].local_path = Some(snapshot_path);
        }
    }
    manifest.resources[index].current = pointer.to_string();
    manifest.resources[index].updated_at = timestamp();
    write_manifest(&paths.manifest_path, &manifest)?;
    print_toon(CommandStatus {
        command: "use",
        status: "ok",
        result: json!({
            "resource": manifest.resources[index],
        }),
    })
}

fn remove(cwd: Option<PathBuf>, target: &str, prune_cache: bool) -> Result<()> {
    let paths = runtime_paths(cwd)?;
    ensure_project(&paths)?;
    let mut manifest = read_manifest(&paths.manifest_path)?;
    let removed = find_manifest_resource(&manifest, target)?.clone();
    let before = manifest.resources.len();
    manifest.resources.retain(|resource| {
        resource.label != target && resource.url != target && resource.id != target
    });
    if manifest.resources.len() == before {
        bail!("resource not found: {target}");
    }
    write_manifest(&paths.manifest_path, &manifest)?;
    let pruned = if prune_cache {
        prune_resource_cache(&paths.db_path, &removed)?
    } else {
        false
    };
    print_toon(CommandStatus {
        command: "remove",
        status: "ok",
        result: json!({
            "target": target,
            "prune_cache_requested": prune_cache,
            "pruned_cache": pruned,
        }),
    })
}

fn doctor(cwd: Option<PathBuf>) -> Result<()> {
    let paths = runtime_paths(cwd)?;
    let manifest_exists = paths.manifest_path.exists();
    let db_ok = ensure_db(&paths.db_path).is_ok();
    let resource_count = if manifest_exists {
        read_manifest(&paths.manifest_path)
            .map(|manifest| manifest.resources.len())
            .unwrap_or_default()
    } else {
        0
    };
    print_toon(CommandStatus {
        command: "doctor",
        status: "ok",
        result: json!({
            "project_root": paths.project_root,
            "manifest_exists": manifest_exists,
            "home": paths.home,
            "db_path": paths.db_path,
            "db_ok": db_ok,
            "project_resource_count": resource_count,
        }),
    })
}

fn install(bin_dir: Option<PathBuf>, force: bool) -> Result<()> {
    let target_dir = match bin_dir {
        Some(path) => path,
        None => UserDirs::new()
            .ok_or_else(|| anyhow!("could not determine home directory"))?
            .home_dir()
            .join(".local")
            .join("bin"),
    };
    fs::create_dir_all(&target_dir)?;
    let target = target_dir.join("ctx");
    if target.exists() && !force {
        bail!(
            "{} already exists; pass --force to replace it",
            target.display()
        );
    }
    let current = std::env::current_exe()?;
    fs::copy(&current, &target)?;
    make_executable(&target)?;
    print_toon(CommandStatus {
        command: "install",
        status: "ok",
        result: json!({
            "source": current,
            "target": target,
        }),
    })
}

fn runtime_paths(cwd: Option<PathBuf>) -> Result<RuntimePaths> {
    let project_root = cwd.unwrap_or(std::env::current_dir()?).canonicalize()?;
    let ctx_dir = project_root.join(".ctx");
    let manifest_path = ctx_dir.join("ctx.json");
    let home = if let Ok(value) = std::env::var("CTX_HOME") {
        PathBuf::from(value)
    } else {
        UserDirs::new()
            .ok_or_else(|| anyhow!("could not determine home directory"))?
            .home_dir()
            .join(".ctx")
    };
    let db_path = home.join("ctx.db");
    Ok(RuntimePaths {
        project_root,
        manifest_path,
        ctx_dir,
        home,
        db_path,
    })
}

fn ensure_project(paths: &RuntimePaths) -> Result<()> {
    if !paths.manifest_path.exists() {
        bail!(
            "no ctx project found at {}; run `ctx init` first",
            paths.manifest_path.display()
        );
    }
    fs::create_dir_all(&paths.home)?;
    ensure_db(&paths.db_path)?;
    Ok(())
}

fn read_manifest(path: &Path) -> Result<Manifest> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read manifest at {}", path.display()))?;
    Ok(serde_json::from_str(&raw)?)
}

fn write_manifest(path: &Path, manifest: &Manifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(manifest)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn upsert_manifest_resource(manifest: &mut Manifest, mut resource: Resource) {
    if manifest.version == 0 {
        manifest.version = 1;
    }
    if let Some(existing) = manifest
        .resources
        .iter_mut()
        .find(|existing| existing.label == resource.label || existing.url == resource.url)
    {
        resource.created_at = existing.created_at.clone();
        *existing = resource;
    } else {
        manifest.resources.push(resource);
    }
}

fn find_manifest_resource<'a>(manifest: &'a Manifest, target: &str) -> Result<&'a Resource> {
    manifest
        .resources
        .iter()
        .find(|resource| {
            resource.label == target || resource.url == target || resource.id == target
        })
        .ok_or_else(|| anyhow!("resource not found: {target}"))
}

fn find_manifest_resource_index(manifest: &Manifest, target: &str) -> Result<usize> {
    manifest
        .resources
        .iter()
        .position(|resource| {
            resource.label == target || resource.url == target || resource.id == target
        })
        .ok_or_else(|| anyhow!("resource not found: {target}"))
}

fn resolve_input(input: &str) -> Result<ResolvedInput> {
    let url = Url::parse(input).map_err(|_| anyhow!("ctx add requires an absolute URL"))?;
    match url.scheme() {
        "http" | "https" => {
            if url.host_str() == Some("github.com") {
                let segments = url
                    .path_segments()
                    .map(|segments| segments.collect::<Vec<_>>())
                    .unwrap_or_default();
                if segments.len() < 2 {
                    bail!("GitHub URL must include owner and repo");
                }
                let owner = segments[0].to_string();
                let repo = segments[1].trim_end_matches(".git").to_string();
                let requested_ref = if segments.get(2) == Some(&"tree") {
                    segments.get(3).map(|value| value.to_string())
                } else {
                    None
                };
                Ok(ResolvedInput::GithubSource {
                    owner,
                    repo,
                    requested_ref,
                    clone_url: format!("https://github.com/{}/{}.git", segments[0], segments[1]),
                })
            } else {
                Ok(ResolvedInput::Docs {
                    url: url.to_string(),
                })
            }
        }
        "file" => {
            let path = url
                .to_file_path()
                .map_err(|_| anyhow!("file URL must point to an absolute local path"))?;
            Ok(ResolvedInput::Notes {
                url: url.to_string(),
                path,
            })
        }
        scheme => bail!("unsupported URL scheme: {scheme}"),
    }
}

fn cache_github_source(
    home: &Path,
    owner: &str,
    repo: &str,
    requested_ref: Option<&str>,
    clone_url: &str,
) -> Result<(String, PathBuf)> {
    let tmp = home
        .join("tmp")
        .join(format!("{}-{}", repo, Uuid::new_v4()));
    fs::create_dir_all(tmp.parent().unwrap())?;
    let mut clone = Command::new("git");
    clone.arg("clone").arg("--depth").arg("1");
    if let Some(reference) = requested_ref {
        clone.arg("--branch").arg(reference);
    }
    clone.arg(clone_url).arg(&tmp);
    run_command(&mut clone, "git clone")?;

    let output = Command::new("git")
        .arg("-C")
        .arg(&tmp)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .context("failed to run git rev-parse")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let commit = String::from_utf8(output.stdout)?.trim().to_string();
    let final_path = home
        .join("sources")
        .join("github.com")
        .join(owner)
        .join(repo)
        .join(&commit);
    if final_path.exists() {
        fs::remove_dir_all(&tmp)?;
    } else {
        fs::create_dir_all(final_path.parent().unwrap())?;
        fs::rename(&tmp, &final_path)?;
    }
    Ok((commit, final_path))
}

fn snapshot_docs(
    home: &Path,
    resource_id: &str,
    url: &str,
    should_index: bool,
    db_path: &Path,
    max_pages: usize,
    concurrency: usize,
) -> Result<SnapshotMetadata> {
    eprintln!("crawling docs: {url}");
    let pages = crawl_docs(url, max_pages, concurrency)?;
    write_snapshot_pages(
        home,
        ResourceKind::Docs,
        resource_id,
        url,
        pages,
        should_index,
        db_path,
    )
}

fn crawl_docs(seed: &str, max_pages: usize, concurrency: usize) -> Result<Vec<SnapshotPage>> {
    let seed_url = Url::parse(seed)?;
    let max_pages = max_pages.max(1);
    let concurrency = concurrency.max(1);
    let client = reqwest::blocking::Client::builder()
        .user_agent("ctx/0.1")
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(concurrency)
        .build()?;
    let mut seen = HashSet::from([canonical_url(&seed_url)]);
    let mut frontier = vec![seed_url.clone()];
    let mut pages = Vec::new();

    while !frontier.is_empty() && pages.len() < max_pages {
        let remaining = max_pages - pages.len();
        let batch = frontier.into_iter().take(remaining).collect::<Vec<_>>();
        let fetched = pool.install(|| {
            batch
                .par_iter()
                .filter_map(|url| fetch_doc_page(&client, url).ok())
                .collect::<Vec<_>>()
        });
        let mut next = BTreeSet::new();
        for fetched_page in fetched {
            let page_url = fetched_page.url.clone();
            for link in &fetched_page.links {
                if is_crawlable_doc_url(&seed_url, link) {
                    let canonical = canonical_url(link);
                    if seen.insert(canonical.clone()) {
                        next.insert(canonical);
                    }
                }
            }
            pages.push(SnapshotPage {
                url: page_url,
                content: fetched_page.text,
            });
            if pages.len() >= max_pages {
                break;
            }
        }
        frontier = next
            .into_iter()
            .filter_map(|value| Url::parse(&value).ok())
            .collect();
    }

    if pages.is_empty() {
        bail!("no crawlable docs pages were fetched from {seed}");
    }
    Ok(pages)
}

struct FetchedDocPage {
    url: String,
    text: String,
    links: Vec<Url>,
}

fn fetch_doc_page(client: &reqwest::blocking::Client, url: &Url) -> Result<FetchedDocPage> {
    eprintln!("fetching docs page: {url}");
    let html = client.get(url.clone()).send()?.error_for_status()?.text()?;
    let links = html_links(url, &html);
    Ok(FetchedDocPage {
        url: canonical_url(url),
        text: html_to_text(&html),
        links,
    })
}

fn html_links(base: &Url, html: &str) -> Vec<Url> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("a[href]").expect("static selector");
    document
        .select(&selector)
        .filter_map(|node| node.value().attr("href"))
        .filter_map(|href| base.join(href).ok())
        .map(strip_fragment_and_query)
        .collect()
}

fn is_crawlable_doc_url(seed: &Url, candidate: &Url) -> bool {
    if candidate.scheme() != seed.scheme()
        || candidate.host_str() != seed.host_str()
        || candidate.port_or_known_default() != seed.port_or_known_default()
    {
        return false;
    }
    let seed_path = normalized_crawl_root(seed.path());
    if !candidate.path().starts_with(&seed_path) {
        return false;
    }
    !looks_like_asset(candidate.path())
}

fn normalized_crawl_root(path: &str) -> String {
    if path == "/" || path.is_empty() {
        "/".to_string()
    } else {
        format!("{}/", path.trim_end_matches('/'))
    }
}

fn looks_like_asset(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".ico", ".css", ".js", ".mjs", ".map",
        ".pdf", ".zip", ".tar", ".gz", ".mp4", ".webm", ".woff", ".woff2", ".ttf",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

fn strip_fragment_and_query(mut url: Url) -> Url {
    url.set_fragment(None);
    url.set_query(None);
    url
}

fn canonical_url(url: &Url) -> String {
    strip_fragment_and_query(url.clone()).to_string()
}

fn write_snapshot_pages(
    home: &Path,
    kind: ResourceKind,
    resource_id: &str,
    source_url: &str,
    pages: Vec<SnapshotPage>,
    _should_index: bool,
    _db_path: &Path,
) -> Result<SnapshotMetadata> {
    let mut hash_input = String::new();
    let mut combined = String::new();
    for page in &pages {
        hash_input.push_str(&page.url);
        hash_input.push('\n');
        hash_input.push_str(&page.content);
        hash_input.push('\n');
        combined.push_str("# ");
        combined.push_str(&page.url);
        combined.push_str("\n\n");
        combined.push_str(&page.content);
        combined.push_str("\n\n");
    }
    let hash = content_hash(&hash_input);
    let fetched_at = timestamp();
    let snapshot_id = format!("{}-{}", fetched_at.replace([':', '-'], ""), &hash[..12]);
    let root = match kind {
        ResourceKind::Docs => home.join("docs"),
        ResourceKind::Notes => home.join("notes"),
        ResourceKind::Source => bail!("source snapshots are not supported"),
    };
    let path = root.join(resource_id).join(&snapshot_id);
    fs::create_dir_all(&path)?;
    fs::write(path.join("content.txt"), &combined)?;
    fs::write(
        path.join("pages.json"),
        serde_json::to_string_pretty(&pages)?,
    )?;
    let metadata = SnapshotMetadata {
        snapshot_id,
        fetched_at,
        source_url: source_url.to_string(),
        content_hash: format!("sha256:{hash}"),
        page_count: pages.len(),
        path: path.display().to_string(),
    };
    fs::write(
        path.join("snapshot.json"),
        serde_json::to_string_pretty(&metadata)?,
    )?;
    Ok(metadata)
}

fn snapshot_notes(
    home: &Path,
    resource_id: &str,
    url: &str,
    path: &Path,
    should_index: bool,
    db_path: &Path,
) -> Result<SnapshotMetadata> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read notes file {}", path.display()))?;
    write_snapshot_pages(
        home,
        ResourceKind::Notes,
        resource_id,
        url,
        vec![SnapshotPage {
            url: url.to_string(),
            content: text,
        }],
        should_index,
        db_path,
    )
}

fn index_snapshot(db_path: &Path, resource: &Resource, snapshot: &SnapshotMetadata) -> Result<()> {
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
    let embeddings = if embeddings_enabled() {
        embed_passages(db_path, &contents)?
    } else {
        vec![None; contents.len()]
    };

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

fn embeddings_enabled() -> bool {
    std::env::var("CTX_EMBEDDINGS")
        .map(|value| !matches!(value.as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(true)
}

fn embed_passages(db_path: &Path, contents: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
    if contents.is_empty() {
        return Ok(Vec::new());
    }
    let mut model = embedding_model(db_path)?;
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

fn embed_query(db_path: &Path, question: &str) -> Result<Vec<f32>> {
    let mut model = embedding_model(db_path)?;
    let embeddings = model.embed(vec![format!("query: {question}")], None)?;
    embeddings
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("embedding model returned no query embedding"))
}

fn embedding_model(db_path: &Path) -> Result<TextEmbedding> {
    let models_dir = db_path
        .parent()
        .ok_or_else(|| anyhow!("database path has no parent"))?
        .join("models");
    fs::create_dir_all(&models_dir)?;
    let options = TextInitOptions::new(EmbeddingModel::AllMiniLML6V2Q)
        .with_cache_dir(models_dir)
        .with_show_download_progress(false);
    TextEmbedding::try_new(options)
}

fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
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

fn html_to_text(html: &str) -> String {
    let document = Html::parse_document(html);
    let selector = Selector::parse("main, article, body").expect("static selector");
    let mut text = String::new();
    for node in document.select(&selector).take(1) {
        for part in node.text() {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                text.push_str(trimmed);
                text.push('\n');
            }
        }
    }
    if text.trim().is_empty() {
        html.to_string()
    } else {
        text
    }
}

fn ensure_db(path: &Path) -> Result<()> {
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

fn upsert_global_resource(
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

fn query_index(
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
    let vector = if embeddings_enabled() {
        vector_candidates(db_path, question, allowed_resource_ids, top_k.max(50) * 10)?
    } else {
        Vec::new()
    };
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
    let query_embedding = embed_query(db_path, question)?;
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
            lexical_rank: None,
            lexical_score: None,
            vector_rank: None,
            vector_score: None,
        });
        entry.final_score += 1.0 / (RRF_K + rank as f64);
        entry.vector_rank = Some(rank);
        entry.vector_score = Some(score);
    }
    let mut out = by_chunk.into_values().collect::<Vec<_>>();
    out.sort_by(|a, b| {
        b.final_score
            .partial_cmp(&a.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

fn list_global_resources(
    db_path: &Path,
    kind: Option<ResourceKind>,
) -> Result<Vec<serde_json::Value>> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT r.id, r.label, r.kind, r.url, r.current, r.local_path, r.updated_at,
                COUNT(s.snapshot_id) AS snapshot_count
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
        out.push(row);
    }
    Ok(out)
}

fn snapshots_for_resources(
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

fn current_content_hash(
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

fn snapshot_path_for_pointer(
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

fn validate_source_pointer(resource: &Resource, pointer: &str) -> Result<()> {
    if resource.current == pointer {
        return Ok(());
    }
    bail!("source pointer changes must be done by adding or caching an explicit GitHub ref first")
}

fn prune_resource_cache(db_path: &Path, resource: &Resource) -> Result<bool> {
    ensure_db(db_path)?;
    if let Some(path) = &resource.local_path {
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

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn allowed_resource_ids(
    manifest: &Manifest,
    label: Option<&str>,
    kind: Option<ResourceKind>,
) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    for resource in &manifest.resources {
        if matches!(resource.kind, ResourceKind::Source) {
            continue;
        }
        if let Some(label) = label
            && resource.label != label
        {
            continue;
        }
        if let Some(kind) = kind
            && resource.kind != kind
        {
            continue;
        }
        out.insert(resource.id.clone());
    }
    if label.is_some() && out.is_empty() {
        bail!("no queryable docs/notes resource matched label");
    }
    Ok(out)
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

fn run_command(command: &mut Command, label: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("failed to run {label}"))?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

fn upsert_agents_block(project_root: &Path) -> Result<()> {
    let path = project_root.join("AGENTS.md");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let block = format!(
        r#"{AGENTS_BLOCK_START}
## ctx

Use `ctx` for this project's local context.

- `ctx query "<question>"` searches project docs and notes.
- `ctx query "<question>" --debug` includes ranking details.
- `ctx path <label>` prints the local path for pinned source repos.
- `ctx show` inspects the project manifest.
- `ctx list --project` shows linked resources.

Source repos are explored on disk. Docs and notes are returned as cited context blocks.
{AGENTS_BLOCK_END}"#
    );
    let updated = if let Some(start) = existing.find(AGENTS_BLOCK_START) {
        if let Some(end_rel) = existing[start..].find(AGENTS_BLOCK_END) {
            let end = start + end_rel + AGENTS_BLOCK_END.len();
            format!(
                "{}{}{}",
                existing[..start].trim_end(),
                if existing[..start].trim().is_empty() {
                    ""
                } else {
                    "\n\n"
                },
                block
            ) + if existing[end..].trim().is_empty() {
                "\n"
            } else {
                "\n\n"
            } + existing[end..].trim_start()
        } else {
            format!("{}\n\n{}\n", existing.trim_end(), block)
        }
    } else if existing.trim().is_empty() {
        format!("{block}\n")
    } else {
        format!("{}\n\n{}\n", existing.trim_end(), block)
    };
    let mut file = fs::File::create(path)?;
    file.write_all(updated.as_bytes())?;
    Ok(())
}

fn print_toon<T: Serialize>(value: T) -> Result<()> {
    let encoded = toon_format::encode_default(&value)?;
    println!("{encoded}");
    Ok(())
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn stable_id(input: &str) -> String {
    let hash = content_hash(input);
    hash[..16].to_string()
}

fn content_hash(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

fn default_label_for_url(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host.trim_start_matches("www.").replace(['.', ':'], "-"))
        })
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "resource".to_string())
}

fn kind_str(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Source => "source",
        ResourceKind::Docs => "docs",
        ResourceKind::Notes => "notes",
    }
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
    fn classifies_github_as_source() {
        let resolved = resolve_input("https://github.com/owner/repo/tree/v1").unwrap();
        match resolved {
            ResolvedInput::GithubSource {
                owner,
                repo,
                requested_ref,
                ..
            } => {
                assert_eq!(owner, "owner");
                assert_eq!(repo, "repo");
                assert_eq!(requested_ref.as_deref(), Some("v1"));
            }
            _ => panic!("expected github source"),
        }
    }

    #[test]
    fn rejects_bare_paths() {
        assert!(resolve_input("/tmp/notes.md").is_err());
    }
}
