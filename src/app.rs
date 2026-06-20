use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{CommandFactory, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;

use crate::agents::upsert_agents_block;
use crate::arxiv::{ArxivRegistry, ResearchPaperRegistry as _};
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
use crate::memory::{
    RememberInput, ResolvedMemoryScope, export_memories, forget_memory, list_memories,
    recall as recall_memories, remember as remember_memory, show_memory,
};
use crate::models::{
    CommandStatus, Defaults, Manifest, MemoryKind, MemoryScope, MemoryStatus, QueryKind,
    ResearchPaperRegistry, ResolvedInput, Resource, ResourceKind, SnapshotMetadata, SnapshotPage,
};
use crate::output::print_toon;
use crate::retrieve::query_index;
use crate::snapshot::{snapshot_docs, snapshot_notes, write_snapshot_pages};
use crate::source::{cache_github_source, validate_source_pointer};
use crate::storage::{
    allowed_global_resource_ids, current_content_hash, ensure_db, find_global_resource,
    index_snapshot, list_global_resource_models, list_global_resources, remove_global_resource,
    snapshot_path_for_pointer, snapshots_for_resources, upsert_global_resource,
};
use crate::util::{content_hash, default_label_for_url, stable_id, timestamp};

#[derive(Parser)]
#[command(name = "ctx")]
#[command(about = "Project context for coding agents")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

const AGENT_HELP: &str = "Agent quick start:
  Read repo AGENTS.md and selected skill docs first; ctx is supporting evidence.
  ctx recall \"<task or error>\" --cwd <repo>
  ctx query \"<question>\" --cwd <repo>
  ctx remember \"<durable lesson>\" --kind fact --subject <topic> --scope project --cwd <repo>

Gotchas:
  Always pass --cwd for the repo you mean.
  Recall returns hints; verify drift-prone facts against live code.
  Query searches linked docs, notes, and research; use ctx path plus rg for source.
  Default docs crawl is up to 2048 pages, not a completeness guarantee.
  Each fetched docs page is capped at 5 MiB; use --max-pages when needed.
  Do not store secrets.";

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
    /// Query project docs, research papers, and notes.
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
    /// Store an explicit operational memory.
    Remember {
        content: String,
        #[arg(long)]
        kind: MemoryKind,
        #[arg(long)]
        subject: String,
        #[arg(long)]
        scope: Option<MemoryScope>,
        #[arg(long)]
        scope_key: Option<String>,
        #[arg(long)]
        trigger: Option<String>,
        #[arg(long)]
        suggested: bool,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Search operational memories.
    Recall {
        question: String,
        #[arg(long, default_value_t = DEFAULT_TOP_K)]
        top_k: usize,
        #[arg(long)]
        agent: bool,
        #[arg(long)]
        scope: Option<MemoryScope>,
        #[arg(long)]
        scope_key: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Inspect or manage stored memories.
    Memory {
        #[command(subcommand)]
        command: MemoryCommands,
    },
    /// Export personal memories and notes.
    Export {
        path: PathBuf,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Import personal memories and notes.
    Import {
        path: PathBuf,
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

#[derive(Subcommand)]
enum MemoryCommands {
    /// List memories visible from the current project.
    List {
        #[arg(long)]
        status: Option<MemoryStatus>,
        #[arg(long)]
        scope: Option<MemoryScope>,
        #[arg(long)]
        scope_key: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Show one memory with stored sections.
    Show {
        id: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// List suggested memories that need review.
    Review {
        #[arg(long)]
        scope: Option<MemoryScope>,
        #[arg(long)]
        scope_key: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Dismiss a memory without deleting its record.
    Forget {
        id: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
}

pub fn run() -> Result<()> {
    let mut args = env::args_os();
    let _program = args.next();
    match args.next().as_deref().and_then(|arg| arg.to_str()) {
        None | Some("--help" | "-h") => {
            print_root_help();
            return Ok(());
        }
        _ => {}
    }
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        print_root_help();
        return Ok(());
    };
    match command {
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
        Commands::Remember {
            content,
            kind,
            subject,
            scope,
            scope_key,
            trigger,
            suggested,
            tags,
            cwd,
        } => remember(RememberCommand {
            cwd,
            content,
            kind,
            subject,
            scope,
            scope_key,
            trigger,
            suggested,
            tags,
        }),
        Commands::Recall {
            question,
            top_k,
            agent,
            scope,
            scope_key,
            cwd,
        } => recall(cwd, &question, top_k, agent, scope, scope_key),
        Commands::Memory { command } => match command {
            MemoryCommands::List {
                status,
                scope,
                scope_key,
                cwd,
            } => memory_list(cwd, status, scope, scope_key),
            MemoryCommands::Show { id, cwd } => memory_show(cwd, &id),
            MemoryCommands::Review {
                scope,
                scope_key,
                cwd,
            } => memory_review(cwd, scope, scope_key),
            MemoryCommands::Forget { id, cwd } => memory_forget(cwd, &id),
        },
        Commands::Export { path, cwd } => export_personal(cwd, &path),
        Commands::Import { path, cwd } => import_personal(cwd, &path),
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

fn print_root_help() {
    let help = Cli::command().render_help().to_string();
    if let Some(index) = help.find("\n\nUsage:") {
        print!("{}\n\n{}{}", &help[..index], AGENT_HELP, &help[index..]);
    } else {
        print!("{help}\n\n{AGENT_HELP}\n");
    }
}

struct RememberCommand {
    cwd: Option<PathBuf>,
    content: String,
    kind: MemoryKind,
    subject: String,
    scope: Option<MemoryScope>,
    scope_key: Option<String>,
    trigger: Option<String>,
    suggested: bool,
    tags: Vec<String>,
}

const PERSONAL_EXPORT_KIND: &str = "ctx_personal_export";
const PERSONAL_EXPORT_VERSION: u32 = 1;
const PERSONAL_EXPORT_LEGEND: &str = "Personal ctx export. Contains memories and notes only; imported content may come from another computer, so paths in the marker may need adjustment.";

#[derive(Debug, Deserialize, Serialize)]
struct PersonalExport {
    version: u32,
    kind: String,
    legend: String,
    exported_at: String,
    memories: Vec<ExportedMemory>,
    notes: Vec<ExportedNote>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExportedMemory {
    scope: String,
    scope_key: Option<String>,
    kind: String,
    status: String,
    subject: String,
    trigger: Option<String>,
    content: String,
    tags: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExportedNote {
    label: String,
    url: String,
    snapshot_id: String,
    content_hash: String,
    content: String,
}

fn remember(command: RememberCommand) -> Result<()> {
    let app = AppContext::load(command.cwd)?;
    app.ensure_global_storage()?;
    let scope = resolve_memory_scope(
        &app.paths,
        command.scope.unwrap_or(MemoryScope::Project),
        command.scope_key,
    )?;
    let status = if command.suggested {
        MemoryStatus::Suggested
    } else {
        MemoryStatus::Active
    };
    let result = remember_memory(
        &app.paths.db_path,
        RememberInput {
            kind: command.kind,
            status,
            scope,
            subject: command.subject,
            trigger: command.trigger,
            content: command.content,
            tags: command.tags,
        },
    )?;
    print_toon(CommandStatus {
        command: "remember",
        status: "ok",
        result,
    })
}

fn recall(
    cwd: Option<PathBuf>,
    question: &str,
    top_k: usize,
    agent: bool,
    scope: Option<MemoryScope>,
    scope_key: Option<String>,
) -> Result<()> {
    let app = AppContext::load(cwd)?;
    app.ensure_global_storage()?;
    let scopes = recall_scopes(&app.paths, scope, scope_key)?;
    let memories = recall_memories(&app.paths.db_path, question, &scopes, top_k, agent)?;
    print_toon(CommandStatus {
        command: "recall",
        status: "ok",
        result: json!({
            "question": question,
            "mode": if agent { "agent" } else { "default" },
            "top_k": top_k,
            "scopes": scopes_json(&scopes),
            "memories": memories,
        }),
    })
}

fn memory_list(
    cwd: Option<PathBuf>,
    status: Option<MemoryStatus>,
    scope: Option<MemoryScope>,
    scope_key: Option<String>,
) -> Result<()> {
    let app = AppContext::load(cwd)?;
    app.ensure_global_storage()?;
    let scopes = recall_scopes(&app.paths, scope, scope_key)?;
    let memories = list_memories(&app.paths.db_path, &scopes, status)?;
    print_toon(CommandStatus {
        command: "memory list",
        status: "ok",
        result: json!({
            "scopes": scopes_json(&scopes),
            "status_filter": status.map(MemoryStatus::as_str),
            "memories": memories,
        }),
    })
}

fn memory_show(cwd: Option<PathBuf>, id: &str) -> Result<()> {
    let app = AppContext::load(cwd)?;
    app.ensure_global_storage()?;
    let scopes = recall_scopes(&app.paths, None, None)?;
    let result = show_memory(&app.paths.db_path, id, &scopes)?;
    print_toon(CommandStatus {
        command: "memory show",
        status: "ok",
        result,
    })
}

fn memory_review(
    cwd: Option<PathBuf>,
    scope: Option<MemoryScope>,
    scope_key: Option<String>,
) -> Result<()> {
    let app = AppContext::load(cwd)?;
    app.ensure_global_storage()?;
    let scopes = recall_scopes(&app.paths, scope, scope_key)?;
    let memories = list_memories(&app.paths.db_path, &scopes, Some(MemoryStatus::Suggested))?;
    print_toon(CommandStatus {
        command: "memory review",
        status: "ok",
        result: json!({
            "scopes": scopes_json(&scopes),
            "memories": memories,
        }),
    })
}

fn memory_forget(cwd: Option<PathBuf>, id: &str) -> Result<()> {
    let app = AppContext::load(cwd)?;
    app.ensure_global_storage()?;
    let scopes = recall_scopes(&app.paths, None, None)?;
    let result = forget_memory(&app.paths.db_path, id, &scopes)?;
    print_toon(CommandStatus {
        command: "memory forget",
        status: "ok",
        result,
    })
}

fn export_personal(cwd: Option<PathBuf>, path: &Path) -> Result<()> {
    let app = AppContext::load(cwd)?;
    app.ensure_global_storage()?;
    let exported_at = timestamp();
    let memories = export_memories(&app.paths.db_path)?
        .into_iter()
        .map(|value| serde_json::from_value(value).context("failed to export memory row"))
        .collect::<Result<Vec<ExportedMemory>>>()?;
    let notes = export_note_snapshots(&app.paths)?;
    let archive = PersonalExport {
        version: PERSONAL_EXPORT_VERSION,
        kind: PERSONAL_EXPORT_KIND.to_string(),
        legend: PERSONAL_EXPORT_LEGEND.to_string(),
        exported_at: exported_at.clone(),
        memories,
        notes,
    };
    let encoded = serde_json::to_string_pretty(&archive)?;
    fs::write(path, format!("{encoded}\n"))
        .with_context(|| format!("failed to write export {}", path.display()))?;
    print_toon(CommandStatus {
        command: "export",
        status: "ok",
        result: json!({
            "path": path,
            "kind": PERSONAL_EXPORT_KIND,
            "exported_at": exported_at,
            "memory_count": archive.memories.len(),
            "note_count": archive.notes.len(),
        }),
    })
}

fn import_personal(cwd: Option<PathBuf>, path: &Path) -> Result<()> {
    let app = AppContext::load(cwd)?;
    app.ensure_global_storage()?;
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read export {}", path.display()))?;
    let archive: PersonalExport = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse export {}", path.display()))?;
    if archive.kind != PERSONAL_EXPORT_KIND {
        bail!("unsupported export kind: {}", archive.kind);
    }
    if archive.version != PERSONAL_EXPORT_VERSION {
        bail!("unsupported export version: {}", archive.version);
    }

    let imported_at = timestamp();
    let mut imported_memories = 0usize;
    for memory in &archive.memories {
        import_memory(&app.paths, memory, &archive.exported_at, &imported_at)?;
        imported_memories += 1;
    }
    let mut imported_notes = 0usize;
    for note in &archive.notes {
        import_note(&app.paths, note, &archive.exported_at, &imported_at)?;
        imported_notes += 1;
    }

    print_toon(CommandStatus {
        command: "import",
        status: "ok",
        result: json!({
            "path": path,
            "kind": PERSONAL_EXPORT_KIND,
            "exported_at": archive.exported_at,
            "imported_at": imported_at,
            "memory_count": imported_memories,
            "note_count": imported_notes,
        }),
    })
}

fn export_note_snapshots(paths: &crate::models::RuntimePaths) -> Result<Vec<ExportedNote>> {
    list_global_resource_models(&paths.db_path, Some(ResourceKind::Notes))?
        .into_iter()
        .map(|resource| {
            let local_path = resource.local_path.as_deref().ok_or_else(|| {
                anyhow!("notes resource has no local snapshot: {}", resource.label)
            })?;
            let content = read_note_snapshot_content(Path::new(local_path))
                .with_context(|| format!("failed to export notes resource {}", resource.label))?;
            Ok(ExportedNote {
                label: resource.label,
                url: resource.url,
                snapshot_id: resource.current,
                content_hash: content_hash(&content),
                content,
            })
        })
        .collect()
}

fn read_note_snapshot_content(snapshot_path: &Path) -> Result<String> {
    let pages_path = snapshot_path.join("pages.json");
    if pages_path.exists() {
        let raw = fs::read_to_string(&pages_path)
            .with_context(|| format!("failed to read {}", pages_path.display()))?;
        let pages = serde_json::from_str::<Vec<SnapshotPage>>(&raw)
            .with_context(|| format!("failed to parse {}", pages_path.display()))?;
        if pages.is_empty() {
            bail!("notes snapshot has no pages: {}", snapshot_path.display());
        }
        return Ok(pages
            .into_iter()
            .map(|page| page.content)
            .collect::<Vec<_>>()
            .join("\n\n"));
    }
    let content_path = snapshot_path.join("content.txt");
    fs::read_to_string(&content_path)
        .with_context(|| format!("failed to read {}", content_path.display()))
}

fn import_memory(
    paths: &crate::models::RuntimePaths,
    memory: &ExportedMemory,
    exported_at: &str,
    imported_at: &str,
) -> Result<()> {
    let kind = parse_memory_kind(&memory.kind)?;
    let status = parse_memory_status(&memory.status)?;
    let scope = imported_memory_scope(paths, memory)?;
    let original_scope = original_memory_scope(memory);
    let content = imported_marked_content(
        exported_at,
        imported_at,
        &format!("original memory scope: {original_scope}"),
        &memory.content,
    );
    remember_memory(
        &paths.db_path,
        RememberInput {
            kind,
            status,
            scope,
            subject: memory.subject.clone(),
            trigger: memory.trigger.clone(),
            content,
            tags: memory.tags.clone(),
        },
    )?;
    Ok(())
}

fn import_note(
    paths: &crate::models::RuntimePaths,
    note: &ExportedNote,
    exported_at: &str,
    imported_at: &str,
) -> Result<()> {
    let original_hash = content_hash(&note.content);
    let id = stable_id(&format!(
        "imported-notes:{}:{}:{}:{}",
        note.label, note.url, exported_at, original_hash
    ));
    let import_url = format!("ctx-import://notes/{id}");
    let content = imported_marked_content(
        exported_at,
        imported_at,
        &format!("original notes source: {}", note.url),
        &note.content,
    );
    let snapshot = write_snapshot_pages(
        &paths.home,
        ResourceKind::Notes,
        &id,
        &import_url,
        vec![SnapshotPage {
            url: import_url.clone(),
            content,
        }],
    )?;
    let now = timestamp();
    let resource = Resource {
        id,
        label: note.label.clone(),
        kind: ResourceKind::Notes,
        url: import_url,
        reason: None,
        current: snapshot.snapshot_id.clone(),
        local_path: Some(snapshot.path.clone()),
        created_at: now.clone(),
        updated_at: now,
    };
    upsert_global_resource(&paths.db_path, &resource, Some(&snapshot))?;
    index_snapshot(&paths.db_path, &resource, &snapshot)?;
    Ok(())
}

fn imported_memory_scope(
    paths: &crate::models::RuntimePaths,
    memory: &ExportedMemory,
) -> Result<ResolvedMemoryScope> {
    match parse_memory_scope(&memory.scope)? {
        MemoryScope::Global => resolve_memory_scope(paths, MemoryScope::Global, None),
        MemoryScope::Project | MemoryScope::Thread => {
            resolve_memory_scope(paths, MemoryScope::Project, None)
        }
    }
}

fn imported_marked_content(
    exported_at: &str,
    imported_at: &str,
    original: &str,
    content: &str,
) -> String {
    format!(
        "> Imported from ctx personal export.\n> Exported at: {exported_at}\n> Imported at: {imported_at}\n> {original}\n\n{content}"
    )
}

fn original_memory_scope(memory: &ExportedMemory) -> String {
    match memory.scope_key.as_deref() {
        Some(scope_key) => format!("{} ({scope_key})", memory.scope),
        None => memory.scope.clone(),
    }
}

fn parse_memory_kind(value: &str) -> Result<MemoryKind> {
    match value {
        "preference" => Ok(MemoryKind::Preference),
        "fact" => Ok(MemoryKind::Fact),
        "decision" => Ok(MemoryKind::Decision),
        "recipe" => Ok(MemoryKind::Recipe),
        "warning" => Ok(MemoryKind::Warning),
        _ => bail!("unsupported memory kind: {value}"),
    }
}

fn parse_memory_status(value: &str) -> Result<MemoryStatus> {
    match value {
        "suggested" => Ok(MemoryStatus::Suggested),
        "active" => Ok(MemoryStatus::Active),
        "dismissed" => Ok(MemoryStatus::Dismissed),
        "superseded" => Ok(MemoryStatus::Superseded),
        _ => bail!("unsupported memory status: {value}"),
    }
}

fn parse_memory_scope(value: &str) -> Result<MemoryScope> {
    match value {
        "global" => Ok(MemoryScope::Global),
        "project" => Ok(MemoryScope::Project),
        "thread" => Ok(MemoryScope::Thread),
        _ => bail!("unsupported memory scope: {value}"),
    }
}

fn resolve_memory_scope(
    paths: &crate::models::RuntimePaths,
    scope: MemoryScope,
    scope_key: Option<String>,
) -> Result<ResolvedMemoryScope> {
    match scope {
        MemoryScope::Global => {
            if scope_key.is_some() {
                bail!("global memories do not accept --scope-key");
            }
            Ok(ResolvedMemoryScope {
                scope,
                scope_key: None,
            })
        }
        MemoryScope::Project => Ok(ResolvedMemoryScope {
            scope,
            scope_key: Some(scope_key.unwrap_or_else(|| paths.project_root.display().to_string())),
        }),
        MemoryScope::Thread => {
            let Some(scope_key) = scope_key else {
                bail!("thread memories require --scope-key");
            };
            Ok(ResolvedMemoryScope {
                scope,
                scope_key: Some(scope_key),
            })
        }
    }
}

fn recall_scopes(
    paths: &crate::models::RuntimePaths,
    scope: Option<MemoryScope>,
    scope_key: Option<String>,
) -> Result<Vec<ResolvedMemoryScope>> {
    if let Some(scope) = scope {
        return resolve_memory_scope(paths, scope, scope_key).map(|scope| vec![scope]);
    }
    Ok(vec![
        ResolvedMemoryScope {
            scope: MemoryScope::Global,
            scope_key: None,
        },
        ResolvedMemoryScope {
            scope: MemoryScope::Project,
            scope_key: Some(paths.project_root.display().to_string()),
        },
    ])
}

fn scopes_json(scopes: &[ResolvedMemoryScope]) -> Vec<serde_json::Value> {
    scopes
        .iter()
        .map(|scope| {
            json!({
                "scope": scope.scope.as_str(),
                "scope_key": scope.scope_key,
            })
        })
        .collect()
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
        ResolvedInput::ResearchPaper {
            registry,
            id: arxiv_id,
            url,
        } => {
            let label = label
                .unwrap_or_else(|| research_paper_label(registry, &arxiv_id).replace('/', "-"));
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
        trusted_manifest_resource(&paths.db_path, &manifest.resources[index])?
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

    for manifest_resource in &manifest.resources {
        let resource = trusted_manifest_resource(&paths.db_path, manifest_resource).ok();
        let ready = match resource.as_ref() {
            Some(resource) if resource.kind == ResourceKind::Source => resource
                .local_path
                .as_deref()
                .map(Path::new)
                .is_some_and(Path::exists),
            Some(resource)
                if matches!(
                    resource.kind,
                    ResourceKind::Docs | ResourceKind::Notes | ResourceKind::ResearchPaper
                ) =>
            {
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
                        extra: None,
                    };
                    index_snapshot(&paths.db_path, resource, &snapshot)?;
                }
                path_ready
            }
            _ => false,
        };
        checked.push(json!({
            "label": manifest_resource.label,
            "kind": manifest_resource.kind,
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
        match find_manifest_resource(manifest, target) {
            Ok(resource) => trusted_manifest_resource(&paths.db_path, resource)?,
            Err(_) => find_global_resource(&paths.db_path, target)?,
        }
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

fn trusted_manifest_resource(db_path: &Path, resource: &Resource) -> Result<Resource> {
    let mut trusted = find_global_resource(db_path, &resource.id).with_context(|| {
        format!(
            "manifest resource '{}' is not in the global cache; run ctx add then ctx link",
            resource.label
        )
    })?;
    if resource.reason.is_some() {
        trusted.reason = resource.reason.clone();
    }
    Ok(trusted)
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
        ResourceKind::Docs | ResourceKind::Notes | ResourceKind::ResearchPaper => {
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
