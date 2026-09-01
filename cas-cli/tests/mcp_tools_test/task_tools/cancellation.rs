use crate::support::*;
use cas::mcp::CasService;
use cas::mcp::tools::{
    IdRequest, LimitRequest, TaskCancelRequest, TaskListRequest, TaskReopenRequest,
    TaskShowRequest, TaskUpdateRequest,
};
use cas::store::{open_agent_store, open_task_store};
use cas::types::{
    AgentRole, Dependency, DependencyType, Task, TaskStatus, TaskTerminalOutcome, TaskType,
};
use rmcp::handler::server::wrapper::Parameters;

fn promote_test_agent(cas_dir: &std::path::Path) {
    let store = open_agent_store(cas_dir).unwrap();
    let id = format!("test-session-{}", std::process::id());
    let mut agent = store.get(&id).unwrap();
    agent.role = AgentRole::Supervisor;
    store.update(&agent).unwrap();
}

#[tokio::test]
async fn cancel_without_commits_persists_pointer_and_lists_as_no_delivery() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    promote_test_agent(&cas_dir);
    let store = open_task_store(&cas_dir).unwrap();
    let mut task = Task::new("cas-cancel-pointer".into(), "superseded work".into());
    task.assignee = Some("test-agent".into());
    store.add(&task).unwrap();

    let service = CasService::new(core.clone(), None);
    let request: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "cancel",
        "id": task.id.clone(),
        "reason": "requester delivered the same change",
        "superseded_by": "https://github.com/example/repo/pull/258"
    }))
    .unwrap();
    let result = extract_text(service.task(Parameters(request)).await.unwrap());
    assert!(
        result.contains("Cancelled task without delivery"),
        "{result}"
    );

    let persisted = store.get(&task.id).unwrap();
    assert_eq!(persisted.status, TaskStatus::Cancelled);
    assert!(!persisted.counts_as_delivered());
    assert_eq!(
        persisted.terminal_outcome,
        Some(TaskTerminalOutcome::Cancelled {
            superseded_by: Some("https://github.com/example/repo/pull/258".into()),
        })
    );

    let listed = extract_text(
        core.cas_task_list(Parameters(TaskListRequest {
            limit: None,
            scope: "all".into(),
            status: Some("cancelled".into()),
            task_type: None,
            label: None,
            assignee: None,
            epic: None,
            sort: None,
            sort_order: None,
            include_foreign: false,
        }))
        .await
        .unwrap(),
    );
    assert!(listed.contains("Cancelled [NO DELIVERY]"), "{listed}");

    let shown = extract_text(
        core.cas_task_show(Parameters(TaskShowRequest {
            id: task.id.clone(),
            with_deps: false,
        }))
        .await
        .unwrap(),
    );
    assert!(
        shown.contains("Outcome: cancelled without delivery"),
        "{shown}"
    );
    assert!(shown.contains("Superseded by: https://github.com/example/repo/pull/258"));

    assert!(
        !store
            .list_ready()
            .unwrap()
            .iter()
            .any(|ready| ready.id == task.id),
        "cancelled work must not re-enter the ready queue"
    );

    let mine_request: LimitRequest = serde_json::from_value(serde_json::json!({})).unwrap();
    let mine = extract_text(core.cas_tasks_mine(Parameters(mine_request)).await.unwrap());
    assert!(
        !mine.contains(&task.id),
        "cancelled work leaked into mine: {mine}"
    );

    let start_error = core
        .cas_task_start(Parameters(IdRequest {
            id: task.id.clone(),
        }))
        .await
        .expect_err("cancelled work must not be startable");
    assert!(
        start_error.message.contains("terminal task"),
        "{}",
        start_error.message
    );
}

#[tokio::test]
async fn cancel_refuses_blank_reason_without_mutation() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    promote_test_agent(&cas_dir);
    let store = open_task_store(&cas_dir).unwrap();
    let task = Task::new(
        "cas-cancel-blank".into(),
        "must explain cancellation".into(),
    );
    store.add(&task).unwrap();

    let error = core
        .cas_task_cancel(Parameters(TaskCancelRequest {
            id: task.id.clone(),
            reason: "   ".into(),
            superseded_by: None,
        }))
        .await
        .expect_err("blank cancellation reason must be rejected");
    assert!(error.message.contains("reason is required"));
    assert_eq!(store.get(&task.id).unwrap().status, TaskStatus::Open);
}

