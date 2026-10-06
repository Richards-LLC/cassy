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
    fixture_with_project(test_env, None)
}

fn fixture_with_project(
    test_env: &mut TestEnvGuard,
    canonical_id: Option<&str>,
) -> (tempfile::TempDir, CasCore, std::path::PathBuf, String) {
    let (temp, core) = setup_cas(test_env);
    let repo = temp.path().to_path_buf();
    let cas_dir = repo.join(".cas");
    // Pin before creating any tasks: the store stamps their origin at creation.
    let project_config = canonical_id
        .map(|id| format!("\n[project]\ncanonical_id = {id:?}\n"))
        .unwrap_or_default();
    std::fs::write(
        cas_dir.join("config.toml"),
        format!(
            // The implementer's own evidence gate (cas-0cd5) is covered by
            // qa_evidence_gate.rs; these tests exercise the independent pass.
            "[factory]\nartifacts_root = {:?}\n[verification]\nenabled = false\n[qa]\nevidence_gate = false\n{project_config}",
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
    if let Some(id) = canonical_id {
        assert_eq!(
            tasks.get(&task_id).unwrap().origin_project.as_deref(),
            Some(id)
        );
    }
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
    pre_existing_follow_up(&mut test_env, false).await;
}

#[tokio::test]
async fn qa_record_follow_up_close_targets_the_open_epic_cas_1980() {
    let mut test_env = TestEnvGuard::temp_home();
    pre_existing_follow_up(&mut test_env, true).await;
}

#[tokio::test]
async fn qa_record_files_ledger_findings_without_duplicates_cas_2849() {
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let tasks = open_task_store(&cas_dir).unwrap();
    tasks.add(&cas::types::Task::new("cas-2a33".to_string(), "Existing fixture defect".to_string())).unwrap();
    assert!(close_text(&core, &task_id).await.contains("INDEPENDENT QA DISPATCHED"));
    let round_task = qa_task_id(&cas_dir, &task_id);
    let reviewer = reviewer_core(&cas_dir, "ledger-reviewer");
    reviewer.cas_task_start(Parameters(IdRequest { id: round_task.clone() })).await.unwrap();
    let service = CasService::new(reviewer, None);
    let head = git(&repo, &["rev-parse", "factory/test-agent"]);
    let ledger = round_evidence(&repo.join("round-1"), &task_id, &head);
    std::fs::write(&ledger, "# QA ledger\n## Pre-existing / limitations\nF10 NORMAL: Native fixture mismatch; existing follow-up cas-2a33.\nF11 NORMAL: Strict contrast capture instability.\n").unwrap();
    let record = |issues: serde_json::Value| verification(serde_json::json!({
        "action": "qa_record", "task_id": task_id, "status": "approved",
        "summary": "delivery passes; older defects are tracked separately",
        "ledger_path": ledger.display().to_string(), "issues": issues.to_string(),
    }));
    let error = service.verification(Parameters(record(serde_json::json!([
        {"id":"F99", "scope":"pre-existing"}
    ])))).await.expect_err("a finding without problem text must not resolve the round");
    assert!(error.message.contains("F99") && error.message.contains("problem"), "{}", error.message);
    assert_eq!(cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now()).unwrap().unwrap().state,
        cas::types::QaPassState::Claimed);
    assert_ne!(tasks.get(&round_task).unwrap().status, TaskStatus::Closed);
    assert!(!tasks.list(None).unwrap().iter().any(|task| task.labels.iter().any(|label| label == "qa-follow-up")));

    let result = extract_text(service.verification(Parameters(record(serde_json::json!([
        {"id":"F10", "scope":"pre-existing"}, {"id":"F11", "scope":"pre-existing"}
    ])))).await.unwrap());
    assert!(result.contains("Already tracked, not filed again: F10 by cas-2a33"), "{result}");
    let follow_ups: Vec<_> = tasks.list(None).unwrap().into_iter()
        .filter(|task| task.labels.iter().any(|label| label == "qa-follow-up")).collect();
    assert_eq!(follow_ups.len(), 1, "F10 already has a task; only F11 is filed");
    assert!(follow_ups[0].title.contains("Strict contrast capture instability"));
    assert!(follow_ups[0].description.contains("Problem: Strict contrast capture instability."));
    assert!(!follow_ups[0].description.contains("(not described)"));
    assert!(tasks.get_dependencies("cas-2a33").unwrap().iter()
        .any(|dep| dep.to_id == task_id && dep.dep_type == DependencyType::Related));
    assert_eq!(tasks.get(&round_task).unwrap().status, TaskStatus::Closed);
}

async fn pre_existing_follow_up(test_env: &mut TestEnvGuard, under_epic: bool) {
    let (temp, core, repo, task_id) =
        fixture_with_project(test_env, under_epic.then_some("qa-follow-up-fixture"));
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    let tasks = open_task_store(&cas_dir).unwrap();
    let delivery_origin = tasks.get(&task_id).unwrap().origin_project;

    let epic_branch = "epic/qa-follow-ups";
    if under_epic {
        git(&repo, &["branch", epic_branch, "main"]);
        for (id, status) in [
            ("cas-old-epic", TaskStatus::Closed),
            ("cas-live-epic", TaskStatus::Open),
        ] {
            let mut epic = cas::types::Task::new(id.to_string(), "QA fixture epic".to_string());
            epic.task_type = cas::types::TaskType::Epic;
            epic.status = status;
            epic.branch = Some(epic_branch.to_string());
            epic.delivery_mode = cas::types::DeliveryMode::LocalMerge;
            epic.deliverables.work_target = Some(cas::types::WorkTarget {
                repo_selector: "project:qa-follow-up-fixture".to_string(),
                target_branch: "main".to_string(),
            });
            tasks.add(&epic).unwrap();
            tasks
                .add_dependency(&cas::types::Dependency::new(
                    task_id.clone(),
                    id.to_string(),
                    DependencyType::ParentChild,
                ))
                .unwrap();
        }
    }
    let initial_park = close_text(&core, &task_id).await;
    assert!(initial_park.contains("INDEPENDENT QA DISPATCHED"), "{initial_park}");
    let round_task = qa_task_id(&cas_dir, &task_id);
    assert_eq!(tasks.get(&round_task).unwrap().origin_project, delivery_origin);
    let reviewer = reviewer_core(&cas_dir, "qa-reviewer");
    let reviewer_service = CasService::new(reviewer.clone(), None);
    reviewer
        .cas_task_start(Parameters(IdRequest {
            id: round_task.clone(),
        }))
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
    assert!(
        refused.message.contains("never rejects a delivery"),
        "{}",
        refused.message
    );
    assert_eq!(
        tasks.get(&task_id).unwrap().status,
        TaskStatus::AwaitingMerge
    );
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
    assert!(
        approved.contains("Pre-existing follow-ups filed"),
        "{approved}"
    );
    let follow_ups: Vec<_> = tasks
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|task| task.labels.iter().any(|label| label == "qa-follow-up"))
        .collect();
    assert_eq!(follow_ups.len(), 1, "one follow-up per pre-existing issue");
    let follow_up = &follow_ups[0];
    assert_eq!(follow_up.origin_project, delivery_origin);
    assert_eq!(
        follow_up.title,
        format!("Pre-existing: Footer links fail contrast at 3.1:1 (found in QA of {task_id})")
    );
    assert!(
        follow_up.description.contains("use --ink-mid"),
        "{}",
        follow_up.description
    );
    assert!(
        follow_up
            .description
            .contains(&ledger.display().to_string())
    );
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
    if under_epic {
        assert_eq!(
            tasks.get_parent_epic(&follow_up.id).unwrap().unwrap().id,
            "cas-live-epic"
        );
        let target = follow_up
            .deliverables
            .work_target
            .as_ref()
            .expect("durable work target");
        assert_eq!(target.repo_selector, "project:qa-follow-up-fixture");
        assert_eq!(target.target_branch, epic_branch);
        assert_eq!(
            follow_up.delivery_mode,
            cas::types::DeliveryMode::LocalMerge
        );
        assert!(
            follow_up.assignee.is_none(),
            "QA does not assign its implementer"
        );

        // Isolate the follow-up's docs-only change from the reviewed UI branch.
        let branch = format!("factory/test-agent-{}", follow_up.id);
        git(&repo, &["checkout", "-q", "-b", &branch, epic_branch]);
        let head = commit_file(
            &repo,
            "docs/follow-up.md",
            "follow-up fix\n",
            &format!("fix({}): follow-up", follow_up.id),
        );
        core.cas_task_start(Parameters(IdRequest {
            id: follow_up.id.clone(),
        }))
        .await
        .unwrap();
        let mut request = close_req(&follow_up.id);
        request.commit_receipt = Some(head.clone());
        let parked = extract_text(core.cas_task_close(Parameters(request)).await.unwrap());
        assert!(parked.contains("MERGE REQUIRED"), "{parked}");
        assert!(parked.contains(epic_branch), "{parked}");
        let stored = tasks.get(&follow_up.id).unwrap();
        assert_eq!(stored.status, TaskStatus::AwaitingMerge);
        assert_eq!(
            stored.deliverables.parked_branch.as_deref(),
            Some(branch.as_str())
        );
        assert_eq!(
            stored.deliverables.factory_branch_anchor.as_deref(),
            Some(head.as_str())
        );
        assert_eq!(
            stored
                .deliverables
                .work_target
                .as_ref()
                .unwrap()
                .target_branch,
            epic_branch
        );
    } else {
        assert!(tasks.get_parent_epic(&follow_up.id).unwrap().is_none());
        assert!(follow_up.deliverables.work_target.is_none());
    }
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
    // cas-de60: an unparked merged delivery needs its own lineage; the
    // shared worker lane is not authoritative delivery evidence.
    let branch = format!("factory/test-agent-{task_id}");
    git(&repo, &["checkout", "-q", "-b", &branch]);
    let delivered = git(&repo, &["rev-parse", "HEAD"]);
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
    git(&repo, &["merge", "-q", "--no-ff", "-m", "merge", &branch]);
    git(&repo, &["checkout", "-q", &branch]);

    let refused = close_text(&core, &task_id).await;
    assert!(refused.contains("INDEPENDENT QA REQUIRED"), "{refused}");
    assert!(refused.contains("INDEPENDENT QA DISPATCHED"), "{refused}");
    assert!(refused.contains("the close waits for its verdict"), "{refused}");
    assert_eq!(tasks.get(&task_id).unwrap().status, TaskStatus::InProgress);
    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0].branch, branch);
    assert_eq!(passes[0].bound_head, delivered);
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
        &format!("git merge --no-ff --no-commit {branch}"),
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

