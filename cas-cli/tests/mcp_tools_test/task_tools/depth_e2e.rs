//! cas-9d74 (EPIC cas-1255 — per-task depth speed mode): capstone end-to-end.
//!
//! cas-0344 shipped the data layer (the `depth` column + serialize/deserialize
//! with NULL→Deep) and cas-6538 shipped the close-gate light-skip. The per-task
//! suites each test ONE layer in a single in-process session. This capstone
//! proves the two layers COMPOSE across a real persistence boundary — the exact
//! seam where cross-task drift would hide:
//!
//!   1. A worker creates a `depth=light` / `deep` / unset task (session 1).
//!   2. A brand-new `open_task_store` reloads the task from SQLite and we assert
//!      the depth round-tripped (data layer survives the write/read boundary).
//!   3. A *different* CasCore + CasService (session 2 — modelling a separate
//!      worker process) starts and closes the task, and we assert the reloaded
//!      depth still drives the close gate: light closes immediately (no jail, no
//!      P0/supervisor-review hop) with an auditable decision note; deep/unset
//!      pend for supervisor review exactly as today.
//!
//! If the data layer and the close gate ever disagree about a task's depth
//! after persistence (e.g. a serialize regression that drops Light to NULL→Deep
//! on read), the light test's close would suddenly pend and this fails — which
//! the single-session per-layer tests cannot catch.

use crate::support::*;
use cas::mcp::CasService;
use cas::store::open_task_store;
use cas::types::{TaskDepth, TaskStatus};
use rmcp::handler::server::wrapper::Parameters;
use std::process::Command;

/// Build a `cas_mcp::TaskRequest` from test JSON (drives the full MCP service
/// dispatch path, like a real client).
fn task_req(value: serde_json::Value) -> cas_mcp::TaskRequest {
    serde_json::from_value(value).expect("TaskRequest should deserialize from test JSON")
}

/// RAII guard that installs factory-worker env vars and clears them on drop, so
/// the close gate runs its worker-under-supervisor-review branch.
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

/// Verification disabled + supervisor-owned review, so a factory-worker close
/// reaches the supervisor-review (P0) transition rather than the jail.
fn write_supervisor_review_config(cas_dir: &std::path::Path) {
    let toml = r#"
[verification]
enabled = false

[code_review]
owner = "supervisor"
"#;
    std::fs::write(cas_dir.join("config.toml"), toml).expect("config.toml should be writable");
}

/// Init a git repo with one staged Rust change so `has_reviewable_changes()`
/// returns true and the close actually reaches the P0 code-review gate.
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
    std::fs::write(project_root.join("base.rs"), "fn main() {}\n").expect("write base.rs");
    git(&["add", "base.rs"]);
    git(&["commit", "-m", "init"]);
    // The close gate re-validates the current factory/test-agent tip before
    // entering PendingSupervisorReview. Keep the worker branch at the
    // integration base and add remote-tracking evidence that it was already
    // published; this shared-checkout fixture exercises the review gate's
    // staged change without modeling an unmerged worker branch.
    git(&["branch", "factory/test-agent"]);
    git(&[
        "update-ref",
        "refs/remotes/origin/factory/test-agent",
        "factory/test-agent",
    ]);
    std::fs::write(
        project_root.join("feature.rs"),
        "pub fn feature() -> u32 { 42 }\n",
    )
    .expect("write feature.rs");
    git(&["add", "feature.rs"]);
}

/// Session-1 create: returns the new task id. Built on its own core/service so
/// the close runs on a freshly-opened one (session 2).
async fn create_task(cas_dir: &std::path::Path, title: &str, depth: Option<&str>) -> String {
    let core = core_with_test_agent(cas_dir);
    let service = CasService::new(core, None);
    let mut body = serde_json::json!({
        "action": "create",
        "title": title,
        "priority": 2,
        "task_type": "task",
    });
    if let Some(d) = depth {
        body["depth"] = serde_json::Value::String(d.to_string());
    }
    let created = service
        .task(Parameters(task_req(body)))
        .await
        .expect("task.create should succeed");
    extract_task_id(&extract_text(created))
        .expect("create output should carry a task id")
        .to_string()
}

/// Session-2 start+close on a fresh core/service; returns the close output text.
async fn start_and_close(cas_dir: &std::path::Path, id: &str) -> String {
    // Fresh session (second worker process) — must seed agent identity itself
    // (cas-48e6); bare with_daemon has no SessionStart mapping / agent_id.
    let core = core_with_test_agent(cas_dir);
    let service = CasService::new(core, None);
    service
        .task(Parameters(task_req(
            serde_json::json!({ "action": "start", "id": id }),
        )))
        .await
        .expect("task.start should succeed");
    extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": id,
                "reason": "All acceptance criteria met.",
            }))))
            .await
            .expect("task.close should return a result"),
    )
}

