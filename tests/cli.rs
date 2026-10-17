use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;
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

    fn resource_path(&self, label: &str) -> String {
        let output = self
            .ctx()
            .args(["list"])
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
        assert!(
            stdout.find("Offline reference workflow:").unwrap() < stdout.find("Usage:").unwrap()
        );
        assert!(stdout.contains("ctx guides"));
        assert!(stdout.contains("Usage:"));
        for obsolete in [
            "--cwd",
            "ctx init",
            "ctx link",
            "AGENTS",
            "ledger",
            "notes",
            "  memory",
            "  hook",
            "  recall",
            "  remember",
        ] {
            assert!(!stdout.contains(obsolete), "{stdout}");
        }
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
fn legacy_notes_data_is_wiped_on_open() {
    const LEGACY_NOTES_KIND: &str = "notes";

    let project = TestProject::new();
    let site = FixtureSite::new(HashMap::from([(
        "/current.md".to_string(),
        "Current documentation contains migration_token.".to_string(),
    )]));
    project
        .ctx()
        .arg("add")
        .arg(format!("{}/current.md", site.base_url))
        .args(["--label", "current-docs"])
        .assert()
        .success();

    let id = "legacy-id";
    let dir = project.home.path().join(LEGACY_NOTES_KIND);
    fs::create_dir_all(dir.join(id)).unwrap();
    fs::write(dir.join(id).join("content.md"), "migration_token").unwrap();
    let conn = rusqlite::Connection::open(project.home.path().join("ctx.db")).unwrap();
    conn.execute(
        "INSERT INTO resources (id, label, kind, url, current, local_path, updated_at)
         VALUES (?1, ?1, ?2, 'https://example.com/legacy', 'snapshot', ?3, '2026-01-01')",
        rusqlite::params![id, LEGACY_NOTES_KIND, dir.join(id).to_str().unwrap()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO snapshots (resource_id, snapshot_id, kind, source_url, content_hash, fetched_at, page_count, path)
         VALUES (?1, 'snapshot', ?2, 'https://example.com/legacy', 'hash', '2026-01-01', 1, ?3)",
        rusqlite::params![id, LEGACY_NOTES_KIND, dir.join(id).to_str().unwrap()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO chunks (resource_id, snapshot_id, kind, label, source_url, chunk_index, content)
         VALUES (?1, 'snapshot', ?2, ?1, 'https://example.com/legacy', 0, 'migration_token')",
        rusqlite::params![id, LEGACY_NOTES_KIND],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO chunks_fts (content, resource_id, snapshot_id, label)
         VALUES ('migration_token', ?1, 'snapshot', ?1)",
        rusqlite::params![id],
    )
    .unwrap();
    for args in [vec!["list"], vec!["query", "migration_token"]] {
        project.ctx().args(args).assert().success();
    }
    for table in ["resources", "snapshots", "chunks"] {
        let count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE kind = ?1"),
                rusqlite::params![LEGACY_NOTES_KIND],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks_fts WHERE resource_id = ?1",
            rusqlite::params![id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    assert!(!dir.exists());
}

#[test]
fn global_docs_can_be_used_without_project_manifest() {
    let project = TestProject::new();
    let site = FixtureSite::new(HashMap::from([(
        "/global.md".to_string(),
        "Global docs shelf contains CTX_GLOBAL_ONLY_TOKEN for retrieval.".to_string(),
    )]));

    project
        .ctx()
        .arg("add")
        .arg(format!("{}/global.md", site.base_url))
        .args(["--label", "global-docs"])
        .assert()
        .success();
    assert!(!project.root.path().join(".ctx/ctx.json").exists());

    let query = project
        .ctx()
        .args(["query", "CTX_GLOBAL_ONLY_TOKEN"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let query = String::from_utf8(query).unwrap();
    assert!(!query.contains("scope:"));
    assert!(query.contains("CTX_GLOBAL_ONLY_TOKEN"));

    let show = project
        .ctx()
        .args(["list", "global-docs", "--snapshots"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let show = String::from_utf8(show).unwrap();
    assert!(show.contains("global-docs"));
    assert!(show.contains("snapshot_id"));

    project
        .ctx()
        .args(["remove", "global-docs"])
        .assert()
        .success();

    let list = project
        .ctx()
        .args(["list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list = String::from_utf8(list).unwrap();
    assert!(!list.contains("global-docs"));
}

#[test]
fn query_can_find_docs_by_resource_label() {
    let project = TestProject::new();
    let site = FixtureSite::new(HashMap::from([(
        "/runbook.md".to_string(),
        "# Escalation Runbook\n\nCTX_LABEL_SEARCH_TOKEN is the body marker.".to_string(),
    )]));

    project
        .ctx()
        .arg("add")
        .arg(format!("{}/runbook.md", site.base_url))
        .args(["--label", "customer-oncall-playbook"])
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "customer oncall playbook"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("customer-oncall-playbook"));
    assert!(stdout.contains("CTX_LABEL_SEARCH_TOKEN"));

    site.set("/second.md", "CTX_SECOND_TOKEN is another body marker.");
    project
        .ctx()
        .args([
            "add",
            &format!("{}/second.md", site.base_url),
            "--label",
            "second-docs",
        ])
        .assert()
        .success();
    let output = project
        .ctx()
        .args([
            "query",
            "body marker",
            "--label",
            "customer-oncall-playbook",
            "--label",
            "second-docs",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("CTX_LABEL_SEARCH_TOKEN"));
    assert!(output.contains("CTX_SECOND_TOKEN"));
    let error = project
        .ctx()
        .args([
            "query",
            "body marker",
            "--label",
            "customer-oncall-playbook",
            "--label",
            "nope",
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    assert!(
        String::from_utf8(error)
            .unwrap()
            .contains("no queryable resource matched label")
    );
}

#[test]
fn query_latest_snapshot_survives_many_updates() {
    let project = TestProject::new();
    let paragraph = format!("crowdingtoken {}", "ordinary filler ".repeat(85));
    let body = |revision| {
        let padding = if revision == 10 {
            " extra".repeat(20)
        } else {
            String::new()
        };
        format!(
            "{}\n\nRevision {revision}.",
            (0..51)
                .map(|_| format!("{paragraph}{padding}"))
                .collect::<Vec<_>>()
                .join("\n\n")
        )
    };
    let site = FixtureSite::new(HashMap::from([("/crowding.md".to_string(), body(0))]));
    project
        .ctx()
        .arg("add")
        .arg(format!("{}/crowding.md", site.base_url))
        .args(["--label", "crowding"])
        .assert()
        .success();
    for revision in 1..=10 {
        site.set("/crowding.md", &body(revision));
        project
            .ctx()
            .args(["update", "crowding"])
            .assert()
            .success();
    }
    let latest_path = project.resource_path("crowding");
    let latest = Path::new(&latest_path)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let conn = rusqlite::Connection::open(project.home.path().join("ctx.db")).unwrap();
    let historical: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunks WHERE snapshot_id != ?1 AND plain_text LIKE '%crowdingtoken%'",
        rusqlite::params![latest], |row| row.get(0),
    ).unwrap();
    assert!(
        historical > 500,
        "only {historical} historical matching chunks"
    );
    let output = project
        .ctx()
        .args([
            "query",
            "crowdingtoken",
            "--label",
            "crowding",
            "--top-k",
            "1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("kind: match"), "{stdout}");
    assert!(stdout.contains("evidence["), "{stdout}");
    assert!(
        stdout.contains(&format!("snapshot_id: \"{latest}\"")),
        "{stdout}"
    );
}

#[test]
fn agent_query_groups_matches_with_surrounding_evidence() {
    let project = TestProject::new();
    let body = [
        "# Review Client Job\nUse this workflow when a client submits a job.",
        "## Step 1: Open Intake\nOpen the submitted job intake record.",
        "## Step 2: Review Details\nCheck compensation gamma details against the client request.",
        "## Step 3: Filler A\nOrdinary filler content that should not be returned.",
        "## Step 4: Filler B\nOrdinary filler content that should not be returned.",
        "## Step 5: Filler C\nOrdinary filler content that should not be returned.",
        "## Step 6: Filler Middle\nThis distant middle section should stay out of agent evidence.",
        "## Step 7: Filler D\nOrdinary filler content that should not be returned.",
        "## Step 8: Filler E\nOrdinary filler content that should not be returned.",
        "## Step 9: Filler F\nOrdinary filler content that should not be returned.",
        "## Step 10: Final Approval\nRecord final approval delta before publishing.",
        "## Step 11: Notify Stakeholders\nShare the outcome with the client team.",
        "## Step 12: Close Intake\nClose the intake task.",
    ]
    .map(|step| format!("{step} {}", "filler ".repeat(200)))
    .join("\n\n");
    let site = FixtureSite::new(HashMap::from([("/agent.md".to_string(), body)]));

    project
        .ctx()
        .arg("add")
        .arg(format!("{}/agent.md", site.base_url))
        .args(["--label", "review-job-docs"])
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "compensation gamma final approval delta"])
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
    assert_eq!(stdout.matches("label: \"review-job-docs\"").count(), 1);
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
        .arg("add")
        .arg(format!("{}/", site.base_url))
        .args(["--label", "fixture-docs"])
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "CTX_SPECIAL_RETRY_FLAG"])
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
        .args(["list", "fixture-docs", "--snapshots"])
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
        .args(["--label", "no-llms-docs"])
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "CTX_NO_LLMS_GUIDE_TOKEN"])
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
        .args(["--label", "llms-docs"])
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "CTX_LLMS_API_TOKEN"])
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
        .args(["list", "llms-docs", "--snapshots"])
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
        .args(["--label", "prior-docs"])
        .assert()
        .success();

    let output = project
        .ctx()
        .args(["query", "CTX_LLMS_PRIOR_TOKEN", "--debug"])
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
        .arg("add")
        .arg(format!("{}/", site.base_url))
        .args(["--label", "fixture-docs"])
        .assert()
        .success();
    let list = project
        .ctx()
        .args(["list", "fixture-docs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list = String::from_utf8(list).unwrap();
    let first = list
        .lines()
        .find_map(|line| {
            let line = line.trim();
            line.strip_prefix("current: ")
                .map(|value| value.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| {
            let header = list
                .lines()
                .find(|line| line.trim().starts_with("resources["))
                .unwrap();
            let columns = header
                .split('{')
                .nth(1)
                .unwrap()
                .split('}')
                .next()
                .unwrap()
                .split(',')
                .collect::<Vec<_>>();
            let index = columns
                .iter()
                .position(|column| *column == "current")
                .unwrap();
            let row = list
                .lines()
                .skip_while(|line| !line.trim().starts_with("resources["))
                .nth(1)
                .unwrap();
            split_toon_row(row)[index].clone()
        });

    project
        .ctx()
        .args(["update", "fixture-docs"])
        .assert()
        .success();
    let list = project
        .ctx()
        .args(["list", "fixture-docs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8(list).unwrap().contains(&first));

    site.set(
        "/",
        "<html><body><main>Changed docs mention beta token.</main></body></html>",
    );
    project
        .ctx()
        .args(["update", "fixture-docs"])
        .assert()
        .success();
    let list = project
        .ctx()
        .args(["list", "fixture-docs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(!String::from_utf8(list).unwrap().contains(&first));
    let latest = project
        .ctx()
        .args(["query", "alpha token", "--label", "fixture-docs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(!String::from_utf8(latest).unwrap().contains("Initial docs"));
    let latest = project
        .ctx()
        .args(["query", "beta token", "--label", "fixture-docs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8(latest).unwrap().contains("Changed docs"));
    let old = project
        .ctx()
        .args([
            "query",
            "alpha token",
            "--label",
            "fixture-docs",
            "--snapshot",
            &first,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8(old).unwrap().contains("Initial docs"));
    let error = project
        .ctx()
        .args([
            "query",
            "x",
            "--label",
            "fixture-docs",
            "--snapshot",
            "does-not-exist",
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    assert!(
        String::from_utf8(error)
            .unwrap()
            .contains("snapshot not found: does-not-exist")
    );
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
    assert!(!project.home.path().join(".codex/skills").exists());
    assert!(!output.contains("skill_path"));
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
        .args(["doctor"])
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
fn add_by_sha_restores_pruned_checkout_and_is_idempotent_offline() {
    let project = TestProject::new();
    let fixture = GitFixture::new(project.root.path());
    let url = format!("https://github.com/fixture/source/tree/{}", fixture.commit);
    project
        .ctx_with_git_config(&fixture.git_config)
        .args(["add", &url, "--label", "fixture-source"])
        .assert()
        .success();
    let first_path = project.resource_path("fixture-source");
    assert!(Path::new(&first_path).join(".git").exists());
    project
        .ctx()
        .args(["remove", "fixture-source", "--prune-cache"])
        .assert()
        .success();
    assert!(!Path::new(&first_path).exists());
    project
        .ctx()
        .args(["path", "fixture-source"])
        .assert()
        .failure();
    project
        .ctx_with_git_config(&fixture.git_config)
        .args(["add", &url, "--label", "fixture-source"])
        .assert()
        .success();
    assert!(
        Path::new(&project.resource_path("fixture-source"))
            .join(".git")
            .exists()
    );
    let empty_config = project.root.path().join("empty-gitconfig");
    fs::write(&empty_config, "").unwrap();
    let output = project
        .ctx_with_git_config(&empty_config)
        .args(["add", &url, "--label", "fixture-source"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8(output)
            .unwrap()
            .contains("existing: true")
    );
    let output = project
        .ctx()
        .args(["list", "fixture-source"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains(&fixture.commit));
    let mut lines = output
        .lines()
        .skip_while(|line| !line.trim().starts_with("resources["));
    let header = lines.next().unwrap();
    let columns = header
        .split('{')
        .nth(1)
        .unwrap()
        .split('}')
        .next()
        .unwrap()
        .split(',')
        .collect::<Vec<_>>();
    let current_index = columns
        .iter()
        .position(|column| *column == "current")
        .unwrap();
    let row = split_toon_row(lines.next().unwrap());
    assert_eq!(row[current_index], fixture.commit);
}

#[test]
#[ignore = "downloads a local embedding model; run for full retrieval smoke coverage"]
fn embeddings_enable_vector_results_when_lexical_search_misses() {
    let project = TestProject::new();
    let site = FixtureSite::new(HashMap::from([(
        "/rust.md".to_string(),
        "The compiler enforces borrowing and lifetimes so references stay valid.".to_string(),
    )]));

    project
        .ctx_with_embeddings()
        .arg("add")
        .arg(format!("{}/rust.md", site.base_url))
        .args(["--label", "rust-docs"])
        .assert()
        .success();

    let output = project
        .ctx_with_embeddings()
        .args(["query", "resource ownership rules", "--debug"])
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
        .arg("add")
        .arg("https://github.com/octocat/Hello-World")
        .args(["--label", "hello-world"])
        .assert()
        .success();
    let output = project
        .ctx()
        .args(["path", "hello-world"])
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
        .args(["--label", "attention-paper"])
        .assert()
        .success();

    let list = project
        .ctx()
        .args(["list", "--kind", "research-paper"])
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
        .args(["list", "attention-paper", "--snapshots"])
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
        ])
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

#[test]
fn add_rejects_label_conflict_before_fetching() {
    let project = TestProject::new();
    let site = FixtureSite::new(HashMap::from([(
        "/".to_string(),
        "Cached docs.".to_string(),
    )]));
    let url = format!("{}/", site.base_url);
    project
        .ctx()
        .args(["add", &url, "--label", "taken"])
        .assert()
        .success();
    let error = project
        .ctx()
        .args(["add", "https://example.invalid/", "--label", "taken"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8(error).unwrap().contains(&format!(
        "label 'taken' is already used by {url}; remove it or choose another label"
    )));
}

#[test]
fn add_persists_reason_and_preserves_it_on_readd_and_update() {
    let project = TestProject::new();
    // Exercise the migration from the existing resource schema, not just a fresh database.
    let conn = rusqlite::Connection::open(project.home.path().join("ctx.db")).unwrap();
    conn.execute_batch("CREATE TABLE resources (id TEXT PRIMARY KEY, label TEXT NOT NULL, kind TEXT NOT NULL, url TEXT NOT NULL, current TEXT NOT NULL, local_path TEXT, updated_at TEXT NOT NULL);").unwrap();
    let site = FixtureSite::new(HashMap::from([(
        "/".to_string(),
        "Initial documentation.".to_string(),
    )]));
    let url = format!("{}/", site.base_url);
    let output = project
        .ctx()
        .args([
            "add",
            &url,
            "--label",
            "reason-docs",
            "--reason",
            "authoritative-reference",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8(output)
            .unwrap()
            .contains("authoritative-reference")
    );
    site.set("/", "Changed documentation.");
    let output = project
        .ctx()
        .args(["add", &url, "--label", "reason-docs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("existing: true"));
    assert!(output.contains("authoritative-reference"));
    let result = project
        .ctx()
        .args(["query", "Initial", "--label", "reason-docs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8(result)
            .unwrap()
            .contains("Initial documentation")
    );
    project
        .ctx()
        .args(["update", "reason-docs"])
        .assert()
        .success();
    let list = project
        .ctx()
        .args(["list", "reason-docs"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8(list)
            .unwrap()
            .contains("authoritative-reference")
    );
    project
        .ctx()
        .args([
            "add",
            &url,
            "--label",
            "reason-docs",
            "--reason",
            "revised-reference",
        ])
        .assert()
        .success();
    let list = project
        .ctx()
        .arg("list")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8(list)
            .unwrap()
            .contains("revised-reference")
    );
}

#[test]
fn invalid_legacy_manifest_is_ignored_by_query_and_doctor() {
    let project = TestProject::new();
    fs::create_dir_all(project.root.path().join(".ctx")).unwrap();
    fs::write(project.root.path().join(".ctx/ctx.json"), "{broken").unwrap();
    project
        .ctx()
        .current_dir(project.root.path())
        .args(["query", "x"])
        .assert()
        .success();
    project
        .ctx()
        .current_dir(project.root.path())
        .arg("doctor")
        .assert()
        .success();
}

#[test]
fn guides_list_embedded_workflows_and_report_unknown_names() {
    let project = TestProject::new();
    let output = project
        .ctx()
        .arg("guides")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    for name in [
        "pin",
        "idiomatic-usage",
        "copy-pattern",
        "tests-as-spec",
        "trace-api",
        "error-to-source",
        "upgrade-impact",
        "why-history",
    ] {
        assert!(
            output
                .lines()
                .any(|line| line.starts_with(&format!("{name}  ")))
        );
    }
    let output = project
        .ctx()
        .args(["guides", "pin"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        include_str!("../guides/pin.md")
    );
    assert!(include_str!("../guides/pin.md").contains("ctx path"));
    let error = project
        .ctx()
        .args(["guides", "nope"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    assert!(
        String::from_utf8(error)
            .unwrap()
            .contains("unknown guide 'nope'; run `ctx guides` to list guides")
    );
}