struct SupervisorRole;

impl SupervisorRole {
    fn enter(env: &mut TestEnvGuard) -> ScopedFactoryEnv<'_> {
        ScopedFactoryEnv::apply(env, &[("CAS_AGENT_ROLE", Some("supervisor"))])
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
    // cas-de60: identify the delivery before its first (post-merge) close.
    let branch = format!("factory/test-agent-{task_id}");
    git(&repo, &["checkout", "-q", "-b", &branch]);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(&task_id).unwrap();
    task.demo_statement = "Open the composer and see even spacing".to_string();
    tasks.update(&task).unwrap();

    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["merge", "-q", "--no-ff", "-m", "merged in May", &branch]);
    let merged = git(&repo, &["rev-parse", &branch]);
    git(&repo, &["checkout", "-q", &branch]);

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
    let _role = SupervisorRole::enter(&mut test_env);
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
    let _role = SupervisorRole::enter(&mut test_env);
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
async fn cas_dd29_waives_an_explicit_pushed_tip_before_park() {
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, _core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare"]);
    git(
        &repo,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "factory/test-agent"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get(&task_id).unwrap();
    let agent = open_agent_store(&cas_dir)
        .unwrap()
        .list(None)
        .unwrap()
        .into_iter()
        .find(|agent| agent.name == "test-agent")
        .unwrap();
    task.assignee = Some(agent.id);
    tasks.update(&task).unwrap();
    let service = CasService::new(supervisor_core(&cas_dir), None);
    let definition = service
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "verification")
        .unwrap();
    assert!(
        definition.input_schema["properties"]
            .get("head_sha")
            .is_some()
    );
    let _role = SupervisorRole::enter(&mut test_env);
    let result = service
        .verification(Parameters(verification(serde_json::json!({
            "action": "qa_waive", "task_id": task_id,
            "head_sha": head, "summary": "operator reviewed the pushed delivery",
        }))))
        .await
        .expect("a pushed delivery can be waived without parking first");
    assert!(extract_text(result).contains("waived"));
    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0].state, cas::types::QaPassState::Waived);
    assert_eq!(passes[0].bound_head, head);
    let task = open_task_store(&cas_dir).unwrap().get(&task_id).unwrap();
    assert_eq!(task.status, TaskStatus::InProgress);
    assert!(task.deliverables.factory_branch_anchor.is_none());
    assert!(task.notes.contains("operator reviewed the pushed delivery"));
}

