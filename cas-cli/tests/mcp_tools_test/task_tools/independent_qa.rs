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
fn fixture(
    test_env: &mut TestEnvGuard,
) -> (tempfile::TempDir, CasCore, std::path::PathBuf, String) {
    let (temp, core) = setup_cas(test_env);
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
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
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
async fn resetting_a_qa_work_item_lets_a_replacement_start_the_same_round_cas_1aef3() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let qa_task = qa_task_id(&cas_dir, &task_id);
    let original = reviewer_core(&cas_dir, "dead-reviewer");
    original
        .cas_task_start(Parameters(IdRequest { id: qa_task.clone() }))
        .await
        .expect("first reviewer claims the round");
    let before = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .unwrap();

    core.cas_task_reset(Parameters(TaskReleaseRequest {
        task_id: qa_task.clone(),
        force: Some(true),
    }))
    .await
    .expect("supervisor reset clears the dead reviewer's task and pass claim");
    let pending = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .unwrap();
    assert_eq!(pending.id, before.id);
    assert_eq!(pending.deadline_at, before.deadline_at);
    assert_eq!(pending.state, cas::types::QaPassState::Pending);
    assert!(pending.reviewer_agent_id.is_none());

    let replacement = reviewer_core(&cas_dir, "replacement-reviewer");
    let started = extract_text(
        replacement
            .cas_task_start(Parameters(IdRequest { id: qa_task }))
            .await
            .expect("replacement reviewer starts the reset QA task"),
    );
    assert!(started.contains("claimed"), "{started}");
    let reclaimed = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .unwrap();
    assert_eq!(reclaimed.id, before.id);
    assert_eq!(reclaimed.reviewer_agent_id.as_deref(), Some("replacement-reviewer"));
}

#[tokio::test]
async fn rejection_returns_the_delivery_and_approval_unlocks_merge_and_close() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
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

/// cas-e371 (GH #1023 finding 1): a narrow, correct delivery on a page with
/// an unrelated older defect passes independent QA. The defect is recorded
/// and filed as a follow-up linked to the delivery; it cannot carry a
/// rejection on its own.
#[tokio::test]
async fn pre_existing_defects_become_linked_follow_ups_and_never_reject_alone() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    let tasks = open_task_store(&cas_dir).unwrap();

    close_text(&core, &task_id).await;
    let round_task = qa_task_id(&cas_dir, &task_id);
    let reviewer = reviewer_core(&cas_dir, "qa-reviewer");
    let reviewer_service = CasService::new(reviewer.clone(), None);
    reviewer
        .cas_task_start(Parameters(IdRequest { id: round_task.clone() }))
        .await
        .unwrap();
    let head = git(&repo, &["rev-parse", "factory/test-agent"]);
    let ledger = round_evidence(&repo.join("round-1"), &task_id, &head);
    let pre_existing = "[{\"severity\":\"normal\",\"scope\":\"pre-existing\",\"problem\":\"Footer links fail contrast at 3.1:1. The base build too.\",\"suggestion\":\"use --ink-mid\",\"file\":\"F01.png\"}]";

    // Rejecting for the page's older defect alone is refused, and changes nothing.
    let refused = reviewer_service
        .verification(Parameters(verification(serde_json::json!({
            "action": "qa_record",
            "task_id": task_id,
            "status": "rejected",
            "summary": "footer contrast fails",
            "issues": pre_existing,
            "ledger_path": ledger.display().to_string(),
        }))))
        .await
        .expect_err("a pre-existing defect never rejects a delivery on its own");
    assert!(refused.message.contains("never rejects a delivery"), "{}", refused.message);
    assert_eq!(tasks.get(&task_id).unwrap().status, TaskStatus::AwaitingMerge);
    assert_ne!(tasks.get(&round_task).unwrap().status, TaskStatus::Closed);

    // Approving records it and files a linked follow-up.
    let approved = extract_text(
        reviewer_service
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_record",
                "task_id": task_id,
                "status": "approved",
                "summary": "spacing change is correct; footer contrast is pre-existing",
                "issues": pre_existing,
                "ledger_path": ledger.display().to_string(),
            }))))
            .await
            .unwrap(),
    );
    assert!(approved.contains("APPROVAL"), "{approved}");
    assert!(approved.contains("Pre-existing follow-ups filed"), "{approved}");
    let follow_ups: Vec<_> = tasks
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|task| task.labels.iter().any(|label| label == "qa-follow-up"))
        .collect();
    assert_eq!(follow_ups.len(), 1, "one follow-up per pre-existing issue");
    let follow_up = &follow_ups[0];
    assert_eq!(
        follow_up.title,
        format!("Pre-existing: Footer links fail contrast at 3.1:1 (found in QA of {task_id})")
    );
    assert!(follow_up.description.contains("use --ink-mid"), "{}", follow_up.description);
    assert!(follow_up.description.contains(&ledger.display().to_string()));
    assert_eq!(follow_up.status, TaskStatus::Open);
    assert!(approved.contains(&follow_up.id), "{approved}");
    assert!(
        tasks
            .get_dependencies(&follow_up.id)
            .unwrap()
            .iter()
            .any(|dep| dep.to_id == task_id && dep.dep_type == DependencyType::Related),
        "the follow-up is linked to the delivery"
    );
    assert!(
        tasks.get(&task_id).unwrap().notes.contains(&follow_up.id),
        "the delivery names its follow-up"
    );
    // The approval stands: the tip may merge.
    let merge_cmd = "git merge --no-ff factory/test-agent";
    assert!(cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, merge_cmd).is_none());
}

