//! cas-619f: the independent QA and polish pass for user-facing deliveries.
//!
//! Drives the real MCP surface end to end: a user-facing delivery parks for
//! merge → Cassy opens a QA round and its work item → the implementer is
//! refused as reviewer → a different agent claims it → a rejection routes the
//! delivery back through request_changes → the next park opens round two →
//! an approval satisfies the supervisor merge guard and the close backstop.

use crate::support::*;
use cas::mcp::tools::*;
use cas::mcp::{CasCore, CasService};
use cas::store::{open_agent_store, open_prompt_queue_store, open_task_store};
use cas::types::{Agent, AgentRole, DependencyType, TaskStatus};
use cas_mcp::types::VerificationRequest;
use rmcp::handler::server::wrapper::Parameters;
use std::path::Path;
use std::process::Command;

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

fn commit_file(repo: &Path, path: &str, body: &str, message: &str) -> String {
    let full = repo.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, body).unwrap();
    git(repo, &["add", path]);
    git(repo, &["commit", "-q", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

fn verification(value: serde_json::Value) -> VerificationRequest {
    serde_json::from_value(value).expect("VerificationRequest")
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

/// A reviewer with a different registered name from the implementer.
fn reviewer_core(cas_dir: &Path, name: &str) -> CasCore {
    let id = format!("qa-session-{name}-{}", std::process::id());
    let agents = open_agent_store(cas_dir).unwrap();
    agents
        .register(&Agent::new_with_role(id.clone(), name.to_string(), AgentRole::Worker))
        .unwrap();
    let core = CasCore::with_daemon(cas_dir.to_path_buf(), None, None);
    core.set_agent_id_for_testing(id);
    core
}

/// Repo with a user-facing delivery on `factory/test-agent` (the fixture
/// agent's registered name, so the fixture core IS the implementer).
fn fixture() -> (tempfile::TempDir, CasCore, std::path::PathBuf, String) {
    let (temp, core) = setup_cas();
    let repo = temp.path().to_path_buf();
    let cas_dir = repo.join(".cas");
    std::fs::write(
        cas_dir.join("config.toml"),
        format!(
            // The implementer's own evidence gate (cas-0cd5) is covered by
            // qa_evidence_gate.rs; these tests exercise the independent pass.
            "[factory]\nartifacts_root = {:?}\n[verification]\nenabled = false\n[qa]\nevidence_gate = false\n",
            repo.join("artifacts").display().to_string()
        ),
    )
    .unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    commit_file(&repo, "README.md", "seed\n", "seed");
    git(&repo, &["checkout", "-q", "-b", "factory/test-agent"]);
    commit_file(&repo, "web/composer.css", ".composer{gap:8px}\n", "composer spacing");

    let task_id = "cas-ui01".to_string();
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = cas::types::Task::new(task_id.clone(), "Composer spacing".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("test-agent".to_string());
    tasks.add(&task).unwrap();
    (temp, core, repo, task_id)
}

/// A round's LEDGER.md plus its cas-c3b8 `bundle.json` for `head`.
fn round_evidence(dir: &Path, task_id: &str, head: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let ledger = dir.join("LEDGER.md");
    std::fs::write(&ledger, "# independent QA ledger\n").unwrap();
    std::fs::write(
        dir.join("bundle.json"),
        serde_json::json!({
            "schema": 1,
            "task_id": task_id,
            "producer": "independent-qa",
            "head_sha": head,
        })
        .to_string(),
    )
    .unwrap();
    ledger
}

fn qa_task_id(cas_dir: &Path, delivery: &str) -> String {
    cas_store::latest_qa_pass(cas_dir, delivery, chrono::Utc::now())
        .unwrap()
        .expect("a QA round")
        .qa_task_id
        .expect("its work item")
}

#[tokio::test]
async fn user_facing_park_dispatches_an_independent_round_and_refuses_self_review() {
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
    let cas_dir = repo.join(".cas");
    let _keep = &temp;

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    assert!(parked.contains("path:web/composer.css"), "{parked}");
    let tasks = open_task_store(&cas_dir).unwrap();
    assert_eq!(tasks.get(&task_id).unwrap().status, TaskStatus::AwaitingMerge);

    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert_eq!(passes.len(), 1);
    let pass = &passes[0];
    assert_eq!(pass.implementer_agent_id, "test-agent");
    assert_eq!(pass.bound_head, git(&repo, &["rev-parse", "factory/test-agent"]));
    let qa_task = tasks.get(pass.qa_task_id.as_deref().unwrap()).unwrap();
    assert!(qa_task.labels.iter().any(|label| label == "qa-pass"));
    assert!(qa_task.description.contains("You are NOT the implementer (test-agent)"));
    assert!(
        tasks
            .get_dependencies(&qa_task.id)
            .unwrap()
            .iter()
            .any(|dep| dep.to_id == task_id && dep.dep_type == DependencyType::Related)
    );

    // The supervisor is handed a CAS-composed envelope naming the taste lane.
    let queued = open_prompt_queue_store(&cas_dir).unwrap().peek_all(50).unwrap();
    let handoff = queued
        .iter()
        .find(|row| row.source == format!("qa-dispatch:{}", pass.id))
        .expect("QA handoff queued for the supervisor");
    assert_eq!(handoff.target, "supervisor");
    assert!(handoff.prompt.starts_with("<cas-qa-dispatch "), "{}", handoff.prompt);
    assert!(handoff.prompt.contains(&format!("lane=taste task_id={}", qa_task.id)));

    // Re-closing the same tip is idempotent.
    let again = close_text(&core, &task_id).await;
    assert!(again.contains("INDEPENDENT QA PENDING"), "{again}");
    assert_eq!(cas_store::list_qa_passes(&cas_dir, &task_id).unwrap().len(), 1);

    // No self-review: the implementer cannot start the QA work item...
    let refused = core
        .cas_task_start(Parameters(IdRequest {
            id: qa_task.id.clone(),
        }))
        .await
        .expect_err("implementer must be refused");
    assert!(refused.message.contains("no self-review"), "{}", refused.message);

    // ...nor record a verdict.
    let ledger = repo.join("LEDGER.md");
    std::fs::write(&ledger, "# ledger\n").unwrap();
    let service = CasService::new(core.clone(), None);
    let self_verdict = service
        .verification(Parameters(verification(serde_json::json!({
            "action": "qa_record",
            "task_id": task_id,
            "status": "approved",
            "summary": "my own work looks great",
            "ledger_path": ledger.display().to_string(),
        }))))
        .await
        .expect_err("implementer verdict must be refused");
    assert!(self_verdict.message.contains("no self-review"), "{}", self_verdict.message);

    // A different agent may start it, which claims the round.
    let reviewer = reviewer_core(&cas_dir, "qa-reviewer");
    let started = extract_text(
        reviewer
            .cas_task_start(Parameters(IdRequest {
                id: qa_task.id.clone(),
            }))
            .await
            .expect("a different agent may review"),
    );
    assert!(started.contains("claimed"), "{started}");
}

#[tokio::test]
async fn rejection_returns_the_delivery_and_approval_unlocks_merge_and_close() {
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    let tasks = open_task_store(&cas_dir).unwrap();

    close_text(&core, &task_id).await;
    let round1_task = qa_task_id(&cas_dir, &task_id);
    let reviewer = reviewer_core(&cas_dir, "qa-reviewer");
    let reviewer_service = CasService::new(reviewer.clone(), None);
    reviewer
        .cas_task_start(Parameters(IdRequest {
            id: round1_task.clone(),
        }))
        .await
        .unwrap();

    // The supervisor's raw git merge is refused while no verdict exists.
    let merge_cmd = "git merge --no-ff factory/test-agent";
    let refusal = cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, merge_cmd)
        .expect("merge must wait for the independent pass");
    assert!(refusal.contains("INDEPENDENT QA REQUIRED"), "{refusal}");

    // Reject with evidence: the delivery goes back to its implementer.
    // A verdict without its evidence bundle is refused.
    let bare = repo.join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    std::fs::write(bare.join("LEDGER.md"), "# no bundle\n").unwrap();
    let unbacked = reviewer_service
        .verification(Parameters(verification(serde_json::json!({
            "action": "qa_record",
            "task_id": task_id,
            "status": "approved",
            "summary": "trust me",
            "ledger_path": bare.join("LEDGER.md").display().to_string(),
        }))))
        .await
        .expect_err("a verdict needs its bundle");
    assert!(unbacked.message.contains("no evidence bundle"), "{}", unbacked.message);

    let round1_head = git(&repo, &["rev-parse", "factory/test-agent"]);
    let ledger1 = round_evidence(&repo.join("round-1"), &task_id, &round1_head);
    let rejected = extract_text(
        reviewer_service
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_record",
                "task_id": task_id,
                "status": "rejected",
                "summary": "focus ring missing on the send button in dark mode",
                "issues": "[{\"severity\":\"high\",\"problem\":\"no focus ring\"}]",
                "ledger_path": ledger1.display().to_string(),
            }))))
            .await
            .unwrap(),
    );
    assert!(rejected.contains("REJECTION"), "{rejected}");
    let reopened = tasks.get(&task_id).unwrap();
    assert_eq!(reopened.status, TaskStatus::Open);
    assert_eq!(reopened.assignee.as_deref(), Some("test-agent"));
    assert!(reopened.notes.contains("Independent QA round 1"), "{}", reopened.notes);
    assert!(reopened.notes.contains(&ledger1.display().to_string()));
    assert_eq!(tasks.get(&round1_task).unwrap().status, TaskStatus::Closed);
    let notices = open_prompt_queue_store(&cas_dir).unwrap().peek_all(50).unwrap();
    assert!(
        notices
            .iter()
            .any(|row| row.target == "test-agent" && row.prompt.contains("focus ring")),
        "the implementer must hear the rejection"
    );

    // Fix, re-deliver: the next park opens round two for the new tip.
    reopened_to_in_progress(&cas_dir, &task_id);
    let fixed_head = commit_file(&repo, "web/composer.css", ".composer{gap:8px}\n:focus-visible{outline:2px solid}\n", "focus ring");
    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let round2 = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .unwrap();
    assert_eq!(round2.round, 2);
    assert_eq!(round2.bound_head, fixed_head);
    reviewer
        .cas_task_start(Parameters(IdRequest {
            id: round2.qa_task_id.clone().unwrap(),
        }))
        .await
        .unwrap();
    let ledger2 = round_evidence(&repo.join("round-2"), &task_id, &fixed_head);
    let approved = extract_text(
        reviewer_service
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_record",
                "task_id": task_id,
                "status": "approved",
                "summary": "journeys clean; rubric 4/4/4/4/5",
                "ledger_path": ledger2.display().to_string(),
            }))))
            .await
            .unwrap(),
    );
    assert!(approved.contains("APPROVAL"), "{approved}");
    assert!(
        tasks.get(&task_id).unwrap().notes.contains(&format!(
            "PLATFORM_PROOF qa-bundle: {}",
            repo.join("round-2").join("bundle.json").display()
        )),
        "the approved round's bundle is cited on the delivery"
    );
    assert!(
        cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, merge_cmd).is_none(),
        "an approved tip may merge"
    );

    // A commit after the approval is not covered.
    let drift_head = commit_file(&repo, "web/composer.css", "/* drift */\n", "unreviewed drift");
    assert_ne!(drift_head, fixed_head);
    assert!(cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, merge_cmd).is_some());
    git(&repo, &["reset", "-q", "--hard", &fixed_head]);

    // Merge the reviewed tip; the close backstop accepts it.
    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["merge", "-q", "--no-ff", "-m", "merge", "factory/test-agent"]);
    git(&repo, &["checkout", "-q", "factory/test-agent"]);
    let closed = close_text(&core, &task_id).await;
    assert!(!closed.contains("INDEPENDENT QA REQUIRED"), "{closed}");
    assert_eq!(tasks.get(&task_id).unwrap().status, TaskStatus::Closed, "{closed}");

    let status = extract_text(
        reviewer_service
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_status",
                "task_id": task_id,
            }))))
            .await
            .unwrap(),
    );
    assert!(status.contains("round 2") && status.contains("passed"), "{status}");
    assert!(status.contains("round 1") && status.contains("failed"), "{status}");
}