#[tokio::test]
async fn cas_dd29_refuses_a_head_that_is_not_the_pushed_branch_tip() {
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, _core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare"]);
    git(
        &repo,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "factory/test-agent"]);
    let pushed = git(&repo, &["rev-parse", "HEAD"]);
    let unpushed = commit_file(&repo, "web/new.css", "a{color:red}\n", "unpushed work");
    // A stale/misleading tracking ref is not proof of what origin carries.
    git(
        &repo,
        &[
            "update-ref",
            "refs/remotes/origin/factory/test-agent",
            &unpushed,
        ],
    );
    let service = CasService::new(supervisor_core(&cas_dir), None);
    let _role = SupervisorRole::enter(&mut test_env);
    for head in [&unpushed, &git(&repo, &["rev-parse", "main"])] {
        let error = service
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_waive", "task_id": task_id,
                "head_sha": head, "summary": "reviewed",
            }))))
            .await
            .expect_err("only the exact pushed tip may be waived");
        assert!(error.message.contains("head_sha"), "{}", error.message);
        assert!(error.message.contains(&pushed), "{}", error.message);
        assert!(
            cas_store::list_qa_passes(&cas_dir, &task_id)
                .unwrap()
                .is_empty()
        );
    }
    git(&repo, &["remote", "remove", "origin"]);
    let error = service
        .verification(Parameters(verification(serde_json::json!({
            "action": "qa_waive", "task_id": task_id,
            "head_sha": pushed, "summary": "reviewed",
        }))))
        .await
        .expect_err("an unreadable remote cannot authorize a pre-park waiver");
    assert!(
        error.message.contains("no readable pushed tip"),
        "{}",
        error.message
    );
    assert!(
        cas_store::list_qa_passes(&cas_dir, &task_id)
            .unwrap()
            .is_empty()
    );
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

    let role = SupervisorRole::enter(&mut test_env);
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
    drop(role);
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
struct GhStub;