#[tokio::test]
async fn pending_round_refuses_both_merge_paths_in_progress_and_awaiting_merge() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
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
            .worktree_merge("test-agent", false, Some(&task_id), false, None, false, None)
            .await
            .expect_err("worktree_merge must wait for the independent QA verdict");
        assert!(
            managed.message.contains(&task_id) && managed.message.contains(&qa_task),
            "{managed:?}"
        );

        // GH #1024: the reported call omitted task_id and merged despite QA.
        let implicit = core
            .worktree_merge("factory/test-agent", false, None, false, None, false, None)
            .await
            .expect_err("worktree_merge without task_id must wait for QA");
        assert!(
            implicit.message.contains(&task_id) && implicit.message.contains(&qa_task),
            "{implicit:?}"
        );
        assert_eq!(
            git(&repo, &["rev-parse", "main"]),
            git(&repo, &["merge-base", "main", "factory/test-agent"])
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

/// A delivery merged into a non-trunk lane (an epic) outside the guarded
/// paths is still sent for review before it closes. The supervisor's handoff
/// reports target ancestry, without assuming it never parked (GH #1026).
#[tokio::test]
async fn merged_into_an_epic_without_a_verdict_is_refused_at_close_and_dispatched() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(&task_id).unwrap();
    task.demo_statement = "Open the composer and see even spacing".to_string();
    tasks.update(&task).unwrap();
    let mut epic = cas::types::Task::new("cas-uiepic".to_string(), "UI epic".to_string());
    epic.task_type = cas::types::TaskType::Epic;
    epic.branch = Some("epic/ui".to_string());
    tasks.add(&epic).unwrap();
    tasks
        .add_dependency(&cas::types::Dependency::new(
            task_id.clone(),
            epic.id.clone(),
            DependencyType::ParentChild,
        ))
        .unwrap();

    // Someone merged into the epic outside the guarded paths before the
    // worker closed.
    git(&repo, &["branch", "epic/ui", "main"]);
    git(&repo, &["checkout", "-q", "epic/ui"]);
    git(&repo, &["merge", "-q", "--no-ff", "-m", "merge", "factory/test-agent"]);
    git(&repo, &["checkout", "-q", "factory/test-agent"]);

    let refused = close_text(&core, &task_id).await;
    assert!(refused.contains("INDEPENDENT QA REQUIRED"), "{refused}");
    assert!(refused.contains("INDEPENDENT QA DISPATCHED"), "{refused}");
    assert!(refused.contains("the close waits for its verdict"), "{refused}");
    assert_eq!(tasks.get(&task_id).unwrap().status, TaskStatus::InProgress);
    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert_eq!(passes.len(), 1);
    let handoff = open_prompt_queue_store(&cas_dir)
        .unwrap()
        .peek_all(50)
        .unwrap()
        .into_iter()
        .find(|row| row.source == format!("qa-dispatch:{}", passes[0].id))
        .expect("QA handoff queued for the supervisor");
    assert!(
        handoff
            .prompt
            .contains("delivered tip @"),
        "{}",
        handoff.prompt
    );
    assert!(handoff.prompt.contains("contained in epic/ui"), "{}", handoff.prompt);
    assert!(!handoff.prompt.contains("never parked"), "{}", handoff.prompt);
    assert!(!handoff.prompt.contains("parked for merge"), "{}", handoff.prompt);
    let guard = cas::qa_pass::supervisor_merge_refusal(
        &cas_dir,
        &repo,
        "git merge --no-ff --no-commit factory/test-agent",
    )
    .expect("the backstop's InProgress round must block a raw merge");
    assert!(guard.contains(&qa_task_id(&cas_dir, &task_id)), "{guard}");
}

/// A live supervisor registered in `cas_dir`, acting through its own core.
fn supervisor_core(cas_dir: &Path) -> CasCore {
    let id = format!("supervisor-session-{}", std::process::id());
    open_agent_store(cas_dir)
        .unwrap()
        .register(&Agent::new_with_role(
            id.clone(),
            "qa-supervisor".to_string(),
            AgentRole::Supervisor,
        ))
        .unwrap();
    let core = CasCore::with_daemon(cas_dir.to_path_buf(), None, None);
    core.set_agent_id_for_testing(id);
    core
}

struct SupervisorRole(Option<String>);

impl SupervisorRole {
    fn enter() -> Self {
        let previous = std::env::var("CAS_AGENT_ROLE").ok();
        // SAFETY: callers hold TestEnvGuard for the whole test body.
        unsafe { std::env::set_var("CAS_AGENT_ROLE", "supervisor") };
        Self(previous)
    }
}