#[tokio::test]
async fn pending_round_refuses_both_merge_paths_in_progress_and_awaiting_merge() {
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let qa_task = qa_task_id(&cas_dir, &task_id);
    let tasks = open_task_store(&cas_dir).unwrap();

    for status in [TaskStatus::AwaitingMerge, TaskStatus::InProgress] {
        let mut task = tasks.get(&task_id).unwrap();
        task.status = status;
        tasks.update(&task).unwrap();

        let raw = cas::qa_pass::supervisor_merge_refusal(
            &cas_dir,
            &repo,
            "git merge --no-ff --no-commit factory/test-agent",
        )
        .unwrap_or_else(|| panic!("raw merge was allowed for {status:?}"));
        assert!(raw.contains(&task_id) && raw.contains(&qa_task), "{raw}");

        let managed = core
            .worktree_merge("test-agent", false, Some(&task_id), false, None)
            .await
            .expect_err("worktree_merge must wait for the independent QA verdict");
        assert!(
            managed.message.contains(&task_id) && managed.message.contains(&qa_task),
            "{managed:?}"
        );
    }

    // The round still binds its branch if the task is reassigned before QA.
    let mut task = tasks.get(&task_id).unwrap();
    task.status = TaskStatus::InProgress;
    task.assignee = Some("replacement-agent".to_string());
    task.deliverables.parked_branch = None;
    tasks.update(&task).unwrap();
    let refusal = cas::qa_pass::supervisor_merge_refusal(
        &cas_dir,
        &repo,
        "git merge factory/test-agent",
    )
    .expect("the recorded round must keep its branch guarded after reassignment");
    assert!(refusal.contains(&qa_task), "{refusal}");
}