/// E2E (light): depth=light round-trips through SQLite and, read back by a
/// separate session, still skips both rigor gates — the task closes immediately
/// with the auditable decision note.
#[tokio::test]
async fn test_e2e_light_depth_persists_then_closes_without_gates() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    write_supervisor_review_config(&cas_dir);
    init_git_repo_with_staged_changes(temp.path());

    // Session 1 — create the light task.
    let id = create_task(&cas_dir, "feel-driven UI tweak", Some("light")).await;

    // Persistence boundary — a fresh store read must see depth=Light.
    let reloaded = open_task_store(&cas_dir)
        .unwrap()
        .get(&id)
        .expect("task should persist");
    assert_eq!(
        reloaded.depth,
        TaskDepth::Light,
        "depth=light must round-trip through SQLite, not collapse to the default"
    );

    // Session 2 — a separate worker process closes it.
    let _worker = FactoryWorkerGuard::enter();
    let close_text = start_and_close(&cas_dir, &id).await;

    assert!(
        !close_text.contains("CODE_REVIEW_REQUIRED"),
        "light close must not trip the code-review gate: {close_text}"
    );
    assert!(
        !close_text.contains("pending_supervisor_review")
            && !close_text.contains("supervisor review"),
        "light close must NOT pend for supervisor review — it closes immediately: {close_text}"
    );

    let task = open_task_store(&cas_dir).unwrap().get(&id).unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Closed,
        "light task must end Closed, not PendingSupervisorReview; got {:?}",
        task.status
    );
    assert_eq!(
        task.depth,
        TaskDepth::Light,
        "depth must remain Light through the close"
    );
    assert!(
        task.notes.contains("depth=light") && task.notes.contains("code-review gate"),
        "light close must record the auditable decision note: {}",
        task.notes
    );
}

/// E2E (deep): depth=deep round-trips and, read back by a separate session,
/// still enforces the P0 gate — the task pends for supervisor review.
#[tokio::test]
async fn test_e2e_deep_depth_persists_then_pends_supervisor_review() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    write_supervisor_review_config(&cas_dir);
    init_git_repo_with_staged_changes(temp.path());

    let id = create_task(&cas_dir, "auth refactor", Some("deep")).await;

    let reloaded = open_task_store(&cas_dir).unwrap().get(&id).unwrap();
    assert_eq!(
        reloaded.depth,
        TaskDepth::Deep,
        "depth=deep must round-trip through SQLite"
    );

    let _worker = FactoryWorkerGuard::enter();
    let close_text = start_and_close(&cas_dir, &id).await;

    assert!(
        close_text.contains("supervisor review")
            || close_text.contains("pending_supervisor_review"),
        "deep close must still pend for supervisor review: {close_text}"
    );

    let task = open_task_store(&cas_dir).unwrap().get(&id).unwrap();
    assert_eq!(
        task.status,
        TaskStatus::PendingSupervisorReview,
        "deep task must pend for supervisor review, not close: {:?}",
        task.status
    );
    assert!(
        !task.notes.contains("depth=light"),
        "deep close must not write the light-skip decision note: {}",
        task.notes
    );
}

/// E2E (unset / legacy): a task created with no depth reads back as Deep
/// (NULL→Deep) across the persistence boundary and is enforced like deep — the
/// default composes with the gate exactly as an explicit deep does.
#[tokio::test]
async fn test_e2e_unset_depth_reads_as_deep_then_pends_supervisor_review() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    write_supervisor_review_config(&cas_dir);
    init_git_repo_with_staged_changes(temp.path());

    let id = create_task(&cas_dir, "legacy task — no depth set", None).await;

    let reloaded = open_task_store(&cas_dir).unwrap().get(&id).unwrap();
    assert_eq!(
        reloaded.depth,
        TaskDepth::Deep,
        "unset depth must read back as Deep (NULL→Deep)"
    );

    let _worker = FactoryWorkerGuard::enter();
    let close_text = start_and_close(&cas_dir, &id).await;

    assert!(
        close_text.contains("supervisor review")
            || close_text.contains("pending_supervisor_review"),
        "unset-depth close must enforce the P0 gate like deep: {close_text}"
    );
    assert_eq!(
        open_task_store(&cas_dir).unwrap().get(&id).unwrap().status,
        TaskStatus::PendingSupervisorReview,
        "unset-depth task must pend for supervisor review"
    );
}
