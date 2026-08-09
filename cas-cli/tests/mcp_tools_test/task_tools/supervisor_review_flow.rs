/// Integration tests for the cas-b51a supervisor-owned review pipeline.
///
/// These tests verify the supervisor-owned review pipeline (Stage 2, cas-865b):
/// - AC1: CodeReviewConfig reads from cas config, defaults to "supervisor"
/// - AC2: owner=supervisor mode → PendingSupervisorReview transition
/// - AC3: owner=worker mode (explicit opt-out) → existing behavior unchanged
/// - AC4: supervisor verify path works on PendingSupervisorReview tasks
/// - AC5: all 5 named test functions listed in spec
use crate::support::*;
use cas::config::{CodeReviewConfig, Config};
use cas::mcp::CasService;
use cas::mcp::tools::VerificationAddRequest;
use cas::store::{open_agent_store, open_task_store, open_verification_store};
use cas::types::{TaskStatus, Verification, VerificationStatus};
use rmcp::handler::server::wrapper::Parameters;
use std::process::Command;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// RAII guard that installs factory-worker env vars for the duration of a
/// test and clears them on drop.
struct FactoryWorkerGuard;

impl FactoryWorkerGuard {
    fn enter() -> Self {
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "worker");
            std::env::set_var("CAS_FACTORY_MODE", "1");
        }
        Self
    }
}

impl Drop for FactoryWorkerGuard {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("CAS_AGENT_ROLE");
            std::env::remove_var("CAS_FACTORY_MODE");
        }
    }
}

/// Write a config.toml to the cas_dir that enables supervisor-owned review
/// and disables verification (so tests don't hit the verification jail).
fn write_supervisor_review_config(cas_dir: &std::path::Path) {
    let toml = r#"
[verification]
enabled = false

[code_review]
owner = "supervisor"
"#;
    std::fs::write(cas_dir.join("config.toml"), toml).expect("config.toml should be writable");
}

/// Write a config.toml with verification disabled and explicit worker code review
/// (the legacy opt-out path — `owner = "worker"` overrides the supervisor default).
fn write_worker_review_config(cas_dir: &std::path::Path) {
    let toml = r#"
[verification]
enabled = false

[code_review]
owner = "worker"
"#;
    std::fs::write(cas_dir.join("config.toml"), toml).expect("config.toml should be writable");
}

/// Init a minimal git repo at `project_root` with one staged change so that
/// `has_reviewable_changes()` returns true.
fn init_git_repo_with_staged_changes(project_root: &std::path::Path) {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
            .expect("git command should run")
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    // Initial commit so HEAD exists
    std::fs::write(project_root.join("base.rs"), "fn main() {}\n")
        .expect("write should succeed");
    git(&["add", "base.rs"]);
    git(&["commit", "-m", "init"]);
    // Stage a reviewable Rust file so has_reviewable_changes() returns true
    std::fs::write(project_root.join("feature.rs"), "pub fn feature() -> u32 { 42 }\n")
        .expect("write should succeed");
    git(&["add", "feature.rs"]);
}

/// Init a git repo where the staged diff contains a `todo!()` violation so
/// `run_lightweight_structural_lint` returns `Fail`.
fn init_git_repo_with_lint_violation(project_root: &std::path::Path) {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
            .expect("git command should run")
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    // Initial commit so HEAD exists.
    std::fs::write(project_root.join("base.rs"), "fn main() {}\n").expect("write should succeed");
    git(&["add", "base.rs"]);
    git(&["commit", "-m", "init"]);
    // Stage a file with a todo!() — this will appear as an added line in `git diff HEAD`.
    std::fs::write(
        project_root.join("wip.rs"),
        "pub fn incomplete() -> u32 { todo!(\"not implemented yet\") }\n",
    )
    .expect("write should succeed");
    git(&["add", "wip.rs"]);
}

// ---------------------------------------------------------------------------
// AC5 named tests
// ---------------------------------------------------------------------------