#[tokio::test]
async fn merged_without_a_verdict_is_refused_at_close_and_dispatched() {
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(&task_id).unwrap();
    task.demo_statement = "Open the composer and see even spacing".to_string();
    tasks.update(&task).unwrap();

    // Someone merged outside the guarded paths before the worker closed.
    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["merge", "-q", "--no-ff", "-m", "merge", "factory/test-agent"]);
    git(&repo, &["checkout", "-q", "factory/test-agent"]);

    let refused = close_text(&core, &task_id).await;
    assert!(refused.contains("INDEPENDENT QA REQUIRED"), "{refused}");
    assert!(refused.contains("INDEPENDENT QA DISPATCHED"), "{refused}");
    assert_eq!(tasks.get(&task_id).unwrap().status, TaskStatus::InProgress);
    assert_eq!(cas_store::list_qa_passes(&cas_dir, &task_id).unwrap().len(), 1);
    let guard = cas::qa_pass::supervisor_merge_refusal(
        &cas_dir,
        &repo,
        "git merge --no-ff --no-commit factory/test-agent",
    )
    .expect("the backstop's InProgress round must block a raw merge");
    assert!(guard.contains(&qa_task_id(&cas_dir, &task_id)), "{guard}");
}

#[tokio::test]
async fn backend_only_deliveries_are_untouched_and_qa_type_on_add_is_refused() {
    let (temp, core, repo, _) = fixture();
    let _env = env_test_lock();
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    git(&repo, &["checkout", "-q", "-b", "factory/backend-agent", "main"]);
    commit_file(&repo, "src/lib.rs", "pub fn x() {}\n", "backend change");
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = cas::types::Task::new("cas-be01".to_string(), "Backend".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("backend-agent".to_string());
    tasks.add(&task).unwrap();

    assert!(
        cas_store::list_qa_passes(&cas_dir, "cas-be01").unwrap().is_empty()
            && cas::qa_pass::supervisor_merge_refusal(
                &cas_dir,
                &repo,
                "git merge factory/backend-agent"
            )
            .is_none()
    );

    let service = CasService::new(core.clone(), None);
    let refused = service
        .verification(Parameters(verification(serde_json::json!({
            "action": "add",
            "task_id": "cas-be01",
            "status": "approved",
            "summary": "typo'd type",
            "verification_type": "qa",
        }))))
        .await
        .expect_err("qa is not a verifications-table type");
    assert!(refused.message.contains("qa_record"), "{}", refused.message);
    let unknown = service
        .verification(Parameters(verification(serde_json::json!({
            "action": "add",
            "task_id": "cas-be01",
            "status": "approved",
            "summary": "typo'd type",
            "verification_type": "tsak",
        }))))
        .await
        .expect_err("unknown types no longer fall through to task");
    assert!(unknown.message.contains("Unknown verification_type"), "{}", unknown.message);
}

fn reopened_to_in_progress(cas_dir: &Path, task_id: &str) {
    let tasks = open_task_store(cas_dir).unwrap();
    let mut task = tasks.get(task_id).unwrap();
    task.status = TaskStatus::InProgress;
    tasks.update(&task).unwrap();
}

#[tokio::test]
async fn docs_and_test_only_deliveries_are_never_gated_even_with_a_demo() {
    let (temp, core, repo, _) = fixture();
    let _env = env_test_lock();
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    git(&repo, &["checkout", "-q", "-b", "factory/docs-agent", "main"]);
    commit_file(&repo, "docs/qa/journeys.md", "# journeys\n", "docs");
    commit_file(&repo, "web/e2e/reply.journey.spec.ts", "test('x', () => {});\n", "spec");
    commit_file(&repo, ".github/workflows/ci.yml", "on: push\n", "ci");
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = cas::types::Task::new("cas-doc01".to_string(), "Journey docs".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("docs-agent".to_string());
    task.demo_statement = "Read the journey catalog".to_string();
    tasks.add(&task).unwrap();

    let parked = close_text(&core, "cas-doc01").await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert!(!parked.contains("INDEPENDENT QA"), "{parked}");
    assert!(cas_store::list_qa_passes(&cas_dir, "cas-doc01").unwrap().is_empty());
    assert!(
        cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, "git merge factory/docs-agent")
            .is_none()
    );
}

#[tokio::test]
async fn supervisor_waiver_needs_a_reason_logs_a_decision_and_shows_in_epic_status() {
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    close_text(&core, &task_id).await;
    let service = CasService::new(core.clone(), None);

    let previous_role = std::env::var("CAS_AGENT_ROLE").ok();
    // SAFETY: env_test_lock is held for the whole test body.
    unsafe { std::env::set_var("CAS_AGENT_ROLE", "supervisor") };
    let without_reason = service
        .verification(Parameters(verification(serde_json::json!({
            "action": "qa_waive",
            "task_id": task_id,
            "summary": "   ",
        }))))
        .await;
    let waived = service
        .verification(Parameters(verification(serde_json::json!({
            "action": "qa_waive",
            "task_id": task_id,
            "summary": "copy-only tweak reviewed live with the operator",
        }))))
        .await;
    // SAFETY: as above.
    unsafe {
        match previous_role {
            Some(role) => std::env::set_var("CAS_AGENT_ROLE", role),
            None => std::env::remove_var("CAS_AGENT_ROLE"),
        }
    }
    let refused = without_reason.expect_err("a waiver without a reason is refused");
    assert!(refused.message.contains("reason"), "{}", refused.message);
    let waived = extract_text(waived.expect("supervisor may waive with a reason"));
    assert!(waived.contains("waived"), "{waived}");

    let task = open_task_store(&cas_dir).unwrap().get(&task_id).unwrap();
    assert!(
        task.notes.contains("✅ DECISION Independent QA waived")
            && task.notes.contains("copy-only tweak reviewed live"),
        "{}",
        task.notes
    );
    assert!(
        cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, "git merge factory/test-agent")
            .is_none(),
        "the waived tip may merge"
    );
    let section = cas::qa_pass::render_epic_qa_section(&cas_dir, &[task]);
    assert!(section.contains("Independent QA:"), "{section}");
    assert!(section.contains("WAIVED by"), "{section}");
    assert!(section.contains("copy-only tweak reviewed live"), "{section}");

    // Workers cannot waive.
    let worker_waive = service
        .verification(Parameters(verification(serde_json::json!({
            "action": "qa_waive",
            "task_id": task_id,
            "summary": "trust me",
        }))))
        .await
        .expect_err("qa_waive is supervisor-only");
    assert!(worker_waive.message.contains("supervisor-only"), "{}", worker_waive.message);
}

