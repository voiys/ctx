use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::{ArgAction, CommandFactory, Parser, Subcommand};
use serde_json::json;

use crate::arxiv::{ArxivRegistry, ResearchPaperRegistry as _};
use crate::constants::{
    DEFAULT_BUDGET_TOKENS, DEFAULT_CRAWL_CONCURRENCY, DEFAULT_MAX_PAGES, DEFAULT_TOP_K,
};
use crate::context::AppContext;
use crate::input::resolve_input;
use crate::install::{default_install_status, install};
use crate::models::{
    CommandStatus, QueryKind, ResearchPaperRegistry, ResolvedInput, Resource, ResourceKind,
};
use crate::output::print_toon;
use crate::retrieve::query_index;
use crate::snapshot::snapshot_docs;
use crate::source::{cache_github_source, cache_github_source_pointer};
use crate::storage::{
    allowed_global_resource_ids, current_content_hash, find_global_resource, index_snapshot,
    list_global_resource_models, list_global_resources, remove_global_resource,
    snapshots_for_resources, upsert_global_resource,
};
use crate::util::{default_label_for_url, stable_id, timestamp};

#[derive(Parser)]
#[command(name = "ctx", version)]
#[command(
    about = "Pin upstream source and read it with rg before falling back to searchable docs and papers."
)]
#[command(
    before_help = "Offline reference workflow:\n  1. ctx add <url> --label <name>\n  2. ctx path <name>, then rg source/tests/examples\n  3. ctx query <question> --label <docs-name> as fallback\nRun `ctx guides` for workflows, `ctx <command> --help` for examples."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Add a source, docs, or paper URL to the global cache.
    #[command(
        after_help = "Docs crawl defaults: up to 2048 pages, capped at 5 MiB per page.\n\nExamples:\n  ctx add https://github.com/clap-rs/clap/tree/v4.6.1 --label clap-4.6.1-src\n  ctx add https://docs.rs/clap/4.6.1/clap/ --label clap-docs --reason 'locked API docs'"
    )]
    Add {
        url: String,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, default_value_t = DEFAULT_MAX_PAGES)]
        max_pages: usize,
        #[arg(long, default_value_t = DEFAULT_CRAWL_CONCURRENCY)]
        concurrency: usize,
    },
    /// List cached resources and optional snapshot history.
    #[command(
        after_help = "Examples:\n  ctx list\n  ctx list clap-docs --snapshots\n  ctx list --kind source"
    )]
    List {
        target: Option<String>,
        #[arg(long)]
        kind: Option<ResourceKind>,
        #[arg(long)]
        snapshots: bool,
    },
    /// Print a cached source checkout path.
    #[command(after_help = "Examples:\n  ctx path clap-4.6.1-src")]
    Path { target: String },
    /// Search cached docs and research papers.
    #[command(
        after_help = "Unscoped query is discovery, not proof. Prefer a known label and a specific question.\nWhen source is unavailable or insufficient, add authoritative docs or a paper and query that label.\nEvidence is matched chunks plus small local context, not a whole-doc summary.\n\nExamples:\n  ctx query 'repeatable arguments' --label clap-docs\n  ctx query 'retrieval fusion' --label paper-a --label paper-b\n  ctx query 'old API' --label clap-docs --snapshot 20260101T000000Z-012345abcdef"
    )]
    Query {
        question: String,
        #[arg(long, default_value_t = DEFAULT_TOP_K)]
        top_k: usize,
        #[arg(long, default_value_t = DEFAULT_BUDGET_TOKENS)]
        budget: usize,
        #[arg(long)]
        debug: bool,
        #[arg(long, action = ArgAction::Append)]
        label: Vec<String>,
        #[arg(long)]
        kind: Option<QueryKind>,
        #[arg(long)]
        snapshot: Option<String>,
    },
    /// Refresh docs/papers or report a pinned source.
    #[command(after_help = "Examples:\n  ctx update clap-docs\n  ctx update clap-docs --force")]
    Update {
        target: String,
        #[arg(long)]
        force: bool,
        #[arg(long, default_value_t = DEFAULT_MAX_PAGES)]
        max_pages: usize,
        #[arg(long, default_value_t = DEFAULT_CRAWL_CONCURRENCY)]
        concurrency: usize,
    },
    /// Remove a resource, optionally pruning its cached files.
    #[command(
        after_help = "Examples:\n  ctx remove clap-docs\n  ctx remove clap-4.6.1-src --prune-cache"
    )]
    Remove {
        target: String,
        #[arg(long)]
        prune_cache: bool,
    },
    /// Check global cache and index health.
    #[command(after_help = "Examples:\n  ctx doctor")]
    Doctor,
    /// Copy the binary and installation metadata.
    #[command(after_help = "Examples:\n  ctx install --force\n  ctx install --bin-dir ./bin")]
    Install {
        #[arg(long)]
        bin_dir: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    /// Read embedded source-first workflow guides.
    #[command(after_help = "Examples:\n  ctx guides\n  ctx guides pin")]
    Guides { name: Option<String> },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => {
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
        Some(Commands::Add {
            url,
            label,
            reason,
            max_pages,
            concurrency,
        }) => add(&url, label, reason, max_pages, concurrency),
        Some(Commands::List {
            target,
            kind,
            snapshots,
        }) => list(target, kind, snapshots),
        Some(Commands::Path { target }) => print_source_path(&target),
        Some(Commands::Query {
            question,
            top_k,
            budget,
            debug,
            label,
            kind,
            snapshot,
        }) => query(QueryCommand {
            question,
            top_k,
            budget_tokens: budget,
            debug,
            label,
            kind,
            snapshot,
        }),
        Some(Commands::Update {
            target,
            force,
            max_pages,
            concurrency,
        }) => update(&target, force, max_pages, concurrency),
        Some(Commands::Remove {
            target,
            prune_cache,
        }) => remove(&target, prune_cache),
        Some(Commands::Doctor) => doctor(),
        Some(Commands::Install { bin_dir, force }) => install(bin_dir, force),
        Some(Commands::Guides { name }) => guides(name.as_deref()),
    }
}

