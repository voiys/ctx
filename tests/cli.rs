use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::{Arc, Mutex};
use std::thread;

use assert_cmd::Command;
use serde_json::{Value, json};
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
        command.env("CODEX_HOME", self.home.path().join(".codex"));
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
        let output = self
            .ctx()
            .args(["list", "--cwd"])
            .arg(self.root.path())
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let output = String::from_utf8(output).unwrap();
        let mut in_resource = false;
        let mut label_index = None;
        let mut local_path_index = None;
        for line in output.lines() {
            let line = line.trim();
            if line.starts_with("resources[")
                && let Some(start) = line.find('{')
                && let Some(end) = line[start + 1..].find('}')
            {
                let columns = line[start + 1..start + 1 + end]
                    .split(',')
                    .collect::<Vec<_>>();
                label_index = columns.iter().position(|column| *column == "label");
                local_path_index = columns.iter().position(|column| *column == "local_path");
                continue;
            }
            if let (Some(label_index), Some(local_path_index)) = (label_index, local_path_index) {
                let fields = split_toon_row(line);
                if fields.get(label_index).is_some_and(|value| value == label)
                    && let Some(value) = fields.get(local_path_index)
                {
                    return value.to_string();
                }
            }
            if let Some(value) = line.strip_prefix("label: ") {
                in_resource = value.trim_matches('"') == label;
            }
            if in_resource && let Some(value) = line.strip_prefix("local_path: ") {
                return value.trim_matches('"').to_string();
            }
        }
        panic!("no local_path for {label} in ctx list output:\n{output}");
    }

    fn assert_manifest_has_no_local_paths(&self) {
        for resource in self.manifest()["resources"].as_array().unwrap() {
            assert!(
                resource.get("local_path").is_none(),
                "manifest leaked local_path: {resource}"
            );
        }
    }

    fn ctx_with_git_config(&self, git_config: &Path) -> Command {
        let mut command = self.ctx();
        command.env("GIT_CONFIG_GLOBAL", git_config);
        command.env("GIT_CONFIG_NOSYSTEM", "1");
        command.env("GIT_ALLOW_PROTOCOL", "file");
        command
    }
}

fn split_toon_row(row: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in row.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(current.trim().trim_matches('"').to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current.trim().trim_matches('"').to_string());
    fields
}

struct GitFixture {
    _remote: TempDir,
    git_config: std::path::PathBuf,
    commit: String,
}

impl GitFixture {
    fn new(project_root: &Path) -> Self {
        let work = tempfile::tempdir().unwrap();
        run_git(work.path(), &["init"]);
        run_git(work.path(), &["config", "user.email", "ctx@example.com"]);
        run_git(work.path(), &["config", "user.name", "ctx test"]);
        run_git(work.path(), &["config", "commit.gpgsign", "false"]);
        fs::write(work.path().join("README.md"), "fixture source\n").unwrap();
        run_git(work.path(), &["add", "README.md"]);
        run_git(work.path(), &["commit", "-m", "initial source"]);
        let commit = run_git(work.path(), &["rev-parse", "HEAD"]);

        let remote = tempfile::tempdir().unwrap();
        let bare_path = remote.path().join("source.git");
        run_git(
            work.path(),
            &[
                "clone",
                "--bare",
                work.path().to_str().unwrap(),
                bare_path.to_str().unwrap(),
            ],
        );

        let git_config = project_root.join("gitconfig");
        fs::write(
            &git_config,
            format!(
                "[url \"file://{}\"]\n\tinsteadOf = https://github.com/fixture/source.git\n",
                bare_path.display()
            ),
        )
        .unwrap();

        Self {
            _remote: remote,
            git_config,
            commit,
        }
    }
}