/// Reject round 1 of the fixture delivery (on `main`) and hand the task back
/// to its implementer, as `qa_record status=rejected` does.
async fn reject_round_one(core: &CasCore, repo: &Path, task_id: &str) -> CasCore {
    let cas_dir = repo.join(".cas");
    let parked = close_text(core, task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let reviewer = reviewer_core(&cas_dir, "qa-reviewer");
    reviewer
        .cas_task_start(Parameters(IdRequest {
            id: qa_task_id(&cas_dir, task_id),
        }))
        .await
        .unwrap();
    let head = git(repo, &["rev-parse", "factory/test-agent"]);
    let ledger = round_evidence(&repo.join("round-1"), task_id, &head);
    let rejected = extract_text(
        CasService::new(reviewer.clone(), None)
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_record",
                "task_id": task_id,
                "status": "rejected",
                "summary": "the new spacing has no regression test",
                "ledger_path": ledger.display().to_string(),
            }))))
            .await
            .unwrap(),
    );
    assert!(rejected.contains("REJECTION"), "{rejected}");
    reopened_to_in_progress(&cas_dir, task_id);
    reviewer
}

/// GH #1001 (cas-627c): a rejected round must lead to round N+1 on the next
/// park even when the corrective commit is not itself user-facing (a test)
/// and the task's target moved between rounds to a branch that already holds
/// round 1. Re-deciding eligibility from the new, smaller diff found nothing
/// user-facing, so no round opened, the gate kept refusing the merge, and a
/// reviewer's qa_record failed with "not found: open independent QA pass".
#[tokio::test]
async fn rejected_round_reopens_after_a_target_change_and_a_test_only_fix() {
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    // `task update target_branch=` needs a canonical project identity; use
    // the one the fixture's rows already carry (the checkout's name).
    let config = cas_dir.join("config.toml");
    let body = std::fs::read_to_string(&config).unwrap();
    let identity = repo.file_name().unwrap().to_string_lossy().to_string();
    std::fs::write(&config, format!("{body}[project]\ncanonical_id = {identity:?}\n")).unwrap();
    cas::store::known_repos::ensure_host_schema().unwrap();
    let reviewer = reject_round_one(&core, &repo, &task_id).await;

    // The supervisor moves the task onto an epic cut from the reviewed tip,
    // so the epic already contains round 1's user-facing change.
    git(&repo, &["branch", "epic/polish", "factory/test-agent"]);
    let request: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "update",
        "id": task_id,
        "target_repo": repo.display().to_string(),
        "target_branch": "epic/polish",
    }))
    .unwrap();
    let retargeted = CasService::new(core.clone(), None).task(Parameters(request)).await;
    let retargeted = match retargeted {
        Ok(result) => extract_text(result),
        Err(error) => error.message.to_string(),
    };
    let tasks = open_task_store(&cas_dir).unwrap();
    assert_eq!(
        tasks.get(&task_id).unwrap().deliverables.work_target.map(|target| target.target_branch),
        Some("epic/polish".to_string()),
        "{retargeted}"
    );

    // The fix the reviewer asked for is a test only.
    let fixed_head = commit_file(
        &repo,
        "web/composer.test.ts",
        "test('gap', () => expect(gap()).toBe(8));\n",
        "regression test for the spacing",
    );
    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let round2 = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .expect("round two");
    assert_eq!(round2.round, 2);
    assert_eq!(round2.bound_head, fixed_head);
    assert!(round2.qa_task_id.is_some(), "round two has its work item");

    // qa_record against the open round succeeds.
    reviewer
        .cas_task_start(Parameters(IdRequest {
            id: round2.qa_task_id.clone().unwrap(),
        }))
        .await
        .unwrap();
    let ledger2 = round_evidence(&repo.join("round-2"), &task_id, &fixed_head);
    let approved = extract_text(
        CasService::new(reviewer, None)
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_record",
                "task_id": task_id,
                "status": "approved",
                "summary": "regression test present; journeys clean",
                "ledger_path": ledger2.display().to_string(),
            }))))
            .await
            .unwrap(),
    );
    assert!(approved.contains("APPROVAL"), "{approved}");
}
