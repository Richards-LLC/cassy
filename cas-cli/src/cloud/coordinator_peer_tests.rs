//! cas-e477: the coordinator's peer-discovery wire contract against the
//! Cassy Cloud `/api/agents` routes (register, heartbeat, paged list).
use super::*;
use crate::cloud::peers::{META_CANONICAL_ID, META_FOCUS, META_ROLE};
use crate::types::AgentRole;
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPO: &str = "github.com/acme/widgets";

fn coordinator(endpoint: &str) -> CloudCoordinator {
    CloudCoordinator::new(CloudConfig {
        endpoint: endpoint.to_string(),
        token: Some("test-token".to_string()),
        ..Default::default()
    })
    .unwrap()
    .with_canonical_id(Some(REPO.to_string()))
}

fn agent_json(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": format!("name-{id}"),
        "agent_type": "primary",
        "status": "active",
        "pid": null,
        "session_id": null,
        "machine_id": "machine-a",
        "last_heartbeat": "2026-10-11T03:00:00.000Z",
        "active_tasks": 0,
        "metadata": {}
    })
}

#[tokio::test]
async fn register_carries_role_repo_and_focus_in_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/agents/register"))
        .and(body_partial_json(json!({
            "metadata": {
                META_ROLE: "supervisor",
                META_CANONICAL_ID: REPO,
                META_FOCUS: "cas-571d",
            }
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status": "success", "agent": agent_json("sup-1")})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = server.uri();
    let registered = tokio::task::spawn_blocking(move || {
        let mut coord = coordinator(&endpoint);
        let mut agent = Agent::new("sup-1".into(), "bright-lark-8".into());
        agent.role = AgentRole::Supervisor;
        coord.register_with_focus(&agent, Some("cas-571d"))
    })
    .await
    .unwrap();
    assert_eq!(registered.unwrap().id, "sup-1");
}

#[tokio::test]
async fn heartbeat_agent_targets_that_agent_and_reads_the_flat_body() {
    let server = MockServer::start().await;
    // The server answers a heartbeat with the bare agent, not {status, agent}.
    Mock::given(method("POST"))
        .and(path("/api/agents/sup-2/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(agent_json("sup-2")))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = server.uri();
    let beat = tokio::task::spawn_blocking(move || coordinator(&endpoint).heartbeat_agent("sup-2"))
        .await
        .unwrap();
    assert_eq!(beat.unwrap().id, "sup-2");
}

#[tokio::test]
async fn list_agent_infos_follows_every_keyset_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/agents"))
        .and(query_param("cursor_id", "a2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "success",
            "agents": [agent_json("a3")],
            "next_cursor": null,
            "next_cursor_id": null,
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/agents"))
        .and(query_param("limit", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "success",
            "agents": [agent_json("a1"), agent_json("a2")],
            "next_cursor": "2026-10-10T00:00:00.000Z",
            "next_cursor_id": "a2",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = server.uri();
    let agents = tokio::task::spawn_blocking(move || coordinator(&endpoint).list_agent_infos())
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<&str> = agents.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["a1", "a2", "a3"]);
}