impl Drop for SupervisorRole {
    fn drop(&mut self) {
        // SAFETY: as in `enter`.
        unsafe {
            match self.0.take() {
                Some(role) => std::env::set_var("CAS_AGENT_ROLE", role),
                None => std::env::remove_var("CAS_AGENT_ROLE"),
            }
        }
    }
}

/// cas-5c38 (GH #999), the domdms cas-0019 shape: a user-facing delivery
/// (demo_statement) was merged to trunk long before anyone closed it, and it
/// never parked. Cassy must not dispatch a review of code already on trunk,
/// qa_waive must not claim the task "must park for merge first", and the
/// supervisor's override with a reason and commit_receipt closes it, with
/// the waiver recorded against that commit.
#[tokio::test]
async fn merged_to_trunk_before_close_is_not_dispatched_and_closes_by_override_cas_5c38() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(&task_id).unwrap();
    task.demo_statement = "Open the composer and see even spacing".to_string();
    tasks.update(&task).unwrap();

    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["merge", "-q", "--no-ff", "-m", "merged in May", "factory/test-agent"]);
    let merged = git(&repo, &["rev-parse", "factory/test-agent"]);
    git(&repo, &["checkout", "-q", "factory/test-agent"]);

    let refused = close_text(&core, &task_id).await;
    assert!(refused.contains("INDEPENDENT QA REQUIRED"), "{refused}");
    assert!(refused.contains("already on trunk main"), "{refused}");
    assert!(refused.contains("no QA pass was opened"), "{refused}");
    assert!(refused.contains("supervisor_override=true"), "{refused}");
    assert!(!refused.contains("DISPATCHED"), "{refused}");
    assert!(
        cas_store::list_qa_passes(&cas_dir, &task_id).unwrap().is_empty(),
        "no QA pass may be opened for a head already on trunk"
    );
    assert!(
        !open_prompt_queue_store(&cas_dir)
            .unwrap()
            .peek_all(50)
            .unwrap()
            .iter()
            .any(|row| row.source.starts_with("qa-dispatch:")),
        "no QA handoff may be queued for code already on trunk"
    );

    let supervisor = supervisor_core(&cas_dir);
    let _role = SupervisorRole::enter();
    let waive = CasService::new(supervisor.clone(), None)
        .verification(Parameters(verification(serde_json::json!({
            "action": "qa_waive",
            "task_id": task_id,
            "summary": "shipped in May; live in production since",
        }))))
        .await
        .expect_err("no recorded delivery tip to bind a waiver to");
    assert!(!waive.message.contains("must park"), "{}", waive.message);
    assert!(
        waive.message.contains("supervisor_override=true")
            && waive.message.contains("commit_receipt"),
        "the refusal must name the route that works: {}",
        waive.message
    );

    let mut request = close_req(&task_id);
    request.supervisor_override = Some(true);
    request.reason = Some("shipped in May; live in production since".to_string());
    request.commit_receipt = Some(merged[..12].to_string());
    let closed = match supervisor.cas_task_close(Parameters(request)).await {
        Ok(result) => extract_text(result),
        Err(error) => error.message.to_string(),
    };
    let task = tasks.get(&task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Closed, "{closed}");
    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert_eq!(passes.len(), 1, "exactly the override's waiver is on record");
    assert_eq!(passes[0].state, cas::types::QaPassState::Waived);
    assert_eq!(passes[0].bound_head, merged, "the waiver binds the merged commit");
    assert!(
        task.notes.contains("Independent QA waived by supervisor")
            && task.notes.contains("shipped in May"),
        "{}",
        task.notes
    );
}

/// cas-5c38: clearing the demo_statement withdraws the pending round it
/// caused, so the gates stop demanding QA for a delivery whose diff is not
/// user-facing.
#[tokio::test]
async fn clearing_the_demo_statement_withdraws_a_pending_round_cas_5c38() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    // A backend-only delivery that is user-facing only through its demo.
    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["branch", "-D", "factory/test-agent"]);
    git(&repo, &["checkout", "-q", "-b", "factory/test-agent"]);
    commit_file(&repo, "src/ops.rs", "pub fn ops() {}\n", "ops tweak");
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(&task_id).unwrap();
    task.demo_statement = "Run the ops job and see it finish".to_string();
    tasks.update(&task).unwrap();

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let qa_task = qa_task_id(&cas_dir, &task_id);
    // The domdms shape: the task is back in progress with the round pending.
    reopened_to_in_progress(&cas_dir, &task_id);

    let request: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "update",
        "id": task_id,
        "demo_statement": "",
    }))
    .unwrap();
    let updated = match CasService::new(core.clone(), None).task(Parameters(request)).await {
        Ok(result) => extract_text(result),
        Err(error) => error.message.to_string(),
    };
    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert_eq!(passes.len(), 1, "{updated}");
    assert!(passes[0].is_withdrawn(), "{updated}\n{:?}", passes[0]);
    assert!(tasks.get(&qa_task).unwrap().is_terminal(), "its QA work item is cancelled");
    assert!(
        cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, "git merge factory/test-agent")
            .is_none(),
        "a withdrawn round no longer gates the merge"
    );
}

