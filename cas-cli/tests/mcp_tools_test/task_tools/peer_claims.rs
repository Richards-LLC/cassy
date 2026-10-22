//! cas-5f28: cross-machine claim awareness.
//!
//! Two supervisors of one repository run two Cassy databases (two machines,
//! or two clones). Their local leases cannot see each other, so `task start`
//! also claims the task in Cassy Cloud. These tests drive the real MCP
//! surface of two databases against a fake cloud that serves the same
//! `/api/agents/tasks/<id>/{claim,release,renew,lock}` contract as
//! production petra-stella-cloud (#150, PR #153): an expired lock is free,
//! and a refused claim's 409 body names the holder and the expiry.

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
    id: String,
    epoch: u64,
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
    /// Registered agents: id -> (name, hostname, last heartbeat).
    agents: HashMap<String, (String, String, DateTime<Utc>)>,
    requests: Vec<String>,
    statuses: HashMap<String, String>,
    replace_on_release: Option<String>,
    revive_on_lock: Option<String>,
    renew_after_lock_reads: Option<(usize, bool)>,
    change_after_lock_read: Option<(usize, Option<String>)>,
    return_released_rows: bool,
    // Pause exactly at the remote handoff, after the local lease has won.
    release_gate: Option<(tokio::sync::oneshot::Sender<()>, std::sync::mpsc::Receiver<()>)>,
    // cas-c55f: hold the next file-lease release until the test resumes it.
    file_release_gate: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
    // cas-978d: hold the next task-claim release until the test resumes it.
    task_release_gate: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
    // cas-da31: hold the next release on its own thread until the test
    // resumes it, while every other request is still served (a slow Cloud).
    async_release_gate: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
    // Observe actual late application, even after the client has timed out.
    async_release_done: Option<std::sync::mpsc::Sender<(serde_json::Value, u16)>>,
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
        "id": lock.id,
        "task_id": key,
        "agent_id": lock.agent_id,
        "status": lock.status,
        "expires_at": lock.expires_at.to_rfc3339(),
        "renewed_at": lock.renewed_at.to_rfc3339(),
        "renewal_count": lock.renewal_count,
        "epoch": lock.epoch,
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
                let async_gate = if request.url().ends_with("/release") {
                    thread_state.lock().unwrap().async_release_gate.take()
                } else {
                    None
                };
                if let Some((entered, resume)) = async_gate {
                    let state = Arc::clone(&thread_state);
                    std::thread::spawn(move || {
                        let _ = entered.send(());
                        let _ = resume.recv();
                        let (status, json) =
                            Self::handle(&state, request.method().as_str(), request.url(), &body);
                        if let Some(done) = state.lock().unwrap().async_release_done.take() {
                            let _ = done.send((serde_json::from_str(&body).unwrap(), status));
                        }
                        let response = tiny_http::Response::from_string(json.to_string())
                            .with_status_code(status)
                            .with_header(
                                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                                    .unwrap(),
                            );
                        let _ = request.respond(response);
                    });
                    continue;
                }
                let gate = if request.url().ends_with("/release") {
                    thread_state.lock().unwrap().release_gate.take()
                } else {
                    None
                };
                if let Some((entered, resume)) = gate {
                    let _ = entered.send(());
                    let _ = resume.recv();
                }
                let file_gate = if !request.url().ends_with("/release") {
                    None
                } else if request.url().starts_with("/api/agents/tasks/file-") {
                    thread_state.lock().unwrap().file_release_gate.take()
                } else {
                    thread_state.lock().unwrap().task_release_gate.take()
                };
                if let Some((entered, resume)) = file_gate {
                    let _ = entered.send(());
                    let _ = resume.recv();
                }
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
        if method == "GET" && (url == "/api/agents" || url.starts_with("/api/agents?")) {
            let agents: Vec<_> = state
                .agents
                .iter()
                .map(|(id, (name, host, seen))| {
                    serde_json::json!({"id": id, "name": name, "agent_type": "primary",
                        "status": state.statuses.get(id).map(String::as_str).unwrap_or("active"), "pid": null, "session_id": null, "machine_id": host,
                        "last_heartbeat": seen.to_rfc3339(), "active_tasks": 0,
                        "metadata": {"hostname": host, "canonical_id": PROJECT}})
                })
                .collect();
            return (200, serde_json::json!({"status": "success", "agents": agents}));
        }
        // Legacy one-account control fixture also serves #155's project
        // list. Direct account reads above still drive the ownership races.
        if method == "GET" && url.starts_with("/api/agents/locks?") {
            let locks: Vec<_> = state.locks.iter()
                .filter(|(_, lock)| lock.status == "active" && lock.expires_at > Utc::now())
                .map(|(key, lock)| lock_json(key, lock)).collect();
            return (200, serde_json::json!({"status":"success", "locks":locks}));
        }
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
                // The production contract (petra-stella-cloud #150, PR #153, as
                // captured by cas-2911): a live lock refuses with the legacy
                // envelope, naming the holder in both owner_agent_id and
                // agent_id; a same-agent re-claim returns the lock unextended;
                // an expired lock is free, and a claim takes it over with the
                // next epoch.
                Some(lock) if lock.agent_id != agent_id && lock.expires_at > now => (
                    409,
                    serde_json::json!({"status": "conflict", "result": null, "lock": null,
                        "error": "already_claimed", "owner_agent_id": lock.agent_id,
                        "agent_id": lock.agent_id, "expires_at": lock.expires_at.to_rfc3339()}),
                ),
                Some(lock) if lock.agent_id == agent_id && lock.expires_at > now => (
                    200,
                    serde_json::json!({"status": "success", "result": "claimed",
                        "lock": lock_json(&key, &lock), "error": null, "owner_agent_id": null}),
                ),
                _ => {
                    let secs = body["duration_secs"].as_i64().unwrap_or(600).clamp(1, 3600);
                    let epoch = state.locks.get(&key).map(|lock| lock.epoch).unwrap_or(0) + 1;
                    let lock = FakeLock {
                        id: uuid::Uuid::new_v4().to_string(),
                        epoch,
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
                // petra-stella-cloud #164: a conditional release names the
                // lock snapshot it expects, both fields or neither; a lock
                // reacquired or renewed since answers 409.
                let expected = (body["expected_epoch"].as_u64(), body["expected_renewal_count"].as_u64());
                match expected {
                    (Some(epoch), Some(renewals)) => {
                        if let Some(lock) = state.locks.get(&key).filter(|lock| lock.status == "active")
                            && (lock.epoch != epoch || u64::from(lock.renewal_count) != renewals)
                        {
                            return (409, serde_json::json!({"error": "lock_snapshot_changed"}));
                        }
                    }
                    (None, None) => {}
                    _ => return (400, serde_json::json!({"error": "expected_epoch and expected_renewal_count go together"})),
                }
                if let Some(winner) = state.replace_on_release.take()
                    && let Some(lock) = state.locks.get_mut(&key)
                { lock.agent_id = winner; lock.renewed_at = now; lock.epoch += 1; }
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
                    (200, serde_json::json!({"lock": json}))
                }
                _ => (404, serde_json::json!({"error": "No active lock found"})),
            },
            ("GET", "lock") => {
                if let Some(holder) = state.revive_on_lock.take() {
                    state.statuses.insert(holder, "active".into());
                }
                let snapshot = if state.return_released_rows { state.locks.get(&key).cloned() } else { active };
                let response = serde_json::json!({"lock": snapshot.map(|lock| lock_json(&key, &lock))});
                let reads = state.requests.iter().filter(|r| r.ends_with("/lock")).count();
                if let Some((at, epoch_only)) = state.renew_after_lock_reads
                    && at == reads && let Some(lock) = state.locks.get_mut(&key)
                {
                    if epoch_only { lock.epoch += 1; } else { lock.renewed_at = now; }
                }
                if state.change_after_lock_read.as_ref().is_some_and(|(at, _)| *at == reads) {
                    let (_, winner) = state.change_after_lock_read.take().unwrap();
                    if let Some(lock) = state.locks.get_mut(&key) {
                        match winner {
                            Some(winner) => {
                                lock.agent_id = winner;
                                lock.renewed_at = now;
                                lock.epoch += 1;
                                lock.claim_reason = None;
                            }
                            None => lock.status = "released".into(),
                        }
                    }
                }
                (200, response)
            },
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

    /// The daemon registered `machine`'s agent and it heartbeats now.
    fn live(&self, machine: &Machine, name: &str) {
        self.state
            .lock()
            .unwrap()
            .agents
            .insert(machine.agent_id.clone(), (name.to_string(), hostname(), Utc::now()));
    }

        /// How many requests so far read a lock directly.
    fn lock_reads(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|request| request.starts_with("GET ") && request.ends_with("/lock"))
            .count()
    }

    fn keys(&self) -> Vec<String> {
        self.state.lock().unwrap().locks.keys().cloned().collect()
    }

    /// The holder's claim was renewed long ago (its daemon stopped), though
    /// it has not run out yet.
    fn stop_renewing(&self, task_id: &str) {
        let mut state = self.state.lock().unwrap();
        for (key, lock) in state.locks.iter_mut() {
            if key.starts_with(&format!("{task_id}~")) {
                lock.renewed_at = Utc::now() - Duration::minutes(20);
            }
        }
    }

    /// The claim was lost: released or expired server-side.
    fn drop_lock(&self, task_id: &str) {
        let mut state = self.state.lock().unwrap();
        for (key, lock) in state.locks.iter_mut() {
            if key.starts_with(&format!("{task_id}~")) {
                lock.status = "released".into();
            }
        }
    }

    /// `agent_id` now holds the claim (it took it while ours was lost).
    fn give_to(&self, task_id: &str, agent_id: &str, reason: Option<String>) {
        let mut state = self.state.lock().unwrap();
        for (key, lock) in state.locks.iter_mut() {
            if key.starts_with(&format!("{task_id}~")) {
                lock.agent_id = agent_id.to_string();
                lock.status = "active".into();
                lock.renewed_at = Utc::now();
                lock.claim_reason = reason.clone();
            }
        }
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
        .register(&Agent::new_with_role(agent_id.clone(), name.to_string(), AgentRole::Standard))
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
            .register(&Agent::new_with_role(id.clone(), name.to_string(), AgentRole::Standard))
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

    cloud.live(&alpha, "alpha-sup");
    let started = start(&alpha.core, "cas-p001", false).await.expect("alpha starts");
    let (key, lock) = cloud.active_lock("cas-p001").expect("alpha's start claims in the cloud");
    assert_eq!(lock.agent_id, alpha.agent_id, "{started}");
    assert_ne!(key, "cas-p001", "the claim key carries the project scope");

    let reads_before = cloud.lock_reads();
    let refused = start(&bravo.core, "cas-p001", false).await.expect_err("bravo is refused");
    // cas-0078: the 409 names the holder and its expiry, and the agent list
    // names its machine, so a refusal needs no second read of the lock.
    assert_eq!(cloud.lock_reads(), reads_before, "{refused}");
    assert!(refused.contains(&lock.expires_at.format("%H:%M UTC").to_string()), "{refused}");
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

    // A stale pending release at bravo must not disturb alpha's live claim
    // when the actual manual MCP entry is refused by the other machine.
    let tasks = open_task_store(&bravo.cas_dir).unwrap();
    let mut task = tasks.get("cas-p001").unwrap();
    task.assignee = Some("bravo-sup".into());
    tasks.update(&task).unwrap();
    let pending = bravo.cas_dir.join("claim-releases.json");
    std::fs::write(&pending, serde_json::json!({"cas-p001": Utc::now().timestamp()}).to_string()).unwrap();
    let manual = claim(&bravo.core, "cas-p001").await.expect_err("the peer refuses manual claim too");
    assert!(manual.contains("HELD BY A PEER") && manual.contains("alpha-sup"), "{manual}");
    assert!(!pending.exists(), "manual acquisition cancels the old pending release before refusal");
    assert_eq!(cloud.active_lock("cas-p001").unwrap().1.agent_id, alpha.agent_id);
    assert!(open_agent_store(&bravo.cas_dir).unwrap().get_lease("cas-p001").unwrap().is_none());

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
    cloud.live(&alpha, "alpha-sup");
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
            no_delivery: None,
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
    // cas-978d: the close releases its claim in the background.
    let key = cas::cloud::peer_claims::claim_key(PROJECT, "cas-p003");
    let mut released = false;
    for _ in 0..200 {
        if cloud.state.lock().unwrap().locks.get(&key).is_none_or(|lock| lock.status != "active") {
            released = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(released, "alpha's close releases its claim");
    start(&bravo.core, "cas-p003", false)
        .await
        .expect("after alpha closes it, the claim is gone");
}

/// A worker's edit of `file` for `task`, as its PreToolUse hook claims it.
async fn edit(m: &Machine, name: &str, task: &str, file: &str) -> cas::cloud::file_leases::FileLease {
    let (root, agent) = (m.cas_dir.clone(), m.agent_id.clone());
    let (name, task, file) = (name.to_string(), task.to_string(), file.to_string());
    tokio::task::spawn_blocking(move || {
        cas::cloud::file_leases::claim(&root, &agent, &name, &task, &file)
    })
    .await
    .unwrap()
}

/// Whether the Cloud claim on `id` is released within a few seconds.
async fn freed(cloud: &FakeCloud, id: &str) -> bool {
    for _ in 0..200 {
        if cloud.active_lock(id).is_none() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    false
}

/// Whether the Cloud lease on `file` is released within a few seconds.
async fn file_freed(cloud: &FakeCloud, file: &str) -> bool {
    freed(cloud, &cas::cloud::file_leases::lease_id(file)).await
}

// Prove progress on this test's single-thread runtime while Cloud holds the
// actual task release. The old inline blocking call cannot poll `progress`
// until its own HTTP deadline returns; no elapsed-time assertion is needed.
async fn with_release_progress<T>(
    cloud: &FakeCloud, operation: impl std::future::Future<Output = T>,
) -> (T, bool) {
    let (entered, observed) = std::sync::mpsc::channel();
    let (resume, held) = std::sync::mpsc::channel();
    let (notify, notified) = tokio::sync::oneshot::channel();
    cloud.state.lock().unwrap().task_release_gate = Some((entered, held));
    let observer = std::thread::spawn(move || {
        observed.recv_timeout(std::time::Duration::from_secs(10)).expect("release reaches Cloud");
        let _ = notify.send(());
    });
    let completed = std::cell::Cell::new(false);
    let operation = async {
        let answer = operation.await;
        completed.set(true);
        answer
    };
    let progress = async {
        notified.await.expect("the Cloud release gate was reached");
        let stalled = completed.get();
        let _ = resume.send(());
        stalled
    };
    let answer = tokio::join!(operation, progress);
    observer.join().unwrap();
    answer
}

/// cas-c55f, cas-978d: releasing, resetting or transferring a task frees the
/// files edited for it, as close does, so another session may take them.
#[tokio::test(flavor = "current_thread")]
async fn release_reset_and_transfer_free_the_tasks_file_leases_cas_c55f_978d() {
    use cas::cloud::file_leases::FileLease;
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");

    alpha.put_task("cas-f001", TaskType::Task);
    start(&alpha.core, "cas-f001", false).await.unwrap();
    assert_eq!(edit(&alpha, "alpha-sup", "cas-f001", "src/released.rs").await, FileLease::Mine);
    let (released, release_stalled) = with_release_progress(&cloud, release(&alpha.core, "cas-f001")).await;
    assert!(file_freed(&cloud, "src/released.rs").await, "release leaves the file held: {released}");

    alpha.put_task("cas-f002", TaskType::Task);
    start(&alpha.core, "cas-f002", false).await.unwrap();
    assert_eq!(edit(&alpha, "alpha-sup", "cas-f002", "src/reset.rs").await, FileLease::Mine);
    let (reset, reset_stalled) = with_release_progress(&cloud, async {
        alpha.core
            .cas_task_reset(Parameters(TaskReleaseRequest { task_id: "cas-f002".into(), force: Some(true) }))
            .await
            .map(extract_text)
            .unwrap_or_else(|error| error.message.to_string())
    }).await;
    assert!(file_freed(&cloud, "src/reset.rs").await, "reset leaves the file held: {reset}");

    alpha.put_task("cas-f004", TaskType::Task);
    start(&alpha.core, "cas-f004", false).await.unwrap();
    assert_eq!(edit(&alpha, "alpha-sup", "cas-f004", "src/transferred.rs").await, FileLease::Mine);
    let _receiver = alpha.second_agent("bravo-wkr");
    let (transferred, transfer_stalled) = with_release_progress(&cloud, async {
        alpha.core
            .cas_task_transfer(Parameters(TaskTransferRequest {
                adopt_branch: None,
                task_id: "cas-f004".into(),
                to_agent: format!("bravo-wkr-session-{}", std::process::id()),
                note: None,
                supervisor_override: None,
            }))
            .await
            .map(extract_text)
            .unwrap_or_else(|error| error.message.to_string())
    }).await;
    assert!(
        file_freed(&cloud, "src/transferred.rs").await,
        "transfer leaves the previous holder's file held: {transferred}"
    );
    assert_eq!([release_stalled, reset_stalled, transfer_stalled], [false; 3],
        "release/reset/transfer must let the async runtime progress while Cloud is held");
}

/// cas-c55f, cas-978d: a close does not wait for a slow Cloud to release
/// the task's claim or its file leases; both releases still reach the Cloud
/// afterwards.
#[tokio::test]
async fn close_does_not_wait_on_slow_cloud_releases_cas_c55f_978d() {
    use cas::cloud::file_leases::FileLease;
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    alpha.put_task("cas-f003", TaskType::Spike);
    start(&alpha.core, "cas-f003", false).await.unwrap();
    assert_eq!(edit(&alpha, "alpha-sup", "cas-f003", "src/closed.rs").await, FileLease::Mine);

    let (entered, release_reached) = std::sync::mpsc::channel();
    let (resume, held) = std::sync::mpsc::channel();
    let (task_entered, task_release_reached) = std::sync::mpsc::channel();
    let (task_resume, task_held) = std::sync::mpsc::channel();
    {
        let mut state = cloud.state.lock().unwrap();
        state.file_release_gate = Some((entered, held));
        state.task_release_gate = Some((task_entered, task_held));
    }
    // The Cloud answers each release 3 s late, or when the test resumes it.
    let (late, task_late) = (resume.clone(), task_resume.clone());
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        let _ = task_late.send(());
        let _ = late.send(());
    });
    let started = std::time::Instant::now();
    let closed = alpha
        .core
        .cas_task_close(Parameters(TaskCloseRequest {
            no_delivery: None,
            stranded_branch_override: None,
            id: "cas-f003".into(),
            reason: Some("spike answered".into()),
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .map(extract_text)
        .unwrap_or_else(|error| error.message.to_string());
    let took = started.elapsed();
    let _ = task_resume.send(());
    let _ = resume.send(());
    let task = open_task_store(&alpha.cas_dir).unwrap().get("cas-f003").unwrap();
    assert_eq!(task.status, TaskStatus::Closed, "{closed}");
    assert!(took < std::time::Duration::from_secs(2), "close waited {took:?} on the Cloud");
    task_release_reached
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the task claim release still reaches the Cloud");
    release_reached
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the file release still reaches the Cloud");
    assert!(freed(&cloud, "cas-f003").await, "the task claim is released after the close");
    assert!(file_freed(&cloud, "src/closed.rs").await, "the file is released after the close");
}

/// Claims renew while the holder works; after it dies its claim runs out,
/// and the cloud frees it, so a peer starts the task and holds the claim.
#[tokio::test]
async fn claims_renew_while_working_and_an_expired_claim_is_taken_over() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    for m in [&alpha, &bravo] {
        m.put_task("cas-p004", TaskType::Task);
    }
    cloud.live(&alpha, "alpha-sup");
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
        .expect("an expired claim does not block");
    assert!(!taken.contains("HELD BY A PEER"), "{taken}");
    assert_eq!(cloud.active_lock("cas-p004").unwrap().1.agent_id, bravo.agent_id, "{taken}");
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
    cloud.live(&alpha, "alpha-sup");
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
    for cancel_at_handoff in [false, true] {
        let cloud = FakeCloud::start();
        let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
        alpha.put_task("cas-p008", TaskType::Task);
        let repo = alpha.cas_dir.parent().unwrap();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git").args(args).current_dir(repo).status().unwrap().success());
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "-q", "--allow-empty", "-m", "seed"]);
        for branch in ["factory/prior-1", "factory/prior-2", "factory/prior-3", "factory/prior-4", "factory/prior-5"] {
            git(&["branch", branch]);
        }
        let tasks = open_task_store(&alpha.cas_dir).unwrap();
        let mut task = tasks.get("cas-p008").unwrap();
        task.notes = (0..78).map(|i| format!("[2026-10-18] Progress {i}: {}", "x".repeat(650))).collect::<Vec<_>>().join("\n\n");
        task.deliverables.work_target = Some(cas::types::WorkTarget {
            repo_selector: format!("project:{PROJECT}"), target_branch: "main".into(),
        });
        tasks.update(&task).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), start(&alpha.core, "cas-p008", false)).await.unwrap().unwrap();
        // The local lease is dropped without touching the cloud (a crashed path).
        open_agent_store(&alpha.cas_dir).unwrap().release_lease("cas-p008", &alpha.agent_id).unwrap();
        let mut task = open_task_store(&alpha.cas_dir).unwrap().get("cas-p008").unwrap();
        task.status = TaskStatus::Open;
        task.assignee = None;
        open_task_store(&alpha.cas_dir).unwrap().update(&task).unwrap();

        let worker = alpha.second_agent("delta-worker");
        if cancel_at_handoff {
            let agents = open_agent_store(&alpha.cas_dir).unwrap();
            let mut caller = agents.list(None).unwrap().into_iter().find(|a| a.name == "delta-worker").unwrap();
            caller.metadata.insert("halt_task_work".into(), "1".into());
            agents.update(&caller).unwrap();
            let (entered, reached) = tokio::sync::oneshot::channel();
            let (resume, wait) = std::sync::mpsc::channel();
            cloud.state.lock().unwrap().release_gate = Some((entered, wait));
            let mut pending = Box::pin(start(&worker, "cas-p008", false));
            tokio::select! {
                result = &mut pending => panic!("start returned before the blocked handoff: {result:?}"),
                result = reached => result.unwrap(),
            }
            // Dropping the real handler models its response-deadline cancellation,
            // synchronized with the HTTP handoff rather than a timing assumption.
            drop(pending);
            resume.send(()).unwrap();
            let tasks = open_task_store(&alpha.cas_dir).unwrap();
            let agents = open_agent_store(&alpha.cas_dir).unwrap();
            assert_eq!(tasks.get("cas-p008").unwrap().status, TaskStatus::InProgress,
                "a cancelled Cloud handoff must not leave an Open task with an active lease");
            let lease = agents.get_lease("cas-p008").unwrap().unwrap();
            assert_ne!(lease.agent_id, alpha.agent_id);
            assert_eq!(lease.status, cas::types::LeaseStatus::Active);
            assert!(!agents.get(&caller.id).unwrap().metadata.contains_key("halt_task_work"),
                "a committed start must clear its old work halt before a cancellable handoff");
            continue;
        }
        let taken = start(&worker, "cas-p008", false).await.expect("a local holder is not a peer");
        let (_, lock) = cloud.active_lock("cas-p008").unwrap();
        assert_ne!(lock.agent_id, alpha.agent_id, "{taken}");
    }
}

