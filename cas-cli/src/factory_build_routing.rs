//! Build admission overflow uses the existing peer directory and mailbox.
use crate::cloud::peers::{PeerDiscovery, discover_peers};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) const CAPACITY_META: &str = "build_capacity_v1";
const ROUTE_MARKER: &str = "BUILD_ROUTE: ";
const FRESH_SECS: i64 = 90;

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
pub(crate) struct RoutePlan {
    key: String,
    source_machine_id: String,
    sender_agent_id: String,
    recipient_agent_id: String,
    recipient: String,
    free_slots: usize,
    load_per_cpu: Option<f64>,
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
    pub(crate) fn send(&self, root: &Path, task: &cas_types::Task) -> Result<String, String> {
        let project_id = crate::cloud::resolve_canonical_id(root)
            .ok_or_else(|| "build routing needs a canonical repository identity".to_string())?;
        let brief = format!(
            "Build capacity overflow: please staff task {} locally, or wait for local admission. \
             Retain its BUILD_ROUTE note; never auto-route it again. Full task and supervisor brief:\n{}",
            task.id,
            serde_json::to_string_pretty(task).map_err(|error| error.to_string())?
        );
        let body = crate::cloud::peer_mailbox::attach_message_intent(&brief, Some("ask"), false)?;
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
            .map_err(|error| {
                format!(
                    "Build route to {} retained; local dispatch withheld: {error}",
                    self.recipient
                )
            })
    }
}

fn machine_id(sender: &cas_types::Agent) -> String {
    sender
        .machine_id
        .clone()
        .unwrap_or_else(cas_types::Agent::get_or_generate_machine_id)
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
    // Any prior route prevents ping-pong. Only the original sender may replay
    // its exact persisted selection, including after a crash before HTTP send.
    if task.notes.contains("build-route:") {
        return Ok(task.notes.lines().find_map(|line| {
            let plan: RoutePlan = serde_json::from_str(line.strip_prefix(ROUTE_MARKER)?).ok()?;
            (plan.source_machine_id == machine_id(sender)).then_some(plan)
        }));
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
    let fresh = |at: DateTime<Utc>| at <= now && now - at <= chrono::Duration::seconds(FRESH_SECS);
    let local_machine = machine_id(sender);
    let mut candidates = peers
        .into_iter()
        .filter_map(|peer| {
            if !peer.live
                || !fresh(peer.last_heartbeat)
                || peer
                    .session
                    .as_deref()
                    .is_none_or(|session| session.trim().is_empty())
                || peer
                    .machine_id
                    .as_deref()
                    .is_none_or(|id| id == local_machine)
            {
                return None;
            }
            let capacity: BuildCapacity =
                serde_json::from_value(peer.build_capacity.clone()?).ok()?;
            if !fresh(capacity.observed_at) || capacity.free_slots == 0 || capacity.cpu_count == 0 {
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

/// Assignment callers stage the route in the same task mutation as the other
/// validated fields, before any local assignment prompt can be enqueued.
pub(crate) fn prepare_assignment(
    root: &Path,
    sender: &cas_types::Agent,
    task: &mut cas_types::Task,
) -> Result<Option<RoutePlan>, String> {
    let config = crate::config::Config::load(root)
        .unwrap_or_default()
        .factory();
    if !config.auto_route_builds && !task.notes.contains("build-route:") {
        return Ok(None);
    }
    let guard = crate::factory_build_guard::inspect_for_task(root, &config, 1, Some(task));
    let assignee = task.assignee.take();
    let plan = prepare(root, sender, task, None, &config, &guard)?;
    if plan.is_none() {
        task.assignee = assignee;
    }
    Ok(plan)
}

pub(crate) async fn deliver(
    plan: RoutePlan,
    root: std::path::PathBuf,
    task: cas_types::Task,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || plan.send(&root, &task))
        .await
        .map_err(|error| error.to_string())?
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
    let Some(plan) = prepare(root, sender, &mut task, brief, config, guard)? else {
        return Ok(None);
    };
    if task.notes != expected.notes {
        task.updated_at = tasks
            .update_if_current(&task, &expected)
            .map_err(|error| error.to_string())?;
    }
    plan.send(root, &task).map(Some)
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
            ("saturated", vec![peer("best", 0, 4.0, 0)], false, "", false),
            ("healthy", vec![peer("best", 3, 4.0, 0)], false, "", false),
            ("off", vec![peer("best", 3, 4.0, 0)], true, "", false),
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
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"agents": peers})),
                )
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
    async fn cas_d1fe_spawn_and_stale_idle_assignment_use_the_peer_route() {
        use crate::mcp::tools::{TaskCreateRequest, TaskUpdateRequest};
        use cas_mcp::FactoryRequest;
        use crate::mcp::{CasCore, CasService};
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
        let occupied = std::fs::OpenOptions::new().read(true).write(true).create(true)
            .truncate(false).open(slots.join("slot-0.lock")).unwrap();
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
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"agents": [peer("best", 3, 4.0, 0)]})),
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
                text.contains("routed to best@best: 3 free slots, load/CPU 0.50"),
                "{action}: {text}"
            );
            let routed = tasks
                .list(None)
                .unwrap()
                .into_iter()
                .find(|task| task.title == format!("Overflow {action}"))
                .unwrap();
            assert!(routed.assignee.is_none(), "{action}");
            assert!(routed.notes.contains("build-route:"));
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
        assert_eq!(sends, 3);
    }
}