/// AC5 test 1: in supervisor-owned review mode, a factory worker close skips
/// the full cas-code-review skill dispatch and transitions the task to
/// `PendingSupervisorReview` instead of triggering `CODE_REVIEW_REQUIRED`.
#[tokio::test]
async fn test_worker_close_in_supervisor_mode_skips_cas_code_review() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    // Write config BEFORE first service creation so the OnceLock picks it up.
    write_supervisor_review_config(&cas_dir);

    // Init git repo at the project root so has_reviewable_changes() returns true.
    init_git_repo_with_staged_changes(temp.path());

    // Rebuild CasCore so it reads from our config.toml.
    let core = core_with_test_agent(&cas_dir);
    let task_store = open_task_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);

    let _worker_guard = FactoryWorkerGuard::enter();

    // Create and start a task.
    let create = task_req(serde_json::json!({
        "action": "create",
        "title": "Feature task for supervisor-mode test",
        "priority": 2,
        "task_type": "task",
    }));
    let created = service
        .task(Parameters(create))
        .await
        .expect("task.create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    service
        .task(Parameters(task_req(
            serde_json::json!({ "action": "start", "id": id }),
        )))
        .await
        .expect("task.start should succeed");

    // Close without a code_review_findings envelope — in supervisor mode this
    // should succeed and transition to pending_supervisor_review, NOT return
    // CODE_REVIEW_REQUIRED.
    let close_result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id,
            "reason": "All acceptance criteria met.",
        }))))
        .await
        .expect("task.close should return a result");

    let close_text = extract_text(close_result);

    // Must NOT see CODE_REVIEW_REQUIRED (the old worker-mode gate).
    assert!(
        !close_text.contains("CODE_REVIEW_REQUIRED"),
        "Supervisor-owned mode must skip CODE_REVIEW_REQUIRED gate; got: {close_text}"
    );

    // Must see the queued-for-supervisor-review confirmation.
    assert!(
        close_text.contains("supervisor review") || close_text.contains("pending_supervisor_review"),
        "Close response should confirm supervisor-review transition; got: {close_text}"
    );

    // Task status must be PendingSupervisorReview, NOT Closed.
    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::PendingSupervisorReview,
        "Task must be in PendingSupervisorReview state after supervisor-mode close"
    );
}

/// AC5 test 2: with explicit `owner = "worker"` (the legacy opt-out), the
/// existing behavior is unchanged — a close without a review envelope returns
/// CODE_REVIEW_REQUIRED.
#[tokio::test]
async fn test_worker_close_in_worker_mode_runs_cas_code_review_unchanged() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    // Use explicit worker opt-out config (verification disabled so we hit the code review gate).
    write_worker_review_config(&cas_dir);

    // Init git repo so has_reviewable_changes() = true.
    init_git_repo_with_staged_changes(temp.path());

    let core = core_with_test_agent(&cas_dir);
    let service = CasService::new(core, None);

    let _worker_guard = FactoryWorkerGuard::enter();

    // Create and start task.
    let create = task_req(serde_json::json!({
        "action": "create",
        "title": "Worker-mode review test",
        "priority": 2,
        "task_type": "task",
    }));
    let created = service
        .task(Parameters(create))
        .await
        .expect("task.create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    service
        .task(Parameters(task_req(
            serde_json::json!({ "action": "start", "id": id }),
        )))
        .await
        .expect("task.start should succeed");

    // Close without a code_review_findings envelope — in worker mode this
    // should return CODE_REVIEW_REQUIRED (unchanged legacy behavior).
    let close_result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id,
            "reason": "All acceptance criteria met.",
        }))))
        .await
        .expect("task.close should return a result");

    let close_text = extract_text(close_result);

    assert!(
        close_text.contains("CODE_REVIEW_REQUIRED"),
        "Worker mode must still require CODE_REVIEW_REQUIRED gate; got: {close_text}"
    );

    // Task must still be InProgress (not closed, not pending review).
    let task_store = open_task_store(&cas_dir).unwrap();
    let task = task_store.get(&id).expect("task should exist");
    assert!(
        task.status != TaskStatus::Closed,
        "Task must remain open when CODE_REVIEW_REQUIRED fires"
    );
    assert_ne!(
        task.status,
        TaskStatus::PendingSupervisorReview,
        "Worker mode must NOT transition to PendingSupervisorReview"
    );
}

/// AC5 test 3: `PendingSupervisorReview` status persists through a store
/// restart (serialize → deserialize round-trip via SQLite).
#[tokio::test]
async fn test_pending_supervisor_review_status_persists_through_restart() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    // Create a task and set it to PendingSupervisorReview directly.
    let mut task = cas::types::Task::new("cas-b51a-test-psr".to_string(), "PSR test".to_string());
    task.status = TaskStatus::PendingSupervisorReview;
    task_store.add(&task).expect("task.add should succeed");

    // Simulate a "restart" by opening a fresh store handle to the same DB.
    let task_store2 = open_task_store(&cas_dir).unwrap();
    let reloaded = task_store2.get("cas-b51a-test-psr").expect("task should exist after reload");

    assert_eq!(
        reloaded.status,
        TaskStatus::PendingSupervisorReview,
        "PendingSupervisorReview status must survive SQLite round-trip"
    );
    // Confirm is_open() / is_ready() semantics are preserved after reload.
    assert!(
        reloaded.is_open(),
        "PendingSupervisorReview task must be considered open"
    );
    assert!(
        !reloaded.is_ready(),
        "PendingSupervisorReview task must NOT be considered ready (for new worker pickup)"
    );
}

