use crate::support::*;
use cas::mcp::tools::*;
use cas::store::{
    EventStore, init_cas_dir, open_agent_store, open_event_store, open_task_store,
    open_verification_store, open_worktree_store,
};
use cas::types::{AgentRole, EventType, TaskStatus, Verification, VerificationType, Worktree};
use rmcp::handler::server::wrapper::Parameters;
use std::process::Command;
use tempfile::TempDir;

fn proof_boundary_git(path: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
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
}

async fn legacy_repository_proof_rejects_drift(isolated: bool) {
    let (temp, service) = setup_cas();
    // `cas_task_close` resolves the acting factory context while it validates
    // the proof root. Keep that process-global context stable for this whole
    // Git-worktree fixture so sibling factory tests cannot invalidate a
    // reviewed.txt proof between the deliberate drift and its restoration.
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n[code_review]\nowner = \"worker\"\n",
    )
    .expect("legacy verification config");

    proof_boundary_git(temp.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(temp.path().join(".gitignore"), ".cas/\n").unwrap();
    std::fs::write(temp.path().join("reviewed.txt"), "reviewed\n").unwrap();
    proof_boundary_git(temp.path(), &["add", ".gitignore", "reviewed.txt"]);
    proof_boundary_git(temp.path(), &["commit", "-q", "-m", "seed"]);

    let isolated_dir = isolated.then(TempDir::new).transpose().unwrap();
    let proof_root = if let Some(dir) = isolated_dir.as_ref() {
        proof_boundary_git(temp.path(), &["branch", "factory/proof-worker"]);
        proof_boundary_git(
            temp.path(),
            &[
                "worktree",
                "add",
                "-q",
                dir.path().to_str().unwrap(),
                "factory/proof-worker",
            ],
        );
        dir.path()
    } else {
        temp.path()
    };

    let created = service
        .cas_task_create(Parameters(simple_task_req(if isolated {
            "Isolated legacy repository proof"
        } else {
            "Shared legacy repository proof with docs-only code review skip"
        })))
        .await
        .expect("create reviewed task");
    let task_id = extract_task_id(&extract_text(created)).unwrap().to_string();
    service
        .cas_task_start(Parameters(IdRequest {
            id: task_id.clone(),
        }))
        .await
        .expect("start reviewed task");

    if isolated {
        let store = open_worktree_store(&cas_dir).unwrap();
        store.init().unwrap();
        let worktree_id = Worktree::generate_id();
        store
            .add(&Worktree::new(
                worktree_id.clone(),
                "factory/proof-worker".to_string(),
                "main".to_string(),
                proof_root.to_path_buf(),
            ))
            .unwrap();
        let task_store = open_task_store(&cas_dir).unwrap();
        let mut task = task_store.get(&task_id).unwrap();
        task.worktree_id = Some(worktree_id);
        task_store.update(&task).unwrap();
    }

    let other = service
        .cas_task_create(Parameters(simple_task_req("Unrelated mutable task")))
        .await
        .expect("create unrelated task");
    let other_id = extract_task_id(&extract_text(other)).unwrap().to_string();

    let close = |reason: &str| TaskCloseRequest {
        stranded_branch_override: None,
        id: task_id.clone(),
        reason: Some(reason.to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let first = extract_text(
        service
            .cas_task_close(Parameters(close("review this exact repository state")))
            .await
            .expect("first close"),
    );
    assert!(first.contains("VERIFICATION REQUIRED"), "{first}");
    let first_dispatch = cas_store::get_latest_verification_dispatch(&cas_dir, &task_id)
        .unwrap()
        .expect("first dispatch");

    service
        .cas_task_update(Parameters(task_status_update(
            &other_id,
            None,
            Some("unrelated work remains available during review"),
        )))
        .await
        .expect("unrelated task update during review");

    std::fs::write(proof_root.join("reviewed.txt"), "mutated during review\n").unwrap();
    let supervisor_id = if isolated {
        "proof-supervisor-isolated"
    } else {
        "proof-supervisor-shared"
    };
    let mut supervisor =
        cas::types::Agent::new(supervisor_id.to_string(), supervisor_id.to_string());
    supervisor.role = AgentRole::Supervisor;
    open_agent_store(&cas_dir)
        .unwrap()
        .register(&supervisor)
        .unwrap();
    let supervisor_core = cas::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor_core.set_agent_id_for_testing(supervisor_id.to_string());
    let verdict = |dispatch_id: String| VerificationAddRequest {
        task_id: task_id.clone(),
        status: "approved".to_string(),
        summary: "reviewed exact repository state".to_string(),
        confidence: Some(0.99),
        issues: None,
        files_reviewed: Some("reviewed.txt".to_string()),
        duration_ms: Some(5),
        verification_type: None,
        verifier_capability: None,
        dispatch_id: Some(dispatch_id),
    };
    let during_error = supervisor_core
        .cas_verification_add(Parameters(verdict(first_dispatch.id.clone())))
        .await
        .expect_err("repository drift during review must reject approval");
    assert!(during_error.message.contains("repository proof"));
    assert_eq!(
        cas_store::get_verification_dispatch(&cas_dir, &first_dispatch.id)
            .unwrap()
            .state,
        cas::types::VerificationDispatchState::Invalidated
    );

    std::fs::write(proof_root.join("reviewed.txt"), "reviewed\n").unwrap();
    let retry = extract_text(
        service
            .cas_task_close(Parameters(close("re-review restored state")))
            .await
            .expect("fresh close cycle"),
    );
    assert!(retry.contains("VERIFICATION REQUIRED"), "{retry}");
    let approved_dispatch = cas_store::get_latest_verification_dispatch(&cas_dir, &task_id)
        .unwrap()
        .expect("fresh dispatch");
    assert_ne!(approved_dispatch.id, first_dispatch.id);
    supervisor_core
        .cas_verification_add(Parameters(verdict(approved_dispatch.id.clone())))
        .await
        .expect("unchanged repository proof approves");

    if isolated {
        std::fs::write(proof_root.join("reviewed.txt"), "mutated after approval\n").unwrap();
    } else {
        proof_boundary_git(
            proof_root,
            &["commit", "-q", "--allow-empty", "-m", "post-review drift"],
        );
    }
    let post_review = extract_text(
        service
            .cas_task_close(Parameters(close("must not reuse stale approval")))
            .await
            .expect("post-review close"),
    );
    assert!(
        post_review.contains("VERIFICATION REQUIRED")
            && !post_review.contains("Closed task:")
            && !post_review.contains("CODE_REVIEW_REQUIRED"),
        "post-review mutation must require a fresh repository proof before code review: {post_review}"
    );
    let post_review_dispatch = cas_store::get_latest_verification_dispatch(&cas_dir, &task_id)
        .unwrap()
        .expect("post-review dispatch");
    assert_ne!(post_review_dispatch.id, approved_dispatch.id);

    service
        .cas_task_update(Parameters(task_status_update(
            &other_id,
            None,
            Some("unrelated work remains available after review"),
        )))
        .await
        .expect("unrelated task update after review");
}

#[tokio::test]
async fn test_legacy_nonisolated_verdict_is_bound_to_repository_proof() {
    legacy_repository_proof_rejects_drift(false).await;
}

#[tokio::test]
async fn test_legacy_isolated_verdict_is_bound_to_repository_proof() {
    legacy_repository_proof_rejects_drift(true).await;
}

// =============================================================================
// cas-5c33: a dispatch is bound to the DELIVERED commits, not the branch tip.
// A worker that merges or fast-forwards to start its next task used to
// invalidate the verdict on already-merged work, and the close retry then
// replayed the same spent dispatch id forever.
// =============================================================================

/// Build a task whose isolated worktree carries one delivered commit beyond
/// its integration base, and return (temp, worker service, cas_dir, task id,
/// worktree path).
async fn delivered_worktree_fixture(
    worker_branch: &str,
) -> (
    TempDir,
    cas::mcp::CasCore,
    std::path::PathBuf,
    String,
    TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let (temp, service) = setup_cas();
    // Ordering contract from support::setup_cas: take the env lock only after
    // setup_cas has released its own brief hold, or this deadlocks.
    let env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n[code_review]\nowner = \"worker\"\n",
    )
    .expect("verification config");

    proof_boundary_git(temp.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(temp.path().join(".gitignore"), ".cas/\n").unwrap();
    std::fs::write(temp.path().join("seed.txt"), "seed\n").unwrap();
    proof_boundary_git(temp.path(), &["add", ".gitignore", "seed.txt"]);
    proof_boundary_git(temp.path(), &["commit", "-q", "-m", "seed"]);

    let worker_dir = TempDir::new().unwrap();
    proof_boundary_git(temp.path(), &["branch", worker_branch]);
    proof_boundary_git(
        temp.path(),
        &[
            "worktree",
            "add",
            "-q",
            worker_dir.path().to_str().unwrap(),
            worker_branch,
        ],
    );
    // The delivered work: one commit beyond main, which is what the verifier
    // is actually asked to review.
    std::fs::write(worker_dir.path().join("delivered.txt"), "delivered\n").unwrap();
    proof_boundary_git(worker_dir.path(), &["add", "delivered.txt"]);
    proof_boundary_git(worker_dir.path(), &["commit", "-q", "-m", "deliver work"]);

    let created = service
        .cas_task_create(Parameters(simple_task_req("Delivered work under review")))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created)).unwrap().to_string();
    service
        .cas_task_start(Parameters(IdRequest {
            id: task_id.clone(),
        }))
        .await
        .expect("start task");

    let store = open_worktree_store(&cas_dir).unwrap();
    store.init().unwrap();
    let worktree_id = Worktree::generate_id();
    store
        .add(&Worktree::new(
            worktree_id.clone(),
            worker_branch.to_string(),
            "main".to_string(),
            worker_dir.path().to_path_buf(),
        ))
        .unwrap();
    let task_store = open_task_store(&cas_dir).unwrap();
    let mut task = task_store.get(&task_id).unwrap();
    task.worktree_id = Some(worktree_id);
    task_store.update(&task).unwrap();

    (temp, service, cas_dir, task_id, worker_dir, env_lock)
}

fn close_request(task_id: &str, reason: &str) -> TaskCloseRequest {
    TaskCloseRequest {
        stranded_branch_override: None,
        id: task_id.to_string(),
        reason: Some(reason.to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    }
}

async fn registered_supervisor(cas_dir: &std::path::Path, name: &str) -> cas::mcp::CasCore {
    let mut supervisor = cas::types::Agent::new(name.to_string(), name.to_string());
    supervisor.role = AgentRole::Supervisor;
    open_agent_store(cas_dir)
        .unwrap()
        .register(&supervisor)
        .unwrap();
    let core = cas::mcp::CasCore::with_daemon(cas_dir.to_path_buf(), None, None);
    core.set_agent_id_for_testing(name.to_string());
    core
}

#[tokio::test]
async fn test_verdict_survives_the_worker_branch_advancing_after_dispatch() {
    let (_temp, service, cas_dir, task_id, worker_dir, _env_lock) =
        delivered_worktree_fixture("factory/proof-mover").await;

    let first = extract_text(
        service
            .cas_task_close(Parameters(close_request(&task_id, "delivered work")))
            .await
            .expect("first close"),
    );
    assert!(first.contains("VERIFICATION REQUIRED"), "{first}");
    let dispatch = cas_store::get_latest_verification_dispatch(&cas_dir, &task_id)
        .unwrap()
        .expect("dispatch");
    let repository = dispatch
        .repository
        .as_ref()
        .expect("a Git worktree binds a repository proof");
    assert!(
        !repository.anchor_commits.is_empty(),
        "the delivered commit must be bound as the proof anchor: {repository:?}"
    );

    // The worker moves on: its next task's commit lands on the same branch.
    // The delivered commit is untouched and still reachable.
    std::fs::write(worker_dir.path().join("next-task.txt"), "next\n").unwrap();
    proof_boundary_git(worker_dir.path(), &["add", "next-task.txt"]);
    proof_boundary_git(worker_dir.path(), &["commit", "-q", "-m", "next task"]);

    let supervisor = registered_supervisor(&cas_dir, "proof-mover-supervisor").await;
    supervisor
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: task_id.clone(),
            status: "approved".to_string(),
            summary: "reviewed the delivered commit".to_string(),
            confidence: Some(0.99),
            issues: None,
            files_reviewed: Some("delivered.txt".to_string()),
            duration_ms: Some(5),
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(dispatch.id.clone()),
        }))
        .await
        .expect("a branch advance that keeps delivered commits reachable must not void the verdict");

    // The verdict record carries both digests so the moved tree is visible.
    let recorded = open_verification_store(&cas_dir)
        .unwrap()
        .get_latest_for_task(&task_id)
        .unwrap()
        .expect("verdict recorded");
    assert!(
        recorded.summary.contains(&repository.state_digest),
        "verdict must name the digest the verifier was handed: {}",
        recorded.summary
    );
    assert!(
        recorded.summary.contains("current head"),
        "verdict must name the digest and head the tree moved to: {}",
        recorded.summary
    );

    let second = extract_text(
        service
            .cas_task_close(Parameters(close_request(&task_id, "delivered work")))
            .await
            .expect("second close"),
    );
    assert!(
        !second.contains("VERIFICATION REQUIRED"),
        "an approved dispatch must not be re-demanded after a harmless branch advance: {second}"
    );
}

#[tokio::test]
async fn test_close_remints_a_fresh_dispatch_when_the_bound_proof_is_dead() {
    let (_temp, service, cas_dir, task_id, worker_dir, _env_lock) =
        delivered_worktree_fixture("factory/proof-rewriter").await;

    let first = extract_text(
        service
            .cas_task_close(Parameters(close_request(&task_id, "delivered work")))
            .await
            .expect("first close"),
    );
    assert!(first.contains("VERIFICATION REQUIRED"), "{first}");
    let stale = cas_store::get_latest_verification_dispatch(&cas_dir, &task_id)
        .unwrap()
        .expect("first dispatch");

    // The delivered commit is rewritten away: the bound proof is now dead and
    // no verdict can ever resolve it.
    proof_boundary_git(worker_dir.path(), &["reset", "-q", "--hard", "main"]);
    std::fs::write(worker_dir.path().join("replacement.txt"), "different\n").unwrap();
    proof_boundary_git(worker_dir.path(), &["add", "replacement.txt"]);
    proof_boundary_git(worker_dir.path(), &["commit", "-q", "-m", "rewritten"]);

    let retry = extract_text(
        service
            .cas_task_close(Parameters(close_request(&task_id, "delivered work")))
            .await
            .expect("retry close"),
    );
    assert!(retry.contains("VERIFICATION REQUIRED"), "{retry}");
    let fresh = cas_store::get_latest_verification_dispatch(&cas_dir, &task_id)
        .unwrap()
        .expect("fresh dispatch");
    assert_ne!(
        fresh.id, stale.id,
        "a dead dispatch must be retired and replaced, never replayed: {retry}"
    );
    assert!(
        retry.contains(&fresh.id),
        "the refusal must name the dispatch that can actually be resolved: {retry}"
    );
    assert_eq!(
        cas_store::get_verification_dispatch(&cas_dir, &stale.id)
            .unwrap()
            .state,
        cas::types::VerificationDispatchState::Invalidated,
        "the stale dispatch must be retired, not left pending"
    );
}

#[tokio::test]
async fn test_worker_main_loop_cannot_self_attest_verification() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let task_store = open_task_store(&cas_dir).expect("open task store");
    let verification_store = open_verification_store(&cas_dir).expect("open verification store");

    let worker_id = format!("test-session-{}", std::process::id());
    let mut worker = agent_store.get(&worker_id).expect("test worker exists");
    worker.role = AgentRole::Worker;
    worker.agent_type = cas::types::AgentType::Worker;
    agent_store.update(&worker).expect("mark caller as worker");

    let created = service
        .cas_task_create(Parameters(simple_task_req(
            "Worker self-attestation must fail closed",
        )))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();
    service
        .cas_task_start(Parameters(IdRequest {
            id: task_id.clone(),
        }))
        .await
        .expect("start task");

    let err = service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: task_id.clone(),
            status: "approved".to_string(),
            summary: "worker main loop claiming its own work passed".to_string(),
            confidence: Some(0.98),
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: None,
        }))
        .await
        .expect_err("worker main-loop self-attestation must be rejected");

    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    let message = err.message.to_string();
    assert!(
        message.contains("task-verifier") && message.contains("supervisor"),
        "rejection must explain legitimate verification paths: {message}"
    );
    assert!(
        verification_store
            .get_latest_for_task(&task_id)
            .expect("verification lookup")
            .is_none(),
        "rejected self-attestation must not persist a verification row"
    );
    assert!(
        !task_store
            .get(&task_id)
            .expect("task remains")
            .pending_verification,
        "rejected add must not mutate pending_verification"
    );
}

#[tokio::test]
async fn test_task_verifier_capability_is_child_bound_and_replay_safe() {
    let (temp, parent_service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("agent store");
    let verification_store = open_verification_store(&cas_dir).expect("verification store");
    let parent_id = format!("test-session-{}", std::process::id());

    let created = parent_service
        .cas_task_create(Parameters(simple_task_req("Capability-gated verification")))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();

    let dispatch = cas_store::create_verification_dispatch(
        &cas_dir,
        &task_id,
        &parent_id,
        &parent_id,
        chrono::Utc::now() + chrono::Duration::minutes(10),
    )
    .expect("create exact-task dispatch");
    let issued = cas_store::issue_verifier_capability(&cas_dir, &task_id, &parent_id)
        .expect("server issues capability");
    let child_id = format!("task-verifier-child-{}", std::process::id());
    cas_store::bind_verifier_capability(&cas_dir, &issued.token, &child_id)
        .expect("bind capability to child");
    cas_store::claim_verification_dispatch(
        &cas_dir,
        &task_id,
        &parent_id,
        &child_id,
        &issued.capability.id,
    )
    .expect("child claims exact dispatch");
    let mut child = cas::types::Agent::new_sub_agent(
        child_id.clone(),
        "task-verifier".to_string(),
        parent_id.clone(),
    );
    child.role = AgentRole::Standard;
    agent_store
        .register(&child)
        .expect("register verifier child");

    let owner_err = parent_service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: task_id.clone(),
            status: "approved".to_string(),
            summary: "owner presenting a stolen child capability".to_string(),
            confidence: Some(0.9),
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: Some(issued.token.clone()),
            dispatch_id: None,
        }))
        .await
        .expect_err("owner session cannot use the child's capability");
    assert_eq!(owner_err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        !owner_err.message.contains(&issued.token),
        "capability rejection diagnostics must never echo the raw bearer"
    );

    let child_service = cas::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
    child_service.set_agent_id_for_testing(child_id.clone());
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("db");
    conn.execute(
        "UPDATE verification_dispatches SET deadline_at = ?2 WHERE id = ?1",
        rusqlite::params![
            dispatch.id,
            (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339(),
        ],
    )
    .expect("expire dispatch before malformed request");
    drop(conn);
    let malformed_issues = format!(
        r#"[{{"file":"src/lib.rs","severity":"{}","category":"security""#,
        issued.token
    );
    let malformed = child_service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: task_id.clone(),
            status: "approved".to_string(),
            summary: "malformed issue retry".to_string(),
            confidence: Some(0.95),
            issues: Some(malformed_issues.clone()),
            files_reviewed: None,
            duration_ms: Some(12),
            verification_type: None,
            verifier_capability: Some(issued.token.clone()),
            dispatch_id: None,
        }))
        .await
        .expect_err("malformed issues must reject before capability consumption");
    assert_eq!(malformed.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert_eq!(
        malformed.message,
        "Invalid verification issues: expected a bounded JSON array of issue objects; input omitted."
    );
    assert!(!malformed.message.contains(&issued.token));
    assert!(!malformed.message.contains(&malformed_issues));
    assert!(
        verification_store
            .get_latest_for_task(&task_id)
            .expect("lookup after malformed issues")
            .is_none(),
        "malformed issues must not persist a verification"
    );
    assert!(
        cas_store::inspect_verifier_capability(&cas_dir, &issued.token)
            .expect("capability remains inspectable")
            .consumed_at
            .is_none(),
        "malformed issues must reject before consuming one-time authority"
    );
    assert_eq!(
        cas_store::get_verification_dispatch(&cas_dir, &dispatch.id)
            .expect("dispatch after malformed issues")
            .state,
        cas::types::VerificationDispatchState::Claimed,
        "malformed issues must reject before an expired dispatch is durably timed out"
    );
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("db");
    conn.execute(
        "UPDATE verification_dispatches SET deadline_at = ?2 WHERE id = ?1",
        rusqlite::params![
            dispatch.id,
            (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
        ],
    )
    .expect("restore dispatch deadline for valid retry");
    drop(conn);

    let raw_path = "/home/verifier/private-proof.json";
    let raw_pat = "ghp_verifier-authored-secret";
    let raw_control = "\u{1b}[31mverifier-control";
    let raw_password = "password=verifier-secret";
    let issues = serde_json::json!([{
        "file": raw_path,
        "line": 12,
        "severity": "blocking",
        "category": "security",
        "code": raw_pat,
        "problem": raw_control,
        "suggestion": raw_password,
    }])
    .to_string();
    let result = child_service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: task_id.clone(),
            status: "approved".to_string(),
            summary: format!("distinct child verified with {}", issued.token),
            confidence: Some(0.95),
            issues: Some(issues),
            files_reviewed: Some(format!("{raw_path},src/lib.rs")),
            duration_ms: Some(12),
            verification_type: None,
            verifier_capability: Some(issued.token.clone()),
            dispatch_id: None,
        }))
        .await
        .expect("bound task-verifier child succeeds once");
    let result_text = extract_text(result);

    let row = verification_store
        .get_latest_for_task(&task_id)
        .expect("lookup")
        .expect("verification row");
    assert_eq!(
        row.provenance,
        cas::types::VerificationProvenance::TaskVerifier
    );
    assert_eq!(row.agent_id.as_deref(), Some(child_id.as_str()));
    assert_eq!(row.issuer_agent_id.as_deref(), Some(parent_id.as_str()));
    assert_eq!(
        row.capability_id.as_deref(),
        Some(issued.capability.id.as_str())
    );
    assert_eq!(row.summary, "[REDACTED_SECRET]");
    assert_eq!(row.issues[0].file, "[REDACTED_PATH]");
    assert_eq!(row.issues[0].code, "[REDACTED_SECRET]");
    assert_eq!(row.issues[0].problem, "[REDACTED_CONTROL]");
    assert_eq!(
        row.issues[0].suggestion.as_deref(),
        Some("[REDACTED_SECRET]")
    );
    assert_eq!(
        row.files_reviewed,
        vec!["[REDACTED_PATH]".to_string(), "src/lib.rs".to_string()]
    );
    let row_payload = serde_json::to_string(&row).expect("serialize verification");
    let event_payload = serde_json::to_string(
        &open_event_store(&cas_dir)
            .expect("event store")
            .list_recent(20)
            .expect("events"),
    )
    .expect("serialize events");
    let show_text = extract_text(
        child_service
            .cas_verification_show(Parameters(VerificationShowRequest { id: row.id.clone() }))
            .await
            .expect("show sanitized verification"),
    );
    let list_text = extract_text(
        child_service
            .cas_verification_list(Parameters(VerificationListRequest {
                task_id: task_id.clone(),
                limit: Some(10),
            }))
            .await
            .expect("list sanitized verification"),
    );
    let latest_text = extract_text(
        child_service
            .cas_verification_latest(Parameters(VerificationListRequest {
                task_id: task_id.clone(),
                limit: Some(1),
            }))
            .await
            .expect("latest sanitized verification"),
    );
    for (surface, payload) in [
        ("result", result_text.as_str()),
        ("row", row_payload.as_str()),
        ("event", event_payload.as_str()),
        ("show", show_text.as_str()),
        ("list", list_text.as_str()),
        ("latest", latest_text.as_str()),
    ] {
        for unsafe_value in [
            issued.token.as_str(),
            raw_path,
            raw_pat,
            raw_control,
            raw_password,
            "verifier-secret",
        ] {
            assert!(
                !payload.contains(unsafe_value),
                "{surface} leaked verifier-authored content: {unsafe_value:?}"
            );
        }
    }
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("db");
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .expect("checkpoint verification writes");
    drop(conn);
    for suffix in ["", "-wal", "-shm"] {
        let path = cas_dir.join(format!("cas.db{suffix}"));
        if let Ok(bytes) = std::fs::read(path) {
            for unsafe_value in [
                issued.token.as_str(),
                raw_path,
                raw_pat,
                raw_control,
                raw_password,
                "verifier-secret",
            ] {
                assert!(
                    !bytes
                        .windows(unsafe_value.len())
                        .any(|window| window == unsafe_value.as_bytes()),
                    "SQLite {suffix} leaked verifier-authored content: {unsafe_value:?}"
                );
            }
        }
    }
    assert_eq!(
        cas_store::get_latest_verification_dispatch(&cas_dir, &task_id)
            .expect("dispatch lookup")
            .expect("dispatch")
            .state,
        cas::types::VerificationDispatchState::Resolved,
        "the capability-bound verdict must atomically resolve its exact dispatch"
    );

    let replay = child_service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id,
            status: "approved".to_string(),
            summary: "replay".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: Some(issued.token),
            dispatch_id: None,
        }))
        .await
        .expect_err("capability replay must fail");
    assert_eq!(replay.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        !replay.message.contains(&issued.capability.token_hash),
        "capability diagnostics must not expose persisted token hashes either"
    );
}

#[tokio::test]
async fn test_legacy_unsafe_verification_rows_are_sanitized_on_public_reads() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let raw_capability = "vcap-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let raw_path = "/home/legacy/private-proof.json";
    let raw_pat = "ghp_legacy-verifier-secret";
    let raw_control = "\u{1b}[31mlegacy-control";
    let _verification_store =
        open_verification_store(&cas_dir).expect("initialize verification schema");
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("db");
    conn.execute(
        "INSERT INTO verifications
         (id, task_id, agent_id, verification_type, status, confidence, summary,
          files_reviewed, duration_ms, created_at)
         VALUES (?1, ?2, NULL, 'task', 'rejected', NULL, ?3, ?4, NULL, ?5)",
        rusqlite::params![
            "ver-legacy-private",
            "cas-legacy-private",
            format!("legacy row contains {raw_capability}"),
            serde_json::json!([raw_path, "src/lib.rs"]).to_string(),
            chrono::Utc::now().to_rfc3339(),
        ],
    )
    .expect("insert legacy verification");
    conn.execute(
        "INSERT INTO verification_issues
         (verification_id, file, line, severity, category, code, problem, suggestion)
         VALUES (?1, ?2, 7, 'blocking', 'security', ?3, ?4, 'password=legacy-secret')",
        rusqlite::params!["ver-legacy-private", raw_path, raw_pat, raw_control],
    )
    .expect("insert legacy issue");
    drop(conn);

    let show = extract_text(
        service
            .cas_verification_show(Parameters(VerificationShowRequest {
                id: "ver-legacy-private".to_string(),
            }))
            .await
            .expect("show legacy verification"),
    );
    let list = extract_text(
        service
            .cas_verification_list(Parameters(VerificationListRequest {
                task_id: "cas-legacy-private".to_string(),
                limit: Some(10),
            }))
            .await
            .expect("list legacy verification"),
    );
    let latest = extract_text(
        service
            .cas_verification_latest(Parameters(VerificationListRequest {
                task_id: "cas-legacy-private".to_string(),
                limit: Some(1),
            }))
            .await
            .expect("latest legacy verification"),
    );

    assert!(show.contains("[REDACTED_SECRET]"));
    assert!(show.contains("[REDACTED_PATH]"));
    assert!(show.contains("[REDACTED_CONTROL]"));
    assert!(show.contains("src/lib.rs"));
    assert!(list.contains("[REDACTED_SECRET]"));
    assert!(latest.contains("[REDACTED_SECRET]"));
    for (surface, payload) in [("show", show), ("list", list), ("latest", latest)] {
        for unsafe_value in [
            raw_capability,
            raw_path,
            raw_pat,
            raw_control,
            "legacy-secret",
        ] {
            assert!(
                !payload.contains(unsafe_value),
                "{surface} leaked legacy verifier content: {unsafe_value:?}"
            );
        }
    }
}

