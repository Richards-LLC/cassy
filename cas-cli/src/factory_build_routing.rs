//! Build admission overflow uses the existing peer directory and mailbox.
use crate::cloud::peers::{PeerDiscovery, discover_peers};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) const CAPACITY_META: &str = "build_capacity_v1";
const ROUTE_MARKER: &str = "BUILD_ROUTE: ";
const FRESH_SECS: i64 = 90;
const ROUTE_LIFETIME_SECS: i64 = 15 * 60;
const EXPIRED_MARKER: &str = "BUILD_ROUTE_EXPIRED: ";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BuildCapacity {
    pub observed_at: DateTime<Utc>,
    pub free_slots: usize,
    pub load_1m: Option<f64>,
    pub cpu_count: usize,
    pub cpu_psi_avg10: Option<f64>,
    pub memory_psi_avg10: Option<f64>,
}

/// Presence samples the existing guard; pressured hosts advertise no capacity.
pub(crate) fn publish_capacity(root: &Path, agent: &mut cas_types::Agent) {
    if agent.role != cas_types::AgentRole::Supervisor {
        agent.metadata.remove(CAPACITY_META);
        return;
    }
    let config = crate::config::Config::load(root)
        .unwrap_or_default()
        .factory();
    let guard = crate::factory_build_guard::inspect_capacity(root, &config);
    let sample = guard.build_capacity(Utc::now());
    if let Ok(value) = serde_json::to_string(&sample) {
        agent.metadata.insert(CAPACITY_META.into(), value);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoutePlan {
    key: String,
    source_machine_id: String,
    sender_agent_id: String,
    recipient_agent_id: String,
    recipient: String,
    free_slots: usize,
    load_per_cpu: Option<f64>,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
}

impl RoutePlan {
    fn receipt(&self) -> String {
        let load = self
            .load_per_cpu
            .map(|load| format!("{load:.2}"))
            .unwrap_or_else(|| "unknown".into());
        format!(
            "routed to {}: {} free slots, load/CPU {load}",
            self.recipient, self.free_slots
        )
    }

    /// The existing outbox makes retries/crash recovery the same logical ask.
    fn send(
        &self,
        root: &Path,
        task: &cas_types::Task,
    ) -> Result<String, crate::cloud::peer_mailbox::PeerSendError> {
        let project_id = crate::cloud::resolve_canonical_id(root)
            .ok_or_else(|| send_error("build routing needs a canonical repository identity"))?;
        let brief = format!(
            "Build capacity overflow: please staff task {} locally, or wait for local admission. \
             Retain its BUILD_ROUTE note; never auto-route it again. Full task and supervisor brief:\n{}",
            task.id,
            serde_json::to_string_pretty(task).map_err(send_error)?
        );
        let body = crate::cloud::peer_mailbox::attach_message_intent(&brief, Some("ask"), false)
            .map_err(send_error)?;
        let message = crate::cloud::peer_mailbox::PeerSend {
            project_id,
            sender_agent_id: self.sender_agent_id.clone(),
            recipient_agent_id: self.recipient_agent_id.clone(),
            body,
            summary: Some(format!("Staff build task {}", task.id)),
            in_reply_to: None,
            dedupe_key: self.key.clone(),
            scope: crate::cloud::peer_mailbox::PeerScope::Project,
            recipient_project_id: None,
        };
        crate::cloud::peer_mailbox::send_build_route(root, message, &self.recipient)
            .map(|_| self.receipt())
    }
}

fn machine_id(sender: &cas_types::Agent) -> String {
    sender
        .machine_id
        .clone()
        .unwrap_or_else(cas_types::Agent::get_or_generate_machine_id)
}

fn has_route(task: &cas_types::Task) -> bool {
    task.notes.lines().any(|line| {
        line.starts_with(ROUTE_MARKER)
            || line.starts_with(EXPIRED_MARKER)
            || line.starts_with("build-route:")
    })
}

fn send_error(error: impl ToString) -> crate::cloud::peer_mailbox::PeerSendError {
    crate::cloud::peer_mailbox::PeerSendError::Failed {
        cause: error.to_string(),
        retry: String::new(),
    }
}

fn live_capacity(peer: &crate::cloud::peers::Peer, now: DateTime<Utc>) -> Option<BuildCapacity> {
    let fresh = |at: DateTime<Utc>| at <= now && now - at <= chrono::Duration::seconds(FRESH_SECS);
    if peer.own != Some(true)
        || !peer.live
        || !fresh(peer.last_heartbeat)
        || peer
            .session
            .as_deref()
            .is_none_or(|session| session.trim().is_empty())
    {
        return None;
    }
    let capacity: BuildCapacity = serde_json::from_value(peer.build_capacity.clone()?).ok()?;
    (fresh(capacity.observed_at) && capacity.cpu_count > 0).then_some(capacity)
}

fn expire(
    root: &Path,
    task: &mut cas_types::Task,
    plan: &RoutePlan,
    cause: &str,
) -> Result<(), String> {
    crate::cloud::peer_mailbox::cancel_build_route(root, &plan.sender_agent_id, &plan.key)?;
    task.notes = task
        .notes
        .lines()
        .map(|line| {
            if line.starts_with(ROUTE_MARKER) {
                format!(
                    "{EXPIRED_MARKER}{} peer={} expired_at={} cause={cause}",
                    plan.key,
                    plan.recipient,
                    Utc::now().to_rfc3339()
                )
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(())
}

/// Revalidate unsent asks at mailbox duty, independently of another spawn.
pub(crate) fn reconcile_retry(root: &Path, sender: &str, key: &str, send_failed: bool) -> Result<bool, String> {
    let task_id = key.rsplit(':').next().ok_or("build route has no task id")?;
    let tasks = crate::store::open_task_store(root).map_err(|error| error.to_string())?;
    let expected = match tasks.get(task_id) {
        Ok(task) => task,
        Err(crate::store::StoreError::TaskNotFound(_) | crate::store::StoreError::NotFound(_)) => {
            crate::cloud::peer_mailbox::cancel_build_route(root, sender, key)?;
            return Ok(true);
        }
        Err(error) => return Err(error.to_string()),
    };
    let plan = expected.notes.lines().find_map(|line| {
        serde_json::from_str::<RoutePlan>(line.strip_prefix(ROUTE_MARKER)?).ok()
            .filter(|plan| plan.key == key && plan.sender_agent_id == sender)
    });
    let Some(plan) = plan else {
        crate::cloud::peer_mailbox::cancel_build_route(root, sender, key)?;
        return Ok(true);
    };
    let enabled = crate::config::Config::load(root).map_err(|error| error.to_string())?.factory().auto_route_builds;
    let conn = rusqlite::Connection::open(root.join("cas.db")).map_err(|error| error.to_string())?;
    cas_store::shared_db::install_busy_handler(&conn).map_err(|error| error.to_string())?;
    // Batch spawns reserve a task before a worker has an assignee. Bound
    // backlogs retain that reservation after the spawn request is processed.
    let queued: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM spawn_queue s LEFT JOIN spawn_task_queues q ON q.request_id=s.id
         WHERE s.action='spawn' AND (s.processed_at IS NULL OR s.spawn_state IN ('queued','provisioning','launched'))
         AND (s.task_id=?1 OR EXISTS(SELECT 1 FROM json_each(q.task_ids) WHERE value=?1)))
         OR EXISTS(SELECT 1 FROM worker_task_queues w WHERE json_extract(w.state,'$.current_task')=?1
         OR EXISTS(SELECT 1 FROM json_each(w.state,'$.pending') WHERE value=?1))",
        [task_id], |row| row.get(0),
    ).map_err(|error| error.to_string())?;
    let cause = if send_failed { "send_failed" }
        else if !enabled { "routing_disabled" }
        else if expected.assignee.is_some() || expected.status != cas_types::TaskStatus::Open || queued { "staffed_locally" }
        else { return Ok(false) };
    let mut task = expected.clone();
    expire(root, &mut task, &plan, cause)?;
    tasks.update_if_current(&task, &expected).map_err(|error| error.to_string())?;
    Ok(true)
}

fn prepare(
    root: &Path,
    sender: &cas_types::Agent,
    task: &mut cas_types::Task,
    brief: Option<&str>,
    config: &crate::config::FactoryConfig,
    guard: &crate::factory_build_guard::BuildGuardSnapshot,
) -> Result<Option<RoutePlan>, String> {
    if sender.role != cas_types::AgentRole::Supervisor
        || task.status != cas_types::TaskStatus::Open
        || task.assignee.is_some()
        || matches!(
            task.task_type,
            cas_types::TaskType::Epic | cas_types::TaskType::Gate
        )
    {
        return Ok(None);
    }
    let agents = crate::store::open_agent_store(root).map_err(|error| error.to_string())?;
    if agents
        .get_lease(&task.id)
        .map_err(|error| error.to_string())?
        .is_some_and(|lease| lease.is_valid())
    {
        return Ok(None);
    }
    // A routed task never moves onward. Its source may replay only while
    // the selected peer and sample are fresh and the route is under 15 min.
    if has_route(task) {
        let plan = task.notes.lines().find_map(|line| {
            serde_json::from_str::<RoutePlan>(line.strip_prefix(ROUTE_MARKER)?).ok()
        });
        let Some(plan) = plan.filter(|plan| plan.source_machine_id == machine_id(sender)) else {
            return Ok(None);
        };
        let now = Utc::now();
        let in_window = plan.created_at.is_some_and(|created| {
            created <= now && now - created < chrono::Duration::seconds(ROUTE_LIFETIME_SECS)
        });
        let peer_ready = config.auto_route_builds
            && in_window
            && discover_peers(root, Some(&sender.id))
                .ok()
                .is_some_and(|discovery| {
                    let PeerDiscovery::Found { peers, .. } = discovery else {
                        return false;
                    };
                    peers.iter().any(|peer| {
                        peer.agent_id == plan.recipient_agent_id
                            && live_capacity(peer, now).is_some()
                    })
                });
        if peer_ready {
            return Ok(Some(plan));
        }
        expire(
            root,
            task,
            &plan,
            if !config.auto_route_builds {
                "automatic routing disabled"
            } else if !in_window {
                "route reached its 15-minute lifetime"
            } else {
                "peer or capacity is no longer fresh"
            },
        )?;
        return Ok(None);
    }
    if !config.auto_route_builds || guard.requested_builders == 0 || guard.violations().is_empty() {
        return Ok(None);
    }
    // Discovery outages retain the caller's existing local admission behaviour.
    let Ok(PeerDiscovery::Found {
        canonical_id,
        peers,
        ..
    }) = discover_peers(root, Some(&sender.id))
    else {
        return Ok(None);
    };
    let now = Utc::now();
    let recent =
        crate::cloud::peer_mailbox::build_route_recent_peers(root, now.timestamp() - FRESH_SECS)?;
    let local_machine = machine_id(sender);
    let mut candidates = peers
        .into_iter()
        .filter_map(|peer| {
            if peer
                .machine_id
                .as_deref()
                .is_none_or(|id| id == local_machine)
                || recent.contains(&peer.agent_id)
            {
                return None;
            }
            let capacity = live_capacity(&peer, now)?;
            if capacity.free_slots == 0 {
                return None;
            }
            let ratio = capacity
                .load_1m
                .filter(|load| load.is_finite() && *load >= 0.0)
                .map(|load| load / capacity.cpu_count as f64);
            Some((peer, capacity, ratio))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.1.free_slots
            .cmp(&a.1.free_slots)
            .then_with(|| {
                a.2.unwrap_or(f64::INFINITY)
                    .total_cmp(&b.2.unwrap_or(f64::INFINITY))
            })
            .then_with(|| a.0.agent_id.cmp(&b.0.agent_id))
    });
    let Some((peer, capacity, ratio)) = candidates.into_iter().next() else {
        return Ok(None);
    };
    let plan = RoutePlan {
        key: format!("build-route:{canonical_id}:{}", task.id),
        source_machine_id: local_machine,
        sender_agent_id: sender.id.clone(),
        recipient_agent_id: peer.agent_id,
        recipient: format!("{}@{}", peer.name, peer.machine),
        free_slots: capacity.free_slots,
        load_per_cpu: ratio,
        created_at: Some(now),
    };
    if let Some(brief) = brief {
        task.notes.push_str(&format!("\n📌 SPAWN BRIEF {brief}\n"));
    }
    task.notes.push_str(&format!(
        "\n{ROUTE_MARKER}{}\n",
        serde_json::to_string(&plan).map_err(|error| error.to_string())?
    ));
    Ok(Some(plan))
}

pub(crate) fn route(
    root: &Path,
    sender: &cas_types::Agent,
    task_id: &str,
    brief: Option<&str>,
    config: &crate::config::FactoryConfig,
    guard: &crate::factory_build_guard::BuildGuardSnapshot,
) -> Result<Option<String>, String> {
    let tasks = crate::store::open_task_store(root).map_err(|error| error.to_string())?;
    let expected = tasks.get(task_id).map_err(|error| error.to_string())?;
    let mut task = expected.clone();
    let plan = prepare(root, sender, &mut task, brief, config, guard)?;
    if task.notes != expected.notes {
        task.updated_at = tasks
            .update_if_current(&task, &expected)
            .map_err(|error| error.to_string())?;
    }
    let Some(plan) = plan else { return Ok(None) };
    match plan.send(root, &task) {
        Ok(receipt) => Ok(Some(receipt)),
        Err(crate::cloud::peer_mailbox::PeerSendError::BuildRouteCooldown(_)) => {
            let expected = task.clone();
            expire(root, &mut task, &plan, "peer routing cooldown")?;
            tasks
                .update_if_current(&task, &expected)
                .map_err(|error| error.to_string())?;
            Ok(None)
        }
        Err(error) => {
            let current = tasks.get(task_id).map_err(|error| error.to_string())?;
            if current.notes.lines().any(|line| line.starts_with(&format!("{EXPIRED_MARKER}{} ", plan.key))) {
                return Ok(None);
            }
            Err(format!("Build route to {} retained; local dispatch withheld: {error}", plan.recipient))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const REPO: &str = "github.com/acme/widgets";

    fn peer(id: &str, slots: usize, load: f64, age: i64) -> serde_json::Value {
        serde_json::json!({
            "id": id, "name": id, "agent_type": "primary", "status": "active",
            "last_heartbeat": (Utc::now() - Duration::seconds(age)).to_rfc3339(),
            "active_tasks": 0, "machine_id": format!("machine-{id}"),
            "user_id": "operator-owner",
            "metadata": {
                "role": "supervisor", "canonical_id": REPO,
                "factory_session": "peer-factory", "hostname": id,
                "build_capacity_v1": serde_json::json!({
                    "observed_at": (Utc::now() - Duration::seconds(age)).to_rfc3339(),
                    "free_slots": slots, "load_1m": load, "cpu_count": 8,
                    "cpu_psi_avg10": null, "memory_psi_avg10": null
                }).to_string()
            }
        })
    }

    fn directory(mut peers: Vec<serde_json::Value>) -> serde_json::Value {
        peers.push(peer("local-sup", 0, 0.0, 0));
        serde_json::json!({"agents": peers})
    }

    async fn failed_route_fixture(server: &MockServer, project: &Path) -> (std::path::PathBuf, cas_types::Agent) {
        let root = crate::store::init_cas_dir(project).unwrap();
        crate::cloud::set_canonical_id_in_config_toml(&root, REPO).unwrap();
        crate::cloud::CloudConfig {
            endpoint: server.uri(), token: Some("test-token".into()), ..Default::default()
        }.save_to_cas_dir(&root).unwrap();
        let mut sender = cas_types::Agent::new("local-sup".into(), "local-sup".into());
        sender.role = cas_types::AgentRole::Supervisor;
        sender.factory_session = Some("local".into());
        crate::store::open_agent_store(&root).unwrap().register(&sender).unwrap();
        crate::store::open_task_store(&root).unwrap().add(&cas_types::Task::new(
            "cas-retry".into(), "Failed build route".into(),
        )).unwrap();
        Mock::given(method("GET")).and(path("/api/agents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(directory(vec![peer("best", 3, 4.0, 0)])))
            .mount(server).await;
        Mock::given(method("POST")).and(path("/api/peer-messages"))
            .respond_with(ResponseTemplate::new(503)).mount(server).await;
        (root, sender)
    }

    async fn retry_route(root: &Path, sender: &cas_types::Agent) -> Result<Option<String>, String> {
        let (root, sender) = (root.to_path_buf(), sender.clone());
        tokio::task::spawn_blocking(move || route(
            &root, &sender, "cas-retry", None, &Default::default(),
            &crate::factory_build_guard::evaluate(8, Some(24.0), 5, 1, 5),
        )).await.unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cas_656d_persistent_send_failure_expires_and_restores_local_fallback() {
        let server = MockServer::start().await;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_USER_CLOUD_JSON", env.home().join("no-cloud.json"));
        let (root, sender) = failed_route_fixture(&server, env.home()).await;
        let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        for attempt in 1..=3 {
            conn.execute("UPDATE peer_message_outbox SET next_attempt_at=0", []).unwrap();
            let result = retry_route(&root, &sender).await;
            if attempt < 3 {
                assert!(result.is_err(), "transient failure {attempt}: {result:?}");
            } else {
                assert!(matches!(result, Ok(None)), "persistent send failure must release local admission: {result:?}");
            }
        }
        let task = crate::store::open_task_store(&root).unwrap().get("cas-retry").unwrap();
        assert!(task.notes.contains("BUILD_ROUTE_EXPIRED:") && task.notes.contains("cause=send_failed"), "{}", task.notes);
        let status = crate::cloud::peer_mailbox::outbox_status_text(&root, "peer-outbox:1").unwrap().unwrap();
        assert!(status.contains("abandoned") && status.contains("attempts: 3"), "{status}");
        assert!(retry_route(&root, &sender).await.unwrap().is_none(), "expired routes must not replay");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cas_656d_mailbox_cancels_obsolete_routes_before_the_retry_is_due() {
        let server = MockServer::start().await;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_USER_CLOUD_JSON", env.home().join("no-cloud.json"));
        for case in ["revalidation-error", "due-revalidation-error", "disabled", "assigned", "batch", "interrupted-cancel"] {
            server.reset().await;
            let project = env.home().join(case);
            std::fs::create_dir_all(&project).unwrap();
            let (root, sender) = failed_route_fixture(&server, &project).await;
            assert!(retry_route(&root, &sender).await.is_err());
            let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
            conn.execute("UPDATE peer_message_outbox SET next_attempt_at=?1", [Utc::now().timestamp() + 3600]).unwrap();
            match case {
                "disabled" => {
                    let mut config = crate::config::Config::load(&root).unwrap();
                    config.set("factory.auto_route_builds", "false").unwrap();
                    config.save(&root).unwrap();
                }
                "assigned" | "interrupted-cancel" => {
                    let tasks = crate::store::open_task_store(&root).unwrap();
                    let mut task = tasks.get("cas-retry").unwrap();
                    task.assignee = Some("local-worker".into());
                    tasks.update(&task).unwrap();
                    if case == "interrupted-cancel" {
                        crate::cloud::peer_mailbox::cancel_build_route(&root, &sender.id, &format!("build-route:{REPO}:cas-retry")).unwrap();
                    }
                }
                "revalidation-error" | "due-revalidation-error" => {
                    let send_root = root.clone();
                    assert!(tokio::task::spawn_blocking(move || crate::cloud::peer_mailbox::send_raw(
                        &send_root, "local-sup", "best", "ordinary peer retry", None, None, "best@best",
                    )).await.unwrap().is_err());
                    conn.execute("UPDATE peer_message_outbox SET next_attempt_at=0 WHERE id=2", []).unwrap();
                    // Keep the route JSON valid/selectable, but make its typed
                    // payload unreadable; the unrelated send behind it is due.
                    conn.execute("UPDATE peer_message_outbox SET payload=json_remove(payload,'$.project_id') WHERE id=1", []).unwrap();
                    if case == "due-revalidation-error" {
                        conn.execute("UPDATE peer_message_outbox SET next_attempt_at=0 WHERE id=1", []).unwrap();
                    }
                    server.reset().await;
                    Mock::given(method("POST")).and(path("/api/peer-messages"))
                        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":"ordinary-sent", "status":"queued"})))
                        .mount(&server).await;
                }
                _ => {
                    crate::store::open_spawn_queue_store(&root).unwrap().enqueue_spawn_with_task_queue(
                        1, &[], true, None, Some("local"), None, None, None, &["cas-retry".into()],
                    ).unwrap();
                }
            }
            let runtime = crate::cloud::peer_mailbox::PeerMailboxRuntime::start(&root, "local").unwrap();
            tokio::task::spawn_blocking(move || runtime.tick_blocking()).await.unwrap();
            let status = crate::cloud::peer_mailbox::outbox_status_text(&root, "peer-outbox:1").unwrap().unwrap();
            if case.ends_with("revalidation-error") {
                let ordinary = crate::cloud::peer_mailbox::outbox_status_text(&root, "peer-outbox:2").unwrap().unwrap();
                assert!(ordinary.contains(": sent"), "one bad route must not block ordinary peer retries: {ordinary}");
                assert!(status.contains("retrying") && status.contains("attempts: 1"), "bad route must remain pending: {status}");
                assert_eq!(server.received_requests().await.unwrap().iter().filter(|r| r.method == "POST" && r.url.path() == "/api/peer-messages").count(), 1);
                continue;
            }
            assert!(status.contains("abandoned"), "{case}: cancel eagerly without another spawn or due retry: {status}");
            let notes = crate::store::open_task_store(&root).unwrap().get("cas-retry").unwrap().notes;
            assert!(notes.contains(EXPIRED_MARKER), "{case}");
            if case == "interrupted-cancel" { assert!(notes.contains("cause=staffed_locally"), "{notes}"); }
            assert_eq!(server.received_requests().await.unwrap().iter().filter(|r| r.method == "POST" && r.url.path() == "/api/peer-messages").count(), 1, "{case}: no obsolete retry may send");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cas_d1fe_routes_admission_overflow_once_and_preserves_local_fallbacks() {
        let server = MockServer::start().await;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_USER_CLOUD_JSON", env.home().join("no-cloud.json"));
        let root = crate::store::init_cas_dir(env.home()).unwrap();
        crate::cloud::set_canonical_id_in_config_toml(&root, REPO).unwrap();
        let cloud = crate::cloud::CloudConfig {
            endpoint: server.uri(),
            token: Some("synthetic-test-token".into()),
            ..Default::default()
        };
        cloud.save_to_cas_dir(&root).unwrap();
        let mut sender = cas_types::Agent::new("local-sup".into(), "local-sup".into());
        sender.role = cas_types::AgentRole::Supervisor;
        sender.factory_session = Some("local-factory".into());
        sender.metadata.insert("hostname".into(), "local".into());
        crate::store::open_agent_store(&root)
            .unwrap()
            .register(&sender)
            .unwrap();
        let tasks = crate::store::open_task_store(&root).unwrap();
        let sample_only = |age| {
            let mut row = peer("best", 3, 4.0, age);
            row["last_heartbeat"] = Utc::now().to_rfc3339().into();
            row
        };
        let mut teammate = peer("best", 3, 4.0, 0);
        teammate["user_id"] = "other-owner".into();
        let mut unknown_owner = peer("best", 3, 4.0, 0);
        unknown_owner.as_object_mut().unwrap().remove("user_id");
        for (case, peers, off, note, expect_route) in [
            (
                "fresh",
                vec![
                    peer("few", 1, 1.0, 0),
                    peer("busy", 3, 8.0, 0),
                    peer("best", 3, 4.0, 0),
                ],
                false,
                "",
                true,
            ),
            ("absent", vec![], false, "", false),
            ("stale", vec![peer("best", 3, 4.0, 91)], false, "", false),
            ("future", vec![peer("best", 3, 4.0, -30)], false, "", false),
            ("old-sample", vec![sample_only(91)], false, "", false),
            ("future-sample", vec![sample_only(-30)], false, "", false),
            ("saturated", vec![peer("best", 0, 4.0, 0)], false, "", false),
            ("healthy", vec![peer("best", 3, 4.0, 0)], false, "", false),
            ("off", vec![peer("best", 3, 4.0, 0)], true, "", false),
            ("teammate", vec![teammate], false, "", false),
            ("unknown-owner", vec![unknown_owner], false, "", false),
            (
                "pingpong",
                vec![peer("best", 3, 4.0, 0)],
                false,
                "build-route:github.com/acme/widgets:cas-pingpong from another-machine",
                false,
            ),
        ] {
            server.reset().await;
            Mock::given(method("GET"))
                .and(path("/api/agents"))
                .respond_with(ResponseTemplate::new(200).set_body_json(directory(peers)))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/api/peer-messages"))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"id": format!("message-{case}"), "status": "queued"}),
                ))
                .mount(&server)
                .await;
            let mut task = cas_types::Task::new(format!("cas-{case}"), "Compile change".into());
            task.description = "Full task description".into();
            task.notes = note.into();
            tasks.add(&task).unwrap();
            let config: crate::config::Config =
                toml::from_str(&format!("[factory]\nauto_route_builds = {}\n", !off)).unwrap();
            let config = config.factory();
            let guard = if case == "healthy" {
                crate::factory_build_guard::evaluate(8, Some(1.0), 0, 1, 5)
            } else {
                crate::factory_build_guard::evaluate(8, Some(24.0), 5, 1, 5)
            };
            let (root_copy, sender_copy, id) = (root.clone(), sender.clone(), task.id.clone());
            let outcome = tokio::task::spawn_blocking(move || {
                route(
                    &root_copy,
                    &sender_copy,
                    &id,
                    Some("Supervisor full brief"),
                    &config,
                    &guard,
                )
            })
            .await
            .unwrap()
            .unwrap();
            assert_eq!(outcome.is_some(), expect_route, "{case}: {outcome:?}");
            if let Some(receipt) = outcome {
                assert!(
                    receipt.contains("best")
                        && receipt.contains("3 free slots")
                        && receipt.contains("0.50"),
                    "{receipt}"
                );
                let updated = tasks.get(&task.id).unwrap();
                assert!(updated.assignee.is_none());
                assert!(updated.notes.contains("build-route:") && updated.notes.contains("best"));
                let (root_copy, sender_copy, id) = (root.clone(), sender.clone(), task.id.clone());
                tokio::task::spawn_blocking(move || {
                    route(
                        &root_copy,
                        &sender_copy,
                        &id,
                        None,
                        &Default::default(),
                        &crate::factory_build_guard::evaluate(8, Some(24.0), 5, 1, 5),
                    )
                })
                .await
                .unwrap()
                .unwrap();
            }
            let sends = server
                .received_requests()
                .await
                .unwrap()
                .into_iter()
                .filter(|r| r.method == "POST" && r.url.path() == "/api/peer-messages")
                .collect::<Vec<_>>();
            assert_eq!(
                sends.len(),
                usize::from(expect_route),
                "{case}: no duplicate route"
            );
            if expect_route {
                let body: serde_json::Value = serde_json::from_slice(&sends[0].body).unwrap();
                assert_eq!(body["recipient_agent_id"], "best");
                assert!(
                    body["body"]
                        .as_str()
                        .unwrap()
                        .contains("Full task description")
                );
                assert!(
                    body["body"]
                        .as_str()
                        .unwrap()
                        .contains("Supervisor full brief")
                );
                assert!(body["body"].as_str().unwrap().contains("ask"));
                assert_eq!(
                    body["dedupe_key"],
                    format!("build-route:{REPO}:{}", task.id)
                );
            } else {
                assert_eq!(tasks.get(&task.id).unwrap().notes, note);
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cas_d1fe_only_spawn_routes_and_idle_assignments_stay_local() {
        use crate::mcp::tools::{TaskCreateRequest, TaskUpdateRequest};
        use crate::mcp::{CasCore, CasService};
        use cas_mcp::FactoryRequest;
        use rmcp::handler::server::wrapper::Parameters;
        let server = MockServer::start().await;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_USER_CLOUD_JSON", env.home().join("no-cloud.json"));
        env.set("CAS_FACTORY_BUILD_GUARD", "on");
        env.set("CAS_FACTORY_MODE", "1");
        let repo = env.home().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--initial-branch=main"]);
        git(&["config", "user.name", "Routing fixture"]);
        git(&["config", "user.email", "routing@example.test"]);
        std::fs::write(repo.join("tracked.txt"), "base\n").unwrap();
        git(&["add", "tracked.txt"]);
        git(&["commit", "-m", "base"]);
        git(&["branch", "factory/idle"]);
        std::fs::write(repo.join("tracked.txt"), "new main\n").unwrap();
        git(&["commit", "-am", "advance target"]);
        git(&["checkout", "factory/idle"]);
        let root = crate::store::init_cas_dir(&repo).unwrap();
        crate::cloud::set_canonical_id_in_config_toml(&root, REPO).unwrap();
        let mut config = crate::config::Config::load(&root).unwrap();
        config.set("factory.max_concurrent_builders", "1").unwrap();
        config.set("factory.stale_threshold_commits", "1").unwrap();
        config.save(&root).unwrap();
        let slots = root.join("worker-check-slots");
        std::fs::create_dir_all(&slots).unwrap();
        let occupied = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(slots.join("slot-0.lock"))
            .unwrap();
        fs2::FileExt::lock_exclusive(&occupied).unwrap();
        crate::cloud::CloudConfig {
            endpoint: server.uri(),
            token: Some("synthetic-test-token".into()),
            ..Default::default()
        }
        .save_to_cas_dir(&root)
        .unwrap();
        Mock::given(method("GET"))
            .and(path("/api/agents"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(directory(vec![peer("best", 3, 4.0, 0)])),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/peer-messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id":"route-receipt", "status":"queued"})),
            )
            .mount(&server)
            .await;
        let mut sender = cas_types::Agent::new("local-sup".into(), "local-sup".into());
        sender.role = cas_types::AgentRole::Supervisor;
        sender.factory_session = Some("local-factory".into());
        let mut worker = cas_types::Agent::new("idle-worker".into(), "idle".into());
        worker.role = cas_types::AgentRole::Worker;
        worker.status = cas_types::AgentStatus::Idle;
        worker.factory_session = sender.factory_session.clone();
        worker
            .metadata
            .insert("clone_path".into(), repo.to_string_lossy().into_owned());
        let agents = crate::store::open_agent_store(&root).unwrap();
        agents.register(&sender).unwrap();
        agents.register(&worker).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        core.set_agent_id_for_testing(sender.id.clone());
        let tasks = core.open_task_store().unwrap();
        for action in ["update", "create", "spawn"] {
            let id = format!("cas-{action}");
            if action != "create" {
                let mut task = cas_types::Task::new(id.clone(), format!("Overflow {action}"));
                task.deliverables.work_target = Some(cas_types::WorkTarget {
                    repo_selector: format!("project:{REPO}"),
                    target_branch: "main".into(),
                });
                tasks.add(&task).unwrap();
            }
            let result = match action {
                "update" => {
                    let req: TaskUpdateRequest =
                        serde_json::from_value(serde_json::json!({"id":id, "assignee":"idle"}))
                            .unwrap();
                    core.cas_task_update(Parameters(req)).await.unwrap()
                }
                "create" => {
                    // Public create has no explicit target; use a current
                    // checkout. The update row exercises the stale idle
                    // worker with its declared main target unchanged.
                    git(&["checkout", "main"]);
                    let req: TaskCreateRequest = serde_json::from_value(serde_json::json!({"title":"Overflow create", "assignee":"idle", "risk":"none"})).unwrap();
                    core.cas_task_create(Parameters(req)).await.unwrap()
                }
                _ => {
                    #[cfg(feature = "mcp-proxy")]
                    let service = CasService::new(core.clone(), None);
                    #[cfg(not(feature = "mcp-proxy"))]
                    let service = CasService::new(core.clone());
                    let ctx = crate::ops::fleet::FleetContext {
                        factory_session: sender.factory_session.clone(),
                        owned_workers: None,
                        supervisor_authorized: true,
                        requester_from_env: true,
                    };
                    let req: FactoryRequest = serde_json::from_value(
                        serde_json::json!({"action":"spawn_workers", "task_id":id}),
                    )
                    .unwrap();
                    service.factory_spawn_workers_in(&ctx, req).await.unwrap()
                }
            };
            let text = result
                .content
                .into_iter()
                .filter_map(|content| match content.raw {
                    rmcp::model::RawContent::Text(text) => Some(text.text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                text.contains("routed to best@best: 3 free slots, load/CPU 0.50")
                    == (action == "spawn"),
                "{action}: {text}"
            );
            let routed = tasks
                .list(None)
                .unwrap()
                .into_iter()
                .find(|task| task.title == format!("Overflow {action}"))
                .unwrap();
            assert_eq!(
                routed.assignee.as_deref(),
                if action == "spawn" {
                    None
                } else {
                    Some("idle")
                },
                "{action}"
            );
            assert_eq!(routed.notes.contains("build-route:"), action == "spawn");
        }
        assert_eq!(
            crate::store::open_spawn_queue_store(&root)
                .unwrap()
                .pending_count()
                .unwrap(),
            0
        );
        let sends = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.method == "POST" && r.url.path() == "/api/peer-messages")
            .count();
        assert_eq!(sends, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cas_d1fe_expired_and_disabled_routes_release_local_admission() {
        let server = MockServer::start().await;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_USER_CLOUD_JSON", env.home().join("no-cloud.json"));
        for case in [
            "absent",
            "stale-sample",
            "old-route",
            "disabled",
            "pending-disabled",
        ] {
            server.reset().await;
            let project = env.home().join(case);
            std::fs::create_dir_all(&project).unwrap();
            let root = crate::store::init_cas_dir(&project).unwrap();
            crate::cloud::set_canonical_id_in_config_toml(&root, REPO).unwrap();
            crate::cloud::CloudConfig {
                endpoint: server.uri(),
                token: Some("test-token".into()),
                ..Default::default()
            }
            .save_to_cas_dir(&root)
            .unwrap();
            let mut sender = cas_types::Agent::new("local-sup".into(), "local-sup".into());
            sender.role = cas_types::AgentRole::Supervisor;
            sender.factory_session = Some("local".into());
            crate::store::open_agent_store(&root)
                .unwrap()
                .register(&sender)
                .unwrap();
            Mock::given(method("GET"))
                .and(path("/api/agents"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(directory(vec![peer("best", 3, 4.0, 0)])),
                )
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/api/peer-messages"))
                .respond_with(
                    ResponseTemplate::new(if case == "pending-disabled" { 503 } else { 200 })
                        .set_body_json(serde_json::json!({"id": "sent", "status": "queued"})),
                )
                .mount(&server)
                .await;
            let tasks = crate::store::open_task_store(&root).unwrap();
            tasks
                .add(&cas_types::Task::new(
                    "cas-release".into(),
                    "Release expired route".into(),
                ))
                .unwrap();
            let invoke = |root: std::path::PathBuf, sender: cas_types::Agent, off: bool| {
                tokio::task::spawn_blocking(move || {
                    let config = crate::config::FactoryConfig {
                        auto_route_builds: !off,
                        ..Default::default()
                    };
                    route(
                        &root,
                        &sender,
                        "cas-release",
                        None,
                        &config,
                        &crate::factory_build_guard::evaluate(8, Some(24.0), 5, 1, 5),
                    )
                })
            };
            let initial = invoke(root.clone(), sender.clone(), false).await.unwrap();
            if case == "pending-disabled" {
                assert!(initial.is_err());
            } else {
                assert!(initial.unwrap().is_some());
            }
            if case == "old-route" {
                let mut task = tasks.get("cas-release").unwrap();
                task.notes = task
                    .notes
                    .lines()
                    .map(|line| {
                        if let Some(json) = line.strip_prefix(ROUTE_MARKER) {
                            let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
                            value["created_at"] =
                                (Utc::now() - Duration::minutes(16)).to_rfc3339().into();
                            format!("{ROUTE_MARKER}{value}")
                        } else {
                            line.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                tasks.update(&task).unwrap();
            }
            server.reset().await;
            let mut remote = peer("best", 3, 4.0, if case == "stale-sample" { 91 } else { 0 });
            remote["last_heartbeat"] = Utc::now().to_rfc3339().into();
            let peers = if case == "absent" {
                vec![]
            } else {
                vec![remote]
            };
            Mock::given(method("GET"))
                .and(path("/api/agents"))
                .respond_with(ResponseTemplate::new(200).set_body_json(directory(peers)))
                .mount(&server)
                .await;
            let result = invoke(root.clone(), sender, case.ends_with("disabled"))
                .await
                .unwrap()
                .unwrap();
            assert!(
                result.is_none(),
                "{case}: route must release instead of replay: {result:?}"
            );
            let updated = tasks.get("cas-release").unwrap();
            assert!(
                updated.notes.to_ascii_lowercase().contains("expired"),
                "{case}: {}",
                updated.notes
            );
            assert!(updated.assignee.is_none());
            if case == "pending-disabled" {
                let status = crate::cloud::peer_mailbox::outbox_status_text(&root, "peer-outbox:1")
                    .unwrap()
                    .unwrap();
                assert!(
                    status.contains("abandoned") && status.contains("expired"),
                    "{status}"
                );
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cas_d1fe_bursts_route_to_each_peer_at_most_once_per_window() {
        let server = MockServer::start().await;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_USER_CLOUD_JSON", env.home().join("no-cloud.json"));
        let root = crate::store::init_cas_dir(env.home()).unwrap();
        crate::cloud::set_canonical_id_in_config_toml(&root, REPO).unwrap();
        crate::cloud::CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".into()),
            ..Default::default()
        }
        .save_to_cas_dir(&root)
        .unwrap();
        let mut sender = cas_types::Agent::new("local-sup".into(), "local-sup".into());
        sender.role = cas_types::AgentRole::Supervisor;
        sender.factory_session = Some("local".into());
        crate::store::open_agent_store(&root)
            .unwrap()
            .register(&sender)
            .unwrap();
        Mock::given(method("GET"))
            .and(path("/api/agents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(directory(vec![
                peer("best", 3, 4.0, 0),
                peer("other", 2, 1.0, 0),
            ])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/peer-messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": "sent", "status": "queued"})),
            )
            .mount(&server)
            .await;
        for (id, recipient) in [
            ("cas-first", Some("best@best")),
            ("cas-second", Some("other@other")),
            ("cas-third", None),
        ] {
            crate::store::open_task_store(&root)
                .unwrap()
                .add(&cas_types::Task::new(id.into(), "Burst task".into()))
                .unwrap();
            let (root, sender) = (root.clone(), sender.clone());
            let result = tokio::task::spawn_blocking(move || {
                route(
                    &root,
                    &sender,
                    id,
                    None,
                    &Default::default(),
                    &crate::factory_build_guard::evaluate(8, Some(24.0), 5, 1, 5),
                )
            })
            .await
            .unwrap()
            .unwrap();
            match recipient {
                Some(recipient) => assert!(
                    result
                        .as_deref()
                        .is_some_and(|text| text.contains(recipient)),
                    "{id}: {result:?}"
                ),
                None => assert!(result.is_none(), "{id}: {result:?}"),
            }
        }
        // A caller that passed selection before another spawn committed must
        // still lose at the durable outbox's atomic cooldown check.
        let sent = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|request| request.method == "POST" && request.url.path() == "/api/peer-messages")
            .unwrap();
        let mut raced: crate::cloud::peer_mailbox::PeerSend =
            serde_json::from_slice(&sent.body).unwrap();
        raced.dedupe_key = format!("build-route:{REPO}:cas-raced");
        let error = tokio::task::spawn_blocking(move || {
            crate::cloud::peer_mailbox::send_build_route(&root, raced, "best@best")
        })
        .await
        .unwrap()
        .unwrap_err();
        assert!(
            matches!(
                error,
                crate::cloud::peer_mailbox::PeerSendError::BuildRouteCooldown(_)
            ),
            "{error}"
        );
    }
}