/// cas-5c38: a no-code task with no code has no delivery for a reviewer to
/// walk, so the independent QA gate never binds it, even with a demo
/// statement. The domdms shape: the branch tip sits on its base, and the
/// branch is not rebuilt.
#[tokio::test]
async fn no_code_tasks_without_code_are_never_gated_by_independent_qa_cas_5c38() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    // No commits: the worker's branch is its base.
    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["branch", "-f", "factory/test-agent", "main"]);
    // Someone else's user-facing merge lands on the target afterwards; it
    // must not be mistaken for this task's delivery.
    git(&repo, &["checkout", "-q", "-b", "factory/other"]);
    commit_file(&repo, "web/other.css", ".other{}\n", "other worker's UI");
    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["merge", "-q", "--no-ff", "-m", "merge other", "factory/other"]);
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(&task_id).unwrap();
    task.demo_statement = "The ops dashboard shows the new tenant".to_string();
    task.execution_note = Some("no-code".to_string());
    task.external_ref = Some("https://example.test/ops-receipt".to_string());
    tasks.update(&task).unwrap();

    let text = close_text(&core, &task_id).await;
    assert!(!text.contains("INDEPENDENT QA"), "{text}");
    assert!(!text.contains("MERGE REQUIRED"), "{text}");
    assert!(cas_store::list_qa_passes(&cas_dir, &task_id).unwrap().is_empty(), "{text}");

    // A round an older Cassy opened for it: a supervisor's qa_waive
    // withdraws it although there is no delivery tip.
    let now = chrono::Utc::now();
    cas_store::open_qa_pass(
        &cas_dir,
        &cas_store::NewQaPass {
            task_id: &task_id,
            implementer_agent_id: "test-agent",
            branch: "factory/test-agent",
            bound_head: "aaaa1111bbbb2222",
            deadline_at: now + chrono::Duration::minutes(30),
            max_rounds: 3,
        },
        now,
    )
    .unwrap();
    let supervisor = supervisor_core(&cas_dir);
    let _role = SupervisorRole::enter();
    let waived = extract_text(
        CasService::new(supervisor, None)
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_waive",
                "task_id": task_id,
                "summary": "ops change verified on the dashboard",
            }))))
            .await
            .expect("qa_waive accepts a no-code task with no delivery tip"),
    );
    assert!(waived.contains("no-code"), "{waived}");
    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert!(passes[0].is_withdrawn(), "{waived}");
}

/// cas-2387: declaring a task no-code never hides user-facing code. A task
/// that kept `execution_note=no-code` (with its external_ref proof) while its
/// branch carries a UI change is sent for independent review at the park,
/// and a raw merge stays blocked until the verdict.
#[tokio::test]
async fn no_code_task_carrying_user_facing_code_is_still_reviewed_cas_2387() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(&task_id).unwrap();
    task.execution_note = Some("no-code".to_string());
    task.external_ref = Some("https://example.test/ops-receipt".to_string());
    tasks.update(&task).unwrap();

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    assert!(parked.contains("path:web/composer.css"), "{parked}");
    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert_eq!(passes.len(), 1);
    assert!(passes[0].state.is_active());
    let guard = cas::qa_pass::supervisor_merge_refusal(
        &cas_dir,
        &repo,
        "git merge --no-ff factory/test-agent",
    )
    .expect("the no-code declaration must not unblock an unreviewed UI merge");
    assert!(guard.contains(&qa_task_id(&cas_dir, &task_id)), "{guard}");
}

#[tokio::test]
async fn backend_only_deliveries_are_untouched_and_qa_type_on_add_is_refused() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, _) = fixture(&mut test_env);
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
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, _) = fixture(&mut test_env);
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
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    close_text(&core, &task_id).await;
    let service = CasService::new(core.clone(), None);

    let previous_role = std::env::var("CAS_AGENT_ROLE").ok();
    // SAFETY: TestEnvGuard is held for the whole test body.
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
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
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

