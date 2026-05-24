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
        .args(["use", "notes", "missing-snapshot", "--cwd"])
        .arg(project.root.path())
        .assert()
        .failure();
}

#[test]
fn remove_prune_cache_deletes_unused_snapshot() {
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
    let cache_path = project.resource_path("notes");
    assert!(std::path::Path::new(&cache_path).exists());

    project
        .ctx()
        .args(["remove", "notes", "--prune-cache", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    assert!(!std::path::Path::new(&cache_path).exists());
    assert!(
        project.manifest()["resources"]
            .as_array()
            .unwrap()
            .is_empty()
    );
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
