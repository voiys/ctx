use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::{Arc, Mutex};
use std::thread;

use assert_cmd::Command;
use rusqlite::Connection;
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

fn ctx_with_home(home: &Path) -> Command {
    let mut command = Command::cargo_bin("ctx").unwrap();
    command.env("CTX_HOME", home);
    command.env("CTX_EMBEDDINGS", "off");
    command
}

fn first_toon_id(stdout: &[u8]) -> String {
    first_toon_field(stdout, "id")
}

fn first_toon_field(stdout: &[u8], field: &str) -> String {
    let stdout = String::from_utf8(stdout.to_vec()).unwrap();
    let prefix = format!("{field}: ");
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .map(|value| value.trim_matches('"').to_string())
        .unwrap_or_else(|| panic!("no {field} line in output:\n{stdout}"))
}

fn first_toon_usize(stdout: &[u8], field: &str) -> usize {
    first_toon_field(stdout, field).parse().unwrap()
}

#[test]
fn root_help_puts_agent_quick_start_before_usage() {
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
        let hint = stdout.find("Agent quick start:").unwrap();
        let usage = stdout.find("Usage:").unwrap();

        assert!(hint < usage, "{stdout}");
        assert!(stdout.contains("Read repo AGENTS.md and selected skill docs first"));
        assert!(stdout.contains("ctx recall \"<task or error>\" --cwd <repo>"));
        assert!(stdout.contains("ctx query \"<question>\" --label <docs-label> --cwd <repo>"));
        assert!(stdout.contains("Unscoped/global query is discovery"));
        assert!(stdout.contains("matched chunks plus small local context"));
        assert!(stdout.contains("Default docs crawl is up to 2048 pages"));
        assert!(stdout.contains("Do not store secrets."));
    }
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
        .args(["recall", "ERROR_ALPHA smoke", "--cwd"])
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
fn personal_export_import_restores_notes_and_memories_with_markers() {
    let source = TestProject::new();
    let note_path = source.root.path().join("personal-notes.md");
    fs::write(
        &note_path,
        "# Personal Notes\n\nCTX_PERSONAL_NOTE_TOKEN belongs in restored notes.",
    )
    .unwrap();

    source
        .ctx()
        .arg("add")
        .arg(format!("file://{}", note_path.display()))
        .args(["--label", "personal-notes", "--cwd"])
        .arg(source.root.path())
        .assert()
        .success();
    source
        .ctx()
        .arg("remember")
        .arg("Project memory carries CTX_PERSONAL_MEMORY_TOKEN.")
        .args(["--kind", "fact", "--subject", "personal.export", "--cwd"])
        .arg(source.root.path())
        .assert()
        .success();

    let export_path = source.root.path().join("ctx-personal-export.json");
    source
        .ctx()
        .arg("export")
        .arg(&export_path)
        .args(["--cwd"])
        .arg(source.root.path())
        .assert()
        .success();
    let export = fs::read_to_string(&export_path).unwrap();
    assert!(export.contains("\"kind\": \"ctx_personal_export\""));
    assert!(export.contains("\"exported_at\""));
    assert!(export.contains("CTX_PERSONAL_NOTE_TOKEN"));
    assert!(export.contains("CTX_PERSONAL_MEMORY_TOKEN"));

    let target_home = tempfile::tempdir().unwrap();
    let target_root = tempfile::tempdir().unwrap();
    ctx_with_home(target_home.path())
        .arg("import")
        .arg(&export_path)
        .args(["--cwd"])
        .arg(target_root.path())
        .assert()
        .success();

    let recall = ctx_with_home(target_home.path())
        .args(["recall", "CTX_PERSONAL_MEMORY_TOKEN", "--cwd"])
        .arg(target_root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recall = String::from_utf8(recall).unwrap();
    assert!(recall.contains("CTX_PERSONAL_MEMORY_TOKEN"));
    assert!(recall.contains("Imported from ctx personal export"));
    assert!(recall.contains("original memory scope: project"));

    let query = ctx_with_home(target_home.path())
        .args(["query", "CTX_PERSONAL_NOTE_TOKEN", "--cwd"])
        .arg(target_root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let query = String::from_utf8(query).unwrap();
    assert!(query.contains("CTX_PERSONAL_NOTE_TOKEN"));
    assert!(query.contains("ctx-import://notes/"));

    let marker_query = ctx_with_home(target_home.path())
        .args(["query", "Imported from ctx personal export", "--cwd"])
        .arg(target_root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let marker_query = String::from_utf8(marker_query).unwrap();
    assert!(marker_query.contains("Imported from ctx personal export"));
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
fn memory_show_and_forget_require_visible_scope() {
    let home = tempfile::tempdir().unwrap();
    let project_a = tempfile::tempdir().unwrap();
    let project_b = tempfile::tempdir().unwrap();

    let remember = ctx_with_home(home.path())
        .arg("remember")
        .arg("Project A direct memory carries CTX_DIRECT_A_ONLY.")
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
        .success()
        .get_output()
        .stdout
        .clone();
    let memory_id = first_toon_id(&remember);

    ctx_with_home(home.path())
        .args(["memory", "show", &memory_id, "--cwd"])
        .arg(project_b.path())
        .assert()
        .failure();

    ctx_with_home(home.path())
        .args(["memory", "forget", &memory_id, "--cwd"])
        .arg(project_b.path())
        .assert()
        .failure();

    ctx_with_home(home.path())
        .args(["memory", "show", &memory_id, "--cwd"])
        .arg(project_a.path())
        .assert()
        .success();
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
fn memory_accept_reject_and_hook_recall_pack_active_context() {
    let project = TestProject::new();

    let suggested = project
        .ctx()
        .arg("remember")
        .arg("When doing hook recall promotion, use COMPACT_SUMMARY_MODE.")
        .args([
            "--kind",
            "preference",
            "--subject",
            "hook.recall",
            "--suggested",
            "--cwd",
        ])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let memory_id = first_toon_id(&suggested);

    let before = project
        .ctx()
        .args(["hook", "recall", "hook recall promotion", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let before = String::from_utf8(before).unwrap();
    assert!(!before.contains("COMPACT_SUMMARY_MODE"));

    let accepted = project
        .ctx()
        .args(["memory", "accept", &memory_id, "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let accepted = String::from_utf8(accepted).unwrap();
    assert!(accepted.contains("status: active"));
    assert!(accepted.contains("confirmed_at:"));

    let recall = project
        .ctx()
        .args([
            "hook",
            "recall",
            "hook recall promotion",
            "--budget",
            "64",
            "--cwd",
        ])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recall = String::from_utf8(recall.clone()).unwrap();
    assert!(recall.contains("COMPACT_SUMMARY_MODE"));
    assert!(recall.contains("<ctx-memory layer=\\\"l1\\\">"));
    assert!(recall.contains("l2_scene_briefs"));
    assert!(first_toon_usize(recall.as_bytes(), "estimated_tokens") <= 64);

    let tight = project
        .ctx()
        .args([
            "hook",
            "recall",
            "hook recall promotion",
            "--budget",
            "4",
            "--cwd",
        ])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(first_toon_usize(&tight, "estimated_tokens") <= 4);

    let rejected = project
        .ctx()
        .arg("remember")
        .arg("Rejected candidate uses REJECTED_MODE.")
        .args([
            "--kind",
            "fact",
            "--subject",
            "hook.reject",
            "--suggested",
            "--cwd",
        ])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rejected_id = first_toon_id(&rejected);
    project
        .ctx()
        .args(["memory", "reject", &rejected_id, "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

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
    assert!(!review.contains("REJECTED_MODE"));
}

#[test]
fn hook_ingest_stores_redacted_event_without_project_init() {
    let project = TestProject::new();

    let output = project
        .ctx()
        .args([
            "hook",
            "ingest",
            "--host",
            "codex",
            "--event",
            "PostToolUse",
            "--session-key",
            "thread-123",
            "--cwd",
        ])
        .arg(project.root.path())
        .write_stdin(r#"{"tool":"shell","api_key":"sk-test-secret","message":"hello hook"}"#)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("hook ingest"));
    assert!(output.contains("queued_jobs: 0"));

    let conn = Connection::open(project.home.path().join("ctx.db")).unwrap();
    let row: (String, String, String, String, String) = conn
        .query_row(
            "SELECT host, event_name, project_root, session_key, payload_json FROM hook_events",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row.0, "codex");
    assert_eq!(row.1, "PostToolUse");
    assert_eq!(
        row.2,
        project
            .root
            .path()
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(row.3, "thread-123");
    assert!(row.4.contains("[redacted]"));
    assert!(!row.4.contains("sk-test-secret"));

    let session_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM agent_sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(session_count, 1);
}

#[test]
fn memory_job_commands_claim_prompt_and_apply_without_background_execution() {
    let project = TestProject::new();
    let schema = r#"{"type":"object","required":["candidates"],"properties":{"candidates":{"type":"array"}}}"#;

    let enqueue = project
        .ctx()
        .args([
            "memory",
            "job",
            "enqueue",
            "--kind",
            "l1_extract",
            "--objective",
            "Extract memory candidates from hook evidence",
            "--evidence-json",
            r#"[{"event_id":"evt1","summary":"Prefer cargo test for Rust changes"}]"#,
            "--result-schema-json",
            schema,
            "--cwd",
        ])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let job_id = first_toon_id(&enqueue);

    let claimed = project
        .ctx()
        .args([
            "memory",
            "job",
            "next",
            "--lease-owner",
            "visible-agent",
            "--cwd",
        ])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let claimed = String::from_utf8(claimed).unwrap();
    assert!(claimed.contains("status: running"));
    assert!(claimed.contains("lease_owner: \"visible-agent\""));
    assert!(claimed.contains("attempts: 1"));

    let prompt = project
        .ctx()
        .args(["memory", "job", "prompt", &job_id, "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let prompt = String::from_utf8(prompt).unwrap();
    assert!(prompt.contains("Return only JSON"));
    assert!(prompt.contains("Do not call a background harness or provider endpoint"));
    assert!(prompt.contains("Prefer cargo test"));

    let result_path = project.root.path().join("job-result.json");
    fs::write(
        &result_path,
        r#"{"candidates":[{"content":"Prefer cargo test for Rust changes"}]}"#,
    )
    .unwrap();
    let applied = project
        .ctx()
        .args(["memory", "job", "apply", &job_id])
        .arg(&result_path)
        .args(["--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let applied = String::from_utf8(applied).unwrap();
    assert!(applied.contains("status: done"));
    assert!(applied.contains("Prefer cargo test"));

    let empty = project
        .ctx()
        .args(["memory", "job", "next", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let empty = String::from_utf8(empty).unwrap();
    assert!(empty.contains("job: null"));

    project
        .ctx()
        .args([
            "memory",
            "job",
            "enqueue",
            "--kind",
            "l1_extract",
            "--objective",
            "Return queued work to the visible harness",
            "--result-schema-json",
            schema,
            "--cwd",
        ])
        .arg(project.root.path())
        .assert()
        .success();
    let process = project
        .ctx()
        .args(["memory", "process", "--lease-owner", "codex", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let process = String::from_utf8(process).unwrap();
    assert!(process.contains("mode: visible_harness"));
    assert!(process.contains("background: false"));
    assert!(process.contains("ctx memory job apply"));
    assert!(process.contains("Return only JSON"));
}

#[test]
fn l1_enqueue_and_apply_stores_review_gated_deduped_memory() {
    let project = TestProject::new();

    let hook = project
        .ctx()
        .args([
            "hook",
            "ingest",
            "--host",
            "codex",
            "--event",
            "UserMessage",
            "--session-key",
            "thread-l1",
            "--cwd",
        ])
        .arg(project.root.path())
        .write_stdin(
            r#"{"role":"user","content":"I prefer terse Rust test summaries.","note":"L1 source evidence"}"#,
        )
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let event_id = first_toon_field(&hook, "event_id");

    project
        .ctx()
        .arg("remember")
        .arg("Active duplicate memory carries CTX_L1_DUPLICATE.")
        .args(["--kind", "fact", "--subject", "l1.duplicate", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success();

    let enqueue = project
        .ctx()
        .args(["memory", "l1", "enqueue", "--limit", "10", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let job_id = first_toon_id(&enqueue);
    let enqueue = String::from_utf8(enqueue).unwrap();
    assert!(enqueue.contains("kind: l1_extract"));
    assert!(enqueue.contains("evidence_count: 1"));

    let prompt = project
        .ctx()
        .args(["memory", "job", "prompt", &job_id, "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let prompt = String::from_utf8(prompt).unwrap();
    assert!(prompt.contains("Extract only durable, reusable memories"));
    assert!(prompt.contains("source_event_ids"));
    assert!(prompt.contains(&event_id));

    let result_path = project.root.path().join("l1-result.json");
    fs::write(
        &result_path,
        format!(
            r#"{{
              "scene_name":"Rust test workflow",
              "candidates":[
                {{
                  "kind":"preference",
                  "subject":"testing.rust",
                  "trigger":"Rust changes",
                  "content":"I prefer terse Rust test summaries.",
                  "tags":["rust"],
                  "source_event_ids":["{event_id}"]
                }},
                {{
                  "kind":"fact",
                  "subject":"l1.duplicate",
                  "content":"Active duplicate memory carries CTX_L1_DUPLICATE.",
                  "source_event_ids":["{event_id}"]
                }}
              ]
            }}"#
        ),
    )
    .unwrap();

    let applied = project
        .ctx()
        .args(["memory", "job", "apply", &job_id])
        .arg(&result_path)
        .args(["--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let applied = String::from_utf8(applied).unwrap();
    assert!(applied.contains("stored_count: 1"));
    assert!(applied.contains("skipped_count: 1"));
    assert!(applied.contains("exact_active_duplicate"));
    assert!(applied.contains("testing.rust"));

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
    assert!(review.contains("I prefer terse Rust test summaries."));
    assert!(review.contains("suggested"));
    assert!(!review.contains("CTX_L1_DUPLICATE"));

    let recall = project
        .ctx()
        .args(["recall", "terse Rust test summaries", "--cwd"])
        .arg(project.root.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recall = String::from_utf8(recall).unwrap();
    assert!(!recall.contains("I prefer terse Rust test summaries."));

    let conn = Connection::open(project.home.path().join("ctx.db")).unwrap();
    let evidence_count: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM memory_evidence e
             JOIN memories m ON m.id = e.memory_id
             WHERE m.subject = 'testing.rust' AND e.source_id = ?1",
            [&event_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(evidence_count, 1);
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
    project
        .ctx()
        .args(["install", "--bin-dir"])
        .arg(&bin_dir)
        .assert()
        .success();
    assert!(bin_dir.join("ctx").exists());
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