/// cas-da31: a start whose local lease fails rolls its fresh Cloud claim
/// back. When that release outlasts the start's Cloud budget it keeps
/// running in the background. A restart by the same agent in that window
/// must keep its own fresh claim: the stale release may not take it.
#[tokio::test]
async fn a_restart_during_a_timed_out_rollback_keeps_its_fresh_claim_cas_da31() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "restart-worker");
    alpha.put_task("cas-restart", TaskType::Task);
    let db = rusqlite::Connection::open(alpha.cas_dir.join("cas.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER refuse_start BEFORE INSERT ON task_leases
         WHEN NEW.task_id = 'cas-restart'
         BEGIN SELECT RAISE(ABORT, 'injected start write failure'); END;",
    )
    .unwrap();
    // The rollback's Cloud release is held, on its own thread, until the test
    // lets it go; every other request is still served.
    let (entered, rollback_reached) = std::sync::mpsc::channel();
    let (resume, held) = std::sync::mpsc::channel();
    cloud.state.lock().unwrap().async_release_gate = Some((entered, held));

    // The failed start returns once its Cloud budget runs out; its rollback
    // release is still held at the Cloud.
    start(&alpha.core, "cas-restart", false).await.unwrap_err();
    rollback_reached
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the rollback asked the Cloud to release");

    db.execute_batch("DROP TRIGGER refuse_start;").unwrap();
    let mut restart = Box::pin(start(&alpha.core, "cas-restart", false));
    // Give the restart time to claim if nothing holds it back.
    let early = tokio::time::timeout(std::time::Duration::from_millis(500), &mut restart).await;
    resume.send(()).unwrap();
    let restarted = match early {
        Ok(result) => result,
        Err(_) => restart.await,
    };
    restarted.expect("the same agent restarts the task");

    // Once the stale release has reached the Cloud, the restart's claim stands.
    let release_served = || {
        cloud.state.lock().unwrap().requests.iter()
            .any(|request| request.starts_with("POST ") && request.ends_with("/release"))
    };
    for _ in 0..200 {
        if release_served() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(release_served(), "the rollback release reached the Cloud");
    let (_, lock) = cloud
        .active_lock("cas-restart")
        .expect("the restart's fresh Cloud claim survives the stale rollback release");
    assert_eq!(lock.agent_id, alpha.agent_id);
}

/// Exercise both remaining release entry paths, including a real HTTP timeout
/// and a live 409 response that must settle the pending retry record.
#[tokio::test]
async fn delayed_retry_and_focus_releases_keep_same_agent_restart_cas_0760() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let mut lost = Vec::new();
    for (entry, timeout) in [("retry", false), ("retry", true), ("focus", true)] {
        let cloud = FakeCloud::start();
        let alpha = machine(&cloud.url, PROJECT, "restart-worker");
        let restart = machine(&cloud.url, PROJECT, "restart-worker");
        assert_eq!(alpha.agent_id, restart.agent_id);
        let task = "cas-release-restart";
        for machine in [&alpha, &restart] {
            machine.put_task(task, TaskType::Task);
        }
        if entry == "focus" {
            assert!(matches!(cas::cloud::peer_claims::claim_epic_focus(
                &alpha.cas_dir, task, &alpha.agent_id,
            ), Some(cas::cloud::peer_claims::Acquire::Claimed)));
        } else {
            start(&alpha.core, task, false).await.unwrap();
            open_agent_store(&alpha.cas_dir).unwrap().release_lease(task, &alpha.agent_id).unwrap();
            // A persisted failed release, retried by the real daemon renewal.
            std::fs::write(alpha.cas_dir.join("claim-releases.json"),
                serde_json::json!({task: Utc::now().timestamp()}).to_string()).unwrap();
        }
        let before = cloud.active_lock(task).unwrap().1;
        let (entered, reached) = std::sync::mpsc::channel();
        let (resume, held) = std::sync::mpsc::channel();
        let (done, applied) = std::sync::mpsc::channel();
        {
            let mut state = cloud.state.lock().unwrap();
            state.async_release_gate = Some((entered, held));
            state.async_release_done = Some(done);
        }
        let root = alpha.cas_dir.clone();
        let agent = alpha.agent_id.clone();
        let mut releasing = Some(std::thread::spawn(move || {
            if entry == "focus" {
                assert_eq!(cas::cloud::peer_claims::release_epic_focus(&root, &agent).as_deref(), Some(task));
            } else {
                cas::cloud::peer_claims::renew_agent_claims(&root, &agent);
            }
        }));
        reached.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        if timeout {
            // Real request timeout releases the fence before Cloud applies it.
            releasing.take().unwrap().join().unwrap();
        }
        start(&restart.core, task, false).await.expect("same agent restarts in its new database");
        let fresh = cloud.active_lock(task).unwrap().1;
        assert!(fresh.renewal_count > before.renewal_count);
        resume.send(()).unwrap();
        let (body, status) = applied.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        if let Some(releasing) = releasing { releasing.join().unwrap(); }
        if cloud.active_lock(task).is_none() { lost.push((entry, timeout)); }
        // Collect both entry failures before asserting so RED covers both.
        if status == 409 {
            assert_eq!(body["expected_epoch"], before.epoch);
            assert_eq!(body["expected_renewal_count"], before.renewal_count);
            if entry == "retry" && !timeout {
                let pending = alpha.cas_dir.join("claim-releases.json");
                assert!(!pending.exists(), "409 is settled, never retried against the fresh claim");
            }
        } else {
            assert_eq!(status, 200, "unexpected fake Cloud response");
        }
    }
    assert!(lost.is_empty(), "stale releases discarded same-agent restart claims: {lost:?}");
}