#[tokio::test]
async fn test_official_child_uses_server_handoff_without_model_visible_bearer() {
    let (temp, parent_service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("agent store");
    let verification_store = open_verification_store(&cas_dir).expect("verification store");
    let parent_id = format!("test-session-{}", std::process::id());

    let created = parent_service
        .cas_task_create(Parameters(simple_task_req("Server-side verifier handoff")))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();
    let dispatch = cas_store::create_verification_dispatch(
        &cas_dir,
        &task_id,
        &parent_id,
        &parent_id,
        chrono::Utc::now() + chrono::Duration::minutes(10),
    )
    .expect("create exact dispatch");

    let prompt = format!("Review exact CAS task {task_id}");
    let pre_input = cas::hooks::HookInput {
        session_id: parent_id.clone(),
        cwd: temp.path().to_string_lossy().to_string(),
        hook_event_name: "PreToolUse".to_string(),
        tool_name: Some("Agent".to_string()),
        tool_input: Some(serde_json::json!({
            "subagent_type": "task-verifier",
            "prompt": prompt,
        })),
        tool_use_id: Some("tool-use-public-handoff".to_string()),
        ..Default::default()
    };
    let pre_output =
        cas::hooks::handle_pre_tool_use(&pre_input, Some(&cas_dir)).expect("PreToolUse");
    let pre_json = serde_json::to_value(pre_output).expect("serialize PreToolUse");
    assert_eq!(
        pre_json,
        serde_json::json!({}),
        "PreToolUse must not emit updatedInput, context, or bearer material"
    );
    assert_eq!(
        pre_input
            .tool_input
            .as_ref()
            .and_then(|value| value.get("prompt"))
            .and_then(|value| value.as_str()),
        Some(prompt.as_str()),
        "original model-visible prompt must remain byte-identical"
    );

    let child_id = format!("official-task-verifier-child-{}", std::process::id());
    let child_input: cas::hooks::HookInput = serde_json::from_value(serde_json::json!({
        "session_id": parent_id.clone(),
        "transcript_path": "/portable/parent.jsonl",
        "cwd": temp.path(),
        "permission_mode": "default",
        "hook_event_name": "SubagentStart",
        "agent_id": child_id.clone(),
        "agent_type": "task-verifier"
    }))
    .expect("official SubagentStart payload");
    let child_output =
        cas::hooks::handle_subagent_start(&child_input, Some(&cas_dir)).expect("SubagentStart");
    assert_eq!(
        serde_json::to_value(child_output).expect("serialize SubagentStart"),
        serde_json::json!({})
    );

    let child = agent_store.get(&child_id).expect("registered child");
    assert_eq!(child.agent_type, cas::types::AgentType::SubAgent);
    assert_eq!(child.role, AgentRole::Standard);
    assert_eq!(child.parent_id.as_deref(), Some(parent_id.as_str()));

    let ordinary_child_id = format!("ordinary-child-{}", std::process::id());
    let ordinary_child = cas::types::Agent::new_sub_agent(
        ordinary_child_id.clone(),
        "general-purpose".to_string(),
        parent_id.clone(),
    );
    agent_store
        .register(&ordinary_child)
        .expect("register ordinary child");
    let ordinary_child_service = cas::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
    ordinary_child_service.set_agent_id_for_testing(ordinary_child_id);
    let ordinary_err = ordinary_child_service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: task_id.clone(),
            status: "approved".to_string(),
            summary: "ordinary standard child without authority".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(dispatch.id.clone()),
        }))
        .await
        .expect_err("ordinary standard child omission must fail closed");
    assert_eq!(ordinary_err.code, rmcp::model::ErrorCode::INVALID_PARAMS);

    let child_service = cas::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
    child_service.set_agent_id_for_testing(child_id.clone());

    let wrong_child_id = format!("wrong-task-verifier-child-{}", std::process::id());
    let wrong_child = cas::types::Agent::new_sub_agent(
        wrong_child_id.clone(),
        "task-verifier".to_string(),
        parent_id.clone(),
    );
    agent_store
        .register(&wrong_child)
        .expect("register wrong verifier child");
    let wrong_child_service = cas::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
    wrong_child_service.set_agent_id_for_testing(wrong_child_id);
    let wrong_child_err = wrong_child_service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: task_id.clone(),
            status: "approved".to_string(),
            summary: "different child attempting sealed authority".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(dispatch.id.clone()),
        }))
        .await
        .expect_err("handoff must be exact-child bound");
    assert_eq!(wrong_child_err.code, rmcp::model::ErrorCode::INVALID_PARAMS);

    let wrong_dispatch_err = child_service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: task_id.clone(),
            status: "approved".to_string(),
            summary: "bound child naming a different dispatch".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some("vdisp-wrong-boundary".to_string()),
        }))
        .await
        .expect_err("handoff must be exact-dispatch bound");
    assert_eq!(
        wrong_dispatch_err.code,
        rmcp::model::ErrorCode::INVALID_PARAMS
    );

    let other_created = parent_service
        .cas_task_create(Parameters(simple_task_req("Unrelated handoff task")))
        .await
        .expect("create unrelated task");
    let other_task_id = extract_task_id(&extract_text(other_created))
        .expect("other task id")
        .to_string();
    let wrong_task_err = child_service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: other_task_id,
            status: "approved".to_string(),
            summary: "bound child targeting a different task".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(dispatch.id.clone()),
        }))
        .await
        .expect_err("handoff must be exact-task bound");
    assert_eq!(wrong_task_err.code, rmcp::model::ErrorCode::INVALID_PARAMS);

    let mut wrong_parent_child = agent_store.get(&child_id).expect("bound child");
    wrong_parent_child.parent_id = Some("different-registered-parent".to_string());
    agent_store
        .update(&wrong_parent_child)
        .expect("mutate child parent for negative case");
    let wrong_parent_err = child_service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: task_id.clone(),
            status: "approved".to_string(),
            summary: "bound child with a different registered parent".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(dispatch.id.clone()),
        }))
        .await
        .expect_err("handoff must be exact-parent bound");
    assert_eq!(
        wrong_parent_err.code,
        rmcp::model::ErrorCode::INVALID_PARAMS
    );
    wrong_parent_child.parent_id = Some(parent_id.clone());
    agent_store
        .update(&wrong_parent_child)
        .expect("restore exact bound parent");

    child_service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: task_id.clone(),
            status: "approved".to_string(),
            summary: "official child verified through sealed handoff".to_string(),
            confidence: Some(0.99),
            issues: None,
            files_reviewed: Some("src/lib.rs".to_string()),
            duration_ms: Some(17),
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(dispatch.id.clone()),
        }))
        .await
        .expect("server-bound child verifies without bearer");

    let row = verification_store
        .get_latest_for_task(&task_id)
        .expect("verification lookup")
        .expect("verification row");
    assert_eq!(
        row.provenance,
        cas::types::VerificationProvenance::TaskVerifier
    );
    assert_eq!(row.agent_id.as_deref(), Some(child_id.as_str()));
    assert_eq!(row.dispatch_id.as_deref(), Some(dispatch.id.as_str()));
    assert!(
        row.capability_id
            .as_deref()
            .is_some_and(|id| id.starts_with("vhnd-"))
    );

    let replay = child_service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id,
            status: "approved".to_string(),
            summary: "replay".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(dispatch.id),
        }))
        .await
        .expect_err("consumed server handoff cannot replay");
    assert_eq!(replay.code, rmcp::model::ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn test_spoofed_supervisor_and_codex_claims_do_not_grant_authority() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let _spoofed_env = ScopedSupervisorEnv::new();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("agent store");
    let worker_id = format!("test-session-{}", std::process::id());
    let mut worker = agent_store.get(&worker_id).expect("test caller");
    worker.name = "Codex".to_string();
    worker.role = AgentRole::Worker;
    worker.agent_type = cas::types::AgentType::Worker;
    agent_store
        .update(&worker)
        .expect("persist worker identity");

    let created = service
        .cas_task_create(Parameters(simple_task_req("Spoof resistance")))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();
    let err = service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id,
            status: "approved".to_string(),
            summary: "caller claims trusted verifier identity".to_string(),
            confidence: Some(1.0),
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: Some("epic".to_string()),
            verifier_capability: None,
            dispatch_id: None,
        }))
        .await
        .expect_err("environment, Codex name, and epic type cannot spoof authority");
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn test_anonymous_or_orphan_verification_add_fails_closed() {
    let _env_lock = env_test_lock();
    let _anonymous_env = ScopedFactoryEnv::apply(&[
        ("CAS_SESSION_ID", None),
        ("CAS_AGENT_NAME", None),
        ("CAS_AGENT_ROLE", Some("supervisor")),
    ]);
    let temp = TempDir::new().expect("temp dir");
    let cas_dir = init_cas_dir(temp.path()).expect("init cas");
    let task_store = open_task_store(&cas_dir).expect("task store");
    let task = cas::types::Task::new(
        "cas-anonymous-verification".to_string(),
        "Anonymous verifier must fail".to_string(),
    );
    task_store.add(&task).expect("task");

    let service = cas::mcp::CasCore::with_daemon(cas_dir, None, None);
    let err = service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: task.id,
            status: "approved".to_string(),
            summary: "orphan supervisor claim".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: None,
        }))
        .await
        .expect_err("anonymous/orphan caller must fail");
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_REQUEST);
}

fn task_status_update(id: &str, status: Option<&str>, notes: Option<&str>) -> TaskUpdateRequest {
    TaskUpdateRequest {
        blocked_by: None,
        depth: None,
        id: id.to_string(),
        title: None,
        notes: notes.map(str::to_string),
        priority: None,
        labels: None,
        description: None,
        design: None,
        acceptance_criteria: None,
        demo_statement: None,
        execution_note: None,
        external_ref: None,
        assignee: None,
        status: status.map(str::to_string),
        epic: None,
        origin_project: None,
        epic_verification_owner: None,
    }
}

/// Seed a supervisor-direct verdict through the generic store contract with
/// the same durable authority fields that production `verification.add`
/// derives before insertion. Tests using this helper exercise downstream
/// close/update behavior, not public verifier authentication.
fn add_exact_supervisor_fixture_verdict(
    cas_dir: &std::path::Path,
    mut verification: Verification,
    dispatch_id: Option<&str>,
) -> cas::types::VerificationDispatch {
    const SUPERVISOR_ID: &str = "fixture-durable-supervisor";
    let agent_store = open_agent_store(cas_dir).expect("agent store");
    let mut supervisor =
        cas::types::Agent::new(SUPERVISOR_ID.to_string(), "fixture-supervisor".to_string());
    supervisor.role = AgentRole::Supervisor;
    agent_store
        .register(&supervisor)
        .expect("register durable fixture supervisor");

    let dispatch = match dispatch_id {
        Some(id) => cas_store::get_verification_dispatch(cas_dir, id).expect("exact dispatch"),
        None => cas_store::create_verification_dispatch(
            cas_dir,
            &verification.task_id,
            "fixture-requester",
            SUPERVISOR_ID,
            chrono::Utc::now() + chrono::Duration::minutes(10),
        )
        .expect("create exact fixture dispatch"),
    };
    verification.provenance = cas::types::VerificationProvenance::SupervisorDirect;
    verification.agent_id = Some(SUPERVISOR_ID.to_string());
    verification.issuer_agent_id = Some(SUPERVISOR_ID.to_string());
    verification.dispatch_id = Some(dispatch.id.clone());
    open_verification_store(cas_dir)
        .expect("verification store")
        .add(&verification)
        .expect("persist exact supervisor verdict");
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("db");
    cas_store::resolve_verification_dispatch_with_conn(
        &conn,
        &dispatch.id,
        SUPERVISOR_ID,
        None,
        true,
    )
    .expect("resolve exact fixture dispatch");
    dispatch
}

#[tokio::test]
async fn test_update_to_closed_is_exact_task_gated_but_other_task_update_remains_available() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("enable verification");

    let create_a = service
        .cas_task_create(Parameters(simple_task_req("Pending close A")))
        .await
        .expect("create A");
    let id_a = extract_task_id(&extract_text(create_a))
        .expect("id A")
        .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("start A");

    let create_b = service
        .cas_task_create(Parameters(simple_task_req("Unrelated task B")))
        .await
        .expect("create B");
    let id_b = extract_task_id(&extract_text(create_b))
        .expect("id B")
        .to_string();

    let close_err = service
        .cas_task_update(Parameters(task_status_update(&id_a, Some("closed"), None)))
        .await
        .expect_err("A update-to-closed must require legitimate verification");
    assert!(
        close_err.message.contains(&id_a) && close_err.message.contains("VERIFICATION REQUIRED"),
        "gate must identify only task A: {}",
        close_err.message
    );

    let unrelated = service
        .cas_task_update(Parameters(task_status_update(
            &id_b,
            None,
            Some("still available while A waits"),
        )))
        .await
        .expect("unrelated B update remains available");
    assert!(extract_text(unrelated).contains("notes"));

    let approved = Verification::approved(
        "ver-update-close-authorized".to_string(),
        id_a.clone(),
        "registered supervisor verdict".to_string(),
    );
    add_exact_supervisor_fixture_verdict(&cas_dir, approved, None);

    service
        .cas_task_update(Parameters(task_status_update(&id_a, Some("closed"), None)))
        .await
        .expect("legitimate verdict allows exact task close update");
    assert_eq!(
        open_task_store(&cas_dir)
            .expect("task store")
            .get(&id_a)
            .expect("A")
            .status,
        TaskStatus::Closed
    );
}

#[tokio::test]
async fn test_update_to_closed_rejects_stale_task_row_behind_current_dispatch() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("enable verification");
    let created = service
        .cas_task_create(Parameters(simple_task_req("Exact update boundary")))
        .await
        .expect("create");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();
    service
        .cas_task_start(Parameters(IdRequest {
            id: task_id.clone(),
        }))
        .await
        .expect("start");

    let verification_store = open_verification_store(&cas_dir).expect("verification store");
    verification_store
        .add(&Verification::approved(
            "ver-stale-task-row".to_string(),
            task_id.clone(),
            "older task-wide approval".to_string(),
        ))
        .expect("stale readable row");
    let dispatch = cas_store::create_verification_dispatch_bound(
        &cas_dir,
        &task_id,
        "requester",
        "registered-supervisor",
        &cas::types::VerificationProofBoundary::task(),
        chrono::Utc::now() + chrono::Duration::minutes(10),
        false,
    )
    .expect("current dispatch");

    service
        .cas_task_update(Parameters(task_status_update(
            &task_id,
            Some("closed"),
            None,
        )))
        .await
        .expect_err("older task-wide approval must not authorize a current dispatch");

    let exact = Verification::approved(
        "ver-exact-update".to_string(),
        task_id.clone(),
        "exact current approval".to_string(),
    );
    add_exact_supervisor_fixture_verdict(&cas_dir, exact, Some(&dispatch.id));

    service
        .cas_task_update(Parameters(task_status_update(
            &task_id,
            Some("closed"),
            None,
        )))
        .await
        .expect("exact current verdict authorizes update");

    {
        let _supervisor = ScopedSupervisorEnv::new();
        service
            .cas_task_reopen(Parameters(TaskReopenRequest {
                id: task_id.clone(),
                reason: Some("invalidate approved proof before rework".to_string()),
            }))
            .await
            .expect("supervisor reopens exact task proof scope");
    }
    assert_eq!(
        cas_store::get_latest_verification_dispatch(&cas_dir, &task_id)
            .unwrap()
            .unwrap()
            .state,
        cas::types::VerificationDispatchState::Invalidated,
        "reopen must invalidate the prior proof cycle"
    );
    service
        .cas_task_update(Parameters(task_status_update(
            &task_id,
            Some("closed"),
            None,
        )))
        .await
        .expect_err("reopened task cannot reuse the invalidated verdict");
}

// cas-3bd4: env_test_lock() now lives in `support.rs` so `setup_cas()`
// can hold it while clearing factory env vars. Tests that need to set
// `CAS_AGENT_ROLE=supervisor` via `ScopedSupervisorEnv` MUST call
// `setup_cas()` FIRST and then acquire `env_test_lock()` — see the
// support.rs docs. Acquiring before calling `setup_cas` would deadlock
// because std `Mutex` is not re-entrant.

#[tokio::test]
async fn test_task_close_blocked_without_verification() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Initialize verification store
    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Create task
    let req = TaskCreateRequest {
        depth: None,
        title: "Task requiring verification".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Start task
    let start_req = IdRequest { id: id.to_string() };
    let _ = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    // Try to close task without verification - should be blocked
    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.to_string(),
        reason: Some("Completed".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");

    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "Close should be blocked without verification: {text}"
    );
    assert!(
        text.contains("Task(subagent_type=\"task-verifier\""),
        "Close warning must include explicit Task() spawn syntax: {text}"
    );

    // A durable dispatch-request verification row must be persisted so the
    // close attempt is observable (no more fire-and-forget). The verdict
    // row will be written later by the task-verifier subagent.
    let latest = verification_store
        .get_latest_for_task(id)
        .unwrap()
        .expect("dispatch-request verification row should exist after close");
    assert_eq!(
        latest.status,
        cas::types::VerificationStatus::Error,
        "Dispatch-request row should have Error status until the subagent writes a verdict"
    );
    assert!(
        latest.summary.contains("Dispatch requested"),
        "Dispatch-request row summary should identify itself: {}",
        latest.summary
    );
}

#[tokio::test]
async fn test_task_close_sets_assignee_for_worktree_merge_jail() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[verification]
enabled = false

[worktrees]
enabled = true
require_merge_on_epic_close = true
"#,
    )
    .expect("should write config");

    let req = TaskCreateRequest {
        depth: None,
        title: "Task with worktree".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    let worktree_store = open_worktree_store(&cas_dir).expect("open worktree store");
    worktree_store.init().expect("init worktree store");
    let worktree_id = Worktree::generate_id();
    let worktree = Worktree::new(
        worktree_id.clone(),
        "cas/test-worktree".to_string(),
        "main".to_string(),
        temp.path().join("worktree"),
    );
    worktree_store.add(&worktree).expect("should add worktree");

    let task_store = open_task_store(&cas_dir).expect("open task store");
    let mut task = task_store.get(id).expect("task should exist");
    task.worktree_id = Some(worktree_id);
    task_store.update(&task).expect("should update task");

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: task.id.clone(),
        reason: Some("Done".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return result");

    let text = extract_text(result);
    assert!(
        text.contains("WORKTREE MERGE REQUIRED"),
        "Close should be blocked for merge: {text}"
    );

    let task = task_store.get(&task.id).expect("task should exist");
    assert!(
        task.pending_worktree_merge,
        "pending_worktree_merge should be set"
    );

    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let agent_id = agent_store
        .list(None)
        .expect("list agents")
        .first()
        .map(|a| a.id.clone())
        .expect("agent should exist");
    assert_eq!(
        task.assignee.as_deref(),
        Some(agent_id.as_str()),
        "assignee should be set to current agent"
    );
}

// cas-6a99 helper: minimal task-create request.
fn simple_task_req(title: &str) -> TaskCreateRequest {
    TaskCreateRequest {
        depth: None,
        title: title.to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    }
}

/// cas-6a99: a sibling task that is merge-gated (`pending_worktree_merge=true`,
/// i.e. "work complete, awaiting supervisor merge") must NOT jail the worker
/// from starting an unrelated task. The worker cannot resolve a merge gate (the
/// supervisor owns the merge), so coupling `start` of B to A's awaiting-merge
/// state is wrong. This is distinct from the verification jail
/// (`pending_verification` / no approved verification), which still blocks —
/// the negative control at the end proves the jail is otherwise intact.
#[tokio::test]
async fn test_task_start_not_blocked_by_merge_gated_sibling_cas_6a99() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Verification ENABLED so check_pending_verification actually runs.
    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("should write config");

    // Task A — start it (claims a lease + sets InProgress + registers the agent).
    let res = service
        .cas_task_create(Parameters(simple_task_req("Task A")))
        .await
        .expect("create A");
    let id_a = extract_task_id(&extract_text(res))
        .expect("id A")
        .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("start A");

    // Simulate a merge-gated close: A is work-complete, awaiting supervisor merge.
    let task_store = open_task_store(&cas_dir).expect("open task store");
    let mut a = task_store.get(&id_a).expect("A exists");
    a.pending_worktree_merge = true;
    task_store.update(&a).expect("flag A merge-gated");

    // Task B — unrelated, no dependency edge on A. Starting it must NOT be blocked.
    let res = service
        .cas_task_create(Parameters(simple_task_req("Task B")))
        .await
        .expect("create B");
    let id_b = extract_task_id(&extract_text(res))
        .expect("id B")
        .to_string();
    let text = extract_text(
        service
            .cas_task_start(Parameters(IdRequest { id: id_b }))
            .await
            .expect("start B should return"),
    );
    assert!(
        !text.contains("VERIFICATION PENDING"),
        "merge-gated sibling A must not block starting B, got: {text}"
    );

    // Clear the merge gate: A is now an unverified InProgress task, but C still
    // starts because verification enforcement is scoped to A's close.
    let mut a = task_store.get(&id_a).expect("A exists");
    a.pending_worktree_merge = false;
    task_store.update(&a).expect("clear A merge gate");
    let res = service
        .cas_task_create(Parameters(simple_task_req("Task C")))
        .await
        .expect("create C");
    let id_c = extract_task_id(&extract_text(res))
        .expect("id C")
        .to_string();
    let text = extract_text(
        service
            .cas_task_start(Parameters(IdRequest { id: id_c }))
            .await
            .expect("start C should return"),
    );
    assert!(
        !text.contains("VERIFICATION PENDING"),
        "unverified sibling A must not block starting C, got: {text}"
    );
}

/// cas-7aef: a normal successful close must record its lease release reason in
/// the dedicated reason field and surface it through the lease-history MCP
/// renderer.
#[tokio::test]
async fn test_normal_close_records_and_renders_lease_history_reason_cas_7aef() {
    let (temp, service) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = false\n",
    )
    .expect("write config");

    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(simple_task_req("Normal close reason")))
            .await
            .expect("create task"),
    ))
    .expect("task id")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("start task");

    let close_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id.clone(),
                reason: Some("implementation complete".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close task"),
    );
    assert!(
        close_text.contains("Closed task:"),
        "normal close should succeed: {close_text}"
    );

    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let lease_history = agent_store
        .get_lease_history(&id, Some(1))
        .expect("lease history for normally closed task");
    assert_eq!(lease_history[0].event_type, "released");
    assert_eq!(lease_history[0].reason.as_deref(), Some("Task closed"));
    assert_eq!(
        lease_history[0].previous_agent_id, None,
        "normal close reason must not be stored as a transfer agent ID"
    );

    let rendered = extract_text(
        service
            .cas_lease_history(Parameters(LeaseHistoryRequest {
                task_id: id,
                limit: Some(1),
            }))
            .await
            .expect("render lease history"),
    );
    assert!(
        rendered.contains("(reason: Task closed)"),
        "renderer must surface the dedicated reason: {rendered}"
    );
    assert!(
        !rendered.contains("(from Task closed)"),
        "renderer must not present a reason as a transfer agent: {rendered}"
    );
}

/// cas-8d5b: the close-time MERGE REQUIRED data-state guard must park the task
/// in a non-worker-actionable state and release the worker lease. The worker can
/// then start unrelated assigned work without a supervisor manually flipping the
/// first task to Blocked.
#[tokio::test]
async fn test_supervisor_negative_result_closes_unmerged_experiment_with_receipts_cas_6c50() {
    let (temp, service, supervisor_id) = setup_cas_with_supervisor_session();
    let _env_lock = env_test_lock();
    let _supervisor_env = ScopedSupervisorEnv::new();
    let cas_dir = temp.path().join(".cas");
    let artifacts_root = temp.path().join("durable-artifacts");
    std::fs::create_dir_all(&artifacts_root).unwrap();
    std::fs::write(
        cas_dir.join("config.toml"),
        format!(
            "[factory]\nartifacts_root = {:?}\n[verification]\nenabled = false\n",
            artifacts_root.display().to_string()
        ),
    )
    .unwrap();

    let repo = temp.path();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "measurement worker")
            .env("GIT_AUTHOR_EMAIL", "measurement@example.test")
            .env("GIT_COMMITTER_NAME", "measurement worker")
            .env("GIT_COMMITTER_EMAIL", "measurement@example.test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);

    let task_id = "cas-negative-result";
    let task_store = open_task_store(&cas_dir).unwrap();
    let mut task = cas::types::Task::new(
        task_id.to_string(),
        "Measure an experiment that must not land".to_string(),
    );
    task.status = TaskStatus::InProgress;
    task.assignee = Some("measurement-worker".to_string());
    task_store.add(&task).unwrap();

    git(&["checkout", "-q", "-b", "factory/measurement-worker"]);
    std::fs::write(repo.join("experiment.yml"), "slower: true\n").unwrap();
    git(&["add", "experiment.yml"]);
    git(&["commit", "-q", "-m", "experiment: measured regression"]);

    let agent_store = open_agent_store(&cas_dir).unwrap();
    let mut caller = agent_store.get(&supervisor_id).unwrap();
    caller.role = AgentRole::Worker;
    agent_store.update(&caller).unwrap();
    let unauthorized = extract_text(
        service
            .cas_task_close_with_completion(
                Parameters(TaskCloseRequest {
                    stranded_branch_override: None,
                    id: task_id.to_string(),
                    reason: Some("worker tries to discard its own delivery".to_string()),
                    supervisor_override: None,
                    legacy_bypass_code_review: None,
                    search_manifest: None,
                    commit_receipt: None,
                }),
                None,
                Some(NegativeResultCloseRequest {
                    artifact_path: None,
                    reference: None,
                }),
                None,
                None,
            )
            .await
            .unwrap(),
    );
    assert!(unauthorized.contains("only a live registered supervisor"));
    assert_eq!(
        task_store.get(task_id).unwrap().status,
        TaskStatus::InProgress
    );
    caller.role = AgentRole::Supervisor;
    agent_store.update(&caller).unwrap();

    let ordinary = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: task_id.to_string(),
                reason: Some("measurement complete".to_string()),
                supervisor_override: Some(true),
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .unwrap(),
    );
    assert!(ordinary.contains("MERGE REQUIRED"), "{ordinary}");
    assert!(ordinary.contains("negative_result=true"), "{ordinary}");
    assert_eq!(
        task_store.get(task_id).unwrap().status,
        TaskStatus::AwaitingMerge,
        "ordinary delivery path must retain the existing merge gate"
    );

    let missing = extract_text(
        service
            .cas_task_close_with_completion(
                Parameters(TaskCloseRequest {
                    stranded_branch_override: None,
                    id: task_id.to_string(),
                    reason: None,
                    supervisor_override: None,
                    legacy_bypass_code_review: None,
                    search_manifest: None,
                    commit_receipt: None,
                }),
                None,
                Some(NegativeResultCloseRequest {
                    artifact_path: None,
                    reference: None,
                }),
                None,
                None,
            )
            .await
            .unwrap(),
    );
    for field in [
        "negative_result_artifact_path",
        "negative_result_reference",
        "reason (the supervisor decision rationale)",
    ] {
        assert!(missing.contains(field), "missing `{field}` in: {missing}");
    }
    assert_eq!(
        task_store.get(task_id).unwrap().status,
        TaskStatus::AwaitingMerge
    );

    let task_artifacts = artifacts_root.join(task_id);
    std::fs::create_dir_all(&task_artifacts).unwrap();
    let proof = task_artifacts.join("measurement.json");
    std::fs::write(&proof, r#"{"baseline_seconds":10,"experiment_seconds":25}"#).unwrap();
    let reference = "https://github.com/pippenz/cas/pull/242";
    let closed = extract_text(
        service
            .cas_task_close_with_completion(
                Parameters(TaskCloseRequest {
                    stranded_branch_override: None,
                    id: task_id.to_string(),
                    reason: Some("experiment regressed the dominant path; do not ship".to_string()),
                    supervisor_override: None,
                    legacy_bypass_code_review: None,
                    search_manifest: None,
                    commit_receipt: None,
                }),
                None,
                Some(NegativeResultCloseRequest {
                    artifact_path: Some(proof.display().to_string()),
                    reference: Some(reference.to_string()),
                }),
                None,
                None,
            )
            .await
            .unwrap(),
    );
    assert!(closed.contains("Closed task:"), "{closed}");
    assert!(closed.contains("measured negative result"), "{closed}");

    let closed_task = task_store.get(task_id).unwrap();
    assert_eq!(closed_task.status, TaskStatus::Closed);
    let receipt = closed_task
        .deliverables
        .negative_result
        .as_ref()
        .expect("structured negative-result evidence");
    assert_eq!(receipt.artifact_path, proof.display().to_string());
    assert_eq!(receipt.reference, reference);
    assert_eq!(receipt.supervisor_id, supervisor_id);
    assert!(closed_task.notes.contains("DECISION: supervisor"));
    assert!(closed_task.notes.contains("intentionally not merged"));
}

