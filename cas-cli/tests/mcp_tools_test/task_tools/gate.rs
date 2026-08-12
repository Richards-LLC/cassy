use crate::support::*;
use cas::mcp::CasService;
use cas::mcp::tools::{IdRequest, TaskUpdateRequest};
use cas::store::{open_agent_store, open_task_store};
use cas::types::{AgentRole, TaskStatus, TaskTerminalOutcome, TaskType};
use rmcp::handler::server::wrapper::Parameters;
use std::process::Command;

fn set_test_agent_role(cas_dir: &std::path::Path, role: AgentRole) {
    let store = open_agent_store(cas_dir).unwrap();
    let id = format!("test-session-{}", std::process::id());
    let mut agent = store.get(&id).unwrap();
    agent.role = role;
    store.update(&agent).unwrap();
}

async fn unified_task(service: &CasService, request: serde_json::Value) -> String {
    let request: cas_mcp::TaskRequest = serde_json::from_value(request).unwrap();
    extract_text(service.task(Parameters(request)).await.unwrap())
}

#[tokio::test]
async fn supervisor_gate_closes_on_decision_and_unblocks_dependent_without_commit() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    set_test_agent_role(&cas_dir, AgentRole::Supervisor);
    let service = CasService::new(core.clone(), None);

    let created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "title": "Approve rollout",
            "task_type": "gate"
        }),
    )
    .await;
    let gate_id = extract_task_id(&created).unwrap().to_string();

    let dependent = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "title": "Begin rollout",
            "task_type": "task",
            "blocked_by": gate_id
        }),
    )
    .await;
    let dependent_id = extract_task_id(&dependent).unwrap().to_string();
    let store = open_task_store(&cas_dir).unwrap();
    assert_eq!(store.get(&gate_id).unwrap().task_type, TaskType::Gate);
    assert!(
        !store
            .list_ready()
            .unwrap()
            .iter()
            .any(|task| task.id == dependent_id),
        "an open Gate must remain a normal dependency blocker"
    );

    let started = unified_task(
        &service,
        serde_json::json!({"action": "start", "id": gate_id, "brief": true}),
    )
    .await;
    assert!(started.contains("Started task"), "{started}");
    assert_eq!(store.get(&gate_id).unwrap().status, TaskStatus::InProgress);

    // A structured but empty decision note is not a decision.
    unified_task(
        &service,
        serde_json::json!({
            "action": "notes",
            "id": gate_id,
            "note_type": "decision",
            "notes": "   "
        }),
    )
    .await;
    let refused = unified_task(
        &service,
        serde_json::json!({
            "action": "close",
            "id": gate_id,
            "reason": "promotion approved"
        }),
    )
    .await;
    assert!(
        refused.contains("no non-empty recorded DECISION note"),
        "{refused}"
    );
    let unchanged = store.get(&gate_id).unwrap();
    assert_eq!(unchanged.status, TaskStatus::InProgress);
    assert_eq!(unchanged.terminal_outcome, None);

    unified_task(
        &service,
        serde_json::json!({
            "action": "notes",
            "id": gate_id,
            "note_type": "decision",
            "notes": "Approve the staged rollout after the error-budget review."
        }),
    )
    .await;
    let closed = unified_task(
        &service,
        serde_json::json!({
            "action": "close",
            "id": gate_id,
            "reason": "promotion approved"
        }),
    )
    .await;
    assert!(closed.contains("Closed task"), "{closed}");

    let persisted = store.get(&gate_id).unwrap();
    assert_eq!(persisted.status, TaskStatus::Closed);
    assert_eq!(
        persisted.terminal_outcome,
        Some(TaskTerminalOutcome::Decision)
    );
    assert!(!persisted.counts_as_delivered());
    assert!(persisted.deliverables.merge_commit.is_none());
    assert!(
        store
            .list_ready()
            .unwrap()
            .iter()
            .any(|task| task.id == dependent_id),
        "Decision-close must satisfy the Gate dependency"
    );
}