#[tokio::test]
async fn start_write_failure_leaves_no_new_lease_cas_d5f4() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let mut orphaned = Vec::new();
    for (entry, fail_lease) in [("start", false), ("start", true), ("claim", true)] {
        let cloud = FakeCloud::start();
        let alpha = machine(&cloud.url, PROJECT, "start-worker");
        alpha.put_task("cas-start-rollback", TaskType::Task);
        if entry == "claim" {
            let tasks = open_task_store(&alpha.cas_dir).unwrap();
            let mut task = tasks.get("cas-start-rollback").unwrap();
            task.assignee = Some("start-worker".into());
            tasks.update(&task).unwrap();
        }
        let db = rusqlite::Connection::open(alpha.cas_dir.join("cas.db")).unwrap();
        let trigger = if fail_lease {
            "CREATE TRIGGER refuse_start BEFORE INSERT ON task_leases
             WHEN NEW.task_id = 'cas-start-rollback'
             BEGIN SELECT RAISE(ABORT, 'injected start write failure'); END;"
        } else {
            "CREATE TRIGGER refuse_start BEFORE UPDATE OF status ON tasks
             WHEN NEW.id = 'cas-start-rollback' AND NEW.status = 'in_progress'
             BEGIN SELECT RAISE(ABORT, 'injected start write failure'); END;"
        };
        db.execute_batch(trigger).unwrap();
        let error = if entry == "claim" {
            claim(&alpha.core, "cas-start-rollback").await
        } else {
            start(&alpha.core, "cas-start-rollback", false).await
        }.unwrap_err();
        assert!(error.contains("injected start write failure"), "{error}");
        assert_eq!(open_task_store(&alpha.cas_dir).unwrap().get("cas-start-rollback").unwrap().status, TaskStatus::Open);
        assert!(open_agent_store(&alpha.cas_dir).unwrap().list_agent_leases(&alpha.agent_id).unwrap().is_empty(),
            "a failed local start must not leave a new lease or an InProgress task");
        if fail_lease && cloud.active_lock("cas-start-rollback").is_some() {
            orphaned.push(entry);
        }
    }
    assert!(orphaned.is_empty(), "local claim errors left just-taken Cloud claims: {orphaned:?}");
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

