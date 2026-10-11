//! cas-5f28: cross-machine claim awareness.
//!
//! Two supervisors of one repository run two Cassy databases (two machines,
//! or two clones). Their local leases cannot see each other, so `task start`
//! also claims the task in Cassy Cloud. These tests drive the real MCP
//! surface of two databases against a fake cloud that serves the same
//! `/api/agents/tasks/<id>/{claim,release,renew,lock}` contract as
//! petra-stella-cloud, including its current flaw (a claim ignores
//! `expires_at`, petra-stella-cloud#150).

use crate::support::*;
use cas::mcp::CasCore;
use cas::mcp::tools::*;
use cas::store::{open_agent_store, open_store, open_task_store};
use cas::types::{Agent, AgentRole, DependencyType, Task, TaskStatus, TaskType};
use chrono::{DateTime, Duration, Utc};
use rmcp::handler::server::wrapper::Parameters;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const PROJECT: &str = "github.com/acme/widget";

#[derive(Clone, Debug)]
struct FakeLock {
    agent_id: String,
    status: String,
    expires_at: DateTime<Utc>,
    renewed_at: DateTime<Utc>,
    renewal_count: u32,
    claim_reason: Option<String>,
}

#[derive(Default)]
struct FakeState {
    locks: HashMap<String, FakeLock>,
    requests: Vec<String>,
}

/// A stateful stand-in for petra-stella-cloud's task-lock routes.
struct FakeCloud {
    url: String,
    state: Arc<Mutex<FakeState>>,
    server: Arc<tiny_http::Server>,
}

impl Drop for FakeCloud {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

fn lock_json(key: &str, lock: &FakeLock) -> serde_json::Value {
    serde_json::json!({
        "id": format!("lock-{key}"),
        "task_id": key,
        "agent_id": lock.agent_id,
        "status": lock.status,
        "expires_at": lock.expires_at.to_rfc3339(),
        "renewed_at": lock.renewed_at.to_rfc3339(),
        "renewal_count": lock.renewal_count,
        "epoch": 1,
        "claim_reason": lock.claim_reason,
    })
}

impl FakeCloud {
    fn start() -> Self {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("fake cloud binds"));
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let state = Arc::new(Mutex::new(FakeState::default()));
        let (thread_server, thread_state) = (Arc::clone(&server), Arc::clone(&state));
        std::thread::spawn(move || {
            for mut request in thread_server.incoming_requests() {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let (status, json) =
                    Self::handle(&thread_state, request.method().as_str(), request.url(), &body);
                let response = tiny_http::Response::from_string(json.to_string())
                    .with_status_code(status)
                    .with_header(
                        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                            .unwrap(),
                    );
                let _ = request.respond(response);
            }
        });
        Self { url, state, server }
    }

    fn handle(
        state: &Mutex<FakeState>,
        method: &str,
        url: &str,
        body: &str,
    ) -> (u16, serde_json::Value) {
        let mut state = state.lock().unwrap();
        state.requests.push(format!("{method} {url}"));
        let body: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
        let Some(rest) = url.strip_prefix("/api/agents/tasks/") else {
            // Registration, heartbeats, sync: accepted and ignored.
            return (404, serde_json::json!({"error": "not found"}));
        };
        let (key, action) = rest.rsplit_once('/').unwrap_or((rest, ""));
        let key = key.to_string();
        let agent_id = body["agent_id"].as_str().unwrap_or_default().to_string();
        let now = Utc::now();
        let active = state.locks.get(&key).filter(|lock| lock.status == "active").cloned();
        match (method, action) {
            ("POST", "claim") => match active {
                // Like the real route: an active row is held whatever its expires_at.
                Some(lock) if lock.agent_id != agent_id => (
                    409,
                    serde_json::json!({"status": "conflict", "result": null, "lock": null,
                        "error": "already_claimed", "owner_agent_id": lock.agent_id}),
                ),
                Some(lock) => (
                    200,
                    serde_json::json!({"status": "success", "result": "claimed",
                        "lock": lock_json(&key, &lock), "error": null, "owner_agent_id": null}),
                ),
                None => {
                    let secs = body["duration_secs"].as_i64().unwrap_or(600).clamp(1, 3600);
                    let lock = FakeLock {
                        agent_id,
                        status: "active".into(),
                        expires_at: now + Duration::seconds(secs),
                        renewed_at: now,
                        renewal_count: 0,
                        claim_reason: body["reason"].as_str().map(str::to_string),
                    };
                    let json = lock_json(&key, &lock);
                    state.locks.insert(key, lock);
                    (200, serde_json::json!({"status": "success", "result": "claimed",
                        "lock": json, "error": null, "owner_agent_id": null}))
                }
            },
            ("POST", "release") => {
                if let Some(lock) = state.locks.get_mut(&key)
                    && lock.status == "active"
                    && lock.agent_id == agent_id
                {
                    lock.status = "released".into();
                }
                (200, serde_json::json!({"status": "success"}))
            }
            ("POST", "renew") => match state.locks.get_mut(&key) {
                Some(lock) if lock.status == "active" && lock.agent_id == agent_id => {
                    let secs = body["duration_secs"].as_i64().unwrap_or(600).clamp(1, 3600);
                    lock.expires_at = now + Duration::seconds(secs);
                    lock.renewed_at = now;
                    lock.renewal_count += 1;
                    let json = lock_json(&key, lock);
                    (200, serde_json::json!({"status": "success", "lock": json}))
                }
                _ => (404, serde_json::json!({"error": "No active lock found"})),
            },
            ("GET", "lock") => (
                200,
                serde_json::json!({"lock": active.map(|lock| lock_json(&key, &lock))}),
            ),
            _ => (404, serde_json::json!({"error": "not found"})),
        }
    }

