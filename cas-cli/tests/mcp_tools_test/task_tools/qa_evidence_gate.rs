//! cas-0cd5: the implementer's QA evidence bundle is required at close for
//! user-facing deliveries, and added test skip markers are refused.
//!
//! Drives the real MCP close: a user-facing delivery on `factory/test-agent`
//! is refused without a bundle, parks for merge with a valid one, is refused
//! again once a later commit makes the bundle stale, and docs-only deliveries
//! stay ungated even with a demo_statement.

use crate::support::*;
use cas::mcp::CasCore;
use cas::mcp::tools::*;
use cas::store::open_task_store;
use cas::types::TaskStatus;
use rmcp::handler::server::wrapper::Parameters;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const TASK: &str = "cas-ev01";

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "CAS Test")
        .env("GIT_AUTHOR_EMAIL", "cas@example.test")
        .env("GIT_COMMITTER_NAME", "CAS Test")
        .env("GIT_COMMITTER_EMAIL", "cas@example.test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Commit now. The delivery commits must fall inside the task's work cycle,
/// and a bundle written afterwards is at least as fresh as the commit.
fn commit_file(repo: &Path, path: &str, body: &str) -> String {
    let full = repo.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, body).unwrap();
    git(repo, &["add", path]);
    git(repo, &["commit", "-q", "-m", path]);
    git(repo, &["rev-parse", "HEAD"])
}