/// A stopped, unregistered holder is recovered after its renewal goes stale;
/// the replacement holds the cloud claim rather than working local-only.
#[tokio::test]
async fn a_claim_whose_holder_is_not_a_live_agent_is_stale() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    for m in [&alpha, &bravo] {
        m.put_task("cas-p010", TaskType::Task);
    }
    // alpha claims, but its daemon never registered (or long stopped), and
    // nothing has renewed the claim for longer than a peer stays live.
    start(&alpha.core, "cas-p010", false).await.unwrap();
    cloud.stop_renewing("cas-p010");
    let taken = start(&bravo.core, "cas-p010", false)
        .await
        .expect("a claim nobody keeps alive does not block");
    assert!(!taken.contains("STALE PEER CLAIM"), "{taken}");
    assert_eq!(cloud.active_lock("cas-p010").unwrap().1.agent_id, bravo.agent_id);
}

/// cas-0078 (e): a holder whose cloud registration failed is not listed, but
/// its claim was just renewed. The renewal is itself a liveness signal, so the
/// peer is refused rather than told the claim is stale.
#[tokio::test]
async fn an_unregistered_holder_with_a_fresh_renewal_still_refuses() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    for m in [&alpha, &bravo] {
        m.put_task("cas-pe11", TaskType::Task);
    }
    // alpha never registered with Cassy Cloud, but it holds a fresh claim.
    start(&alpha.core, "cas-pe11", false).await.unwrap();
    let refused = start(&bravo.core, "cas-pe11", false)
        .await
        .expect_err("a claim renewed within the live window is a live claim");
    assert!(refused.contains("HELD BY A PEER"), "{refused}");
    assert!(refused.contains("alpha-sup"), "{refused}");
    assert_eq!(cloud.active_lock("cas-pe11").unwrap().1.agent_id, alpha.agent_id);
}