impl GhStub {
    fn install<'a>(
        env: &'a mut TestEnvGuard,
        dir: &Path,
        head_sha: &str,
    ) -> (ScopedFactoryEnv<'a>, std::path::PathBuf) {
        let gh = dir.join("gh");
        let log = dir.join("gh.log");
        // `gh pr view 2546` knows PR #2546 (head factory/test-agent); every
        // other PR lookup fails like an unknown PR. `repo view` follows an
        // origin rename unless the failure marker is present. Status POSTs
        // succeed. Every call is logged.
        cas::test_paths::warm_stub(
            &gh,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$CAS_TEST_GH_LOG"
if [ "$1" = "repo" ] && [ "$2" = "view" ] && [ "$3" = "acme/gabber" ]; then
  if [ -f "$CAS_TEST_GH_LOG.lookup-fails" ]; then exit 1; fi
  printf '{"nameWithOwner":"canonical-owner/gabber"}'
  exit 0
fi
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
        let mut scope = ScopedFactoryEnv::apply(
            env,
            &[
                ("CAS_QA_GH", None),
                ("CAS_TEST_GH_LOG", None),
                ("CAS_TEST_GH_HEAD", None),
            ],
        );
        scope.guard().set("CAS_QA_GH", &gh);
        scope.guard().set("CAS_TEST_GH_LOG", &log);
        scope.guard().set("CAS_TEST_GH_HEAD", head_sha);
        (scope, log)
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
    raw_github_merges_with_status_lookup(false).await;
}

/// cas-28c8: a failed metadata lookup still publishes the required status
/// against the explicit origin, even with an upstream repository present.
#[tokio::test]
async fn independent_qa_status_falls_back_to_explicit_origin_cas_28c8() {
    raw_github_merges_with_status_lookup(true).await;
}

async fn raw_github_merges_with_status_lookup(lookup_fails: bool) {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    let config = cas_dir.join("config.toml");
    let body = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, format!("{body}github_status = true\n")).unwrap();
    let head = git(&repo, &["rev-parse", "factory/test-agent"]);
    // cas-0169: the project's own GitHub repository; rounds are scoped to it.
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/gabber.git",
        ],
    );
    git(
        &repo,
        &["remote", "add", "upstream", "https://github.com/upstream/wrong.git"],
    );
    let stub_dir = repo.join("stub-bin");
    std::fs::create_dir_all(&stub_dir).unwrap();
    let (mut gh, gh_log) = GhStub::install(&mut test_env, &stub_dir, &head);
    if lookup_fails {
        std::fs::write(gh_log.with_extension("log.lookup-fails"), "fail repo view\n").unwrap();
    }
    let publish_repo = if lookup_fails {
        "acme/gabber"
    } else {
        "canonical-owner/gabber"
    };

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let qa_task = qa_task_id(&cas_dir, &task_id);
    // Repository side: the open round is a pending required check on the
    // delivered head.
    wait_for_gh_call(
        &gh_log,
        &[
            &format!("repos/{publish_repo}/statuses/{head}"),
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

    // A GraphQL merge by node id is refused while a round is open, naming
    // the open rounds, rather than waved through.
    let command = "gh api graphql -f query='mutation { mergePullRequest(input: {pullRequestId: \"PR_kw\"}) { clientMutationId } }'";
    let refusal = cas::qa_pass::github_merge_refusal(&cas_dir, &repo, command)
        .unwrap_or_else(|| panic!("unmapped merge allowed: {command}"));
    assert!(
        refusal.contains("cannot tell which delivery") && refusal.contains(&task_id),
        "{command}: {refusal}"
    );

    // cas-0169: a PR in this repository whose head lookup fails is still
    // refused, but the denial names the failure and blames no task.
    for command in [
        "gh pr merge 77 --squash",
        "gh pr merge 77 --repo acme/gabber --squash",
        "gh pr merge 77 --repo github.com/ACME/gabber --squash",
    ] {
        let refusal = cas::qa_pass::github_merge_refusal(&cas_dir, &repo, command)
            .unwrap_or_else(|| panic!("unmapped merge allowed: {command}"));
        assert!(
            refusal.contains("could not look up the head")
                && refusal.contains("Lookup failure: `")
                && refusal.contains("exited with")
                && refusal.contains("1 open round(s)"),
            "{command}: {refusal}"
        );
        assert!(
            !refusal.contains(&task_id) && !refusal.contains(&qa_task),
            "a failed lookup must not blame unrelated tasks: {refusal}"
        );
    }

    // cas-0169: another repository's PR cannot belong to this project's QA
    // round, so it is never held, whatever the lookup would say.
    for command in [
        "gh pr merge 120 --repo Richards-LLC/petra-stella-cloud --auto --squash",
        "bash -ic 'gh pr merge 120 --repo Richards-LLC/petra-stella-cloud --auto --squash'",
        "gh pr merge https://github.com/Richards-LLC/petra-stella-cloud/pull/120 --squash",
        "gh api -X PUT repos/Richards-LLC/petra-stella-cloud/pulls/120/merge",
        "gh pr merge 2546 -R other-owner/gabber --squash",
    ] {
        assert!(
            cas::qa_pass::github_merge_refusal(&cas_dir, &repo, command).is_none(),
            "another repository's merge was held by this project's round: {command}"
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
    let _role = SupervisorRole::enter(gh.guard());
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
            &format!("repos/{publish_repo}/statuses/{head}"),
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

/// The parked-request compatibility path accepts a live Standard supervisor
/// session, not a worker or dead identity; explicit receipts remain stricter.
#[tokio::test]
async fn parked_qa_request_compatibility_does_not_authorize_worker_or_dead_caller_cas_9ffa() {
    for (role, shutdown, receipt) in [
        (AgentRole::Worker, false, false),
        (AgentRole::Supervisor, true, false),
        (AgentRole::Standard, true, false),
        (AgentRole::Standard, false, true),
    ] {
        let mut env = TestEnvGuard::temp_home();
        let (_temp, _core, repo, task_id) = fixture(&mut env);
        let cas_dir = repo.join(".cas");
        let tasks = open_task_store(&cas_dir).unwrap();
        let mut task = tasks.get(&task_id).unwrap();
        task.status = TaskStatus::AwaitingMerge;
        tasks.update(&task).unwrap();
        let agents = open_agent_store(&cas_dir).unwrap();
        let mut caller = Agent::new_with_role("qa-caller".into(), "qa-caller".into(), role);
        agents.register(&caller).unwrap();
        if shutdown {
            caller.status = cas::types::AgentStatus::Shutdown;
            agents.update(&caller).unwrap();
        }
        let core = CasCore::with_daemon(cas_dir.clone(), None, None);
        core.set_agent_id_for_testing(caller.id);
        let service = CasService::new(core, None);
        let _role = SupervisorRole::enter(&mut env);
        let mut request = serde_json::json!({
            "action": "qa_request", "task_id": task_id, "summary": "inspect delivery",
        });
        if receipt {
            request["head_sha"] = serde_json::json!(git(&repo, &["rev-parse", "HEAD"]));
        }
        let refusal = service.verification(Parameters(verification(request))).await
            .expect_err("supervisor environment alone must not authorize this caller");
        assert!(refusal.message.contains("live registered supervisor"), "{}", refusal.message);
        assert!(cas_store::list_qa_passes(&cas_dir, &task_id).unwrap().is_empty());
        assert_eq!(tasks.get(&task_id).unwrap().status, TaskStatus::AwaitingMerge);
    }
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

    let _role = SupervisorRole::enter(&mut test_env);
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
    test_env.remove(VAR);
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
    test_env.set(VAR, &env_file);
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

#[test]
fn gh_and_supervisor_scopes_restore_values_on_panic_cas_6651() {
    let mut test_env = TestEnvGuard::temp_home();
    test_env.set("CAS_QA_GH", "original-gh");
    test_env.remove("CAS_TEST_GH_LOG");
    test_env.remove("CAS_TEST_GH_HEAD");
    test_env.set("CAS_AGENT_ROLE", "worker");
    let stub_dir = test_env.home().join("stub-bin");
    std::fs::create_dir_all(&stub_dir).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (mut gh, log) = GhStub::install(&mut test_env, &stub_dir, "fixture-head");
        let _role = SupervisorRole::enter(gh.guard());
        assert_eq!(
            std::env::var_os("CAS_QA_GH"),
            Some(stub_dir.join("gh").into_os_string())
        );
        assert_eq!(
            std::env::var_os("CAS_TEST_GH_LOG"),
            Some(log.into_os_string())
        );
        assert_eq!(
            std::env::var("CAS_TEST_GH_HEAD").as_deref(),
            Ok("fixture-head")
        );
        assert_eq!(std::env::var("CAS_AGENT_ROLE").as_deref(), Ok("supervisor"));
        panic!("exercise QA scope restoration");
    }));
    let panic = result.expect_err("the deliberate fixture panic must unwind");
    assert_eq!(panic.downcast_ref::<&str>().copied(), Some("exercise QA scope restoration"));
    assert_eq!(std::env::var("CAS_QA_GH").as_deref(), Ok("original-gh"));
    assert!(std::env::var_os("CAS_TEST_GH_LOG").is_none());
    assert!(std::env::var_os("CAS_TEST_GH_HEAD").is_none());
    assert_eq!(std::env::var("CAS_AGENT_ROLE").as_deref(), Ok("worker"));
}

/// cas-7877, the cas-4a8e1 shape: the first park was stacked on another
/// task's UI commit and opened a round nobody reviewed. The worker re-parks
/// from a backend-only tip. That tip owes no review, so the stale round is
/// withdrawn (its work item cancelled) instead of re-dispatched as a
/// "re-review after round 1".
#[tokio::test]
async fn a_backend_only_repark_withdraws_an_unreviewed_round_instead_of_re_reviewing_cas_7877() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let stale_qa_task = qa_task_id(&cas_dir, &task_id);

    // The supervisor declines that delivery administratively (the lane was
    // stacked on another task's UI commit)...
    {
        let supervisor = supervisor_core(&cas_dir);
        let _role = SupervisorRole::enter(&mut test_env);
        supervisor
            .cas_task_request_changes(Parameters(TaskRequestChangesRequest {
                id: task_id.clone(),
                reason: "administrative: re-park from the backend-only cherry-pick".into(),
            }))
            .await
            .expect("supervisor declines the stacked delivery");
    }
    // ...and the worker re-parks from a clean, backend-only per-task branch
    // (cas-73b8); the UI commit stays behind on factory/test-agent.
    core.cas_task_start(Parameters(IdRequest { id: task_id.clone() }))
        .await
        .expect("restart after request_changes");
    git(&repo, &["checkout", "-q", "-b", "factory/test-agent-cas-ui01", "main"]);
    commit_file(&repo, "src/halt.rs", "pub fn halt() {}\n", "backend-only fix");
    let reparked = close_text(&core, &task_id).await;
    assert!(reparked.contains("MERGE REQUIRED"), "{reparked}");
    assert!(!reparked.contains("re-review"), "{reparked}");
    assert!(!reparked.contains("INDEPENDENT QA DISPATCHED"), "{reparked}");
    assert!(!reparked.contains("INDEPENDENT QA PENDING"), "{reparked}");

    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert_eq!(passes.len(), 1, "no new round: {passes:?}");
    assert!(passes[0].is_withdrawn(), "{:?}", passes[0]);
    let tasks = open_task_store(&cas_dir).unwrap();
    assert_eq!(tasks.get(&stale_qa_task).unwrap().status, TaskStatus::Cancelled);
    assert!(
        cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, "git merge factory/test-agent-cas-ui01")
            .is_none(),
        "the merge no longer waits on a review nobody owes"
    );
    let again = close_text(&core, &task_id).await;
    assert!(!again.contains("INDEPENDENT QA"), "{again}");
}

