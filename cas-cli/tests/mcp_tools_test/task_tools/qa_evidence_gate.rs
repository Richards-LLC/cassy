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
fn fixture(test_env: &mut TestEnvGuard, delivered: &[(&str, &str)], demo: &str) -> Fx {
    let (temp, core) = setup_cas(test_env);
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
            ("visual-qa.stdout", "PASS\n"),
            ("critique.md", "Scored by test-agent\n"),
        ] {
            std::fs::write(dir.join(name), body).unwrap();
        }
        // The strict run's own report, of a local build (cas-a6a3).
        std::fs::write(
            dir.join("visual-qa/visual-qa.json"),
            serde_json::json!({
                "status": "PASS", "strict": true,
                "generatedAt": chrono::Utc::now().to_rfc3339(),
                "urls": ["http://127.0.0.1:4173/"]
            })
            .to_string(),
        )
        .unwrap();
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

/// The cas-8cfe incident had a commit-time anchor at A, a clean delivery at B,
/// and a fresh B bundle before the first park. Exercise the public close path
/// both with the incident's explicit receipt and with branch discovery.
#[tokio::test]
async fn first_park_binds_fresh_bundle_and_receipt_to_current_tip_cas_8cfe() {
    for explicit_receipt in [false, true] {
        let mut test_env = TestEnvGuard::temp_home();
        let fx = fixture(
            &mut test_env,
            &[("web/composer.css", ".composer{gap:8px}\n")],
            "Open the composer; spacing is even",
        );
        let cas_dir = fx.repo.join(".cas");
        let config = cas_dir.join("config.toml");
        let body = std::fs::read_to_string(&config).unwrap();
        std::fs::write(
            &config,
            body.replace("independent_pass = false", "independent_pass = true"),
        )
        .unwrap();
        let branch = format!("factory/test-agent-{TASK}");
        git(&fx.repo, &["checkout", "-q", "-b", &branch]);
        std::fs::write(fx.repo.join(".git/info/exclude"), "/.cas/\n/artifacts/\n").unwrap();
        let old_head = git(&fx.repo, &["rev-parse", "HEAD"]);
        let tasks = open_task_store(&cas_dir).unwrap();
        let mut task = tasks.get(TASK).unwrap();
        task.deliverables.factory_branch_anchor = Some(old_head.clone());
        tasks.update(&task).unwrap();
        fx.write_bundle(&old_head);
        let head = commit_file(&fx.repo, "web/composer.css", ".composer{gap:12px}\n");
        assert_ne!(head, old_head);
        assert!(git(&fx.repo, &["status", "--porcelain"]).is_empty());

        let request = || {
            let mut request = close_req(TASK);
            if explicit_receipt {
                request.commit_receipt = Some(head.clone());
            }
            request
        };
        let stale = extract_text(fx.core.cas_task_close(Parameters(request())).await.unwrap());
        assert!(stale.contains("QA evidence bundle is stale"), "{stale}");
        assert_eq!(fx.status(), TaskStatus::InProgress);
        assert!(
            cas_store::list_qa_passes(&cas_dir, TASK)
                .unwrap()
                .is_empty()
        );

        let bundle = fx.write_bundle(&head);
        let parked = extract_text(fx.core.cas_task_close(Parameters(request())).await.unwrap());
        assert!(parked.contains("MERGE REQUIRED"), "{parked}");
        assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
        assert!(fx.notes().contains(&format!(
            "QA evidence bundle accepted: {}",
            bundle.canonicalize().unwrap().display()
        )));
        let stored = tasks.get(TASK).unwrap();
        assert_eq!(stored.status, TaskStatus::AwaitingMerge);
        assert_eq!(
            stored.deliverables.factory_branch_anchor.as_deref(),
            Some(head.as_str())
        );
        assert_eq!(
            stored.deliverables.parked_branch.as_deref(),
            Some(branch.as_str())
        );
        let passes = cas_store::list_qa_passes(&cas_dir, TASK).unwrap();
        assert_eq!(passes.len(), 1);
        let pass = &passes[0];
        assert_eq!(
            pass.bound_head, head,
            "fresh evidence must dispatch the current bytes"
        );
        let qa = tasks.get(pass.qa_task_id.as_deref().unwrap()).unwrap();
        assert!(qa.description.contains(&head), "{}", qa.description);
        let refusal = cas::qa_pass::branch_merge_refusal(&cas_dir, &fx.repo, &branch)
            .expect("the newly dispatched current tip still needs independent approval");
        assert!(refusal.contains(&head[..8]), "{refusal}");
    }
}