/// AC5 test 4: `mcp__cas__verification action=add` works on a task in
/// `PendingSupervisorReview` state — the supervisor can record a verdict
/// without any guard blocking them.
#[tokio::test]
async fn test_supervisor_verify_on_pending_review_task_works() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    write_supervisor_review_config(&cas_dir);
    init_git_repo_with_staged_changes(temp.path());

    // Bind a real worker and supervisor to one factory session so the close
    // path can create an exact supervisor-owned dispatch.
    let session = "cas-ad76-cross-process";
    let agent_store = open_agent_store(&cas_dir).unwrap();
    let worker_id = "cas-ad76-worker";
    let mut worker = cas::types::Agent::new(worker_id.to_string(), "test-agent".to_string());
    worker.role = cas::types::AgentRole::Worker;
    worker.agent_type = cas::types::AgentType::Worker;
    worker.factory_session = Some(session.to_string());
    worker.heartbeat();
    agent_store.register(&worker).unwrap();
    let supervisor_id = "cas-ad76-supervisor";
    let mut supervisor =
        cas::types::Agent::new(supervisor_id.to_string(), "review-supervisor".to_string());
    supervisor.role = cas::types::AgentRole::Supervisor;
    supervisor.factory_session = Some(session.to_string());
    supervisor.heartbeat();
    agent_store.register(&supervisor).unwrap();

    let worker_core = cas::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
    worker_core.set_agent_id_for_testing(worker_id.to_string());
    let worker_service = CasService::new(worker_core, None);
    let worker_guard = FactoryWorkerGuard::enter();
    let created = worker_service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "cas-e86c supervisor review reproduction",
            "priority": 1,
            "task_type": "bug",
        }))))
        .await
        .expect("worker creates task");
    let task_id = extract_task_id(&extract_text(created)).unwrap().to_string();
    worker_service
        .task(Parameters(task_req(
            serde_json::json!({"action": "start", "id": task_id}),
        )))
        .await
        .expect("worker starts task");

    let first_close = extract_text(
        worker_service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": task_id,
                "reason": "exact delivery reviewed by supervisor",
            }))))
            .await
            .expect("worker queues review"),
    );
    let dispatch = cas_store::get_latest_verification_dispatch(&cas_dir, &task_id)
        .expect("dispatch lookup")
        .unwrap_or_else(|| panic!("pending close must create a dispatch: {first_close}"));
    assert!(
        first_close.contains(&dispatch.id),
        "close guidance must return its exact dispatch: {first_close}"
    );

    // A repeated close must reuse the one active boundary, never create a
    // second dispatch or verification row.
    let retry = extract_text(
        worker_service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": task_id,
                "reason": "exact delivery reviewed by supervisor",
            }))))
            .await
            .expect("idempotent close retry"),
    );
    assert!(retry.contains(&dispatch.id), "retry guidance: {retry}");
    assert!(
        agent_store.get_lease(&task_id).unwrap().is_none(),
        "pending-review close must release the worker lease"
    );

    let rejected_created = worker_service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Supervisor review rejection projection",
            "priority": 1,
            "task_type": "bug",
        }))))
        .await
        .expect("worker creates rejection task");
    let rejected_task_id = extract_task_id(&extract_text(rejected_created))
        .unwrap()
        .to_string();
    worker_service
        .task(Parameters(task_req(
            serde_json::json!({"action": "start", "id": rejected_task_id}),
        )))
        .await
        .expect("worker starts rejection task");
    worker_service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": rejected_task_id,
            "reason": "ready for a rejecting review",
        }))))
        .await
        .expect("worker queues rejection task");
    let rejected_dispatch =
        cas_store::get_latest_verification_dispatch(&cas_dir, &rejected_task_id)
            .unwrap()
            .expect("rejection task dispatch");
    drop(worker_guard);
    drop(worker_service);

    // Simulate process restart and cross-process supervisor review with a
    // fresh core bound to the registered supervisor identity.
    let supervisor_core = cas::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor_core.set_agent_id_for_testing(supervisor_id.to_string());
    let add = || VerificationAddRequest {
        task_id: task_id.clone(),
        status: "approved".to_string(),
        summary: "Code review complete — no P0 findings.".to_string(),
        confidence: Some(0.98),
        issues: None,
        files_reviewed: Some("feature.rs".to_string()),
        duration_ms: Some(10),
        verification_type: None,
        verifier_capability: None,
        dispatch_id: Some(dispatch.id.clone()),
    };
    let mut mismatched = add();
    mismatched.dispatch_id = Some("vdisp-does-not-exist".to_string());
    assert!(
        supervisor_core
            .cas_verification_add(Parameters(mismatched))
            .await
            .is_err(),
        "a supervisor must not verify against a mismatched dispatch ID"
    );
    supervisor_core
        .cas_verification_add(Parameters(add()))
        .await
        .expect("supervisor resolves exact pending review dispatch");
    supervisor_core
        .cas_verification_add(Parameters(add()))
        .await
        .expect("identical supervisor review retry is idempotent");
    let mut conflicting = add();
    conflicting.summary = "A different verdict for the same boundary".to_string();
    assert!(
        supervisor_core
            .cas_verification_add(Parameters(conflicting))
            .await
            .is_err(),
        "a conflicting retry must fail closed"
    );
    supervisor_core
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: rejected_task_id.clone(),
            status: "rejected".to_string(),
            summary: "P0 finding requires amendment".to_string(),
            confidence: Some(0.99),
            issues: None,
            files_reviewed: Some("feature.rs".to_string()),
            duration_ms: Some(12),
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(rejected_dispatch.id),
        }))
        .await
        .expect("supervisor rejects exact pending review dispatch");
    let rejected_task = open_task_store(&cas_dir).unwrap().get(&rejected_task_id).unwrap();
    assert_eq!(rejected_task.status, TaskStatus::Blocked);
    assert!(!rejected_task.pending_verification);

    let latest = open_verification_store(&cas_dir)
        .unwrap()
        .get_latest_for_task(&task_id)
        .unwrap()
        .expect("exact verdict");
    assert_eq!(latest.status, VerificationStatus::Approved);
    assert_eq!(latest.dispatch_id.as_deref(), Some(dispatch.id.as_str()));
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
    let (dispatches, verdicts, capabilities, handoffs): (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM verification_dispatches WHERE task_id = ?1),
               (SELECT COUNT(*) FROM verifications WHERE task_id = ?1),
               (SELECT COUNT(*) FROM verification_capabilities WHERE task_id = ?1),
               (SELECT COUNT(*) FROM verification_handoffs)",
            rusqlite::params![task_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!((dispatches, verdicts, capabilities, handoffs), (1, 1, 0, 0));
}

