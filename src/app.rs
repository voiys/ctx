use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use clap::{Parser, Subcommand};
use serde_json::json;
use url::Url;

use crate::agents::upsert_agents_block;
use crate::arxiv::snapshot_arxiv;
use crate::constants::{
    DEFAULT_BUDGET_TOKENS, DEFAULT_CRAWL_CONCURRENCY, DEFAULT_MAX_PAGES, DEFAULT_TOP_K,
};
use crate::context::AppContext;
use crate::input::resolve_input;
use crate::install::install;
use crate::manifest::{
    allowed_resource_ids, find_manifest_resource, find_manifest_resource_index, read_manifest,
    upsert_manifest_resource, write_manifest,
};
use crate::models::{
    CommandStatus, Defaults, Manifest, QueryKind, ResolvedInput, Resource, ResourceKind,
    SnapshotMetadata,
};
use crate::output::print_toon;
use crate::retrieve::query_index;
use crate::snapshot::{snapshot_docs, snapshot_notes};
use crate::source::{cache_github_source, validate_source_pointer};
use crate::storage::{
    allowed_global_resource_ids, current_content_hash, ensure_db, find_global_resource,
    index_snapshot, list_global_resource_models, list_global_resources, remove_global_resource,
    snapshot_path_for_pointer, snapshots_for_resources, upsert_global_resource,
};
use crate::util::{default_label_for_url, stable_id, timestamp};

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
    /// Add a source, docs, or notes URL to the global cache.
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
    /// Refresh a queryable snapshot or report a source pin.
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
    /// Query project docs, arXiv papers, and notes.
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
    /// Link a global resource into this project.
    Link {
        target: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Unlink a resource from this project manifest.
    Unlink {
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
    /// Remove a global resource.
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

pub fn run() -> Result<()> {
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
        Commands::Link {
            target,
            reason,
            cwd,
        } => link(cwd, &target, reason),
        Commands::Unlink { target, cwd } => unlink(cwd, &target),
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
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
    app.init_storage()?;

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
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
    app.ensure_global_storage()?;
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
            let snapshot = snapshot_docs(&paths.home, &id, &url, max_pages, concurrency)?;
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
            let snapshot = snapshot_notes(&paths.home, &id, &url, &path)?;
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
        ResolvedInput::ArxivPaper {
            id: arxiv_id,
            abs_url,
        } => {
            let label = label.unwrap_or_else(|| format!("arxiv-{arxiv_id}").replace('/', "-"));
            let id = stable_id(&format!("arxiv:{abs_url}:{label}"));
            let snapshot = snapshot_arxiv(&paths.home, &id, &arxiv_id, &abs_url)?;
            let resource = Resource {
                id,
                label,
                kind: ResourceKind::Arxiv,
                url: abs_url,
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
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
    app.ensure_global_storage()?;
    let mut manifest = read_optional_manifest(paths)?;
    let linked_index = manifest
        .as_ref()
        .and_then(|manifest| find_manifest_resource_index(manifest, target).ok());
    let mut resource = if let (Some(manifest), Some(index)) = (manifest.as_ref(), linked_index) {
        manifest.resources[index].clone()
    } else {
        find_global_resource(&paths.db_path, target)?
    };

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
                max_pages,
                concurrency,
            )?;
            let changed = current_content_hash(&paths.db_path, &resource.id, &resource.current)?
                .is_none_or(|hash| hash != snapshot.content_hash);
            if changed || force {
                resource.current = snapshot.snapshot_id.clone();
                resource.local_path = Some(snapshot.path.clone());
                resource.updated_at = timestamp();
                if let (Some(manifest), Some(index)) = (&mut manifest, linked_index) {
                    manifest.resources[index] = resource.clone();
                    write_manifest(&paths.manifest_path, manifest)?;
                }
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
            let snapshot = snapshot_notes(&paths.home, &resource.id, &resource.url, &file_path)?;
            let changed = current_content_hash(&paths.db_path, &resource.id, &resource.current)?
                .is_none_or(|hash| hash != snapshot.content_hash);
            if changed || force {
                resource.current = snapshot.snapshot_id.clone();
                resource.local_path = Some(snapshot.path.clone());
                resource.updated_at = timestamp();
                if let (Some(manifest), Some(index)) = (&mut manifest, linked_index) {
                    manifest.resources[index] = resource.clone();
                    write_manifest(&paths.manifest_path, manifest)?;
                }
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
        ResourceKind::Arxiv => {
            let arxiv_id = resource
                .url
                .trim_start_matches("https://arxiv.org/abs/")
                .to_string();
            let snapshot = snapshot_arxiv(&paths.home, &resource.id, &arxiv_id, &resource.url)?;
            let changed = current_content_hash(&paths.db_path, &resource.id, &resource.current)?
                .is_none_or(|hash| hash != snapshot.content_hash);
            if changed || force {
                resource.current = snapshot.snapshot_id.clone();
                resource.local_path = Some(snapshot.path.clone());
                resource.updated_at = timestamp();
                if let (Some(manifest), Some(index)) = (&mut manifest, linked_index) {
                    manifest.resources[index] = resource.clone();
                    write_manifest(&paths.manifest_path, manifest)?;
                }
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
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
    app.ensure_project()?;
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
            ResourceKind::Docs | ResourceKind::Notes | ResourceKind::Arxiv => {
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
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
    app.ensure_global_storage()?;
    let manifest = read_optional_manifest(paths)?;
    let scope = if manifest.is_some() {
        "project"
    } else {
        "global"
    };
    let query_kind = kind.map(Into::into);
    let allowed = if let Some(manifest) = &manifest {
        allowed_resource_ids(manifest, label.as_deref(), query_kind)?
    } else {
        allowed_global_resource_ids(&paths.db_path, label.as_deref(), query_kind)?
    };
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
            "scope": scope,
            "debug": debug,
            "results": results,
        }),
    })
}

fn show(cwd: Option<PathBuf>, target: Option<String>, snapshots: bool) -> Result<()> {
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
    app.ensure_global_storage()?;
    let manifest = read_optional_manifest(paths)?;
    let scope = if manifest.is_some() {
        "project"
    } else {
        "global"
    };
    let resources = if let Some(target) = target {
        if let Some(manifest) = &manifest {
            find_manifest_resource(manifest, &target)
                .cloned()
                .or_else(|_| find_global_resource(&paths.db_path, &target))
                .map(|resource| vec![resource])?
        } else {
            vec![find_global_resource(&paths.db_path, &target)?]
        }
    } else if let Some(manifest) = &manifest {
        manifest.resources.clone()
    } else {
        list_global_resource_models(&paths.db_path, None)?
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
            "scope": scope,
            "resources": resources,
            "snapshots": snapshot_rows,
        }),
    })
}

fn list(cwd: Option<PathBuf>, project_only: bool, kind: Option<ResourceKind>) -> Result<()> {
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
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
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
    app.ensure_global_storage()?;
    let manifest = read_optional_manifest(paths)?;
    let resource = if let Some(manifest) = &manifest {
        find_manifest_resource(manifest, target)
            .cloned()
            .or_else(|_| find_global_resource(&paths.db_path, target))?
    } else {
        find_global_resource(&paths.db_path, target)?
    };
    if resource.kind != ResourceKind::Source {
        bail!("ctx path only supports source resources");
    }
    let Some(path) = resource.local_path else {
        bail!("source resource has no cached path");
    };
    println!("{path}");
    Ok(())
}

fn link(cwd: Option<PathBuf>, target: &str, reason: Option<String>) -> Result<()> {
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
    app.ensure_project()?;
    let mut manifest = read_manifest(&paths.manifest_path)?;
    let mut resource = find_global_resource(&paths.db_path, target)?;
    if reason.is_some() {
        resource.reason = reason;
    }
    upsert_manifest_resource(&mut manifest, resource.clone());
    write_manifest(&paths.manifest_path, &manifest)?;
    print_toon(CommandStatus {
        command: "link",
        status: "ok",
        result: json!({
            "resource": resource,
            "manifest_path": paths.manifest_path,
        }),
    })
}

fn unlink(cwd: Option<PathBuf>, target: &str) -> Result<()> {
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
    app.ensure_project()?;
    let mut manifest = read_manifest(&paths.manifest_path)?;
    let removed = find_manifest_resource(&manifest, target)?.clone();
    manifest.resources.retain(|resource| {
        resource.label != target && resource.url != target && resource.id != target
    });
    write_manifest(&paths.manifest_path, &manifest)?;
    print_toon(CommandStatus {
        command: "unlink",
        status: "ok",
        result: json!({
            "resource": removed,
            "manifest_path": paths.manifest_path,
        }),
    })
}

fn use_pointer(cwd: Option<PathBuf>, label: &str, pointer: &str) -> Result<()> {
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
    app.ensure_project()?;
    let mut manifest = read_manifest(&paths.manifest_path)?;
    let index = find_manifest_resource_index(&manifest, label)?;
    match manifest.resources[index].kind {
        ResourceKind::Source => validate_source_pointer(&manifest.resources[index], pointer)?,
        ResourceKind::Docs | ResourceKind::Notes | ResourceKind::Arxiv => {
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
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
    app.ensure_global_storage()?;
    let removed = find_global_resource(&paths.db_path, target)?;
    let pruned = remove_global_resource(&paths.db_path, &removed, prune_cache)?;
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
    let app = AppContext::load(cwd)?;
    let paths = &app.paths;
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

fn read_optional_manifest(paths: &crate::models::RuntimePaths) -> Result<Option<Manifest>> {
    if paths.manifest_path.exists() {
        Ok(Some(read_manifest(&paths.manifest_path)?))
    } else {
        Ok(None)
    }
}