async fn claim(core: &CasCore, id: &str) -> Result<String, String> {
    core.cas_task_claim(Parameters(TaskClaimRequest {
        task_id: id.to_string(),
        duration_secs: 600,
        reason: None,
    }))
    .await
    .map(extract_text)
    .map_err(|error| error.message.to_string())
}

/// cas-0078 (a): a second agent of the same database whose local lease is
/// refused never takes the cloud claim. The original holder keeps it, so a
/// peer on another machine is still refused.
#[tokio::test]
async fn a_refused_local_claim_leaves_the_cloud_claim_with_its_holder() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    for m in [&alpha, &bravo] {
        m.put_task("cas-pa01", TaskType::Task);
    }
    cloud.live(&alpha, "alpha-sup");
    start(&alpha.core, "cas-pa01", false).await.expect("alpha starts");
    // The task row is unassigned again (e.g. edited), but alpha's lease stands.
    let mut task = open_task_store(&alpha.cas_dir).unwrap().get("cas-pa01").unwrap();
    task.assignee = None;
    task.status = TaskStatus::Open;
    open_task_store(&alpha.cas_dir).unwrap().update(&task).unwrap();

    let delta = alpha.second_agent("delta-sup");
    let refused = start(&delta, "cas-pa01", false)
        .await
        .expect_err("the local lease refuses delta");
    assert!(refused.contains("locked by"), "{refused}");
    let (_, lock) = cloud.active_lock("cas-pa01").expect("the claim survives a refused start");
    assert_eq!(lock.agent_id, alpha.agent_id, "{refused}");

    // The claim path needs the task assigned to the claimant before it asks
    // the cloud; alpha's local lease still stands.
    let mut task = open_task_store(&alpha.cas_dir).unwrap().get("cas-pa01").unwrap();
    task.assignee = Some("delta-sup".into());
    open_task_store(&alpha.cas_dir).unwrap().update(&task).unwrap();
    // An old pending release must be cancelled by this actual manual entry
    // even when local arbitration preserves the existing holder.
    let pending = alpha.cas_dir.join("claim-releases.json");
    std::fs::write(&pending, serde_json::json!({"cas-pa01": Utc::now().timestamp()}).to_string()).unwrap();
    let claimed = claim(&delta, "cas-pa01").await.unwrap_or_else(|error| error);
    assert!(!pending.exists(), "manual claim left an old pending release behind");
    assert!(claimed.contains("already claimed"), "{claimed}");
    let (_, lock) = cloud.active_lock("cas-pa01").expect("the claim survives a refused claim");
    assert_eq!(lock.agent_id, alpha.agent_id, "{claimed}");

    let peer = start(&bravo.core, "cas-pa01", false)
        .await
        .expect_err("a peer is still refused");
    assert!(peer.contains("HELD BY A PEER"), "{peer}");
    assert!(peer.contains("alpha-sup"), "{peer}");
}