#[tokio::test]
async fn direct_status_updates_cannot_change_terminal_work() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    promote_test_agent(&cas_dir);
    let store = open_task_store(&cas_dir).unwrap();
    let task = Task::new(
        "cas-cancel-update-bypass".into(),
        "guard cancellation verb".into(),
    );
    store.add(&task).unwrap();

    let direct_cancel: TaskUpdateRequest = serde_json::from_value(serde_json::json!({
        "id": task.id,
        "status": "cancelled"
    }))
    .unwrap();
    let error = core
        .cas_task_update(Parameters(direct_cancel))
        .await
        .expect_err("direct cancellation must not bypass supervisor authority and reason");
    assert!(error.message.contains("action=cancel"), "{}", error.message);
    assert_eq!(store.get(&task.id).unwrap().status, TaskStatus::Open);

    core.cas_task_cancel(Parameters(TaskCancelRequest {
        id: task.id.clone(),
        reason: "replacement landed".into(),
        superseded_by: Some("cas-replacement".into()),
    }))
    .await
    .unwrap();
    let direct_reopen: TaskUpdateRequest = serde_json::from_value(serde_json::json!({
        "id": task.id,
        "status": "open"
    }))
    .unwrap();
    let error = core
        .cas_task_update(Parameters(direct_reopen))
        .await
        .expect_err("direct reopen must not bypass supervisor-authorized reopen");
    assert!(error.message.contains("action=reopen"), "{}", error.message);
    let persisted = store.get(&task.id).unwrap();
    assert_eq!(persisted.status, TaskStatus::Cancelled);
    assert!(matches!(
        persisted.terminal_outcome,
        Some(TaskTerminalOutcome::Cancelled { .. })
    ));

    let mut closed = Task::new(
        "cas-closed-update-bypass".into(),
        "guard terminal reopen verb".into(),
    );
    closed.status = TaskStatus::Closed;
    closed.closed_at = Some(chrono::Utc::now());
    store.add(&closed).unwrap();
    let direct_closed_reopen: TaskUpdateRequest = serde_json::from_value(serde_json::json!({
        "id": closed.id,
        "status": "in_progress"
    }))
    .unwrap();
    let error = core
        .cas_task_update(Parameters(direct_closed_reopen))
        .await
        .expect_err("direct Closed -> active update must use the attributed reopen action");
    assert!(error.message.contains("action=reopen"), "{}", error.message);
    assert_eq!(
        store.get("cas-closed-update-bypass").unwrap().status,
        TaskStatus::Closed
    );
}

#[tokio::test]
async fn epic_cancel_requires_terminal_children_then_cancels_without_delivery() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    promote_test_agent(&cas_dir);
    let store = open_task_store(&cas_dir).unwrap();
    let mut epic = Task::new("cas-cancel-epic".into(), "superseded epic".into());
    epic.task_type = TaskType::Epic;
    let child = Task::new("cas-cancel-child".into(), "superseded child".into());
    store.add(&epic).unwrap();
    store.add(&child).unwrap();
    store
        .add_dependency(&Dependency {
            from_id: child.id.clone(),
            to_id: epic.id.clone(),
            dep_type: DependencyType::ParentChild,
            created_at: chrono::Utc::now(),
            created_by: None,
        })
        .unwrap();

    let blocked = core
        .cas_task_cancel(Parameters(TaskCancelRequest {
            id: epic.id.clone(),
            reason: "replacement epic landed".into(),
            superseded_by: Some("cas-replacement".into()),
        }))
        .await
        .expect_err("non-terminal child must block epic cancellation");
    assert!(blocked.message.contains(&child.id));

    core.cas_task_cancel(Parameters(TaskCancelRequest {
        id: child.id.clone(),
        reason: "covered by replacement".into(),
        superseded_by: Some("cas-replacement".into()),
    }))
    .await
    .unwrap();
    core.cas_task_cancel(Parameters(TaskCancelRequest {
        id: epic.id.clone(),
        reason: "replacement epic landed".into(),
        superseded_by: Some("cas-replacement".into()),
    }))
    .await
    .unwrap();

    let persisted = store.get(&epic.id).unwrap();
    assert_eq!(persisted.status, TaskStatus::Cancelled);
    assert!(!persisted.has_delivery_to_integrate());
}

#[tokio::test]
async fn registered_supervisor_can_reopen_cancelled_task_and_clear_outcome() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    promote_test_agent(&cas_dir);
    let store = open_task_store(&cas_dir).unwrap();
    let task = Task::new("cas-cancel-reopen".into(), "mistaken cancellation".into());
    store.add(&task).unwrap();
    core.cas_task_cancel(Parameters(TaskCancelRequest {
        id: task.id.clone(),
        reason: "thought replacement landed".into(),
        superseded_by: Some("cas-replacement".into()),
    }))
    .await
    .unwrap();

    core.cas_task_reopen(Parameters(TaskReopenRequest {
        id: task.id.clone(),
        reason: Some("replacement was reverted".into()),
    }))
    .await
    .unwrap();

    let reopened = store.get(&task.id).unwrap();
    assert_eq!(reopened.status, TaskStatus::Open);
    assert_eq!(reopened.terminal_outcome, None);
    assert_eq!(reopened.closed_at, None);
    assert!(
        reopened
            .notes
            .contains("Reopened: actor=test-agent (test-session-")
            && reopened.notes.contains(") reason=replacement was reverted"),
        "reopen audit must retain both the registered actor identity and reason: {:?}",
        reopened.notes
    );
}