#[tokio::test]
async fn test_merge_required_close_parks_awaiting_merge_and_releases_gate_cas_8d5b() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent.role = AgentRole::Worker;
        agent_store.update(&agent).expect("mark test agent worker");
    }

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("write config");

    let repo = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "epic/cas-8d5b"]);
    git(&["checkout", "-q", "-b", "factory/test-agent"]);
    std::fs::write(repo.join("worker.txt"), "worker\n").unwrap();
    git(&["add", "worker.txt"]);
    git(&["commit", "-q", "-m", "worker change"]);

    let task_store = open_task_store(&cas_dir).expect("open task store");

    let epic_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: "Merge epic".to_string(),
                description: None,
                priority: 2,
                task_type: "epic".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create epic"),
    ))
    .expect("epic id")
    .to_string();
    {
        let mut epic = task_store.get(&epic_id).expect("epic exists");
        epic.branch = Some("epic/cas-8d5b".to_string());
        task_store.update(&epic).expect("update epic branch");
    }

    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id.clone()),
                ..simple_task_req("Task A")
            }))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("start A");
    {
        let mut task_a = task_store.get(&id_a).expect("A exists after start");
        task_a.assignee = Some("test-agent".to_string());
        task_store.update(&task_a).expect("set A assignee");
    }

    let close_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("ready for merge".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close A returns"),
    );
    assert!(
        close_text.contains("MERGE REQUIRED"),
        "close must reject on stranded factory branch: {close_text}"
    );

    let parked = task_store.get(&id_a).expect("A exists");
    assert_eq!(parked.status, TaskStatus::AwaitingMerge);
    assert!(!parked.pending_verification);
    assert!(!parked.pending_worktree_merge);
    assert!(
        parked.notes.contains("awaiting_merge"),
        "audit note should name parked state: {}",
        parked.notes
    );

    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let agent_id = agent_store
        .list(None)
        .expect("list agents")
        .into_iter()
        .find(|agent| agent.name == "test-agent")
        .expect("test agent exists")
        .id;
    assert!(
        agent_store
            .list_agent_leases(&agent_id)
            .expect("list leases")
            .iter()
            .all(|lease| lease.task_id != id_a),
        "MERGE REQUIRED close must release A's active lease"
    );
    let lease_history = agent_store
        .get_lease_history(&id_a, Some(1))
        .expect("lease history for parked task");
    assert_eq!(lease_history[0].event_type, "released");
    assert_eq!(
        lease_history[0].reason.as_deref(),
        Some("MERGE REQUIRED: parked awaiting_merge"),
        "MERGE REQUIRED park path must not record the successful close reason"
    );
    assert_eq!(lease_history[0].previous_agent_id, None);

    // cas-627f: the flagship close-rejected `WorkerIdle` notification is
    // built from `AgentSummary::active_lease`, which used to be resolved
    // ONLY from `list_agent_leases` (status='active' rows). Since the
    // assertion above just proved A's lease is released, `active_lease`
    // must now fall back to resolving A by assignee + AwaitingMerge status
    // directly from the task table — confirmed P1,
    // docs/reviews/2026-07-07-cas-b646-epic.md.
    let director_data = cas_factory::DirectorData::load_fast(&cas_dir).expect("load director data");
    let agent_summary = director_data
        .agents
        .iter()
        .find(|a| a.name == "test-agent")
        .expect("test-agent present in director data");
    let active_lease = agent_summary.active_lease.as_ref().expect(
        "active_lease must resolve for the parked AwaitingMerge task even with the lease released",
    );
    assert_eq!(active_lease.task_id, id_a);
    assert_eq!(active_lease.task_status, TaskStatus::AwaitingMerge);
    assert_eq!(
        active_lease.close_rejected_reason.as_deref(),
        Some("MERGE REQUIRED"),
        "close_rejected_reason must carry the rejection reason for the operator notification"
    );

    let id_b = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(simple_task_req("Task B")))
            .await
            .expect("create B"),
    ))
    .expect("id B")
    .to_string();
    let start_b = extract_text(
        service
            .cas_task_start(Parameters(IdRequest { id: id_b }))
            .await
            .expect("start B should return"),
    );
    assert!(
        start_b.contains("Started task:"),
        "awaiting_merge A must not block the worker's next task: {start_b}"
    );
    assert!(
        !start_b.contains("VERIFICATION PENDING"),
        "awaiting_merge A must not trip verification jail: {start_b}"
    );

    git(&["checkout", "-q", "epic/cas-8d5b"]);
    git(&["merge", "--no-ff", "-q", "factory/test-agent"]);
    git(&["checkout", "-q", "factory/test-agent"]);
    let verification_store = open_verification_store(&cas_dir).expect("open verification store");
    verification_store
        .add(&Verification::approved(
            "ver-cas-8d5b".to_string(),
            id_a.clone(),
            "Simulated approval after supervisor merge".to_string(),
        ))
        .expect("record verification approval");
    let close_after_merge = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("merged and ready to close".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close A after merge returns"),
    );
    assert!(
        close_after_merge.contains("Closed task:"),
        "awaiting_merge task must become closeable after merge guard passes: {close_after_merge}"
    );
    let closed = task_store.get(&id_a).expect("A exists");
    assert_eq!(closed.status, TaskStatus::Closed);
    assert!(
        closed.deliverables.factory_branch_anchor.is_some(),
        "successful close must preserve the task-specific anchor as a durable \
         receipt for the parent epic close guard"
    );
}

/// cas-a844: when the worker's factory branch has a genuine git merge
/// conflict against the parent branch (not just unmerged commits), the
/// MERGE REQUIRED close rejection must say so and name the alternative
/// (worker `task start`), and the parked task must record
/// `deliverables.merge_conflicted = true` — so status output never reads a
/// conflicted park identically to a clean, supervisor-actionable one.
#[tokio::test]
async fn test_a844_merge_conflict_flags_task_and_names_alternative() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent.role = AgentRole::Worker;
        agent_store.update(&agent).expect("mark test agent worker");
    }

    let repo = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "epic/cas-a844"]);
    git(&["checkout", "-q", "-b", "factory/test-agent"]);
    // Worker edits seed.txt on its factory branch.
    std::fs::write(repo.join("seed.txt"), "worker's edit\n").unwrap();
    git(&["commit", "-q", "-am", "worker edits seed"]);
    // The epic branch picks up a CONFLICTING edit to the same file
    // underneath the worker (e.g. another task landed and touched it).
    git(&["checkout", "-q", "epic/cas-a844"]);
    std::fs::write(repo.join("seed.txt"), "epic's conflicting edit\n").unwrap();
    git(&["commit", "-q", "-am", "epic edits seed differently"]);
    git(&["checkout", "-q", "factory/test-agent"]);

    let task_store = open_task_store(&cas_dir).expect("open task store");

    let epic_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: "Merge epic".to_string(),
                description: None,
                priority: 2,
                task_type: "epic".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create epic"),
    ))
    .expect("epic id")
    .to_string();
    {
        let mut epic = task_store.get(&epic_id).expect("epic exists");
        epic.branch = Some("epic/cas-a844".to_string());
        task_store.update(&epic).expect("update epic branch");
    }

    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id.clone()),
                ..simple_task_req("Task A")
            }))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("start A");
    {
        let mut task_a = task_store.get(&id_a).expect("A exists after start");
        task_a.assignee = Some("test-agent".to_string());
        task_store.update(&task_a).expect("set A assignee");
    }

    let close_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("ready for merge".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close A returns"),
    );
    assert!(
        close_text.contains("MERGE REQUIRED"),
        "close must still reject on stranded factory branch: {close_text}"
    );
    assert!(
        close_text.to_lowercase().contains("conflict"),
        "refusal must say this is a genuine conflict, not just unmerged commits: {close_text}"
    );
    assert!(
        close_text.contains("task action=start") || close_text.contains("action=start"),
        "refusal must name the worker task-start alternative: {close_text}"
    );
    assert!(
        close_text.contains("seed.txt"),
        "refusal must name the actual conflicting file(s), not just say 'conflict': {close_text}"
    );

    let parked = task_store.get(&id_a).expect("A exists");
    assert_eq!(parked.status, TaskStatus::AwaitingMerge);
    assert!(
        parked.deliverables.merge_conflicted,
        "a genuine conflict must be flagged on the parked task"
    );
    assert_eq!(
        parked.deliverables.parked_branch.as_deref(),
        Some("factory/test-agent")
    );

    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("assigned worker can resume conflicted park");
    let resumed = task_store.get(&id_a).expect("A exists after resume");
    assert_eq!(resumed.status, TaskStatus::InProgress);
    assert!(
        resumed.deliverables.factory_branch_anchor.is_none(),
        "conflict rework must invalidate the anchor captured by MERGE REQUIRED"
    );
    assert!(
        resumed.deliverables.parked_branch.is_none(),
        "conflict rework must clear the parked branch receipt"
    );
    assert!(
        !resumed.deliverables.merge_conflicted,
        "conflict rework must clear the prior close cycle's conflict flag"
    );
    assert!(
        resumed.notes.to_lowercase().contains("merge conflict"),
        "resume must record a decision note naming the conflict: {}",
        resumed.notes
    );
}

/// cas-a844 negative control: unmerged-but-cleanly-mergeable commits must
/// NOT be flagged as conflicted, and the refusal must not claim a conflict
/// that doesn't exist.
#[tokio::test]
async fn test_a844_clean_divergence_not_flagged_as_conflict() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent.role = AgentRole::Worker;
        agent_store.update(&agent).expect("mark test agent worker");
    }

    let repo = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "epic/cas-a844-clean"]);
    git(&["checkout", "-q", "-b", "factory/test-agent"]);
    std::fs::write(repo.join("worker.txt"), "worker\n").unwrap();
    git(&["add", "worker.txt"]);
    git(&["commit", "-q", "-m", "worker change"]);

    let task_store = open_task_store(&cas_dir).expect("open task store");
    let epic_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: "Merge epic".to_string(),
                description: None,
                priority: 2,
                task_type: "epic".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create epic"),
    ))
    .expect("epic id")
    .to_string();
    {
        let mut epic = task_store.get(&epic_id).expect("epic exists");
        epic.branch = Some("epic/cas-a844-clean".to_string());
        task_store.update(&epic).expect("update epic branch");
    }

    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id.clone()),
                ..simple_task_req("Task A")
            }))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("start A");
    {
        let mut task_a = task_store.get(&id_a).expect("A exists after start");
        task_a.assignee = Some("test-agent".to_string());
        task_store.update(&task_a).expect("set A assignee");
    }

    let close_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("ready for merge".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close A returns"),
    );
    assert!(close_text.contains("MERGE REQUIRED"));
    assert!(
        !close_text.to_lowercase().contains("conflict"),
        "a cleanly-mergeable divergence must not be described as a conflict: {close_text}"
    );

    let parked = task_store.get(&id_a).expect("A exists");
    assert_eq!(parked.status, TaskStatus::AwaitingMerge);
    assert!(
        !parked.deliverables.merge_conflicted,
        "clean divergence must not be flagged as a merge conflict"
    );
}

/// cas-627f: a worker retrying `close` on an already-parked (AwaitingMerge)
/// task — the documented #1 worker failure mode while waiting on a
/// supervisor merge — must get the same rejection message WITHOUT
/// `park_task_awaiting_merge` re-running: no duplicate audit note appended
/// to `task.notes`, no duplicate `WorkerVerificationBlocked` close-rejection
/// activity event recorded.
#[tokio::test]
async fn test_repeated_merge_required_close_does_not_duplicate_park_audit_cas_627f() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent.role = AgentRole::Worker;
        agent_store.update(&agent).expect("mark test agent worker");
    }

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("write config");

    let repo = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "epic/cas-627f"]);
    git(&["checkout", "-q", "-b", "factory/test-agent"]);
    std::fs::write(repo.join("worker.txt"), "worker\n").unwrap();
    git(&["add", "worker.txt"]);
    git(&["commit", "-q", "-m", "worker change"]);

    let task_store = open_task_store(&cas_dir).expect("open task store");

    let epic_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: "Merge epic".to_string(),
                description: None,
                priority: 2,
                task_type: "epic".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create epic"),
    ))
    .expect("epic id")
    .to_string();
    {
        let mut epic = task_store.get(&epic_id).expect("epic exists");
        epic.branch = Some("epic/cas-627f".to_string());
        task_store.update(&epic).expect("update epic branch");
    }

    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id.clone()),
                ..simple_task_req("Task A")
            }))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("start A");
    {
        let mut task_a = task_store.get(&id_a).expect("A exists after start");
        task_a.assignee = Some("test-agent".to_string());
        task_store.update(&task_a).expect("set A assignee");
    }

    let close_req = || TaskCloseRequest {
        stranded_branch_override: None,
        id: id_a.clone(),
        reason: Some("ready for merge".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };

    let first_close = extract_text(
        service
            .cas_task_close(Parameters(close_req()))
            .await
            .expect("first close returns"),
    );
    assert!(
        first_close.contains("MERGE REQUIRED"),
        "first close must reject on stranded factory branch: {first_close}"
    );

    let parked_once = task_store.get(&id_a).expect("A exists");
    assert_eq!(parked_once.status, TaskStatus::AwaitingMerge);
    let notes_after_first = parked_once.notes.clone();

    let event_store = open_event_store(&cas_dir).expect("open event store");
    let close_rejected_count = |store: &dyn EventStore| {
        store
            .list_recent(50)
            .expect("list recent events")
            .into_iter()
            .filter(|e| {
                e.event_type == EventType::WorkerVerificationBlocked
                    && e.metadata
                        .as_ref()
                        .and_then(|m| m.get("close_rejected"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    && e.metadata
                        .as_ref()
                        .and_then(|m| m.get("task_id"))
                        .and_then(|v| v.as_str())
                        == Some(id_a.as_str())
            })
            .count()
    };
    let rejection_events_after_first = close_rejected_count(event_store.as_ref());
    assert_eq!(
        rejection_events_after_first, 1,
        "first rejection should record exactly one close_rejected activity event"
    );

    // Retry close on the already-parked task — same stranded branch, no
    // supervisor merge has happened yet.
    let second_close = extract_text(
        service
            .cas_task_close(Parameters(close_req()))
            .await
            .expect("second close returns"),
    );
    assert!(
        second_close.contains("MERGE REQUIRED"),
        "retry must repeat the same rejection message: {second_close}"
    );

    let parked_twice = task_store.get(&id_a).expect("A exists");
    assert_eq!(parked_twice.status, TaskStatus::AwaitingMerge);
    assert_eq!(
        parked_twice.notes, notes_after_first,
        "repeated close on an already-parked task must not append a duplicate audit note"
    );

    let rejection_events_after_second = close_rejected_count(event_store.as_ref());
    assert_eq!(
        rejection_events_after_second, 1,
        "repeated close on an already-parked task must not emit a duplicate \
         close-rejection activity event"
    );
}

/// cas-3d37: reproduces the live ordering exactly, end to end through
/// PostToolUse and `cas_task_close`. The worker commits and pushes first,
/// the supervisor merges without a prior close/park, and the worker's first
/// close succeeds using the commit-time task anchor.
#[tokio::test]
async fn test_merge_before_first_close_uses_commit_hook_anchor_cas_3d37() {
    use cas::hooks::{HookInput, handle_post_tool_use};
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let worker_id = {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent.role = AgentRole::Worker;
        let id = agent.id.clone();
        agent_store.update(&agent).expect("mark test agent worker");
        id
    };

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("write config");

    let repo = temp.path();
    let remote = TempDir::new().expect("remote tempdir");
    let bare_ok = Command::new("git")
        .args(["init", "--bare", "-q"])
        .current_dir(remote.path())
        .status()
        .expect("init bare remote")
        .success();
    assert!(bare_ok, "bare remote init failed");
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&[
        "remote",
        "add",
        "origin",
        &remote.path().display().to_string(),
    ]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "epic/cas-3d37"]);
    git(&["push", "-q", "-u", "origin", "epic/cas-3d37"]);
    git(&["checkout", "-q", "-b", "factory/test-agent"]);

    let task_store = open_task_store(&cas_dir).expect("open task store");
    let epic_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: "Merge-before-close epic".to_string(),
                description: None,
                priority: 2,
                task_type: "epic".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create epic"),
    ))
    .expect("epic id")
    .to_string();
    {
        let mut epic = task_store.get(&epic_id).expect("epic exists");
        epic.branch = Some("epic/cas-3d37".to_string());
        task_store.update(&epic).expect("update epic branch");
    }

    let task_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id),
                ..simple_task_req("Merge before first close")
            }))
            .await
            .expect("create task"),
    ))
    .expect("task id")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest {
            id: task_id.clone(),
        }))
        .await
        .expect("start task");
    {
        let mut task = task_store.get(&task_id).expect("task exists after start");
        task.assignee = Some("test-agent".to_string());
        task_store.update(&task).expect("set task assignee");
    }

    // Worker commit + push, before any close attempt.
    std::fs::write(repo.join("work.rs"), "fn merged_work() {}\n").unwrap();
    git(&["add", "work.rs"]);
    git(&["commit", "-q", "-m", "fix: merged task work"]);
    handle_post_tool_use(
        &HookInput {
            session_id: worker_id,
            cwd: repo.display().to_string(),
            hook_event_name: "PostToolUse".to_string(),
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({
                "command": "git commit -q -m 'fix: merged task work'"
            })),
            // Real Codex 0.145.0 PostToolUse shape: unified exec matches as
            // Bash, while `tool_response` is the model-facing output string
            // rather than Claude's `{exitCode, stdout}` object.
            tool_response: Some(serde_json::json!("")),
            agent_role: Some("worker".to_string()),
            ..Default::default()
        },
        Some(&cas_dir),
    )
    .expect("post-tool hook");
    let anchored = task_store.get(&task_id).expect("anchored task");
    let anchor = anchored
        .deliverables
        .factory_branch_anchor
        .clone()
        .expect("commit hook recorded task anchor");
    git(&["push", "-q", "-u", "origin", "factory/test-agent"]);

    // Supervisor merges and pushes without any prior task close/park.
    git(&["checkout", "-q", "epic/cas-3d37"]);
    git(&["merge", "--no-ff", "-q", "origin/factory/test-agent"]);
    git(&["push", "-q", "origin", "epic/cas-3d37"]);
    git(&["checkout", "-q", "factory/test-agent"]);
    let merged = Command::new("git")
        .args(["merge-base", "--is-ancestor", &anchor, "epic/cas-3d37"])
        .current_dir(repo)
        .status()
        .expect("ancestry check")
        .success();
    assert!(merged, "task anchor must be merged into the epic");

    open_verification_store(&cas_dir)
        .expect("open verification store")
        .add(&Verification::approved(
            "ver-cas-3d37".to_string(),
            task_id.clone(),
            "Approved literal merge-before-close regression".to_string(),
        ))
        .expect("record verification approval");

    let close_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: task_id.clone(),
                reason: Some("work was merged before first close".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("first close returns"),
    );
    assert!(
        close_text.contains("Closed task:"),
        "worker's first close after supervisor merge must succeed: {close_text}"
    );
    assert_eq!(
        task_store.get(&task_id).expect("task exists").status,
        TaskStatus::Closed
    );
}

/// cas-4b3f (AC b): reproduces BUG-close-guard-branch-head-not-task-commits.md
/// end to end through `cas_task_close`. Worker completes task A on
/// `factory/test-agent`, gets MERGE REQUIRED (parked — this is where the
/// commit-tip anchor is snapshotted), the supervisor merges task A's commit
/// into the epic branch, and then the SAME worker starts task B serially on
/// the SAME `factory/test-agent` branch (the natural one-worker-many-tasks
/// workflow) before retrying task A's close. Pre-fix, the retry recomputed
/// against branch HEAD (now carrying task B's unmerged commit) and
/// false-rejected task A even though its own commits were already merged.
/// Post-fix, the anchor recorded at the first rejection lets task A's close
/// succeed without waiting on task B.
#[tokio::test]
async fn test_serial_second_task_on_same_branch_does_not_restrand_first_close_cas_4b3f() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent.role = AgentRole::Worker;
        agent_store.update(&agent).expect("mark test agent worker");
    }

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("write config");

    let repo = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "epic/cas-4b3f-serial"]);
    git(&["checkout", "-q", "-b", "factory/test-agent"]);
    std::fs::write(repo.join("task_a.txt"), "task A work\n").unwrap();
    git(&["add", "task_a.txt"]);
    git(&["commit", "-q", "-m", "feat: task A"]);

    let task_store = open_task_store(&cas_dir).expect("open task store");

    let epic_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: "Serial-task merge epic".to_string(),
                description: None,
                priority: 2,
                task_type: "epic".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create epic"),
    ))
    .expect("epic id")
    .to_string();
    {
        let mut epic = task_store.get(&epic_id).expect("epic exists");
        epic.branch = Some("epic/cas-4b3f-serial".to_string());
        task_store.update(&epic).expect("update epic branch");
    }

    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id.clone()),
                ..simple_task_req("Task A")
            }))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("start A");
    {
        let mut task_a = task_store.get(&id_a).expect("A exists after start");
        task_a.assignee = Some("test-agent".to_string());
        task_store.update(&task_a).expect("set A assignee");
    }

    // First close attempt: MERGE REQUIRED — parks A and snapshots the
    // factory branch's current tip (task A's commit) as the anchor.
    let first_close = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("ready for merge".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("first close returns"),
    );
    assert!(
        first_close.contains("MERGE REQUIRED"),
        "first close must reject on stranded factory branch: {first_close}"
    );
    let parked = task_store.get(&id_a).expect("A exists");
    assert_eq!(parked.status, TaskStatus::AwaitingMerge);
    assert!(
        parked.deliverables.factory_branch_anchor.is_some(),
        "first rejection must snapshot the factory branch anchor onto the task"
    );

    // Supervisor merges task A's commit into the epic branch.
    git(&["checkout", "-q", "epic/cas-4b3f-serial"]);
    git(&["merge", "--no-ff", "-q", "factory/test-agent"]);
    git(&["checkout", "-q", "factory/test-agent"]);

    // The SAME worker starts task B serially on the SAME branch, before
    // task A's close is retried.
    let id_b = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id.clone()),
                ..simple_task_req("Task B")
            }))
            .await
            .expect("create B"),
    ))
    .expect("id B")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_b.clone() }))
        .await
        .expect("start B");
    {
        let mut task_b = task_store.get(&id_b).expect("B exists after start");
        task_b.assignee = Some("test-agent".to_string());
        task_store.update(&task_b).expect("set B assignee");
    }
    std::fs::write(repo.join("task_b.txt"), "task B work (unmerged)\n").unwrap();
    git(&["add", "task_b.txt"]);
    git(&["commit", "-q", "-m", "feat: task B (not yet merged)"]);

    // Approve task A's verification (mirrors the sibling cas-8d5b test —
    // isolates this test from the verification jail so it proves the
    // merge-anchor fix specifically).
    let verification_store = open_verification_store(&cas_dir).expect("open verification store");
    verification_store
        .add(&Verification::approved(
            "ver-cas-4b3f-serial".to_string(),
            id_a.clone(),
            "Simulated approval after supervisor merge".to_string(),
        ))
        .expect("record verification approval");

    // Retry task A's close: must now succeed — anchored to task A's own
    // (already-merged) commit, not branch HEAD (which carries task B's
    // still-unmerged commit).
    let second_close = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("merged and ready to close".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("second close returns"),
    );
    assert!(
        second_close.contains("Closed task:"),
        "task A's close must succeed once ITS OWN commits are merged, \
         regardless of task B's later unmerged work on the same branch — \
         pre-fix this false-rejected with MERGE REQUIRED again: {second_close}"
    );
    assert_eq!(
        task_store.get(&id_a).expect("A exists").status,
        TaskStatus::Closed
    );
}

/// cas-38e2: reproduces the live incident found while merging cas-4b3f/
/// cas-ac2e/cas-c093/cas-f781/cas-b082 in this same factory session — a
/// worker's commit is merged into the epic branch and PUSHED to origin
/// (so `origin/<epic>` genuinely contains it), but the closing worker's
/// OWN local `<epic>` ref is still at the pre-merge tip. Every other
/// worker this session hit MERGE REQUIRED on already-integrated work and
/// had to be closed from the supervisor's own (fresh) checkout as a
/// workaround. Post-fix, the gate falls back to `origin/<epic>` before
/// rejecting, so the worker's own close succeeds directly.
#[tokio::test]
async fn test_stale_local_epic_ref_falls_back_to_origin_cas_38e2() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent.role = AgentRole::Worker;
        agent_store.update(&agent).expect("mark test agent worker");
    }

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("write config");

    let bare = tempfile::tempdir().expect("bare tempdir");
    let bare_status = Command::new("git")
        .args(["init", "-q", "--bare"])
        .current_dir(bare.path())
        .status()
        .expect("git init --bare");
    assert!(bare_status.success());

    let repo = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    let git_output = |args: &[&str]| -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["remote", "add", "origin", bare.path().to_str().unwrap()]);
    git(&["checkout", "-q", "-b", "epic/cas-38e2"]);
    let old_epic_tip = git_output(&["rev-parse", "epic/cas-38e2"]);
    git(&["checkout", "-q", "-b", "factory/test-agent"]);
    std::fs::write(repo.join("worker.txt"), "worker\n").unwrap();
    git(&["add", "worker.txt"]);
    git(&["commit", "-q", "-m", "worker change"]);

    let task_store = open_task_store(&cas_dir).expect("open task store");

    let epic_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: "Stale-ref merge epic".to_string(),
                description: None,
                priority: 2,
                task_type: "epic".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create epic"),
    ))
    .expect("epic id")
    .to_string();
    {
        let mut epic = task_store.get(&epic_id).expect("epic exists");
        epic.branch = Some("epic/cas-38e2".to_string());
        task_store.update(&epic).expect("update epic branch");
    }

    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id.clone()),
                ..simple_task_req("Task A")
            }))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("start A");
    {
        let mut task_a = task_store.get(&id_a).expect("A exists after start");
        task_a.assignee = Some("test-agent".to_string());
        task_store.update(&task_a).expect("set A assignee");
    }

    // Supervisor (simulated in the same checkout): merge + push the epic
    // branch to origin. This is what makes `origin/epic/cas-38e2` genuinely
    // contain the worker's commit.
    git(&["checkout", "-q", "epic/cas-38e2"]);
    git(&["merge", "-q", "--no-ff", "factory/test-agent"]);
    git(&["push", "-q", "origin", "epic/cas-38e2"]);

    // Now force the local epic branch ref back to its pre-merge tip —
    // simulating the closing worker's own view not having observed the
    // merge yet, even though origin (and everyone else) has.
    git(&["checkout", "-q", "factory/test-agent"]);
    git(&["branch", "-f", "epic/cas-38e2", &old_epic_tip]);

    let verification_store = open_verification_store(&cas_dir).expect("open verification store");
    verification_store
        .add(&Verification::approved(
            "ver-cas-38e2".to_string(),
            id_a.clone(),
            "Simulated approval".to_string(),
        ))
        .expect("record verification approval");

    let close_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("merged and pushed to origin".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close returns"),
    );
    assert!(
        close_text.contains("Closed task:"),
        "a commit already reachable from origin/epic/cas-38e2 must not bounce \
         off this repo's stale local epic branch ref: {close_text}"
    );
    assert_eq!(
        task_store.get(&id_a).expect("A exists").status,
        TaskStatus::Closed
    );
}