struct QueryCommand {
    question: String,
    top_k: usize,
    budget_tokens: usize,
    debug: bool,
    label: Vec<String>,
    kind: Option<QueryKind>,
    snapshot: Option<String>,
}

fn add(
    input_url: &str,
    label: Option<String>,
    mut reason: Option<String>,
    max_pages: usize,
    concurrency: usize,
) -> Result<()> {
    let app = AppContext::load()?;
    let paths = &app.paths;
    app.ensure_global_storage()?;
    let resolved = resolve_input(input_url)?;
    let (stored_url, resolved_label) = match &resolved {
        ResolvedInput::GithubSource { repo, .. } => {
            (input_url.to_string(), label.unwrap_or_else(|| repo.clone()))
        }
        ResolvedInput::Docs { url } => (
            url.clone(),
            label.unwrap_or_else(|| default_label_for_url(url)),
        ),
        ResolvedInput::ResearchPaper { registry, id, url } => (
            url.clone(),
            label.unwrap_or_else(|| research_paper_label(*registry, id).replace('/', "-")),
        ),
    };
    if let Some(mut existing) = list_global_resource_models(&paths.db_path, None)?
        .into_iter()
        .find(|resource| resource.label == resolved_label)
    {
        if existing.url != stored_url {
            bail!(
                "label '{}' is already used by {}; remove it or choose another label",
                resolved_label,
                existing.url
            );
        }
        if existing
            .local_path
            .as_deref()
            .map(Path::new)
            .is_some_and(Path::exists)
        {
            if let Some(reason) = reason {
                existing.reason = Some(reason);
                upsert_global_resource(&paths.db_path, &existing, None)?;
            }
            return print_toon(CommandStatus {
                command: "add",
                status: "ok",
                result: json!({"resource": existing, "existing": true}),
            });
        }
        if reason.is_none() {
            reason = existing.reason;
        }
    }
    let label = resolved_label;
    let now = timestamp();

    let (resource, extra) = match resolved {
        ResolvedInput::GithubSource {
            owner,
            repo,
            requested_ref,
            clone_url,
        } => {
            let id = stable_id(&format!("source:{clone_url}:{label}"));
            let (commit, path) = if let Some(reference) = requested_ref.as_deref()
                && reference.len() == 40
                && reference
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                cache_github_source_pointer(&paths.home, &owner, &repo, reference, &clone_url)?
            } else {
                cache_github_source(
                    &paths.home,
                    &owner,
                    &repo,
                    requested_ref.as_deref(),
                    &clone_url,
                )?
            };
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
            index_snapshot(&paths.db_path, &resource, &snapshot)?;
            (resource, json!({"snapshot": snapshot}))
        }
        ResolvedInput::ResearchPaper {
            registry,
            id: arxiv_id,
            url,
        } => {
            let id = stable_id(&format!(
                "research-paper:{}:{url}:{label}",
                research_paper_registry_name(registry)
            ));
            let snapshot = match registry {
                ResearchPaperRegistry::Arxiv => {
                    ArxivRegistry.snapshot(&paths.home, &id, &arxiv_id, &url)?
                }
            };
            let resource = Resource {
                id,
                label,
                kind: ResourceKind::ResearchPaper,
                url,
                reason,
                current: snapshot.snapshot_id.clone(),
                local_path: Some(snapshot.path.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            upsert_global_resource(&paths.db_path, &resource, Some(&snapshot))?;
            index_snapshot(&paths.db_path, &resource, &snapshot)?;
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

fn update(target: &str, force: bool, max_pages: usize, concurrency: usize) -> Result<()> {
    let app = AppContext::load()?;
    let paths = &app.paths;
    app.ensure_global_storage()?;
    let mut resource = find_global_resource(&paths.db_path, target)?;

    match resource.kind {
        ResourceKind::Source => {
            print_toon(CommandStatus {
                command: "update",
                status: "ok",
                result: json!({
                    "message": "source resources are pinned; add an explicit ref under a new label to change them",
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
        ResourceKind::ResearchPaper => {
            let arxiv_id = resource
                .url
                .trim_start_matches("https://arxiv.org/abs/")
                .to_string();
            let snapshot =
                ArxivRegistry.snapshot(&paths.home, &resource.id, &arxiv_id, &resource.url)?;
            let changed = current_content_hash(&paths.db_path, &resource.id, &resource.current)?
                .is_none_or(|hash| hash != snapshot.content_hash);
            if changed || force {
                resource.current = snapshot.snapshot_id.clone();
                resource.local_path = Some(snapshot.path.clone());
                resource.updated_at = timestamp();
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

fn query(command: QueryCommand) -> Result<()> {
    let app = AppContext::load()?;
    app.ensure_global_storage()?;
    let allowed = allowed_global_resource_ids(
        &app.paths.db_path,
        &command.label,
        command.kind.map(Into::into),
        command.snapshot.as_deref(),
    )?;
    let results = query_index(
        &app.paths.db_path,
        &command.question,
        &allowed,
        command.top_k,
        command.budget_tokens,
        command.debug,
    )?;
    print_toon(CommandStatus {
        command: "query",
        status: "ok",
        result: json!({
            "question": command.question, "top_k": command.top_k, "budget_tokens": command.budget_tokens,
            "debug": command.debug, "mode": "agent", "results": results,
        }),
    })
}

fn list(target: Option<String>, kind: Option<ResourceKind>, snapshots: bool) -> Result<()> {
    let app = AppContext::load()?;
    app.ensure_global_storage()?;
    let resources = if let Some(target) = target {
        let resource = find_global_resource(&app.paths.db_path, &target)?;
        if kind.is_none_or(|kind| resource.kind == kind) {
            vec![resource]
        } else {
            Vec::new()
        }
    } else {
        list_global_resource_models(&app.paths.db_path, kind)?
    };
    let ids = resources
        .iter()
        .map(|resource| resource.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let rows = list_global_resources(&app.paths.db_path, kind)?
        .into_iter()
        .filter(|row| row["id"].as_str().is_some_and(|id| ids.contains(id)))
        .collect::<Vec<_>>();
    let snapshot_rows = if snapshots {
        snapshots_for_resources(&app.paths.db_path, &resources)?
    } else {
        Vec::new()
    };
    print_toon(CommandStatus {
        command: "list",
        status: "ok",
        result: json!({
            "home": app.paths.home, "resources": rows, "snapshots": snapshot_rows,
        }),
    })
}

fn print_source_path(target: &str) -> Result<()> {
    let app = AppContext::load()?;
    app.ensure_global_storage()?;
    let resource = find_global_resource(&app.paths.db_path, target)?;
    if resource.kind != ResourceKind::Source {
        bail!("ctx path only supports source resources");
    }
    let Some(path) = resource.local_path else {
        bail!("source resource has no cached path");
    };
    println!("{path}");
    Ok(())
}

fn doctor() -> Result<()> {
    let app = AppContext::load()?;
    app.ensure_global_storage()?;
    print_toon(CommandStatus {
        command: "doctor",
        status: "ok",
        result: json!({
            "home": app.paths.home, "db_path": app.paths.db_path, "db_ok": true,
            "install": default_install_status()?,
        }),
    })
}

fn remove(target: &str, prune_cache: bool) -> Result<()> {
    let app = AppContext::load()?;
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

fn research_paper_label(registry: ResearchPaperRegistry, id: &str) -> String {
    match registry {
        ResearchPaperRegistry::Arxiv => format!("arxiv-{id}"),
    }
}

fn research_paper_registry_name(registry: ResearchPaperRegistry) -> &'static str {
    match registry {
        ResearchPaperRegistry::Arxiv => "arxiv",
    }
}

fn guides(name: Option<&str>) -> Result<()> {
    let guides = [
        (
            "pin",
            "Pin the exact installed version and retain restore commands.",
            include_str!("../guides/pin.md"),
        ),
        (
            "idiomatic-usage",
            "Read upstream examples and tests for idiomatic API usage.",
            include_str!("../guides/idiomatic-usage.md"),
        ),
        (
            "copy-pattern",
            "Mirror an upstream composition and its error handling.",
            include_str!("../guides/copy-pattern.md"),
        ),
        (
            "tests-as-spec",
            "Read pinned test assertions as the API contract.",
            include_str!("../guides/tests-as-spec.md"),
        ),
        (
            "trace-api",
            "Trace definitions, re-exports, defaults and callers.",
            include_str!("../guides/trace-api.md"),
        ),
        (
            "error-to-source",
            "Trace exact error text to its guard and real caller.",
            include_str!("../guides/error-to-source.md"),
        ),
        (
            "upgrade-impact",
            "Compare pinned subpaths and map every affected use.",
            include_str!("../guides/upgrade-impact.md"),
        ),
        (
            "why-history",
            "Optionally deepen a shallow pin to explain history.",
            include_str!("../guides/why-history.md"),
        ),
    ];
    if let Some(name) = name {
        let Some((_, _, content)) = guides.iter().find(|(guide, _, _)| *guide == name) else {
            bail!("unknown guide '{name}'; run `ctx guides` to list guides");
        };
        print!("{content}");
    } else {
        println!(
            "Live code wins over cached references.\nResolve exact installed versions; do not guess.\nCite file:line @ SHA."
        );
        for (name, summary, _) in guides {
            println!("{name}  {summary}");
        }
    }
    Ok(())
}