/// An older approval must not authorize a later clean tip, even after the
/// implementer replaces its evidence with a fresh commit-bound bundle.
#[tokio::test]
async fn refreshed_bundle_dispatches_current_tip_without_reusing_old_approval_cas_8cfe() {
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(
        &mut test_env,
        &[("web/composer.css", ".composer{gap:8px}\n")],
        "Open the composer; spacing is even",
    );
    let cas_dir = fx.repo.join(".cas");
    let config = cas_dir.join("config.toml");
    let body = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        body.replace("independent_pass = false", "independent_pass = true"),
    )
    .unwrap();
    let branch = format!("factory/test-agent-{TASK}");
    git(&fx.repo, &["checkout", "-q", "-b", &branch]);
    std::fs::write(fx.repo.join(".git/info/exclude"), "/.cas/\n/artifacts/\n").unwrap();
    let old_head = git(&fx.repo, &["rev-parse", "HEAD"]);
    fx.write_bundle(&old_head);
    let parked = close_text(&fx.core, TASK).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    // Seed the prior independent verdict through the durable store; the
    // behavior under test is subsequent MCP close and the real merge guard.
    cas_store::claim_qa_pass(&cas_dir, TASK, "reviewer", chrono::Utc::now()).unwrap();
    let approved = cas_store::resolve_qa_pass(
        &cas_dir,
        TASK,
        "reviewer",
        cas::types::QaVerdict::Approved,
        "the previous bytes passed",
        None,
        fx.artifacts
            .join(TASK)
            .join("old-review/LEDGER.md")
            .to_str()
            .unwrap(),
        chrono::Utc::now(),
    )
    .unwrap();
    assert_eq!(approved.bound_head, old_head);
    assert!(cas::qa_pass::branch_merge_refusal(&cas_dir, &fx.repo, &branch).is_none());

    let head = commit_file(&fx.repo, "web/composer.css", ".composer{gap:12px}\n");
    fx.write_bundle(&head);
    assert!(git(&fx.repo, &["status", "--porcelain"]).is_empty());
    let before = cas::qa_pass::branch_merge_refusal(&cas_dir, &fx.repo, &branch)
        .expect("approval of A cannot authorize B before re-close");
    assert!(before.contains(&head[..8]), "{before}");
    let mut request = close_req(TASK);
    request.commit_receipt = Some(head.clone());
    let refreshed = extract_text(fx.core.cas_task_close(Parameters(request)).await.unwrap());
    assert!(
        refreshed.contains("INDEPENDENT QA DISPATCHED"),
        "{refreshed}"
    );
    let current = cas_store::latest_qa_pass(&cas_dir, TASK, chrono::Utc::now())
        .unwrap()
        .unwrap();
    assert_ne!(current.id, approved.id);
    assert_eq!(current.bound_head, head);
    assert_eq!(current.state, cas::types::QaPassState::Pending);
    let tasks = open_task_store(&cas_dir).unwrap();
    assert_eq!(
        tasks
            .get(TASK)
            .unwrap()
            .deliverables
            .factory_branch_anchor
            .as_deref(),
        Some(head.as_str())
    );
    let after = cas::qa_pass::branch_merge_refusal(&cas_dir, &fx.repo, &branch)
        .expect("a fresh implementer bundle cannot substitute for approval of B");
    assert!(after.contains(&head[..8]), "{after}");
    let passes = cas_store::list_qa_passes(&cas_dir, TASK).unwrap();
    let prior = passes.iter().find(|pass| pass.id == approved.id).unwrap();
    assert_eq!(prior.state, cas::types::QaPassState::Passed);
    assert_eq!(
        prior.bound_head, old_head,
        "prior verdict remains immutable"
    );
}