/// AC5 test 5: the `CodeReviewConfig` default owner is "supervisor" (cas-865b).
/// `"worker"` is now the explicit opt-out for the legacy inline dispatch.
#[tokio::test]
async fn test_owner_config_default_is_supervisor() {
    // Test CodeReviewConfig direct default.
    let default_cfg = CodeReviewConfig::default();
    assert_eq!(
        default_cfg.owner, "supervisor",
        "CodeReviewConfig::default() owner must be 'supervisor' after cas-865b flip"
    );
    assert!(
        default_cfg.supervisor_owned(),
        "supervisor_owned() must return true for default config"
    );

    // Test Config deserialization from empty TOML (no [code_review] section).
    let toml_no_section = "";
    let cfg: Config = toml::from_str(toml_no_section).expect("empty TOML should parse");
    assert!(
        cfg.code_review.is_none(),
        "Absent [code_review] section must deserialize to None"
    );
    // When code_review is None, unwrap_or_default() resolves to CodeReviewConfig::default()
    // which is now supervisor-owned.
    let supervisor_owned = cfg
        .code_review
        .clone()
        .unwrap_or_default()
        .supervisor_owned();
    assert!(
        supervisor_owned,
        "Missing [code_review] section must default to supervisor mode via CodeReviewConfig::default()"
    );

    // Test explicit owner = "supervisor" round-trip.
    let toml_supervisor = "[code_review]\nowner = \"supervisor\"\n";
    let cfg2: Config = toml::from_str(toml_supervisor).expect("supervisor TOML should parse");
    let cr2 = cfg2.code_review.as_ref().expect("code_review section should be present");
    assert_eq!(cr2.owner, "supervisor", "TOML owner = 'supervisor' must round-trip");
    assert!(cr2.supervisor_owned(), "supervisor_owned() must be true for owner = 'supervisor'");

    // Test explicit owner = "worker" round-trip (legacy opt-out).
    let toml_worker = "[code_review]\nowner = \"worker\"\n";
    let cfg3: Config = toml::from_str(toml_worker).expect("worker TOML should parse");
    let cr3 = cfg3.code_review.as_ref().expect("code_review section should be present");
    assert!(!cr3.supervisor_owned(), "supervisor_owned() must be false for owner = 'worker'");
}