/// cas-cf64 (P2, anchor freshness — Scenario B): park → merge → close →
/// REOPEN → rework → close again must NOT silently Proceed using the
/// stale anchor from the FIRST close cycle. Before this fix,
/// `park_task_awaiting_merge`'s `is_none()` guard meant the anchor (set to
/// the tip at the first rejection) was NEVER cleared or updated once a
/// task closed and was later reopened — so a second round of genuinely
/// new, unmerged work would still check against the OLD (already-merged)
/// anchor and false-Proceed. `cas_task_reopen` clears the anchor before
/// rework starts, so the reworked task's retry correctly re-evaluates from
/// scratch; cas-eaf8 intentionally preserves it while the task stays Closed
/// so the parent epic can use it as a task-specific merge receipt.
#[tokio::test]
async fn test_reopened_task_does_not_reuse_stale_anchor_cas_cf64() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent.role = AgentRole::Worker;
        agent_store.update(&agent).expect("mark test agent worker");
    }

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("write config");

    let repo = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "epic/cas-cf64-scenario-b"]);
    git(&["checkout", "-q", "-b", "factory/test-agent"]);
    std::fs::write(repo.join("v1.txt"), "first pass\n").unwrap();
    git(&["add", "v1.txt"]);
    git(&["commit", "-q", "-m", "feat: first pass"]);

    let task_store = open_task_store(&cas_dir).expect("open task store");

    let epic_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: "Anchor-freshness epic".to_string(),
                description: None,
                priority: 2,
                task_type: "epic".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create epic"),
    ))
    .expect("epic id")
    .to_string();
    {
        let mut epic = task_store.get(&epic_id).expect("epic exists");
        epic.branch = Some("epic/cas-cf64-scenario-b".to_string());
        task_store.update(&epic).expect("update epic branch");
    }

    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id.clone()),
                ..simple_task_req("Task A")
            }))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("start A");
    {
        let mut task_a = task_store.get(&id_a).expect("A exists after start");
        task_a.assignee = Some("test-agent".to_string());
        task_store.update(&task_a).expect("set A assignee");
    }

    // First close attempt: MERGE REQUIRED — parks and snapshots the anchor.
    let first_close = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("ready for merge".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("first close returns"),
    );
    assert!(
        first_close.contains("MERGE REQUIRED"),
        "first close must reject on stranded factory branch: {first_close}"
    );
    let parked = task_store.get(&id_a).expect("A exists");
    assert!(
        parked.deliverables.factory_branch_anchor.is_some(),
        "first rejection must snapshot the anchor"
    );

    // Supervisor merges the first-pass commit into the epic branch.
    git(&["checkout", "-q", "epic/cas-cf64-scenario-b"]);
    git(&["merge", "-q", "--no-ff", "factory/test-agent"]);
    git(&["checkout", "-q", "factory/test-agent"]);

    let verification_store = open_verification_store(&cas_dir).expect("open verification store");
    verification_store
        .add(&Verification::approved(
            "ver-cas-cf64-first".to_string(),
            id_a.clone(),
            "Simulated approval after first merge".to_string(),
        ))
        .expect("record verification approval");

    let second_close = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("merged, closing".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("second close returns"),
    );
    assert!(
        second_close.contains("Closed task:"),
        "close must succeed once the anchored commit is merged: {second_close}"
    );
    assert_eq!(
        task_store.get(&id_a).expect("A exists").status,
        TaskStatus::Closed
    );

    // Reopen the task — this must clear the stale anchor.
    // cas-3c23: reopen is now supervisor-gated, so this "supervisor decides
    // rework is needed" scenario must run under CAS_AGENT_ROLE=supervisor.
    let reopen_text = {
        let _sup = ScopedSupervisorEnv::new();
        extract_text(
            service
                .cas_task_reopen(Parameters(TaskReopenRequest {
                    id: id_a.clone(),
                    reason: Some("new commit requires a fresh close cycle".to_string()),
                }))
                .await
                .expect("reopen returns"),
        )
    };
    assert!(
        reopen_text.contains("Reopened task:"),
        "reopen should succeed: {reopen_text}"
    );
    let reopened = task_store.get(&id_a).expect("A exists");
    assert_eq!(reopened.status, TaskStatus::Open);
    assert!(
        reopened.deliverables.factory_branch_anchor.is_none(),
        "reopen must clear the stale factory_branch_anchor"
    );

    // Worker reworks the SAME task on the SAME branch — a genuinely new,
    // unmerged commit.
    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("restart A after reopen");
    {
        let mut task_a = task_store.get(&id_a).expect("A exists after restart");
        task_a.assignee = Some("test-agent".to_string());
        task_store.update(&task_a).expect("set A assignee again");
    }
    std::fs::write(repo.join("v2.txt"), "reworked, NOT yet merged\n").unwrap();
    git(&["add", "v2.txt"]);
    git(&["commit", "-q", "-m", "feat: rework after reopen"]);

    let third_close = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("reworked, claiming done".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("third close returns"),
    );
    assert!(
        third_close.contains("MERGE REQUIRED"),
        "the reworked commit must be caught as unmerged — a stale anchor \
         from the FIRST close cycle must not let this silently Proceed: {third_close}"
    );
    assert_ne!(
        task_store.get(&id_a).expect("A exists").status,
        TaskStatus::Closed,
        "rejected close must not transition task to Closed"
    );
}

/// cas-4b3f (AC c): reproduces BUG-close-guard-nonepic-task-targets-main.md.
/// A standalone (non-epic) task whose worker has fully committed AND
/// MERGED their work onto the repo's real integration branch (resolved via
/// `resolve_standalone_merge_target` — here git's detected default branch,
/// `main`, since no `[factory] epic_base_branch` override is configured)
/// must close cleanly. cas-cf64 replaced cas-4b3f's "skip the gate when no
/// epic parent" behavior with "resolve the REAL target and actually check
/// it" — this proves the positive (already-integrated) side of that still
/// works, not just the negative (still-unmerged) side covered by the
/// sibling test below.
#[tokio::test]
async fn test_nonepic_task_resolves_default_branch_and_proceeds_when_merged_cas_cf64() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = false\n",
    )
    .expect("write config");

    let repo = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "factory/standalone-worker"]);
    std::fs::write(repo.join("work.rs"), "// standalone work\n").unwrap();
    git(&["add", "work.rs"]);
    git(&["commit", "-q", "-m", "feat: standalone task work"]);
    // Actually merge into the repo's real default branch — this is what
    // "already integrated" genuinely means when there's no epic parent.
    git(&["checkout", "-q", "main"]);
    git(&["merge", "-q", "--no-ff", "factory/standalone-worker"]);
    git(&["checkout", "-q", "factory/standalone-worker"]);

    let create_req = TaskCreateRequest {
        depth: None,
        title: "cas-cf64: no-epic close resolves default branch, proceeds when merged".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None, // <-- standalone, no epic parent recorded
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();

    let task_store = open_task_store(&cas_dir).expect("open task store");
    let mut task = task_store.get(&id).expect("task exists");
    task.status = TaskStatus::InProgress;
    task.assignee = Some("standalone-worker".to_string());
    task_store.update(&task).expect("update task");

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("done, merged onto main".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        !resp.contains("MERGE REQUIRED"),
        "work already merged onto the resolved default branch must not \
         false-reject: {resp}"
    );
    assert!(
        resp.contains("Closed task:"),
        "close should succeed once the resolved target genuinely contains the work: {resp}"
    );
}

/// cas-cf64 (P2, standalone-task backstop gap): the negative side of the
/// test above — a standalone (non-epic) task whose worker committed real
/// code to `factory/<assignee>` but NEVER merged it anywhere must now be
/// REJECTED at close. Before this fix, cas-4b3f's "skip the gate when no
/// epic parent resolves" left exactly this hole: the code above proves the
/// gate now runs against the REAL resolved target (git's detected default
/// branch, `main`, absent a configured override) instead of skipping.
#[tokio::test]
async fn test_nonepic_task_with_unmerged_code_is_rejected_not_skipped_cas_cf64() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = false\n",
    )
    .expect("write config");

    let repo = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "factory/standalone-worker"]);
    std::fs::write(repo.join("work.rs"), "// standalone work, never merged\n").unwrap();
    git(&["add", "work.rs"]);
    git(&["commit", "-q", "-m", "feat: standalone task work"]);
    // Deliberately NOT merged into main or anywhere else.

    let create_req = TaskCreateRequest {
        depth: None,
        title: "cas-cf64: no-epic close with unmerged code must reject".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None, // <-- standalone, no epic parent recorded
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();

    let task_store = open_task_store(&cas_dir).expect("open task store");
    let mut task = task_store.get(&id).expect("task exists");
    task.status = TaskStatus::InProgress;
    task.assignee = Some("standalone-worker".to_string());
    task_store.update(&task).expect("update task");

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("claiming done but never merged".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("MERGE REQUIRED"),
        "a standalone task with real committed-unmerged code on \
         factory/<assignee> must be rejected, not silently skipped: {resp}"
    );
    assert!(
        resp.contains("main"),
        "rejection must name the resolved real target (git's detected \
         default branch), not skip or say nothing: {resp}"
    );
    assert_ne!(
        task_store.get(&id).expect("task exists").status,
        TaskStatus::Closed,
        "rejected close must not transition task to Closed"
    );
}

/// cas-cf64 (Chore/Spike no longer exempt): a Chore-type standalone task
/// that commits real code to `factory/<assignee>` and never merges it must
/// ALSO be rejected — cas-4b3f's type-based exemption (Chore/Spike skip
/// this gate outright) was the other half of the backstop gap.
#[tokio::test]
async fn test_chore_type_task_with_unmerged_code_is_no_longer_exempt_cas_cf64() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = false\n",
    )
    .expect("write config");

    let repo = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "factory/chore-worker"]);
    std::fs::write(repo.join("cleanup.rs"), "// chore cleanup, never merged\n").unwrap();
    git(&["add", "cleanup.rs"]);
    git(&["commit", "-q", "-m", "chore: cleanup"]);

    let create_req = TaskCreateRequest {
        depth: None,
        title: "cas-cf64: chore-type task with unmerged code must reject".to_string(),
        description: None,
        priority: 2,
        task_type: "chore".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();

    let task_store = open_task_store(&cas_dir).expect("open task store");
    let mut task = task_store.get(&id).expect("task exists");
    task.status = TaskStatus::InProgress;
    task.assignee = Some("chore-worker".to_string());
    task_store.update(&task).expect("update task");

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("claiming done but never merged".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("MERGE REQUIRED"),
        "a Chore-type task with real committed-unmerged code must no longer \
         be exempt from the merge-state gate: {resp}"
    );
    assert_ne!(
        task_store.get(&id).expect("task exists").status,
        TaskStatus::Closed,
        "rejected close must not transition task to Closed"
    );
}

/// cas-cf64 negative control (preserve the original cas-4b3f intent): a
/// Chore-type task with genuinely NO code (docs/notes-only, no factory
/// branch at all) must still close on notes alone — dropping the type
/// exemption must not turn every docs-only chore into a false reject.
#[tokio::test]
async fn test_chore_type_task_with_zero_commits_still_closes_on_notes_cas_cf64() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = false\n",
    )
    .expect("write config");

    let create_req = TaskCreateRequest {
        depth: None,
        title: "cas-cf64: docs-only chore, no factory branch at all".to_string(),
        description: None,
        priority: 2,
        task_type: "chore".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();

    // No factory/<assignee> branch is ever created for this assignee — the
    // gate must gracefully treat "branch doesn't exist" as merged, same as
    // every other gate in this file.
    let task_store = open_task_store(&cas_dir).expect("open task store");
    let mut task = task_store.get(&id).expect("task exists");
    task.status = TaskStatus::InProgress;
    task.assignee = Some("someone-with-no-branch".to_string());
    task_store.update(&task).expect("update task");

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("resolved via notes, no code needed".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("Closed task:"),
        "a genuinely no-code chore must still close on notes alone: {resp}"
    );
}

/// cas-895d: a worker completes their work, writes tests, runs build, and
/// calls `task.close` — all while leaving the actual edits uncommitted in
/// their worktree. The pre-fix close path accepted this because
/// verification and the additive-only gate never looked at working-tree
/// state; the work got GC'd with the worktree.
///
/// Post-fix, the close path runs `git status --porcelain` against the
/// worker's worktree and rejects closes with any tracked modifications,
/// staged-but-uncommitted additions, deletes, or renames. Only committed
/// work — or genuinely scratch untracked files — may pass.
///
/// This test wires up a real git repo as the "worker worktree", attaches
/// it to a task via `task.worktree_id`, and exercises the close path
/// directly. verification_enabled=false so the test isolates the new
/// gate from the task-verifier flow.
#[tokio::test]
async fn test_task_close_blocks_on_uncommitted_worker_worktree() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Disable verification so we isolate the cas-895d uncommitted-work
    // gate from the task-verifier jail.
    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[verification]
enabled = false
"#,
    )
    .expect("write config");

    // Create a real git repo in a tempdir to play the role of a worker
    // worktree. One committed file, so HEAD exists and `git status`
    // behaves normally.
    let worktree_path = temp.path().join("worker-worktree");
    std::fs::create_dir_all(&worktree_path).expect("mkdir worktree");
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(&worktree_path)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(worktree_path.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    // cas-d987: fork a worker branch off main so that commits in Scenario C
    // land beyond `parent_branch` ("main"). Before this fix the test committed
    // directly on main, so `count_worker_branch_commits(path, "main")` returned
    // 0 (HEAD == main, rev-list HEAD..HEAD = 0) and the cas-ee2b zero-commit
    // gate rejected the close in Scenario C. Compare with
    // `test_additive_only_uses_worker_branch_not_main_worktree` which correctly
    // checks out a worker branch before making commits.
    git(&["checkout", "-q", "-b", "factory/895d-worker"]);

    // Register the worktree in cas and attach it to a task.
    let worktree_store = open_worktree_store(&cas_dir).expect("open worktree store");
    worktree_store.init().expect("init worktree store");
    let worktree_id = Worktree::generate_id();
    let worktree = Worktree::new(
        worktree_id.clone(),
        "factory/895d-worker".to_string(),
        "main".to_string(),
        worktree_path.clone(),
    );
    worktree_store.add(&worktree).expect("add worktree");

    let create_req = TaskCreateRequest {
        depth: None,
        title: "cas-895d regression: committed-state close gate".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();

    let task_store = open_task_store(&cas_dir).expect("open task store");
    let mut task = task_store.get(&id).expect("task exists");
    task.status = cas::types::TaskStatus::InProgress;
    task.worktree_id = Some(worktree_id.clone());
    task_store.update(&task).expect("update task");

    // Scenario A: worker modified an existing tracked file but never
    // committed. Closing must fail with UNCOMMITTED WORK.
    std::fs::write(worktree_path.join("seed.txt"), "worker edit\n").unwrap();
    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("claims to be done".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("UNCOMMITTED WORK"),
        "uncommitted tracked edit must reject close: {resp}"
    );
    assert!(
        resp.contains("seed.txt"),
        "error must name the dirty file: {resp}"
    );
    assert_ne!(
        task_store.get(&id).expect("task exists").status,
        cas::types::TaskStatus::Closed,
        "rejected close must not transition task to Closed"
    );

    // Scenario B: worker staged a new file but never committed. Same
    // lost-work scenario — must still block (status `A `).
    std::fs::write(worktree_path.join("seed.txt"), "seed\n").unwrap(); // revert
    std::fs::write(worktree_path.join("new.rs"), "fn main() {}\n").unwrap();
    git(&["add", "new.rs"]);
    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("claims to be done".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("UNCOMMITTED WORK"),
        "staged-but-uncommitted must reject close: {resp}"
    );
    assert!(
        resp.contains("new.rs"),
        "error must name the new file: {resp}"
    );

    // Scenario C: worker actually commits their work. Close must now
    // succeed (verification is disabled in this test's config).
    git(&["commit", "-q", "-m", "feat: add new.rs"]);
    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("Committed and ready".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("Closed task:"),
        "committed work must pass the gate: {resp}"
    );
    assert_eq!(
        task_store.get(&id).expect("task exists").status,
        cas::types::TaskStatus::Closed,
        "committed close must transition task to Closed"
    );
}

/// cas-4b3f (AC a, root cause): `resolve_worker_worktree_path` previously
/// consulted ONLY "System A" (`task.worktree_id`, a `WorktreeStore` row) —
/// which is populated exclusively for epic-type tasks
/// (`cas_task_start`/`lifecycle.rs`: "Worktrees are scoped to epics, not
/// individual tasks") behind a config flag that's disabled by default. A
/// real single-task factory worker isolated via `spawn_workers
/// isolate=true` ("System B") lives at the fixed convention
/// `<cas_root>/worktrees/<assignee>` and is NEVER registered in the
/// WorktreeStore, so `task.worktree_id` is always `None` for it — meaning
/// the cas-895d uncommitted-work gate (and cas-490f/cas-762e/cas-ee2b)
/// silently no-opped for the overwhelmingly common production case. This
/// is exactly the data-loss near-miss from
/// BUG-merge-gate-inconsistent-close-without-integration.md (the
/// `sturdy-finch-54` incident: two tasks closed as done+verified while the
/// code was entirely uncommitted in the worker's real worktree).
///
/// This test wires up a System-B-shaped worktree — `task.assignee` set,
/// real git repo at `<cas_root>/worktrees/<assignee>` — with deliberately
/// NO `WorktreeStore` row and NO `task.worktree_id`, and proves the
/// uncommitted-work gate now fires anyway.
#[tokio::test]
async fn test_task_close_blocks_on_uncommitted_system_b_worker_worktree_cas_4b3f() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Disable verification so we isolate the cas-895d uncommitted-work
    // gate from the task-verifier flow.
    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = false\n",
    )
    .expect("write config");

    // System B convention: `<cas_root>/worktrees/<assignee>` — deliberately
    // NOT registered in the WorktreeStore at all.
    let worktree_path = cas_dir.join("worktrees").join("sturdy-finch-54");
    std::fs::create_dir_all(&worktree_path).expect("mkdir worktree");
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(&worktree_path)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(worktree_path.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "factory/sturdy-finch-54"]);

    let create_req = TaskCreateRequest {
        depth: None,
        title: "cas-4b3f regression: System B uncommitted close gate".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();

    let task_store = open_task_store(&cas_dir).expect("open task store");
    let mut task = task_store.get(&id).expect("task exists");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = Some("sturdy-finch-54".to_string());
    // Deliberately NOT setting task.worktree_id: System B workers never get
    // that field populated — that's the entire bug this test pins.
    task_store.update(&task).expect("update task");

    // Worker "completes" the task but never commits — working tree stays
    // dirty (mirrors the doc's "3 modified files + 2 untracked test files").
    std::fs::write(
        worktree_path.join("seed.txt"),
        "worker edit, never committed\n",
    )
    .unwrap();

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("done".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("UNCOMMITTED WORK"),
        "a System-B worker's uncommitted edit must reject close — pre-fix \
         this silently passed because resolve_worker_worktree_path only \
         checked System A: {resp}"
    );
    assert!(
        resp.contains("seed.txt"),
        "error must name the dirty file: {resp}"
    );
    assert_ne!(
        task_store.get(&id).expect("task exists").status,
        cas::types::TaskStatus::Closed,
        "rejected close must not transition task to Closed"
    );

    // Confirm the gate isn't just blanket-rejecting: commit the work and
    // retry — close must now succeed.
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "fix: commit the work"]);
    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("actually done now".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("Closed task:"),
        "committed System-B work must pass the gate: {resp}"
    );
    assert_eq!(
        task_store.get(&id).expect("task exists").status,
        cas::types::TaskStatus::Closed
    );
}

/// cas-bc1b regression: `execution_note=additive-only` close must inspect
/// the **worker branch's committed history**, not the main worktree's
/// unstaged state. Before the fix the additive-only check ran
/// `git diff --name-status HEAD` in `cas_root.parent()` (the main
/// worktree), so a pristine worker branch with a purely-additive commit
/// would be rejected because of an unrelated dirty file in main.
///
/// This test wires up:
///   * A real git repo with `main` committed and a `factory/worker`
///     branch forked off — standing in for the worker worktree.
///   * A cas worktree row pointing at that path with parent_branch="main".
///   * A task with execution_note=additive-only and that worktree_id.
///
/// The worker commits one purely-additive file on their branch, then
/// dirties an **unrelated** tracked file and leaves it uncommitted
/// (simulating the cas-4333 Cargo.lock drift). Close must succeed: the
/// branch diff is additive, and the uncommitted drift is ignored
/// because the check inspects committed history, not unstaged state.
#[tokio::test]
async fn test_additive_only_uses_worker_branch_not_main_worktree() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Disable verification — we're testing the additive-only gate.
    // Also implicitly disables cas-895d uncommitted-work gate from
    // firing on the drift file (we want *this* test to prove the
    // additive-only fix works independently of the cas-895d gate).
    //
    // Actually: cas-895d's gate fires BEFORE additive-only and rejects
    // any dirty worker worktree. Since the simulated drift is in the
    // same worktree, cas-895d would catch it first. To isolate the
    // cas-bc1b fix, we intentionally leave the worker worktree clean
    // and rely on the fact that pre-fix code would have looked at the
    // MAIN worktree (cas_root.parent()) where unrelated drift lives.
    // The CAS root's parent is a clean Git repository, separate from
    // the worker worktree below. That lets the factory close path pass
    // its repository-identity check while still proving the constrained
    // execution-note gate reads the worker branch rather than main.
    // We prove the fix by committing a modification on the branch and
    // asserting the gate now catches it (which it wouldn't have
    // under the legacy `git diff HEAD` in main path — that one is
    // empty in tempdir because tempdir isn't a git repo).
    //
    // The "post-fix catches branch modifications" angle is the
    // cleaner assertion: pre-fix, the check ran in a non-git tempdir
    // and returned empty for every scenario; post-fix, it runs in
    // the worker branch and sees the real commits.
    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[verification]
enabled = false
"#,
    )
    .expect("write config");

    proof_boundary_git(temp.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(temp.path().join(".gitignore"), ".cas/\nworker-worktree/\n").unwrap();
    std::fs::write(temp.path().join("main.txt"), "main checkout\n").unwrap();
    proof_boundary_git(temp.path(), &["add", ".gitignore", "main.txt"]);
    proof_boundary_git(temp.path(), &["commit", "-q", "-m", "main: initial"]);

    // Real git repo playing the role of a worker worktree.
    let worktree_path = temp.path().join("worker-worktree");
    std::fs::create_dir_all(&worktree_path).expect("mkdir worktree");
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(&worktree_path)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(worktree_path.join("existing.txt"), "original\n").unwrap();
    git(&["add", "existing.txt"]);
    git(&["commit", "-q", "-m", "main: initial"]);
    git(&["checkout", "-q", "-b", "factory/worker"]);

    // Register the worktree with parent_branch="main".
    let worktree_store = open_worktree_store(&cas_dir).expect("open worktree store");
    worktree_store.init().expect("init worktree store");
    let worktree_id = Worktree::generate_id();
    let worktree = Worktree::new(
        worktree_id.clone(),
        "factory/worker".to_string(),
        "main".to_string(),
        worktree_path.clone(),
    );
    worktree_store.add(&worktree).expect("add worktree");

    let task_store = open_task_store(&cas_dir).expect("open task store");

    let additive_req = |title: &str| TaskCreateRequest {
        depth: None,
        title: title.to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: Some("additive-only".to_string()),
        epic: None,
    };

    // --- Scenario A: worker branch has a purely-additive commit.
    //     Close must succeed.
    std::fs::write(worktree_path.join("new.rs"), "fn main() {}\n").unwrap();
    git(&["add", "new.rs"]);
    git(&["commit", "-q", "-m", "feat: add new.rs"]);
    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(additive_req("cas-bc1b: additive branch commit")))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();
    {
        let mut t = task_store.get(&id_a).expect("task");
        t.status = cas::types::TaskStatus::InProgress;
        t.worktree_id = Some(worktree_id.clone());
        task_store.update(&t).expect("update task");
    }
    let resp_a = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("committed and additive".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close returns"),
    );
    assert!(
        resp_a.contains("Closed task:"),
        "purely-additive branch commit must pass: {resp_a}"
    );
    assert_eq!(
        task_store.get(&id_a).expect("task").status,
        cas::types::TaskStatus::Closed
    );

    // --- Scenario B: worker branch also has a commit modifying an
    //     existing tracked file. Additive-only must now reject. Pre-fix
    //     this would have been missed entirely — the check ran in the
    //     main worktree (not a git repo in the test) and silently no-
    //     oped.
    std::fs::write(worktree_path.join("existing.txt"), "worker edit\n").unwrap();
    git(&["add", "existing.txt"]);
    git(&["commit", "-q", "-m", "fix: edit existing.txt"]);
    let id_b = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(additive_req(
                "cas-bc1b: modifying branch commit",
            )))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();
    {
        let mut t = task_store.get(&id_b).expect("task");
        t.status = cas::types::TaskStatus::InProgress;
        t.worktree_id = Some(worktree_id.clone());
        task_store.update(&t).expect("update task");
    }
    let resp_b = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_b.clone(),
                reason: Some("claims to be additive".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close returns"),
    );
    assert!(
        resp_b.contains("ADDITIVE-ONLY VIOLATION"),
        "committed modification on worker branch must trigger additive-only gate: {resp_b}"
    );
    assert!(
        resp_b.contains("existing.txt"),
        "error must name the modified file: {resp_b}"
    );
    assert_ne!(
        task_store.get(&id_b).expect("task").status,
        cas::types::TaskStatus::Closed,
        "violation must not transition task to Closed"
    );

    // --- Scenario C: value-only is the accurate posture for a copy/i18n
    //     change to an existing file. It follows the ordinary close path;
    //     no worker-supplied review envelope or queue transition is needed.
    git(&["checkout", "-q", "main"]);
    git(&["checkout", "-q", "-b", "factory/value-only"]);
    std::fs::write(worktree_path.join("existing.txt"), "localized value\n").unwrap();
    git(&["add", "existing.txt"]);
    git(&["commit", "-q", "-m", "fix: localize existing value"]);
    let id_c = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                execution_note: Some("value-only".to_string()),
                ..additive_req("cas-8ad8: value-only branch commit")
            }))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();
    {
        let mut t = task_store.get(&id_c).expect("task");
        t.status = cas::types::TaskStatus::InProgress;
        t.worktree_id = Some(worktree_id.clone());
        task_store.update(&t).expect("update task");
    }
    // Customer-visible value changes remain reviewable by the normal
    // verification/merge gates. Make the fixture a factory worker explicitly:
    // setup_cas clears ambient factory env so the test cannot accidentally
    // exercise a solo caller's close behavior.
    let _worker = FactoryWorkerEnv::enter();
    let resp_c = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_c.clone(),
                reason: Some("localized existing value".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close returns"),
    );
    assert!(resp_c.contains("Closed task:"), "value-only close should complete: {resp_c}");
    assert_eq!(
        task_store.get(&id_c).expect("task").status,
        cas::types::TaskStatus::Closed,
        "value-only must not enter the retired review queue"
    );
}

/// cas-895d + cas-bc1b follow-up regression: a task with `worktree_id = None`
/// (non-isolated worker, or direct CLI flow) must skip the close gates
/// entirely, even when the main repo is a live git repo with dirty state.
///
/// This plugs the test-harness hole the earlier cas-895d and cas-bc1b
/// tests created: they both used non-git tempdirs as `cas_root.parent()`,
/// so the gates silently no-oped regardless of whether they had the
/// worktree-scoping logic right. Production use has a real git repo
/// with active drift, and running either gate there would reject every
/// close of a non-isolated task.
///
/// Scenarios:
///   * Uncommitted-work gate (cas-895d) — must not fire.
///   * Additive-only gate (cas-bc1b) — must not fire even with
///     `execution_note=additive-only` and committed modifications on
///     the main branch.
#[tokio::test]
async fn test_close_gates_skipped_for_non_isolated_task_with_dirty_main() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Disable verification so we isolate the close gates.
    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[verification]
enabled = false
"#,
    )
    .expect("write config");

    // Turn the directory containing `.cas/` into a real git repo with
    // an active session's worth of dirty state:
    //   * one committed file on main
    //   * one modified tracked file (simulates supervisor mid-edit)
    //   * one staged new file (simulates another non-isolated worker)
    //   * one modification to an existing file committed on main but
    //     not on this task's branch (simulates cas-bc1b scenario on
    //     a non-isolated worker — there IS no branch, so the check
    //     must not fire)
    //
    // Pre-refinement cas-895d+cas-bc1b, both gates would run against
    // this tree and reject the close because of the dirty/staged
    // state that has nothing to do with the task. Post-refinement,
    // both gates skip entirely because `task.worktree_id == None`.
    let project_root = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(project_root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    // Initialize with .cas ignored so the cas metadata doesn't show up
    // as dirt (it isn't what we're testing here).
    //
    // The drift files are deliberately docs-only (`.md`) so they don't
    // also trip the cas-b39f code-review gate — that gate correctly
    // scans the main tree for reviewable changes and would require a
    // findings envelope. It's an independent concern from the
    // cas-895d/cas-bc1b fix this test is validating, so we pick
    // non-reviewable content for the drift. The cas-895d gate itself
    // checks every non-`??` status line regardless of file type, so
    // `.md` dirt exercises it just as well as `.rs`.
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(project_root.join(".gitignore"), ".cas/\n").unwrap();
    std::fs::write(project_root.join("shared.md"), "# shared\n\n- one\n").unwrap();
    git(&["add", ".gitignore", "shared.md"]);
    git(&["commit", "-q", "-m", "main: initial"]);

    // Now dirty the main tree the way a live session would:
    //   - modify shared.md (unstaged)
    //   - stage a brand-new file
    std::fs::write(project_root.join("shared.md"), "# shared\n\n- one\n- two\n").unwrap();
    std::fs::write(project_root.join("supervisor_wip.md"), "# in flight\n").unwrap();
    git(&["add", "supervisor_wip.md"]);

    // --- Scenario A: uncommitted-work gate (cas-895d) MUST NOT fire
    //     for a task with no worktree_id, even with the above drift.
    let task_store = open_task_store(&cas_dir).expect("open task store");

    let create_req = TaskCreateRequest {
        depth: None,
        title: "Non-isolated task over dirty main (cas-895d skip)".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();
    let _ = service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("start");

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("non-isolated direct CLI flow".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns"),
    );
    assert!(
        resp.contains("Closed task:"),
        "non-isolated task must not be rejected by cas-895d gate on \
         dirty main worktree: {resp}"
    );
    assert!(
        !resp.contains("UNCOMMITTED WORK"),
        "cas-895d gate must not fire for non-isolated tasks: {resp}"
    );
    assert_eq!(
        task_store.get(&id).expect("task").status,
        cas::types::TaskStatus::Closed
    );

    // --- Scenario B: additive-only gate (cas-bc1b) MUST NOT fire for a
    //     non-isolated task, even with execution_note=additive-only.
    //     For this we also commit a *modification* on main to prove
    //     the gate isn't running a branch-diff against the working
    //     tree's history either — the task has no branch of its own.
    git(&["add", "shared.md"]);
    git(&["commit", "-q", "-m", "main: extend shared.md"]);

    let create_additive_req = TaskCreateRequest {
        depth: None,
        title: "Non-isolated additive-only task (cas-bc1b skip)".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: Some("additive-only".to_string()),
        epic: None,
    };
    let additive_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_additive_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();
    let _ = service
        .cas_task_start(Parameters(IdRequest {
            id: additive_id.clone(),
        }))
        .await
        .expect("start");

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: additive_id.clone(),
        reason: Some("additive-only non-isolated".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns"),
    );
    assert!(
        resp.contains("Closed task:"),
        "non-isolated additive-only task must not be rejected by \
         cas-bc1b gate on dirty main worktree: {resp}"
    );
    assert!(
        !resp.contains("ADDITIVE-ONLY VIOLATION"),
        "cas-bc1b gate must not fire for non-isolated tasks: {resp}"
    );
    assert_eq!(
        task_store.get(&additive_id).expect("task").status,
        cas::types::TaskStatus::Closed
    );
}

/// cas-895d complement: a task with no attached worktree and a clean
/// project root still passes the gate. Ensures the gate doesn't break
/// non-factory (direct CLI) flows where there's no worktree to inspect.
#[tokio::test]
async fn test_task_close_passes_without_worktree_and_clean_cwd() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[verification]
enabled = false
"#,
    )
    .expect("write config");

    let create_req = TaskCreateRequest {
        depth: None,
        title: "Notes-only task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();

    let _ = service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("start");

    // cas_root.parent() for the test is the temp dir which is not a
    // git repo → check_uncommitted_work returns empty → close passes.
    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("done, no files touched".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("Closed task:"),
        "non-git project root must not block close: {resp}"
    );
}

