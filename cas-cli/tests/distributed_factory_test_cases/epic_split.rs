//! cas-4736: two supervisors of one repository split an epic between them,
//! across two "machines" (two CAS databases) and one fake Cassy Cloud, through
//! the real MCP surface and the factory daemon's peer-mailbox tick.
//!
//! The soundwave supervisor proposes taking children A–C of an epic and
//! offers D–F; the prowl supervisor accepts. Each is then refused when
//! starting, claiming or spawning for the other's children, and neither ever
//! claims for the other. An unaccepted proposal expires, a proposal whose own
//! share cannot all be claimed leaves no claim behind, and either side can
//! cancel without touching the other side's claims.

use cas::cloud::peer_mailbox::PeerMailboxRuntime;
use cas::cloud::{CloudCoordinator, resolve_canonical_id};
use cas::mcp::tools::TaskStartRequest;
use cas::store::{open_agent_store, open_task_store};
use cas::types::{Agent, AgentRole, Dependency, DependencyType, Task, TaskStatus, TaskType};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{Value, json};

use super::TestMachine;
use super::fake_cloud::FakeCloud;

const WIDGETS: &str = "github.com/acme/widgets";
const EPIC: &str = "cas-e100";
const SOUNDWAVE_TAKES: [&str; 3] = ["cas-a1", "cas-a2", "cas-a3"];
const PROWL_TAKES: [&str; 3] = ["cas-d1", "cas-d2", "cas-d3"];

/// One machine: its database, its supervisor's id and MCP surface, and the
/// MCP core of one of its workers (supervisors delegate starts to workers).
struct Side {
    machine: TestMachine,
    id: &'static str,
    service: cas::mcp::CasService,
    worker: cas::mcp::CasCore,
}

/// Register `agent` in the cloud (as its daemon does) and locally.
fn register(machine: &TestMachine, agent: &Agent) {
    let config = cas::cloud::CloudConfig::load_from_cas_dir(&machine.cas_dir).unwrap();
    CloudCoordinator::new(config)
        .unwrap()
        .with_canonical_id(resolve_canonical_id(&machine.cas_dir))
        .with_machine_label(Some(cas::cloud::peers::machine_label_for(&machine.cas_dir)))
        .register_with_focus(agent, Some(EPIC))
        .unwrap();
    open_agent_store(&machine.cas_dir).unwrap().register(agent).unwrap();
}

fn side(cloud: &FakeCloud, machine_name: &str, id: &'static str, name: &str) -> Side {
    side_account(cloud, machine_name, id, name, "test-token")
}

fn side_account(
    cloud: &FakeCloud,
    machine_name: &str,
    id: &'static str,
    name: &str,
    token: &str,
) -> Side {
    let machine = TestMachine::new(machine_name);
    cloud.seed(&machine.cas_dir, WIDGETS);
    let mut config = cloud.config();
    config.token = Some(token.into());
    config.save_to_cas_dir(&machine.cas_dir).unwrap();
    std::fs::write(
        machine.cas_dir.join("config.toml"),
        format!(
            "[project]\ncanonical_id = {WIDGETS:?}\n[verification]\nenabled = false\n\
             [cloud]\nmachine_label = {:?}\n",
            format!("{}-host", machine_name.trim_start_matches("machine-"))
        ),
    )
    .unwrap();
    // A supervisor and one of its workers, in one factory session.
    for (agent_id, agent_name, role) in [
        (id.to_string(), name.to_string(), AgentRole::Supervisor),
        (format!("{id}-w"), format!("{name}-worker"), AgentRole::Worker),
    ] {
        let mut agent = Agent::new(agent_id, agent_name);
        agent.role = role;
        agent.machine_id = Some(machine_name.to_string());
        agent.factory_session = Some(format!("factory-{id}"));
        register(&machine, &agent);
    }
    // The same epic and children, as every clone sees them after a pull.
    let tasks = open_task_store(&machine.cas_dir).unwrap();
    let mut epic = Task::new(EPIC.to_string(), "shared epic".to_string());
    epic.task_type = TaskType::Epic;
    tasks.add(&epic).unwrap();
    for child in SOUNDWAVE_TAKES.iter().chain(PROWL_TAKES.iter()) {
        let mut task = Task::new(child.to_string(), format!("child {child}"));
        task.status = TaskStatus::Open;
        tasks.add(&task).unwrap();
        tasks
            .add_dependency(&Dependency::new(
                child.to_string(),
                EPIC.to_string(),
                DependencyType::ParentChild,
            ))
            .unwrap();
    }
    let worker = cas::mcp::CasCore::with_daemon(machine.cas_dir.clone(), None, None);
    worker.set_agent_id_for_testing(format!("{id}-w"));
    let service_core = cas::mcp::CasCore::with_daemon(machine.cas_dir.clone(), None, None);
    service_core.set_agent_id_for_testing(id.to_string());
    Side {
        machine,
        id,
        service: cas::mcp::CasService::new(service_core, None),
        worker,
    }
}