/// cas-b5ac: A close whose diff contains a `todo!()` call must be rejected by
/// the lightweight structural lint gate when owner=supervisor is configured.
/// The close must:
///   1. Return an MCP-level error (is_error=true), not Ok.
///   2. Name the offending lint rule in the error message.
///   3. Leave the task in InProgress — no transition to PendingSupervisorReview.
#[tokio::test]
async fn test_lint_fail_close_blocked_before_pending_supervisor_review() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    // Enable supervisor-owned review and disable verification.
    write_supervisor_review_config(&cas_dir);

    // Create a git repo whose staged diff includes a `todo!()` so the lint fires.
    init_git_repo_with_lint_violation(temp.path());

    let core = core_with_test_agent(&cas_dir);
    let task_store = open_task_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);

    let _worker_guard = FactoryWorkerGuard::enter();

    // Create and start a task.
    let create = task_req(serde_json::json!({
        "action": "create",
        "title": "WIP task with todo violation",
        "priority": 2,
        "task_type": "task",
    }));
    let created = service
        .task(Parameters(create))
        .await
        .expect("task.create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    service
        .task(Parameters(task_req(
            serde_json::json!({ "action": "start", "id": id }),
        )))
        .await
        .expect("task.start should succeed");

    // Attempt close — the staged diff contains `todo!()`, so lint must fail.
    let close_result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id,
            "reason": "Done.",
        }))))
        .await
        .expect("close returns Ok(CallToolResult) even when lint fails");

    // AC1: close must return is_error=true at the MCP level (not a silent success).
    assert_eq!(
        close_result.is_error,
        Some(true),
        "Lint-fail close must set is_error=true so the worker knows it was rejected"
    );

    let close_text = extract_text(close_result);

    // AC2: error message must name the offending lint rule.
    assert!(
        close_text.contains("todo!(") || close_text.contains("LIGHTWEIGHT LINT FAILED"),
        "Error message must identify the offending lint rule; got: {close_text}"
    );

    // AC3: task must remain InProgress — no PendingSupervisorReview transition on lint failure.
    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::InProgress,
        "Task must remain InProgress after lint failure; got: {:?}",
        task.status
    );
    assert_ne!(
        task.status,
        TaskStatus::PendingSupervisorReview,
        "Lint failure must NOT transition task to PendingSupervisorReview"
    );
}

/// cas-865b: A factory worker close on a project with **no** `[code_review]`
/// section in config.toml must enter supervisor-review mode (PendingSupervisorReview)
/// rather than falling back to the legacy worker path.
///
/// This is the runtime counterpart to the config-layer assertion in
/// `test_owner_config_default_is_supervisor`.  It exercises the fixed
/// `close_ops.rs` line that was previously `.unwrap_or(false)` — which hard-
/// coded worker mode for absent sections.  After cas-865b the absent-section
/// path must track `CodeReviewConfig::default().supervisor_owned()` = true.
#[tokio::test]
async fn test_worker_close_absent_code_review_section_defaults_to_supervisor_mode() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");

    // Write a config with NO [code_review] section — only verification disabled.
    // This is the absent-section case that close_ops.rs must now treat as
    // supervisor mode.
    let toml_no_code_review = "[verification]\nenabled = false\n";
    std::fs::write(cas_dir.join("config.toml"), toml_no_code_review)
        .expect("config.toml should be writable");

    // Init git repo at the project root so has_reviewable_changes() returns true.
    init_git_repo_with_staged_changes(temp.path());

    let core = core_with_test_agent(&cas_dir);
    let task_store = open_task_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);

    let _worker_guard = FactoryWorkerGuard::enter();

    let create = task_req(serde_json::json!({
        "action": "create",
        "title": "Absent-config default-supervisor close test",
        "priority": 2,
        "task_type": "task",
    }));
    let created = service
        .task(Parameters(create))
        .await
        .expect("task.create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    service
        .task(Parameters(task_req(
            serde_json::json!({ "action": "start", "id": id }),
        )))
        .await
        .expect("task.start should succeed");

    // Close without code_review_findings — absent [code_review] must default to
    // supervisor mode and transition to PendingSupervisorReview.
    let close_result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id,
            "reason": "All acceptance criteria met.",
        }))))
        .await
        .expect("task.close should return a result");

    let close_text = extract_text(close_result);

    // Must NOT see CODE_REVIEW_REQUIRED (the old worker-mode gate).
    assert!(
        !close_text.contains("CODE_REVIEW_REQUIRED"),
        "Absent [code_review] must NOT fall back to CODE_REVIEW_REQUIRED; got: {close_text}"
    );

    // Must see the supervisor-review confirmation.
    assert!(
        close_text.contains("supervisor review") || close_text.contains("pending_supervisor_review"),
        "Absent [code_review] must transition to supervisor review; got: {close_text}"
    );

    // Task status must be PendingSupervisorReview.
    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::PendingSupervisorReview,
        "Absent [code_review] section must default to PendingSupervisorReview; got: {:?}",
        task.status
    );
}