/// cas-3894 AC1: the recorded deadlock, end to end through `cas_task_close`.
/// A worker's own InProgress task is halted by an unrelated, informational
/// urgent (a checkpoint nudge / task briefing — not a redirect about this
/// task). Close must succeed without needing a new assignment: halt is
/// exempt for the caller's own owned task, and every other gate (no git repo
/// here, verification disabled) is already satisfied exactly as in
/// `test_task_close_passes_without_worktree_and_clean_cwd` above.
#[tokio::test]
async fn test_3894_halted_worker_can_close_own_in_progress_task() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[verification]
enabled = false
"#,
    )
    .expect("write config");

    let create_req = TaskCreateRequest {
        depth: None,
        title: "Finished, gate-green work".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();

    let _ = service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("start"); // sets task.assignee = "test-agent", status = InProgress

    // Simulate the urgent-stop halt landing from an unrelated, informational
    // message (checkpoint nudge / task briefing) — exactly like `message.rs`
    // does via `apply_halt_metadata`, but driven directly here since this
    // test is at the CasCore level, not the coordination MCP surface.
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|a| a.name == "test-agent")
            .expect("test agent exists");
        agent
            .metadata
            .insert("halt_task_work".to_string(), "1".to_string());
        agent_store.update(&agent).expect("apply halt");
    }
    assert!(
        agent_store
            .list(None)
            .unwrap()
            .into_iter()
            .find(|a| a.name == "test-agent")
            .unwrap()
            .metadata
            .get("halt_task_work")
            .is_some(),
        "halt must be armed before the close attempt for this test to be meaningful"
    );

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("done, no files touched".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        !resp.contains("WORK HALTED"),
        "a halted worker must be able to close its OWN InProgress task \
         without needing a new assignment (cas-3894): {resp}"
    );
    assert!(
        resp.contains("Closed task:"),
        "close must actually succeed, not merely avoid the halt message: {resp}"
    );
}

/// cas-a699: an unrelated urgent halt must not strand a worker after its own
/// completed delivery has received an approved, current-cycle verdict.
#[tokio::test]
async fn test_a699_halted_approved_delivery_recloses() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("enable verification");

    let created = service
        .cas_task_create(Parameters(simple_task_req(
            "Approved supervisor-review task with an unrelated urgent halt",
        )))
        .await
        .expect("create task");
    let id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("start task");

    // Model the durable delivery projection after the worker's first close.
    // The exact supervisor verdict below resolves the same current cycle.
    let task_store = open_task_store(&cas_dir).expect("task store");
    let mut task = task_store.get(&id).expect("task exists");
    task.status = TaskStatus::AwaitingMerge;
    task.pending_verification = true;
    task_store
        .update(&task)
        .expect("park for delivery verification");
    add_exact_supervisor_fixture_verdict(
        &cas_dir,
        Verification::approved(
            "ver-a699-approved-review".to_string(),
            id.clone(),
            "supervisor approved the current review cycle".to_string(),
        ),
        None,
    );

    let agent_store = open_agent_store(&cas_dir).expect("agent store");
    let mut agent = agent_store
        .list(None)
        .expect("list agents")
        .into_iter()
        .find(|agent| agent.name == "test-agent")
        .expect("test agent exists");
    agent
        .metadata
        .insert("halt_task_work".to_string(), "1".to_string());
    agent_store
        .update(&agent)
        .expect("arm unrelated urgent halt");

    let response = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id.clone(),
                reason: Some("approved review re-close".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("approved review re-close must be a legitimate halt exit"),
    );
    assert!(
        response.contains("Closed task:"),
        "approved review re-close must complete, not only skip WORK HALTED: {response}"
    );
    assert_eq!(
        task_store.get(&id).expect("closed task exists").status,
        TaskStatus::Closed
    );
}

/// cas-0447 (GH #187) pinning regression for the cas-3894 behavior already on
/// main: with halt armed while the caller still owns an InProgress task, an
/// already-merged commit receipt closes successfully without a task-start
/// round trip. This is deliberately an end-to-end receipt/ancestry fixture,
/// not only a unit assertion on the ownership predicate.
#[tokio::test]
async fn test_0447_halted_inprogress_with_merged_receipt_closes_without_restart() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let repo = temp.path();
    let git = |args: &[&str]| -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "epic/cas-0447"]);
    git(&["checkout", "-q", "-b", "factory/test-agent"]);

    let agent_store = open_agent_store(&cas_dir).expect("agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent.role = AgentRole::Worker;
        agent_store.update(&agent).expect("mark worker");
    }
    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("enable verification");
    let task_store = open_task_store(&cas_dir).expect("task store");

    let epic_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                task_type: "epic".to_string(),
                ..simple_task_req("cas-0447 epic")
            }))
            .await
            .expect("create epic"),
    ))
    .expect("epic id")
    .to_string();
    {
        let mut epic = task_store.get(&epic_id).expect("epic");
        epic.branch = Some("epic/cas-0447".to_string());
        task_store.update(&epic).expect("set epic branch");
    }
    let task_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id),
                ..simple_task_req("cas-0447 finishing task")
            }))
            .await
            .expect("create task"),
    ))
    .expect("task id")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest {
            id: task_id.clone(),
        }))
        .await
        .expect("start task");

    std::fs::write(repo.join("worker.txt"), "finished work\n").unwrap();
    git(&["add", "worker.txt"]);
    git(&["commit", "-q", "-m", &format!("{task_id}: worker change")]);
    let receipt = git(&["rev-parse", "HEAD"]);
    open_verification_store(&cas_dir)
        .expect("verification store")
        .add(&Verification::approved(
            "ver-cas-0447".to_string(),
            task_id.clone(),
            "approved before merge".to_string(),
        ))
        .expect("approve task");

    // Merge while the task projection remains InProgress; this is the GH #187
    // finishing-worker shape, where only bookkeeping remains.
    git(&["checkout", "-q", "epic/cas-0447"]);
    git(&["merge", "--no-ff", "-q", "factory/test-agent"]);
    git(&["checkout", "-q", "factory/test-agent"]);

    {
        let mut agent = agent_store
            .list(None)
            .unwrap()
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .unwrap();
        agent
            .metadata
            .insert("halt_task_work".to_string(), "1".to_string());
        agent_store.update(&agent).expect("arm halt");
    }

    let merged = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: task_id.clone(),
                reason: Some("finished and merged".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: Some(receipt),
            }))
            .await
            .expect("merged receipt close must bypass halt"),
    );
    assert!(merged.contains("Closed task:"), "close response: {merged}");
    assert_eq!(task_store.get(&task_id).unwrap().status, TaskStatus::Closed);
}

/// cas-3894 AC2 (safety property): the halt-exemption is ownership-bound.
/// A halted worker must still be refused when attempting to close a task it
/// does NOT own — the exemption never lets a halted worker act on anyone
/// else's work, which is what would actually defeat a genuine redirect
/// aimed at a different assignee.
#[tokio::test]
async fn test_3894_halted_worker_still_blocked_closing_unowned_task() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[verification]
enabled = false
"#,
    )
    .expect("write config");

    let task_store = open_task_store(&cas_dir).expect("open task store");
    let id = task_store.generate_id().expect("generate_id");
    let mut task = cas::types::Task::new(id.clone(), "Someone else's work".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("someone-else".to_string()); // NOT "test-agent"
    task_store
        .add(&task)
        .expect("add task owned by another agent");

    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|a| a.name == "test-agent")
            .expect("test agent exists");
        agent
            .metadata
            .insert("halt_task_work".to_string(), "1".to_string());
        agent_store.update(&agent).expect("apply halt");
    }

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("trying to close someone else's task".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    // The halt gate rejects via `Err(McpError)`, not an `Ok` tool-error
    // response — unlike the exempt path, which returns `Ok`. Accept either
    // shape so this test asserts the message content regardless of which
    // one the gate uses.
    let resp = match service.cas_task_close(Parameters(close_req)).await {
        Ok(result) => extract_text(result),
        Err(e) => e.message.to_string(),
    };
    assert!(
        resp.contains("WORK HALTED"),
        "halt must still block close of a task the caller does not own: {resp}"
    );
}

#[tokio::test]
async fn test_epic_close_requires_epic_verification_type() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Create epic
    let req = TaskCreateRequest {
        depth: None,
        title: "Epic requiring epic verification".to_string(),
        description: None,
        priority: 2,
        task_type: "epic".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Start epic
    let start_req = IdRequest { id: id.to_string() };
    let _ = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    // Close without verification should be blocked
    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.to_string(),
        reason: Some("Completed".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");
    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "Epic close should be blocked without verification: {text}"
    );

    // Add a task-level verification - should NOT unblock epic close
    let task_dispatch = cas_store::get_latest_verification_dispatch(&cas_dir, id)
        .unwrap()
        .unwrap();
    let task_ver = Verification::approved(
        "ver-epic-task".to_string(),
        id.to_string(),
        "Task-level verification".to_string(),
    );
    add_exact_supervisor_fixture_verdict(&cas_dir, task_ver, Some(&task_dispatch.id));

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.to_string(),
        reason: Some("Completed".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");
    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "Epic close should still be blocked with task-level verification: {text}"
    );

    // Add epic-level verification - should unblock
    let epic_dispatch = cas_store::get_latest_verification_dispatch(&cas_dir, id)
        .unwrap()
        .expect("fresh epic proof cycle");
    assert_ne!(epic_dispatch.id, task_dispatch.id);
    let mut epic_ver = Verification::approved(
        "ver-epic-ok".to_string(),
        id.to_string(),
        "Epic verification passed".to_string(),
    );
    epic_ver.verification_type = VerificationType::Epic;
    add_exact_supervisor_fixture_verdict(&cas_dir, epic_ver, Some(&epic_dispatch.id));

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.to_string(),
        reason: Some("Completed".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should succeed");
    let text = extract_text(result);
    assert!(
        text.contains("Closed") || text.contains("closed"),
        "Epic should close with epic verification: {text}"
    );
}

#[tokio::test]
async fn test_task_lifecycle_with_verification() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Create task
    let req = TaskCreateRequest {
        depth: None,
        title: "Lifecycle task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Start task
    let start_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    let text = extract_text(result);
    assert!(text.contains("Started") || text.contains("in_progress"));

    // Create an approved verification record
    let verification = Verification::approved(
        "ver-test".to_string(),
        id.to_string(),
        "All checks passed".to_string(),
    );
    add_exact_supervisor_fixture_verdict(&cas_dir, verification, None);

    // Close task - should succeed now with verification
    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.to_string(),
        reason: Some("Completed successfully".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should succeed");

    let text = extract_text(result);
    assert!(
        text.contains("Closed") || text.contains("closed"),
        "Task should close with verification: {text}"
    );
    assert!(
        text.contains("verified"),
        "Should indicate verification: {text}"
    );
}

#[tokio::test]
async fn test_task_close_blocked_with_rejected_verification() {
    use cas::types::VerificationIssue;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Create task
    let req = TaskCreateRequest {
        depth: None,
        title: "Task with rejected verification".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Start task
    let start_req = IdRequest { id: id.to_string() };
    let _ = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    // Create a rejected verification record with issues
    let issues = vec![VerificationIssue::new(
        "src/main.rs".to_string(),
        "todo_comment".to_string(),
        "TODO comment found".to_string(),
    )];
    let verification = Verification::rejected(
        "ver-reject".to_string(),
        id.to_string(),
        "Found incomplete work".to_string(),
        issues,
    );
    add_exact_supervisor_fixture_verdict(&cas_dir, verification, None);

    // Try to close task - should be blocked due to rejected verification
    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.to_string(),
        reason: Some("Completed".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");

    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION FAILED"),
        "Close should be blocked with rejected verification: {text}"
    );
    assert!(text.contains("1 issue"), "Should show issue count: {text}");
}

/// Regression test for cas-7de3: `task.close` must either dispatch a verifier
/// (creating a verification row) or close the task with an explicit skip
/// reason recorded in notes/metadata. The pre-fix behavior returned a
/// `⚠️ VERIFICATION REQUIRED` warning string while leaving the task in
/// `InProgress` with no verification row — a fire-and-forget that silently
/// drops the close attempt. This test fails on main and passes once the
/// dispatch/skip path is wired up.
#[tokio::test]
async fn test_task_close_runs_verifier_or_skips_cleanly() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Create + start a task.
    let req = TaskCreateRequest {
        depth: None,
        title: "Dispatch-on-close regression task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");
    let id = extract_task_id(&extract_text(result))
        .expect("should have task ID")
        .to_string();

    let _ = service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start should succeed");

    // Close with a clean, acceptance-criteria-satisfying reason. This is the
    // exact shape of close call that triggered the cas-7de3 regression: the
    // handler is supposed to dispatch a verifier (or record a skip), not just
    // print a warning and leave the task open.
    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("Completed all acceptance criteria. Deployed to prod.".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");
    let response_text = extract_text(result);

    // Re-read DB state after the call.
    let task_after = task_store.get(&id).expect("task should still exist");
    let verification_row = verification_store
        .get_latest_for_task(&id)
        .expect("verification lookup should not error");

    let dispatched_verifier = verification_row.is_some();
    let closed_with_skip_reason = task_after.status == cas::types::TaskStatus::Closed
        && (task_after
            .notes
            .to_lowercase()
            .contains("verification skipped")
            || task_after
                .close_reason
                .as_deref()
                .map(|r| r.to_lowercase().contains("verification skipped"))
                .unwrap_or(false));

    assert!(
        dispatched_verifier || closed_with_skip_reason,
        "task.close must either dispatch a verifier (create a verification row) \
         or close the task with an explicit skip reason. Got:\n\
         \x20 - response text: {response_text}\n\
         \x20 - task status after close: {:?}\n\
         \x20 - verification row present: {dispatched_verifier}\n\
         \x20 - task notes: {:?}\n\
         \x20 - task close_reason: {:?}\n\
         This is the cas-7de3 regression: the handler returned a fire-and-forget \
         warning without actually running verification or recording a skip.",
        task_after.status,
        task_after.notes,
        task_after.close_reason,
    );
}

// === cas-26e1: supervisor escape hatch ===
//
// These tests lock down the supervisor-close bypass that shipped in
// close_ops.rs lines 64-82 (`assignee_inactive` path). Precedent: gabber-studio
// April 2-3 session `f21e74e7-3c57-4cf6-a295-ca6b8e113e79` closed ~12 worker
// tasks via this hatch after workers wedged (cas-bd17, cas-d6b0, cas-ce02,
// cas-79e9, cas-74b7, cas-6f19, cas-901d, cas-e3a3, cas-80de, cas-c5be,
// cas-ff22, cas-2bf7).
//
// The hatch is STRUCTURAL, not a reason-string match: it fires when BOTH
// `is_supervisor_from_env()` is true AND the task's assignee is missing /
// not-found / heartbeat-expired. The "verification skipped — assignee inactive"
// string is only a display note the handler appends to the success message
// (close_ops.rs:487); the supervisor's close_reason does not gate the hatch.
//
// These tests MUST still pass after cas-4acd narrowed the per-tool
// verification jail at server/mod.rs:646-663 to stop exempting `task.close`
// for factory workers. That narrowing affects the pre-handler jail; the bypass
// itself lives inside close_ops.rs and is unaffected — these tests verify
// that directly.

/// Shared RAII guard that **snapshots** the prior value of each factory env
/// var it mutates and restores it on drop — setting it back to its previous
/// value, or removing it only if it was originally absent — instead of
/// blindly `remove_var`-ing. This prevents a guard from clobbering a
/// pre-existing factory env value owned by the surrounding test/process
/// (cas-7cc9: the old guards unconditionally removed CAS_AGENT_ROLE /
/// CAS_FACTORY_MODE / CAS_FACTORY_WORKER_CLI / CAS_FACTORY_SUPERVISOR_CLI on
/// drop, leaking test pollution and breaking sibling factory env assumptions).
///
/// Every caller acquires `env_test_lock()` for the guard's full lifetime, so
/// these process-global mutations never race another test thread.
struct ScopedFactoryEnv {
    /// (key, prior value) captured at construction, replayed on drop.
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl ScopedFactoryEnv {
    /// Apply each `(key, desired)` pair, capturing the prior value first:
    /// `Some(v)` sets `key=v`; `None` removes `key`. On drop every key is
    /// restored to the value captured here.
    fn apply(vars: &[(&'static str, Option<&str>)]) -> Self {
        let mut saved = Vec::with_capacity(vars.len());
        // SAFETY: callers hold env_test_lock() for the guard's lifetime, so
        // no other test thread can observe a torn read of these vars.
        unsafe {
            for (key, desired) in vars {
                saved.push((*key, std::env::var_os(key)));
                match desired {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self { saved }
    }
}

impl Drop for ScopedFactoryEnv {
    fn drop(&mut self) {
        // SAFETY: same env_test_lock() contract as `apply`.
        unsafe {
            for (key, prior) in self.saved.drain(..) {
                match prior {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

/// Small RAII guard so CAS_AGENT_ROLE is set to `supervisor` for the test
/// body and restored to its prior value on drop, even on panic.
struct ScopedSupervisorEnv {
    _env: ScopedFactoryEnv,
}

impl ScopedSupervisorEnv {
    fn new() -> Self {
        // SAFETY: setup_cas documents the same env_test_lock contract; the
        // guard snapshots and restores rather than blindly removing.
        Self {
            _env: ScopedFactoryEnv::apply(&[("CAS_AGENT_ROLE", Some("supervisor"))]),
        }
    }
}

/// A supervisor-owned epic has no ordinary task assignee by design. Once the
/// configured verification owner passes the close gate, the response and
/// audit row must describe the epic verification semantics rather than the
/// unrelated orphan-recovery path.
#[tokio::test]
async fn test_close_supervisor_owned_epic_uses_owner_closed_wording() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let owner_id = agent_store
        .list(None)
        .expect("list agents")
        .first()
        .map(|agent| agent.id.clone())
        .expect("setup_cas should register the closing agent");

    let req = TaskCreateRequest {
        depth: None,
        title: "Supervisor-owned epic".to_string(),
        description: None,
        priority: 2,
        task_type: "epic".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed"),
    ))
    .expect("should have task ID")
    .to_string();

    let mut epic = task_store.get(&id).expect("epic should exist");
    epic.status = cas::types::TaskStatus::InProgress;
    epic.assignee = None;
    epic.epic_verification_owner = Some(owner_id);
    task_store.update(&epic).expect("should update epic");

    let _guard = ScopedSupervisorEnv::new();
    let response_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id.clone(),
                reason: Some("all child tasks complete".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("owner should close epic"),
    );

    assert!(
        response_text
            .contains("epic verification: owner-closed; child tasks individually verified"),
        "owner-close must explain epic verification semantics: {response_text}"
    );
    assert!(
        !response_text.contains("orphaned task"),
        "healthy supervisor-owned epic must not be labeled orphaned: {response_text}"
    );

    let persisted = task_store.get(&id).expect("closed epic should persist");
    assert_eq!(persisted.status, cas::types::TaskStatus::Closed);
    assert_eq!(
        persisted.close_reason.as_deref(),
        Some("all child tasks complete")
    );

    let row = verification_store
        .get_latest_for_task(&id)
        .expect("verification lookup")
        .expect("owner-close should write an auditable Skipped row");
    assert_eq!(row.status, cas::types::VerificationStatus::Skipped);
    assert!(
        row.summary.contains("closed by its verification owner")
            && !row.summary.contains("orphaned"),
        "audit row must describe owner-close semantics: {}",
        row.summary
    );
}

/// Positive: supervisor closes an orphaned task (no assignee) → bypass fires.
/// Task goes to Closed without running the verifier and without writing a
/// verification row. The close_reason passed by the supervisor is preserved
/// on the task and the response carries the
/// "(verification skipped — assignee inactive)" marker.
#[tokio::test]
async fn test_close_supervisor_bypass_orphaned_task() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Create + start a task, then strip its assignee to simulate the
    // orphaned-worker state the hatch is designed to recover from.
    let req = TaskCreateRequest {
        depth: None,
        title: "Orphaned worker task for escape-hatch test".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let create_text = extract_text(
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed"),
    );
    let id = extract_task_id(&create_text)
        .expect("should have task ID")
        .to_string();

    // Note: cas_task_start would set the assignee to the current test agent,
    // which would then be "alive" and short-circuit the inactive path. We want
    // the orphaned branch (`No assignee at all → orphaned`), so we set status
    // directly and leave assignee = None.
    let mut task = task_store.get(&id).expect("task should exist");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = None;
    task_store.update(&task).expect("should update task");

    // Now flip the process into supervisor mode for the close call only.
    let _guard = ScopedSupervisorEnv::new();

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("verification skipped — assignee inactive".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should succeed via supervisor bypass");
    let response_text = extract_text(result);

    assert!(
        response_text.contains("Closed"),
        "bypass close should report success: {response_text}"
    );
    // cas-3bd4: orphaned (no-assignee) closes now cite the accurate
    // reason — "orphaned task, no assignee" — instead of the catch-all
    // "assignee inactive" phrase that was always emitted regardless of
    // actual assignee state.
    assert!(
        response_text.contains("verification skipped — orphaned task, no assignee"),
        "response must carry the orphaned-task bypass marker: {response_text}"
    );
    assert!(
        !response_text.contains("VERIFICATION REQUIRED"),
        "bypass must not drop into the jail path: {response_text}"
    );

    let task_after = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task_after.status,
        cas::types::TaskStatus::Closed,
        "supervisor bypass must transition task to Closed"
    );
    assert_eq!(
        task_after.close_reason.as_deref(),
        Some("verification skipped — assignee inactive"),
        "supervisor close_reason must be preserved verbatim"
    );
    assert!(
        task_after
            .notes
            .to_lowercase()
            .contains("verification skipped"),
        "close_reason must also appear in the task notes timeline: {}",
        task_after.notes
    );

    // Per cas-82d6: the bypass path MUST write a durable `Skipped`
    // verification row so downstream workers that inherit a BlockedBy on
    // this task are not jailed by `check_pending_verification` (which used
    // to only accept `Approved`). The row is the audit trail for "closed
    // without running the verifier".
    let verification_row = verification_store
        .get_latest_for_task(&id)
        .expect("verification lookup should not error")
        .expect("supervisor bypass must write a Skipped verification row");
    assert_eq!(
        verification_row.status,
        cas::types::VerificationStatus::Skipped,
        "bypass row must be Skipped, got {:?}",
        verification_row.status
    );
}

/// Positive: supervisor closes a task whose assignee points at an agent that
/// does not exist in the agent store. This exercises the "assignee not found →
/// treat as inactive" branch distinct from the None-assignee branch above.
#[tokio::test]
async fn test_close_supervisor_bypass_ghost_assignee() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();

    let req = TaskCreateRequest {
        depth: None,
        title: "Task assigned to a ghost agent".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed"),
    ))
    .expect("should have task ID")
    .to_string();

    let mut task = task_store.get(&id).expect("task should exist");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = Some("ghost-agent-does-not-exist".to_string());
    task_store.update(&task).expect("should update task");

    let _guard = ScopedSupervisorEnv::new();

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("verification skipped — assignee inactive (ghost agent)".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let response_text = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("task_close should succeed via supervisor bypass"),
    );

    // cas-3bd4: a ghost assignee (agent row missing from the store) is
    // now reported as "assignee unknown" — the pre-cas-3bd4 path
    // always said "assignee inactive" regardless of the true state,
    // because `agent_store.get(name)` unwrap_or(true) collapsed every
    // lookup failure into the same bucket. The new path keeps the
    // supervisor bypass behavior but cites the real reason.
    assert!(
        response_text.contains("Closed")
            && response_text.contains("verification skipped — assignee unknown"),
        "ghost-assignee bypass should close and mark skipped: {response_text}"
    );

    let task_after = task_store.get(&id).expect("task should exist");
    assert_eq!(task_after.status, cas::types::TaskStatus::Closed);
    // Per cas-82d6: bypass now writes a Skipped row so downstream
    // BlockedBy consumers don't hit the MCP jail.
    let row = verification_store
        .get_latest_for_task(&id)
        .expect("verification lookup should not error")
        .expect("ghost-assignee bypass must write a Skipped verification row");
    assert_eq!(row.status, cas::types::VerificationStatus::Skipped);
}

/// cas-3bd4 regression: a factory worker's `task.assignee` stores the agent's
/// display *name* (e.g. `"mighty-viper-52"`), not its session id. The pre-fix
/// `agent_store.get(task.assignee)` therefore always failed, `unwrap_or(true)`
/// treated the assignee as inactive, and supervisor closes silently succeeded
/// with the misleading message `"verification skipped — assignee inactive"`
/// even when the worker was demonstrably alive and holding a fresh lease.
///
/// Post-fix, the close path resolves liveness from the task's active lease
/// (`TaskLease.agent_id` is the real session id), which survives the name/id
/// mismatch. A supervisor closing such a task without `bypass_code_review=true`
/// must now drop into the normal verification path; with the flag set, the
/// close proceeds but the audit message cites "supervisor bypass", never
/// "assignee inactive".
#[tokio::test]
async fn test_close_supervisor_active_worker_assignee_by_name() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");

    // Register a fresh, alive agent with a distinct display name so the
    // id-vs-name mismatch is unambiguous.
    let mut worker = cas::types::Agent::new(
        "test-worker-by-name".to_string(),
        "mighty-viper-99".to_string(),
    );
    worker.agent_type = cas::types::AgentType::Worker;
    worker.role = cas::types::AgentRole::Worker;
    worker.heartbeat(); // ensure fresh last_heartbeat + Active status
    agent_store.register(&worker).expect("register worker");

    let create_req = TaskCreateRequest {
        depth: None,
        title: "Task held by a by-name assignee".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create should succeed"),
    ))
    .expect("task id")
    .to_string();

    // Store the assignee as the NAME (production bug shape) and put the
    // task in-progress, then claim it on behalf of the worker so the lease
    // carries the real session id.
    let mut task = task_store.get(&id).expect("task exists");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = Some("mighty-viper-99".to_string());
    task_store.update(&task).expect("update task");
    agent_store
        .try_claim(
            &id,
            &worker.id,
            600,
            Some("worker lease for cas-3bd4 repro"),
        )
        .expect("worker claim should succeed");

    // Flip the caller to supervisor for the close attempt.
    let _guard = ScopedSupervisorEnv::new();

    // --- Attempt 1: no bypass flag. The close MUST drop into the normal
    //     verification path (worker is alive + holding a lease), not the
    //     bypass branch. Pre-fix this path falsely reported the worker as
    //     inactive and closed the task.
    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("worker finished, asking supervisor to close".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let response_text = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("task_close returns a result"),
    );
    assert!(
        response_text.contains("VERIFICATION REQUIRED"),
        "active-by-name assignee must NOT trigger inactive bypass — got: {response_text}"
    );
    assert!(
        !response_text.contains("Closed task:"),
        "no bypass flag + active assignee must not transition to Closed: {response_text}"
    );
    assert!(
        !response_text.contains("assignee inactive"),
        "active assignee must never be reported as inactive: {response_text}"
    );
    assert_ne!(
        task_store.get(&id).expect("task exists").status,
        cas::types::TaskStatus::Closed,
        "active assignee + no bypass must leave the task open"
    );

    // --- Attempt 2: review bypass cannot erase the exact dispatch created by
    //     attempt 1. Supervisor-direct recovery must name and resolve it.
    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        reason: Some("supervisor forced close after alignment".to_string()),
        supervisor_override: Some(true),
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let response_text = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("task_close returns a result"),
    );
    assert!(
        response_text.contains("VERIFICATION REQUIRED"),
        "supervisor review bypass must not replace an exact verdict: {response_text}"
    );
    assert!(
        !response_text.contains("assignee inactive"),
        "active assignee must never be reported as inactive even with bypass: {response_text}"
    );
    assert_ne!(
        task_store.get(&id).expect("task exists").status,
        cas::types::TaskStatus::Closed,
        "active exact dispatch must keep the task open"
    );
}

/// Negative: supervisor closes a task whose assignee is the currently-alive
/// test agent. `is_heartbeat_expired(300)` is false for a freshly registered
/// agent, so the bypass does NOT fire and close drops into the normal
/// verification path. This pins the bypass to the specific inactive-assignee
/// precondition and proves the hatch isn't a catch-all "supervisor closes
/// anything" escape.
///
/// After cas-4acd narrowed the per-tool jail at server/mod.rs:646-663 to stop
/// exempting `task.close` for factory workers, the jail text returned here
/// comes from `close_ops.rs` (VERIFICATION REQUIRED) — exactly what we assert.
#[tokio::test]
async fn test_close_supervisor_no_bypass_when_assignee_alive() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Grab the alive test agent registered by setup_cas.
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let alive_agent_id = agent_store
        .list(None)
        .expect("list agents")
        .first()
        .map(|a| a.id.clone())
        .expect("setup_cas should register a test agent");

    let req = TaskCreateRequest {
        depth: None,
        title: "Task with an alive assignee".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed"),
    ))
    .expect("should have task ID")
    .to_string();

    let mut task = task_store.get(&id).expect("task should exist");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = Some(alive_agent_id);
    task_store.update(&task).expect("should update task");

    let _guard = ScopedSupervisorEnv::new();

    let close_req = TaskCloseRequest {
        stranded_branch_override: None,
        id: id.clone(),
        // Intentionally still use the "verification skipped" phrase to prove
        // the bypass is structural (assignee state), not reason-driven. Even
        // with this phrase, an alive assignee must keep the jail engaged.
        reason: Some("verification skipped — assignee inactive".to_string()),
        supervisor_override: None,
        legacy_bypass_code_review: None,
        search_manifest: None,
        commit_receipt: None,
    };
    let response_text = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("task_close should return a result"),
    );

    assert!(
        response_text.contains("VERIFICATION REQUIRED"),
        "alive assignee must NOT trigger the bypass — expected VERIFICATION REQUIRED: {response_text}"
    );
    assert!(
        !response_text.contains("Closed task:"),
        "alive assignee path must not report a closed task: {response_text}"
    );

    let task_after = task_store.get(&id).expect("task should exist");
    assert_ne!(
        task_after.status,
        cas::types::TaskStatus::Closed,
        "alive assignee + supervisor must not transition task to Closed"
    );

    // A dispatch-request verification row should have been persisted for the
    // normal path (cas-7de3 regression coverage). This also confirms the
    // close attempt exercised the dispatch branch, not the bypass branch.
    let verification_row = verification_store
        .get_latest_for_task(&id)
        .expect("verification lookup should not error")
        .expect("alive-assignee close should persist a dispatch-request row");
    assert_eq!(
        verification_row.status,
        cas::types::VerificationStatus::Error,
        "dispatch-request row should have Error status until a verdict lands"
    );
}
// =============================================================================
// cas-9a3a: task-verifier spawn regression
//
// These tests lock in the post-cas-4acd contract between the three layers
// involved in verifier dispatch:
//
//   1. `authorize_agent_action` (cas-cli/src/mcp/server/mod.rs) — the narrowed
//      factory-worker exemption. All mutations EXCEPT `task.close` remain
//      exempt for workers; `task.close` falls through to
//      `check_pending_verification`. This preserves the bba6fbf fix for the
//      mutation-cascade problem while restoring the jail lever on the one
//      action that actually triggers verifier dispatch.
//   2. `cas_task_close` (close_ops.rs) — writes a durable dispatch-request
//      Verification row and returns a warning with explicit
//      `Task(subagent_type="task-verifier", prompt="Verify task <id>")` syntax.
//   3. The pre_tool hook (pre_tool.rs:164-242) — on a Task/Agent spawn with
//      subagent_type="task-verifier", clears `pending_verification` for the
//      current agent's jailed tasks. The hook path is exercised end-to-end by
//      `cas-cli/tests/e2e/hook_e2e/jail_core.rs::test_agent_tool_spawns_task_verifier_and_unjails`
//      (feature-gated behind `claude_rs_e2e`; see docs/verifier-dispatch-trace.md).
//      The tests below simulate the post-hook state by clearing
//      `pending_verification` directly and writing an approved Verification
//      row, which is what the hook + task-verifier subagent would have done.
// =============================================================================

/// Guard that installs Claude factory-worker env vars for the duration of a
/// test and restores the prior environment on drop. Explicitly clears
/// CAS_FACTORY_WORKER_CLI so a `codex` value leaked from a sibling
/// CodexWorkerEnv guard can't make worker_harness_from_env() report Codex in
/// this Claude-worker context (cas-7cc9 / R2: the old guard left
/// CAS_FACTORY_WORKER_CLI untouched on enter and omitted it on drop).
struct FactoryWorkerEnv {
    _env: ScopedFactoryEnv,
}

impl FactoryWorkerEnv {
    fn enter() -> Self {
        Self {
            _env: ScopedFactoryEnv::apply(&[
                ("CAS_AGENT_ROLE", Some("worker")),
                ("CAS_FACTORY_MODE", Some("1")),
                ("CAS_FACTORY_WORKER_CLI", None),
            ]),
        }
    }
}

/// Build a TaskRequest with only the fields a test needs, via JSON so we
/// don't have to list every Optional field on the struct.
fn task_req(value: serde_json::Value) -> cas_mcp::TaskRequest {
    serde_json::from_value(value).expect("TaskRequest should deserialize from test JSON")
}

/// cas-102c (GH #330 + #333): exercise both field failures through the public
/// unified task service. The worker branch is cut from a main tip containing
/// a recent supervisor hotfix while its declared parent remains behind. It then
/// authors zero commits and supplies its proof only on the close call.
#[tokio::test]
async fn no_code_close_ignores_inherited_base_and_accepts_inline_external_ref_cas_102c() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = false\n",
    )
    .expect("write config");

    let worker_path = temp.path().join("refund-worker");
    std::fs::create_dir_all(&worker_path).expect("create worker checkout");
    let inherited_commit_date = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&worker_path)
            .env("GIT_AUTHOR_NAME", "Supervisor")
            .env("GIT_AUTHOR_EMAIL", "supervisor@example.test")
            .env("GIT_COMMITTER_NAME", "Supervisor")
            .env("GIT_COMMITTER_EMAIL", "supervisor@example.test")
            .env("GIT_AUTHOR_DATE", &inherited_commit_date)
            .env("GIT_COMMITTER_DATE", &inherited_commit_date)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(worker_path.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["branch", "epic/refunds"]);
    std::fs::write(worker_path.join("hotfix.rs"), "pub fn hotfix() {}\n").unwrap();
    git(&["add", "hotfix.rs"]);
    git(&["commit", "-q", "-m", "supervisor hotfix"]);
    git(&["checkout", "-q", "-b", "factory/refund-worker"]);
    let starting_head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&worker_path)
        .output()
        .expect("read HEAD");

    let worktree_store = open_worktree_store(&cas_dir).expect("open worktree store");
    worktree_store.init().expect("init worktree store");
    let worktree_id = Worktree::generate_id();
    worktree_store
        .add(&Worktree::new(
            worktree_id.clone(),
            "factory/refund-worker".to_string(),
            "epic/refunds".to_string(),
            worker_path.clone(),
        ))
        .expect("record worktree");

    let service = CasService::new(core, None);
    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Issue refunds without repository changes",
            "priority": 2,
            "task_type": "chore",
            "execution_note": "no-code"
        }))))
        .await
        .expect("create task");
    let id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();
    let task_store = open_task_store(&cas_dir).expect("open task store");
    let mut task = task_store.get(&id).expect("task exists");
    task.status = TaskStatus::InProgress;
    task.assignee = Some("refund-worker".to_string());
    task.worktree_id = Some(worktree_id);
    task_store.update(&task).expect("attach worker checkout");

    let proof = "80a8d559d docs/release-notes/refunds.md";
    let closed = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id,
            "reason": "Refunds issued and credits granted",
            "external_ref": proof
        }))))
        .await
        .expect("close task");
    let response = extract_text(closed);
    assert!(
        response.contains("Closed task:"),
        "zero-commit task must close cleanly: {response}"
    );
    let stored = task_store.get(&id).expect("closed task exists");
    assert_eq!(stored.status, TaskStatus::Closed);
    assert_eq!(stored.external_ref.as_deref(), Some(proof));
    let ending_head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&worker_path)
        .output()
        .expect("read HEAD");
    assert_eq!(
        starting_head.stdout, ending_head.stdout,
        "task authored no commit"
    );
}