/// cas-ce39: a re-park at a new tip while a reviewer holds the round used to
/// supersede it silently: the reviewer kept reviewing a dead head and the old
/// QA task stayed open. Now the old round's work item is cancelled, pointing
/// at the new one, the reviewer is told to stop, and the park says so.
#[tokio::test]
async fn a_new_tip_supersedes_a_claimed_round_and_tells_its_reviewer_cas_ce39() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let round1 = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .expect("round one");
    let old_qa_task = round1.qa_task_id.clone().expect("its work item");
    let reviewer = reviewer_core(&cas_dir, "qa-reviewer");
    reviewer
        .cas_task_start(Parameters(IdRequest { id: old_qa_task.clone() }))
        .await
        .expect("the reviewer claims round one");

    // The implementer pushes a new tip and parks again.
    let new_head = commit_file(&repo, "web/composer.css", ".composer{gap:12px}\n", "wider gap");
    let reparked = close_text(&core, &task_id).await;
    assert!(reparked.contains("INDEPENDENT QA DISPATCHED"), "{reparked}");
    assert!(reparked.contains("SUPERSEDED"), "{reparked}");
    assert!(reparked.contains("claimed by qa-reviewer"), "{reparked}");
    assert!(reparked.contains("Reviewer qa-reviewer told to stop"), "{reparked}");

    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    let old = passes.iter().find(|pass| pass.id == round1.id).unwrap();
    assert_eq!(old.state, cas_types::QaPassState::Superseded);
    let current = passes
        .iter()
        .find(|pass| pass.state.is_active())
        .expect("one open round for the new tip");
    assert_eq!(current.bound_head, new_head);
    let new_qa_task = current.qa_task_id.clone().expect("the new round has its work item");
    assert!(reparked.contains(&new_qa_task), "{reparked}");

    // The old work item is cancelled and points at the new one.
    let tasks = open_task_store(&cas_dir).unwrap();
    let cancelled = tasks.get(&old_qa_task).unwrap();
    assert_eq!(cancelled.status, TaskStatus::Cancelled);
    assert_eq!(
        cancelled.terminal_outcome,
        Some(cas_types::TaskTerminalOutcome::Cancelled {
            superseded_by: Some(new_qa_task.clone()),
        })
    );
    assert!(
        cancelled.close_reason.as_deref().unwrap_or_default().contains("superseded"),
        "{:?}",
        cancelled.close_reason
    );

    // The reviewer is messaged with the new round.
    let queued = open_prompt_queue_store(&cas_dir).unwrap().peek_all(100).unwrap();
    let notice = queued
        .iter()
        .find(|row| row.source == format!("qa-dispatch:superseded:{}", round1.id))
        .expect("superseded notice queued for the reviewer");
    assert_eq!(notice.target, "qa-reviewer");
    assert!(notice.prompt.contains("STOP reviewing"), "{}", notice.prompt);
    assert!(notice.prompt.contains(&new_qa_task), "{}", notice.prompt);
    assert!(notice.prompt.contains(&current.id), "{}", notice.prompt);
}

/// cas-ce39, pending case: an unclaimed round for the old tip is retired the
/// same way (its work item cancelled, pointing at the new one) and nobody is
/// messaged, because nobody had started it.
#[tokio::test]
async fn a_new_tip_supersedes_a_pending_round_without_messaging_anyone_cas_ce39() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let round1 = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .expect("round one");
    let old_qa_task = round1.qa_task_id.clone().expect("its work item");

    commit_file(&repo, "web/composer.css", ".composer{gap:12px}\n", "wider gap");
    let reparked = close_text(&core, &task_id).await;
    assert!(reparked.contains("SUPERSEDED"), "{reparked}");
    assert!(reparked.contains("pending, unclaimed"), "{reparked}");
    assert!(!reparked.contains("told to stop"), "{reparked}");

    let tasks = open_task_store(&cas_dir).unwrap();
    assert_eq!(tasks.get(&old_qa_task).unwrap().status, TaskStatus::Cancelled);
    let queued = open_prompt_queue_store(&cas_dir).unwrap().peek_all(100).unwrap();
    assert!(
        !queued
            .iter()
            .any(|row| row.source == format!("qa-dispatch:superseded:{}", round1.id)),
        "no reviewer to message for an unclaimed round"
    );
    // Re-parking the same new tip again changes nothing further.
    let again = close_text(&core, &task_id).await;
    assert!(again.contains("INDEPENDENT QA PENDING"), "{again}");
    assert!(!again.contains("SUPERSEDED"), "{again}");
}

