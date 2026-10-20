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

    let accepted = prowl
        .factory(json!({"id": id, "decision": "accept"}))
        .await
        .unwrap();
    assert!(accepted.contains("Accepted epic split"), "{accepted}");
    for child in PROWL_TAKES {
        assert_eq!(cloud_holder(cloud, child).as_deref(), Some("pr"), "{child}");
    }
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

/// The demo: propose, accept, and each side refused on the other's children.
#[tokio::test(flavor = "multi_thread")]
async fn epic_split_each_supervisor_is_refused_on_the_others_children() {
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

    // Each side is refused on the other's children: start, claim and spawn.
    for child in PROWL_TAKES {
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

    // Force takes a child anyway, visibly.
    let forced = soundwave.start("cas-d3", true).await.unwrap();
    assert!(forced.contains("SPLIT OVERRIDE"), "{forced}");

    // The peers detail view shows the split on both sides.
    for supervisor in [&soundwave, &prowl] {
        let peers = supervisor
            .coordination(json!({"action": "peers"}))
            .await
            .unwrap();
        assert!(peers.contains("Epic splits:") && peers.contains(&id), "{peers}");
        assert!(peers.contains("accepted"), "{peers}");
    }
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