#[tokio::test]
async fn user_facing_close_without_a_bundle_is_rejected_with_the_next_command() {
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(&mut test_env,
        &[("web/composer.css", ".composer{gap:8px}\n")],
        "Open the composer; spacing is even",
    );

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
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(&mut test_env,
        &[("web/composer.css", ".composer{gap:8px}\n")],
        "Open the composer; spacing is even",
    );
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
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(&mut test_env, &[("docs/guide.md", "# Guide\n")], "Read the guide");

    let parked = close_text(&fx.core, TASK).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert!(!parked.contains("QA evidence"), "{parked}");
}

#[tokio::test]
async fn demo_only_non_web_delivery_needs_the_evidence_ledger() {
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(&mut test_env,
        &[("cas-cli/src/report.rs", "pub fn report() {}\n")],
        "Run cas report; it prints totals",
    );

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
async fn demo_only_ledger_accepts_a_row_without_outer_pipes() {
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(&mut test_env,
        &[("cas-cli/src/report.rs", "pub fn report() {}\n")],
        "Run cas report; it prints totals",
    );
    let ledger = fx.artifacts.join(TASK).join("LEDGER.md");
    std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    std::fs::write(
        &ledger,
        "M01 | cas report | totals | totals | PASS | real-build | qa/M01.txt | -\n",
    )
    .unwrap();

    let parked = close_text(&fx.core, TASK).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert!(
        fx.notes().contains("QA evidence ledger accepted"),
        "{}",
        fx.notes()
    );
}

#[tokio::test]
async fn healer_style_fixme_is_refused_even_on_a_test_only_delivery() {
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(&mut test_env,
        &[(
            "hub-web/e2e/generated/fleet.spec.ts",
            "import { test } from \"@playwright/test\";\ntest(\"fleet\", async () => {\n  test.fixme(true, \"verdict disagrees\");\n});\n",
        )],
        "",
    );

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
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(&mut test_env,
        &[("web/composer.css", ".composer{gap:8px}\n")],
        "Open the composer",
    );
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
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(&mut test_env,
        &[("cas-cli/src/ui/status.rs", "pub fn draw() {}\n")],
        "Run cas status; the table fits 80 columns",
    );
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
async fn factory_input_and_pty_geometry_accept_real_build_ledger_cas_266e() {
    let mut test_env = TestEnvGuard::temp_home();
    // Historical input and winsize changes share these mixed-purpose files;
    // a stdout capture cannot exercise their interactive pane transitions.
    for paths in [
        vec!["cas-cli/src/ui/factory/app/sidecar_and_selection.rs", "cas-cli/src/ui/factory/daemon/runtime/client_input.rs", "crates/cas-mux/src/pane/mod.rs"],
        vec!["cas-cli/src/ui/factory/app/mod.rs", "cas-cli/src/ui/factory/daemon/runtime/output.rs", "crates/cas-pty/src/pty.rs"],
    ] {
        let delivered: Vec<_> = paths.iter().map(|path| (*path, "pub fn interaction() {}\n")).collect();
        let fx = fixture(&mut test_env, &delivered, "Run the real factory; click and resize a pane");
        let task_dir = fx.artifacts.join(TASK);
        std::fs::create_dir_all(&task_dir).unwrap();
        let ledger = task_dir.join("LEDGER.md");
        std::fs::write(&ledger, "| M01 | pane | forwards | forwards | PASS | fixture | capture.txt | - |\n").unwrap();
        let refused = close_text(&fx.core, TASK).await;
        assert!(refused.contains("no row with verdict PASS and label real-build"), "{refused}");
        std::fs::write(&ledger, "| M01 | pane | forwards | forwards | PASS | real-build | capture.txt | - |\n").unwrap();
        let parked = close_text(&fx.core, TASK).await;
        assert!(parked.contains("MERGE REQUIRED"), "{parked}");
        assert!(fx.notes().contains("QA evidence ledger accepted"));
        assert!(!fx.notes().contains("terminal-qa receipt accepted"));
    }
}

#[tokio::test]
async fn mixed_factory_and_cli_output_still_requires_terminal_qa_cas_266e() {
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(&mut test_env, &[
        ("cas-cli/src/ui/factory/daemon/runtime/client_input.rs", "pub fn click() {}\n"),
        ("cas-cli/src/cli/status.rs", "pub fn status() { println!(\"Ready\"); }\n"),
    ], "Run the factory and cas status");
    let task_dir = fx.artifacts.join(TASK);
    std::fs::create_dir_all(&task_dir).unwrap();
    std::fs::write(task_dir.join("LEDGER.md"), "| M01 | status | Ready | Ready | PASS | real-build | capture.txt | - |\n").unwrap();
    let refused = close_text(&fx.core, TASK).await;
    assert!(refused.contains("terminal-qa receipt is missing"), "{refused}");
    let report = task_dir.join("terminal-qa/cas-status/report.md");
    std::fs::create_dir_all(report.parent().unwrap()).unwrap();
    std::fs::write(&report, "terminal-qa: PASS cas-status · 11 runs · 0 fail\n").unwrap();
    assert!(close_text(&fx.core, TASK).await.contains("MERGE REQUIRED"));
}

#[tokio::test]
async fn supervisor_override_waives_the_gate_with_a_logged_decision() {
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(&mut test_env,
        &[("web/composer.css", ".composer{gap:8px}\n")],
        "Open the composer",
    );
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

/// cas-ba4a: start the worker's next task on the same factory lane — a lease
/// claimed now, status `status`, so the in-progress-sibling guard does not
/// apply — then commit that task's untagged work on top of the lane.
fn start_next_task_on_the_lane(fx: &Fx, id: &str, status: TaskStatus, path: &str) -> String {
    let cas_dir = fx.repo.join(".cas");
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut next = cas::types::Task::new(id.to_string(), "next task on the same lane".to_string());
    next.status = status;
    next.assignee = Some("test-agent".to_string());
    tasks.add(&next).unwrap();
    cas::store::open_agent_store(&cas_dir)
        .unwrap()
        .try_claim(id, &format!("test-session-{}", std::process::id()), 600, Some("start"))
        .unwrap();
    commit_file(&fx.repo, path, "// next task\n")
}

fn anchor(fx: &Fx) -> Option<String> {
    open_task_store(&fx.repo.join(".cas"))
        .unwrap()
        .get(TASK)
        .unwrap()
        .deliverables
        .factory_branch_anchor
}

fn merge_into_main(fx: &Fx, commit: &str) {
    git(&fx.repo, &["checkout", "-q", "main"]);
    git(&fx.repo, &["merge", "-q", "--no-ff", "-m", "merge parked delivery", commit]);
    git(&fx.repo, &["checkout", "-q", "factory/test-agent"]);
}

/// GH #1144: the receipt is the batch's squash; the parked anchor still
/// identifies this task's delivery, including when a failed re-close blocked it.
#[tokio::test]
async fn batch_squash_receipt_keeps_docs_only_qa_scope_cas_e5b1() {
    for status in [TaskStatus::AwaitingMerge, TaskStatus::Blocked] {
        let mut env = TestEnvGuard::temp_home();
        let fx = fixture(&mut env, &[("DESIGN.md", "Document the layout\n")], "");
        let head = git(&fx.repo, &["rev-parse", "HEAD"]);
        let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
        let mut task = tasks.get(TASK).unwrap();
        task.status = status;
        task.deliverables.factory_branch_anchor = Some(head.clone());
        task.deliverables.parked_branch = Some("factory/test-agent".into());
        tasks.update(&task).unwrap();

        git(&fx.repo, &["checkout", "-q", "-b", "batch/qa"]);
        commit_file(&fx.repo, "web/SiblingDrawer.vue", "<template>Sibling</template>\n");
        git(&fx.repo, &["checkout", "-q", "main"]);
        git(&fx.repo, &["merge", "--squash", "batch/qa"]);
        git(&fx.repo, &["commit", "-q", "-m", &format!("{TASK}: squash integration batch")]);
        let squash = git(&fx.repo, &["rev-parse", "HEAD"]);
        git(&fx.repo, &["checkout", "-q", "factory/test-agent"]);

        let mut request = close_req(TASK);
        request.commit_receipt = Some(squash);
        let text = extract_text(fx.core.cas_task_close(Parameters(request)).await.unwrap());
        assert_eq!(fx.status(), TaskStatus::Closed, "{status:?}: {text}");
        assert_eq!(anchor(&fx).as_deref(), Some(head.as_str()));
        assert!(!fx.notes().contains("QA evidence bundle accepted"), "{}", fx.notes());
    }
}

/// cas-ba4a: the worker parks A at X with a valid QA bundle, starts B on the
/// same lane and commits B's work. A pre-merge retry must not move A's anchor
/// onto B's commit, and once X merges A's plain re-close judges X — not the
/// lane tip — so it closes without an override.
#[tokio::test]
async fn parked_delivery_recloses_on_its_anchor_after_the_next_task_moves_the_lane_cas_ba4a() {
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(&mut test_env,
        &[("web/composer.css", ".composer{gap:8px}\n")],
        "Open the composer; spacing is even",
    );
    let parked_at = git(&fx.repo, &["rev-parse", "factory/test-agent"]);
    fx.write_bundle(&parked_at);
    let parked = close_text(&fx.core, TASK).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert_eq!(fx.status(), TaskStatus::AwaitingMerge);
    assert_eq!(anchor(&fx).as_deref(), Some(parked_at.as_str()));

    for (id, status) in [("cas-b0b1", TaskStatus::Open), ("cas-b0b2", TaskStatus::Blocked)] {
        let lane_tip =
            start_next_task_on_the_lane(&fx, id, status, &format!("web/next-{status:?}.css"));
        let retry = close_text(&fx.core, TASK).await;
        assert!(retry.contains("MERGE REQUIRED"), "{status:?}: {retry}");
        assert!(!retry.contains("stale"), "{status:?}: {retry}");
        assert_eq!(
            anchor(&fx).as_deref(),
            Some(parked_at.as_str()),
            "B ({status:?}) at {lane_tip} must not become A's delivery"
        );
    }

    merge_into_main(&fx, &parked_at);
    let closed = close_text(&fx.core, TASK).await;
    assert!(!closed.contains("QA evidence bundle is stale"), "{closed}");
    assert_eq!(fx.status(), TaskStatus::Closed, "{closed}");
}

/// cas-ba4a: blast-radius proof scope is measured on the parked anchor. The
/// next task's untagged commit on the same lane touches a module outside A's
/// proof targets; counting it (the old HEAD-based attribution) refused A's
/// re-close with "uncovered source modules".
#[tokio::test]
async fn parked_delivery_proof_scope_ignores_the_next_tasks_commits_cas_ba4a() {
    let mut test_env = TestEnvGuard::temp_home();
    let fx = fixture(&mut test_env, &[], "");
    let cas_dir = fx.repo.join(".cas");
    let agent_id = format!("test-session-{}", std::process::id());
    let agents = cas::store::open_agent_store(&cas_dir).unwrap();
    agents.try_claim(TASK, &agent_id, 600, Some("start A")).unwrap();
    let parked_at = commit_file(&fx.repo, "crates/widget/src/composer.rs", "pub fn composer() {}\n");
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(TASK).unwrap();
    task.risk = vec![cas::types::TaskRisk::BlastRadius];
    task.proof_targets = vec!["composer".to_string()];
    task.status = TaskStatus::AwaitingMerge;
    task.deliverables.factory_branch_anchor = Some(parked_at.clone());
    tasks.update(&task).unwrap();
    agents.release_lease(TASK, &agent_id).unwrap();

    start_next_task_on_the_lane(&fx, "cas-b0b3", TaskStatus::Blocked, "crates/widget/src/other.rs");
    merge_into_main(&fx, &parked_at);

    let closed = close_text(&fx.core, TASK).await;
    assert!(!closed.contains("uncovered source modules"), "{closed}");
    assert_eq!(fx.status(), TaskStatus::Closed, "{closed}");
}

fn cas_6f10_supervisor(fx: &Fx) -> CasCore {
    let cas_dir = fx.repo.join(".cas");
    let id = format!("cas-6f10-supervisor-{}", std::process::id());
    cas::store::open_agent_store(&cas_dir)
        .unwrap()
        .register(&cas::types::Agent::new_with_role(
            id.clone(),
            "deployed-owner".into(),
            cas::types::AgentRole::Supervisor,
        ))
        .unwrap();
    let core = CasCore::with_daemon(cas_dir, None, None);
    core.set_agent_id_for_testing(id);
    core
}

#[tokio::test]
async fn cas_6f10_exact_waiver_satisfies_worker_real_build_ledger_gate() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(
        &mut env,
        &[("src/feature.rs", "pub fn feature() {}\n")],
        "Verify deployed endpoint after batch release",
    );
    let head = git(&fx.repo, &["rev-parse", "HEAD"]);
    let tasks = open_task_store(&fx.repo.join(".cas")).unwrap();
    let mut task = tasks.get(TASK).unwrap();
    task.status = TaskStatus::AwaitingMerge;
    task.deliverables.factory_branch_anchor = Some(head.clone());
    tasks.update(&task).unwrap();
    let agents = cas::store::open_agent_store(&fx.repo.join(".cas")).unwrap();
    let worker_id = format!("test-session-{}", std::process::id());
    let mut worker = agents.get(&worker_id).unwrap();
    worker.role = cas::types::AgentRole::Worker;
    agents.update(&worker).unwrap();
    let dir = fx.artifacts.join(TASK);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("LEDGER.md"),
        "| M01 | endpoint | ok | mocked | PASS | fixture | qa/fixture.txt | - |\n",
    )
    .unwrap();
    assert!(
        close_text(&fx.core, TASK)
            .await
            .contains("label real-build")
    );
    let supervisor = cas_6f10_supervisor(&fx);
    let service = cas::mcp::CasService::new(supervisor, None);
    env.set("CAS_AGENT_ROLE", "supervisor");
    let waived = extract_text(service.verification(Parameters(serde_json::from_value(serde_json::json!({
        "action": "qa_waive", "task_id": TASK, "summary": "Supervisor owns deployed verification after the batch"
    })).unwrap())).await.unwrap());
    assert!(waived.contains("waived"), "{waived}");
    env.remove("CAS_AGENT_ROLE");
    merge_into_main(&fx, &head);
    let mut request = close_req(TASK);
    request.commit_receipt = Some(head.clone());
    let closed = extract_text(fx.core.cas_task_close(Parameters(request)).await.unwrap());
    assert!(closed.contains("Closed task:"), "{closed}");
    assert_eq!(fx.status(), TaskStatus::Closed);
    assert!(
        fx.notes().contains("QA evidence ledger waived"),
        "{}",
        fx.notes()
    );
    assert!(fx.notes().contains(&head));
}

#[tokio::test]
async fn cas_6f10_deferred_deployed_row_parks_with_named_obligation() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(
        &mut env,
        &[("src/feature.rs", "pub fn feature() {}\n")],
        "Verify deployed endpoint after batch release",
    );
    let _supervisor = cas_6f10_supervisor(&fx);
    let head = git(&fx.repo, &["rev-parse", "HEAD"]);
    let dir = fx.artifacts.join(TASK);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("LEDGER.md"), "| M01 | staging GET | served tip | pending deployment | DEFERRED | deployed-verification | deferred: deployed-verification owner=deployed-owner | - |\n").unwrap();
    let parked = close_text(&fx.core, TASK).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert_eq!(fx.status(), TaskStatus::AwaitingMerge);
    let notes = fx.notes();
    assert!(notes.contains("POST-DEPLOY OBLIGATION"), "{notes}");
    assert!(
        notes.contains("deployed-owner") && notes.contains(&head),
        "{notes}"
    );
}

