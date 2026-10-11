//! cas-e477: peer supervisor discovery across "machines" (separate CAS
//! directories) through one fake Cassy Cloud agent registry.
//!
//! The fake implements the three `/api/agents` routes the coordinator uses
//! with the server's real shapes: register upserts and echoes
//! `{status, agent}`, heartbeat answers with the bare agent, and the list is
//! scoped by user only (the token), never by project.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cas::cloud::peers::{PeerDiscovery, discover_peers};
use cas::cloud::{CloudConfig, CloudCoordinator};
use cas::types::{Agent, AgentRole};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::TestMachine;

type Registry = Arc<Mutex<BTreeMap<String, Value>>>;

struct Register(Registry);
impl Respond for Register {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let id = body["id"].as_str().unwrap().to_string();
        let agent = json!({
            "id": id,
            "name": body["name"],
            "agent_type": body["agent_type"],
            "status": "active",
            "pid": body["pid"],
            "session_id": null,
            "machine_id": body["machine_id"],
            "last_heartbeat": chrono::Utc::now().to_rfc3339(),
            "active_tasks": 0,
            "metadata": body["metadata"],
        });
        self.0.lock().unwrap().insert(id, agent.clone());
        ResponseTemplate::new(200).set_body_json(json!({"status": "success", "agent": agent}))
    }
}

struct List(Registry);
impl Respond for List {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let agents: Vec<Value> = self.0.lock().unwrap().values().cloned().collect();
        ResponseTemplate::new(200).set_body_json(json!({
            "status": "success",
            "agents": agents,
            "next_cursor": null,
            "next_cursor_id": null,
        }))
    }
}

struct Heartbeat(Registry);
impl Respond for Heartbeat {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let id = request.url.path().split('/').nth(3).unwrap().to_string();
        let mut registry = self.0.lock().unwrap();
        let agent = registry.get_mut(&id).unwrap();
        agent["last_heartbeat"] = json!(chrono::Utc::now().to_rfc3339());
        ResponseTemplate::new(200).set_body_json(agent.clone())
    }
}

async fn fake_cloud() -> (MockServer, Registry) {
    let server = MockServer::start().await;
    let registry: Registry = Arc::default();
    Mock::given(method("POST"))
        .and(path("/api/agents/register"))
        .respond_with(Register(registry.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/agents"))
        .respond_with(List(registry.clone()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/api/agents/[^/]+/heartbeat$"))
        .respond_with(Heartbeat(registry.clone()))
        .mount(&server)
        .await;
    (server, registry)
}

/// Point a machine's project at the fake cloud under `canonical_id`.
fn seed_project(machine: &TestMachine, endpoint: &str, canonical_id: &str) {
    let config = CloudConfig {
        endpoint: endpoint.to_string(),
        token: Some("test-token".to_string()),
        ..Default::default()
    };
    config.save_to_cas_dir(&machine.cas_dir).unwrap();
    std::fs::write(
        machine.cas_dir.join("config.toml"),
        format!("[project]\ncanonical_id = \"{canonical_id}\"\n"),
    )
    .unwrap();
}

/// Register a supervisor of `cas_dir`'s project, as the daemon does.
fn register_supervisor(cas_dir: &Path, id: &str, machine_id: &str, focus: &str) {
    let config = CloudConfig::load_from_cas_dir(cas_dir).unwrap();
    let canonical_id = cas::cloud::resolve_canonical_id(cas_dir);
    let mut coordinator = CloudCoordinator::new(config)
        .unwrap()
        .with_canonical_id(canonical_id);
    let mut agent = Agent::new(id.to_string(), format!("sup-{id}"));
    agent.role = AgentRole::Supervisor;
    agent.machine_id = Some(machine_id.to_string());
    agent.factory_session = Some(format!("factory-{id}"));
    coordinator
        .register_with_focus(&agent, Some(focus))
        .unwrap();
}

fn peers_of(cas_dir: &Path, self_id: &str) -> Vec<cas::cloud::peers::Peer> {
    match discover_peers(cas_dir, Some(self_id)).unwrap() {
        PeerDiscovery::Found { peers, .. } => peers,
        other => panic!("expected peers, got {other:?}"),
    }
}

#[tokio::test]
async fn two_supervisors_of_one_repo_list_each_other_and_not_other_repos() {
    let (server, registry) = fake_cloud().await;
    let endpoint = server.uri();

    let alpha = TestMachine::new("machine-alpha");
    let beta = TestMachine::new("machine-beta");
    let elsewhere = TestMachine::new("machine-gamma");
    seed_project(&alpha, &endpoint, "github.com/acme/widgets");
    // Another clone of the same repo, spelled as its git remote.
    seed_project(&beta, &endpoint, "git@github.com:acme/widgets.git");
    seed_project(&elsewhere, &endpoint, "github.com/acme/gadgets");

    let (a, b, g) = (
        alpha.cas_dir.clone(),
        beta.cas_dir.clone(),
        elsewhere.cas_dir.clone(),
    );
    let (alpha_peers, beta_peers, after_stale) = tokio::task::spawn_blocking(move || {
        register_supervisor(&a, "a", "machine-alpha", "cas-571d");
        register_supervisor(&b, "b", "machine-beta", "cas-f9c7");
        register_supervisor(&g, "g", "machine-gamma", "cas-0000");

        let alpha_peers = peers_of(&a, "a");
        let beta_peers = peers_of(&b, "b");

        // Beta stops heartbeating: alpha still lists it, but as stale.
        let stale = (chrono::Utc::now() - chrono::Duration::minutes(15)).to_rfc3339();
        registry.lock().unwrap().get_mut("b").unwrap()["last_heartbeat"] = json!(stale);
        (alpha_peers, beta_peers, peers_of(&a, "a"))
    })
    .await
    .unwrap();

    assert_eq!(alpha_peers.len(), 1, "{alpha_peers:?}");
    assert_eq!(alpha_peers[0].agent_id, "b");
    assert_eq!(alpha_peers[0].machine_id.as_deref(), Some("machine-beta"));
    assert_eq!(alpha_peers[0].focus.as_deref(), Some("cas-f9c7"));
    assert_eq!(alpha_peers[0].session.as_deref(), Some("factory-b"));
    assert!(alpha_peers[0].live);

    assert_eq!(beta_peers.len(), 1, "{beta_peers:?}");
    assert_eq!(beta_peers[0].agent_id, "a");
    assert_eq!(beta_peers[0].machine_id.as_deref(), Some("machine-alpha"));
    assert_eq!(beta_peers[0].focus.as_deref(), Some("cas-571d"));

    assert_eq!(after_stale.len(), 1);
    assert!(!after_stale[0].live, "a 15-minute-old heartbeat is stale");
    assert!(after_stale[0].heartbeat_age_secs >= 15 * 60);
}

#[tokio::test]
async fn discovery_reports_logged_out_projects_instead_of_an_empty_list() {
    let machine = TestMachine::new("machine-solo");
    std::fs::write(
        machine.cas_dir.join("config.toml"),
        "[project]\ncanonical_id = \"github.com/acme/widgets\"\n",
    )
    .unwrap();
    let cas_dir = machine.cas_dir.clone();
    let found = tokio::task::spawn_blocking(move || discover_peers(&cas_dir, None))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found, PeerDiscovery::NotLoggedIn);
}