fn text(result: rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match &content.raw {
            rmcp::model::RawContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Side {
    async fn task(&self, request: Value) -> Result<String, String> {
        self.service
            .task(Parameters(serde_json::from_value(request).unwrap()))
            .await
            .map(text)
            .map_err(|error| error.message.to_string())
    }

    async fn factory(&self, request: Value) -> Result<String, String> {
        let mut request = request;
        request["action"] = json!(request.get("action").cloned().unwrap_or(json!("epic_split")));
        let request: cas_mcp::types::CoordinationRequest = serde_json::from_value(request).unwrap();
        self.service
            .factory(Parameters(request))
            .await
            .map(text)
            .map_err(|error| error.message.to_string())
    }

    async fn coordination(&self, request: Value) -> Result<String, String> {
        let request: cas_mcp::types::CoordinationRequest = serde_json::from_value(request).unwrap();
        self.service
            .coordination(Parameters(request))
            .await
            .map(text)
            .map_err(|error| error.message.to_string())
    }

    /// A start by this side's worker.
    async fn start(&self, id: &str, force: bool) -> Result<String, String> {
        self.worker
            .cas_task_start_with_options(Parameters(TaskStartRequest {
                id: id.to_string(),
                brief: Some(true),
                force: force.then_some(true),
            }))
            .await
            .map(text)
            .map_err(|error| error.message.to_string())
    }

    /// The supervisor assigns `id` to its worker, then the worker claims it.
    async fn claim(&self, id: &str) -> Result<String, String> {
        let tasks = open_task_store(&self.machine.cas_dir).unwrap();
        let mut task = tasks.get(id).unwrap();
        task.assignee = Some(format!("{}-w", self.id));
        tasks.update(&task).unwrap();
        let request: cas::mcp::tools::TaskClaimRequest =
            serde_json::from_value(json!({"task_id": id})).unwrap();
        self.worker
            .cas_task_claim(Parameters(request))
            .await
            .map(text)
            .map_err(|error| error.message.to_string())
    }

    /// One tick of this machine's factory-daemon peer mailbox puller.
    async fn tick(&self) -> cas::cloud::peer_mailbox::DeliveryReport {
        let (cas_dir, session) = (self.machine.cas_dir.clone(), format!("factory-{}", self.id));
        tokio::task::spawn_blocking(move || {
            PeerMailboxRuntime::start(&cas_dir, &session)
                .expect("a logged-in project runs the mailbox")
                .tick_blocking()
        })
        .await
        .unwrap()
    }
}

/// The agent holding the active cloud claim on `task_id`, by claim key.
fn cloud_holder(cloud: &FakeCloud, task_id: &str) -> Option<String> {
    let key = cas::cloud::peer_claims::claim_key(WIDGETS, task_id);
    cloud
        .state
        .lock()
        .unwrap()
        .locks
        .get(&key)
        .filter(|lock| lock["status"] == "active")
        .and_then(|lock| lock["agent_id"].as_str().map(str::to_string))
}

fn split_id(proposed: &str) -> String {
    let start = proposed.find("split-").expect("the receipt names the split");
    proposed[start..start + "split-".len() + 8].to_string()
}

fn csv(ids: &[&str]) -> String {
    ids.join(",")
}

fn assert_split_decision(side: &Side, id: &str, decision: &str) {
    let rows = cas::store::open_prompt_queue_store(&side.machine.cas_dir)
        .unwrap().peek_all(100).unwrap();
    let row = rows.iter().find(|row| {
        // pin: select the split protocol's public decision kind, not its human wording.
        row.prompt.contains(id) && row.prompt.contains(&format!("kind=\"{decision}\""))
    }).unwrap_or_else(|| panic!("{id} {decision} must reach the proposer: {rows:?}"));
    assert_eq!(row.origin, Some(cas_store::QueueOrigin::Daemon));
    // pin: split decision envelopes are typed independently of reply/FYI intent.
    assert!(row.prompt.contains("kind=\"epic_split_decided\""), "{}", row.prompt);
    assert!(cas::cloud::peer_mailbox::parse_envelope(&row.prompt).unwrap().wakes_idle());
}

/// Soundwave proposes A–C for itself and D–F for prowl; prowl's daemon
/// receives it and prowl accepts. Returns the split id.
async fn propose_and_accept(cloud: &FakeCloud, soundwave: &Side, prowl: &Side) -> String {
    let proposed = soundwave
        .factory(json!({"id": EPIC, "target": "prowl", "mine": csv(&SOUNDWAVE_TAKES), "theirs": csv(&PROWL_TAKES)}))
        .await
        .unwrap();
    assert!(proposed.contains("Proposed epic split"), "{proposed}");
    let id = split_id(&proposed);
    for child in SOUNDWAVE_TAKES {
        assert_eq!(cloud_holder(cloud, child).as_deref(), Some("sw"), "{child}");
    }
    for child in PROWL_TAKES {
        assert_eq!(cloud_holder(cloud, child), None, "the proposer never claims {child} for prowl");
    }

    let report = prowl.tick().await;
    assert_eq!(report.delivered, 1, "{report:?}");
    let listed = prowl.factory(json!({})).await.unwrap();
    assert!(listed.contains(&id) && listed.contains("[pending, incoming]"), "{listed}");
    for child in PROWL_TAKES {
        assert_eq!(cloud_holder(cloud, child), None, "receiving a proposal claims nothing");
    }

    // A heartbeat or a same-parent task update between receive and accept
    // must leave the pending shares available for the subsequent acceptance.
    let claims = cas::cloud::peer_claims::PeerClaims::for_project(&prowl.machine.cas_dir).unwrap();
    assert_eq!(cas::cloud::epic_split::renew_split_claims(&prowl.machine.cas_dir, &claims, prowl.id, 600), 0);
    assert!(cas_store::list_epic_split_child_releases(&prowl.machine.cas_dir).unwrap().is_empty(),
        "an incoming proposal is not an obsolete split");
    prowl.task(json!({"action": "update", "id": "cas-d1", "epic": EPIC})).await.unwrap();
    assert!(cas_store::list_epic_split_child_releases(&prowl.machine.cas_dir).unwrap().is_empty(),
        "keeping the same parent must preserve both pending shares");

    let accepted = prowl
        .factory(json!({"id": id, "decision": "accept"}))
        .await
        .unwrap();
    assert!(accepted.contains("Accepted epic split"), "{accepted}");
    for child in PROWL_TAKES {
        assert_eq!(cloud_holder(cloud, child).as_deref(), Some("pr"), "{child}");
    }
    assert_eq!(cas::cloud::epic_split::renew_split_claims(&prowl.machine.cas_dir, &claims, prowl.id, 600), 3,
        "every accepted child must remain renewable after the pending heartbeat");
    for child in SOUNDWAVE_TAKES {
        assert_eq!(
            cloud_holder(cloud, child).as_deref(),
            Some("sw"),
            "accepting never claims or moves the proposer's {child}"
        );
    }
    let report = soundwave.tick().await;
    assert_eq!(report.delivered, 1, "{report:?}");
    let listed = soundwave.factory(json!({})).await.unwrap();
    assert!(listed.contains(&id) && listed.contains("[accepted, outgoing]"), "{listed}");
    id
}

#[tokio::test(flavor = "multi_thread")]
async fn shipped_epic_splits_stop_binding_without_cancellation_cas_43d8() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let mut failures = Vec::new();
    for (old_status, reparented, released, repair_error, cleanup_pending) in [
        (TaskStatus::Closed, false, true, false, false),
        (TaskStatus::Cancelled, false, true, false, false),
        (TaskStatus::InProgress, true, true, false, false),
        (TaskStatus::InProgress, false, false, false, false),
        (TaskStatus::InProgress, false, false, true, false),
        (TaskStatus::Closed, true, true, false, true),
    ] {
        let cloud = FakeCloud::start();
        let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
        let prowl = side(&cloud, "machine-prowl", "pr", "prowl");
        let root = &soundwave.machine.cas_dir;
        let tasks = open_task_store(root).unwrap();
        let mut old_epic = Task::new("cas-8a2e".into(), "shipped v47".into());
        old_epic.task_type = TaskType::Epic;
        old_epic.status = old_status;
        tasks.add(&old_epic).unwrap();
        let child = "cas-a1";
        if !reparented {
            for id in std::iter::once(child).chain(repair_error.then_some("cas-a2")) {
                tasks.remove_dependency_of_type(id, EPIC, DependencyType::ParentChild).unwrap();
                tasks.add_dependency(&Dependency::new(
                    id.into(), old_epic.id.clone(), DependencyType::ParentChild,
                )).unwrap();
            }
        }
        let now = chrono::Utc::now();
        let historical = cas_store::EpicSplit {
            id: "split-687eb1ce".into(),
            epic_id: old_epic.id.clone(),
            local_agent_id: if repair_error || cleanup_pending { "sw" } else { "gone-owner" }.into(),
            peer_agent_id: "gone-peer".into(),
            peer_name: "patient-gopher-67".into(),
            peer_machine: "prowl".into(),
            direction: cas_store::SplitDirection::Outgoing,
            mine: if cleanup_pending { vec![child.into()] } else if repair_error { vec!["cas-a2".into()] } else { vec![] },
            theirs: if cleanup_pending { vec![] } else { vec![child.into()] },
            state: if cleanup_pending { cas_store::SplitState::Cancelled } else { cas_store::SplitState::Accepted },
            reason: None,
            message_id: None,
            expires_at: now - chrono::Duration::days(4),
            created_at: now - chrono::Duration::days(4),
            decided_at: Some(now - chrono::Duration::days(4)),
            decided_by: None,
            cleanup_pending,
            expiry_notice_pending: false,
        };
        cas_store::insert_epic_split(root, &historical).unwrap();
        let claims = cas::cloud::peer_claims::PeerClaims::for_project(root).unwrap();
        for id in &historical.mine {
            assert_eq!(claims.acquire(id, cas::cloud::peer_claims::ClaimKind::Task,
                "sw", "soundwave", 600, &|_| false), cas::cloud::peer_claims::Acquire::Claimed);
        }
        if repair_error {
            // A real outbox failure leaves a committed task and failed repair.
            // Keep the trigger installed so this never depends on the 5s cache window.
            let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
            conn.execute_batch("CREATE TRIGGER fail_split_repair BEFORE INSERT ON sync_queue
                WHEN NEW.entity_type = 'task' BEGIN
                SELECT RAISE(FAIL, 'injected split repair failure'); END;").unwrap();
            let mut task = tasks.get(child).unwrap();
            task.title.push_str(" with pending sync");
            assert!(tasks.update(&task).unwrap_err().to_string().contains("injected split repair failure"));
            assert!(tasks.init().unwrap_err().to_string().contains("injected split repair failure"));
            assert!(open_task_store(root).is_err(), "the normal task-store open must replay the repair failure");
        }
        let released_now = cas::cloud::epic_split::split_hold_for(root, child).is_none();
        let renewed = repair_error.then(|| cas::cloud::epic_split::renew_split_claims(root, &claims, "sw", 600));
        if repair_error {
            rusqlite::Connection::open(root.join("cas.db")).unwrap()
                .execute_batch("DROP TRIGGER fail_split_repair;").unwrap();
            tasks.init().unwrap();
        }
        if (released_now, renewed) != (released, repair_error.then_some(1)) {
            failures.push(format!("binding/renewal: {old_status:?}, reparented={reparented}, repair_error={repair_error}: released={released_now}, renewed={renewed:?}"));
        }
        if released && !reparented {
            tasks
                .remove_dependency_of_type(child, &old_epic.id, DependencyType::ParentChild)
                .unwrap();
            tasks
                .add_dependency(&Dependency::new(
                    child.into(),
                    EPIC.into(),
                    DependencyType::ParentChild,
                ))
                .unwrap();
        }
        let outcome = soundwave
            .factory(json!({"id": if released { EPIC } else { &old_epic.id },
            "target": "prowl", "mine": child}))
            .await;
        if released && !cleanup_pending {
            let proposed = outcome.unwrap();
            assert!(proposed.contains("Proposed epic split"), "{proposed}");
            let id = split_id(&proposed);
            assert_eq!(prowl.tick().await.delivered, 1);
            let accepted = prowl
                .factory(json!({"id": id, "decision": "accept"}))
                .await
                .unwrap();
            assert!(accepted.contains("Accepted epic split"), "{accepted}");
        } else if let Err(refusal) = outcome {
            if !(
                refusal.contains(&historical.id)
                    && refusal.contains(&old_epic.id)
                    && refusal.contains(if cleanup_pending { "cancelled" } else { "accepted" })
                    && refusal.contains(&old_status.to_string())
                    && refusal
                        .contains("factory action=epic_split id=split-687eb1ce decision=cancel")
            ) {
                failures.push(format!("overlap refusal must identify its historical reservation: {refusal}"));
            }
        } else {
            failures.push(format!("overlap unexpectedly allowed: cleanup_pending={cleanup_pending}, {outcome:?}"));
        }
        if cleanup_pending {
            assert_eq!(cloud_holder(&cloud, child).as_deref(), Some("sw"), "cleanup still owns the old claim");
        }
        assert_eq!(
            cas_store::get_epic_split(root, &historical.id).unwrap(),
            Some(historical),
            "the fix must not cancel or rewrite the historical split"
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The demo: propose, accept, and each side refused on the other's children.
#[tokio::test(flavor = "multi_thread")]
async fn epic_split_assignment_and_start_require_reasoned_supervisor_override_cas_0795() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let prowl = side(&cloud, "machine-prowl", "pr", "prowl");

    // A worker cannot split an epic.
    let worker_core = cas::mcp::CasCore::with_daemon(soundwave.machine.cas_dir.clone(), None, None);
    worker_core.set_agent_id_for_testing("sw-w".to_string());
    let request: cas_mcp::types::CoordinationRequest = serde_json::from_value(
        json!({"action": "epic_split", "id": EPIC, "target": "prowl", "mine": "cas-a1", "theirs": "cas-d1"}),
    )
    .unwrap();
    let refused = cas::mcp::CasService::new(worker_core, None)
        .factory(Parameters(request))
        .await
        .unwrap_err();
    assert!(refused.message.contains("registered supervisor"), "{}", refused.message);

    let id = propose_and_accept(&cloud, &soundwave, &prowl).await;
    assert_split_decision(&soundwave, &id, "accept");

    let tasks = open_task_store(&soundwave.machine.cas_dir).unwrap();
    let before = tasks.get("cas-d3").unwrap();
    let refused = soundwave.task(json!({"action": "update", "id": "cas-d3",
        "assignee": "soundwave-worker", "title": "must not apply", "force": true}))
        .await.unwrap_err();
    assert!(refused.contains("SPLIT TO A PEER") && refused.contains("prowl@")
        && refused.contains(&id), "{refused}");
    let after = tasks.get("cas-d3").unwrap();
    assert_eq!(after.assignee, before.assignee);
    assert_eq!(after.title, before.title);

    let refused = soundwave.task(json!({"action": "update", "id": "cas-d3",
        "assignee": "soundwave-worker", "supervisor_override": true, "reason": "  "}))
        .await.unwrap_err();
    assert!(refused.contains("reason"), "{refused}");

    let worker_core = cas::mcp::CasCore::with_daemon(soundwave.machine.cas_dir.clone(), None, None);
    worker_core.set_agent_id_for_testing("sw-w".into());
    let worker_service = cas::mcp::CasService::new(worker_core, None);
    for action in ["update", "start"] {
        let refused = worker_service.task(Parameters(serde_json::from_value(json!({
            "action": action, "id": "cas-d3", "assignee": "soundwave-worker",
            "supervisor_override": true, "force": true, "reason": "worker cannot take over"
        })).unwrap())).await.unwrap_err();
        let expected = if action == "update" { "registered supervisor" } else { "SPLIT TO A PEER" };
        assert!(refused.message.contains(expected), "{refused}");
    }

    let reason = "peer agreed to hand this child back";
    let assigned = soundwave.task(json!({"action": "update", "id": "cas-d3",
        "assignee": "soundwave-worker", "supervisor_override": true, "reason": reason}))
        .await.unwrap();
    let task = tasks.get("cas-d3").unwrap();
    assert_eq!(task.assignee.as_deref(), Some("soundwave-worker"), "{assigned}");
    assert!(task.notes.contains(&id) && task.notes.contains(reason)
        && task.notes.contains("supervisor=sw"), "{}", task.notes);
    let mut other = Agent::new_with_role("sw-other".into(), "other-worker".into(), AgentRole::Worker);
    other.factory_session = Some("factory-sw".into());
    register(&soundwave.machine, &other);
    let other_core = cas::mcp::CasCore::with_daemon(soundwave.machine.cas_dir.clone(), None, None);
    other_core.set_agent_id_for_testing(other.id);
    let refused = other_core.cas_task_start_with_options(Parameters(TaskStartRequest {
        id: "cas-d3".into(), brief: Some(true), force: Some(true),
    })).await.unwrap_err();
    assert!(refused.message.contains("SPLIT TO A PEER") && refused.message.contains(&id), "{refused}");
    assert_eq!(tasks.get("cas-d3").unwrap().status, TaskStatus::Open);
    let refused = soundwave.task(json!({"action": "transfer", "id": "cas-d3",
        "to_agent": "other-worker", "supervisor_override": true, "reason": reason}))
        .await.unwrap_err();
    assert!(refused.contains("SPLIT TO A PEER"), "{refused}");

    // Each side is refused on children without a takeover: start, claim and spawn.
    for child in ["cas-d1", "cas-d2"] {
        let refused = soundwave.start(child, false).await.unwrap_err();
        assert!(refused.contains("SPLIT TO A PEER") && refused.contains(&id), "{refused}");
        assert!(refused.contains("prowl@"), "{refused}");
    }
    for child in SOUNDWAVE_TAKES {
        let refused = prowl.start(child, false).await.unwrap_err();
        assert!(refused.contains("SPLIT TO A PEER") && refused.contains("soundwave@"), "{refused}");
    }
    let refused = soundwave.claim("cas-d2").await.unwrap_err();
    assert!(refused.contains("SPLIT TO A PEER"), "{refused}");
    let refused = soundwave
        .factory(json!({"action": "spawn_workers", "count": 1, "task_id": "cas-d1", "force": true}))
        .await.unwrap_err();
    assert!(refused.contains("SPLIT TO A PEER"), "{refused}");
    assert!(cas::store::open_spawn_queue_store(&soundwave.machine.cas_dir)
        .unwrap().peek(10).unwrap().is_empty());
    let refused = prowl.claim("cas-a2").await.unwrap_err();
    assert!(refused.contains("SPLIT TO A PEER"), "{refused}");
    let refused = soundwave
        .factory(json!({"action": "spawn_workers", "count": 1, "task_id": "cas-d1", "isolate": true}))
        .await
        .unwrap_err();
    assert!(refused.contains("SPLIT TO A PEER"), "{refused}");

    // cas-0078 no-theft: every refused start, claim and spawn left the real
    // holders' cloud claims exactly where they were.
    for child in PROWL_TAKES {
        assert_eq!(cloud_holder(&cloud, child).as_deref(), Some("pr"), "{child}");
    }
    for child in SOUNDWAVE_TAKES {
        assert_eq!(cloud_holder(&cloud, child).as_deref(), Some("sw"), "{child}");
    }

    // Each starts its own share: the worker wins its local lease and only
    // then takes its own supervisor's split claim (take_from_local).
    soundwave.start("cas-a1", false).await.unwrap();
    prowl.start("cas-d1", false).await.unwrap();
    assert_eq!(cloud_holder(&cloud, "cas-a1").as_deref(), Some("sw-w"));
    assert_eq!(cloud_holder(&cloud, "cas-d1").as_deref(), Some("pr-w"));

    // Force alone cannot take a split child; only the bound worker consumes the decision.
    let refused = soundwave.start("cas-d2", true).await.unwrap_err();
    assert!(refused.contains("SPLIT TO A PEER") && refused.contains(&id), "{refused}");
    let forced = soundwave.start("cas-d3", false).await.unwrap();
    assert!(forced.contains("SPLIT OVERRIDE"), "{forced}");
    let task = tasks.get("cas-d3").unwrap();
    assert_eq!(task.status, TaskStatus::InProgress);
    assert!(task.notes.contains(reason) && task.notes.contains(&id), "{}", task.notes);
    assert!(serde_json::to_value(&task).unwrap()["deliverables"].get("split_takeover").is_none());
    assert_eq!(cloud_holder(&cloud, "cas-d3").as_deref(), Some("pr"));
    let refused = soundwave.start("cas-d3", true).await.unwrap_err();
    assert!(refused.contains("SPLIT TO A PEER"), "consumed grant cannot be replayed: {refused}");

    // The peers detail view shows the split on both sides.
    for supervisor in [&soundwave, &prowl] {
        let peers = supervisor
            .coordination(json!({"action": "peers"}))
            .await
            .unwrap();
        assert!(peers.contains("Epic splits:") && peers.contains(&id), "{peers}");
        assert!(peers.contains("accepted"), "{peers}");
    }

    // Retarget a peer-owned child through MCP, then really start it on the
    // new supervisor's worker. A cloud release alone cannot prove admission.
    let mut next = Task::new("cas-8503-next".into(), "Next epic".into());
    next.task_type = TaskType::Epic;
    open_task_store(&prowl.machine.cas_dir).unwrap().add(&next).unwrap();
    prowl.task(json!({"action": "update", "id": "cas-a3", "epic": next.id})).await.unwrap();
    assert_eq!(cloud_holder(&cloud, "cas-a3"), None, "retarget releases the remote split owner's claim");
    let started = prowl.start("cas-a3", false).await.unwrap();
    assert!(started.contains("Started task:"), "{started}");
    assert_eq!(cloud_holder(&cloud, "cas-a3").as_deref(), Some("pr-w"));
    assert_eq!(open_task_store(&prowl.machine.cas_dir).unwrap().get("cas-a3").unwrap().status, TaskStatus::InProgress);
}

/// Either side cancels: the canceller releases only its own claims and the
/// other side's daemon marks it cancelled without touching that side's.
#[tokio::test(flavor = "multi_thread")]
async fn epic_split_cancel_releases_only_the_cancellers_own_claims() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let prowl = side(&cloud, "machine-prowl", "pr", "prowl");
    let id = propose_and_accept(&cloud, &soundwave, &prowl).await;

    let cancelled = prowl
        .factory(json!({"id": id, "decision": "cancel", "reason": "operator reassigned prowl"}))
        .await
        .unwrap();
    assert!(cancelled.contains("released 3 of your own claim"), "{cancelled}");
    for child in PROWL_TAKES {
        assert_eq!(cloud_holder(&cloud, child), None, "{child}");
    }
    for child in SOUNDWAVE_TAKES {
        assert_eq!(cloud_holder(&cloud, child).as_deref(), Some("sw"), "{child} untouched");
    }
    // prowl's split gate is lifted at once; soundwave's own cloud claim
    // still refuses prowl on cas-a2.
    let held = prowl.start("cas-a2", false).await.unwrap_err();
    assert!(held.contains("HELD BY A PEER") && !held.contains("SPLIT"), "{held}");
    let report = soundwave.tick().await;
    assert_eq!(report.delivered, 1, "{report:?}");
    assert_split_decision(&soundwave, &id, "cancel");
    let listed = soundwave.factory(json!({})).await.unwrap();
    assert!(listed.contains("[cancelled, outgoing]"), "{listed}");
    assert!(listed.contains("operator reassigned prowl"), "{listed}");
    for child in SOUNDWAVE_TAKES {
        assert_eq!(
            cloud_holder(&cloud, child).as_deref(),
            Some("sw"),
            "receiving a cancel never releases {child}"
        );
    }
    // The split no longer refuses soundwave on prowl's former share.
    soundwave.start("cas-d1", false).await.unwrap();
    // A cancelled split cannot be cancelled again.
    let again = soundwave
        .factory(json!({"id": id, "decision": "cancel"}))
        .await
        .unwrap_err();
    assert!(again.contains("already cancelled"), "{again}");
}

/// One task changes hands without an unrelated filler share. Exercise both
/// directions through MCP, the mailbox and the real start guard.
async fn one_sided_handover(request: bool) {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let prowl = side(&cloud, "machine-prowl", "pr", "prowl");
    let (proposer, recipient, shares) = if request {
        // Prowl requests one child from soundwave, omitting theirs.
        (&prowl, &soundwave, json!({"mine": "cas-d1"}))
    } else {
        // Soundwave grants one child to prowl with an explicit empty mine.
        (&soundwave, &prowl, json!({"mine": "", "theirs": "cas-d1"}))
    };
    let mut proposal = shares;
    proposal["id"] = json!(EPIC);
    proposal["target"] = json!(if request { "soundwave" } else { "prowl" });
    let proposed = proposer.factory(proposal).await.unwrap();
    let id = split_id(&proposed);
    assert_eq!(cloud_holder(&cloud, "cas-d1").as_deref(), request.then_some("pr"));
    assert_eq!(recipient.tick().await.delivered, 1);

    let accepted = recipient.factory(json!({"id": id, "decision": "accept"})).await.unwrap();
    assert!(accepted.contains("Accepted epic split"), "{accepted}");
    assert_eq!(proposer.tick().await.delivered, 1);
    for child in SOUNDWAVE_TAKES.iter().chain(PROWL_TAKES.iter()) {
        assert_eq!(
            cloud_holder(&cloud, child).as_deref(),
            (*child == "cas-d1").then_some("pr"),
            "only the handed-over task is claimed: {child}"
        );
    }
    let refused = soundwave.start("cas-d1", false).await.unwrap_err();
    assert!(refused.contains("SPLIT TO A PEER") && refused.contains(&id), "{refused}");

    let cancelled = prowl.factory(json!({"id": id, "decision": "cancel"})).await.unwrap();
    assert!(cancelled.contains("released 1 of your own claim"), "{cancelled}");
    assert_eq!(cloud_holder(&cloud, "cas-d1"), None);
    assert_eq!(soundwave.tick().await.delivered, 1);
    soundwave.start("cas-d1", false).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn epic_split_one_sided_request() {
    one_sided_handover(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn epic_split_one_sided_grant() {
    one_sided_handover(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn epic_split_reject_notifies_and_releases_requester_cas_ae81() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let prowl = side(&cloud, "machine-prowl", "pr", "prowl");
    let proposed = prowl.factory(json!({"id": EPIC, "target": "soundwave", "mine": "cas-d1"}))
        .await.unwrap();
    let id = split_id(&proposed);
    assert_eq!(soundwave.tick().await.delivered, 1);
    soundwave.factory(json!({"id": id, "decision": "reject", "reason": "host not ready"}))
        .await.unwrap();
    assert_eq!(prowl.tick().await.delivered, 1);
    assert_eq!(cloud_holder(&cloud, "cas-d1"), None, "rejection releases the requester's own claim");
    assert_split_decision(&prowl, &id, "reject");
    // Crash after release but before its settlement write: prevent a new
    // supervisor-owned split claim until that older cleanup is settled.
    rusqlite::Connection::open(prowl.machine.cas_dir.join("cas.db")).unwrap()
        .execute("UPDATE epic_splits SET cleanup_pending=1 WHERE id=?1", [&id]).unwrap();
    let held = prowl.factory(json!({"id": EPIC, "target": "soundwave", "mine": "cas-d1"}))
        .await.unwrap_err();
    assert!(held.contains(&id), "{held}");
    assert!(prowl.tick().await.errors.is_empty());
    soundwave.start("cas-d1", false).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn epic_split_status_reports_decider_and_claims_cas_ae81() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let prowl = side(&cloud, "machine-prowl", "pr", "prowl");
    let id = propose_and_accept(&cloud, &soundwave, &prowl).await;
    let status = soundwave.factory(json!({"id": id, "decision": "status"})).await.unwrap();
    for expected in [&id, "accepted", "decided_at", "decider", "pr", "cas-a1", "sw", "cas-d1", "claims"] {
        assert!(status.contains(expected), "status must show {expected}: {status}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn epic_split_decided_reminder_is_supported_cas_ae81() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let result = soundwave.coordination(json!({"action": "remind", "remind_event": "epic_split_decided",
        "remind_filter": "{\"split_id\":\"split-12345678\"}", "remind_message": "continue after decision"}))
        .await.unwrap();
    assert!(result.contains("Reminder"), "{result}");
    let reminders = cas::store::open_reminder_store(&soundwave.machine.cas_dir).unwrap()
        .get_event_reminders("epic_split_decided").unwrap();
    assert_eq!(reminders.len(), 1);
    assert_eq!(reminders[0].trigger_event.as_deref(), Some("epic_split_decided"));
    assert_eq!(reminders[0].trigger_filter, Some(json!({"split_id": "split-12345678"})));
}

fn expire(side: &Side, id: &str, seconds_ago: i64) {
    rusqlite::Connection::open(side.machine.cas_dir.join("cas.db")).unwrap()
        .execute("UPDATE epic_splits SET expires_at=?1 WHERE id=?2",
            rusqlite::params![(chrono::Utc::now() - chrono::Duration::seconds(seconds_ago)).to_rfc3339(), id])
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn epic_split_expired_orphan_recovery_preserves_live_gap_cas_44c8() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let mut soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let _prowl = side(&cloud, "machine-prowl", "pr", "prowl");
    let id = split_id(&soundwave.factory(json!({"id": EPIC, "target": "prowl", "mine": "cas-a1"}))
        .await.unwrap());
    expire(&soundwave, &id, 1);
    let refused = soundwave.factory(json!({"id": EPIC, "target": "prowl", "mine": "cas-a1"}))
        .await.unwrap_err();
    assert!(refused.contains(&id), "live-owner expiry gap still reserves the claim: {refused}");

    let mut successor = Agent::new("sw-next".into(), "replacement-soundwave-cas-44c8".into());
    successor.role = AgentRole::Supervisor;
    successor.factory_session = Some("factory-sw-next".into());
    successor.machine_id = Some("machine-soundwave".into());
    register(&soundwave.machine, &successor);
    let core = cas::mcp::CasCore::with_daemon(soundwave.machine.cas_dir.clone(), None, None);
    core.set_agent_id_for_testing(successor.id.clone());
    soundwave.service = cas::mcp::CasService::new(core, None);
    soundwave.id = "sw-next";
    assert!(soundwave.factory(json!({"id": id, "decision": "cancel"})).await.is_err(),
        "a successor cannot supersede a live owner's expiry gap");
    let agents = open_agent_store(&soundwave.machine.cas_dir).unwrap();
    let mut owner = agents.get("sw").unwrap();
    owner.status = cas::types::AgentStatus::Shutdown;
    owner.last_heartbeat = chrono::Utc::now() - chrono::Duration::minutes(2);
    agents.update(&owner).unwrap();

    let recovered = soundwave.factory(json!({"id": id, "decision": "cancel", "reason": "recover ended session"}))
        .await.unwrap();
    assert_eq!(cloud_holder(&cloud, "cas-a1"), None, "supersession releases the ended owner's claim: {recovered}");
    let old = cas_store::get_epic_split(&soundwave.machine.cas_dir, &id).unwrap().unwrap();
    assert_eq!(old.state, cas_store::SplitState::Cancelled);
    assert_eq!(old.decided_by.as_deref(), Some("sw-next"));
    let audit = old.reason.unwrap();
    assert!(audit.contains("sw-next") && audit.contains("sw") && audit.contains("recover ended session"), "{audit}");
    soundwave.factory(json!({"id": EPIC, "target": "prowl", "mine": "cas-a1"})).await.unwrap();
    cas::cloud::epic_split::reconcile(&soundwave.machine.cas_dir, "factory-sw", chrono::Utc::now()).unwrap();
    assert_eq!(cloud_holder(&cloud, "cas-a1").as_deref(), Some("sw-next"),
        "the orphan cannot later release its replacement's claim");
}

#[tokio::test(flavor = "multi_thread")]
async fn epic_split_reconcile_filters_owner_before_limit_cas_537c() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let _prowl = side(&cloud, "machine-prowl", "pr", "prowl");
    let id = split_id(&soundwave.factory(json!({"id": EPIC, "target": "prowl", "mine": "cas-a1"}))
        .await.unwrap());
    expire(&soundwave, &id, 1);
    let mut dead = Agent::new("dead-owner".into(), "dead-owner".into());
    dead.role = AgentRole::Supervisor;
    dead.factory_session = Some("ended-session".into());
    dead.status = cas::types::AgentStatus::Shutdown;
    open_agent_store(&soundwave.machine.cas_dir).unwrap().register(&dead).unwrap();
    let original = cas_store::get_epic_split(&soundwave.machine.cas_dir, &id).unwrap().unwrap();
    // Fill the entire work budget with earlier rows this session cannot own.
    for index in 0..64 {
        let mut orphan = original.clone();
        orphan.id = format!("split-orphan-{index:02}");
        orphan.local_agent_id = if index % 2 == 0 { "missing-owner" } else { "dead-owner" }.into();
        orphan.expires_at -= chrono::Duration::minutes(1);
        cas_store::insert_epic_split(&soundwave.machine.cas_dir, &orphan).unwrap();
    }
    let report = soundwave.tick().await;
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(cloud_holder(&cloud, "cas-a1"), None, "other sessions must not exhaust this session's work budget");
}

#[tokio::test(flavor = "multi_thread")]
async fn epic_split_reconcile_continues_after_failure_cas_537c() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let _prowl = side(&cloud, "machine-prowl", "pr", "prowl");
    let first = split_id(&soundwave.factory(json!({"id": EPIC, "target": "prowl", "mine": "cas-a1"}))
        .await.unwrap());
    let second = split_id(&soundwave.factory(json!({"id": EPIC, "target": "prowl", "mine": "cas-a2"}))
        .await.unwrap());
    expire(&soundwave, &first, 2);
    expire(&soundwave, &second, 1);
    cloud.inject_failure("POST", "/release", 403, 1);
    let report = soundwave.tick().await;
    assert!(!report.errors.is_empty(), "the first cleanup must fail: {report:?}");
    assert_eq!(cloud_holder(&cloud, "cas-a1").as_deref(), Some("sw"));
    assert_eq!(cloud_holder(&cloud, "cas-a2"), None, "one failed split must not stop later work");
    assert!(soundwave.tick().await.errors.is_empty());
    assert_eq!(cloud_holder(&cloud, "cas-a1"), None, "failed work remains retryable");
}

#[tokio::test(flavor = "multi_thread")]
async fn epic_split_independent_expiry_notifies_each_side_once_cas_537c() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let prowl = side(&cloud, "machine-prowl", "pr", "prowl");
    let id = split_id(&soundwave.factory(json!({"id": EPIC, "target": "prowl", "mine": "cas-a1"}))
        .await.unwrap());
    assert_eq!(prowl.tick().await.delivered, 1);
    for side in [&soundwave, &prowl] {
        expire(side, &id, 1);
        // Reconcile both clocks before either daemon pulls the other's notice.
        cas::cloud::epic_split::reconcile(&side.machine.cas_dir, &format!("factory-{}", side.id), chrono::Utc::now())
            .unwrap();
    }
    for side in [&soundwave, &prowl] {
        let report = side.tick().await;
        assert!(report.errors.is_empty(), "{report:?}");
        let rows = cas::store::open_prompt_queue_store(&side.machine.cas_dir).unwrap().peek_all(100).unwrap();
        // pin: count the protocol decision, independent of human expiry wording.
        let notices = rows.iter().filter(|row| row.prompt.contains(&id) && row.prompt.contains("kind=\"expired\""));
        assert_eq!(notices.count(), 1, "one logical expiry notice for {}: {rows:?}", side.id);
    }
}

/// An unaccepted proposal expires; a proposal whose own share cannot all be
/// claimed is refused and leaves no claim or proposal behind.
#[tokio::test(flavor = "multi_thread")]
async fn epic_split_expires_unaccepted_and_claims_all_or_nothing() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let prowl = side(&cloud, "machine-prowl", "pr", "prowl");

    let empty = soundwave
        .factory(json!({"id": EPIC, "target": "prowl", "mine": "", "theirs": ""}))
        .await
        .unwrap_err();
    assert!(empty.contains("mine") && empty.contains("theirs"), "{empty}");
    assert!(cloud.messages_to("pr").is_empty());

    // All or nothing: prowl already works cas-a2, so soundwave's proposal to
    // take A–C fails, and cas-a1 (claimable) is not left claimed.
    prowl.start("cas-a2", false).await.unwrap();
    let refused = soundwave
        .factory(json!({"id": EPIC, "target": "prowl", "mine": csv(&SOUNDWAVE_TAKES), "theirs": csv(&PROWL_TAKES)}))
        .await
        .unwrap_err();
    assert!(refused.contains("nothing claimed") && refused.contains("cas-a2"), "{refused}");
    assert_eq!(cloud_holder(&cloud, "cas-a1"), None);
    assert_eq!(cloud_holder(&cloud, "cas-a2").as_deref(), Some("pr-w"));
    assert_eq!(cloud_holder(&cloud, "cas-a3"), None);
    assert!(cloud.messages_to("pr").is_empty(), "nothing was proposed");
    assert!(soundwave.factory(json!({})).await.unwrap().contains("No epic splits"));

    // Expiry: move both stored deadlines into the past, without a timed sleep.
    let proposed = soundwave
        .factory(json!({"id": EPIC, "target": "prowl", "mine": "cas-a1", "theirs": "cas-a3"}))
        .await
        .unwrap();
    let id = split_id(&proposed);
    assert_eq!(prowl.tick().await.delivered, 1);
    // While open, the proposer's own gate holds.
    let refused = soundwave.start("cas-a3", false).await.unwrap_err();
    assert!(refused.contains("SPLIT TO A PEER") && refused.contains("pending"), "{refused}");
    for supervisor in [&soundwave, &prowl] {
        rusqlite::Connection::open(supervisor.machine.cas_dir.join("cas.db")).unwrap()
            .execute("UPDATE epic_splits SET expires_at=?1 WHERE id=?2",
                rusqlite::params![(chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(), id])
            .unwrap();
    }

    let gap = soundwave.factory(json!({"id": EPIC, "target": "prowl", "mine": "cas-a1"})).await;
    assert!(gap.as_ref().is_err_and(|error| error.contains(&id)),
        "unprocessed expiry must reserve its old claim before a new proposal: {gap:?}");

    let expiry = soundwave.tick().await;
    assert!(expiry.errors.is_empty(), "{expiry:?}");
    assert_eq!(cloud_holder(&cloud, "cas-a1"), None, "expiry releases the proposer's own claim");
    assert_split_decision(&soundwave, &id, "expired");
    assert_eq!(prowl.tick().await.delivered, 1);
    assert_split_decision(&prowl, &id, "expired");

    // Simulate restart after the outbox send but before settling its work flag.
    let sent = cloud.messages_to("pr").len();
    rusqlite::Connection::open(soundwave.machine.cas_dir.join("cas.db")).unwrap()
        .execute("UPDATE epic_splits SET expiry_notice_pending=1 WHERE id=?1", [&id]).unwrap();
    let retry = soundwave.tick().await;
    assert!(retry.errors.is_empty(), "{retry:?}");
    assert_eq!(cloud.messages_to("pr").len(), sent, "expiry reuses its original logical send");

    let late = prowl
        .factory(json!({"id": id, "decision": "accept"}))
        .await
        .unwrap_err();
    assert!(late.contains("expired"), "{late}");
    assert_eq!(cloud_holder(&cloud, "cas-a3"), None, "an expired proposal claims nothing");
    for supervisor in [&soundwave, &prowl] {
        let listed = supervisor.factory(json!({})).await.unwrap();
        assert!(listed.contains(&id) && listed.contains("[expired,"), "{listed}");
    }
    // The gate lifts with the proposal.
    soundwave.start("cas-a3", false).await.unwrap();
    soundwave.factory(json!({"id": EPIC, "target": "prowl", "mine": "cas-a1"})).await.unwrap();
    assert!(soundwave.tick().await.errors.is_empty());
    assert_eq!(cloud_holder(&cloud, "cas-a1").as_deref(), Some("sw"), "the new split keeps its claim after old expiry cleanup");
}

/// cas-0078 no-theft for split claims: a split never takes a cloud claim from
/// the agent that holds it. A child one's own worker already works stays
/// that worker's when the supervisor proposes it, a refused start on the
/// other side leaves it alone, and cancelling releases only the supervisor's
/// own split claims, never its worker's.
#[tokio::test(flavor = "multi_thread")]
async fn epic_split_never_takes_a_claim_from_its_holder() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let prowl = side(&cloud, "machine-prowl", "pr", "prowl");

    // soundwave's worker already works cas-a1.
    soundwave.start("cas-a1", false).await.unwrap();
    assert_eq!(cloud_holder(&cloud, "cas-a1").as_deref(), Some("sw-w"));

    let proposed = soundwave
        .factory(json!({"id": EPIC, "target": "prowl", "mine": "cas-a1,cas-a2", "theirs": "cas-d1"}))
        .await
        .unwrap();
    let id = split_id(&proposed);
    assert_eq!(
        cloud_holder(&cloud, "cas-a1").as_deref(),
        Some("sw-w"),
        "proposing never moves the worker's claim to the supervisor"
    );
    assert_eq!(cloud_holder(&cloud, "cas-a2").as_deref(), Some("sw"));

    assert_eq!(prowl.tick().await.delivered, 1);
    prowl
        .factory(json!({"id": id, "decision": "accept"}))
        .await
        .unwrap();
    assert_eq!(soundwave.tick().await.delivered, 1);

    // prowl's refused start and claim of soundwave's children take nothing.
    for child in ["cas-a1", "cas-a2"] {
        let refused = prowl.start(child, false).await.unwrap_err();
        assert!(refused.contains("SPLIT TO A PEER"), "{refused}");
    }
    let refused = prowl.claim("cas-a2").await.unwrap_err();
    assert!(refused.contains("SPLIT TO A PEER"), "{refused}");
    assert_eq!(cloud_holder(&cloud, "cas-a1").as_deref(), Some("sw-w"));
    assert_eq!(cloud_holder(&cloud, "cas-a2").as_deref(), Some("sw"));
    assert_eq!(cloud_holder(&cloud, "cas-d1").as_deref(), Some("pr"));

    // soundwave cancels: its own split claim goes, its worker's stays, and
    // prowl's is untouched.
    let cancelled = soundwave
        .factory(json!({"id": id, "decision": "cancel"}))
        .await
        .unwrap();
    assert!(cancelled.contains("released 1 of your own claim"), "{cancelled}");
    assert_eq!(cloud_holder(&cloud, "cas-a1").as_deref(), Some("sw-w"));
    assert_eq!(cloud_holder(&cloud, "cas-a2"), None);
    assert_eq!(cloud_holder(&cloud, "cas-d1").as_deref(), Some("pr"));
    assert_eq!(prowl.tick().await.delivered, 1);
    assert_eq!(
        cloud_holder(&cloud, "cas-d1").as_deref(),
        Some("pr"),
        "receiving the cancel never releases prowl's claim"
    );
}

/// cas-c7c7 (O2): the split prowl receives names soundwave by the machine
/// label prowl's discovery shows for it, not by its registered machine id.
#[tokio::test(flavor = "multi_thread")]
async fn a_received_split_names_the_proposer_by_its_machine_label() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let soundwave = side(&cloud, "machine-soundwave", "sw", "soundwave");
    let prowl = side(&cloud, "machine-prowl", "pr", "prowl");
    let proposed = soundwave
        .factory(json!({"id": EPIC, "target": "prowl", "mine": "cas-a1", "theirs": "cas-d1"}))
        .await
        .unwrap();
    let id = split_id(&proposed);
    assert_eq!(prowl.tick().await.delivered, 1);

    let received = cas_store::get_epic_split(&prowl.machine.cas_dir, &id).unwrap().unwrap();
    let discovered = match cas::cloud::peers::discover_peers(&prowl.machine.cas_dir, Some("pr")).unwrap() {
        cas::cloud::peers::PeerDiscovery::Found { peers, .. } => {
            peers.into_iter().find(|peer| peer.agent_id == "sw").unwrap().machine
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(discovered, "soundwave-host", "discovery shows soundwave's own label");
    assert_eq!(received.peer_machine, discovered, "the split names soundwave by the same label");
}

/// Cloud #155: actual bearer accounts share one team project. Refusals go
/// through real MCP handlers; the local lease and spawn queue must stay empty.
#[tokio::test]
async fn cas_8ea7_two_accounts_refuse_start_claim_spawn_and_show_team_claims() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    {
        let mut state = cloud.state.lock().unwrap();
        state.accounts.insert("token-a".into(), "account-a".into());
        state.accounts.insert("token-b".into(), "account-b".into());
        state.team_projects.insert(
            WIDGETS.into(),
            (
                "sandbox-team".into(),
                vec!["account-a".into(), "account-b".into()],
            ),
        );
    }
    let alpha = side_account(&cloud, "machine-alpha", "a", "alpha", "token-a");
    let beta = side_account(&cloud, "machine-beta", "b", "beta", "token-b");
    let claims = cas::cloud::peer_claims::PeerClaims::for_project(&alpha.machine.cas_dir).unwrap();
    assert_eq!(
        claims.acquire(
            "cas-a1",
            cas::cloud::peer_claims::ClaimKind::Task,
            "a",
            "alpha",
            600,
            &|_| false
        ),
        cas::cloud::peer_claims::Acquire::Claimed
    );
    // Assign to beta's worker so start reaches the peer check.
    let tasks = beta.machine.task_store();
    let mut task = tasks.get("cas-a1").unwrap();
    task.assignee = Some("b-w".into());
    tasks.update(&task).unwrap();
    for refused in [
        beta.start("cas-a1", false).await.unwrap_err(),
        beta.claim("cas-a1").await.unwrap_err(),
    ] {
        assert!(
            refused.contains("HELD BY A PEER")
                && refused.contains("alpha")
                && refused.contains("alpha-host")
                && refused.contains("account-a"),
            "{refused}"
        );
    }
    let refused = beta
        .factory(json!({"action":"spawn_workers", "count":1, "task_id":"cas-a1", "isolate":true}))
        .await
        .unwrap_err();
    assert!(
        refused.contains("HELD BY A PEER") && refused.contains("alpha-host"),
        "{refused}"
    );
    assert!(
        cas::store::open_spawn_queue_store(&beta.machine.cas_dir)
            .unwrap()
            .peek(10)
            .unwrap()
            .is_empty()
    );
    assert!(
        beta.machine
            .agent_store()
            .get_lease("cas-a1")
            .unwrap()
            .is_none()
    );
    let report = beta
        .coordination(json!({"action":"peers", "peer":"alpha", "detail":true, "json":true}))
        .await
        .unwrap();
    let report: Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        report["peers"][0]["claims"][0]["task_id"], "cas-a1",
        "{report}"
    );
    assert_eq!(report["peers"][0]["user_id"], "account-a");
    assert_eq!(report["peers"][0]["claims_status"]["state"], "ready");
    assert!(claims.release_own("cas-a1", "a").unwrap());
    assert!(
        beta.claim("cas-a1")
            .await
            .unwrap()
            .contains("Task claimed:")
    );
}