/// cas-7877: a reviewed round still binds. After a recorded rejection, a
/// backend-only re-park is reviewed again (GH #1001 / cas-627c), not withdrawn.
#[tokio::test]
async fn a_rejected_round_is_still_re_reviewed_after_a_backend_only_repark_cas_7877() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let _keep = &temp;
    let _reviewer = reject_round_one(&core, &repo, &task_id).await;

    // The new tip alone is backend-only.
    git(&repo, &["checkout", "-q", "-b", "factory/test-agent-cas-ui01", "main"]);
    commit_file(&repo, "src/fix.rs", "pub fn fix() {}\n", "backend fix for the rejection");
    let reparked = close_text(&core, &task_id).await;
    assert!(reparked.contains("re-review after round 1"), "{reparked}");
    assert!(!reparked.contains("WITHDRAWN"), "{reparked}");
}

/// cas-7877: cancelling a QA work item withdraws its round, so the delivery is
/// no longer gated on a review that will not happen.
#[tokio::test]
async fn cancelling_a_qa_work_item_withdraws_its_round_cas_7877() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let qa_task = qa_task_id(&cas_dir, &task_id);

    let supervisor = supervisor_core(&cas_dir);
    let _role = SupervisorRole::enter(&mut test_env);
    let cancelled = extract_text(
        supervisor
            .cas_task_cancel(Parameters(TaskCancelRequest {
                id: qa_task.clone(),
                reason: "spurious round: the lane was stacked on another task's UI commit".into(),
                superseded_by: None,
            }))
            .await
            .expect("supervisor cancels the QA work item"),
    );
    assert!(cancelled.contains("withdrawn"), "{cancelled}");
    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert!(passes[0].is_withdrawn(), "{:?}", passes[0]);
    assert!(
        cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, "git merge factory/test-agent")
            .is_none(),
        "a cancelled work item no longer gates the merge"
    );
}