/// cas-0078 (b): renewal re-acquires a claim that was lost, and reports a
/// conflict, naming the holder, when a peer took it meanwhile.
#[tokio::test]
async fn renewal_reacquires_a_lost_claim_and_reports_a_peer_that_took_it() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    for m in [&alpha, &bravo] {
        m.put_task("cas-pb01", TaskType::Task);
    }
    cloud.live(&alpha, "alpha-sup");
    cloud.live(&bravo, "bravo-sup");
    start(&alpha.core, "cas-pb01", false).await.unwrap();

    cloud.drop_lock("cas-pb01");
    let report = cas::cloud::peer_claims::renew_agent_claims(&alpha.cas_dir, &alpha.agent_id);
    let (_, lock) = cloud.active_lock("cas-pb01").expect("renewal re-acquires the lost claim");
    assert_eq!(lock.agent_id, alpha.agent_id, "{report:?}");
    assert_eq!(report.reacquired, 1, "{report:?}");

    let reason = serde_json::json!({"v": 1, "kind": "task", "name": "bravo-sup",
        "machine": "host-b", "project": PROJECT})
    .to_string();
    cloud.give_to("cas-pb01", &bravo.agent_id, Some(reason));
    let report = cas::cloud::peer_claims::renew_agent_claims(&alpha.cas_dir, &alpha.agent_id);
    assert_eq!(report.conflicts.len(), 1, "{report:?}");
    let (task, hold) = &report.conflicts[0];
    assert_eq!(task, "cas-pb01");
    assert_eq!(hold.name(), "bravo-sup", "{}", hold.describe());
    assert_eq!(cloud.active_lock("cas-pb01").unwrap().1.agent_id, bravo.agent_id);
}

fn factory_service(m: &Machine) -> cas::mcp::CasService {
    let core = CasCore::with_daemon(m.cas_dir.clone(), None, None);
    core.set_agent_id_for_testing(m.agent_id.clone());
    cas::mcp::CasService::new(core, None)
}

async fn factory(
    service: &cas::mcp::CasService,
    request: serde_json::Value,
) -> Result<String, String> {
    let request: cas_mcp::types::FactoryRequest =
        serde_json::from_value(request).expect("a valid factory request");
    service
        .factory_request(Parameters(request))
        .await
        .map(extract_text)
        .map_err(|error| error.message.to_string())
}

/// cas-0078 (c): a pinned focus claims the epic in the cloud, renewal keeps
/// that claim alive while the focus stands, and clearing the focus releases it.
#[tokio::test]
async fn the_focus_claim_renews_while_focused_and_releases_on_clear() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let session = format!("peer-focus-{}", std::process::id());
    env.set("CAS_FACTORY_SESSION", &session);
    let path = cas::ui::factory::metadata_path(&session);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let metadata =
        cas::ui::factory::create_metadata(&session, 12345, "supervisor", &[], None, None, None);
    std::fs::write(&path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();

    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    alpha.put_task("cas-pf01", TaskType::Epic);
    cloud.live(&alpha, "alpha-sup");
    let service = factory_service(&alpha);

    let pinned = factory(&service, serde_json::json!({"action": "focus_epic", "id": "cas-pf01"}))
        .await
        .expect("focus pins");
    let (_, before) = cloud.active_lock("cas-pf01").expect("the focus is claimed");
    assert_eq!(before.agent_id, alpha.agent_id, "{pinned}");

    let report = cas::cloud::peer_claims::renew_agent_claims(&alpha.cas_dir, &alpha.agent_id);
    let (_, after) = cloud.active_lock("cas-pf01").expect("the focus claim stays");
    assert_eq!(after.renewal_count, before.renewal_count + 1, "{report:?}");

    let cleared = factory(&service, serde_json::json!({"action": "focus_epic", "clear": true}))
        .await
        .expect("focus clears");
    assert!(cloud.active_lock("cas-pf01").is_none(), "clearing releases the claim: {cleared}");
}

/// cas-0078 (d): a spawn on a task a live peer holds is refused, naming the
/// holder, before any spawn queue row exists.
#[tokio::test]
async fn a_spawn_on_a_peer_claimed_task_is_refused_before_queueing() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    for m in [&alpha, &bravo] {
        m.put_task("cas-ps01", TaskType::Task);
    }
    cloud.live(&alpha, "alpha-sup");
    start(&alpha.core, "cas-ps01", false).await.unwrap();

    let service = factory_service(&bravo);
    let refused = factory(
        &service,
        serde_json::json!({"action": "spawn_workers", "count": 1, "task_id": "cas-ps01"}),
    )
    .await
    .expect_err("a peer-held task is not spawned");
    assert!(refused.contains("HELD BY A PEER"), "{refused}");
    assert!(refused.contains("alpha-sup"), "{refused}");
    assert!(refused.contains("force=true"), "{refused}");
    let queued = cas::store::open_spawn_queue_store(&bravo.cas_dir).unwrap().peek(10).unwrap();
    assert!(queued.is_empty(), "nothing is queued: {queued:?}");
}

/// cas-0078 (b): a claim lost to a peer reaches the supervisor that owns the
/// losing agent, once, as a daemon notice naming the peer and its machine.
#[tokio::test]
async fn a_claim_lost_to_a_peer_notifies_the_owning_supervisor_once() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let alpha = machine(&cloud.url, PROJECT, "alpha-sup");
    let bravo = machine(&cloud.url, PROJECT, "bravo-sup");
    for m in [&alpha, &bravo] {
        m.put_task("cas-pb02", TaskType::Task);
    }
    // alpha's database runs a factory: a supervisor and its worker.
    let session = format!("peer-loss-{}", std::process::id());
    let agents = open_agent_store(&alpha.cas_dir).unwrap();
    let mut supervisor = Agent::new_with_role(
        format!("zen-sup-{}", std::process::id()),
        "zen-sup".into(),
        AgentRole::Supervisor,
    );
    supervisor.factory_session = Some(session.clone());
    agents.register(&supervisor).unwrap();
    let mut worker = Agent::new_with_role(
        format!("quick-worker-{}", std::process::id()),
        "quick-worker".into(),
        AgentRole::Worker,
    );
    worker.factory_session = Some(session.clone());
    agents.register(&worker).unwrap();
    // The worker holds the local lease, but a peer holds the cloud claim.
    agents.try_claim("cas-pb02", &worker.id, 600, Some("Task started")).unwrap();
    cloud.live(&bravo, "bravo-sup");
    start(&bravo.core, "cas-pb02", false).await.expect("bravo claims it");

    let first = cas::cloud::peer_claims::renew_agent_claims(&alpha.cas_dir, &worker.id);
    assert_eq!(first.conflicts.len(), 1, "{first:?}");
    let again = cas::cloud::peer_claims::renew_agent_claims(&alpha.cas_dir, &worker.id);
    assert_eq!(again.conflicts.len(), 1, "{again:?}");

    let notices: Vec<_> = cas::store::open_prompt_queue_store(&alpha.cas_dir)
        .unwrap()
        .peek_all(100)
        .unwrap()
        .into_iter()
        .filter(|row| row.target == "zen-sup")
        .collect();
    let prompts: Vec<&str> = notices.iter().map(|row| row.prompt.as_str()).collect();
    assert_eq!(notices.len(), 1, "one deduped notice: {prompts:?}");
    let expected = format!("claim on cas-pb02 lost to bravo-sup@{}; stop work or coordinate", hostname());
    assert!(prompts[0].contains(&expected), "{prompts:?}");
    assert_eq!(notices[0].factory_session.as_deref(), Some(session.as_str()));
}

