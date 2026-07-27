//! cas-6538 (EPIC cas-1255 — per-task depth speed mode): close-gate light-skip.
//!
//! These tests pin the enforcement layer: a `depth=light` task skips the two
//! *rigor* gates at close time — the verification jail and the P0 code-review
//! gate (including the supervisor-review queue hop that IS the P0 gate under
//! `owner = "supervisor"`) — and records an auditable decision note. The
//! regression guard is the whole point: `depth=deep`/unset must arm the jail
//! and pend for supervisor review exactly as today, proven by paired tests
//! that fail if the skip leaks to deep.

use crate::support::*;
use cas::mcp::tools::*;
use cas::mcp::{CasCore, CasService};
use cas::store::open_task_store;
use cas::types::TaskStatus;
use rmcp::handler::server::wrapper::Parameters;
use std::process::Command;

/// Build a `cas_mcp::TaskRequest` from test JSON.
fn task_req(value: serde_json::Value) -> cas_mcp::TaskRequest {
    serde_json::from_value(value).expect("TaskRequest should deserialize from test JSON")
}

/// Typed `TaskCreateRequest` with the given depth (`None` = unset/default).
/// The solo tests drive `cas_task_close` *directly* on the core — bypassing
/// the dispatch-layer MCP jail (`authorize_agent_action`) the same way the
/// existing verification_flow tests do — to isolate the close_ops gate.
fn create_req(title: &str, depth: Option<&str>) -> TaskCreateRequest {
    TaskCreateRequest {
        depth: depth.map(str::to_string),
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

/// RAII guard that installs factory-worker env vars and clears them on drop.
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

/// Supervisor-owned review + verification disabled, so a factory-worker close
/// reaches the supervisor-review transition rather than the verification jail.
fn write_supervisor_review_config(cas_dir: &std::path::Path) {
    let toml = r#"
[verification]
enabled = false

[code_review]
owner = "supervisor"
"#;
    std::fs::write(cas_dir.join("config.toml"), toml).expect("config.toml should be writable");
}

/// Init a git repo at `project_root` with one staged Rust change so that
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
    std::fs::write(project_root.join("base.rs"), "fn main() {}\n").expect("write should succeed");
    git(&["add", "base.rs"]);
    git(&["commit", "-m", "init"]);
    std::fs::write(
        project_root.join("feature.rs"),
        "pub fn feature() -> u32 { 42 }\n",
    )
    .expect("write should succeed");
    git(&["add", "feature.rs"]);
}

// ---------------------------------------------------------------------------
// Solo (non-factory) close — verification jail branch, exercised directly on
// `cas_task_close`.
//
// setup_cas() leaves verification ENABLED (no config written), so the close_ops
// jail arms for deep and skips for light. These call the core method directly
// (bypassing the dispatch-layer MCP jail) to isolate the close_ops gate — the
// same convention every existing verification_flow close test uses.
// ---------------------------------------------------------------------------

/// AC: closing a `depth=light` task skips the verification jail — close
/// succeeds, `pending_verification` stays false, and a decision note records
/// the skip.
#[tokio::test]
async fn test_light_depth_solo_close_skips_verification_jail() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    let created = core
        .cas_task_create(Parameters(create_req("light task — solo close", Some("light"))))
        .await
        .expect("task_create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    core.cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start should succeed");

    let close_text = extract_text(
        core.cas_task_close(Parameters(TaskCloseRequest {
            id: id.clone(),
            reason: Some("Feel-driven pass complete.".to_string()),
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: None,        }))
        .await
        .expect("task_close should return a result"),
    );

    assert!(
        !close_text.contains("VERIFICATION REQUIRED"),
        "light close must not arm the verification jail: {close_text}"
    );
    assert!(
        close_text.contains("Closed"),
        "light close should report success: {close_text}"
    );

    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::Closed,
        "light task must transition to Closed"
    );
    assert!(
        !task.pending_verification,
        "light close must leave pending_verification false"
    );
    // Auditable decision note recording exactly what was skipped and why.
    assert!(
        task.notes.contains("depth=light")
            && task.notes.to_lowercase().contains("decision")
            && task.notes.contains("verification jail")
            && task.notes.contains("code-review gate"),
        "light close must record an auditable decision note naming both skipped \
         gates: {}",
        task.notes
    );
}