/// cas-099d (recurrence of GH #272, #294, #304, and #333): the unified
/// close surface accepts `execution_note`, so no-code intent supplied there
/// must participate in the same close attempt. In particular, the first close
/// attempt must persist it before an exact verification dispatch freezes the
/// reviewed scope, and a task whose dispatch is already approved must still be
/// closable without a supervisor reopen. Ordinary zero-commit code tasks stay
/// refused with a recovery command that works in their current state.
#[tokio::test]
async fn inline_no_code_intent_survives_dispatch_and_approved_proof_cas_099d() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n[worktrees]\nenabled = false\n",
    )
    .expect("write config");

    let worker_path = temp.path().join("no-code-worker");
    std::fs::create_dir_all(&worker_path).expect("create worker checkout");
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&worker_path)
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
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(worker_path.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "factory/no-code-worker"]);

    let worktree_store = open_worktree_store(&cas_dir).expect("open worktree store");
    worktree_store.init().expect("init worktree store");
    let worktree_id = Worktree::generate_id();
    worktree_store
        .add(&Worktree::new(
            worktree_id.clone(),
            "factory/no-code-worker".to_string(),
            "main".to_string(),
            worker_path,
        ))
        .expect("record worktree");

    let service = CasService::new(core, None);
    let task_store = open_task_store(&cas_dir).expect("open task store");
    macro_rules! create_zero_commit_bug {
        ($title:expr, $depth:expr) => {{
            let created = service
                .task(Parameters(task_req(serde_json::json!({
                    "action": "create",
                    "title": $title,
                    "priority": 2,
                    "task_type": "bug",
                    "depth": $depth
                }))))
                .await
                .expect("create zero-commit bug task");
            let id = extract_task_id(&extract_text(created))
                .expect("task id")
                .to_string();
            service
                .task(Parameters(task_req(serde_json::json!({
                    "action": "start",
                    "id": id
                }))))
                .await
                .expect("start zero-commit bug task");
            let mut task = task_store.get(&id).expect("task exists");
            task.worktree_id = Some(worktree_id.clone());
            task_store.update(&task).expect("attach zero-commit checkout");
            id
        }};
    }

    // Before dispatch: a light task reaches the close gate directly and may
    // declare no-code intent on the close request itself.
    let before_dispatch = create_zero_commit_bug!("No-code before dispatch", "light");
    let response = extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": before_dispatch,
                "reason": "Published the operational disposition",
                "execution_note": "no-code",
                "external_ref": "artifact:cas-099d/before-dispatch"
            }))))
            .await
            .expect("inline no-code close before dispatch"),
    );
    assert!(response.contains("Closed task:"), "{response}");
    assert_eq!(
        task_store
            .get(&before_dispatch)
            .expect("closed task")
            .execution_note
            .as_deref(),
        Some("no-code")
    );

    // After dispatch: the first deep close creates an exact dispatch. The
    // inline intent must already be durable before that proof boundary locks.
    let after_dispatch = create_zero_commit_bug!("No-code after dispatch", "deep");
    let first = extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": after_dispatch,
                "reason": "Published the operational disposition",
                "execution_note": "no-code",
                "external_ref": "artifact:cas-099d/after-dispatch"
            }))))
            .await
            .expect("first close creates dispatch"),
    );
    assert!(first.contains("VERIFICATION REQUIRED"), "{first}");
    let dispatch = cas_store::get_latest_verification_dispatch(&cas_dir, &after_dispatch)
        .expect("dispatch lookup")
        .expect("exact dispatch");
    assert_eq!(
        task_store
            .get(&after_dispatch)
            .expect("dispatched task")
            .execution_note
            .as_deref(),
        Some("no-code"),
        "close metadata must be stored before verification locks its scope"
    );
    add_exact_supervisor_fixture_verdict(
        &cas_dir,
        Verification::approved(
            "ver-cas-099d-after-dispatch".to_string(),
            after_dispatch.clone(),
            "no-code disposition approved".to_string(),
        ),
        Some(&dispatch.id),
    );
    let closed = extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": after_dispatch,
                "reason": "Published the operational disposition",
                "external_ref": "artifact:cas-099d/after-dispatch"
            }))))
            .await
            .expect("close after approved dispatch"),
    );
    assert!(closed.contains("Closed task:"), "{closed}");

    // After approval: reproduce the reported trap by approving a task whose
    // stored execution_note is still empty, then supply the intent on close.
    let after_approval = create_zero_commit_bug!("No-code after approval", "deep");
    add_exact_supervisor_fixture_verdict(
        &cas_dir,
        Verification::approved(
            "ver-cas-099d-before-inline-intent".to_string(),
            after_approval.clone(),
            "existing no-code evidence approved".to_string(),
        ),
        None,
    );
    let closed = extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": after_approval,
                "reason": "Published the operational disposition",
                "execution_note": "no-code",
                "external_ref": "artifact:cas-099d/after-approval"
            }))))
            .await
            .expect("approved no-code close without supervisor reopen"),
    );
    assert!(closed.contains("Closed task:"), "{closed}");
    assert_eq!(
        task_store
            .get(&after_approval)
            .expect("closed approved task")
            .execution_note
            .as_deref(),
        Some("no-code")
    );

    // Negative control: a real code task cannot use absence of commits as
    // evidence of completion, and its recovery command must work even when an
    // approved proof would lock ordinary task.update.
    let code_task = create_zero_commit_bug!("Real code task with no commits", "light");
    let refused = extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": code_task,
                "reason": "No code was committed"
            }))))
            .await
            .expect("zero-commit code refusal"),
    );
    assert!(refused.contains("ZERO-COMMIT CLOSE ON CODE TASK"), "{refused}");
    assert!(!refused.contains("no no execution_note"), "{refused}");
    assert!(
        refused.contains(&format!(
            "task action=close id={code_task} execution_note=no-code external_ref=<portable-reference>"
        )),
        "the refusal must name the close-time recovery that works under proof lock: {refused}"
    );
    assert_eq!(
        task_store.get(&code_task).expect("refused task").status,
        TaskStatus::InProgress
    );
}

/// Narrowed jail — positive case.
///
/// A factory worker who holds an in-progress task with no approved
/// verification must be blocked by `authorize_agent_action` when they
/// attempt `task.close`. Before cas-4acd this path was exempt and the
/// worker saw a passive warning from close_ops instead; after the fix the
/// MCP layer itself rejects the call with `VERIFICATION_JAIL_BLOCKED` and
/// explicit Task() spawn instructions.
#[tokio::test]
async fn test_factory_worker_close_creates_task_scoped_dispatch() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // cas-8edb: under default `[code_review] owner = "supervisor"`, worker
    // closes are no longer jailed (the verification-jail lever is replaced
    // by the supervisor-review queue). This test exists to pin the legacy
    // `owner = "worker"` jail behavior, so opt back in explicitly.
    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[code_review]
owner = "worker"
"#,
    )
    .expect("should write legacy code_review config");

    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    // Create and start a task so it's leased + InProgress with no verification.
    let create = task_req(serde_json::json!({
        "action": "create",
        "title": "Factory worker close-path jail regression",
        "priority": 2,
        "task_type": "task",
    }));
    let created = service
        .task(Parameters(create))
        .await
        .expect("task.create should succeed for factory worker");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    let start = task_req(serde_json::json!({ "action": "start", "id": id }));
    service
        .task(Parameters(start))
        .await
        .expect("task.start should succeed — not jailed yet");

    // Attempt to close. The close path itself creates a durable exact-task
    // dispatch; there is no global dispatcher rejection.
    let close = task_req(serde_json::json!({
        "action": "close",
        "id": id,
        "reason": "Completed all acceptance criteria. Deployed to prod.",
    }));
    let result = service
        .task(Parameters(close))
        .await
        .expect("close returns task-scoped verification guidance");
    let msg = extract_text(result);
    assert!(
        msg.contains("VERIFICATION REQUIRED") && msg.contains("vdispatch-"),
        "close must create an explicit dispatch, got: {msg}"
    );
    // cas-778a: factory workers cannot spawn task-verifier themselves.
    // The jail error for factory workers must recommend forwarding to supervisor
    // via mcp__cas__coordination, NOT the Task() spawn syntax.
    assert!(
        msg.contains("mcp__cas__coordination"),
        "factory worker jail error must recommend mcp__cas__coordination, got: {msg}"
    );
    assert!(
        !msg.contains("Task(subagent_type=\"task-verifier\""),
        "factory worker jail error must NOT instruct spawning task-verifier (workers can't), got: {msg}"
    );
}

// =============================================================================
// Retired supervisor-owned review mode: workers use the standard exact
// verification dispatch regardless of the old `[code_review]` setting.
//
// These tests pin the post-migration behavior for diagnostic and
// additive-only worker closes: neither shape invokes the deleted review
// queue, and both still receive the normal exact verification gate.
// =============================================================================

#[tokio::test]
async fn test_worker_close_zero_diff_uses_standard_verification_cas_8387() {
    let (_temp, core) = setup_cas();
    let _env_lock = env_test_lock();

    // No config.toml written: the removed code-review owner setting has no
    // bearing on the standard verification gate.
    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "cas-8edb: clean zero-diff worker close",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id.clone(),
        }))))
        .await
        .expect("start");

    // Close: the MCP action is admitted, then close_ops creates the exact
    // verification dispatch required by the current contract.
    let result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id.clone(),
            "reason": "Diagnostic only — no code changes.",
        }))))
        .await
        .expect("worker close must return verification guidance");
    let text = extract_text(result);
    assert!(
        !text.contains("VERIFICATION_JAIL_BLOCKED"),
        "owner=supervisor worker close must bypass MCP jail, got: {text}"
    );
    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "worker close must use the standard close_ops verification gate, got: {text}"
    );
}

#[tokio::test]
async fn test_worker_close_additive_only_uses_standard_verification_cas_8387() {
    let (_temp, core) = setup_cas();
    let _env_lock = env_test_lock();

    // Default config: the removed code-review owner setting is irrelevant.
    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "cas-8edb: additive-only worker close",
            "priority": 2,
            "task_type": "task",
            "execution_note": "additive-only",
        }))))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id.clone(),
        }))))
        .await
        .expect("start");

    // Additive-only is a data-state declaration, not a verification bypass.
    let result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id.clone(),
            "reason": "Additive-only docs change — no existing files modified.",
        }))))
        .await
        .expect("worker close must return verification guidance");
    let text = extract_text(result);
    assert!(
        !text.contains("VERIFICATION_JAIL_BLOCKED"),
        "owner=supervisor additive-only worker close must bypass MCP jail, got: {text}"
    );
    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "additive-only worker close must use standard verification, got: {text}"
    );
}

#[tokio::test]
async fn test_legacy_owner_worker_still_requires_exact_close_verification_cas_8edb() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Opt back in to legacy `owner = "worker"` mode. This must still jail a
    // worker close that does not submit a `code_review_findings` envelope —
    // the legacy contract is unchanged by cas-8edb.
    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[code_review]
owner = "worker"
"#,
    )
    .expect("should write legacy code_review config");

    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "cas-8edb: legacy owner=worker still jails",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id.clone(),
        }))))
        .await
        .expect("start");

    let result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id.clone(),
            "reason": "Done.",
        }))))
        .await
        .expect("owner=worker close returns task-scoped verification guidance");
    let msg = extract_text(result);
    assert!(
        msg.contains("VERIFICATION REQUIRED") && msg.contains("vdispatch-"),
        "legacy owner=worker must still gate this close, got: {msg}"
    );
}

/// cas-82d6: a `Skipped` verification row (supervisor bypass audit trail)
/// must satisfy both the MCP jail (`check_pending_verification`) and the
/// close_ops verification gate. Without this, downstream workers that pick
/// up the same task via resumption — or anyone re-closing a task already
/// bypassed — would be trapped by `VERIFICATION_JAIL_BLOCKED`.
#[tokio::test]
async fn test_skipped_verification_row_satisfies_jail_and_close() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let verification_store = open_verification_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    // Create + start a task so it's leased + InProgress.
    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Task with a pre-existing Skipped verification row",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id.clone(),
        }))))
        .await
        .expect("start");

    // Insert a Skipped verification row as if a supervisor had previously
    // closed this task via the orphaned-assignee bypass and then it got
    // resumed/reopened.
    let ver_id = verification_store.generate_id().expect("gen ver id");
    let mut row = cas::types::Verification::skipped(
        ver_id,
        id.clone(),
        "cas-82d6 test fixture — supervisor bypass audit row".to_string(),
    );
    row.verification_type = VerificationType::Task;
    verification_store.add(&row).expect("add skipped row");

    // Close as factory worker. Without the cas-82d6 fix this would hit the
    // narrowed MCP jail (check_pending_verification only accepted Approved)
    // OR the close_ops gate (only accepted Approved). With the fix, Skipped
    // is treated as "has verification record → proceed".
    let result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id.clone(),
            "reason": "Completed all acceptance criteria.",
        }))))
        .await
        .expect("close must succeed when a Skipped row exists");
    let text = extract_text(result);
    assert!(
        text.contains("Closed"),
        "close should succeed with Skipped row present, got: {text}"
    );
    assert!(
        !text.contains("VERIFICATION REQUIRED"),
        "Skipped row must satisfy close_ops gate, got: {text}"
    );
    assert!(
        !text.contains("VERIFICATION_JAIL_BLOCKED"),
        "Skipped row must satisfy MCP jail, got: {text}"
    );
}

/// Narrowed jail — negative case (bba6fbf cascade fix preserved).
///
/// The same factory worker holding a jailed task must still be able to
/// perform OTHER mutations (here, `task.update` on an unrelated task).
/// Only `task.close` triggers the jail now.
#[tokio::test]
async fn test_factory_worker_non_close_mutation_still_exempt() {
    let (_temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    // Task A: will be leased + jailed (no verification record).
    let jailed = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Jailed task A",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create A");
    let jailed_id = extract_task_id(&extract_text(jailed))
        .expect("A id")
        .to_string();
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": jailed_id.clone(),
        }))))
        .await
        .expect("start A");

    // Task B: unrelated, should still be mutable.
    let other = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Unrelated task B",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create B");
    let other_id = extract_task_id(&extract_text(other))
        .expect("B id")
        .to_string();

    // An update on task B is a mutating action. With the narrowed jail it
    // must still be allowed for a factory worker even though task A is
    // blocking a hypothetical close.
    let update = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": other_id,
            "priority": 1,
        }))))
        .await
        .expect("non-close mutation must remain exempt from the narrowed jail");
    let update_text = extract_text(update);
    assert!(
        !update_text.contains("VERIFICATION_JAIL_BLOCKED"),
        "update on unrelated task must not be blocked: {update_text}"
    );
}

/// Full happy path: hook clears jail, verifier writes approved row, close
/// succeeds.
///
/// This simulates the post-pre_tool-hook state. The hook path itself is
/// covered by the e2e test noted in the section header; here we lock in
/// that close_ops.rs correctly observes hook-clearance + approved row and
/// completes the close cleanly.
#[tokio::test]
async fn test_task_close_succeeds_after_verifier_clearance() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Post-hook clearance happy path",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id.clone(),
        }))))
        .await
        .expect("start");

    // Simulate the pre_tool hook: clear pending_verification on the agent's
    // jailed task. (The real hook sets this flag first when close is
    // attempted; here we bypass that attempt since it's covered by
    // test_factory_worker_close_hits_narrowed_jail above.)
    let mut task = task_store.get(&id).expect("task fetch");
    task.pending_verification = false;
    task.updated_at = chrono::Utc::now();
    task_store
        .update(&task)
        .expect("clear pending_verification");

    // Simulate the task-verifier subagent writing an approved verification
    // row via mcp__cas__verification add. This is what the hook+subagent
    // sequence produces on a successful verification run.
    let ver = Verification::approved(
        "ver-9a3a-cleared".to_string(),
        id.clone(),
        "Simulated: hook cleared jail, subagent approved work".to_string(),
    );
    verification_store.add(&ver).expect("record approval");

    // Close must now succeed cleanly — the narrowed jail sees an approved
    // verification and lets it through, close_ops records the closure.
    let closed = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id.clone(),
            "reason": "Completed after verifier clearance.",
        }))))
        .await
        .expect("close should succeed after hook cleared jail + approved row");
    let close_text = extract_text(closed);
    assert!(
        close_text.to_lowercase().contains("closed"),
        "successful close response must mention closure: {close_text}"
    );

    let final_task = task_store.get(&id).expect("task after close");
    assert_eq!(
        final_task.status,
        cas::types::TaskStatus::Closed,
        "task must be persisted as Closed after the successful close"
    );
}

