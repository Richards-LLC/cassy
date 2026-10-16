//! cas-f707: cloud presence driven through the embedded daemon, against a
//! stateful fake of the Cassy Cloud agent registry. The daemon's own entry
//! points run here (SessionStart, the heartbeat tick, `set_agent_id`), not
//! the library calls they make.

use super::*;
use cas_types::Task;
use std::path::{Path, PathBuf};
use crate::cloud::presence::{self, PresenceState};
use crate::cloud::{CloudConfig, PROJECT_SCOPE_KEY};
use crate::cloud::peers::{META_BRANCH, META_CANONICAL_ID, META_FACTORY_SESSION, META_FOCUS};
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
    /// The `metadata` body of every heartbeat, in order.
    heartbeat_metadata: Vec<Value>,
    /// cas-66c7: when set, every request with another bearer token is
    /// refused with 401, as production refuses an invalid API key.
    valid_token: Option<String>,
    /// Requests refused for their token.
    rejected_tokens: Vec<String>,
}

/// The `/api/agents` routes of Cassy Cloud as shipped (cloud PR #153), with
/// state: register upserts; heartbeat answers 404 for an agent it does not
/// know and merges a `{"metadata": {...}}` body; list returns what was
/// registered, filtered by `?project=`.
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
        if let Some(valid) = state.valid_token.clone() {
            let sent = request
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            if sent != format!("Bearer {valid}") {
                state.rejected_tokens.push(sent);
                return ResponseTemplate::new(401).set_body_string(r#"{"error":"Invalid API key"}"#);
            }
        }
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
                let project = request
                    .url
                    .query_pairs()
                    .find(|(key, _)| key == "project")
                    .map(|(_, value)| value.to_string());
                let agents: Vec<Value> = state
                    .agents
                    .values()
                    .filter(|agent| {
                        project.as_deref().is_none_or(|project| {
                            agent["metadata"][META_CANONICAL_ID].as_str() == Some(project)
                        })
                    })
                    .cloned()
                    .collect();
                ResponseTemplate::new(200).set_body_json(json!({"agents": agents}))
            }
            ("POST", heartbeat) if heartbeat.ends_with("/heartbeat") => {
                let id = heartbeat
                    .trim_start_matches("/api/agents/")
                    .trim_end_matches("/heartbeat")
                    .to_string();
                let patch = serde_json::from_slice::<Value>(&request.body)
                    .ok()
                    .and_then(|body| body.get("metadata").cloned());
                if let Some(patch) = &patch {
                    state.heartbeat_metadata.push(patch.clone());
                }
                match state.agents.get_mut(&id) {
                    Some(agent) => {
                        agent["last_heartbeat"] = json!(now);
                        if let Some(Value::Object(patch)) = patch {
                            for (key, value) in patch {
                                agent["metadata"][key] = value;
                            }
                        }
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
    /// Another clone observes cloud registration without treating this
    /// project's own locally registered supervisor as a peer (cas-559c).
    peer_root: PathBuf,
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
    let peer_root = init_cas_dir(&home.join("peer-clone")).unwrap();
    crate::cloud::set_canonical_id_in_config_toml(&peer_root, REPO).unwrap();
    config.save_to_cas_dir(&peer_root).unwrap();
    assert!(
        crate::store::open_agent_store(&peer_root)
            .unwrap()
            .list(None)
            .unwrap()
            .is_empty()
    );

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
            peer_root,
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
    daemon.await_cloud_presence().await;
}

/// One daemon heartbeat tick, waiting for its cloud presence upkeep.
async fn heartbeat_tick(daemon: &EmbeddedDaemon) {
    // Settle any earlier kick first, so this tick is not skipped.
    daemon.await_cloud_presence().await;
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
    assert!(peer_ids(&fx.peer_root, "someone-else").is_empty());

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
        peer_ids(&fx.peer_root, "someone-else"),
        vec![(me.to_string(), Some("cas-571d".to_string()))]
    );
    let shown = crate::cloud::peers::peers_section(&fx.cas_root, Some(me));
    assert!(
        shown.starts_with("Cloud presence: registered (focus cas-571d"),
        "{shown}"
    );

    // Every registration and list call carries the project scope; every
    // heartbeat re-binds the project and carries the factory session.
    let state = fx.cloud.state();
    for registration in &state.registrations {
        assert_eq!(registration[PROJECT_SCOPE_KEY], json!(REPO), "{registration}");
        assert_eq!(registration["metadata"][META_CANONICAL_ID], json!(REPO));
        assert_eq!(registration["metadata"][META_FACTORY_SESSION], json!(SESSION));
    }
    assert!(!state.heartbeat_metadata.is_empty());
    for beat in &state.heartbeat_metadata {
        assert_eq!(beat[META_CANONICAL_ID], json!(REPO), "{beat}");
        assert_eq!(beat[META_FACTORY_SESSION], json!(SESSION), "{beat}");
        assert_eq!(beat[META_FOCUS], json!("cas-571d"), "{beat}");
        assert_eq!(
            beat[crate::cloud::peers::META_HOSTNAME],
            json!(crate::cloud::peers::machine_label()),
            "{beat}"
        );
    }
    assert!(
        state
            .list_queries
            .iter()
            .all(|query| query.contains("project=github.com%2Facme%2Fwidgets")),
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
        peer_ids(&fx.peer_root, "someone-else"),
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

    let state = fx.cloud.state();
    let focuses: Vec<Option<String>> = state
        .registrations
        .iter()
        .map(|body| body["metadata"]["focus"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        focuses,
        vec![Some("cas-old-epic".to_string()), Some("cas-new-epic".to_string())]
    );
    // The heartbeat carried the cleared focus; the cloud merged it.
    assert_eq!(state.heartbeat_metadata.last().unwrap()[META_FOCUS], json!(""));
    drop(state);
    assert_eq!(peer_ids(&fx.peer_root, "someone-else"), vec![(me.to_string(), None)]);
}

/// The real factory sequence (daring-eagle-74's repro at c25f0e3db): `cas
/// serve` registers the supervisor eagerly from CAS_SESSION_ID and hands the
/// id to its daemon, then the harness's SessionStart arrives for the same
/// session. Exactly one cloud registration must result, carrying the project
/// and factory session; a later heartbeat 404 registers again.
#[tokio::test]
async fn cas_f707_eager_supervisor_then_session_start_registers_once() {
    let (mut fx, daemon) = fixture("cas-571d").await;
    fx._env.set("CAS_AGENT_ROLE", "supervisor");
    let me = "sup-f707-factory";
    let daemon = Arc::new(daemon);
    let core = crate::mcp::server::CasCore::with_daemon(
        fx.cas_root.clone(),
        None,
        Some(Arc::clone(&daemon)),
    );

    // mcp/server/runtime.rs eager registration: the row, then set_agent_id.
    core.register_agent(me.to_string(), "zen-viper-42".to_string(), None)
        .unwrap();
    daemon.set_agent_id(me.to_string()).await;
    daemon.await_cloud_presence().await;
    let agent = crate::store::open_agent_store(&fx.cas_root)
        .unwrap()
        .get(me)
        .unwrap();
    assert_eq!(agent.role, AgentRole::Supervisor);

    // The harness's SessionStart for the same session.
    session_start(&daemon, me, "zen-viper-42").await;
    daemon.await_cloud_presence().await;

    {
        let state = fx.cloud.state();
        assert_eq!(state.registrations.len(), 1, "{:?}", state.registrations);
        let body = &state.registrations[0];
        assert_eq!(body["id"], json!(me));
        assert_eq!(body[PROJECT_SCOPE_KEY], json!(REPO));
        assert_eq!(body["metadata"][META_CANONICAL_ID], json!(REPO));
        assert_eq!(body["metadata"][META_FACTORY_SESSION], json!(SESSION));
        assert_eq!(body["metadata"]["role"], json!("supervisor"));
    }
    assert_eq!(
        peer_ids(&fx.peer_root, "someone-else"),
        vec![(me.to_string(), Some("cas-571d".to_string()))]
    );

    // The cloud loses it: the next heartbeat's 404 registers it again.
    fx.cloud.state().agents.clear();
    heartbeat_tick(&daemon).await;
    assert_eq!(fx.cloud.state().heartbeats.last(), Some(&(me.to_string(), 404)));
    assert_eq!(fx.cloud.state().registrations.len(), 2);
    assert_eq!(peer_ids(&fx.peer_root, "someone-else").len(), 1);
}

#[tokio::test]
async fn cas_f707_branch_is_registered_and_refreshed_on_the_heartbeat() {
    let (fx, daemon) = fixture("cas-571d").await;
    // The project root (the parent of .cas) is a checkout on an epic branch.
    let root = fx.cas_root.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/epic/same-repo\n").unwrap();
    let me = "sup-f707-branch";
    session_start(&daemon, me, "branch-sup").await;
    assert_eq!(
        fx.cloud.state().registrations[0]["metadata"][META_BRANCH],
        json!("epic/same-repo")
    );

    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/factory/next\n").unwrap();
    heartbeat_tick(&daemon).await;
    let record = presence::load(&fx.cas_root, me).unwrap();
    assert_eq!(record.branch.as_deref(), Some("factory/next"));
    let state = fx.cloud.state();
    assert_eq!(state.registrations.len(), 1, "a branch change rides the heartbeat");
    assert_eq!(
        state.heartbeat_metadata.last().unwrap()[META_BRANCH],
        json!("factory/next")
    );
    assert_eq!(state.agents[me]["metadata"][META_BRANCH], json!("factory/next"));
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

/// An explicit `cloud.auto_sync = false` keeps the project's agents out of
/// the registry, says so, and calls discovery one-way; turning it back on
/// (the default) registers again.
#[tokio::test]
async fn cas_f707_auto_sync_false_opts_out_of_presence_and_says_so() {
    let (fx, _) = fixture("cas-571d").await;
    let set_auto_sync = |value: &str| {
        let mut config = crate::config::Config::load(&fx.cas_root).unwrap();
        config.set("cloud.auto_sync", value).unwrap();
        config.save(&fx.cas_root).unwrap();
    };
    set_auto_sync("false");
    // The daemon as `cas serve` builds it for this project.
    let daemon = EmbeddedDaemon::new(EmbeddedDaemonConfig {
        cas_root: fx.cas_root.clone(),
        index_code: false,
        cloud_sync_enabled: false,
        ..Default::default()
    });
    let me = "sup-f707-optout";
    session_start(&daemon, me, "private-sup").await;
    heartbeat_tick(&daemon).await;
    assert!(presence::push_agent(&fx.cas_root, me).is_none());
    {
        let state = fx.cloud.state();
        assert!(state.registrations.is_empty(), "{:?}", state.registrations);
        assert!(state.heartbeats.is_empty(), "{:?}", state.heartbeats);
    }
    let shown = crate::cloud::peers::peers_section(&fx.cas_root, Some(me));
    assert!(
        shown.starts_with("Cloud presence: disabled (cloud.auto_sync=false)"),
        "{shown}"
    );
    assert!(shown.contains("discovery is one-way"), "{shown}");
    // `cas status` without a known caller says the same.
    let anonymous = crate::cloud::peers::peers_section(&fx.cas_root, None);
    assert!(anonymous.starts_with("Cloud presence: disabled"), "{anonymous}");

    // Back on: the default path registers.
    set_auto_sync("true");
    let daemon = EmbeddedDaemon::new(EmbeddedDaemonConfig {
        cas_root: fx.cas_root.clone(),
        index_code: false,
        ..Default::default()
    });
    session_start(&daemon, me, "private-sup").await;
    assert_eq!(fx.cloud.state().registrations.len(), 1);
    let shown = crate::cloud::peers::peers_section(&fx.cas_root, Some(me));
    assert!(shown.starts_with("Cloud presence: registered"), "{shown}");
}

/// cas-66c7: rewrite the project's cloud.json token as `cas login` (or a
/// rotation) would, leaving the endpoint as it is.
fn write_token(cas_root: &Path, token: Option<&str>) {
    let mut config = CloudConfig::load_from_cas_dir(cas_root).unwrap();
    config.token = token.map(str::to_string);
    config.save_to_cas_dir(cas_root).unwrap();
}

fn daemon_for(cas_root: &Path) -> EmbeddedDaemon {
    EmbeddedDaemon::new(EmbeddedDaemonConfig {
        cas_root: cas_root.to_path_buf(),
        index_code: false,
        ..Default::default()
    })
}

/// A factory started before `cas login` registers on the first heartbeat
/// after the token appears, without a restart.
#[tokio::test]
async fn cas_66c7_a_late_login_registers_without_a_restart() {
    let (fx, _) = fixture("cas-571d").await;
    write_token(&fx.cas_root, None);
    let daemon = daemon_for(&fx.cas_root);
    let me = "sup-66c7-late";
    session_start(&daemon, me, "late-sup").await;
    heartbeat_tick(&daemon).await;
    assert!(fx.cloud.state().registrations.is_empty(), "logged out: nothing sent");

    write_token(&fx.cas_root, Some("synthetic-test-token"));
    heartbeat_tick(&daemon).await;
    assert_eq!(fx.cloud.state().registrations.len(), 1, "registered within one tick");
    assert_eq!(
        presence::load(&fx.cas_root, me).unwrap().state,
        PresenceState::Registered
    );
    assert_eq!(
        peer_ids(&fx.peer_root, "someone-else"),
        vec![(me.to_string(), Some("cas-571d".to_string()))]
    );
}

/// A factory started with an invalid token re-registers once the token is
/// fixed (or rotated), without a restart, and stops using the old one.
#[tokio::test]
async fn cas_66c7_a_fixed_or_rotated_token_reregisters_without_a_restart() {
    let (fx, _) = fixture("cas-571d").await;
    fx.cloud.state().valid_token = Some("rotated-token".to_string());
    write_token(&fx.cas_root, Some("stale-token"));
    let daemon = daemon_for(&fx.cas_root);
    let me = "sup-66c7-rotated";
    session_start(&daemon, me, "rotated-sup").await;
    let record = presence::load(&fx.cas_root, me).unwrap();
    assert_eq!(record.state, PresenceState::Failing);
    assert!(record.cause.unwrap_or_default().contains("401"));

    write_token(&fx.cas_root, Some("rotated-token"));
    let rejected_before = fx.cloud.state().rejected_tokens.len();
    heartbeat_tick(&daemon).await;
    let record = presence::load(&fx.cas_root, me).unwrap();
    assert_eq!(record.state, PresenceState::Registered, "{record:?}");
    let state = fx.cloud.state();
    assert_eq!(
        state.rejected_tokens.len(),
        rejected_before,
        "no request after the fix used the stale token"
    );
    assert_eq!(state.agents.len(), 1);
}

/// Cloud sync also follows the login: a daemon started logged out syncs
/// after `cas login`, and a rotated token is used from the next cycle.
#[tokio::test]
async fn cas_66c7_cloud_sync_follows_a_late_login_and_a_rotated_token() {
    use wiremock::matchers::{method, path};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        // The syncer's own constant: the cas-2eb3 guard allows one pull URL.
        .and(path(crate::cloud::syncer::pull::PULL_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "entries": [], "tasks": [], "rules": [], "skills": [],
            "specs": [], "events": [], "prompts": [],
            "file_changes": [], "commit_links": [],
            "pulled_at": chrono::Utc::now().to_rfc3339(),
        })))
        .mount(&server)
        .await;
    let mut env = TestEnvGuard::temp_home();
    let home = env.home().to_path_buf();
    env.set("CAS_USER_CLOUD_JSON", home.join("no-user-cloud.json"));
    let cas_root = init_cas_dir(&home).unwrap();
    crate::cloud::set_canonical_id_in_config_toml(&cas_root, REPO).unwrap();
    let mut config = CloudConfig::default();
    config.endpoint = server.uri();
    config.token = None;
    config.save_to_cas_dir(&cas_root).unwrap();

    let daemon = daemon_for(&cas_root);
    assert!(daemon.trigger_cloud_sync().await.is_err(), "logged out: no syncer");

    let tokens_since = |from: usize| {
        let server = &server;
        async move {
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .skip(from)
                .map(|request| {
                    request
                        .headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string()
                })
                .collect::<Vec<_>>()
        }
    };

    write_token(&cas_root, Some("first-token"));
    daemon.trigger_cloud_sync().await.unwrap();
    let after_login = tokens_since(0).await;
    assert!(!after_login.is_empty(), "the late login syncs");
    assert!(after_login.iter().all(|t| t == "Bearer first-token"), "{after_login:?}");

    write_token(&cas_root, Some("rotated-token"));
    daemon.trigger_cloud_sync().await.unwrap();
    let after_rotation = tokens_since(after_login.len()).await;
    assert!(!after_rotation.is_empty());
    assert!(
        after_rotation.iter().all(|t| t == "Bearer rotated-token"),
        "{after_rotation:?}"
    );
}

#[tokio::test]
async fn cas_4ac8_session_end_is_local_and_next_start_drains_after_restart() {
    use wiremock::matchers::{method, path};
    let (fx, daemon) = fixture("cas-571d").await;
    session_start(&daemon, "ending-4ac8", "ending").await;
    daemon.await_cloud_presence().await;
    Mock::given(method("POST"))
        .and(path("/api/agents/ending-4ac8/shutdown"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
        .up_to_n_times(1)
        .with_priority(1)
        .expect(1)
        .mount(&fx._server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/agents/ending-4ac8/shutdown"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"released_locks":0})))
        .with_priority(2)
        .expect(1)
        .mount(&fx._server)
        .await;
    let root = fx.cas_root.clone();
    let start = std::time::Instant::now();
    tokio::task::spawn_blocking(move || {
        crate::hooks::handlers::handle_session_end(
            &crate::hooks::HookInput {
                session_id: "ending-4ac8".into(),
                ..Default::default()
            },
            Some(&root),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(
        start.elapsed() < std::time::Duration::from_secs(1),
        "SessionEnd waited for Cloud"
    );
    assert!(
        fx._server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| !request.url.path().ends_with("/shutdown"))
    );
    let pending = crate::cloud::peers::presence_line(&fx.cas_root, Some("ending-4ac8"));
    assert!(pending.contains("shutdown pending"), "{pending}");
    drop(daemon);
    // A fresh daemon/session must recover the file without the ended local row.
    let restarted = EmbeddedDaemon::new(EmbeddedDaemonConfig {
        cas_root: fx.cas_root.clone(),
        index_code: false,
        ..Default::default()
    });
    session_start(&restarted, "successor-4ac8", "successor").await;
    restarted.await_cloud_presence().await;
    let health = crate::cloud::peers::presence_line(&fx.cas_root, Some("successor-4ac8"));
    assert!(!health.contains("shutdown pending"), "{health}");
    assert_eq!(
        fx._server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path().ends_with("/shutdown"))
            .count(),
        2
    );
}

#[tokio::test]
async fn cas_4ac8_heartbeat_without_owned_agent_drains_without_resurrection() {
    use wiremock::matchers::{method, path};
    let (fx, daemon) = fixture("cas-571d").await;
    session_start(&daemon, "ending-heartbeat", "ending").await;
    daemon.await_cloud_presence().await;
    Mock::given(method("POST"))
        .and(path("/api/agents/ending-heartbeat/shutdown"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"released_locks":0})))
        .with_priority(1)
        .expect(1)
        .mount(&fx._server)
        .await;
    let root = fx.cas_root.clone();
    tokio::task::spawn_blocking(move || {
        crate::hooks::handlers::handle_session_end(
            &crate::hooks::HookInput {
                session_id: "ending-heartbeat".into(),
                ..Default::default()
            },
            Some(&root),
        )
    })
    .await
    .unwrap()
    .unwrap();
    daemon
        .handle_socket_event(DaemonEvent::SessionEnd {
            session_id: "ending-heartbeat".into(),
            cc_pid: Some(our_cc_pid()),
        })
        .await;
    assert!(daemon.agent_id.read().await.is_none());
    heartbeat_tick(&daemon).await;
    heartbeat_tick(&daemon).await;
    assert!(daemon.agent_id.read().await.is_none());
    assert!(
        crate::store::open_agent_store(&fx.cas_root)
            .unwrap()
            .get("ending-heartbeat")
            .is_err()
    );
    assert!(!crate::cloud::peers::presence_line(&fx.cas_root, None).contains("shutdown pending"));
    let requests = fx._server.received_requests().await.unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.url.path().ends_with("/shutdown"))
            .count(),
        1
    );
    assert_eq!(
        fx.cloud
            .state()
            .registrations
            .iter()
            .filter(|body| body["id"] == "ending-heartbeat")
            .count(),
        1
    );
}

#[tokio::test]
async fn cas_4ac8_session_end_does_not_wait_for_slow_cloud() {
    use wiremock::matchers::{method, path};
    let (fx, daemon) = fixture("cas-571d").await;
    session_start(&daemon, "ending-slow", "ending").await;
    daemon.await_cloud_presence().await;
    Mock::given(method("POST"))
        .and(path("/api/agents/ending-slow/shutdown"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(10)))
        .with_priority(1)
        .expect(0)
        .mount(&fx._server)
        .await;
    let root = fx.cas_root.clone();
    let started = std::time::Instant::now();
    tokio::task::spawn_blocking(move || {
        crate::hooks::handlers::handle_session_end(
            &crate::hooks::HookInput {
                session_id: "ending-slow".into(),
                ..Default::default()
            },
            Some(&root),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert!(crate::cloud::peers::presence_line(&fx.cas_root, None).contains("shutdown pending"));
}

#[tokio::test]
async fn cas_4ac8_session_start_replies_before_slow_cleanup() {
    use wiremock::matchers::{method, path};
    let (fx, daemon) = fixture("cas-571d").await;
    session_start(&daemon, "ending-fast-reply", "ending").await;
    daemon.await_cloud_presence().await;
    Mock::given(method("POST"))
        .and(path("/api/agents/ending-fast-reply/shutdown"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"released_locks": 0}))
            .set_delay(std::time::Duration::from_secs(1)))
        .with_priority(1).expect(1).mount(&fx._server).await;
    let root = fx.cas_root.clone();
    tokio::task::spawn_blocking(move || crate::hooks::handlers::handle_session_end(
        &crate::hooks::HookInput {session_id: "ending-fast-reply".into(), ..Default::default()}, Some(&root)))
        .await.unwrap().unwrap();
    let started = std::time::Instant::now();
    // Use the real handler without the fixture helper's eventual-cloud wait.
    daemon.handle_socket_event(DaemonEvent::SessionStart {
        session_id: "successor-fast-reply".into(), agent_name: Some("successor".into()),
        agent_role: Some("supervisor".into()), cc_pid: our_cc_pid(), clone_path: None,
    }).await;
    assert!(started.elapsed() < std::time::Duration::from_millis(500), "reply waited {:?}", started.elapsed());
    daemon.await_cloud_presence().await;
    assert_eq!(fx.cloud.state().registrations.len(), 2);
}

#[tokio::test]
async fn cas_4ac8_same_session_resume_can_end_again() {
    use wiremock::matchers::{method, path};
    let (fx, daemon) = fixture("cas-571d").await;
    let id = "same-session-4ac8";
    Mock::given(method("POST"))
        .and(path(format!("/api/agents/{id}/shutdown")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"released_locks": 0})))
        .with_priority(1).expect(2).mount(&fx._server).await;
    session_start(&daemon, id, "resuming").await;
    let original = crate::store::open_agent_store(&fx.cas_root).unwrap().get(id).unwrap();
    for incarnation in 0..2 {
        if incarnation == 1 {
            let resumed = daemon_for(&fx.cas_root);
            session_start(&resumed, id, "resuming").await;
            let row = crate::store::open_agent_store(&fx.cas_root).unwrap().get(id).unwrap();
            assert!(row.registered_at > original.registered_at);
            assert!(!crate::cloud::shutdown::is_ended(&fx.cas_root, &row).unwrap());
            assert_eq!(fx.cloud.state().registrations.len(), 2);
        }
        let root = fx.cas_root.clone();
        tokio::task::spawn_blocking(move || crate::hooks::handlers::handle_session_end(
            &crate::hooks::HookInput { session_id: id.into(), ..Default::default() }, Some(&root)))
            .await.unwrap().unwrap();
        let root = fx.cas_root.clone();
        let endpoint = fx._server.uri();
        tokio::task::spawn_blocking(move || {
            let coordinator = crate::cloud::CloudCoordinator::new(CloudConfig {
                endpoint, token: Some("synthetic-test-token".into()), ..Default::default()
            }).unwrap();
            crate::cloud::shutdown::drain(&root, &coordinator, chrono::Utc::now()).unwrap();
        }).await.unwrap();
        let requests = fx._server.received_requests().await.unwrap();
        assert_eq!(requests.iter().filter(|r| r.url.path().ends_with("/shutdown")).count(), incarnation + 1);
    }
}

#[tokio::test]
async fn cas_4ac8_round3_clear_auto_reinsert_stays_ended() {
    use wiremock::matchers::{method, path};
    let (fx, daemon) = fixture("cas-571d").await;
    let daemon = Arc::new(daemon);
    let id = "clear-auto-reinsert";
    session_start(&daemon, id, "clearing").await;
    Mock::given(method("POST")).and(path(format!("/api/agents/{id}/shutdown")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"released_locks": 0})))
        .with_priority(1).expect(1).mount(&fx._server).await;
    let root = fx.cas_root.clone();
    tokio::task::spawn_blocking(move || crate::hooks::handlers::handle_session_end(
        &crate::hooks::HookInput {session_id: id.into(), ..Default::default()}, Some(&root)))
        .await.unwrap().unwrap();
    let core = crate::mcp::server::CasCore::with_daemon(fx.cas_root.clone(), None, Some(daemon.clone()));
    core.ensure_agent_active(id).unwrap();
    daemon.await_cloud_presence().await;
    let row = crate::store::open_agent_store(&fx.cas_root).unwrap().get(id).unwrap();
    assert!(row.cc_session_id.is_none());
    assert!(crate::cloud::shutdown::is_ended(&fx.cas_root, &row).unwrap());
    assert_eq!(fx.cloud.state().registrations.len(), 1, "automatic reinsertion must not register with Cloud");
}