// ---------------------------------------------------------------------------
// cas-9684: action=start must reject (not silently clobber) PSR status
// ---------------------------------------------------------------------------

/// cas-9684: `action=start` on a PendingSupervisorReview task must return an
/// error rather than silently resetting the status to InProgress.
///
/// Before this fix, line 572 of lifecycle.rs unconditionally set
/// `task.status = TaskStatus::InProgress`, dropping the task from
/// `list status=pending_supervisor_review` without any warning.
#[tokio::test]
async fn test_start_on_pending_supervisor_review_task_is_rejected() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    // Create a task directly in PendingSupervisorReview state (simulates a
    // worker that already closed → PSR transition).
    let mut task = cas::types::Task::new("cas-9684-start-psr".to_string(), "PSR start guard test".to_string());
    task.status = TaskStatus::PendingSupervisorReview;
    task_store.add(&task).expect("task.add should succeed");

    let core = core_with_test_agent(&cas_dir);
    let service = CasService::new(core, None);

    // Attempt to start the PSR task — must be rejected.
    let start_result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": "cas-9684-start-psr",
        }))))
        .await;

    // The call must either return an Err or an Ok with is_error=true.
    let was_rejected = match &start_result {
        Err(_) => true,
        Ok(result) => result.is_error == Some(true),
    };
    assert!(
        was_rejected,
        "action=start on a PSR task must be rejected; got: {:?}",
        start_result.ok().map(|r| extract_text(r))
    );

    // Status must remain PendingSupervisorReview — must NOT have been reset.
    let task_after = task_store.get("cas-9684-start-psr").expect("task should exist");
    assert_eq!(
        task_after.status,
        TaskStatus::PendingSupervisorReview,
        "PSR status must survive an action=start attempt; got: {:?}",
        task_after.status
    );
}

// ---------------------------------------------------------------------------
// cas-6e4c: orphan-recovery (release) must not reset PSR to Open
// ---------------------------------------------------------------------------

/// cas-6e4c: When a worker's lease expires and `action=release` fires the
/// orphan-recovery path, a PendingSupervisorReview task must NOT be reset
/// to Open. The work is complete; only supervisor review is pending.
///
/// Before this fix, task_claiming.rs excluded only Closed and Open from the
/// auto-recovery reset — PSR tasks were silently reverted to Open, making
/// the worker's completed work disappear.
#[tokio::test]
async fn test_release_orphan_recovery_does_not_reset_psr_to_open() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    // Create a task in PendingSupervisorReview — simulates a worker that
    // finished close → PSR transition but whose lease was never released.
    let mut task = cas::types::Task::new("cas-6e4c-release-psr".to_string(), "PSR release guard test".to_string());
    task.status = TaskStatus::PendingSupervisorReview;
    task_store.add(&task).expect("task.add should succeed");

    let core = core_with_test_agent(&cas_dir);
    let service = CasService::new(core, None);

    // Call action=release with no active lease — triggers the orphan-recovery
    // path that previously reset status to Open.
    let release_result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "release",
            "id": "cas-6e4c-release-psr",
        }))))
        .await;

    // The release call may succeed or return an error, but must NOT have
    // reset the task to Open.
    let _ = release_result; // result shape is unimportant for this assertion

    let task_after = task_store.get("cas-6e4c-release-psr").expect("task should exist");
    assert_ne!(
        task_after.status,
        TaskStatus::Open,
        "PSR task must NOT be reset to Open by orphan-recovery release; got: {:?}",
        task_after.status
    );
    assert_eq!(
        task_after.status,
        TaskStatus::PendingSupervisorReview,
        "PSR task must retain PendingSupervisorReview after release; got: {:?}",
        task_after.status
    );
}

// ---------------------------------------------------------------------------
// cas-7fe9: PSR transition must release the worker's lease
// ---------------------------------------------------------------------------