/// cas-54b0: a QA work item cancelled without withdrawing its round, the
/// shape a runtime before cas-7877 left behind (cas-e641 on 2026-10-05). The
/// round stays pending, linked to a task that will never run.
fn orphan_cancelled_qa_task(cas_dir: &Path, qa_task: &str) {
    let tasks = open_task_store(cas_dir).unwrap();
    let mut item = tasks.get(qa_task).unwrap();
    item.status = TaskStatus::Cancelled;
    item.closed_at = Some(chrono::Utc::now());
    item.close_reason = Some("bound to a stale anchor".into());
    tasks.update(&item).unwrap();
}

/// cas-54b0: the supervisor's qa_request at the same tip reported "INDEPENDENT
/// QA PENDING ... QA task <cancelled>" and opened nothing, so no reviewer could
/// ever record a verdict. It now withdraws the orphaned round, saying why, and
/// opens a fresh one with a live work item.
#[tokio::test]
async fn qa_request_replaces_a_round_whose_work_item_was_cancelled_cas_54b0() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let stale = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .unwrap();
    let stale_qa_task = stale.qa_task_id.clone().expect("its work item");
    orphan_cancelled_qa_task(&cas_dir, &stale_qa_task);

    let _role = SupervisorRole::enter(&mut test_env);
    let service = CasService::new(supervisor_core(&cas_dir), None);
    let requested = extract_text(
        service
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_request",
                "task_id": task_id,
                "summary": "the previous QA task was cancelled",
            }))))
            .await
            .expect("the supervisor's request opens a live round"),
    );
    assert!(
        requested.contains("INDEPENDENT QA DISPATCHED"),
        "{requested}"
    );
    assert!(!requested.contains("INDEPENDENT QA PENDING"), "{requested}");
    assert!(
        requested.contains(&stale_qa_task),
        "names the cancelled work item: {requested}"
    );

    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    let retired = passes.iter().find(|pass| pass.id == stale.id).unwrap();
    assert!(retired.is_withdrawn(), "{retired:?}");
    assert!(
        retired.summary.as_deref().is_some_and(
            |summary| summary.contains(&stale_qa_task) && summary.contains("cancelled")
        ),
        "the withdrawal reason names the cancelled work item: {retired:?}"
    );
    let fresh = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .unwrap();
    assert_ne!(fresh.id, stale.id);
    assert!(fresh.state.is_active());
    assert_eq!(fresh.bound_head, stale.bound_head);
    let fresh_qa_task = fresh.qa_task_id.expect("the fresh round's work item");
    assert_ne!(fresh_qa_task, stale_qa_task);
    assert_eq!(
        open_task_store(&cas_dir)
            .unwrap()
            .get(&fresh_qa_task)
            .unwrap()
            .status,
        TaskStatus::Open
    );
}

