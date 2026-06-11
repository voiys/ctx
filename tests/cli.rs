use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;
use tiny_http::{Response, Server};

struct TestProject {
    root: TempDir,
    home: TempDir,
}

impl TestProject {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn ctx(&self) -> Command {
        let mut command = Command::cargo_bin("ctx").unwrap();
        command.env("CTX_HOME", self.home.path());
        command.env("CTX_EMBEDDINGS", "off");
        command
    }

    fn ctx_with_embeddings(&self) -> Command {
        let mut command = Command::cargo_bin("ctx").unwrap();
        command.env("CTX_HOME", self.home.path());
        command
    }

    fn manifest(&self) -> Value {
        let raw = fs::read_to_string(self.root.path().join(".ctx/ctx.json")).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    fn resource_current(&self, label: &str) -> String {
        self.manifest()["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|resource| resource["label"] == label)
            .unwrap()["current"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn resource_path(&self, label: &str) -> String {
        self.manifest()["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|resource| resource["label"] == label)
            .unwrap()["local_path"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

fn ctx_with_home(home: &Path) -> Command {
    let mut command = Command::cargo_bin("ctx").unwrap();
    command.env("CTX_HOME", home);
    command.env("CTX_EMBEDDINGS", "off");
    command
}

fn first_toon_id(stdout: &[u8]) -> String {
    let stdout = String::from_utf8(stdout.to_vec()).unwrap();
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("id: "))
        .map(|value| value.trim_matches('"').to_string())
        .unwrap_or_else(|| panic!("no id line in output:\n{stdout}"))
}

#[test]
fn init_writes_agents_block_with_memory_guidance() {
    let project = TestProject::new();

    project
        .ctx()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let agents_path = project.root.path().join("AGENTS.md");
    let agents = fs::read_to_string(&agents_path).unwrap();
    assert!(agents.contains("Use `ctx` for this project's local context and operational memory."));
    assert!(agents.contains("ctx recall \"<task, repo, or failure pattern>\" --cwd <repo>"));
    assert!(agents.contains("ctx remember \"<concise reusable lesson>\""));
    assert!(agents.contains("--suggested"));
    assert!(agents.contains("ctx query \"<question>\" --cwd <repo>"));

    project
        .ctx()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let agents = fs::read_to_string(agents_path).unwrap();
    assert_eq!(agents.matches("<!-- ctx:start -->").count(), 1);
    assert_eq!(agents.matches("<!-- ctx:end -->").count(), 1);
}

struct FixtureSite {
    base_url: String,
    pages: Arc<Mutex<HashMap<String, String>>>,
}

impl FixtureSite {
    fn new(pages: HashMap<String, String>) -> Self {
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        let pages = Arc::new(Mutex::new(pages));
        let thread_pages = Arc::clone(&pages);
        thread::spawn(move || {
            while let Ok(request) = server.recv() {
                let path = request
                    .url()
                    .split('?')
                    .next()
                    .unwrap_or("/")
                    .trim_end_matches('/')
                    .to_string();
                let key = if path.is_empty() { "/" } else { path.as_str() };
                let body = thread_pages
                    .lock()
                    .unwrap()
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| "not found".to_string());
                let status = if body == "not found" { 404 } else { 200 };
                let response = Response::from_string(body).with_status_code(status);
                let _ = request.respond(response);
            }
        });

        Self {
            base_url: format!("http://{addr}"),
            pages,
        }
    }

    fn set(&self, path: &str, body: &str) {
        self.pages
            .lock()
            .unwrap()
            .insert(path.to_string(), body.to_string());
    }
}

#[test]
fn global_notes_can_be_used_without_project_manifest() {
    let project = TestProject::new();
    let note_path = project.root.path().join("global-notes.md");
    fs::write(
        &note_path,
        "Global docs shelf contains CTX_GLOBAL_ONLY_TOKEN for retrieval.",
    )
    .unwrap();

    project
        .ctx()
        .arg("add")
        .arg(format!("file://{}", note_path.display()))
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "global-notes"])
        .assert()
        .success();
    assert!(!project.root.path().join(".ctx/ctx.json").exists());

    let query = project
        .ctx()
        .args(["query", "CTX_GLOBAL_ONLY_TOKEN", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let query = String::from_utf8(query).unwrap();
    assert!(query.contains("scope: global"));
    assert!(query.contains("CTX_GLOBAL_ONLY_TOKEN"));

    let show = project
        .ctx()
        .args(["show", "global-notes", "--snapshots", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let show = String::from_utf8(show).unwrap();
    assert!(show.contains("global-notes"));
    assert!(show.contains("snapshot_id"));

    project
        .ctx()
        .args(["remove", "global-notes", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let list = project
        .ctx()
        .args(["list", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list = String::from_utf8(list).unwrap();
    assert!(!list.contains("global-notes"));
}

#[test]
fn project_link_adds_existing_global_resource_to_manifest() {
    let project = TestProject::new();
    let note_path = project.root.path().join("project-notes.md");
    fs::write(
        &note_path,
        "Project linked docs mention CTX_PROJECT_LINKED_TOKEN.",
    )
    .unwrap();

    project
        .ctx()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    project
        .ctx()
        .arg("add")
        .arg(format!("file://{}", note_path.display()))
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "project-notes"])
        .assert()
        .success();
    assert_eq!(project.manifest()["resources"].as_array().unwrap().len(), 0);

    let link = project
        .ctx()
        .args([
            "link",
            "project-notes",
            "--reason",
            "needed for tests",
            "--cwd",
        ])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let link = String::from_utf8(link).unwrap();
    assert!(link.contains("command: link"));

    let manifest = project.manifest();
    assert_eq!(manifest["resources"][0]["label"], "project-notes");
    assert_eq!(manifest["resources"][0]["reason"], "needed for tests");

    let query = project
        .ctx()
        .args(["query", "CTX_PROJECT_LINKED_TOKEN", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let query = String::from_utf8(query).unwrap();
    assert!(query.contains("scope: project"));
    assert!(query.contains("CTX_PROJECT_LINKED_TOKEN"));

    project
        .ctx()
        .args(["unlink", "project-notes", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    assert_eq!(project.manifest()["resources"].as_array().unwrap().len(), 0);

    let global_query = project
        .ctx()
        .args(["query", "project linked token", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let global_query = String::from_utf8(global_query).unwrap();
    assert!(!global_query.contains("Project linked docs"));
}

#[test]
fn notes_can_be_added_queried_and_debugged() {
    let project = TestProject::new();
    let note_path = project.root.path().join("notes.md");
    fs::write(
        &note_path,
        "# Retry Notes\n\n## Timeout Recovery\n\nRetries use exponential backoff and jitter. ERR_RETRY_TIMEOUT means the retry budget was exhausted.",
    )
    .unwrap();

    project
        .ctx()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    project
        .ctx()
        .arg("add")
        .arg(format!("file://{}", note_path.display()))
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "retry-notes"])
        .assert()
        .success();
    project
        .ctx()
        .args(["link", "retry-notes", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "retry timeout backoff", "--debug", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("retry-notes"));
    assert!(stdout.contains("ERR_RETRY_TIMEOUT"));
    assert!(stdout.contains("debug:"));
    assert!(stdout.contains("heading_path[2]:"));
    assert!(stdout.contains("Timeout Recovery"));
    assert!(stdout.contains("section_index:"));
}

#[test]
fn memories_can_be_remembered_recalled_grouped_and_forgotten() {
    let project = TestProject::new();
    let content = r#"# Migration Recipe

## Trigger

ERROR_ALPHA appears during parser migration.

## Step One

Run the focused parser tests.

## Step Two

Update the fixture schema.

## Step Three

Run the smoke validation.
"#;

    let remember = project
        .ctx()
        .arg("remember")
        .arg(content)
        .args([
            "--kind",
            "recipe",
            "--subject",
            "parser.migration",
            "--scope",
            "project",
            "--tag",
            "parser",
            "--cwd",
        ])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let memory_id = first_toon_id(&remember);

    let recall = project
        .ctx()
        .args(["recall", "ERROR_ALPHA smoke", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recall = String::from_utf8(recall).unwrap();
    assert!(recall.contains("parser.migration"));
    assert!(recall.contains("ERROR_ALPHA"));

    let agent = project
        .ctx()
        .args(["recall", "ERROR_ALPHA smoke", "--agent", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let agent = String::from_utf8(agent).unwrap();
    assert!(agent.contains("mode: agent"));
    assert!(agent.contains("context-between"));
    assert!(agent.contains("Step One"));
    assert!(agent.contains("Step Two"));
    assert!(agent.contains("Step Three"));

    project
        .ctx()
        .args(["memory", "show", &memory_id, "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    project
        .ctx()
        .args(["memory", "forget", &memory_id, "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let recall = project
        .ctx()
        .args(["recall", "ERROR_ALPHA smoke", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recall = String::from_utf8(recall).unwrap();
    assert!(!recall.contains("parser.migration"));
}

#[test]
fn project_scoped_recall_excludes_other_projects() {
    let home = tempfile::tempdir().unwrap();
    let project_a = tempfile::tempdir().unwrap();
    let project_b = tempfile::tempdir().unwrap();

    ctx_with_home(home.path())
        .arg("remember")
        .arg("Project A memory carries CTX_PROJECT_A_ONLY.")
        .args([
            "--kind",
            "fact",
            "--subject",
            "project.scope",
            "--scope",
            "project",
            "--cwd",
        ])
        .arg(project_a.path())
        .assert()
        .success();

    let from_a = ctx_with_home(home.path())
        .args(["recall", "CTX_PROJECT_A_ONLY", "--cwd"])
        .arg(project_a.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let from_a = String::from_utf8(from_a).unwrap();
    assert!(from_a.contains("CTX_PROJECT_A_ONLY"));

    let from_b = ctx_with_home(home.path())
        .args(["recall", "CTX_PROJECT_A_ONLY", "--cwd"])
        .arg(project_b.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let from_b = String::from_utf8(from_b).unwrap();
    assert!(!from_b.contains("Project A memory carries"));
}

#[test]
fn suggested_memories_are_reviewable_but_not_recalled_by_default() {
    let project = TestProject::new();
    let remember = project
        .ctx()
        .arg("remember")
        .arg("Suggested memory mentions CTX_SUGGESTED_ONLY.")
        .args([
            "--kind",
            "warning",
            "--subject",
            "review.queue",
            "--suggested",
            "--cwd",
        ])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let memory_id = first_toon_id(&remember);

    let review = project
        .ctx()
        .args(["memory", "review", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let review = String::from_utf8(review).unwrap();
    assert!(review.contains("CTX_SUGGESTED_ONLY"));
    assert!(review.contains("suggested"));

    let show = project
        .ctx()
        .args(["memory", "show", &memory_id, "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let show = String::from_utf8(show).unwrap();
    assert!(show.contains("sections"));

    let recall = project
        .ctx()
        .args(["recall", "CTX_SUGGESTED_ONLY", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recall = String::from_utf8(recall).unwrap();
    assert!(!recall.contains("Suggested memory mentions"));
}

#[test]
fn docs_crawl_recursively_indexes_linked_pages() {
    let site = FixtureSite::new(HashMap::from([
        (
            "/".to_string(),
            r#"<html><body><main>
            <a href="/guide">Guide</a>
            <a href="/api">API</a>
            Home page talks about retries.
            </main></body></html>"#
                .to_string(),
        ),
        (
            "/guide".to_string(),
            "<html><body><main>Guide page explains exponential backoff.</main></body></html>"
                .to_string(),
        ),
        (
            "/api".to_string(),
            "<html><body><main>API reference documents CTX_SPECIAL_RETRY_FLAG.</main></body></html>"
                .to_string(),
        ),
    ]));
    let project = TestProject::new();

    project
        .ctx()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    project
        .ctx()
        .arg("add")
        .arg(format!("{}/", site.base_url))
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "fixture-docs"])
        .assert()
        .success();
    project
        .ctx()
        .args(["link", "fixture-docs", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "CTX_SPECIAL_RETRY_FLAG", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("fixture-docs"));
    assert!(stdout.contains("CTX_SPECIAL_RETRY_FLAG"));

    let show = project
        .ctx()
        .args(["show", "fixture-docs", "--snapshots", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let show = String::from_utf8(show).unwrap();
    assert!(show.contains("page_count"));
    assert!(show.contains(",3,") || show.contains("page_count: 3"));
}

#[test]
fn docs_crawl_falls_back_when_llms_txt_is_absent() {
    let site = FixtureSite::new(HashMap::from([
        (
            "/docs".to_string(),
            r#"<html><body><main>
            <a href="/docs/guide">Guide</a>
            Docs home mentions CTX_NO_LLMS_HOME_TOKEN.
            </main></body></html>"#
                .to_string(),
        ),
        (
            "/docs/guide".to_string(),
            "Guide page mentions CTX_NO_LLMS_GUIDE_TOKEN.".to_string(),
        ),
    ]));
    let project = TestProject::new();

    project
        .ctx()
        .arg("add")
        .arg(format!("{}/docs", site.base_url))
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "no-llms-docs"])
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "CTX_NO_LLMS_GUIDE_TOKEN", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("no-llms-docs"));
    assert!(stdout.contains("CTX_NO_LLMS_GUIDE_TOKEN"));
}

#[test]
fn docs_crawl_discovers_llms_txt_links() {
    let site = FixtureSite::new(HashMap::from([
        (
            "/docs".to_string(),
            "<html><body><main>Docs home without direct llms-linked token.</main></body></html>"
                .to_string(),
        ),
        (
            "/docs/llms.txt".to_string(),
            "- [Guide](guide.md)\n- [API](api.md)".to_string(),
        ),
        (
            "/docs/guide.md".to_string(),
            "Guide page mentions CTX_LLMS_GUIDE_TOKEN.".to_string(),
        ),
        (
            "/docs/api.md".to_string(),
            "API page mentions CTX_LLMS_API_TOKEN.".to_string(),
        ),
    ]));
    let project = TestProject::new();

    project
        .ctx()
        .arg("add")
        .arg(format!("{}/docs", site.base_url))
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "llms-docs"])
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "CTX_LLMS_API_TOKEN", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("llms-docs"));
    assert!(stdout.contains("CTX_LLMS_API_TOKEN"));

    let show = project
        .ctx()
        .args(["show", "llms-docs", "--snapshots", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let show = String::from_utf8(show).unwrap();
    assert!(show.contains(",4,") || show.contains("page_count: 4"));
}

#[test]
fn query_debug_shows_llms_txt_source_prior() {
    let site = FixtureSite::new(HashMap::from([
        (
            "/docs".to_string(),
            "<html><body><main>Seed page mentions ordinary widgets.</main></body></html>"
                .to_string(),
        ),
        (
            "/docs/llms.txt".to_string(),
            "Curated llms docs mention CTX_LLMS_PRIOR_TOKEN.".to_string(),
        ),
    ]));
    let project = TestProject::new();

    project
        .ctx()
        .arg("add")
        .arg(format!("{}/docs", site.base_url))
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "prior-docs"])
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "CTX_LLMS_PRIOR_TOKEN", "--debug", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("source_prior: llms_txt"));
    assert!(stdout.contains("source_prior_score: 0.002"));
}

#[test]
fn update_keeps_pointer_on_noop_and_moves_on_content_change() {
    let site = FixtureSite::new(HashMap::from([(
        "/".to_string(),
        "<html><body><main>Initial docs mention alpha token.</main></body></html>".to_string(),
    )]));
    let project = TestProject::new();

    project
        .ctx()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    project
        .ctx()
        .arg("add")
        .arg(format!("{}/", site.base_url))
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "fixture-docs"])
        .assert()
        .success();
    project
        .ctx()
        .args(["link", "fixture-docs", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    let first = project.resource_current("fixture-docs");

    project
        .ctx()
        .args(["update", "fixture-docs", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    assert_eq!(project.resource_current("fixture-docs"), first);

    site.set(
        "/",
        "<html><body><main>Changed docs mention beta token.</main></body></html>",
    );
    project
        .ctx()
        .args(["update", "fixture-docs", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    assert_ne!(project.resource_current("fixture-docs"), first);
}

#[test]
fn use_rejects_missing_snapshot() {
    let project = TestProject::new();
    let note_path = project.root.path().join("notes.md");
    fs::write(&note_path, "A note about cache pruning.").unwrap();
    project
        .ctx()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    project
        .ctx()
        .arg("add")
        .arg(format!("file://{}", note_path.display()))
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "notes"])
        .assert()
        .success();
    project
        .ctx()
        .args(["link", "notes", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    project
        .ctx()
        .args(["use", "notes", "missing-snapshot", "--cwd"])
        .arg(project.root.path())
        .assert()
        .failure();
}

#[test]
fn unlink_removes_project_link_without_deleting_cache() {
    let project = TestProject::new();
    let note_path = project.root.path().join("notes.md");
    fs::write(&note_path, "A note about deleting cache entries.").unwrap();
    project
        .ctx()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    project
        .ctx()
        .arg("add")
        .arg(format!("file://{}", note_path.display()))
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "notes"])
        .assert()
        .success();
    project
        .ctx()
        .args(["link", "notes", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    let cache_path = project.resource_path("notes");
    assert!(std::path::Path::new(&cache_path).exists());

    project
        .ctx()
        .args(["unlink", "notes", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    assert!(std::path::Path::new(&cache_path).exists());
    assert!(
        project.manifest()["resources"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    project
        .ctx()
        .args(["remove", "notes", "--prune-cache", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    assert!(!std::path::Path::new(&cache_path).exists());
}

#[test]
fn install_copies_binary_to_requested_bin_dir() {
    let project = TestProject::new();
    let bin_dir = project.root.path().join("bin");
    project
        .ctx()
        .args(["install", "--bin-dir"])
        .arg(&bin_dir)
        .assert()
        .success();
    assert!(bin_dir.join("ctx").exists());
}

#[test]
#[ignore = "downloads a local embedding model; run for full retrieval smoke coverage"]
fn embeddings_enable_vector_results_when_lexical_search_misses() {
    let project = TestProject::new();
    let note_path = project.root.path().join("notes.md");
    fs::write(
        &note_path,
        "The compiler enforces borrowing and lifetimes so references stay valid.",
    )
    .unwrap();

    project
        .ctx_with_embeddings()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    project
        .ctx_with_embeddings()
        .arg("add")
        .arg(format!("file://{}", note_path.display()))
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "rust-notes"])
        .assert()
        .success();
    project
        .ctx_with_embeddings()
        .args(["link", "rust-notes", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let output = project
        .ctx_with_embeddings()
        .args(["query", "memory ownership rules", "--debug", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("borrowing and lifetimes"));
    assert!(stdout.contains("rrf_hybrid"));
    assert!(stdout.contains("vector_rank"));
}

#[test]
#[ignore = "live GitHub smoke test; run when network is available"]
fn live_github_source_can_be_cached_and_resolved_by_path() {
    let project = TestProject::new();
    project
        .ctx()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    project
        .ctx()
        .arg("add")
        .arg("https://github.com/octocat/Hello-World")
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "hello-world"])
        .assert()
        .success();
    project
        .ctx()
        .args(["link", "hello-world", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    let output = project
        .ctx()
        .args(["path", "hello-world", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let path = String::from_utf8(output).unwrap();
    assert!(std::path::Path::new(path.trim()).join(".git").exists());
}

#[test]
#[ignore = "live arXiv smoke test; run when network is available"]
fn live_arxiv_paper_can_be_added_and_queried() {
    let project = TestProject::new();
    project
        .ctx()
        .arg("add")
        .arg("https://arxiv.org/abs/1706.03762")
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "attention-paper"])
        .assert()
        .success();

    let list = project
        .ctx()
        .args(["list", "--kind", "research-paper", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list = String::from_utf8(list).unwrap();
    assert!(list.contains("attention-paper"));
    assert!(list.contains(",research_paper,"));

    let show = project
        .ctx()
        .args(["show", "attention-paper", "--snapshots", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let show = String::from_utf8(show).unwrap();
    assert!(show.contains("registry: arxiv"));
    assert!(show.contains("version: v"));

    let output = project
        .ctx()
        .args([
            "query",
            "What architecture dispenses with recurrence and convolutions?",
            "--label",
            "attention-paper",
            "--kind",
            "research-paper",
            "--cwd",
        ])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("Attention Is All You Need"));
    assert!(stdout.contains("Transformer"));
    assert!(stdout.contains("https://arxiv.org/abs/1706.03762"));
}
