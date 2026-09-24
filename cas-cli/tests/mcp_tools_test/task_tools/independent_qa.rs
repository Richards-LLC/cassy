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

/// A delivery merged into a non-trunk lane (an epic) outside the guarded
/// paths is still sent for review before it closes. The supervisor's handoff
/// says it was merged, never that it "parked for merge" (cas-5c38).
#[tokio::test]
async fn merged_into_an_epic_without_a_verdict_is_refused_at_close_and_dispatched() {
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
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
            .contains("was merged into epic/ui before any QA round (it never parked)"),
        "{}",
        handoff.prompt
    );
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
        // SAFETY: callers hold env_test_lock for the whole test body.
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
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
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
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
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
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
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
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
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

/// cas-ce39: a re-park at a new tip while a reviewer holds the round used to
/// supersede it silently: the reviewer kept reviewing a dead head and the old
/// QA task stayed open. Now the old round's work item is cancelled, pointing
/// at the new one, the reviewer is told to stop, and the park says so.
#[tokio::test]
async fn a_new_tip_supersedes_a_claimed_round_and_tells_its_reviewer_cas_ce39() {
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
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
    let (temp, core, repo, task_id) = fixture();
    let _env = env_test_lock();
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
