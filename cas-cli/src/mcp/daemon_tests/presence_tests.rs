//! cas-f707: cloud presence driven through the embedded daemon, against a
//! stateful fake of the Cassy Cloud agent registry. The daemon's own entry
//! points run here (SessionStart, the heartbeat tick, `set_agent_id`), not
//! the library calls they make.

use super::*;
use cas_types::Task;
use std::path::{Path, PathBuf};
use crate::cloud::presence::{self, PresenceState};
use crate::cloud::{CloudConfig, PROJECT_SCOPE_KEY};
use crate::store::init_cas_dir;
use crate::test_support::TestEnvGuard;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const REPO: &str = "github.com/acme/widgets";
const SESSION: &str = "cassy-f707-session";

#[derive(Default)]
struct Registry {
    /// Registrations still to refuse with 503.
    reject_registrations: u32,
    agents: HashMap<String, Value>,
    registrations: Vec<Value>,
    list_queries: Vec<String>,
    heartbeats: Vec<(String, u16)>,
}

/// The `/api/agents` routes of Cassy Cloud, with state: register upserts,
/// heartbeat answers 404 for an agent it does not know, list returns what
/// was registered.
#[derive(Clone, Default)]
struct FakeCloud(Arc<Mutex<Registry>>);

impl FakeCloud {
    fn state(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.0.lock().unwrap()
    }
}

impl Respond for FakeCloud {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.state();
        let path = request.url.path().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        match (request.method.as_str(), path.as_str()) {
            ("POST", "/api/agents/register") => {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                state.registrations.push(body.clone());
                if state.reject_registrations > 0 {
                    state.reject_registrations -= 1;
                    return ResponseTemplate::new(503).set_body_string("registry unavailable");
                }
                let id = body["id"].as_str().unwrap().to_string();
                let agent = json!({
                    "id": id,
                    "name": body["name"],
                    "agent_type": body["agent_type"],
                    "status": "active",
                    "pid": body["pid"],
                    "session_id": null,
                    "machine_id": body["machine_id"],
                    "last_heartbeat": now,
                    "active_tasks": 0,
                    "metadata": body["metadata"],
                });
                state.agents.insert(id, agent.clone());
                ResponseTemplate::new(200).set_body_json(json!({"status": "success", "agent": agent}))
            }
            ("GET", "/api/agents") => {
                state
                    .list_queries
                    .push(request.url.query().unwrap_or_default().to_string());
                let agents: Vec<Value> = state.agents.values().cloned().collect();
                ResponseTemplate::new(200).set_body_json(json!({"agents": agents}))
            }
            ("POST", heartbeat) if heartbeat.ends_with("/heartbeat") => {
                let id = heartbeat
                    .trim_start_matches("/api/agents/")
                    .trim_end_matches("/heartbeat")
                    .to_string();
                match state.agents.get_mut(&id) {
                    Some(agent) => {
                        agent["last_heartbeat"] = json!(now);
                        let agent = agent.clone();
                        state.heartbeats.push((id, 200));
                        ResponseTemplate::new(200).set_body_json(agent)
                    }
                    None => {
                        state.heartbeats.push((id, 404));
                        ResponseTemplate::new(404).set_body_string("agent not found")
                    }
                }
            }
            _ => ResponseTemplate::new(404),
        }
    }
}

struct Fixture {
    _env: TestEnvGuard,
    cas_root: PathBuf,
    cloud: FakeCloud,
    _server: MockServer,
}

/// A project logged in to the fake cloud, with a factory session pinned to
/// `focus`, and a daemon for it.
async fn fixture(focus: &str) -> (Fixture, EmbeddedDaemon) {
    let server = MockServer::start().await;
    let cloud = FakeCloud::default();
    Mock::given(any())
        .respond_with(cloud.clone())
        .mount(&server)
        .await;

    let mut env = TestEnvGuard::temp_home();
    let home = env.home().to_path_buf();
    env.set("CAS_USER_CLOUD_JSON", home.join("no-user-cloud.json"));
    env.set("CAS_FACTORY_SESSION", SESSION);
    let cas_root = init_cas_dir(&home).unwrap();
    crate::cloud::set_canonical_id_in_config_toml(&cas_root, REPO).unwrap();
    let mut config = CloudConfig::default();
    config.endpoint = server.uri();
    config.token = Some("synthetic-test-token".to_string());
    config.save_to_cas_dir(&cas_root).unwrap();

    let metadata_path = crate::ui::factory::metadata_path(SESSION);
    std::fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
    let mut metadata = crate::ui::factory::create_metadata(
        SESSION,
        std::process::id(),
        "sup-f707",
        &[],
        None,
        Some(home.to_str().unwrap()),
        None,
    );
    metadata.pinned_epic_id = Some(focus.to_string());
    std::fs::write(&metadata_path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();

    let daemon = EmbeddedDaemon::new(EmbeddedDaemonConfig {
        cas_root: cas_root.clone(),
        index_code: false,
        ..Default::default()
    });
    (
        Fixture {
            _env: env,
            cas_root,
            cloud,
            _server: server,
        },
        daemon,
    )
}

/// The harness PID the daemon treats as its own (`is_our_agent`).
fn our_cc_pid() -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::process::parent_id()
    }
    #[cfg(not(unix))]
    {
        std::process::id()
    }
}