fn close_req(id: &str) -> TaskCloseRequest {
    TaskCloseRequest {
        stranded_branch_override: None,
        id: id.to_string(),
        reason: Some("delivered".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    }
}

async fn close_text(core: &CasCore, id: &str) -> String {
    match core.cas_task_close(Parameters(close_req(id))).await {
        Ok(result) => extract_text(result),
        Err(error) => error.message.to_string(),
    }
}

struct Fx {
    _temp: tempfile::TempDir,
    core: CasCore,
    repo: PathBuf,
    artifacts: PathBuf,
}

/// Repo whose `factory/test-agent` delivers `delivered` (path, body), for a
/// task with the given demo_statement. The fixture core is the implementer.
fn fixture(delivered: &[(&str, &str)], demo: &str) -> Fx {
    let (temp, core) = setup_cas();
    let repo = temp.path().to_path_buf();
    let artifacts = repo.join("artifacts");
    std::fs::write(
        repo.join(".cas").join("config.toml"),
        format!(
            "[factory]\nartifacts_root = {:?}\n[verification]\nenabled = false\n[qa]\nindependent_pass = false\n",
            artifacts.display().to_string()
        ),
    )
    .unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    commit_file(&repo, "README.md", "seed\n");
    git(&repo, &["checkout", "-q", "-b", "factory/test-agent"]);
    for (path, body) in delivered {
        commit_file(&repo, path, body);
    }
    let tasks = open_task_store(&repo.join(".cas")).unwrap();
    let mut task = cas::types::Task::new(TASK.to_string(), "Composer spacing".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("test-agent".to_string());
    task.demo_statement = demo.to_string();
    tasks.add(&task).unwrap();
    Fx {
        _temp: temp,
        core,
        repo,
        artifacts,
    }
}

impl Fx {
    fn status(&self) -> TaskStatus {
        open_task_store(&self.repo.join(".cas"))
            .unwrap()
            .get(TASK)
            .unwrap()
            .status
    }

    fn notes(&self) -> String {
        open_task_store(&self.repo.join(".cas"))
            .unwrap()
            .get(TASK)
            .unwrap()
            .notes
    }

    /// A complete contract-v1 bundle for `head`, cited by a platform_proof note.
    fn write_bundle(&self, head: &str) -> PathBuf {
        let dir = self.artifacts.join(TASK).join("qa");
        std::fs::create_dir_all(dir.join("visual-qa")).unwrap();
        let file = std::fs::File::create(dir.join("trace.zip")).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("test.trace", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(
            concat!(
                r#"{"type":"before","callId":"expect@1","method":"expect","title":"Expect \"toHaveText\""}"#,
                "\n",
                r#"{"type":"after","callId":"expect@1","endTime":2}"#
            )
            .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap();
        for (name, body) in [
            (
                "trace-actions.txt",
                "  1. 0:00.1  Expect \"toHaveText\"  2ms\n",
            ),
            ("receipt.webm", "webm"),
            ("final.aria.yml", "- heading"),
            ("final.aria.json", "{}"),
            ("M01.png", "png"),
            ("visual-qa/app-light-desktop.png", "png"),
            ("visual-qa/app-light-phone.png", "png"),
            ("visual-qa/app-dark-desktop.png", "png"),
            ("visual-qa/app-dark-phone.png", "png"),
            ("visual-qa/visual-qa.md", "# Visual QA — PASS\n"),
            ("visual-qa/visual-qa.json", "{}"),
            ("visual-qa.stdout", "PASS\n"),
            ("critique.md", "Scored by test-agent\n"),
        ] {
            std::fs::write(dir.join(name), body).unwrap();
        }
        let manifest = serde_json::json!({
            "schema": 1, "task_id": TASK, "producer": "cas-qa-craft", "head_sha": head,
            "created_at": chrono::Utc::now().to_rfc3339(), "visual_change": false,
            "visual_qa_status": "pass",
            "files": {
                "trace": "trace.zip", "trace_actions": "trace-actions.txt", "receipt": "receipt.webm",
                "aria_yaml": "final.aria.yml", "aria_json": "final.aria.json", "cells": ["M01.png"], "a11y": [],
                "polish_screenshots": ["visual-qa/app-light-desktop.png", "visual-qa/app-light-phone.png",
                                       "visual-qa/app-dark-desktop.png", "visual-qa/app-dark-phone.png"],
                "visual_qa": "visual-qa/visual-qa.md", "visual_qa_json": "visual-qa/visual-qa.json",
                "visual_qa_stdout": "visual-qa.stdout", "critique": "critique.md"
            },
            "critique_score": {"distinctiveness": 4, "fit": 4, "hierarchy": 4, "craft": 4, "accessibility": 4}
        });
        let path = dir.join("bundle.json");
        std::fs::write(&path, manifest.to_string()).unwrap();
        let tasks = open_task_store(&self.repo.join(".cas")).unwrap();
        let mut task = tasks.get(TASK).unwrap();
        task.notes = format!(
            "{}\n[now] 🧪 PLATFORM_PROOF qa-bundle: {}",
            task.notes,
            path.display()
        );
        tasks.update(&task).unwrap();
        path
    }
}

#[tokio::test]
async fn user_facing_close_without_a_bundle_is_rejected_with_the_next_command() {
    let fx = fixture(
        &[("web/composer.css", ".composer{gap:8px}\n")],
        "Open the composer; spacing is even",
    );
    let _env = env_test_lock();

    let refused = close_text(&fx.core, TASK).await;
    assert!(
        refused.starts_with("TASK CLOSE REJECTED: cas-ev01 is user-facing ("),
        "{refused}"
    );
    assert!(refused.contains("path:web/composer.css"), "{refused}");
    assert!(refused.contains("demo_statement"), "{refused}");
    assert!(
        refused.contains("QA evidence bundle is not cited"),
        "{refused}"
    );
    assert!(refused.contains("note_type=platform_proof"), "{refused}");
    assert!(refused.contains("qa-bundle:"), "{refused}");
    assert!(
        !refused.contains("MERGE REQUIRED"),
        "must refuse before the park: {refused}"
    );
    assert_eq!(
        fx.status(),
        TaskStatus::InProgress,
        "an unevidenced delivery must not park"
    );
}

#[tokio::test]
async fn valid_bundle_lets_the_delivery_park_and_a_later_commit_makes_it_stale() {
    let fx = fixture(
        &[("web/composer.css", ".composer{gap:8px}\n")],
        "Open the composer; spacing is even",
    );
    let _env = env_test_lock();
    let head = git(&fx.repo, &["rev-parse", "factory/test-agent"]);
    let bundle = fx.write_bundle(&head);

    let parked = close_text(&fx.core, TASK).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert_eq!(fx.status(), TaskStatus::AwaitingMerge);
    assert!(
        fx.notes().contains(&format!(
            "QA evidence bundle accepted: {}",
            bundle.canonicalize().unwrap().display()
        )),
        "{}",
        fx.notes()
    );

    // The implementer pushes a fix after recording evidence: the bundle no
    // longer describes the delivery.
    let later = commit_file(&fx.repo, "web/composer.css", ".composer{gap:12px}\n");
    let stale = close_text(&fx.core, TASK).await;
    assert!(
        stale.starts_with("TASK CLOSE REJECTED: cas-ev01 is user-facing"),
        "{stale}"
    );
    assert!(stale.contains("QA evidence bundle is stale"), "{stale}");
    assert!(
        stale.contains(&later[..8]),
        "the refusal names the new head: {stale}"
    );
}

#[tokio::test]
async fn docs_only_delivery_with_a_demo_statement_is_not_gated() {
    let fx = fixture(&[("docs/guide.md", "# Guide\n")], "Read the guide");
    let _env = env_test_lock();

    let parked = close_text(&fx.core, TASK).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert!(!parked.contains("QA evidence"), "{parked}");
}

#[tokio::test]
async fn demo_only_non_web_delivery_needs_the_evidence_ledger() {
    let fx = fixture(
        &[("cas-cli/src/report.rs", "pub fn report() {}\n")],
        "Run cas report; it prints totals",
    );
    let _env = env_test_lock();

    let refused = close_text(&fx.core, TASK).await;
    assert!(
        refused.contains("is user-facing (demo_statement)"),
        "{refused}"
    );
    assert!(
        refused.contains("QA evidence ledger is missing"),
        "{refused}"
    );

    let ledger = fx.artifacts.join(TASK).join("LEDGER.md");
    std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    std::fs::write(
        &ledger,
        "| M01 | cas report | totals | totals | PASS | real-build | qa/M01.txt | - |\n",
    )
    .unwrap();
    let parked = close_text(&fx.core, TASK).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
}

#[tokio::test]
async fn healer_style_fixme_is_refused_even_on_a_test_only_delivery() {
    let fx = fixture(
        &[(
            "hub-web/e2e/generated/fleet.spec.ts",
            "import { test } from \"@playwright/test\";\ntest(\"fleet\", async () => {\n  test.fixme(true, \"verdict disagrees\");\n});\n",
        )],
        "",
    );
    let _env = env_test_lock();

    let refused = close_text(&fx.core, TASK).await;
    assert!(
        refused.contains("adds test skip/focus markers without a stated reason"),
        "{refused}"
    );
    assert!(
        refused.contains("hub-web/e2e/generated/fleet.spec.ts:3 `test.fixme`"),
        "{refused}"
    );
    assert!(refused.contains("cas-allow-skip:"), "{refused}");
    assert_eq!(fx.status(), TaskStatus::InProgress);

    // A stated reason turns the marker into a recorded decision.
    commit_file(
        &fx.repo,
        "hub-web/e2e/generated/fleet.spec.ts",
        "import { test } from \"@playwright/test\";\ntest(\"fleet\", async () => {\n  // cas-allow-skip: WebKit cannot grant clipboard in CI\n  test.fixme(true, \"clipboard\");\n});\n",
    );
    let parked = close_text(&fx.core, TASK).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert!(
        fx.notes().contains("Allowed skip marker hub-web/e2e/generated/fleet.spec.ts:4 `test.fixme` — WebKit cannot grant clipboard in CI"),
        "{}",
        fx.notes()
    );
}

#[tokio::test]
async fn evidence_gate_can_be_disabled_per_project() {
    let fx = fixture(
        &[("web/composer.css", ".composer{gap:8px}\n")],
        "Open the composer",
    );
    let _env = env_test_lock();
    let config = fx.repo.join(".cas").join("config.toml");
    let body = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        body.replace("[qa]\n", "[qa]\nevidence_gate = false\n"),
    )
    .unwrap();

    let parked = close_text(&fx.core, TASK).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
}

#[tokio::test]
async fn demo_only_terminal_rendering_change_also_needs_a_terminal_qa_receipt() {
    let fx = fixture(
        &[("cas-cli/src/ui/status.rs", "pub fn draw() {}\n")],
        "Run cas status; the table fits 80 columns",
    );
    let _env = env_test_lock();
    let task_dir = fx.artifacts.join(TASK);
    std::fs::create_dir_all(&task_dir).unwrap();
    std::fs::write(
        task_dir.join("LEDGER.md"),
        "| M01 | cas status | fits | fits | PASS | real-build | qa/M01.txt | - |\n",
    )
    .unwrap();

    let refused = close_text(&fx.core, TASK).await;
    assert!(
        refused.contains("terminal-qa receipt is missing"),
        "{refused}"
    );
    assert!(refused.contains("scripts/terminal-qa.mjs"), "{refused}");

    let report = task_dir.join("terminal-qa/cas-status/report.md");
    std::fs::create_dir_all(report.parent().unwrap()).unwrap();
    std::fs::write(
        &report,
        "terminal-qa: PASS cas-status · 14 runs · 0 fail · 0 warn\n",
    )
    .unwrap();
    let parked = close_text(&fx.core, TASK).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
}

#[tokio::test]
async fn supervisor_override_waives_the_gate_with_a_logged_decision() {
    let fx = fixture(
        &[("web/composer.css", ".composer{gap:8px}\n")],
        "Open the composer",
    );
    let _env = env_test_lock();
    let cas_dir = fx.repo.join(".cas");
    let id = format!("supervisor-session-{}", std::process::id());
    cas::store::open_agent_store(&cas_dir)
        .unwrap()
        .register(&cas::types::Agent::new_with_role(
            id.clone(),
            "fixture-supervisor".to_string(),
            cas::types::AgentRole::Supervisor,
        ))
        .unwrap();
    let supervisor = CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing(id);

    let mut request = close_req(TASK);
    request.supervisor_override = Some(true);
    request.reason = Some("copy-only change reviewed live with the operator".to_string());
    let text = match supervisor.cas_task_close(Parameters(request)).await {
        Ok(result) => extract_text(result),
        Err(error) => error.message.to_string(),
    };
    assert!(
        !text.contains("TASK CLOSE REJECTED: cas-ev01 is user-facing"),
        "{text}"
    );
    let notes = fx.notes();
    assert!(
        notes.contains("✅ DECISION QA evidence gate waived by supervisor override: copy-only change reviewed live with the operator"),
        "{notes}"
    );
    assert!(
        notes.contains("QA evidence bundle is not cited"),
        "the waived refusal is recorded: {notes}"
    );
}
