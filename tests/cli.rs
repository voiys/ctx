use std::collections::HashMap;
use std::fs;
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
        "Retries use exponential backoff and jitter. ERR_RETRY_TIMEOUT means the retry budget was exhausted.",
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