/// cas-54b0 review: a work item Cassy cannot read is not a missing one. With
/// the QA task's row unreadable (a store error that is not "not found"),
/// qa_request leaves the open round and its reviewer alone instead of
/// withdrawing it and dispatching a duplicate.
#[tokio::test]
async fn an_unreadable_qa_work_item_keeps_its_open_round_cas_54b0() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let open = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .unwrap();
    let qa_task = open.qa_task_id.clone().expect("its work item");
    // A text priority makes the row fail to decode: a database error, not
    // TaskNotFound.
    let db = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
    db.execute(
        "UPDATE tasks SET priority = 'unreadable' WHERE id = ?1",
        rusqlite::params![qa_task],
    )
    .unwrap();
    drop(db);
    assert!(
        !matches!(
            open_task_store(&cas_dir).unwrap().get(&qa_task),
            Ok(_)
                | Err(cas_store::StoreError::TaskNotFound(_) | cas_store::StoreError::NotFound(_))
        ),
        "the fixture must produce a store error other than not found"
    );

    let _role = SupervisorRole::enter(&mut test_env);
    let service = CasService::new(supervisor_core(&cas_dir), None);
    let requested = extract_text(
        service
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_request",
                "task_id": task_id,
                "summary": "re-request while the store is unhealthy",
            }))))
            .await
            .expect("the request answers"),
    );
    assert!(requested.contains("INDEPENDENT QA PENDING"), "{requested}");
    assert!(!requested.contains("withdrawn"), "{requested}");
    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert_eq!(passes.len(), 1, "no duplicate round: {passes:?}");
    assert_eq!(passes[0].id, open.id);
    assert!(
        passes[0].state.is_active() && !passes[0].is_withdrawn(),
        "{:?}",
        passes[0]
    );
    assert_eq!(passes[0].qa_task_id.as_deref(), Some(qa_task.as_str()));
}

/// cas-54b0: cancelling an already-cancelled QA work item returned "Already
/// cancelled" and left its orphaned round pending. The retry now repairs it.
#[tokio::test]
async fn cancelling_an_already_cancelled_qa_work_item_withdraws_its_orphaned_round_cas_54b0() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let qa_task = qa_task_id(&cas_dir, &task_id);
    orphan_cancelled_qa_task(&cas_dir, &qa_task);

    let supervisor = supervisor_core(&cas_dir);
    let _role = SupervisorRole::enter(&mut test_env);
    let retried = extract_text(
        supervisor
            .cas_task_cancel(Parameters(TaskCancelRequest {
                id: qa_task.clone(),
                reason: "bound to a stale anchor".into(),
                superseded_by: None,
            }))
            .await
            .expect("cancelling again is not an error"),
    );
    assert!(retried.contains("Already cancelled"), "{retried}");
    assert!(retried.contains("withdrawn"), "{retried}");
    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert!(passes[0].is_withdrawn(), "{:?}", passes[0]);
    assert!(
        cas::qa_pass::supervisor_merge_refusal(&cas_dir, &repo, "git merge factory/test-agent")
            .is_none(),
        "the orphaned round no longer gates the merge"
    );
}

/// cas-54b0: the cancel withdrew a round only when its work item still carried
/// the qa-pass label. The round's own link to the work item now decides.
#[tokio::test]
async fn cancelling_an_unlabelled_qa_work_item_still_withdraws_its_round_cas_54b0() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "{parked}");
    let qa_task = qa_task_id(&cas_dir, &task_id);
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut item = tasks.get(&qa_task).unwrap();
    item.labels.clear();
    tasks.update(&item).unwrap();

    let supervisor = supervisor_core(&cas_dir);
    let _role = SupervisorRole::enter(&mut test_env);
    let cancelled = extract_text(
        supervisor
            .cas_task_cancel(Parameters(TaskCancelRequest {
                id: qa_task.clone(),
                reason: "stale round".into(),
                superseded_by: None,
            }))
            .await
            .expect("supervisor cancels the QA work item"),
    );
    assert!(cancelled.contains("withdrawn"), "{cancelled}");
    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert!(passes[0].is_withdrawn(), "{:?}", passes[0]);
}

/// cas-3760 (GH #1066): the target gained an unrelated `.scss` commit that
/// this checkout's local target branch has not caught up with. A CI-only
/// delivery branched from the fresh target is classified against
/// `origin/<target>`, so it is not user-facing and no round is dispatched.
#[tokio::test]
async fn a_stale_local_target_does_not_make_a_ci_only_delivery_user_facing_cas_3760() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;

    // origin/main moves on with someone else's stylesheet change...
    git(&repo, &["checkout", "-q", "main"]);
    let styled = commit_file(
        &repo,
        "apps/frontend/app/src/assets/styles/_auth-pages.scss",
        ".auth{margin:0}\n",
        "unrelated auth page styles",
    );
    git(&repo, &["update-ref", "refs/remotes/origin/main", &styled]);
    // ...while this checkout's local main stays behind it.
    git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);
    // The delivery branches from the fresh target and changes CI only.
    git(&repo, &["checkout", "-q", "-B", "factory/test-agent", &styled]);
    commit_file(&repo, ".github/workflows/ci.yml", "on: push\n", "ci only");

    let parked = close_text(&core, &task_id).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert!(!parked.contains("INDEPENDENT QA"), "{parked}");
    assert!(cas_store::list_qa_passes(&cas_dir, &task_id).unwrap().is_empty());
}