/// Regression guard: a `depth=deep` (explicit) task still arms the jail.
/// Fails if the light-skip leaks to deep.
#[tokio::test]
async fn test_deep_depth_solo_close_still_arms_verification_jail() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    let created = core
        .cas_task_create(Parameters(create_req("deep task — solo close", Some("deep"))))
        .await
        .expect("task_create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    core.cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start should succeed");

    let close_text = extract_text(
        core.cas_task_close(Parameters(TaskCloseRequest {
            id: id.clone(),
            reason: Some("Done.".to_string()),
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: None,        }))
        .await
        .expect("task_close should return a result"),
    );

    assert!(
        close_text.contains("VERIFICATION REQUIRED"),
        "deep close must still arm the verification jail: {close_text}"
    );

    let task = task_store.get(&id).expect("task should exist");
    assert!(
        task.pending_verification,
        "deep close must set pending_verification = true"
    );
    assert_ne!(
        task.status,
        TaskStatus::Closed,
        "deep close must NOT close while verification is pending"
    );
    assert!(
        !task.notes.contains("depth=light"),
        "deep close must not write the light-skip decision note: {}",
        task.notes
    );
}

/// Regression guard: an unset-depth (legacy / NULL→Deep) task behaves like
/// deep — the jail arms. Proves the default reads as Deep at the close gate.
#[tokio::test]
async fn test_unset_depth_solo_close_still_arms_verification_jail() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    let created = core
        .cas_task_create(Parameters(create_req("unset-depth task — solo close", None)))
        .await
        .expect("task_create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    core.cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start should succeed");

    let close_text = extract_text(
        core.cas_task_close(Parameters(TaskCloseRequest {
            id: id.clone(),
            reason: Some("Done.".to_string()),
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: None,        }))
        .await
        .expect("task_close should return a result"),
    );

    assert!(
        close_text.contains("VERIFICATION REQUIRED"),
        "unset-depth close must arm the jail (NULL→Deep): {close_text}"
    );
    assert!(
        task_store
            .get(&id)
            .expect("task should exist")
            .pending_verification,
        "unset-depth close must set pending_verification = true"
    );
}

// ---------------------------------------------------------------------------
// Factory-worker close under supervisor-owned review — P0 gate branch.
//
// Under `owner = "supervisor"` a worker close with reviewable changes pends
// to PendingSupervisorReview (the queue hop IS the P0 gate). depth=light must
// instead close immediately; depth=deep must still pend.
// ---------------------------------------------------------------------------

/// AC: a `depth=light` factory-worker close with reviewable changes treats the
/// P0 code-review gate as satisfied — the task closes immediately rather than
/// pending for supervisor review, with the decision note recorded.
#[tokio::test]
async fn test_light_depth_factory_close_skips_p0_gate_and_closes() {
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
            "title": "light feature task — factory close",
            "priority": 2,
            "task_type": "task",
            "depth": "light",
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
                "reason": "All acceptance criteria met.",
            }))))
            .await
            .expect("task.close should return a result"),
    );

    assert!(
        !close_text.contains("CODE_REVIEW_REQUIRED"),
        "light close must not trip the code-review gate: {close_text}"
    );
    assert!(
        !close_text.contains("pending_supervisor_review")
            && !close_text.contains("supervisor review"),
        "light close must NOT pend for supervisor review — it closes immediately: {close_text}"
    );

    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::Closed,
        "light factory close must transition straight to Closed, not \
         PendingSupervisorReview; got {:?}",
        task.status
    );
    assert!(
        task.notes.contains("depth=light") && task.notes.contains("code-review gate"),
        "light factory close must record the decision note: {}",
        task.notes
    );
}

/// Regression guard: a `depth=deep` factory-worker close with reviewable
/// changes still pends for supervisor review. Fails if the P0-gate skip leaks
/// to deep.
#[tokio::test]
async fn test_deep_depth_factory_close_still_pends_supervisor_review() {
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
            "title": "deep feature task — factory close",
            "priority": 2,
            "task_type": "task",
            "depth": "deep",
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
                "reason": "All acceptance criteria met.",
            }))))
            .await
            .expect("task.close should return a result"),
    );

    assert!(
        close_text.contains("supervisor review")
            || close_text.contains("pending_supervisor_review"),
        "deep close must still pend for supervisor review: {close_text}"
    );

    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::PendingSupervisorReview,
        "deep factory close must pend for supervisor review, not close: {:?}",
        task.status
    );
    assert!(
        !task.notes.contains("depth=light"),
        "deep close must not write the light-skip decision note: {}",
        task.notes
    );
}

// =============================================================================
// cas-3c23: reopen is supervisor-only; start-on-closed guidance is role-aware.
//
// Root cause of the cas-a7c8 grok thrash loop: an idle worker told (by a
// stale re-dispatch) to work an already-Closed ticket would follow the
// start-guard's literal instruction ("Use reopen first"), reopen the task
// itself (no role check existed on `cas_task_reopen`), re-verify already-
// shipped code, and re-close it — stomping main. These tests pin: (1) a
// non-supervisor caller cannot reopen a Closed task, (2) a supervisor still
// can, and (3) the start-on-closed error message no longer tells a worker
// to reopen.
// =============================================================================