async fn session_start(daemon: &EmbeddedDaemon, session_id: &str, name: &str) {
    daemon
        .handle_socket_event(DaemonEvent::SessionStart {
            session_id: session_id.to_string(),
            agent_name: Some(name.to_string()),
            agent_role: Some("supervisor".to_string()),
            cc_pid: our_cc_pid(),
            clone_path: None,
        })
        .await;
}

/// One daemon heartbeat tick, waiting for its cloud presence upkeep.
async fn heartbeat_tick(daemon: &EmbeddedDaemon) {
    daemon.send_agent_heartbeat().await;
    daemon.await_cloud_presence().await;
}

fn peer_ids(cas_root: &Path, viewer: &str) -> Vec<(String, Option<String>)> {
    match crate::cloud::peers::discover_peers(cas_root, Some(viewer)).unwrap() {
        crate::cloud::peers::PeerDiscovery::Found { peers, .. } => peers
            .into_iter()
            .map(|peer| (peer.agent_id, peer.focus))
            .collect(),
        other => panic!("expected peers, got {other:?}"),
    }
}

#[tokio::test]
async fn cas_f707_daemon_retries_a_rejected_registration_and_reregisters_after_a_heartbeat_404() {
    let (fx, daemon) = fixture("cas-571d").await;
    fx.cloud.state().reject_registrations = 1;
    let me = "sup-f707-alpha";

    // SessionStart: the cloud refuses the first registration.
    session_start(&daemon, me, "alpha-sup").await;
    let record = presence::load(&fx.cas_root, me).expect("presence recorded");
    assert_eq!(record.state, PresenceState::Failing);
    assert!(
        record.cause.as_deref().unwrap_or_default().contains("503"),
        "{record:?}"
    );
    let shown = crate::cloud::peers::peers_section(&fx.cas_root, Some(me));
    assert!(
        shown.starts_with("Cloud presence: failing: Failed to register agent (503)"),
        "the failure and its cause are visible in peers: {shown}"
    );
    assert!(peer_ids(&fx.cas_root, "someone-else").is_empty());

    // The next heartbeat tick retries and the cloud accepts it.
    heartbeat_tick(&daemon).await;
    let record = presence::load(&fx.cas_root, me).unwrap();
    assert_eq!(record.state, PresenceState::Registered, "{record:?}");
    assert_eq!(record.focus.as_deref(), Some("cas-571d"));
    assert_eq!(fx.cloud.state().registrations.len(), 2);

    // A registered agent's tick is a plain heartbeat.
    heartbeat_tick(&daemon).await;
    assert_eq!(fx.cloud.state().registrations.len(), 2);
    assert_eq!(fx.cloud.state().heartbeats.last(), Some(&(me.to_string(), 200)));

    // The cloud loses the agent: the heartbeat gets 404 and the daemon
    // registers it again within the same tick.
    fx.cloud.state().agents.clear();
    heartbeat_tick(&daemon).await;
    assert_eq!(fx.cloud.state().heartbeats.last(), Some(&(me.to_string(), 404)));
    assert_eq!(fx.cloud.state().registrations.len(), 3);
    assert_eq!(
        presence::load(&fx.cas_root, me).unwrap().state,
        PresenceState::Registered
    );

    // Discoverable by a peer, with its focus; registered in peers.
    assert_eq!(
        peer_ids(&fx.cas_root, "someone-else"),
        vec![(me.to_string(), Some("cas-571d".to_string()))]
    );
    let shown = crate::cloud::peers::peers_section(&fx.cas_root, Some(me));
    assert!(
        shown.starts_with("Cloud presence: registered (focus cas-571d"),
        "{shown}"
    );

    // Every registration and list call carries the project scope.
    let state = fx.cloud.state();
    for registration in &state.registrations {
        assert_eq!(registration[PROJECT_SCOPE_KEY], json!(REPO), "{registration}");
    }
    assert!(
        state
            .list_queries
            .iter()
            .all(|query| query.contains("canonical_id=github.com%2Facme%2Fwidgets")),
        "{:?}",
        state.list_queries
    );
}