#[tokio::test]
async fn cas_6f10_deferred_owner_must_be_registered_supervisor() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(
        &mut env,
        &[("src/feature.rs", "pub fn feature() {}\n")],
        "Verify deployed endpoint",
    );
    let dir = fx.artifacts.join(TASK);
    std::fs::create_dir_all(&dir).unwrap();
    for owner in ["unregistered", "test-agent"] {
        std::fs::write(dir.join("LEDGER.md"), format!("| M01 | staging GET | tip | pending | DEFERRED | deployed-verification | deferred: deployed-verification owner={owner} | - |\n")).unwrap();
        let refused = close_text(&fx.core, TASK).await;
        assert!(
            refused.contains("must identify one registered supervisor"),
            "{refused}"
        );
        assert_eq!(fx.status(), TaskStatus::InProgress);
        assert!(!fx.notes().contains("POST-DEPLOY OBLIGATION"));
    }
    let _supervisor = cas_6f10_supervisor(&fx);
    std::fs::write(dir.join("LEDGER.md"), "| M01 | staging GET | tip | pending | DEFERRED | deployed-verification | deferred: deployed-verification owner=deployed-owner extra | - |\n").unwrap();
    assert!(
        close_text(&fx.core, TASK)
            .await
            .contains("invalid deployed-verification deferral")
    );
}