/// Close a light-depth task with no factory-role env set (the same "solo"
/// path used above) and return its id so callers can drive further
/// lifecycle calls against the now-Closed task.
async fn create_started_and_closed_light_task(core: &CasCore, title: &str) -> String {
    let created = core
        .cas_task_create(Parameters(create_req(title, Some("light"))))
        .await
        .expect("task_create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    core.cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start should succeed");

    let close_text = extract_text(
        core.cas_task_close(Parameters(TaskCloseRequest {
            id: id.clone(),
            reason: Some("Feel-driven pass complete.".to_string()),
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: None,        }))
        .await
        .expect("task_close should return a result"),
    );
    assert!(
        close_text.contains("Closed"),
        "setup close should succeed: {close_text}"
    );
    id
}

/// A non-supervisor caller (no CAS_AGENT_ROLE=supervisor) must NOT be able
/// to reopen a Closed task — the task stays Closed and the rejection points
/// the caller at the supervisor instead of performing the reopen.
#[tokio::test]
async fn test_worker_cannot_reopen_closed_task() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    // Deliberately no CAS_AGENT_ROLE=supervisor — a plain/worker caller.
    let id = create_started_and_closed_light_task(&core, "worker reopen rejection").await;

    let result = core
        .cas_task_reopen(Parameters(TaskReopenRequest {
            id: id.clone(),
            reason: None,
        }))
        .await;

    match result {
        Err(e) => {
            let msg = e.message.to_string();
            assert!(
                msg.contains("supervisor") || msg.contains("CAS_AGENT_ROLE"),
                "rejection should point the worker at the supervisor: {msg}"
            );
        }
        Ok(ok) => panic!(
            "expected reopen to be rejected for a non-supervisor caller, got: {}",
            extract_text(ok)
        ),
    }

    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::Closed,
        "task must remain Closed after a rejected worker reopen attempt"
    );
}

/// A supervisor caller (CAS_AGENT_ROLE=supervisor) can still reopen a
/// Closed task — the guard is a role check, not a blanket ban.
#[tokio::test]
async fn test_supervisor_can_reopen_closed_task() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    let id = create_started_and_closed_light_task(&core, "supervisor reopen happy path").await;

    let reopen_text = {
        // SAFETY: held under env_test_lock() for the whole scope.
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "supervisor");
        }
        let text = extract_text(
            core.cas_task_reopen(Parameters(TaskReopenRequest {
                id: id.clone(),
                reason: None,
            }))
            .await
            .expect("supervisor reopen should succeed"),
        );
        unsafe {
            std::env::remove_var("CAS_AGENT_ROLE");
        }
        text
    };

    assert!(
        reopen_text.contains("Reopened task:"),
        "supervisor reopen should succeed: {reopen_text}"
    );
    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::Open,
        "supervisor reopen must transition the task back to Open"
    );
}

/// cas-cd24 AC1: `reopen` on a Blocked task transitions it to Open and
/// records the supplied reason on the audit trail (task.notes) — the
/// motivating case from BUG-blocked-tasks-cannot-be-reopened.md (a
/// supervisor withdraws an acceptance criterion that wrongly blocked the
/// task, and needs a documented way to put it back in play).
#[tokio::test]
async fn test_cd24_supervisor_can_reopen_blocked_task_with_reason() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    let created = core
        .cas_task_create(Parameters(create_req("blocked reopen with reason", Some("light"))))
        .await
        .expect("task_create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    // Force the task into Blocked directly — there is no dedicated "block"
    // MCP action; workers block a task via `update status=blocked`, which
    // this bypasses to isolate the reopen behavior under test.
    {
        let mut task = task_store.get(&id).expect("task should exist");
        task.status = TaskStatus::Blocked;
        task_store.update(&task).expect("force task to Blocked");
    }

    let reopen_text = {
        // SAFETY: held under env_test_lock() for the whole scope.
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "supervisor");
        }
        let text = extract_text(
            core.cas_task_reopen(Parameters(TaskReopenRequest {
                id: id.clone(),
                reason: Some("Withdrew the iOS background-auto-start AC — Apple does not permit it.".to_string()),
            }))
            .await
            .expect("supervisor reopen of a blocked task should succeed"),
        );
        unsafe {
            std::env::remove_var("CAS_AGENT_ROLE");
        }
        text
    };
    assert!(
        reopen_text.contains("Reopened task:"),
        "blocked-task reopen should succeed: {reopen_text}"
    );

    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::Open,
        "reopen must transition a Blocked task to Open"
    );
    assert!(
        task.notes.contains("Withdrew the iOS background-auto-start AC"),
        "reopen reason must be captured in the audit trail (task.notes): {:?}",
        task.notes
    );
    assert!(
        task.notes.contains("Unblocked"),
        "blocked-task reopen note should say Unblocked, not Reopened: {:?}",
        task.notes
    );
}