#[tokio::test]
async fn cas_f707_eager_registration_path_pushes_to_the_cloud() {
    let (fx, daemon) = fixture("cas-571d").await;
    let me = "sup-f707-eager";
    // The eager MCP registration writes the row, then hands the id to the
    // daemon (mcp/server register_agent_with_hints → set_agent_id).
    let store = crate::store::open_agent_store(&fx.cas_root).unwrap();
    let mut agent = Agent::new(me.to_string(), "eager-sup".to_string());
    agent.role = AgentRole::Supervisor;
    agent.pid = Some(our_cc_pid());
    store.register(&agent).unwrap();

    daemon.set_agent_id(me.to_string()).await;
    daemon.await_cloud_presence().await;

    assert_eq!(fx.cloud.state().registrations.len(), 1);
    assert_eq!(
        presence::load(&fx.cas_root, me).unwrap().state,
        PresenceState::Registered
    );
    assert_eq!(
        peer_ids(&fx.cas_root, "someone-else"),
        vec![(me.to_string(), Some("cas-571d".to_string()))]
    );
}

#[tokio::test]
async fn cas_f707_focus_epic_refreshes_the_registered_focus() {
    let (fx, daemon) = fixture("cas-old-epic").await;
    let me = "sup-f707-focus";
    session_start(&daemon, me, "focus-sup").await;
    assert_eq!(
        presence::load(&fx.cas_root, me).unwrap().focus.as_deref(),
        Some("cas-old-epic")
    );

    let tasks = crate::store::open_task_store(&fx.cas_root).unwrap();
    let mut epic = Task::new("cas-new-epic".to_string(), "new epic".to_string());
    epic.task_type = cas_types::TaskType::Epic;
    tasks.add(&epic).unwrap();
    crate::ops::fleet::focus_epic(
        &fx.cas_root,
        SESSION,
        crate::ops::fleet::FocusEpic::Pin {
            epic_id: "cas-new-epic",
            delivery_mode: None,
        },
    )
    .unwrap();

    // focus_epic's own push (the MCP handler's path) ...
    let pushed = presence::push_agent(&fx.cas_root, me).unwrap();
    assert_eq!(pushed.focus.as_deref(), Some("cas-new-epic"));
    // ... and the daemon heartbeat, which catches a focus change made
    // elsewhere (the Commander hub): pin back and tick.
    crate::ops::fleet::focus_epic(
        &fx.cas_root,
        SESSION,
        crate::ops::fleet::FocusEpic::Clear,
    )
    .unwrap();
    heartbeat_tick(&daemon).await;
    let record = presence::load(&fx.cas_root, me).unwrap();
    assert_eq!(record.state, PresenceState::Registered);
    assert_eq!(record.focus, None, "a cleared focus is pushed too");

    let focuses: Vec<Option<String>> = fx
        .cloud
        .state()
        .registrations
        .iter()
        .map(|body| body["metadata"]["focus"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        focuses,
        vec![
            Some("cas-old-epic".to_string()),
            Some("cas-new-epic".to_string()),
            None
        ]
    );
}

#[tokio::test]
async fn cas_f707_a_failing_registration_waits_out_its_backoff() {
    let (fx, daemon) = fixture("cas-571d").await;
    fx.cloud.state().reject_registrations = 2;
    let me = "sup-f707-backoff";
    session_start(&daemon, me, "backoff-sup").await;
    // Failure 1 is retried on the next tick; failure 2 starts a 30 s wait.
    heartbeat_tick(&daemon).await;
    let record = presence::load(&fx.cas_root, me).unwrap();
    assert_eq!((record.state, record.failures), (PresenceState::Failing, 2));
    heartbeat_tick(&daemon).await;
    assert_eq!(
        fx.cloud.state().registrations.len(),
        2,
        "no attempt before the backoff runs out"
    );
    let shown = crate::cloud::peers::peers_section(&fx.cas_root, Some(me));
    assert!(shown.contains("2 failed attempts, next retry in"), "{shown}");
}