    /// The single active lock whose key names `task_id`.
    fn active_lock(&self, task_id: &str) -> Option<(String, FakeLock)> {
        self.state
            .lock()
            .unwrap()
            .locks
            .iter()
            .find(|(key, lock)| key.starts_with(&format!("{task_id}~")) && lock.status == "active")
            .map(|(key, lock)| (key.clone(), lock.clone()))
    }

    fn keys(&self) -> Vec<String> {
        self.state.lock().unwrap().locks.keys().cloned().collect()
    }

    /// The holder died: its last renewal ran out.
    fn age_out(&self, task_id: &str) {
        let mut state = self.state.lock().unwrap();
        for (key, lock) in state.locks.iter_mut() {
            if key.starts_with(&format!("{task_id}~")) {
                lock.expires_at = Utc::now() - Duration::minutes(5);
            }
        }
    }
}

/// One machine: its own database, logged in to `cloud`, pinned to `project`.
struct Machine {
    _temp: tempfile::TempDir,
    cas_dir: PathBuf,
    core: CasCore,
    agent_id: String,
}

fn machine(endpoint: &str, project: &str, name: &str) -> Machine {
    let temp = tempfile::TempDir::new().unwrap();
    let cas_dir = temp.path().join(".cas");
    std::fs::create_dir_all(&cas_dir).unwrap();
    open_store(&cas_dir).unwrap().init().unwrap();
    open_task_store(&cas_dir).unwrap().init().unwrap();
    let agents = open_agent_store(&cas_dir).unwrap();
    agents.init().unwrap();
    std::fs::write(
        cas_dir.join("config.toml"),
        format!("[project]\ncanonical_id = {project:?}\n[verification]\nenabled = false\n"),
    )
    .unwrap();
    std::fs::write(
        cas_dir.join("cloud.json"),
        serde_json::json!({"endpoint": endpoint, "token": "test-token"}).to_string(),
    )
    .unwrap();
    let agent_id = format!("{name}-session-{}", std::process::id());
    agents
        .register(&Agent::new_with_role(agent_id.clone(), name.to_string(), AgentRole::Supervisor))
        .unwrap();
    let core = CasCore::with_daemon(cas_dir.clone(), None, None);
    core.set_agent_id_for_testing(agent_id.clone());
    Machine { _temp: temp, cas_dir, core, agent_id }
}

impl Machine {
    /// The same task as every other machine sees it after a cloud pull.
    fn put_task(&self, id: &str, task_type: TaskType) {
        let mut task = Task::new(id.to_string(), format!("shared {id}"));
        task.task_type = task_type;
        task.status = TaskStatus::Open;
        open_task_store(&self.cas_dir).unwrap().add(&task).unwrap();
    }

    fn child_of(&self, child: &str, epic: &str) {
        self.put_task(child, TaskType::Task);
        open_task_store(&self.cas_dir)
            .unwrap()
            .add_dependency(&cas::types::Dependency::new(
                child.to_string(),
                epic.to_string(),
                DependencyType::ParentChild,
            ))
            .unwrap();
    }

    fn second_agent(&self, name: &str) -> CasCore {
        let id = format!("{name}-session-{}", std::process::id());
        open_agent_store(&self.cas_dir)
            .unwrap()
            .register(&Agent::new_with_role(id.clone(), name.to_string(), AgentRole::Worker))
            .unwrap();
        let core = CasCore::with_daemon(self.cas_dir.clone(), None, None);
        core.set_agent_id_for_testing(id);
        core
    }
}