/// cas-c29a/cas-08ca: exact-task verification-dispatch timeout recovery.
///
/// A task enters `pending_verification` on the first close attempt and the
/// If the task-verifier crashes, a retry after the durable deadline marks only
/// the named dispatch timed_out and releases only that task. A different task's
/// pending transition remains untouched.
#[tokio::test]
async fn test_close_auto_escalates_stale_verification_dispatch() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    let verification_store = open_verification_store(&cas_dir).unwrap();
    let task_store = open_task_store(&cas_dir).unwrap();

    // Create + start task.
    let req = TaskCreateRequest {
        depth: None,
        title: "Stuck in verification jail".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create");
    let id = extract_task_id(&extract_text(result))
        .expect("task id")
        .to_string();
    let _ = service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start");

    // First close — sets pending_verification and writes dispatch-request row.
    let _ = service
        .cas_task_close(Parameters(TaskCloseRequest {
            stranded_branch_override: None,
            id: id.clone(),
            reason: Some("Completed".to_string()),
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .expect("first close returns a result");

    let task_after_first = task_store.get(&id).expect("task exists");
    assert!(
        task_after_first.pending_verification,
        "first close must set pending_verification"
    );

    let legacy_dispatch = verification_store
        .get_latest_for_task(&id)
        .expect("get dispatch row")
        .expect("dispatch row exists");
    assert_eq!(
        legacy_dispatch.status,
        cas::types::VerificationStatus::Error
    );
    assert!(legacy_dispatch.summary.starts_with("Dispatch requested"));

    let mut other = cas::types::Task::new(
        "cas-timeout-isolation-b".to_string(),
        "Unrelated pending task B".to_string(),
    );
    other.status = TaskStatus::InProgress;
    other.pending_verification = true;
    task_store.add(&other).expect("add B");
    cas_store::create_verification_dispatch(
        &cas_dir,
        &other.id,
        "worker-b",
        "supervisor-b",
        chrono::Utc::now() + chrono::Duration::minutes(10),
    )
    .expect("create live B dispatch");

    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("db");
    conn.execute(
        "UPDATE verification_dispatches SET deadline_at = ?2 WHERE task_id = ?1",
        rusqlite::params![
            id,
            (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339()
        ],
    )
    .expect("expire exact A dispatch");

    // Second close — should auto-escalate instead of looping.
    let result = service
        .cas_task_close(Parameters(TaskCloseRequest {
            stranded_branch_override: None,
            id: id.clone(),
            reason: Some("Completed".to_string()),
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .expect("second close returns a result");
    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION TIMED OUT"),
        "retry after timeout must report escalation, got: {text}"
    );
    assert!(
        !text.contains("VERIFICATION REQUIRED"),
        "escalation must not fall back to the standard jail message"
    );

    // pending_verification is cleared only for A.
    let task_after_escalation = task_store.get(&id).expect("task exists");
    assert!(
        !task_after_escalation.pending_verification,
        "auto-escalation must clear pending_verification"
    );

    assert_eq!(
        cas_store::get_latest_verification_dispatch(&cas_dir, &id)
            .expect("A dispatch lookup")
            .expect("A dispatch")
            .state,
        cas::types::VerificationDispatchState::TimedOut
    );
    assert!(
        task_store.get(&other.id).expect("B").pending_verification,
        "timing out A must not clear B"
    );
    assert_eq!(
        cas_store::get_latest_verification_dispatch(&cas_dir, &other.id)
            .expect("B dispatch lookup")
            .expect("B dispatch")
            .state,
        cas::types::VerificationDispatchState::Pending,
        "timing out A must not mutate B's dispatch"
    );
}

// =============================================================================
// cas-a90f3: verification.add supervisor authz error message clarity
//
// The original rejection — "Supervisors can only verify epics, not individual
// tasks" — was misleading. Field-confirmed in gabber-studio logs: the rule
// actually depends on whether the task has a *currently live* assignee at
// call time. Several supervisor calls on individual tasks succeed (orphaned,
// dead/expired assignee, supervisor-is-assignee, task-verifier subagent
// context); the rejection only fires for the active-assignee case.
//
// This test pins the new error wording: it must name the rule (active
// assignee), include the offending assignee id, list the three supervisor
// exemptions, and give a concrete remediation path.
// =============================================================================

/// Minimal CasCore rooted in `temp` with a *Supervisor-role* agent
/// pre-set as the current session. `support::setup_cas` always registers a
/// Standard-role agent and pins it via OnceLock, so we can't reuse it for
/// this test — we need the verification-tools authz path to see
/// `agent.role == AgentRole::Supervisor`.
///
/// Mirrors `support::setup_cas`'s factory-env-clearing block (it briefly
/// holds `env_test_lock()` for the mutation, matching the support.rs
/// ordering contract). Callers should `let _env_lock = env_test_lock();`
/// **after** this returns to hold the lock for the test body — std `Mutex`
/// is not re-entrant, so taking it before would deadlock.
///
/// Returns the temp dir guard, the core (used by tests as `service` —
/// MCP tool methods are defined directly on `CasCore`), and the supervisor
/// session id.
fn setup_cas_with_supervisor_session() -> (TempDir, cas::mcp::CasCore, String) {
    // Clear factory env vars under the shared env lock so a parallel
    // sibling test cannot observe a torn read. Match the four vars
    // `support::setup_cas` clears so the two helpers do not drift.
    {
        let _env_guard = env_test_lock();
        // SAFETY: we hold the process-wide env lock for the duration of
        // this block; no other test thread can observe a torn env read.
        unsafe {
            std::env::remove_var("CAS_AGENT_ROLE");
            std::env::remove_var("CAS_FACTORY_MODE");
            std::env::remove_var("CAS_FACTORY_SUPERVISOR_CLI");
            std::env::remove_var("CAS_FACTORY_WORKER_CLI");
        }
    }

    let temp = TempDir::new().expect("temp dir");
    let cas_root = init_cas_dir(temp.path()).expect("init_cas_dir");

    let agent_store = open_agent_store(&cas_root).expect("open agent store");
    let supervisor_id = format!("supervisor-test-cas-a90f3-{}", std::process::id());
    let mut supervisor =
        cas::types::Agent::new(supervisor_id.clone(), "alpha-supervisor".to_string());
    supervisor.role = cas::types::AgentRole::Supervisor;
    supervisor.heartbeat();
    agent_store
        .register(&supervisor)
        .expect("register supervisor");

    let core = cas::mcp::CasCore::with_daemon(cas_root, None, None);
    core.set_agent_id_for_testing(supervisor_id.clone());

    (temp, core, supervisor_id)
}

/// A registered Supervisor role is server-authenticated external authority,
/// even when a live worker owns the task. Caller-supplied names and environment
/// claims are irrelevant; the persisted provenance must reflect the role gate.
#[tokio::test]
async fn test_registered_supervisor_can_verify_live_worker_task() {
    // Per support.rs ordering contract: setup helper FIRST (it briefly
    // grabs the lock to clear factory env vars), then acquire the lock
    // for the test body. std `Mutex` is not re-entrant — reversing the
    // order would deadlock. Clearing the factory env vars ensures
    // `worker_harness_from_env()` falls back to Claude (subagents=true)
    // and the supervisor authz branch actually runs.
    let (temp, service, supervisor_id) = setup_cas_with_supervisor_session();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let task_store = open_task_store(&cas_dir).expect("open task store");

    // Register a fresh, alive worker — distinct from the supervisor session
    // and freshly heartbeated so `is_alive() && !is_heartbeat_expired(300)`.
    let worker_id = format!("fresh-worker-cas-a90f3-{}", std::process::id());
    let mut worker = cas::types::Agent::new(worker_id.clone(), "wild-cheetah-29".to_string());
    worker.agent_type = cas::types::AgentType::Worker;
    worker.role = cas::types::AgentRole::Worker;
    worker.heartbeat();
    agent_store.register(&worker).expect("register worker");

    // Create a regular (non-Epic) task and assign the live worker to it.
    let create_req = TaskCreateRequest {
        depth: None,
        title: "Live worker task — supervisor must not verify behind their back".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create should succeed"),
    ))
    .expect("task id")
    .to_string();

    let mut task = task_store.get(&id).expect("task exists");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = Some(worker_id.clone());
    task.pending_verification = true;
    task_store.update(&task).expect("update task");
    let recovery_dispatch = cas_store::create_verification_dispatch(
        &cas_dir,
        &id,
        &worker_id,
        "unavailable-original-owner",
        chrono::Utc::now() + chrono::Duration::minutes(10),
    )
    .expect("create dispatch owned by unavailable session");

    service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: id.clone(),
            status: "approved".to_string(),
            summary: "registered supervisor external verification".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(recovery_dispatch.id.clone()),
        }))
        .await
        .expect("registered supervisor verification should succeed");

    let verification_store = open_verification_store(&cas_dir).expect("verification store");
    let row = verification_store
        .get_latest_for_task(&id)
        .expect("verification lookup")
        .expect("supervisor verification row");
    assert_eq!(
        row.provenance,
        cas::types::VerificationProvenance::SupervisorDirect
    );
    assert_eq!(row.agent_id.as_deref(), Some(supervisor_id.as_str()));
    assert_eq!(row.issuer_agent_id.as_deref(), Some(supervisor_id.as_str()));
    assert!(row.capability_id.is_none());
    assert_eq!(
        cas_store::get_latest_verification_dispatch(&cas_dir, &id)
            .expect("dispatch lookup")
            .expect("dispatch")
            .state,
        cas::types::VerificationDispatchState::Resolved,
        "registered supervisor direct authority must recover an unavailable owner"
    );
    assert!(
        !task_store.get(&id).expect("task").pending_verification,
        "supervisor verdict must atomically clear only the named task transition"
    );
}

// =============================================================================
// cas-8aaf: Codex/Claude close-block message correctness
//
// Regression guard for the VERIFICATION_JAIL_BLOCKED guidance routing fix.
//
// When a factory worker hits the verification jail (legacy owner=worker config),
// the suggested action must use the correct MCP alias for the worker's harness:
//   - Claude workers: mcp__cas__coordination
//   - Codex workers:  mcp__cs__coordination
//
// Under default supervisor-owned review (owner=supervisor), Codex workers must
// NOT hit the jail at all — verification_required_for_task_type() returns false
// for Codex harnesses that don't support subagents.
// =============================================================================

/// Guard that installs factory-worker env vars for a Codex worker context.
/// Sets CAS_FACTORY_WORKER_CLI=codex in addition to the standard ROLE/MODE so
/// worker_harness_from_env() returns Codex and is_worker_without_subagents_from_env()
/// returns true. Snapshots and restores the prior value of each var on drop
/// (cas-7cc9) rather than blindly removing it, so a surrounding factory env
/// is left exactly as it was found.
struct CodexWorkerEnv {
    _env: ScopedFactoryEnv,
}

impl CodexWorkerEnv {
    fn enter() -> Self {
        Self {
            _env: ScopedFactoryEnv::apply(&[
                ("CAS_AGENT_ROLE", Some("worker")),
                ("CAS_FACTORY_MODE", Some("1")),
                ("CAS_FACTORY_WORKER_CLI", Some("codex")),
            ]),
        }
    }
}

// =============================================================================
// cas-7cc9 — env guards must snapshot/restore prior values, not blind-remove.
//
// Regression coverage for the R2 finding off cas-8aaf's headless review: the
// factory env guards (CodexWorkerEnv / FactoryWorkerEnv / ScopedSupervisorEnv /
// ScopedSupervisorCliEnv) used to unconditionally `remove_var` their vars on
// drop, clobbering any pre-existing factory env owned by the surrounding
// test/process. After the fix they snapshot the prior value and restore it (or
// remove only vars that were originally absent). These tests hold
// env_test_lock() for their whole body and do not call setup_cas(), so they
// exercise the guard against a deliberately non-empty starting environment.
// =============================================================================

/// CodexWorkerEnv must leave pre-existing factory env values exactly as it
/// found them: prior values are restored on drop, not removed. (AC1, AC3)
#[test]
fn test_codex_worker_env_restores_prior_factory_values_on_drop_cas_7cc9() {
    let _env_lock = env_test_lock();

    // Establish a non-empty prior environment that differs from what the
    // guard installs, so a blind remove-on-drop would be observable.
    // SAFETY: env_test_lock held for the entire test body.
    unsafe {
        std::env::set_var("CAS_AGENT_ROLE", "supervisor");
        std::env::set_var("CAS_FACTORY_MODE", "0");
        std::env::set_var("CAS_FACTORY_WORKER_CLI", "claude");
    }

    {
        let _env = CodexWorkerEnv::enter();
        // Inside the guard the Codex-worker values are active.
        assert_eq!(std::env::var("CAS_AGENT_ROLE").as_deref(), Ok("worker"));
        assert_eq!(std::env::var("CAS_FACTORY_MODE").as_deref(), Ok("1"));
        assert_eq!(
            std::env::var("CAS_FACTORY_WORKER_CLI").as_deref(),
            Ok("codex")
        );
    }

    // After drop the prior values are restored verbatim — NOT removed.
    assert_eq!(
        std::env::var("CAS_AGENT_ROLE").as_deref(),
        Ok("supervisor"),
        "prior CAS_AGENT_ROLE must survive the guard scope"
    );
    assert_eq!(
        std::env::var("CAS_FACTORY_MODE").as_deref(),
        Ok("0"),
        "prior CAS_FACTORY_MODE must survive the guard scope"
    );
    assert_eq!(
        std::env::var("CAS_FACTORY_WORKER_CLI").as_deref(),
        Ok("claude"),
        "prior CAS_FACTORY_WORKER_CLI must survive the guard scope"
    );

    // Clean up the values this test introduced so no sibling depends on them.
    // SAFETY: still holding env_test_lock.
    unsafe {
        std::env::remove_var("CAS_AGENT_ROLE");
        std::env::remove_var("CAS_FACTORY_MODE");
        std::env::remove_var("CAS_FACTORY_WORKER_CLI");
    }
}

/// CodexWorkerEnv must remove vars that were originally absent (so it doesn't
/// leak its own injected values), confirming the snapshot==None branch. (AC2, AC4)
#[test]
fn test_codex_worker_env_removes_originally_absent_vars_on_drop_cas_7cc9() {
    let _env_lock = env_test_lock();

    // Start from a clean slate: these vars are absent before the guard.
    // SAFETY: env_test_lock held for the entire test body.
    unsafe {
        std::env::remove_var("CAS_AGENT_ROLE");
        std::env::remove_var("CAS_FACTORY_MODE");
        std::env::remove_var("CAS_FACTORY_WORKER_CLI");
    }

    {
        let _env = CodexWorkerEnv::enter();
        assert_eq!(
            std::env::var("CAS_FACTORY_WORKER_CLI").as_deref(),
            Ok("codex")
        );
    }

    // Originally-absent vars must be removed again, leaving no pollution.
    assert!(
        std::env::var_os("CAS_AGENT_ROLE").is_none(),
        "CAS_AGENT_ROLE must be removed when it was originally absent"
    );
    assert!(
        std::env::var_os("CAS_FACTORY_MODE").is_none(),
        "CAS_FACTORY_MODE must be removed when it was originally absent"
    );
    assert!(
        std::env::var_os("CAS_FACTORY_WORKER_CLI").is_none(),
        "CAS_FACTORY_WORKER_CLI must be removed when it was originally absent"
    );
}

/// FactoryWorkerEnv (Claude-worker context) must clear a leaked
/// CAS_FACTORY_WORKER_CLI on enter so worker_harness_from_env() can't report
/// Codex, and must restore the leaked value on drop instead of omitting it
/// (cas-7cc9 / R2). (AC1, AC2)
#[test]
fn test_factory_worker_env_clears_and_restores_worker_cli_cas_7cc9() {
    let _env_lock = env_test_lock();

    // Simulate a `codex` CLI value leaked from a sibling Codex context.
    // SAFETY: env_test_lock held for the entire test body.
    unsafe {
        std::env::set_var("CAS_FACTORY_WORKER_CLI", "codex");
    }

    {
        let _env = FactoryWorkerEnv::enter();
        // A Claude-worker context must not observe a stale codex CLI.
        assert!(
            std::env::var_os("CAS_FACTORY_WORKER_CLI").is_none(),
            "FactoryWorkerEnv must clear a leaked CAS_FACTORY_WORKER_CLI on enter"
        );
        assert_eq!(std::env::var("CAS_AGENT_ROLE").as_deref(), Ok("worker"));
        assert_eq!(std::env::var("CAS_FACTORY_MODE").as_deref(), Ok("1"));
    }

    // The leaked prior value is restored on drop, not blindly removed.
    assert_eq!(
        std::env::var("CAS_FACTORY_WORKER_CLI").as_deref(),
        Ok("codex"),
        "FactoryWorkerEnv must restore the prior CAS_FACTORY_WORKER_CLI on drop"
    );

    // Clean up so no sibling inherits the simulated leak.
    // SAFETY: still holding env_test_lock.
    unsafe {
        std::env::remove_var("CAS_FACTORY_WORKER_CLI");
    }
}

/// cas-8aaf: a Codex factory worker under supervisor-owned review (the default)
/// must NOT hit VERIFICATION_JAIL_BLOCKED on close. The Codex harness does not
/// support subagents, so verification_required_for_task_type() returns false and
/// the jail short-circuits.
///
/// This pins the fix from pty.rs injecting CAS_FACTORY_WORKER_CLI=codex into
/// the `cs` MCP server env — without it worker_harness_from_env() defaults to
/// Claude, which DOES require verification, breaking every Codex worker close.
#[tokio::test]
async fn test_codex_worker_close_not_jailed_under_supervisor_owned_review_cas_8aaf() {
    let (_temp, core) = setup_cas();
    let _env_lock = env_test_lock();

    // No config.toml written => default code_review.owner = "supervisor" (cas-865b).
    let service = CasService::new(core, None);
    let _env = CodexWorkerEnv::enter();

    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "cas-8aaf: Codex close not jailed under supervisor-owned review",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id.clone(),
        }))))
        .await
        .expect("start");

    // Close. Must NOT return VERIFICATION_JAIL_BLOCKED — Codex workers don't
    // support subagents so verification is bypassed under supervisor_owned review.
    let result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id.clone(),
            "reason": "All acceptance criteria satisfied. No reviewable code changes.",
        }))))
        .await
        .expect("close must not error for Codex worker under supervisor-owned review");
    let text = extract_text(result);
    assert!(
        !text.contains("VERIFICATION_JAIL_BLOCKED"),
        "Codex worker under owner=supervisor must not hit verification jail; got: {text}"
    );
}

/// cas-8aaf: a Claude factory worker under legacy owner=worker config that hits
/// the verification jail must receive mcp__cas__coordination guidance (not
/// Task(subagent_type="task-verifier"), which is the non-factory-worker branch).
///
/// This pins the existing behavior and guards against the guidance regressing to
/// the non-factory branch. Complements the Codex variant below.
#[tokio::test]
async fn test_claude_worker_close_dispatch_recommends_cas_coordination_cas_8aaf() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Opt into legacy owner=worker so the jail fires for Claude workers.
    std::fs::write(
        cas_dir.join("config.toml"),
        "[code_review]\nowner = \"worker\"\n",
    )
    .expect("write legacy code_review config");

    let service = CasService::new(core, None);
    // Claude worker: CAS_FACTORY_WORKER_CLI not set => defaults to Claude harness.
    let _env = FactoryWorkerEnv::enter();

    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "cas-8aaf: Claude worker jail guidance uses mcp__cas__coordination",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id.clone(),
        }))))
        .await
        .expect("start");

    let result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id.clone(),
            "reason": "Done.",
        }))))
        .await
        .expect("close returns task-scoped verification guidance");
    let msg = extract_text(result);
    assert!(
        msg.contains("VERIFICATION REQUIRED") && msg.contains("vdispatch-"),
        "Claude worker close must create a task-scoped dispatch; got: {msg}"
    );
    // cas-778a + cas-8aaf: Claude factory workers must use mcp__cas__coordination.
    assert!(
        msg.contains("mcp__cas__coordination"),
        "Claude worker jail must recommend mcp__cas__coordination; got: {msg}"
    );
    // Must NOT instruct spawning task-verifier (workers can't do it) or use Codex alias.
    assert!(
        !msg.contains("Task(subagent_type=\"task-verifier\""),
        "Claude factory worker jail must not suggest Task() spawn; got: {msg}"
    );
    assert!(
        !msg.contains("mcp__cs__coordination"),
        "Claude factory worker jail must not suggest Codex alias; got: {msg}"
    );
}

/// cas-8aaf: a Codex factory worker under legacy owner=worker config must NOT
/// be jailed. Because Codex doesn't support subagents, verification_policy()
/// returns task_mode=Bypassed, so verification_required_for_task_type() returns
/// false. The check_pending_verification loop skips the task and the jail never
/// fires — even under owner=worker. This is correct: Codex workers cannot run
/// the task-verifier subagent, so jailing them would deadlock every close.
///
/// Pre-fix (CAS_FACTORY_WORKER_CLI absent → defaults to Claude → verification
/// required), Codex workers would hit VERIFICATION_JAIL_BLOCKED with the wrong
/// guidance. Post-fix, harness is detected correctly and the jail bypasses.
#[tokio::test]
async fn test_codex_worker_not_jailed_even_under_owner_worker_cas_8aaf() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Force legacy owner=worker to confirm Codex workers are still bypassed.
    std::fs::write(
        cas_dir.join("config.toml"),
        "[code_review]\nowner = \"worker\"\n",
    )
    .expect("write legacy code_review config");

    let service = CasService::new(core, None);
    // Codex worker env: CAS_FACTORY_WORKER_CLI=codex makes harness=Codex.
    let _env = CodexWorkerEnv::enter();

    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "cas-8aaf: Codex worker bypasses jail even under owner=worker",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id.clone(),
        }))))
        .await
        .expect("start");

    // Close must succeed: verification_required_for_task_type returns false for
    // Codex (no subagent support), so the jail check skips the task entirely.
    let result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id.clone(),
            "reason": "Done.",
        }))))
        .await
        .expect("Codex worker close must not be jailed even under owner=worker");
    let text = extract_text(result);
    assert!(
        !text.contains("VERIFICATION_JAIL_BLOCKED"),
        "Codex worker must bypass verification jail (harness has no subagent support); got: {text}"
    );
}

// =============================================================================
// cas-a3ca: verification jail must scope to the requested task
//
// Regression guard for the cross-task verification jail leakage that
// surfaced during the cas-3cb7 smoke test. Worker `safety-triage` completed
// and verified cas-cdee, then started cas-8236 before cas-cdee could close.
// The subsequent `task.close id=cas-cdee` was blocked with
// VERIFICATION_JAIL_BLOCKED naming cas-8236 (the in-progress, unverified
// task), not cas-cdee. The close gate was evaluating ALL agent leases, not
// just the one being closed.
//
// Fix: `check_pending_verification` now accepts `close_task_id: Option<&str>`.
// When Some(id), leases for tasks OTHER than id are skipped — only the
// requested task's own verification state can block its close.
// =============================================================================

/// cas-a3ca (positive path): close of verified task A must not be blocked by
/// unrelated in-progress task B held by the same agent.
///
/// Sequence: create+start+verify A, create+start B (no verification),
/// `task.close id=A` → must succeed.
///
/// Uses legacy owner=worker so the jail fires for task.close (under
/// owner=supervisor factory workers are fully exempt from close-time jail).
#[tokio::test]
async fn test_close_verified_task_not_blocked_by_unrelated_unverified_task_cas_a3ca() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Legacy owner=worker so the jail check fires for task.close
    std::fs::write(
        cas_dir.join("config.toml"),
        "[code_review]\nowner = \"worker\"\n",
    )
    .expect("write legacy code_review config");

    let service = CasService::new(core, None);
    // Claude factory worker — CAS_FACTORY_WORKER_CLI not set → defaults to
    // Claude harness (supports subagents → verification required for tasks).
    let _env = FactoryWorkerEnv::enter();

    // --- Task A: create, start, add approved verification ---
    let id_a = extract_task_id(&extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "create",
                "title": "cas-a3ca: task A — completed and verified",
                "priority": 2,
                "task_type": "task",
            }))))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();

    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id_a.clone(),
        }))))
        .await
        .expect("start A");

    // Add approved verification for A
    {
        let verification_store =
            open_verification_store(&cas_dir).expect("open verification store");
        let ver = Verification::approved(
            format!("ver-a3ca-a-{}", id_a),
            id_a.clone(),
            "verified by supervisor".to_string(),
        );
        verification_store
            .add(&ver)
            .expect("add verification for A");
    }

    // --- Task B: create, start — deliberately NOT verified ---
    let id_b = extract_task_id(&extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "create",
                "title": "cas-a3ca: task B — in progress, unverified",
                "priority": 2,
                "task_type": "task",
            }))))
            .await
            .expect("create B"),
    ))
    .expect("id B")
    .to_string();

    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id_b.clone(),
        }))))
        .await
        .expect("start B");

    // --- Close A: must succeed despite B being in-progress and unverified ---
    //
    // Pre-fix: `check_pending_verification` iterated all agent leases, found
    // task B (unverified), returned Some((B, title)) → jail blocked close of A
    // with "VERIFICATION_JAIL_BLOCKED" naming B, not A.
    //
    // Post-fix: jail passes `close_task_id = Some(A)`, so only A's lease is
    // evaluated. A has an approved verification → no block.
    let result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id_a.clone(),
            "reason": "cas-a3ca: verified task A close must not be blocked by unverified task B",
        }))))
        .await
        .expect("close A must succeed when A is verified, even if B is unverified");

    let text = extract_text(result);
    assert!(
        !text.contains("VERIFICATION_JAIL_BLOCKED"),
        "close of verified task A must not be blocked by unverified task B; got: {text}"
    );
    // Confirm A is actually closed (not just a soft pass)
    assert!(
        text.to_lowercase().contains("closed") || text.to_lowercase().contains("success"),
        "expected A to be closed; got: {text}"
    );
}

/// cas-a3ca (negative path / jail still fires for the requested task): closing
/// task A when A ITSELF has no verification must still be blocked.
///
/// This guards against a regression where the task-scoping change accidentally
/// disabled the jail for the task being closed.
#[tokio::test]
async fn test_close_unverified_task_still_blocked_by_own_missing_verification_cas_a3ca() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Legacy owner=worker so the jail fires for task.close
    std::fs::write(
        cas_dir.join("config.toml"),
        "[code_review]\nowner = \"worker\"\n",
    )
    .expect("write legacy code_review config");

    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    // Task A: in progress, NO verification
    let id_a = extract_task_id(&extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "create",
                "title": "cas-a3ca: task A — unverified, must be blocked at close",
                "priority": 2,
                "task_type": "task",
            }))))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();

    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id_a.clone(),
        }))))
        .await
        .expect("start A");

    // Attempt to close A without any verification — must be blocked
    let result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id_a.clone(),
            "reason": "unverified — should be blocked",
        }))))
        .await
        .expect("close of unverified A returns task-scoped guidance");

    let msg = extract_text(result);
    assert!(
        msg.contains("VERIFICATION REQUIRED") && msg.contains("vdispatch-"),
        "close of unverified task A must be exact-task gated; got: {msg}"
    );
    // Error must name task A, not some other task
    assert!(
        msg.contains(&id_a),
        "task-scoped close gate must name task A ({id_a}); got: {msg}"
    );
}

/// cas-9fd4 (GH #341): the direct close gate must tell a supervisor exactly
/// how to resolve a dispatch whose bound worker/verifier is unavailable.
#[tokio::test]
async fn test_pending_dispatch_close_gate_prints_supervisor_recovery_cas_9fd4() {
    let (_temp, service) = setup_cas();
    let _env_lock = env_test_lock();

    let created = service
        .cas_task_create(Parameters(simple_task_req(
            "cas-9fd4: direct pending-dispatch recovery guidance",
        )))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("start");

    let text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id.clone(),
                reason: Some("Ready for supervisor verification".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("direct close returns verification guidance"),
    );
    assert!(
        text.contains("VERIFICATION REQUIRED") && text.contains("vdispatch-"),
        "direct close must create and name the exact dispatch: {text}"
    );
    assert!(
        text.contains(&format!(
            "mcp__cas__verification action=add task_id={id} dispatch_id=vdispatch-"
        )) && text.contains("status=approved"),
        "gate must print the registered-supervisor recovery call: {text}"
    );
}

/// cas-a3ca (replay sequence): the exact cas-cdee/cas-8236 scenario.
///
/// Worker has task A (verified, merge-ready) and starts task B while A's close
/// is delayed. `task.close id=A` must succeed — the fact that B is in-progress
/// and unverified is irrelevant to A's close.
#[tokio::test]
async fn test_cdee_cas8236_sequence_close_verified_task_while_second_task_in_progress_cas_a3ca() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    std::fs::write(
        cas_dir.join("config.toml"),
        "[code_review]\nowner = \"worker\"\n",
    )
    .expect("write owner=worker config");

    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    // cas-cdee analogue: completed and supervisor-verified
    let id_cdee = extract_task_id(&extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "create",
                "title": "cas-a3ca replay: cas-cdee analogue — verified",
                "priority": 2,
                "task_type": "task",
            }))))
            .await
            .expect("create cdee"),
    ))
    .expect("id cdee")
    .to_string();

    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id_cdee.clone(),
        }))))
        .await
        .expect("start cdee");

    {
        let verification_store =
            open_verification_store(&cas_dir).expect("open verification store");
        let ver = Verification::approved(
            format!("ver-a3ca-cdee-{}", id_cdee),
            id_cdee.clone(),
            "supervisor-verified, merge landed on main".to_string(),
        );
        verification_store
            .add(&ver)
            .expect("add verification for cdee analogue");
    }

    // cas-8236 analogue: worker started this BEFORE closing cdee
    let id_8236 = extract_task_id(&extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "create",
                "title": "cas-a3ca replay: cas-8236 analogue — in progress, unverified",
                "priority": 2,
                "task_type": "task",
            }))))
            .await
            .expect("create 8236"),
    ))
    .expect("id 8236")
    .to_string();

    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id_8236.clone(),
        }))))
        .await
        .expect("start 8236");

    // Now retry the close of cdee — this is where the bug was.
    // Pre-fix: blocked with VERIFICATION_JAIL_BLOCKED naming cas-8236.
    // Post-fix: close of cdee succeeds because cdee has approved verification.
    let result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id_cdee.clone(),
            "reason": "cas-cdee replay: verified and merged, close must not be blocked by 8236",
        }))))
        .await
        .expect("close cdee must succeed — verified task must not be blocked by unverified 8236");

    let text = extract_text(result);
    assert!(
        !text.contains("VERIFICATION_JAIL_BLOCKED"),
        "close of verified cdee analogue must not be blocked by unverified 8236 analogue; got: {text}"
    );
    // Must not name the wrong task in any error
    assert!(
        !text.contains(&id_8236),
        "close error must not reference the unrelated task {id_8236}; got: {text}"
    );
}