#[tokio::test]
async fn gate_start_is_supervisor_only_without_weakening_ordinary_start_protection() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let store = open_task_store(&cas_dir).unwrap();
    let service = CasService::new(core.clone(), None);

    let mut gate = cas::types::Task::new("cas-gate-worker-start".into(), "Gate".into());
    gate.task_type = TaskType::Gate;
    store.add(&gate).unwrap();
    let worker_error = core
        .cas_task_start(Parameters(IdRequest {
            id: gate.id.clone(),
        }))
        .await
        .expect_err("a worker must not start a supervisor-owned Gate");
    assert!(
        worker_error
            .message
            .contains("only a live registered supervisor"),
        "{}",
        worker_error.message
    );
    assert_eq!(store.get(&gate.id).unwrap().status, TaskStatus::Open);

    unified_task(
        &service,
        serde_json::json!({
            "action": "notes",
            "id": gate.id.clone(),
            "note_type": "decision",
            "notes": "Approve the rollout."
        }),
    )
    .await;
    let worker_close = unified_task(
        &service,
        serde_json::json!({"action": "close", "id": gate.id.clone()}),
    )
    .await;
    assert!(
        worker_close.contains("only a live registered supervisor"),
        "{worker_close}"
    );
    assert_eq!(store.get(&gate.id).unwrap().status, TaskStatus::Open);

    set_test_agent_role(&cas_dir, AgentRole::Supervisor);
    let direct_close: TaskUpdateRequest = serde_json::from_value(serde_json::json!({
        "id": gate.id.clone(),
        "status": "closed"
    }))
    .unwrap();
    let direct_close_error = core
        .cas_task_update(Parameters(direct_close))
        .await
        .expect_err("direct status update must not bypass the Gate decision classifier");
    assert!(
        direct_close_error.message.contains("GATE UPDATE REJECTED"),
        "{}",
        direct_close_error.message
    );
    assert_eq!(store.get(&gate.id).unwrap().status, TaskStatus::Open);

    let ordinary = cas::types::Task::new("cas-ordinary-supervisor-start".into(), "Work".into());
    store.add(&ordinary).unwrap();
    let supervisor_error = core
        .cas_task_start(Parameters(IdRequest {
            id: ordinary.id.clone(),
        }))
        .await
        .expect_err("supervisors must still delegate ordinary work");
    assert!(
        supervisor_error
            .message
            .contains("Supervisors cannot start non-epic tasks"),
        "{}",
        supervisor_error.message
    );
    assert_eq!(store.get(&ordinary.id).unwrap().status, TaskStatus::Open);
}

#[tokio::test]
async fn cas525c_supervisor_proof_scope_fix_reopens_with_decision_not_review_failure() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(temp.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/example/cas525c-fixture.git",
    ]);
    std::fs::write(temp.path().join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    Command::new("git")
        .args(["commit", "-q", "-m", "seed"])
        .current_dir(temp.path())
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .unwrap();
    git(&["branch", "release"]);
    cas::store::known_repos::ensure_host_schema().unwrap();

    let service = CasService::new(core.clone(), None);
    let created = unified_task(
        &service,
        serde_json::json!({
            "action": "create",
            "title": "Correct delivery branch",
            "target_repo": temp.path().to_str().unwrap(),
            "target_branch": "main"
        }),
    )
    .await;
    let task_id = extract_task_id(&created).unwrap().to_string();
    let store = open_task_store(&cas_dir).unwrap();
    let mut task = store.get(&task_id).unwrap();
    task.status = TaskStatus::AwaitingMerge;
    store.update(&task).unwrap();
    set_test_agent_role(&cas_dir, AgentRole::Supervisor);

    let fixed = unified_task(
        &service,
        serde_json::json!({
            "action": "update",
            "id": task_id,
            "target_branch": "release",
            "proof_scope_fix": true,
            "reason": "The task was created against the wrong integration branch."
        }),
    )
    .await;
    assert!(fixed.contains("Corrected proof scope"), "{fixed}");
    let corrected = store.get(&task_id).unwrap();
    assert_eq!(corrected.status, TaskStatus::Open);
    assert_eq!(
        corrected
            .deliverables
            .work_target
            .as_ref()
            .unwrap()
            .target_branch,
        "release"
    );
    assert!(corrected.notes.contains("DECISION: proof scope corrected"));
    assert!(
        corrected
            .notes
            .contains("no failed-review verdict was recorded")
    );
    assert!(!corrected.notes.contains("changes requested by supervisor"));
}
