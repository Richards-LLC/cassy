use crate::support::*;
use cas::mcp::tools::{TaskCancelRequest, TaskListRequest, TaskReopenRequest, TaskShowRequest};
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
    let task = Task::new("cas-cancel-pointer".into(), "superseded work".into());
    store.add(&task).unwrap();

    let result = extract_text(
        core.cas_task_cancel(Parameters(TaskCancelRequest {
            id: task.id.clone(),
            reason: "requester delivered the same change".into(),
            superseded_by: Some("https://github.com/example/repo/pull/258".into()),
        }))
        .await
        .unwrap(),
    );
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
        }))
        .await
        .unwrap(),
    );
    assert!(listed.contains("Cancelled [NO DELIVERY]"), "{listed}");

    let shown = extract_text(
        core.cas_task_show(Parameters(TaskShowRequest {
            id: task.id,
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
            .contains("Reopened: replacement was reverted")
    );
}