fn run_git(cwd: &Path, args: &[&str]) -> String {
    let output = StdCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn root_help_explains_the_efficient_grounding_workflow_before_usage() {
    for args in [Vec::<&str>::new(), vec!["--help"]] {
        let output = Command::cargo_bin("ctx")
            .unwrap()
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let stdout = String::from_utf8(output).unwrap();
        let hint = stdout.find("Efficient grounding workflow:").unwrap();
        let usage = stdout.find("Usage:").unwrap();

        assert!(hint < usage, "{stdout}");
        assert!(stdout.contains("ctx show --cwd <repo>"));
        assert!(
            stdout.contains("ctx query \"<specific question>\" --label <docs-label> --cwd <repo>")
        );
        assert!(stdout.contains("real lockfile/manifest"));
        assert!(stdout.contains("official docs or version-pinned source"));
        assert!(stdout.contains("ctx agents --cwd <repo>"));
        assert!(stdout.contains("Unscoped/global query is discovery"));
        assert!(stdout.contains("matched chunks plus small local context"));
        assert!(stdout.contains("Default docs crawl is up to 2048 pages"));
        assert!(stdout.contains("not mandatory ceremony"));
        assert!(!stdout.contains("  memory"));
        assert!(!stdout.contains("  hook"));
        assert!(!stdout.contains("  recall"));
        assert!(!stdout.contains("  remember"));
    }
}

#[test]
fn init_and_agents_write_idempotent_grounding_guidance() {
    let project = TestProject::new();

    project
        .ctx()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let agents_path = project.root.path().join("AGENTS.md");
    let agents = fs::read_to_string(&agents_path).unwrap();
    assert!(agents.contains("authoritative external or version-pinned references"));
    assert!(agents.contains("Live source and runtime behavior win"));
    assert!(agents.contains("ctx show --cwd <repo>"));
    assert!(agents.contains("ctx path <source-label> --cwd <repo>"));
    assert!(agents.contains("ctx query \"<specific question>\" --label <label> --cwd <repo>"));
    assert!(agents.contains("manifest or lockfile"));
    assert!(agents.contains("ctx agents --cwd <repo>"));
    assert!(!agents.contains("memory"));
    assert!(!agents.contains("hook"));

    project
        .ctx()
        .args(["agents", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let agents = fs::read_to_string(agents_path).unwrap();
    assert_eq!(agents.matches("<!-- ctx:start -->").count(), 1);
    assert_eq!(agents.matches("<!-- ctx:end -->").count(), 1);
}

#[cfg(unix)]
#[test]
fn init_refuses_symlinked_agents_file() {
    let project = TestProject::new();
    let outside = project.root.path().join("outside.md");
    fs::write(&outside, "keep me").unwrap();
    std::os::unix::fs::symlink(&outside, project.root.path().join("AGENTS.md")).unwrap();

    project
        .ctx()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .failure();

    assert_eq!(fs::read_to_string(outside).unwrap(), "keep me");
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
    project.assert_manifest_has_no_local_paths();

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
fn query_can_find_notes_by_resource_label() {
    let project = TestProject::new();
    let note_path = project.root.path().join("notes.md");
    fs::write(
        &note_path,
        "# Escalation Runbook\n\nCTX_LABEL_SEARCH_TOKEN is the body marker.",
    )
    .unwrap();

    project
        .ctx()
        .arg("add")
        .arg(format!("file://{}", note_path.display()))
        .args(["--label", "customer-oncall-playbook", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "customer oncall playbook", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("customer-oncall-playbook"));
    assert!(stdout.contains("CTX_LABEL_SEARCH_TOKEN"));
}

#[test]
fn agent_query_groups_matches_with_surrounding_evidence() {
    let project = TestProject::new();
    let note_path = project.root.path().join("agent-notes.md");
    fs::write(
        &note_path,
        [
            "# Review Client Job",
            "",
            "Use this workflow when a client submits a job.",
            "",
            "## Step 1: Open Intake",
            "",
            "Open the submitted job intake record.",
            "",
            "## Step 2: Review Details",
            "",
            "Check compensation gamma details against the client request.",
            "",
            "## Step 3: Filler A",
            "",
            "Ordinary filler content that should not be returned.",
            "",
            "## Step 4: Filler B",
            "",
            "Ordinary filler content that should not be returned.",
            "",
            "## Step 5: Filler C",
            "",
            "Ordinary filler content that should not be returned.",
            "",
            "## Step 6: Filler Middle",
            "",
            "This distant middle section should stay out of agent evidence.",
            "",
            "## Step 7: Filler D",
            "",
            "Ordinary filler content that should not be returned.",
            "",
            "## Step 8: Filler E",
            "",
            "Ordinary filler content that should not be returned.",
            "",
            "## Step 9: Filler F",
            "",
            "Ordinary filler content that should not be returned.",
            "",
            "## Step 10: Final Approval",
            "",
            "Record final approval delta before publishing.",
            "",
            "## Step 11: Notify Stakeholders",
            "",
            "Share the outcome with the client team.",
            "",
            "## Step 12: Close Intake",
            "",
            "Close the intake task.",
        ]
        .join("\n"),
    )
    .unwrap();

    project
        .ctx()
        .arg("add")
        .arg(format!("file://{}", note_path.display()))
        .args(["--label", "review-job-notes", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "compensation gamma final approval delta", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("mode: agent"));
    assert!(stdout.contains("evidence["));
    assert!(stdout.contains("kind: match"));
    assert!(stdout.contains("kind: \"context-between\""));
    assert!(stdout.contains("kind: \"context-before\""));
    assert!(stdout.contains("kind: \"context-after\""));
    assert!(stdout.contains("Step 2: Review Details"));
    assert!(stdout.contains("Step 10: Final Approval"));
    assert!(!stdout.contains("Step 6: Filler Middle"));
    assert_eq!(stdout.matches("label: \"review-job-notes\"").count(), 1);
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
    project.assert_manifest_has_no_local_paths();

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
    project.assert_manifest_has_no_local_paths();
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
fn legacy_manifest_local_path_is_stripped_on_next_write() {
    let project = TestProject::new();
    let note_path = project.root.path().join("notes.md");
    fs::write(&note_path, "A note about portable manifests.").unwrap();
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
    project.assert_manifest_has_no_local_paths();

    let mut manifest = project.manifest();
    manifest["resources"][0]["local_path"] = json!("/tmp/machine-local-cache-path");
    fs::write(
        project.root.path().join(".ctx/ctx.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    assert!(
        project.manifest()["resources"][0]
            .get("local_path")
            .is_some()
    );

    let current = project.resource_current("notes");
    project
        .ctx()
        .args(["use", "notes", &current, "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    assert_eq!(project.resource_current("notes"), current);
    project.assert_manifest_has_no_local_paths();
}

#[test]
fn manifest_spoofed_paths_do_not_drive_update_sync_or_path() {
    let project = TestProject::new();
    let safe_note = project.root.path().join("safe.md");
    fs::write(&safe_note, "Safe note carries CTX_SAFE_NOTE_ONLY.").unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret_note = outside.path().join("secret.md");
    fs::write(&secret_note, "Secret note carries CTX_SECRET_NOTE_ONLY.").unwrap();
    let evil_snapshot = outside.path().join("snapshot");
    fs::create_dir_all(&evil_snapshot).unwrap();
    fs::write(
        evil_snapshot.join("pages.json"),
        r#"[{"url":"file:///evil","content":"Evil snapshot carries CTX_EVIL_SNAPSHOT_ONLY."}]"#,
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
        .arg(format!("file://{}", safe_note.display()))
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

    let mut manifest = project.manifest();
    manifest["resources"][0]["url"] = json!(format!("file://{}", secret_note.display()));
    manifest["resources"][0]["local_path"] = json!(evil_snapshot.display().to_string());
    fs::write(
        project.root.path().join(".ctx/ctx.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    project
        .ctx()
        .args(["sync", "--reindex", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let evil_query = project
        .ctx()
        .args(["query", "CTX_EVIL_SNAPSHOT_ONLY", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        !String::from_utf8(evil_query)
            .unwrap()
            .contains("Evil snapshot carries")
    );

    project
        .ctx()
        .args(["update", "notes", "--force", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let secret_query = project
        .ctx()
        .args(["query", "CTX_SECRET_NOTE_ONLY", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        !String::from_utf8(secret_query)
            .unwrap()
            .contains("Secret note carries")
    );

    manifest["resources"][0] = json!({
        "id": "spoofed-source",
        "label": "spoofed-source",
        "kind": "source",
        "url": "https://github.com/example/repo",
        "reason": null,
        "current": "main",
        "local_path": outside.path().display().to_string(),
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    });
    fs::write(
        project.root.path().join(".ctx/ctx.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    project
        .ctx()
        .args(["path", "spoofed-source", "--cwd"])
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
    let output = project
        .ctx()
        .args(["install", "--bin-dir"])
        .arg(&bin_dir)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("install_version_status: new_install"));
    assert!(bin_dir.join("ctx").exists());
    let metadata_path = bin_dir.join("ctx.install.json");
    assert!(metadata_path.exists());
    let metadata: Value =
        serde_json::from_str(&fs::read_to_string(metadata_path).unwrap()).unwrap();
    assert_eq!(metadata["version"], env!("CARGO_PKG_VERSION"));
    let skill_dir = project.home.path().join(".codex/skills/ctx");
    assert!(skill_dir.join("SKILL.md").exists());
    assert!(skill_dir.join("agents/openai.yaml").exists());
    let skill = fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
    assert!(skill.contains("name: ctx"));
    assert!(skill.contains("Do not invoke for every task"));
    assert!(skill.contains("exact installed version"));
}

#[test]
fn install_reports_replacing_untracked_binary() {
    let project = TestProject::new();
    let bin_dir = project.root.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::write(bin_dir.join("ctx"), "old binary placeholder").unwrap();
    let output = project
        .ctx()
        .args(["install", "--bin-dir"])
        .arg(&bin_dir)
        .arg("--force")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("install_version_status: replaced_untracked_install"));
    assert!(output.contains("existing ctx install had no version metadata"));
}

#[test]
fn doctor_reports_outdated_default_install_metadata() {
    let project = TestProject::new();
    let bin_dir = project.home.path().join(".local/bin");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::write(bin_dir.join("ctx"), "old binary placeholder").unwrap();
    fs::write(
        bin_dir.join("ctx.install.json"),
        r#"{
  "version": "0.1.0",
  "installed_at": "2026-01-01T00:00:00Z",
  "source": "/tmp/ctx",
  "target": "/tmp/home/.local/bin/ctx"
}
"#,
    )
    .unwrap();
    let output = project
        .ctx()
        .env("HOME", project.home.path())
        .args(["doctor", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("installed_version: \"0.1.0\""));
    assert!(output.contains("status: outdated"));
    assert!(output.contains("default ctx install is older"));
}

#[test]
fn sync_rehydrates_missing_source_checkout_from_manifest() {
    let project = TestProject::new();
    let fixture = GitFixture::new(project.root.path());
    project
        .ctx()
        .args(["init", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    project
        .ctx_with_git_config(&fixture.git_config)
        .arg("add")
        .arg("https://github.com/fixture/source")
        .args(["--cwd"])
        .arg(project.root.path())
        .args(["--label", "fixture-source"])
        .assert()
        .success();
    project
        .ctx()
        .args(["link", "fixture-source", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    project.assert_manifest_has_no_local_paths();
    assert_eq!(project.resource_current("fixture-source"), fixture.commit);

    let output = project
        .ctx()
        .args(["path", "fixture-source", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first_path = String::from_utf8(output).unwrap().trim().to_string();
    assert!(std::path::Path::new(&first_path).join(".git").exists());

    project
        .ctx()
        .args(["remove", "fixture-source", "--prune-cache", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();
    assert!(!std::path::Path::new(&first_path).exists());
    project
        .ctx()
        .args(["path", "fixture-source", "--cwd"])
        .arg(project.root.path())
        .assert()
        .failure();

    let sync = project
        .ctx_with_git_config(&fixture.git_config)
        .args(["sync", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let sync = String::from_utf8(sync).unwrap();
    assert!(
        sync.contains("resources[1]{label,kind,ready,rehydrated,message}")
            && sync.contains("\"fixture-source\",source,true,true"),
        "{sync}"
    );

    let output = project
        .ctx()
        .args(["path", "fixture-source", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second_path = String::from_utf8(output).unwrap().trim().to_string();
    assert!(std::path::Path::new(&second_path).join(".git").exists());
    assert_eq!(project.resource_current("fixture-source"), fixture.commit);
    project.assert_manifest_has_no_local_paths();
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
        .args(["query", "resource ownership rules", "--debug", "--cwd"])
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