/// cas-7fe9: After a worker close transitions a task to PendingSupervisorReview,
/// the worker's lease must be released so the supervisor can claim it immediately
/// without hitting a "Task is locked by <worker>" error.
///
/// Before this fix, the PSR early-return path in close_ops.rs omitted the
/// `release_lease_for_task` call that the normal close path performs, leaving
/// a phantom lease for up to 10 minutes after the worker moved on.
#[tokio::test]
async fn test_psr_transition_releases_worker_lease() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    write_supervisor_review_config(&cas_dir);
    init_git_repo_with_staged_changes(temp.path());

    let core = core_with_test_agent(&cas_dir);
    let task_store = open_task_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);

    let _worker_guard = FactoryWorkerGuard::enter();

    // Create and start a task — action=start installs a lease on the task.
    let create_result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "PSR lease-release test (cas-7fe9)",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create should succeed");
    let id = extract_task_id(&extract_text(create_result))
        .expect("should have task ID")
        .to_string();

    service
        .task(Parameters(task_req(serde_json::json!({ "action": "start", "id": id }))))
        .await
        .expect("start should succeed");

    // Close in supervisor-review mode — should transition to PSR and release lease.
    let close_result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id,
            "reason": "All acceptance criteria met.",
        }))))
        .await
        .expect("close should return Ok");

    let close_text = extract_text(close_result);
    assert!(
        close_text.contains("supervisor review") || close_text.contains("pending_supervisor_review"),
        "Close must transition to supervisor review mode; got: {close_text}"
    );

    // Task must be in PendingSupervisorReview state.
    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::PendingSupervisorReview,
        "Task must be in PendingSupervisorReview after supervisor-mode close; got: {:?}",
        task.status
    );

    // The worker's lease must have been released — no active lease on the task.
    // Before cas-7fe9, the PSR early-return omitted `release_lease_for_task`,
    // leaving a phantom lease that blocked supervisor claim for ~10 min.
    let agent_store = open_agent_store(&cas_dir).unwrap();
    let lease = agent_store
        .get_lease(&id)
        .expect("get_lease must not error");
    assert!(
        lease.is_none(),
        "Worker lease must be released after PSR transition (cas-7fe9 fix); \
        got active lease: {:?}",
        lease
    );
}

// ---------------------------------------------------------------------------
// Helpers (local copies of patterns from verification_flow.rs)
// ---------------------------------------------------------------------------

fn task_req(value: serde_json::Value) -> cas_mcp::TaskRequest {
    serde_json::from_value(value).expect("TaskRequest should deserialize from test JSON")
}

// ---------------------------------------------------------------------------
// cas-1932 (GH #62 symptoms 1-2): the zero-diff spike close trap
// ---------------------------------------------------------------------------

/// GH #62 symptom 1: after the supervisor records an APPROVED verification,
/// the worker's re-close must COMPLETE the close instead of re-queuing to
/// `pending_supervisor_review` forever.
///
/// Before the fix the second close ran the same queue-hop branch as the first
/// — the approved verdict on record was never consulted — so no worker close
/// could ever finish and the supervisor had to close on the worker's behalf.
#[tokio::test]
async fn test_worker_reclose_after_approved_verification_completes_close() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    write_supervisor_review_config(&cas_dir);
    init_git_repo_with_staged_changes(temp.path());

    let core = core_with_test_agent(&cas_dir);
    let task_store = open_task_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);

    let _worker_guard = FactoryWorkerGuard::enter();

    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Spike closed under supervisor review",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("task.create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    service
        .task(Parameters(task_req(
            serde_json::json!({ "action": "start", "id": id }),
        )))
        .await
        .expect("task.start should succeed");

    // First close: queues for supervisor review (unchanged behavior).
    let first = extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": id,
                "reason": "Characterization complete.",
            }))))
            .await
            .expect("first close should return a result"),
    );
    assert!(
        !first.contains("CODE_REVIEW_REQUIRED"),
        "supervisor mode must not raise the worker-mode gate; got: {first}"
    );
    assert_eq!(
        task_store.get(&id).expect("task should exist").status,
        TaskStatus::PendingSupervisorReview,
        "precondition: first close queues the task for supervisor review"
    );

    // Supervisor resolves the exact durable dispatch through the public MCP
    // path. A task-wide legacy row must not bypass an active proof boundary.
    let dispatch = cas_store::get_latest_verification_dispatch(&cas_dir, &id)
        .unwrap()
        .expect("pending supervisor dispatch");
    let supervisor_id = "cas-1932-supervisor";
    let mut supervisor =
        cas::types::Agent::new(supervisor_id.to_string(), "review-supervisor".to_string());
    supervisor.role = cas::types::AgentRole::Supervisor;
    supervisor.heartbeat();
    open_agent_store(&cas_dir)
        .unwrap()
        .register(&supervisor)
        .unwrap();
    let supervisor_core = cas::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor_core.set_agent_id_for_testing(supervisor_id.to_string());
    supervisor_core
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: id.clone(),
            status: "approved".to_string(),
            summary: "Reviewed off the queue — no findings.".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(dispatch.id),
        }))
        .await
        .expect("supervisor verdict should resolve the exact dispatch");
    let ver_id = open_verification_store(&cas_dir)
        .unwrap()
        .get_latest_for_task(&id)
        .unwrap()
        .expect("supervisor verdict")
        .id;

    // Second close by the worker: must complete, not re-queue.
    let second = extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": id,
                "reason": "Characterization complete; supervisor approved.",
            }))))
            .await
            .expect("second close should return a result"),
    );
    assert!(
        !second.contains("CODE_REVIEW_REQUIRED"),
        "an approved supervisor verdict must satisfy the review gate; got: {second}"
    );

    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::Closed,
        "worker re-close after an approved verification must close the task, \
         not re-queue it; close said: {second}"
    );
    assert!(
        task.notes.contains(&ver_id),
        "the close must record which verdict authorized it; notes: {}",
        task.notes
    );
}