/// One rejected round: start the open round's QA task as `reviewer`, record
/// a rejection for `head`, and put the delivery back in progress.
async fn reject_open_round(reviewer: &CasCore, repo: &Path, task_id: &str, round: u32, head: &str) {
    let cas_dir = repo.join(".cas");
    reviewer
        .cas_task_start(Parameters(IdRequest {
            id: qa_task_id(&cas_dir, task_id),
        }))
        .await
        .unwrap();
    let ledger = round_evidence(&repo.join(format!("round-{round}")), task_id, head);
    let rejected = extract_text(
        CasService::new(reviewer.clone(), None)
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_record",
                "task_id": task_id,
                "status": "rejected",
                "summary": format!("round {round}: a distinct real defect"),
                "ledger_path": ledger.display().to_string(),
            }))))
            .await
            .unwrap(),
    );
    assert!(rejected.contains("REJECTION"), "{rejected}");
    reopened_to_in_progress(&cas_dir, task_id);
}

/// cas-624f: after `max_rounds` (3) rejections Cassy escalates, and the
/// escalation offers "a fix plan with the implementer". That option had no
/// executable path: the supervisor's qa_request was refused with the same
/// escalation. Now a supervisor qa_request with the fix plan opens round 4,
/// with its QA task and deadline, logged as an override. The worker's own
/// close stays capped, so a 4th rejection escalates again.
#[tokio::test]
async fn supervisor_fix_plan_opens_one_round_past_the_escalation_cas_624f() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, core, repo, task_id) = fixture(&mut test_env);
    let cas_dir = repo.join(".cas");
    let _keep = &temp;
    let reviewer = reviewer_core(&cas_dir, "qa-reviewer");

    let mut head = git(&repo, &["rev-parse", "HEAD"]);
    for round in 1..=3u32 {
        let parked = close_text(&core, &task_id).await;
        assert!(parked.contains("INDEPENDENT QA DISPATCHED"), "round {round}: {parked}");
        reject_open_round(&reviewer, &repo, &task_id, round, &head).await;
        head = commit_file(
            &repo,
            "web/composer.css",
            &format!(".composer{{gap:{}px}}\n", 8 + round),
            &format!("fix round {round}"),
        );
    }

    // The worker's park after three rejections escalates; no round opens.
    let escalated = close_text(&core, &task_id).await;
    assert!(escalated.contains("INDEPENDENT QA ESCALATED: 3 rejected rounds"), "{escalated}");
    assert!(escalated.contains("action=qa_request"), "the escalation names the fix-plan path: {escalated}");
    let passes = cas_store::list_qa_passes(&cas_dir, &task_id).unwrap();
    assert_eq!(passes.len(), 3, "no fourth round from the worker's close");
    assert_eq!(open_task_store(&cas_dir).unwrap().get(&task_id).unwrap().status, TaskStatus::AwaitingMerge);

    // The supervisor's fix plan opens round 4 with its QA task and deadline.
    let _role = SupervisorRole::enter(&mut test_env);
    let service = CasService::new(supervisor_core(&cas_dir), None);
    let fix_plan = "pair with the implementer on the focus order; reviewer checks keyboard-only first";
    let requested = extract_text(
        service
            .verification(Parameters(verification(serde_json::json!({
                "action": "qa_request",
                "task_id": task_id,
                "summary": fix_plan,
            }))))
            .await
            .expect("the supervisor's fix plan opens one more round"),
    );
    assert!(requested.contains("INDEPENDENT QA DISPATCHED"), "{requested}");
    let round4 = cas_store::latest_qa_pass(&cas_dir, &task_id, chrono::Utc::now())
        .unwrap()
        .expect("round four");
    assert_eq!(round4.round, 4);
    assert_eq!(round4.bound_head, head);
    assert!(round4.state.is_active());
    assert!(round4.deadline_at > chrono::Utc::now(), "round four has a deadline");
    let tasks = open_task_store(&cas_dir).unwrap();
    let qa_task = tasks.get(round4.qa_task_id.as_deref().expect("round four's QA task")).unwrap();
    assert!(qa_task.description.contains(fix_plan), "{}", qa_task.description);
    let notes = tasks.get(&task_id).unwrap().notes;
    assert!(
        notes.contains("Independent QA fix plan: supervisor") && notes.contains("past the escalation after 3 rejected rounds"),
        "the override is logged: {notes}"
    );

    // A 4th rejection escalates again: the worker's next park opens nothing.
    drop(_role);
    reject_open_round(&reviewer, &repo, &task_id, 4, &head).await;
    commit_file(&repo, "web/composer.css", ".composer{gap:12px}\n", "fix round 4");
    let escalated_again = close_text(&core, &task_id).await;
    assert!(escalated_again.contains("INDEPENDENT QA ESCALATED: 4 rejected rounds"), "{escalated_again}");
    assert_eq!(cas_store::list_qa_passes(&cas_dir, &task_id).unwrap().len(), 4);
}