/// cas-c7c7: this fake's claim routes answer every recorded production
/// claim exchange (cas-2911 fixtures) with the same status, keys and
/// literals.
#[test]
fn contract_parity_claims_fake_matches_production() {
    use crate::cloud_contract_replay::{Replay, fixtures};
    let cloud = FakeCloud::start();
    let mut replay = Replay::new(&cloud.url, "0c7c7a12");
    for exchange in fixtures("claims")
        .into_iter()
        .filter(|exchange| exchange.path.starts_with("/api/agents/tasks/"))
    {
        if exchange.name == "claim_takeover_after_expiry" {
            // The capture claimed for one second and waited for it to run out.
            std::thread::sleep(std::time::Duration::from_millis(2500));
        }
        replay.check(&exchange);
    }
    replay.assert_clean();
}

#[tokio::test]
async fn cas_50713_ended_or_stale_holder_is_taken_over_without_claim_lost() {
    for ended in [true, false] {
        let mut env = TestEnvGuard::temp_home();
        env.set("XDG_CONFIG_HOME", env.home().join(".config"));
        let cloud = FakeCloud::start();
        let old = machine(&cloud.url, PROJECT, "old-worker");
        let new = machine(&cloud.url, PROJECT, "replacement");
        for m in [&old, &new] {
            m.put_task("cas-stale", TaskType::Task);
        }
        cloud.live(&old, "old-worker");
        start(&old.core, "cas-stale", false).await.unwrap();
        {
            let mut state = cloud.state.lock().unwrap();
            if ended {
                state
                    .statuses
                    .insert(old.agent_id.clone(), "shutdown".into());
            } else {
                state.agents.get_mut(&old.agent_id).unwrap().2 =
                    Utc::now() - Duration::seconds(301);
            }
        }
        if !ended { cloud.stop_renewing("cas-stale"); }
        let taken = start(&new.core, "cas-stale", false)
            .await
            .expect("dead holder must not refuse");
        assert!(!taken.contains("STALE PEER CLAIM") && !taken.contains("PEER CLAIM OVERRIDE"));
        assert_eq!(
            cloud.active_lock("cas-stale").unwrap().1.agent_id,
            new.agent_id
        );
        let report = cas::cloud::peer_claims::renew_agent_claims(&new.cas_dir, &new.agent_id);
        assert!(report.conflicts.is_empty(), "{report:?}");
        assert!(
            !cas::store::open_prompt_queue_store(&new.cas_dir)
                .unwrap()
                .peek_all(100)
                .unwrap()
                .iter()
                // pin: this alarm class is the externally observed false-conflict regression.
                .any(|row| row.prompt.contains("PEER CLAIM LOST"))
        );
    }
}

#[tokio::test]
async fn cas_50713_renewal_never_announces_a_dead_peer_as_a_live_conflict() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let old = machine(&cloud.url, PROJECT, "dead-peer");
    let new = machine(&cloud.url, PROJECT, "successor");
    new.put_task("cas-renew", TaskType::Task);
    start(&new.core, "cas-renew", false).await.unwrap();
    cloud.give_to("cas-renew", &old.agent_id, None);
    cloud.live(&old, "dead-peer");
    cloud
        .state
        .lock()
        .unwrap()
        .statuses
        .insert(old.agent_id.clone(), "shutdown".into());
    let report = cas::cloud::peer_claims::renew_agent_claims(&new.cas_dir, &new.agent_id);
    assert!(
        report.conflicts.is_empty(),
        "dead claim is not a competitor: {report:?}"
    );
    assert_eq!(
        cloud.active_lock("cas-renew").unwrap().1.agent_id,
        new.agent_id
    );
    assert!(
        cas::store::open_prompt_queue_store(&new.cas_dir)
            .unwrap()
            .peek_all(100)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn cas_50713_successor_focus_moves_the_ended_local_supervisors_claim() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let old = machine(&cloud.url, PROJECT, "predecessor");
    old.put_task("cas-focus", TaskType::Epic);
    assert_eq!(
        cas::cloud::peer_claims::claim_epic_focus(&old.cas_dir, "cas-focus", &old.agent_id),
        Some(cas::cloud::peer_claims::Acquire::Claimed)
    );
    let agents = open_agent_store(&old.cas_dir).unwrap();
    let mut ended = agents.get(&old.agent_id).unwrap();
    ended.status = cas::types::AgentStatus::Shutdown;
    ended.pid = None;
    ended.last_heartbeat = Utc::now() - Duration::minutes(20);
    agents.register(&ended).unwrap();
    old.second_agent("successor");
    let new_id = format!("successor-session-{}", std::process::id());
    assert_eq!(
        cas::cloud::peer_claims::claim_epic_focus(&old.cas_dir, "cas-focus", &new_id),
        Some(cas::cloud::peer_claims::Acquire::Claimed)
    );
    assert_eq!(cloud.active_lock("cas-focus").unwrap().1.agent_id, new_id);
}


#[tokio::test]
async fn cas_50713_recovery_keeps_a_revived_holder_or_raced_live_winner() {
    for revived in [true, false] {
        let mut env = TestEnvGuard::temp_home();
        env.set("XDG_CONFIG_HOME", env.home().join(".config"));
        let cloud = FakeCloud::start();
        let old = machine(&cloud.url, PROJECT, "old-worker");
        let new = machine(&cloud.url, PROJECT, "replacement");
        let winner = machine(&cloud.url, PROJECT, "live-winner");
        for m in [&old, &new] { m.put_task("cas-race", TaskType::Task); }
        cloud.live(&old, "old-worker");
        cloud.live(&winner, "live-winner");
        start(&old.core, "cas-race", false).await.unwrap();
        {
            let mut state = cloud.state.lock().unwrap();
            state.statuses.insert(old.agent_id.clone(), "shutdown".into());
            if revived { state.revive_on_lock = Some(old.agent_id.clone()); }
            else { state.replace_on_release = Some(winner.agent_id.clone()); }
        }
        let refused = start(&new.core, "cas-race", false).await.expect_err("live ownership wins recovery race");
        assert!(refused.contains("HELD BY A PEER"), "{refused}");
        assert_eq!(cloud.active_lock("cas-race").unwrap().1.agent_id,
            if revived { &old.agent_id } else { &winner.agent_id }.clone());
        assert_eq!(open_task_store(&new.cas_dir).unwrap().get("cas-race").unwrap().status, TaskStatus::Open);
    }
}