/// GH #62 symptom 1 (negative case): without an approved verdict the re-close
/// must still queue for supervisor review. The consumption path is an exit for
/// reviewed work only — it must not become a way to skip the queue.
#[tokio::test]
async fn test_worker_reclose_without_approved_verification_still_queues() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    write_supervisor_review_config(&cas_dir);
    init_git_repo_with_staged_changes(temp.path());

    let core = core_with_test_agent(&cas_dir);
    let task_store = open_task_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);

    let _worker_guard = FactoryWorkerGuard::enter();

    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Unreviewed task",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("task.create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    service
        .task(Parameters(task_req(
            serde_json::json!({ "action": "start", "id": id }),
        )))
        .await
        .expect("task.start should succeed");

    for attempt in ["first", "second"] {
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": id,
                "reason": "Done.",
            }))))
            .await
            .unwrap_or_else(|_| panic!("{attempt} close should return a result"));
    }

    // A REJECTED verdict must not authorize the close either.
    let verification_store = open_verification_store(&cas_dir).unwrap();
    let ver_id = verification_store.generate_id().expect("should generate ID");
    let mut row = Verification::new(ver_id, id.clone());
    row.status = VerificationStatus::Rejected;
    row.summary = "Needs rework.".to_string();
    verification_store.add(&row).expect("verdict should persist");

    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id,
            "reason": "Done.",
        }))))
        .await
        .expect("third close should return a result");

    assert_eq!(
        task_store.get(&id).expect("task should exist").status,
        TaskStatus::PendingSupervisorReview,
        "without an APPROVED verdict the task must stay in the review queue"
    );
}

/// GH #62 symptom 2: a zero-commit spike closed in a dirty shared checkout
/// must not trip `CODE_REVIEW_REQUIRED` — the checkout's pre-existing WIP is
/// not the task's diff. Run in worker-owned review mode so the code-review
/// gate itself (not the supervisor queue hop) is what would fire.
#[tokio::test]
async fn test_zero_commit_spike_in_dirty_shared_checkout_closes_without_code_review() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    write_worker_review_config(&cas_dir);
    // Dirty shared checkout: staged reviewable changes the task never made.
    init_git_repo_with_staged_changes(temp.path());

    let core = core_with_test_agent(&cas_dir);
    let task_store = open_task_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);

    let _worker_guard = FactoryWorkerGuard::enter();

    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Characterization-only spike",
            "priority": 2,
            "task_type": "spike",
        }))))
        .await
        .expect("task.create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    service
        .task(Parameters(task_req(
            serde_json::json!({ "action": "start", "id": id }),
        )))
        .await
        .expect("task.start should succeed");

    let close_text = extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": id,
                "reason": "Characterization written up in the task notes; no code changes.",
            }))))
            .await
            .expect("close should return a result"),
    );

    assert!(
        !close_text.contains("CODE_REVIEW_REQUIRED"),
        "a spike that produced no commits must not be charged with the shared \
         checkout's pre-existing WIP; got: {close_text}"
    );
    assert_eq!(
        task_store.get(&id).expect("task should exist").status,
        TaskStatus::Closed,
        "the zero-diff spike close must complete"
    );
}