#[tokio::test]
async fn cas_4ac8_round3_failed_unregister_real_start_is_new_incarnation() {
    let (fx, daemon) = fixture("cas-571d").await;
    let id = "failed-delete-resume";
    session_start(&daemon, id, "resuming").await;
    let agents = crate::store::open_agent_store(&fx.cas_root).unwrap();
    let old = agents.get(id).unwrap();
    let db = rusqlite::Connection::open(fx.cas_root.join("cas.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_agent_delete BEFORE DELETE ON agents BEGIN SELECT RAISE(FAIL, 'injected delete failure'); END;").unwrap();
    let root = fx.cas_root.clone();
    tokio::task::spawn_blocking(move || crate::hooks::handlers::handle_session_end(
        &crate::hooks::HookInput {session_id: id.into(), ..Default::default()}, Some(&root)))
        .await.unwrap().unwrap();
    assert!(agents.get(id).is_ok(), "fixture must retain the failed-delete row");
    let root = fx.cas_root.clone();
    let result = tokio::task::spawn_blocking(move || crate::hooks::handlers::cleanup_agent_leases(&root, id)).await.unwrap();
    assert!(result.is_none(), "unregister failure must be reported, not returned as successful cleanup");
    let resumed = daemon_for(&fx.cas_root);
    session_start(&resumed, id, "resuming").await;
    let current = agents.get(id).unwrap();
    assert!(current.registered_at > old.registered_at, "real SessionStart must refresh the retained shutdown row");
    assert!(!crate::cloud::shutdown::is_ended(&fx.cas_root, &current).unwrap());
}

#[tokio::test]
async fn cas_4ac8_round3_session_start_burst_keeps_pending_registrations() {
    use wiremock::matchers::{method, path};
    let (fx, daemon) = fixture("cas-571d").await;
    let cloud = fx.cloud.clone();
    Mock::given(method("POST")).and(path("/api/agents/register"))
        .respond_with(move |request: &Request| cloud.respond(request).set_delay(std::time::Duration::from_millis(300)))
        .up_to_n_times(1).with_priority(1).mount(&fx._server).await;
    for id in ["burst-one", "burst-two", "burst-three"] {
        daemon.handle_socket_event(DaemonEvent::SessionStart {
            session_id: id.into(), agent_name: Some(id.into()), agent_role: Some("supervisor".into()),
            cc_pid: our_cc_pid(), clone_path: None,
        }).await;
    }
    daemon.await_cloud_presence().await;
    let state = fx.cloud.state();
    assert_eq!(state.registrations.len(), 3);
    for id in ["burst-one", "burst-two", "burst-three"] {
        assert!(state.agents.contains_key(id), "lost pending SessionStart for {id}");
    }
}

#[tokio::test]
async fn cas_4ac8_round3_resume_resets_failing_backoff() {
    let (fx, daemon) = fixture("cas-571d").await;
    let id = "resume-backoff";
    fx.cloud.state().reject_registrations = 2;
    session_start(&daemon, id, "failing").await;
    heartbeat_tick(&daemon).await;
    let record = presence::load(&fx.cas_root, id).unwrap();
    assert!(record.next_retry_at.unwrap() > chrono::Utc::now());
    let root = fx.cas_root.clone();
    tokio::task::spawn_blocking(move || crate::hooks::handlers::handle_session_end(
        &crate::hooks::HookInput {session_id: id.into(), ..Default::default()}, Some(&root)))
        .await.unwrap().unwrap();
    let resumed = daemon_for(&fx.cas_root);
    session_start(&resumed, id, "resuming").await;
    assert_eq!(presence::load(&fx.cas_root, id).unwrap().state, PresenceState::Registered);
}

#[tokio::test]
async fn cas_4ac8_round3_heartbeat_kicks_once_with_and_without_owned_agent() {
    let (fx, daemon) = fixture("cas-571d").await;
    let before = daemon.presence_kick_count.load(Ordering::Relaxed);
    heartbeat_tick(&daemon).await;
    assert_eq!(daemon.presence_kick_count.load(Ordering::Relaxed) - before, 1);
    session_start(&daemon, "one-kick", "supervisor").await;
    let before = daemon.presence_kick_count.load(Ordering::Relaxed);
    let requests = fx.cloud.state().heartbeats.len();
    heartbeat_tick(&daemon).await;
    assert_eq!(daemon.presence_kick_count.load(Ordering::Relaxed) - before, 1);
    assert_eq!(fx.cloud.state().heartbeats.len() - requests, 1);
}

#[tokio::test]
async fn cas_4ac8_round4_panicked_presence_pass_does_not_disable_later_kick() {
    let (fx, daemon) = fixture("cas-571d").await;
    // A real failed blocking pass leaves the same pending/handle state as an
    // unexpected panic in store/presence work. The next public heartbeat recovers.
    *daemon.presence_pending.lock().unwrap() = Some(Default::default());
    let failed = tokio::task::spawn_blocking(|| panic!("injected presence pass panic"));
    while !failed.is_finished() { tokio::task::yield_now().await; }
    *daemon.presence_task.lock().await = Some(failed);
    let id = "panic-recovery";
    let agent = crate::types::Agent::new(id.into(), id.into());
    crate::store::open_agent_store(&fx.cas_root).unwrap().register(&agent).unwrap();
    daemon.kick_cloud_presence(id.into()).await;
    tokio::time::timeout(std::time::Duration::from_secs(2), daemon.await_cloud_presence()).await
        .expect("a panicked pass must not leave presence permanently pending");
    assert!(fx.cloud.state().agents.contains_key(id));
}

#[tokio::test]
async fn cas_4ac8_round4_unicode_id_presence_completes() {
    let (fx, daemon) = fixture("cas-571d").await;
    let id = "ééé中-agent";
    let agent = crate::types::Agent::new(id.into(), id.into());
    crate::store::open_agent_store(&fx.cas_root).unwrap().register(&agent).unwrap();
    daemon.kick_cloud_presence(id.into()).await;
    tokio::time::timeout(std::time::Duration::from_secs(2), daemon.await_cloud_presence()).await
        .expect("Unicode id must not panic in presence logging");
    assert!(fx.cloud.state().agents.contains_key(id));
}

#[tokio::test]
async fn cas_4ac8_round4_resume_after_failed_shutdown_or_revive() {
    let (fx, daemon) = fixture("cas-571d").await;
    for failure in ["shutdown-fails", "revived", "auto-active"] {
        let id = format!("resume-{failure}");
        let initial = daemon_for(&fx.cas_root);
        session_start(&initial, &id, &id).await;
        let agents = crate::store::open_agent_store(&fx.cas_root).unwrap();
        let old = agents.get(&id).unwrap();
        let db = rusqlite::Connection::open(fx.cas_root.join("cas.db")).unwrap();
        db.execute_batch("CREATE TRIGGER fail_delete BEFORE DELETE ON agents BEGIN SELECT RAISE(FAIL, 'injected delete failure'); END;").unwrap();
        if failure == "shutdown-fails" {
            db.execute_batch("CREATE TRIGGER fail_shutdown BEFORE UPDATE OF status ON agents WHEN NEW.status = 'shutdown' BEGIN SELECT RAISE(FAIL, 'injected shutdown failure'); END;").unwrap();
        }
        let root = fx.cas_root.clone();
        let ended = id.clone();
        tokio::task::spawn_blocking(move || crate::hooks::handlers::cleanup_agent_leases(&root, &ended)).await.unwrap();
        if failure == "shutdown-fails" { db.execute_batch("DROP TRIGGER fail_shutdown;").unwrap(); }
        if failure == "revived" { agents.revive(&id).unwrap(); }
        if failure == "auto-active" {
            let mut automatic = crate::types::Agent::new(id.clone(), id.clone());
            automatic.cc_session_id = None;
            agents.register(&automatic).unwrap();
        }
        db.execute_batch("DROP TRIGGER fail_delete;").unwrap();
        assert_eq!(agents.get(&id).unwrap().status, crate::types::AgentStatus::Active);
        assert!(crate::cloud::shutdown::is_ended(&fx.cas_root, &agents.get(&id).unwrap()).unwrap());
        let resumed = daemon_for(&fx.cas_root);
        session_start(&resumed, &id, &id).await;
        let current = agents.get(&id).unwrap();
        assert!(current.registered_at > old.registered_at, "{failure}: resume must advance the intent-owned incarnation");
        assert!(!crate::cloud::shutdown::is_ended(&fx.cas_root, &current).unwrap(), "{failure}");
    }
    drop(daemon);
}

#[tokio::test]
async fn cas_4ac8_round4_delete_failure_preserves_interruption_note() {
    let (fx, daemon) = fixture("cas-571d").await;
    let id = "interrupted-delete";
    session_start(&daemon, id, id).await;
    let tasks = crate::store::open_task_store(&fx.cas_root).unwrap();
    let mut task = Task::new("cas-interrupted".into(), "Work interrupted by SessionEnd".into());
    task.status = crate::types::TaskStatus::InProgress;
    tasks.add(&task).unwrap();
    let agents = crate::store::open_agent_store(&fx.cas_root).unwrap();
    agents.try_claim(&task.id, id, 300, None).unwrap();
    assert!(!agents.list_agent_leases(id).unwrap().is_empty());
    let db = rusqlite::Connection::open(fx.cas_root.join("cas.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_delete BEFORE DELETE ON agents BEGIN SELECT RAISE(FAIL, 'injected delete failure'); END;").unwrap();
    let root = fx.cas_root.clone();
    let result = tokio::task::spawn_blocking(move || crate::hooks::handlers::cleanup_agent_leases(&root, id)).await.unwrap();
    assert!(result.is_none());
    assert!(agents.list_agent_leases(id).unwrap().is_empty());
    assert!(tasks.get(&task.id).unwrap().notes.contains("INTERRUPTED"), "released work needs its interruption note even if row deletion fails");
}

#[tokio::test]
async fn cas_4ac8_round4_unicode_session_start_and_heartbeat_complete() {
    let (fx, daemon) = fixture("cas-571d").await;
    let id = "ééé中-session";
    session_start(&daemon, id, "unicode-session").await;
    heartbeat_tick(&daemon).await;
    assert!(fx.cloud.state().agents.contains_key(id));
    assert_eq!(fx.cloud.state().heartbeats.len(), 1);
}

/// A surviving MCP core is still bound to the pre-clear session. Its identity
/// recovery must not replace the daemon's newer hook-announced heartbeat owner.
#[tokio::test]
async fn cas_9735_old_core_after_clear_keeps_new_session_heartbeat() {
    for retained in [false, true] {
        let (mut fx, daemon) = fixture("cas-9735").await;
        fx._env.set("CAS_AGENT_ROLE", "supervisor");
        let daemon = Arc::new(daemon);
        let old_id = "pre-clear-session";
        let new_id = "post-clear-session";
        session_start(&daemon, old_id, "supervisor").await;
        let cloud = fx.cloud.clone();
        Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(format!("/api/agents/{old_id}/shutdown")))
            .respond_with(move |_: &Request| {
                cloud.state().agents.remove(old_id);
                ResponseTemplate::new(200).set_body_json(json!({"released_locks":0}))
            }).with_priority(1).expect(1).mount(&fx._server).await;
        let store = crate::store::open_agent_store(&fx.cas_root).unwrap();
        let core = crate::mcp::server::CasCore::with_daemon(
            fx.cas_root.clone(), None, Some(Arc::clone(&daemon)),
        );
        core.bind_agent_identity(old_id.into(), crate::mcp::server::AgentIdentitySource::ServerInternal).unwrap();
        let service = crate::mcp::tools::CasService::new(core,
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        daemon.handle_socket_event(DaemonEvent::SessionEnd {
            session_id: old_id.into(), cc_pid: Some(our_cc_pid()),
        }).await;
        let db = rusqlite::Connection::open(fx.cas_root.join("cas.db")).unwrap();
        if retained {
            db.execute_batch("CREATE TRIGGER fail_old_delete BEFORE DELETE ON agents WHEN OLD.id='pre-clear-session' BEGIN SELECT RAISE(FAIL, 'retained cleanup fixture'); END;").unwrap();
        }
        let root = fx.cas_root.clone();
        let cleaned = tokio::task::spawn_blocking(move || crate::hooks::handlers::cleanup_agent_leases(&root, old_id)).await.unwrap();
        if retained {
            assert!(cleaned.is_none());
            db.execute_batch("DROP TRIGGER fail_old_delete").unwrap();
        } else {
            assert!(cleaned.is_some());
            assert!(store.get(old_id).is_err());
        }
        session_start(&daemon, new_id, "supervisor").await;
        daemon.await_cloud_presence().await;
        assert_eq!(daemon.agent_id.read().await.as_deref(), Some(new_id));
        let result = service.coordination(rmcp::handler::server::wrapper::Parameters(
            serde_json::from_value::<cas_mcp::types::CoordinationRequest>(json!({"action":"whoami"})).unwrap(),
        )).await;
        assert!(result.is_ok(), "retained={retained}: {result:?}");
        // The single-thread runtime schedules the recovery task before this
        // barrier; then settle its real HTTP presence work before observing.
        tokio::spawn(async {}).await.unwrap();
        daemon.await_cloud_presence().await;
        assert_eq!(daemon.agent_id.read().await.as_deref(), Some(new_id), "old-core recovery stole heartbeat ownership (retained={retained})");
        let recovered = store.get(old_id).unwrap();
        let mut shutdown = crate::cloud::shutdown::Snapshot::load(&fx.cas_root);
        shutdown.refresh_after_lease(&fx.cas_root);
        assert!(shutdown.is_ended(&fx.cas_root, &recovered, chrono::Utc::now()).unwrap(), "automatic recovery must not remove S's fence");
        let before = store.get(new_id).unwrap().last_heartbeat;
        heartbeat_tick(&daemon).await;
        assert!(store.get(new_id).unwrap().last_heartbeat > before);
        assert_eq!(daemon.agent_id.read().await.as_deref(), Some(new_id));
        assert_eq!(fx.cloud.state().heartbeats.last().map(|(id, _)| id.as_str()), Some(new_id));
        assert!(!fx.cloud.state().agents.contains_key(old_id), "S's cleanup must complete");
        assert_eq!(fx.cloud.state().registrations.iter().filter(|row| row["id"] == old_id).count(), 1, "old core must not resurrect S in Cloud");
        fx._server.verify().await; // The real drain completed exactly one shutdown.
        assert!(crate::cloud::shutdown::is_ended(&fx.cas_root, &store.get(old_id).unwrap()).unwrap());
    }
}