#[tokio::test]
async fn cas_50713_successor_focus_recognizes_an_unregistered_local_predecessor() {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let old = machine(&cloud.url, PROJECT, "predecessor");
    old.put_task("cas-focus", TaskType::Epic);
    assert_eq!(
        cas::cloud::peer_claims::claim_epic_focus(&old.cas_dir, "cas-focus", &old.agent_id),
        Some(cas::cloud::peer_claims::Acquire::Claimed)
    );
    let agents = open_agent_store(&old.cas_dir).unwrap();
    let mut ended = agents.get(&old.agent_id).unwrap();
    ended.status = cas::types::AgentStatus::Shutdown;
    ended.pid = None;
    ended.last_heartbeat = Utc::now() - Duration::minutes(20);
    agents.unregister(&ended.id).unwrap();
    old.second_agent("successor");
    let new_id = format!("successor-session-{}", std::process::id());
    assert_eq!(
        cas::cloud::peer_claims::claim_epic_focus(&old.cas_dir, "cas-focus", &new_id),
        Some(cas::cloud::peer_claims::Acquire::Claimed)
    );
    assert_eq!(cloud.active_lock("cas-focus").unwrap().1.agent_id, new_id);
}




#[tokio::test]
async fn cas_50713_late_presence_with_fresh_renewal_refuses_and_reports_claim_lost() {
    for status in ["active", "stale"] {
        let mut env = TestEnvGuard::temp_home();
        env.set("XDG_CONFIG_HOME", env.home().join(".config"));
        let cloud = FakeCloud::start();
        let old = machine(&cloud.url, PROJECT, "renewing-peer");
        let new = machine(&cloud.url, PROJECT, "contender");
        for m in [&old, &new] {
            m.put_task("cas-busy", TaskType::Task);
        }
        new.put_task("cas-lost", TaskType::Task);
        start(&new.core, "cas-lost", false).await.unwrap();
        cloud.live(&old, "renewing-peer");
        start(&old.core, "cas-busy", false).await.unwrap();
        {
            let mut state = cloud.state.lock().unwrap();
            state.statuses.insert(old.agent_id.clone(), status.into());
            state.agents.get_mut(&old.agent_id).unwrap().2 = Utc::now() - Duration::seconds(301);
        }
        assert!(start(&new.core, "cas-busy", false).await.is_err());
        assert_eq!(
            cloud.active_lock("cas-busy").unwrap().1.agent_id,
            old.agent_id
        );
        cloud.give_to("cas-lost", &old.agent_id, None);
        let report = cas::cloud::peer_claims::renew_agent_claims(&new.cas_dir, &new.agent_id);
        assert_eq!(report.conflicts.len(), 1, "{report:?}");
        assert!(
            cas::store::open_prompt_queue_store(&new.cas_dir)
                .unwrap()
                .peek_all(100)
                .unwrap()
                .iter()
                // pin: live competing claims must retain this alarm class even when presence is late.
                .any(|row| row.prompt.contains("PEER CLAIM LOST"))
        );
    }
}

#[tokio::test]
async fn cas_50713_changed_same_owner_snapshot_is_not_released() {
    for epoch_only in [true, false] {
        let mut env = TestEnvGuard::temp_home();
        env.set("XDG_CONFIG_HOME", env.home().join(".config"));
        let cloud = FakeCloud::start();
        let old = machine(&cloud.url, PROJECT, "ended-peer");
        let new = machine(&cloud.url, PROJECT, "contender");
        for m in [&old, &new] {
            m.put_task("cas-race", TaskType::Task);
        }
        cloud.live(&old, "ended-peer");
        start(&old.core, "cas-race", false).await.unwrap();
        {
            let mut state = cloud.state.lock().unwrap();
            state
                .statuses
                .insert(old.agent_id.clone(), "shutdown".into());
            state.renew_after_lock_reads = Some((1, epoch_only));
        }
        // A fresh stale advisory can permit local work; the changed Cloud
        // snapshot must never be released by this recovery attempt.
        let _ = start(&new.core, "cas-race", false).await;
        assert_eq!(
            cloud.active_lock("cas-race").unwrap().1.agent_id,
            old.agent_id
        );
        assert!(
            !cloud
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .any(|r| r.ends_with("/release"))
        );
    }
}


async fn assert_renewal_snapshot_race(mode: &str) {
    let mut env = TestEnvGuard::temp_home();
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let cloud = FakeCloud::start();
    let dead = machine(&cloud.url, PROJECT, "dead-peer");
    let worker = machine(&cloud.url, PROJECT, "local-worker");
    let winner = machine(&cloud.url, PROJECT, "third-live-peer");
    worker.put_task("cas-renew-race", TaskType::Task);
    start(&worker.core, "cas-renew-race", false).await.unwrap();
    cloud.live(&dead, "dead-peer");
    cloud.live(&winner, "third-live-peer");
    cloud.give_to("cas-renew-race", &dead.agent_id, None);
    {
        let mut state = cloud.state.lock().unwrap();
        state.requests.clear();
        state.return_released_rows = mode == "released-row";
        state
            .statuses
            .insert(dead.agent_id.clone(), "shutdown".into());
        match mode {
            "released" | "released-row" => state.change_after_lock_read = Some((1, None)),
            "reassigned" => state.change_after_lock_read = Some((1, Some(winner.agent_id.clone()))),
            "epoch" => state.renew_after_lock_reads = Some((1, true)),
            _ => unreachable!(),
        }
    }
    let report = cas::cloud::peer_claims::renew_agent_claims(&worker.cas_dir, &worker.agent_id);
    let rows = cas::store::open_prompt_queue_store(&worker.cas_dir)
        .unwrap()
        .peek_all(100)
        .unwrap();
    assert!(
        rows.iter().all(|row| !row.prompt.contains("dead-peer")),
        "{rows:?}"
    );
    if mode == "reassigned" {
        assert_eq!(report.conflicts.len(), 1, "{report:?}");
        assert_eq!(report.conflicts[0].1.agent_id, winner.agent_id);
        assert_eq!(report.conflicts[0].1.name(), "third-live-peer");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0].prompt.contains("third-live-peer"));
        assert_eq!(
            cloud.active_lock("cas-renew-race").unwrap().1.agent_id,
            winner.agent_id
        );
    } else {
        assert!(report.conflicts.is_empty(), "{report:?}");
        assert_eq!(report.failed, 1, "retry is advisory: {report:?}");
        assert!(rows.is_empty(), "{rows:?}");
        if mode.starts_with("released") {
            assert!(cloud.active_lock("cas-renew-race").is_none());
        } else {
            assert_eq!(
                cloud.active_lock("cas-renew-race").unwrap().1.agent_id,
                dead.agent_id
            );
        }
    }
    assert!(
        !cloud
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|r| r.ends_with("/release"))
    );
}

#[tokio::test]
async fn cas_50713_renewal_does_not_alarm_after_lock_released_between_reads() {
    for mode in ["released", "released-row"] {
        assert_renewal_snapshot_race(mode).await;
    }
}

#[tokio::test]
async fn cas_50713_renewal_names_actual_peer_after_reassignment_between_reads() {
    assert_renewal_snapshot_race("reassigned").await;
}

#[tokio::test]
async fn cas_50713_renewal_does_not_alarm_after_dead_holder_epoch_changes() {
    assert_renewal_snapshot_race("epoch").await;
}