/// Points `gh` at a test double for the scope of a test holding
/// `TestEnvGuard`, and restores the previous value on drop.
struct GhStub {
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl GhStub {
    fn install(dir: &Path, head_sha: &str) -> (Self, std::path::PathBuf) {
        let gh = dir.join("gh");
        let log = dir.join("gh.log");
        // `gh pr view 2546` knows PR #2546 (head factory/test-agent); every
        // other lookup fails like an unknown PR. `gh api --method POST` is a
        // status publication and succeeds. Every call is logged.
        cas::test_paths::warm_stub(
            &gh,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$CAS_TEST_GH_LOG"
if [ "$1" = "pr" ] && [ "$2" = "view" ] && [ "$3" = "2546" ]; then
  printf '{"headRefName":"factory/test-agent","headRefOid":"%s"}' "$CAS_TEST_GH_HEAD"
  exit 0
fi
if [ "$1" = "api" ] && [ "$2" = "--method" ] && [ "$3" = "POST" ]; then
  printf '{}'
  exit 0
fi
exit 1
"#,
        );
        let mut previous = Vec::new();
        for (key, value) in [
            ("CAS_QA_GH", gh.as_os_str().to_owned()),
            ("CAS_TEST_GH_LOG", log.as_os_str().to_owned()),
            ("CAS_TEST_GH_HEAD", head_sha.into()),
        ] {
            previous.push((key, std::env::var_os(key)));
            // SAFETY: callers hold TestEnvGuard for the whole test body.
            unsafe { std::env::set_var(key, value) };
        }
        (Self { previous }, log)
    }
}

impl Drop for GhStub {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            // SAFETY: as in `install`.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

/// Wait for the background status publisher to log a line with every needle.
fn wait_for_gh_call(log: &Path, needles: &[&str]) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let calls = std::fs::read_to_string(log).unwrap_or_default();
        if calls
            .lines()
            .any(|line| needles.iter().all(|needle| line.contains(needle)))
        {
            return calls;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no gh call with {needles:?}; calls so far:\n{calls}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// cas-2ee2 (GH #1023 finding 3): cas-a6cf's PR #2546 was merged with
/// `gh pr merge` before the supervisor read its QA dispatch. Every raw GitHub
/// merge path is now held to the same verdict as `worktree_merge`, for every
/// role; the merge request leads with the QA hold; and with
/// `qa.github_status` on, the repository-side required check follows the
/// round (pending → success on waiver).
#[tokio::test]
async fn raw_github_merges_wait_for_the_independent_verdict_cas_2ee2() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    let config = cas_dir.join("config.toml");
    let body = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, format!("{body}github_status = true\n")).unwrap();
    let head = git(&repo, &["rev-parse", "factory/test-agent"]);
    let stub_dir = repo.join("stub-bin");
    std::fs::create_dir_all(&stub_dir).unwrap();
    let (_gh, gh_log) = GhStub::install(&stub_dir, &head);

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let qa_task = qa_task_id(&cas_dir, &task_id);
    // Repository side: the open round is a pending required check on the
    // delivered head.
    wait_for_gh_call(
        &gh_log,
        &[
            &format!("statuses/{head}"),
            "state=pending",
            "context=cassy/independent-qa",
        ],
    );

    let raw_merges = [
        // Resolved through GitHub: PR #2546's head is the delivery branch.
        "gh pr merge 2546 --squash --delete-branch",
        "gh pr merge https://github.com/acme/gabber/pull/2546 --merge",
        "gh api -X PUT repos/acme/gabber/pulls/2546/merge -f merge_method=squash",
        // Named by branch, or the checked-out branch's PR (auto-merge too).
        "gh pr merge factory/test-agent --rebase",
        "gh pr merge --auto --squash",
    ];
    for command in raw_merges {
        let supervisor = cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, command)
            .unwrap_or_else(|| panic!("supervisor merge allowed: {command}"));
        assert!(
            supervisor.contains(&task_id)
                && supervisor.contains(&qa_task)
                && supervisor.contains("raw GitHub merge"),
            "{command}: {supervisor}"
        );
        assert!(
            cas::qa_pass::github_merge_refusal(&cas_dir, &repo, command).is_some(),
            "any role's merge allowed: {command}"
        );
    }

    // A PR Cassy cannot map, and a GraphQL merge by node id, are refused
    // while a round is open rather than waved through.
    for command in [
        "gh pr merge 77 --squash",
        "gh api graphql -f query='mutation { mergePullRequest(input: {pullRequestId: \"PR_kw\"}) { clientMutationId } }'",
    ] {
        let refusal = cas::qa_pass::github_merge_refusal(&cas_dir, &repo, command)
            .unwrap_or_else(|| panic!("unmapped merge allowed: {command}"));
        assert!(
            refusal.contains("cannot tell which delivery") && refusal.contains(&task_id),
            "{command}: {refusal}"
        );
    }

    // A PR number the worker reported binds the delivery without GitHub.
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(&task_id).unwrap();
    task.deliverables.delivery_pr_number = Some(3000);
    tasks.update(&task).unwrap();
    let recorded = cas::qa_pass::github_merge_refusal(&cas_dir, &repo, "gh pr merge 3000")
        .expect("the recorded PR must wait for QA");
    assert!(recorded.contains(&qa_task), "{recorded}");

    // The merge request the supervisor reads leads with the QA hold.
    let hold = cas::qa_pass::merge_request_qa_hold(&cas_dir, &tasks.get(&task_id).unwrap(), &head)
        .expect("an open round holds the merge request");
    assert!(hold.starts_with("⏸ QA HOLD"), "{hold}");
    assert!(hold.contains(&qa_task) && hold.contains("gh pr merge"), "{hold}");

    // A logged supervisor waiver opens every path for exactly this head.
    let _role = SupervisorRole::enter();
    let waived = extract_text(
        CasService::new(core.clone(), None)
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_waive",
                "task_id": task_id,
                "summary": "copy-only hotfix reviewed live with the operator",
            }))))
            .await
            .expect("supervisor waiver"),
    );
    assert!(waived.contains("waived"), "{waived}");
    wait_for_gh_call(
        &gh_log,
        &[
            &format!("statuses/{head}"),
            "state=success",
            "waived by supervisor: copy-only hotfix",
        ],
    );
    for command in raw_merges.iter().copied().chain(["gh pr merge 3000"]) {
        assert!(
            cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, command).is_none(),
            "the waived head may merge: {command}"
        );
    }
    assert!(
        cas::qa_pass::merge_request_qa_hold(&cas_dir, &tasks.get(&task_id).unwrap(), &head)
            .is_none()
    );

    // A commit pushed after the waiver is not covered.
    let drift = commit_file(&repo, "web/composer.css", "/* drift */\n", "unreviewed drift");
    assert_ne!(drift, head);
    assert!(
        cas::qa_pass::github_merge_refusal(&cas_dir, &repo, "gh pr merge 3000").is_some(),
        "the recorded PR's new head has no verdict"
    );
}