async fn start(core: &CasCore, id: &str, force: bool) -> Result<String, String> {
    core.cas_task_start_with_options(Parameters(TaskStartRequest {
        id: id.to_string(),
        brief: Some(true),
        force: force.then_some(true),
    }))
    .await
    .map(extract_text)
    .map_err(|error| error.message.to_string())
}

async fn release(core: &CasCore, id: &str) -> String {
    match core
        .cas_task_release(Parameters(TaskReleaseRequest { task_id: id.to_string(), force: None }))
        .await
    {
        Ok(result) => extract_text(result),
        Err(error) => error.message.to_string(),
    }
}

fn hostname() -> String {
    Agent::get_or_generate_machine_id()
}

/// Acceptance: B's start of a task A holds is refused with A's identity; once
/// A releases it, B starts it. The claim key is scoped to the repository.
#[tokio::test]
async fn peer_start_is_refused_with_the_holders_identity_until_it_releases() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    for m in [&alpha, &bravo] {
        m.put_task("cas-p001", TaskType::Task);
    }

    let started = start(&alpha.core, "cas-p001", false).await.expect("alpha starts");
    let (key, lock) = cloud.active_lock("cas-p001").expect("alpha's start claims in the cloud");
    assert_eq!(lock.agent_id, alpha.agent_id, "{started}");
    assert_ne!(key, "cas-p001", "the claim key carries the project scope");

    let refused = start(&bravo.core, "cas-p001", false).await.expect_err("bravo is refused");
    assert!(refused.contains("alpha-sup"), "{refused}");
    assert!(refused.contains(&hostname()), "{refused}");
    assert!(refused.contains("message"), "names how to reach the holder: {refused}");
    assert!(refused.contains("force=true"), "names the override: {refused}");
    let task = open_task_store(&bravo.cas_dir).unwrap().get("cas-p001").unwrap();
    assert_eq!(task.status, TaskStatus::Open, "a refused start changes nothing: {refused}");
    assert!(
        open_agent_store(&bravo.cas_dir).unwrap().get_lease("cas-p001").unwrap().is_none(),
        "no local lease either"
    );

    let released = release(&alpha.core, "cas-p001").await;
    assert!(cloud.active_lock("cas-p001").is_none(), "release frees the cloud claim: {released}");
    let taken = start(&bravo.core, "cas-p001", false).await.expect("bravo starts after release");
    assert_eq!(cloud.active_lock("cas-p001").unwrap().1.agent_id, bravo.agent_id, "{taken}");
}

/// The operator may override a live peer claim; the start says so.
#[tokio::test]
async fn force_overrides_a_peer_claim_with_a_visible_note() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    for m in [&alpha, &bravo] {
        m.put_task("cas-p002", TaskType::Task);
    }
    start(&alpha.core, "cas-p002", false).await.unwrap();
    let forced = start(&bravo.core, "cas-p002", true).await.expect("force starts");
    assert!(forced.contains("alpha-sup"), "{forced}");
    assert!(forced.to_lowercase().contains("override"), "{forced}");
    let task = open_task_store(&bravo.cas_dir).unwrap().get("cas-p002").unwrap();
    assert_eq!(task.status, TaskStatus::InProgress);
}