/// cas-cd24 AC2: Closed->Open reopen behavior is unchanged by the
/// blocked-task support — `closed_at` still clears on reopen.
#[tokio::test]
async fn test_cd24_closed_reopen_still_clears_closed_at() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    let id = create_started_and_closed_light_task(&core, "closed reopen clears closed_at").await;
    let closed_task = task_store.get(&id).expect("task should exist");
    assert!(
        closed_task.closed_at.is_some(),
        "sanity: a closed task must carry closed_at"
    );

    {
        // SAFETY: held under env_test_lock() for the whole scope.
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "supervisor");
        }
        core.cas_task_reopen(Parameters(TaskReopenRequest {
            id: id.clone(),
            reason: None,
        }))
        .await
        .expect("supervisor reopen should succeed");
        unsafe {
            std::env::remove_var("CAS_AGENT_ROLE");
        }
    }

    let reopened = task_store.get(&id).expect("task should exist");
    assert_eq!(reopened.status, TaskStatus::Open);
    assert!(
        reopened.closed_at.is_none(),
        "cas-cd24 must not change the existing Closed->Open closed_at reset"
    );
}

/// cas-cd24 AC1 (error-message half of the fix): a task that is neither
/// Closed nor Blocked still gets a clear rejection, and the rejection now
/// names the working alternative (`update status=open`) instead of the old
/// "only closed tasks can be reopened" dead end.
#[tokio::test]
async fn test_cd24_reopen_rejects_other_statuses_and_names_alternative() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();

    let created = core
        .cas_task_create(Parameters(create_req(
            "in-progress task cannot be reopened",
            Some("light"),
        )))
        .await
        .expect("task_create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();
    core.cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start should succeed");

    let result = {
        // SAFETY: held under env_test_lock() for the whole scope.
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "supervisor");
        }
        let r = core
            .cas_task_reopen(Parameters(TaskReopenRequest {
                id: id.clone(),
                reason: None,
            }))
            .await;
        unsafe {
            std::env::remove_var("CAS_AGENT_ROLE");
        }
        let _ = temp;
        r
    };

    match result {
        Err(e) => {
            let msg = e.message.to_string();
            assert!(
                msg.contains("update id=") && msg.contains("status=open"),
                "rejection must name the update-status-open alternative: {msg}"
            );
        }
        Ok(ok) => panic!(
            "expected reopen to be rejected for an in-progress task, got: {}",
            extract_text(ok)
        ),
    }
}

/// The start-on-closed error message must no longer tell a worker to
/// "Use reopen first" — that instruction is exactly what caused the
/// cas-a7c8 thrash loop. A worker caller should instead be told to report
/// to the supervisor without reopening.
#[tokio::test]
async fn test_start_on_closed_message_is_worker_appropriate() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();

    let id = create_started_and_closed_light_task(&core, "start-on-closed worker message").await;

    // Deliberately no CAS_AGENT_ROLE=supervisor.
    let result = core
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await;

    let _ = temp; // keep tempdir alive for the duration of the call above
    match result {
        Err(e) => {
            let msg = e.message.to_string();
            assert!(
                !msg.contains("Use reopen first"),
                "worker-facing start-on-closed message must not instruct reopen: {msg}"
            );
            assert!(
                msg.contains("do not reopen") && msg.contains("supervisor"),
                "worker-facing start-on-closed message should direct to the supervisor: {msg}"
            );
        }
        Ok(ok) => panic!("expected start-on-closed to fail, got: {}", extract_text(ok)),
    }
}

/// A supervisor caller starting a Closed task still gets an error (start
/// does not silently reopen), but the message is the supervisor-appropriate
/// variant that does not carry the worker refusal wording.
#[tokio::test]
async fn test_start_on_closed_message_is_supervisor_appropriate() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();

    let id =
        create_started_and_closed_light_task(&core, "start-on-closed supervisor message").await;

    let result = {
        // SAFETY: held under env_test_lock() for the whole scope.
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "supervisor");
        }
        let r = core
            .cas_task_start(Parameters(IdRequest { id: id.clone() }))
            .await;
        unsafe {
            std::env::remove_var("CAS_AGENT_ROLE");
        }
        r
    };

    let _ = temp; // keep tempdir alive for the duration of the call above
    match result {
        Err(e) => {
            let msg = e.message.to_string();
            assert!(
                !msg.contains("do not reopen"),
                "supervisor-facing message should not use the worker refusal wording: {msg}"
            );
        }
        Ok(ok) => panic!("expected start-on-closed to fail, got: {}", extract_text(ok)),
    }
}