/// cas-74284, the cas-470e shape: the worker's `factory/<name>` is frozen for
/// an earlier parked task, so this delivery lives on its per-task branch and
/// carries no demo_statement. 3.33.0 measured `factory/<name>`, found no
/// user-facing path and dispatched nothing at the park. The park must judge
/// the per-task branch's own diff.
#[tokio::test]
async fn per_task_branch_park_without_a_demo_dispatches_from_its_own_diff_cas_74284() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["branch", "-f", "factory/test-agent", "main"]);
    git(
        &repo,
        &[
            "checkout",
            "-q",
            "-b",
            &format!("factory/test-agent-{task_id}"),
            "main",
        ],
    );
    let head = commit_file(
        &repo,
        "web/composer.css",
        ".composer{gap:8px}\n",
        "composer spacing",
    );
    assert!(
        open_task_store(&cas_dir)
            .unwrap()
            .get(&task_id)
            .unwrap()
            .demo_statement
            .is_empty(),
        "the shape under test has no demo_statement"
    );

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    assert!(parked.contains("path:web/composer.css"), "{parked}");
    let pass = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .expect("the park opened a round");
    assert_eq!(pass.bound_head, head, "the round binds the per-task tip");
}

/// cas-74284: a task labelled per `qa.user_facing_labels` (now including
/// `hub-web`, the label cas-470e carried) cannot be created without a
/// demo_statement.
#[tokio::test]
async fn hub_web_labelled_task_needs_a_demo_statement_at_create_cas_74284() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, _repo, _task_id) = fixture(&mut test_env);
    let _keep = &temp;
    let service = CasService::new(core.clone(), None);
    let create = |demo: Option<&str>| {
        let mut request = serde_json::json!({
            "action": "create",
            "title": "hub-web: settled card offers Send again",
            "labels": "hub-web,qa-followup",
            "risk": "none",
        });
        if let Some(demo) = demo {
            request["demo_statement"] = serde_json::json!(demo);
        }
        serde_json::from_value::<cas_mcp::TaskRequest>(request).unwrap()
    };
    let refused = service
        .task(Parameters(create(None)))
        .await
        .expect_err("a hub-web task without a demo_statement is refused");
    assert!(
        refused.message.contains("TASK CREATE REJECTED") && refused.message.contains("hub-web"),
        "{}",
        refused.message
    );
    service
        .task(Parameters(create(Some(
            "As an operator, I tap Send again on a settled card and see my message resent",
        ))))
        .await
        .expect("with a demo_statement it is created");
}

/// cas-74284: a supervisor can open an independent QA round for a parked
/// delivery the park did not judge user-facing (no demo_statement, no
/// surface path), which the delivery-proof scope lock otherwise leaves with
/// no route to a review before merge.
#[tokio::test]
async fn supervisor_can_request_independent_qa_for_a_parked_delivery_cas_74284() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["branch", "-D", "factory/test-agent"]);
    git(&repo, &["checkout", "-q", "-b", "factory/test-agent"]);
    let head = commit_file(&repo, "src/send.rs", "pub fn resend() {}\n", "resend");

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert!(!parked.contains("INDEPENDENT QA"), "{parked}");
    assert!(
        cas_store::list_qa_passes(&cas_dir, &task_id)
            .unwrap()
            .is_empty()
    );
    let merge_cmd = "git merge --no-ff factory/test-agent";
    assert!(cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, merge_cmd).is_none());

    let service = CasService::new(core.clone(), None);
    let request = |task: &str, summary: &str| {
        verification(serde_json::json!({
            "action": "qa_request",
            "task_id": task,
            "summary": summary,
        }))
    };
    let worker = service
        .verification(Parameters(request(&task_id, "please review")))
        .await
        .expect_err("qa_request is supervisor-only");
    assert!(
        worker.message.contains("supervisor-only"),
        "{}",
        worker.message
    );

    let _role = SupervisorRole::enter();
    let no_reason = service
        .verification(Parameters(request(&task_id, "   ")))
        .await
        .expect_err("a request needs a reason");
    assert!(
        no_reason.message.contains("summary"),
        "{}",
        no_reason.message
    );

    let tasks = open_task_store(&cas_dir).unwrap();
    let mut unparked = cas::types::Task::new("cas-ui02".to_string(), "Unparked".to_string());
    unparked.status = TaskStatus::InProgress;
    unparked.assignee = Some("test-agent".to_string());
    tasks.add(&unparked).unwrap();
    let not_parked = service
        .verification(Parameters(request("cas-ui02", "please review")))
        .await
        .expect_err("only a parked delivery can be requested");
    assert!(
        not_parked.message.contains("not parked"),
        "{}",
        not_parked.message
    );

    let requested = extract_text(
        service
            .verification(Parameters(request(
                &task_id,
                "changes the Commander resend path users see",
            )))
            .await
            .expect("supervisor request opens a round"),
    );
    assert!(
        requested.contains("INDEPENDENT QA DISPATCHED"),
        "{requested}"
    );
    let pass = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .expect("a round is open");
    assert_eq!(pass.bound_head, head);
    let qa_task = tasks.get(pass.qa_task_id.as_deref().unwrap()).unwrap();
    assert!(
        qa_task
            .description
            .contains("requested by supervisor: changes the Commander resend path users see"),
        "{}",
        qa_task.description
    );
    assert!(
        open_prompt_queue_store(&cas_dir)
            .unwrap()
            .peek_all(50)
            .unwrap()
            .iter()
            .any(|row| row.source == format!("qa-dispatch:{}", pass.id)),
        "the supervisor is handed the QA dispatch"
    );
    assert!(
        tasks
            .get(&task_id)
            .unwrap()
            .notes
            .contains("Independent QA requested by supervisor"),
        "the request is a logged decision"
    );
    // From now on the merge waits for the round's verdict.
    assert!(cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, merge_cmd).is_some());

    // Asking again is idempotent for the same tip.
    let again = extract_text(
        service
            .verification(Parameters(request(&task_id, "still needed")))
            .await
            .expect("repeat request"),
    );
    assert!(again.contains("INDEPENDENT QA PENDING"), "{again}");
    assert_eq!(
        cas_store::list_qa_passes(&cas_dir, &task_id).unwrap().len(),
        1
    );
}

