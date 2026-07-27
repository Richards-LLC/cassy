//! cas-6d0b: `task action=close` on an already-Closed task must short-circuit.
//!
//! Bug report: docs/requests/BUG-task-close-double-close-not-detected-2026-07-14.md
//!
//! Pre-fix, a second close returned success ("Closed task: …"), overwrote
//! `closed_at`, appended a second `Closed:` note, and could demand
//! `CODE_REVIEW_REQUIRED` before mutating. Expected: distinct already-closed
//! result, no mutation, no review-gate demand.

use crate::support::*;
use cas::mcp::tools::*;
use cas::mcp::CasService;
use cas::store::open_task_store;
use cas::types::TaskStatus;
use rmcp::handler::server::wrapper::Parameters;
use std::process::Command;

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

fn task_req(value: serde_json::Value) -> cas_mcp::TaskRequest {
    serde_json::from_value(value).expect("TaskRequest should deserialize from test JSON")
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

fn write_worker_review_config(cas_dir: &std::path::Path) {
    let toml = r#"
[verification]
enabled = false

[code_review]
owner = "worker"
"#;
    std::fs::write(cas_dir.join("config.toml"), toml).expect("config.toml should be writable");
}

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

/// AC1/AC2: second close returns already-closed style result and does not
/// overwrite `closed_at` or append another Closed note.
#[tokio::test]
async fn test_close_on_already_closed_is_non_destructive() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    let created = core
        .cas_task_create(Parameters(create_req(
            "double-close non-destructive",
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

    let first_close = extract_text(
        core.cas_task_close(Parameters(TaskCloseRequest {
            id: id.clone(),
            reason: Some("First closer wins.".to_string()),
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: None,        }))
        .await
        .expect("first close should return a result"),
    );
    assert!(
        first_close.contains("Closed task:"),
        "first close must succeed as a real close: {first_close}"
    );

    let after_first = task_store.get(&id).expect("task should exist");
    assert_eq!(after_first.status, TaskStatus::Closed);
    let closed_at_first = after_first
        .closed_at
        .expect("closed_at must be set after first close");
    let notes_first = after_first.notes.clone();
    let close_reason_first = after_first.close_reason.clone();
    let closed_note_count_first = notes_first.matches("Closed:").count();
    assert!(
        closed_note_count_first >= 1,
        "first close should leave a Closed note: {notes_first}"
    );

    // Second close with a different reason — must not claim credit or mutate.
    let second_close = extract_text(
        core.cas_task_close(Parameters(TaskCloseRequest {
            id: id.clone(),
            reason: Some("Second closer races.".to_string()),
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: None,        }))
        .await
        .expect("second close should return a result"),
    );

    let lower = second_close.to_lowercase();
    assert!(
        lower.contains("already closed") || lower.contains("already-closed"),
        "second close must report already-closed style result, got: {second_close}"
    );
    assert!(
        !second_close.contains("Closed task:"),
        "second close must NOT imply this call performed the close: {second_close}"
    );

    let after_second = task_store.get(&id).expect("task should exist");
    assert_eq!(after_second.status, TaskStatus::Closed);
    assert_eq!(
        after_second.closed_at,
        Some(closed_at_first),
        "closed_at must not be overwritten by a second close"
    );
    assert_eq!(
        after_second.notes, notes_first,
        "notes must not gain a second Closed note on re-close"
    );
    assert_eq!(
        after_second.close_reason, close_reason_first,
        "close_reason must remain the first closer's reason"
    );
    assert_eq!(
        after_second.notes.matches("Closed:").count(),
        closed_note_count_first,
        "Closed note count must stay stable"
    );
}

/// AC3: already-Closed must skip CODE_REVIEW_REQUIRED even under worker-owned
/// review with reviewable diffs (the path that raced in the bug report).
#[tokio::test]
async fn test_close_on_already_closed_skips_code_review_gate() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    // Close once outside factory mode (light skips jail).
    let created = core
        .cas_task_create(Parameters(create_req(
            "double-close skips review gate",
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

    let first = extract_text(
        core.cas_task_close(Parameters(TaskCloseRequest {
            id: id.clone(),
            reason: Some("Initial close.".to_string()),
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: None,        }))
        .await
        .expect("first close should return a result"),
    );
    assert!(
        first.contains("Closed task:"),
        "first close should succeed: {first}"
    );
    let closed_at = task_store
        .get(&id)
        .expect("task")
        .closed_at
        .expect("closed_at set");
    let notes_before = task_store.get(&id).expect("task").notes;

    // Now arm the worker-owned review path that would demand CODE_REVIEW_REQUIRED
    // on an open task with reviewable changes.
    write_worker_review_config(&cas_dir);
    init_git_repo_with_staged_changes(temp.path());
    let core2 = core_with_test_agent(&cas_dir);
    let service = CasService::new(core2, None);
    let _worker_guard = FactoryWorkerGuard::enter();

    let second = extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": id,
                "reason": "Racing closer with reviewable diff.",
            }))))
            .await
            .expect("second close should return a result"),
    );

    assert!(
        !second.contains("CODE_REVIEW_REQUIRED"),
        "already-Closed must not demand CODE_REVIEW_REQUIRED: {second}"
    );
    let lower = second.to_lowercase();
    assert!(
        lower.contains("already closed") || lower.contains("already-closed"),
        "second close must report already-closed: {second}"
    );
    assert!(
        !second.contains("Closed task:"),
        "must not claim this call closed the task: {second}"
    );

    let after = task_store.get(&id).expect("task");
    assert_eq!(after.status, TaskStatus::Closed);
    assert_eq!(after.closed_at, Some(closed_at));
    assert_eq!(after.notes, notes_before);
}
