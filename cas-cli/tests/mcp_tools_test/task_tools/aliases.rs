//! GH #891: public unified MCP action and parameter aliases.

use crate::support::*;
use cas::mcp::CasService;
use cas_mcp::{CoordinationRequest, TaskRequest};
use rmcp::handler::server::wrapper::Parameters;

async fn create_task(service: &CasService, title: &str) -> String {
    let request = TaskRequest {
        action: "create".to_string(),
        title: Some(title.to_string()),
        task_type: Some("task".to_string()),
        risk: Some("none".to_string()),
        ..serde_json::from_value(serde_json::json!({"action":"create"})).unwrap()
    };
    let text = extract_text(service.task(Parameters(request)).await.unwrap());
    extract_task_id(&text)
        .expect("create should return a task id")
        .to_string()
}

#[tokio::test]
async fn task_get_alias_resolves_to_show() {
    let (_temp, core) = setup_cas();
    let service = CasService::new(core, None);
    let id = create_task(&service, "get alias target").await;

    let request: TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "get",
        "id": id
    }))
    .expect("get action should deserialize");
    let text = extract_text(service.task(Parameters(request)).await.unwrap());

    assert!(text.contains("get alias target"), "show response: {text}");
}

#[tokio::test]
async fn task_dep_add_blocked_by_alias_resolves_to_to_id() {
    let (_temp, core) = setup_cas();
    let service = CasService::new(core, None);
    let dependent = create_task(&service, "dependent").await;
    let blocker = create_task(&service, "blocker").await;

    let request: TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "dep_add",
        "id": dependent,
        "blocked_by": blocker,
        "dep_type": "blocks"
    }))
    .expect("blocked_by alias should deserialize");
    let text = extract_text(service.task(Parameters(request)).await.unwrap());

    assert!(
        text.contains("Added dependency"),
        "dep_add response: {text}"
    );
}

#[tokio::test]
async fn task_dep_remove_blocked_by_alias_resolves_to_to_id() {
    let (_temp, core) = setup_cas();
    let service = CasService::new(core, None);
    let dependent = create_task(&service, "dependent for remove").await;
    let blocker = create_task(&service, "blocker for remove").await;

    let add: TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "dep_add",
        "id": dependent,
        "to_id": blocker,
        "dep_type": "blocks"
    }))
    .expect("canonical dependency should deserialize");
    service.task(Parameters(add)).await.unwrap();

    let remove: TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "dep_remove",
        "id": dependent,
        "blocked_by": blocker,
        "dep_type": "blocks"
    }))
    .expect("blocked_by alias should deserialize for dep_remove");
    let text = extract_text(service.task(Parameters(remove)).await.unwrap());

    assert!(
        text.contains("Removed dependency"),
        "dep_remove response: {text}"
    );
}

#[test]
fn task_close_summary_alias_maps_to_notes() {
    let request: TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "close",
        "id": "cas-alias",
        "summary": "close notes"
    }))
    .expect("summary alias should deserialize for close");

    assert_eq!(request.notes.as_deref(), Some("close notes"));
}

#[test]
fn coordination_factory_aliases_map_to_canonical_fields() {
    let shutdown: CoordinationRequest = serde_json::from_value(serde_json::json!({
        "action": "shutdown_workers",
        "target": "worker-a",
        "reason": "finished"
    }))
    .expect("shutdown aliases should deserialize");
    let factory = shutdown.to_factory_request();
    assert_eq!(factory.worker_names.as_deref(), Some("worker-a"));
    assert_eq!(factory.reason.as_deref(), Some("finished"));

    let hold: CoordinationRequest = serde_json::from_value(serde_json::json!({
        "action": "hold_worker",
        "worker_names": "worker-b"
    }))
    .expect("hold alias should deserialize");
    assert_eq!(
        hold.to_factory_request().target.as_deref(),
        Some("worker-b")
    );
}

#[tokio::test]
async fn coordination_inbox_alias_resolves_to_inbox_poll() {
    let (_temp, core) = setup_cas();
    let service = CasService::new(core, None);
    let request: CoordinationRequest = serde_json::from_value(serde_json::json!({
        "action": "inbox"
    }))
    .expect("inbox alias should deserialize");
    let text = extract_text(service.coordination(Parameters(request)).await.unwrap());

    assert!(
        text.contains("No unread messages") || text.contains("Inbox"),
        "inbox response: {text}"
    );
}