#[tokio::test]
async fn cas_6f10_waiver_does_not_cover_skip_marker() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(
        &mut env,
        &[
            ("src/feature.rs", "pub fn feature() {}\n"),
            ("tests/test.spec.ts", "test.only('feature', () => {});\n"),
        ],
        "Verify deployed endpoint",
    );
    let head = git(&fx.repo, &["rev-parse", "HEAD"]);
    let _supervisor = cas_6f10_supervisor(&fx);
    let supervisor_id = format!("cas-6f10-supervisor-{}", std::process::id());
    cas_store::waive_qa_pass(
        &fx.repo.join(".cas"),
        TASK,
        &supervisor_id,
        "test-agent",
        "factory/test-agent",
        &head,
        "Evidence waived, not skips",
        chrono::Utc::now(),
    )
    .unwrap();
    let refused = close_text(&fx.core, TASK).await;
    assert!(
        refused.contains("adds test skip/focus markers"),
        "{refused}"
    );
    assert_eq!(fx.status(), TaskStatus::InProgress);
}

#[tokio::test]
async fn cas_6f10_waiver_does_not_cover_another_tip() {
    let mut env = TestEnvGuard::temp_home();
    let fx = fixture(
        &mut env,
        &[("src/feature.rs", "pub fn feature() {}\n")],
        "Verify deployed endpoint",
    );
    let parent = git(&fx.repo, &["rev-parse", "HEAD^"]);
    let _supervisor = cas_6f10_supervisor(&fx);
    let supervisor_id = format!("cas-6f10-supervisor-{}", std::process::id());
    cas_store::waive_qa_pass(
        &fx.repo.join(".cas"),
        TASK,
        &supervisor_id,
        "test-agent",
        "factory/test-agent",
        &parent,
        "Only the old tip is waived",
        chrono::Utc::now(),
    )
    .unwrap();
    let refused = close_text(&fx.core, TASK).await;
    assert!(
        refused.contains("QA evidence ledger is missing"),
        "{refused}"
    );
    assert_eq!(fx.status(), TaskStatus::InProgress);
    assert!(!fx.notes().contains("QA evidence ledger waived"));
}