/// Closing releases the claim too, and another repository's task with the same
/// id never collides with this one.
#[tokio::test]
async fn close_releases_and_other_repositories_never_collide() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    let other = machine(&cloud.url, "github.com/acme/other", "charlie-sup");
    for m in [&alpha, &bravo, &other] {
        m.put_task("cas-p003", TaskType::Spike);
    }
    start(&alpha.core, "cas-p003", false).await.unwrap();
    start(&other.core, "cas-p003", false)
        .await
        .expect("a different repository's cas-p003 is a different task");
    assert_eq!(cloud.keys().len(), 2, "{:?}", cloud.keys());

    let closed = alpha
        .core
        .cas_task_close(Parameters(TaskCloseRequest {
            stranded_branch_override: None,
            id: "cas-p003".into(),
            reason: Some("spike answered".into()),
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .map(extract_text)
        .unwrap_or_else(|error| error.message.to_string());
    let task = open_task_store(&alpha.cas_dir).unwrap().get("cas-p003").unwrap();
    assert_eq!(task.status, TaskStatus::Closed, "{closed}");
    start(&bravo.core, "cas-p003", false)
        .await
        .expect("after alpha closes it, the claim is gone");
}

/// Claims renew while the holder works; after it dies its claim ages out and
/// a peer may start the task, told whose claim went stale.
#[tokio::test]
async fn claims_renew_while_working_and_a_dead_holders_claim_goes_stale() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    for m in [&alpha, &bravo] {
        m.put_task("cas-p004", TaskType::Task);
    }
    start(&alpha.core, "cas-p004", false).await.unwrap();
    let before = cloud.active_lock("cas-p004").unwrap().1;
    let renewed = cas::cloud::peer_claims::renew_agent_claims(&alpha.cas_dir, &alpha.agent_id);
    assert_eq!(renewed.renewed, 1, "{renewed:?}");
    let after = cloud.active_lock("cas-p004").unwrap().1;
    assert_eq!(after.renewal_count, before.renewal_count + 1);
    assert!(after.expires_at >= before.expires_at);

    // alpha's machine stops: nothing renews, and the claim runs out.
    cloud.age_out("cas-p004");
    let taken = start(&bravo.core, "cas-p004", false)
        .await
        .expect("a stale claim does not block");
    assert!(taken.contains("alpha-sup"), "{taken}");
    assert!(taken.to_lowercase().contains("expired"), "{taken}");
}

/// Two supervisors working children of one epic are told about each other.
#[tokio::test]
async fn epic_focus_overlap_warns_without_refusing() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    for m in [&alpha, &bravo] {
        m.put_task("cas-pe01", TaskType::Epic);
        m.child_of("cas-p005", "cas-pe01");
        m.child_of("cas-p006", "cas-pe01");
    }
    start(&alpha.core, "cas-p005", false).await.unwrap();
    assert!(cloud.active_lock("cas-pe01").is_some(), "the epic focus is claimed too");
    let shared = start(&bravo.core, "cas-p006", false)
        .await
        .expect("a sibling task is not refused");
    assert!(shared.contains("cas-pe01"), "{shared}");
    assert!(shared.contains("alpha-sup"), "{shared}");
}

/// With the cloud unreachable the local lease still works, and the start says
/// that peers could not be checked.
#[tokio::test]
async fn an_unreachable_cloud_falls_back_to_the_local_lease_with_a_warning() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let closed_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", closed_port.local_addr().unwrap());
    drop(closed_port);
    let alpha = machine(&endpoint, PROJECT, "alpha-sup");
    alpha.put_task("cas-p007", TaskType::Task);
    let started = start(&alpha.core, "cas-p007", false).await.expect("local lease still works");
    assert!(started.to_lowercase().contains("peer"), "{started}");
    assert!(started.to_lowercase().contains("unreachable"), "{started}");
    assert!(
        open_agent_store(&alpha.cas_dir).unwrap().get_lease("cas-p007").unwrap().is_some(),
        "{started}"
    );
}

/// A cloud claim left by another agent of this same database is not a peer:
/// the local lease decides, and the claim moves to the new holder.
#[tokio::test]
async fn a_claim_held_by_a_local_agent_moves_with_the_local_lease() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    alpha.put_task("cas-p008", TaskType::Task);
    start(&alpha.core, "cas-p008", false).await.unwrap();
    // The local lease is dropped without touching the cloud (a crashed path).
    open_agent_store(&alpha.cas_dir).unwrap().release_lease("cas-p008", &alpha.agent_id).unwrap();
    let mut task = open_task_store(&alpha.cas_dir).unwrap().get("cas-p008").unwrap();
    task.status = TaskStatus::Open;
    task.assignee = None;
    open_task_store(&alpha.cas_dir).unwrap().update(&task).unwrap();

    let worker = alpha.second_agent("delta-worker");
    let taken = start(&worker, "cas-p008", false).await.expect("a local holder is not a peer");
    let (_, lock) = cloud.active_lock("cas-p008").unwrap();
    assert_ne!(lock.agent_id, alpha.agent_id, "{taken}");
}

/// A pulled task in progress under an assignee from another machine names
/// that holder in the start advisory.
#[tokio::test]
async fn in_progress_elsewhere_names_the_remote_assignee() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    let mut task = Task::new("cas-p009".into(), "pulled".into());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("echo-sup".into());
    open_task_store(&bravo.cas_dir).unwrap().add(&task).unwrap();
    let started = start(&bravo.core, "cas-p009", true).await.unwrap_or_else(|e| e);
    assert!(started.contains("echo-sup"), "{started}");
    assert!(started.to_lowercase().contains("another machine"), "{started}");
}