/// cas-d5c1 (GH #1023 finding 6): a project's QA preflight runs when the
/// reviewer starts the QA work item. A missing env file refuses the start as
/// a blocker, so the round is not claimed and its deadline is not spent. Once
/// the environment is ready the start claims the round and reports the
/// preflight, including the capacity hook's line. No secret value appears in
/// the response or the recorded note.
#[tokio::test]
async fn qa_preflight_blocks_an_unready_reviewer_and_reports_a_ready_one_cas_d5c1() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    const VAR: &str = "CAS_TEST_QA_BACKEND_ENV_FILE";
    struct Unset;
    impl Drop for Unset {
        fn drop(&mut self) {
            // SAFETY: the test holds TestEnvGuard for its whole body.
            unsafe { std::env::remove_var(VAR) };
        }
    }
    let _unset = Unset;
    // SAFETY: as above.
    unsafe { std::env::remove_var(VAR) };
    let config = cas_dir.join("config.toml");
    let body = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        format!(
            "{body}preflight_env_files = [\"{VAR}\"]\n\
             preflight_hook = \"echo qa-staging-creator topped up for $CAS_QA_DELIVERY_TASK\"\n"
        ),
    )
    .unwrap();

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let qa_task = qa_task_id(&cas_dir, &task_id);
    let reviewer = reviewer_core(&cas_dir, "qa-reviewer");

    let blocked = reviewer
        .cas_task_start(Parameters(IdRequest {
            id: qa_task.clone(),
        }))
        .await
        .expect_err("an unready reviewer environment is a blocker");
    assert!(
        blocked.message.contains("QA PREFLIGHT BLOCKED"),
        "{}",
        blocked.message
    );
    assert!(
        blocked.message.contains(&format!("{VAR} is not set")),
        "{}",
        blocked.message
    );
    assert!(
        blocked.message.contains("blocker=true"),
        "{}",
        blocked.message
    );
    let pass = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .unwrap();
    assert_eq!(
        pass.state,
        cas::types::QaPassState::Pending,
        "the round was not claimed"
    );
    assert!(pass.reviewer_agent_id.is_none());
    let tasks = open_task_store(&cas_dir).unwrap();
    assert_ne!(tasks.get(&qa_task).unwrap().status, TaskStatus::InProgress);

    // The operator exports the path; the file holds secrets that must not
    // surface anywhere.
    let env_file = repo.join("backend.env");
    std::fs::write(&env_file, "STAGING_API_SECRET=never-print-this-value\n").unwrap();
    // SAFETY: as above.
    unsafe { std::env::set_var(VAR, &env_file) };
    let started = extract_text(
        reviewer
            .cas_task_start(Parameters(IdRequest {
                id: qa_task.clone(),
            }))
            .await
            .expect("a ready reviewer starts and claims the round"),
    );
    assert!(started.contains("claimed"), "{started}");
    assert!(started.contains("QA preflight"), "{started}");
    assert!(
        started.contains(&format!("READY env file {VAR}")),
        "{started}"
    );
    assert!(
        started.contains(&format!("qa-staging-creator topped up for {task_id}")),
        "{started}"
    );
    let notes = tasks.get(&qa_task).unwrap().notes;
    assert!(
        notes.contains("QA preflight for round 1") && notes.contains("BLOCKED"),
        "{notes}"
    );
    assert!(notes.contains("ready"), "{notes}");
    for text in [started.as_str(), notes.as_str(), &*blocked.message] {
        assert!(
            !text.contains("never-print-this-value"),
            "secret leaked: {text}"
        );
    }
}