// =============================================================================
// cas-1b80: Codex VERIFICATION_JAIL_BLOCKED guidance must use mcp__cs__coordination
//
// Regression guard ensuring that when a Codex factory worker does hit the
// VERIFICATION_JAIL_BLOCKED path, the emitted guidance uses the Codex MCP
// alias mcp__cs__coordination — not mcp__cas__coordination (the Claude alias)
// and not Task(subagent_type=...) (only valid for non-worker callers).
//
// The path that fires the jail for a Codex worker:
//   - Legacy owner=worker config (jail fires at task.close)
//   - Codex worker (CAS_FACTORY_WORKER_CLI=codex → worker_harness=Codex)
//   - Claude supervisor (default; CAS_FACTORY_SUPERVISOR_CLI absent)
//   - Epic task type: verification_policy(Claude, Codex).epic_required() = true
//     because epic_mode depends on the SUPERVISOR's subagent capability, and
//     Claude supports subagents. task_required() is false for Codex workers
//     (non-epic tasks bypass the jail), but epic_required() is true.
//   - No approved verification → check_pending_verification returns Some
//     → VERIFICATION_JAIL_BLOCKED fires with worker_coordination_tool()
//     → CAS_FACTORY_WORKER_CLI=codex → returns mcp__cs__coordination
//
// Without the cas-8aaf fix (CAS_FACTORY_WORKER_CLI not injected into the Codex
// cs MCP server env), worker_harness_from_env() would return Claude, making
// worker_coordination_tool() return mcp__cas__coordination — an alias that
// Codex workers cannot execute.
// =============================================================================

/// cas-1b80: a Codex factory worker closing an Epic task under legacy
/// owner=worker config must receive VERIFICATION_JAIL_BLOCKED guidance that
/// uses mcp__cs__coordination (the executable Codex alias).
///
/// This is the one task type where a Codex worker can hit the jail:
/// verification_policy(Claude, Codex).epic_required() returns true because
/// the epic_mode is determined by the supervisor's subagent capability, not
/// the worker's. A Claude supervisor (the default) supports subagents so
/// epics require supervisor verification — and the Codex worker must receive
/// the correct alias to message the supervisor.
#[tokio::test]
async fn test_codex_worker_epic_close_jail_recommends_cs_coordination_cas_1b80() {
    let (temp, core) = setup_cas();
    // setup_cas() clears CAS_FACTORY_SUPERVISOR_CLI (among other vars), so
    // supervisor_harness_from_env() defaults to Claude — the prerequisite for
    // verification_policy(Claude, Codex).epic_required() returning true.
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Opt into legacy owner=worker so the verification jail fires at task.close.
    // Under owner=supervisor (default), factory workers are exempt (cas-8edb).
    std::fs::write(
        cas_dir.join("config.toml"),
        "[code_review]\nowner = \"worker\"\n",
    )
    .expect("write legacy code_review config");

    let service = CasService::new(core, None);
    // Codex worker: CAS_FACTORY_WORKER_CLI=codex makes worker_harness_from_env()
    // return Codex, so worker_coordination_tool() returns mcp__cs__coordination.
    let _env = CodexWorkerEnv::enter();

    // Create an Epic task. For Codex workers under a Claude supervisor,
    // verification_policy(Claude, Codex).epic_required() returns true
    // (epic_mode = Required because the supervisor/Claude supports subagents).
    // This is the only task type where a Codex worker can hit the jail.
    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "cas-1b80: Codex worker epic close must use cs coordination alias",
            "priority": 2,
            "task_type": "epic",
        }))))
        .await
        .expect("create epic");
    let id = extract_task_id(&extract_text(created))
        .expect("epic id")
        .to_string();

    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id.clone(),
        }))))
        .await
        .expect("start epic");

    // Attempt to close without verification. Because task_type=Epic and the
    // supervisor is Claude, verification is required even for Codex workers.
    // The jail must fire and the guidance must use the Codex MCP alias.
    let result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id.clone(),
            "reason": "Done.",
        }))))
        .await
        .expect("Codex worker epic close returns task-scoped verification guidance");

    let msg = extract_text(result);

    assert!(
        msg.contains("VERIFICATION REQUIRED") && msg.contains("vdispatch-"),
        "Codex worker epic close must create a task-scoped dispatch; got: {msg}"
    );
    // cas-1b80: Codex factory workers must receive the Codex MCP alias.
    assert!(
        msg.contains("mcp__cs__coordination"),
        "Codex worker jail must recommend mcp__cs__coordination; got: {msg}"
    );
    // Must NOT use the Claude alias — that is not executable by a Codex worker.
    assert!(
        !msg.contains("mcp__cas__coordination"),
        "Codex worker jail must not suggest Claude alias mcp__cas__coordination; got: {msg}"
    );
    // Must NOT suggest spawning a task-verifier subagent — factory workers
    // cannot do that; the jail must route to the supervisor instead.
    assert!(
        !msg.contains("Task(subagent_type=\"task-verifier\""),
        "Codex worker jail must not suggest Task() spawn; got: {msg}"
    );
}

// =============================================================================
// cas-7998: harness-aware supervisor verification alias + close-reason quoting
//
// Two guidance paths in close_ops still hardcoded `mcp__cas__verification`,
// handing a Codex supervisor an alias they cannot call:
//   1. the `supervisor_is_assignee` self-verify branch in the VERIFICATION
//      REQUIRED gate, and
//   2. the VERIFICATION TIMED OUT auto-escalation arm.
// Both must resolve via supervisor_verification_tool() (mcp__cs__verification
// for a Codex supervisor). Separately, the factory-worker jail message embeds
// the free-text close reason inside a quoted `message="..."` coordination
// command; a reason containing a quote/newline must be escaped (covered by the
// escape_close_reason_for_quoted_command unit tests in close_ops.rs).
// =============================================================================

/// RAII guard that pins CAS_FACTORY_SUPERVISOR_CLI for the duration of a test
/// so supervisor_verification_tool() resolves the Codex alias, then restores
/// the prior value on drop (cas-7cc9: snapshot/restore via ScopedFactoryEnv
/// instead of an unconditional remove). setup_cas() clears this var, so callers
/// that run after it see the same baseline as before.
struct ScopedSupervisorCliEnv {
    _env: ScopedFactoryEnv,
}

impl ScopedSupervisorCliEnv {
    fn set(cli: &str) -> Self {
        // SAFETY: env-sensitive tests serialize via env_test_lock(); see setup_cas().
        Self {
            _env: ScopedFactoryEnv::apply(&[("CAS_FACTORY_SUPERVISOR_CLI", Some(cli))]),
        }
    }
}

/// Drive the `supervisor_is_assignee` self-verify branch and assert the direct
/// verification alias tracks the supervisor harness. Returns the rendered
/// guidance so each harness variant can assert on it.
async fn supervisor_self_assignee_close_guidance(supervisor_cli: Option<&str>) -> String {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");

    // get_agent_id() returns this session id in the test harness (setup_cas()).
    let sup_id = format!("test-session-{}", std::process::id());
    // Refresh the supervisor agent's heartbeat so the assignee-inactive bypass
    // does NOT fire — we need to reach the self-assignee jail branch, not the
    // orphan skip-verification hatch. (setup_cas() registers this agent Active,
    // but a fresh heartbeat keeps the test robust against clock skew.)
    agent_store
        .heartbeat(&sup_id)
        .expect("refresh supervisor heartbeat");

    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "cas-7998: supervisor self-assigned task".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();

    // Assign the task to the supervisor themselves and mark it in-progress.
    let mut task = task_store.get(&id).expect("task exists");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = Some(sup_id.clone());
    task_store.update(&task).expect("update task");

    let _sup = ScopedSupervisorEnv::new();
    let _cli = supervisor_cli.map(ScopedSupervisorCliEnv::set);

    let response = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id.clone(),
                reason: Some("Self-implemented; ready to self-verify".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close returns a result"),
    );
    assert!(
        response.contains("VERIFICATION REQUIRED"),
        "supervisor self-assignee must hit the verification gate: {response}"
    );
    assert!(
        response.contains("You implemented this task yourself"),
        "must take the supervisor-self-assignee branch: {response}"
    );
    response
}

/// cas-7998 (AC3): a Codex supervisor closing their own task must receive the
/// Codex verification alias in the self-verify guidance.
#[tokio::test]
async fn test_supervisor_self_assignee_close_uses_codex_verification_alias_cas_7998() {
    let response = supervisor_self_assignee_close_guidance(Some("codex")).await;
    assert!(
        response.contains("mcp__cs__verification"),
        "Codex supervisor self-verify guidance must use mcp__cs__verification: {response}"
    );
    assert!(
        !response.contains("mcp__cas__verification"),
        "Codex supervisor must not be handed the Claude verification alias: {response}"
    );
}

/// cas-7998 (AC3): a Claude supervisor (default) still receives the Claude
/// verification alias — the harness-aware change must not regress the common
/// path.
#[tokio::test]
async fn test_supervisor_self_assignee_close_uses_claude_verification_alias_cas_7998() {
    let response = supervisor_self_assignee_close_guidance(None).await;
    assert!(
        response.contains("mcp__cas__verification"),
        "Claude supervisor self-verify guidance must use mcp__cas__verification: {response}"
    );
    assert!(
        !response.contains("mcp__cs__verification"),
        "Claude supervisor must not be handed the Codex verification alias: {response}"
    );
}

/// cas-7998 (AC2): the VERIFICATION TIMED OUT auto-escalation arm must use the
/// supervisor harness's verification alias for the "record verdict directly"
/// fallback. A Codex supervisor must see mcp__cs__verification, not the Claude
/// alias they cannot call.
#[tokio::test]
async fn test_timeout_escalation_uses_codex_supervisor_verification_alias_cas_7998() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "cas-7998: timeout escalation alias".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("start");

    // First close arms pending_verification + writes the dispatch-request row.
    let _ = service
        .cas_task_close(Parameters(TaskCloseRequest {
            stranded_branch_override: None,
            id: id.clone(),
            reason: Some("Completed".to_string()),
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .expect("first close returns a result");

    // Expire the authoritative typed deadline so the retry auto-escalates.
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("db");
    conn.execute(
        "UPDATE verification_dispatches SET deadline_at = ?2 WHERE task_id = ?1",
        rusqlite::params![
            id,
            (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339()
        ],
    )
    .expect("expire typed dispatch");

    // Codex supervisor harness drives the alias selection in the timeout arm.
    let _cli = ScopedSupervisorCliEnv::set("codex");

    let text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id.clone(),
                reason: Some("Completed".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("second close returns a result"),
    );
    assert!(
        text.contains("VERIFICATION TIMED OUT"),
        "retry after timeout must report escalation: {text}"
    );
    assert!(
        text.contains("mcp__cs__verification"),
        "Codex supervisor timeout guidance must use mcp__cs__verification: {text}"
    );
    assert!(
        !text.contains("mcp__cas__verification"),
        "Codex supervisor timeout guidance must not use the Claude alias: {text}"
    );
}

/// cas-062d: successful close must durable-push `task_closed` to the owning
/// supervisor queue (session-isolated). Covers the close path that lives in
/// verification_flow's domain (supervisor orphan bypass → Closed).
#[tokio::test]
async fn test_062d_close_lifecycle_push_to_owning_supervisor() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Register a factory-session supervisor that owns lifecycle events.
    let session = "sess-062d-vf";
    let agent_store = open_agent_store(&cas_dir).expect("agent store");
    let mut sup = cas::types::Agent::new("sup-062d-vf".to_string(), "sup-062d-vf".to_string());
    sup.role = AgentRole::Supervisor;
    sup.factory_session = Some(session.to_string());
    agent_store.register(&sup).expect("register supervisor");

    // SAFETY: hold env_test_lock for the factory session + supervisor role.
    let _guard = ScopedFactoryEnv::apply(&[
        ("CAS_FACTORY_SESSION", Some(session)),
        ("CAS_AGENT_ROLE", Some("supervisor")),
    ]);

    let task_store = open_task_store(&cas_dir).unwrap();
    let create_text = extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: "062d close lifecycle".to_string(),
                description: None,
                priority: 2,
                task_type: "task".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create"),
    );
    let id = extract_task_id(&create_text).expect("task id").to_string();

    // Orphan InProgress so supervisor bypass skips verification.
    let mut task = task_store.get(&id).expect("task");
    task.status = TaskStatus::InProgress;
    task.assignee = None;
    task_store.update(&task).expect("orphan task");

    let close_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id.clone(),
                reason: Some("062d close proof".to_string()),
                supervisor_override: Some(true),
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close"),
    );
    assert!(
        close_text.contains("Closed") || close_text.contains(&id),
        "close response: {close_text}"
    );
    assert_eq!(
        task_store.get(&id).unwrap().status,
        TaskStatus::Closed,
        "task must be Closed after successful close"
    );

    let queue = cas::store::open_supervisor_queue_store(&cas_dir).expect("queue");
    let pending = queue.peek("sup-062d-vf", 20).expect("peek");
    assert!(
        pending.iter().any(|n| {
            n.event_type == "task_lifecycle"
                && n.payload.contains("task_closed")
                && n.payload.contains(&id)
        }),
        "close must durable-push task_closed to owning supervisor. pending={pending:?}"
    );
}

/// cas-60393 (G-M1/X-M1 deadlock): a task already parked `AwaitingMerge` and
/// assigned to the caller must be able to re-close once its commit is
/// actually merged, even though the worker's agent record carries a
/// `halt_task_work` flag armed by an **earlier, unrelated** urgent stop.
/// Before the fix this call is rejected with `WORK HALTED` forever, because
/// starting an `AwaitingMerge` task (the only thing that clears halt) is
/// illegal — the exact deadlock this task exists to break.
#[tokio::test]
async fn test_60393_owned_awaiting_merge_recloses_despite_preexisting_halt() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent.role = AgentRole::Worker;
        agent_store.update(&agent).expect("mark test agent worker");
    }

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("write config");

    let repo = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "epic/cas-60393"]);
    git(&["checkout", "-q", "-b", "factory/test-agent"]);
    std::fs::write(repo.join("worker.txt"), "worker\n").unwrap();
    git(&["add", "worker.txt"]);
    git(&["commit", "-q", "-m", "worker change"]);

    let task_store = open_task_store(&cas_dir).expect("open task store");

    let epic_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: "Merge epic".to_string(),
                description: None,
                priority: 2,
                task_type: "epic".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create epic"),
    ))
    .expect("epic id")
    .to_string();
    {
        let mut epic = task_store.get(&epic_id).expect("epic exists");
        epic.branch = Some("epic/cas-60393".to_string());
        task_store.update(&epic).expect("update epic branch");
    }

    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id.clone()),
                ..simple_task_req("Task A")
            }))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("start A");
    {
        let mut task_a = task_store.get(&id_a).expect("A exists after start");
        task_a.assignee = Some("test-agent".to_string());
        task_store.update(&task_a).expect("set A assignee");
    }

    // First close: no merge yet → parks AwaitingMerge (MERGE REQUIRED).
    let close_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("ready for merge".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close A returns"),
    );
    assert!(
        close_text.contains("MERGE REQUIRED"),
        "close must reject on stranded factory branch: {close_text}"
    );
    assert_eq!(
        task_store.get(&id_a).expect("A exists").status,
        TaskStatus::AwaitingMerge
    );

    // Simulate the deadlock precondition: an EARLIER, unrelated urgent stop
    // armed halt_task_work on this worker (not the merge-done hand-off —
    // cas-126b already covers that path; this is a halt that predates it).
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent
            .metadata
            .insert("halt_task_work".to_string(), "1".to_string());
        agent_store.update(&agent).expect("arm unrelated halt");
    }

    // Now the supervisor actually merges the branch.
    git(&["checkout", "-q", "epic/cas-60393"]);
    git(&["merge", "--no-ff", "-q", "factory/test-agent"]);
    git(&["checkout", "-q", "factory/test-agent"]);
    let verification_store = open_verification_store(&cas_dir).expect("open verification store");
    verification_store
        .add(&Verification::approved(
            "ver-cas-60393".to_string(),
            id_a.clone(),
            "Simulated approval after supervisor merge".to_string(),
        ))
        .expect("record verification approval");

    // Re-close must succeed DESPITE the pre-existing halt: this is the
    // caller's own AwaitingMerge task and the merge-integrity gate now says
    // Proceed.
    let close_after_merge = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("merged and ready to close".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close A after merge returns"),
    );
    assert!(
        !close_after_merge.contains("WORK HALTED"),
        "a pre-existing unrelated halt must not deadlock re-close of the caller's \
         own merged AwaitingMerge task: {close_after_merge}"
    );
    assert!(
        close_after_merge.contains("Closed task:"),
        "awaiting_merge task must become closeable after merge guard passes \
         even under a pre-existing halt: {close_after_merge}"
    );
    assert_eq!(
        task_store.get(&id_a).expect("A exists").status,
        TaskStatus::Closed
    );
}

/// cas-60393: the halt exemption is narrow — it never bypasses the
/// merge-integrity gate. An `AwaitingMerge` task whose branch has NOT
/// actually been merged yet must still bounce `MERGE REQUIRED`, even though
/// the halt check itself was skipped for this owned task.
#[tokio::test]
async fn test_60393_unmerged_awaiting_merge_still_bounces_merge_required_under_halt() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent.role = AgentRole::Worker;
        agent_store.update(&agent).expect("mark test agent worker");
    }

    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("write config");

    let repo = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "epic/cas-60393b"]);
    git(&["checkout", "-q", "-b", "factory/test-agent"]);
    std::fs::write(repo.join("worker.txt"), "worker\n").unwrap();
    git(&["add", "worker.txt"]);
    git(&["commit", "-q", "-m", "worker change"]);

    let task_store = open_task_store(&cas_dir).expect("open task store");

    let epic_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                depth: None,
                title: "Merge epic".to_string(),
                description: None,
                priority: 2,
                task_type: "epic".to_string(),
                labels: None,
                notes: None,
                blocked_by: None,
                design: None,
                acceptance_criteria: None,
                external_ref: None,
                assignee: None,
                demo_statement: None,
                execution_note: None,
                epic: None,
            }))
            .await
            .expect("create epic"),
    ))
    .expect("epic id")
    .to_string();
    {
        let mut epic = task_store.get(&epic_id).expect("epic exists");
        epic.branch = Some("epic/cas-60393b".to_string());
        task_store.update(&epic).expect("update epic branch");
    }

    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(TaskCreateRequest {
                epic: Some(epic_id.clone()),
                ..simple_task_req("Task A")
            }))
            .await
            .expect("create A"),
    ))
    .expect("id A")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_a.clone() }))
        .await
        .expect("start A");
    {
        let mut task_a = task_store.get(&id_a).expect("A exists after start");
        task_a.assignee = Some("test-agent".to_string());
        task_store.update(&task_a).expect("set A assignee");
    }

    // Park AwaitingMerge (no merge yet).
    let close_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("ready for merge".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("close A returns"),
    );
    assert!(close_text.contains("MERGE REQUIRED"));

    // Arm an unrelated pre-existing halt (same as the happy-path test) but do
    // NOT merge the branch this time.
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent
            .metadata
            .insert("halt_task_work".to_string(), "1".to_string());
        agent_store.update(&agent).expect("arm unrelated halt");
    }

    let retry_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                stranded_branch_override: None,
                id: id_a.clone(),
                reason: Some("retry before merge".to_string()),
                supervisor_override: None,
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            }))
            .await
            .expect("retry close A returns"),
    );
    assert!(
        retry_text.contains("MERGE REQUIRED"),
        "unmerged AwaitingMerge must still bounce MERGE REQUIRED, halt-exempt \
         or not: {retry_text}"
    );
    assert!(
        !retry_text.contains("Closed task:"),
        "the halt exemption must never manufacture a false close success: {retry_text}"
    );
    assert_eq!(
        task_store.get(&id_a).expect("A exists").status,
        TaskStatus::AwaitingMerge,
        "task must remain parked, not falsely closed"
    );
}

/// cas-60393 → superseded by cas-3894 for this exact scenario.
///
/// This test originally asserted that a halted worker closing its OWN,
/// already-approved, InProgress task ("Task B", assignee = "test-agent",
/// the caller) was still refused with `WORK HALTED`. That was cas-60393's
/// deliberate scope boundary at the time: only `AwaitingMerge` was exempt.
///
/// cas-3894 supersedes that boundary: two recorded production deadlocks
/// showed a worker holding finished, gate-green `InProgress` work halted by
/// an entirely unrelated, informational urgent (a checkpoint nudge, a task
/// briefing — neither was a redirect about this task), with no reachable
/// escape. `halt_exempt_for_owned_task` now exempts the caller's own
/// `InProgress` task exactly as it already did for `AwaitingMerge`, so this
/// close must now SUCCEED. The true remaining safety boundary — a halted
/// worker still cannot touch a task it does not own — is covered by
/// `test_3894_halted_worker_still_blocked_closing_unowned_task` above.
#[tokio::test]
async fn test_3894_halt_no_longer_blocks_close_of_own_inprogress_task() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent.role = AgentRole::Worker;
        agent_store.update(&agent).expect("mark test agent worker");
    }

    let task_store = open_task_store(&cas_dir).expect("open task store");
    let verification_store = open_verification_store(&cas_dir).expect("open verification store");

    let id_b = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(simple_task_req("Task B")))
            .await
            .expect("create B"),
    ))
    .expect("id B")
    .to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id_b.clone() }))
        .await
        .expect("start B");
    {
        let mut task_b = task_store.get(&id_b).expect("B exists after start");
        task_b.assignee = Some("test-agent".to_string());
        task_store.update(&task_b).expect("set B assignee");
    }
    // Give B an approved verification so the ONLY thing standing between it
    // and a successful close is the halt flag under test.
    verification_store
        .add(&Verification::approved(
            "ver-cas-60393-b".to_string(),
            id_b.clone(),
            "pre-approved for isolation".to_string(),
        ))
        .expect("record verification approval");

    // Arm halt (unrelated urgent stop) on the worker.
    {
        let mut agent = agent_store
            .list(None)
            .expect("list agents")
            .into_iter()
            .find(|agent| agent.name == "test-agent")
            .expect("test agent exists");
        agent
            .metadata
            .insert("halt_task_work".to_string(), "1".to_string());
        agent_store.update(&agent).expect("arm halt");
    }

    // cas-3894: the caller's own InProgress task is now halt-exempt, so this
    // must succeed instead of erroring — the inverse of the old assertion.
    let close_result = service
        .cas_task_close(Parameters(TaskCloseRequest {
            stranded_branch_override: None,
            id: id_b.clone(),
            reason: Some("done".to_string()),
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .expect("halted worker must be able to close its OWN InProgress task (cas-3894)");
    let text = extract_text(close_result);
    assert!(
        text.contains("Closed task:"),
        "expected a successful close, got: {text}"
    );
    assert_eq!(
        task_store.get(&id_b).expect("B exists").status,
        TaskStatus::Closed,
        "the exempt close must actually close the task"
    );
}

/// cas-da92: embedded absolute paths and separator-obfuscated secrets must be
/// rejected at the verifier public boundary, so they can reach neither durable
/// verifier evidence (direct write + update/close persistence) nor any JSON /
/// diagnostic projection. Portable identifiers must still survive untouched.
#[tokio::test]
async fn test_verifier_embedded_paths_and_obfuscated_secrets_never_persist_or_project() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    std::fs::write(
        cas_dir.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("enable verification");

    let created = service
        .cas_task_create(Parameters(simple_task_req("Embedded verifier evidence")))
        .await
        .expect("create task");
    let task_id = extract_task_id(&extract_text(created))
        .expect("task id")
        .to_string();
    service
        .cas_task_start(Parameters(IdRequest {
            id: task_id.clone(),
        }))
        .await
        .expect("start task");

    let embedded_path = "/home/operator/private-proof.json";
    let embedded_drive = r"C:\Users\operator\private-proof.json";
    let embedded_unc = r"\\build-host\proofs\private-proof.json";
    let embedded_file_url = "file:///etc/shadow";
    let obfuscated_bearer = "Bearer\tverifier-secret-material";
    let obfuscated_key = "token = verifier-secret-material";
    let obfuscated_akia = "at=AKIAIOSFODNN7EXAMPLE";

    let verification_store = open_verification_store(&cas_dir).expect("verification store");
    let mut verdict = Verification::approved(
        "ver-embedded-boundary".to_string(),
        task_id.clone(),
        format!("verified; evidence at={embedded_path}"),
    );
    verdict.issues = vec![cas::types::VerificationIssue::blocking(
        format!("[proof]({embedded_path})"),
        Some(12),
        "security".to_string(),
        obfuscated_bearer.to_string(),
        obfuscated_key.to_string(),
        Some(format!("share={embedded_unc}")),
    )];
    verdict.files_reviewed = vec![
        format!("evidence={embedded_file_url}"),
        format!("at={embedded_drive}"),
        obfuscated_akia.to_string(),
        "src/lib.rs".to_string(),
    ];
    add_exact_supervisor_fixture_verdict(&cas_dir, verdict, None);

    // Durable persistence: the raw SQLite rows must already be redacted, so no
    // later projection is load-bearing for containment.
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("db");
    let (stored_summary, stored_files): (String, String) = conn
        .query_row(
            "SELECT summary, files_reviewed FROM verifications WHERE id = ?1",
            rusqlite::params!["ver-embedded-boundary"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("stored verification row");
    let (stored_file, stored_code, stored_problem, stored_suggestion): (
        String,
        String,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT file, code, problem, suggestion FROM verification_issues
             WHERE verification_id = ?1",
            rusqlite::params!["ver-embedded-boundary"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("stored issue row");
    drop(conn);

    assert_eq!(stored_summary, "[REDACTED_PATH]");
    assert_eq!(stored_file, "[REDACTED_PATH]");
    assert_eq!(stored_code, "[REDACTED_SECRET]");
    assert_eq!(stored_problem, "[REDACTED_SECRET]");
    assert_eq!(stored_suggestion, "[REDACTED_PATH]");
    assert!(
        stored_files.contains("src/lib.rs"),
        "portable identifier must survive durable persistence: {stored_files}"
    );

    let row = verification_store
        .get_latest_for_task(&task_id)
        .expect("lookup")
        .expect("verification row");
    assert_eq!(
        row.files_reviewed,
        vec![
            "[REDACTED_PATH]".to_string(),
            "[REDACTED_PATH]".to_string(),
            "[REDACTED_SECRET]".to_string(),
            "src/lib.rs".to_string(),
        ]
    );

    // Update/close persistence: the sanitized verdict still authorizes close,
    // and the close projection cannot echo the rejected content either.
    let close_text = extract_text(
        service
            .cas_task_update(Parameters(task_status_update(
                &task_id,
                Some("closed"),
                None,
            )))
            .await
            .expect("sanitized verdict authorizes close"),
    );
    assert_eq!(
        open_task_store(&cas_dir)
            .expect("task store")
            .get(&task_id)
            .expect("task")
            .status,
        TaskStatus::Closed
    );

    // JSON + diagnostic projections.
    let row_payload = serde_json::to_string(&row).expect("serialize verification row");
    let event_payload = serde_json::to_string(
        &open_event_store(&cas_dir)
            .expect("event store")
            .list_recent(50)
            .expect("events"),
    )
    .expect("serialize events");
    let show_text = extract_text(
        service
            .cas_verification_show(Parameters(VerificationShowRequest {
                id: "ver-embedded-boundary".to_string(),
            }))
            .await
            .expect("show"),
    );
    let list_text = extract_text(
        service
            .cas_verification_list(Parameters(VerificationListRequest {
                task_id: task_id.clone(),
                limit: Some(10),
            }))
            .await
            .expect("list"),
    );
    let latest_text = extract_text(
        service
            .cas_verification_latest(Parameters(VerificationListRequest {
                task_id: task_id.clone(),
                limit: Some(1),
            }))
            .await
            .expect("latest"),
    );

    for (surface, payload) in [
        ("stored_summary", stored_summary),
        ("stored_files", stored_files),
        ("close", close_text),
        ("row_json", row_payload),
        ("events", event_payload),
        ("show", show_text.clone()),
        ("list", list_text),
        ("latest", latest_text),
    ] {
        for unsafe_value in [
            embedded_path,
            embedded_drive,
            embedded_unc,
            embedded_file_url,
            "/home/operator",
            "/etc/shadow",
            r"C:\Users\operator",
            r"\\build-host",
            "verifier-secret-material",
            "AKIAIOSFODNN7EXAMPLE",
        ] {
            assert!(
                !payload.contains(unsafe_value),
                "{surface} leaked embedded verifier content: {unsafe_value:?}"
            );
        }
    }

    assert!(
        show_text.contains("[REDACTED_PATH]")
            && show_text.contains("[REDACTED_SECRET]")
            && show_text.contains("src/lib.rs"),
        "show must render redaction markers while keeping portable identifiers: {show_text}"
    );
}
