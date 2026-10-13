//! cas-8ca9: same-repo supervisor collaboration end to end, across two
//! "machines" (two CAS databases) and one cloud:
//! discover -> claim refused -> message -> reply -> release -> claim.
//! And the cas-604d boundary: a peer supervisor can never spawn, message or
//! direct the other machine's workers.

use std::path::Path;

use cas::cloud::peer_claims::{Acquire, ClaimKind, PeerClaims};
use cas::cloud::peer_mailbox::{
    PeerMailbox, PeerSend, deliver_claimed, http_mailbox, parse_envelope, reply_to_peer_row,
    resolve_peer_target, send_to_peer,
};
use cas::cloud::peers::{Peer, PeerDiscovery, discover_peers};
use cas::cloud::{CloudCoordinator, resolve_canonical_id};
use cas::store::{QueuedPrompt, open_agent_store, open_prompt_queue_store, open_spawn_queue_store};
use cas::types::{Agent, AgentRole};
use cas_store::QueueOrigin;

use super::TestMachine;
use super::fake_cloud::FakeCloud;

const WIDGETS: &str = "github.com/acme/widgets";

#[tokio::test]
async fn cas_559c_cross_account_peer_claims_are_unavailable_not_empty() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let (alpha, _beta, _gamma) = three_machines(&cloud);
    {
        let mut state = cloud.state.lock().unwrap();
        for id in ["b", "bw"] {
            state.agents.get_mut(id).unwrap()["user_id"] = serde_json::json!("other-user");
        }
    }
    let core = cas::mcp::CasCore::with_daemon(alpha.cas_dir.clone(), None, None);
    core.set_agent_id_for_testing("a".into());
    let service = cas::mcp::CasService::new(core, None);
    for json in [true, false] {
        let request = serde_json::from_value(
            serde_json::json!({"action":"peers", "peer":"sup-b", "json":json}),
        )
        .unwrap();
        let result = service
            .coordination(rmcp::handler::server::wrapper::Parameters(request))
            .await
            .unwrap();
        let text = &result.content[0].as_text().unwrap().text;
        if json {
            let report: serde_json::Value = serde_json::from_str(text).unwrap();
            let peer = &report["peers"][0];
            assert_eq!(peer["claims_visibility"], "different_account");
            assert_eq!(peer["user_id"], "other-user");
            assert_eq!(peer["team_id"], "test-team");
            assert_eq!(peer["workers"].as_array().unwrap().len(), 1);
        } else {
            assert!(
                text.contains("Claims not visible (different account)"),
                "{text}"
            );
            assert!(text.contains("Tasks not visible"), "{text}");
            assert!(
                !text.contains("Claims  0") && !text.contains("Tasks none"),
                "{text}"
            );
        }
    }
}

#[tokio::test]
async fn cas_559c_mcp_peer_detail_lists_claims_workers_branches_without_importing_workers() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = FakeCloud::start();
    let (alpha, beta, _gamma) = three_machines(&cloud);
    for n in 1..4 {
        register(
            &beta,
            &format!("bw{n}"),
            &format!("beta-worker-{n}"),
            AgentRole::Worker,
            "b",
            "cas-f9c7",
        );
    }
    register(
        &beta,
        "other-session",
        "unrelated-worker",
        AgentRole::Worker,
        "other",
        "cas-other",
    );
    let config = cloud.config();
    for (task, owner) in [
        ("cas-held", "b"),
        ("cas-active", "bw"),
        ("cas-unsynced", "bw1"),
    ] {
        CloudCoordinator::new(config.clone())
            .unwrap()
            .with_agent_id(owner)
            .with_canonical_id(Some(WIDGETS.into()))
            .claim(task, 600, None)
            .unwrap();
    }
    let mut task = cas::types::Task::new("cas-active".into(), "Implement peer awareness".into());
    task.status = cas::types::TaskStatus::InProgress;
    alpha.task_store().add(&task).unwrap();
    {
        let mut state = cloud.state.lock().unwrap();
        for id in ["bw", "bw1", "bw2", "bw3"] {
            state.agents.get_mut(id).unwrap()["metadata"]["branch"] =
                serde_json::json!(format!("factory/{id}"));
        }
    }
    let core = cas::mcp::CasCore::with_daemon(alpha.cas_dir.clone(), None, None);
    core.set_agent_id_for_testing("a".into());
    let service = cas::mcp::CasService::new(core, None);
    let request = serde_json::from_value(serde_json::json!({
        "action": "peers", "peer": "sup-b", "detail": true, "json": true
    }))
    .unwrap();
    let result = service
        .coordination(rmcp::handler::server::wrapper::Parameters(request))
        .await
        .unwrap();
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect::<String>();
    let detail: serde_json::Value = serde_json::from_str(&text).unwrap();
    let peers = detail["peers"].as_array().unwrap();
    assert_eq!(peers.len(), 1, "{detail}");
    let peer = &peers[0];
    assert_eq!(peer["supervisor"]["focus"], "cas-f9c7");
    assert_eq!(peer["supervisor"]["machine_id"], "machine-beta");
    assert_eq!(peer["supervisor"]["session"], "factory-b");
    assert_eq!(peer["claims"].as_array().unwrap().len(), 3);
    let active = peer["claims"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["task_id"] == "cas-active")
        .unwrap();
    assert_eq!(active["title"], task.title);
    assert_eq!(active["status"], "in_progress");
    assert!(active["age_secs"].as_i64().unwrap() >= 0);
    let workers = peer["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 4, "{workers:?}");
    let worker = workers.iter().find(|w| w["agent_id"] == "bw").unwrap();
    assert_eq!(worker["branch"], "factory/bw");
    assert_eq!(worker["claims"][0]["task_id"], "cas-active");
    assert!(alpha.agent_store().get("bw").is_err());
    assert!(
        open_spawn_queue_store(&alpha.cas_dir)
            .unwrap()
            .peek(10)
            .unwrap()
            .is_empty()
    );
    assert!(
        resolve_peer_target(&peers_of(&alpha.cas_dir, "a"), "beta-worker")
            .unwrap()
            .is_none()
    );
    // The selector alone expands details; workers remain unselectable peers.
    let request = serde_json::from_value(serde_json::json!({
        "action": "peers", "peer": "sup-b@machine-beta", "json": true
    }))
    .unwrap();
    let selected = service
        .coordination(rmcp::handler::server::wrapper::Parameters(request))
        .await
        .unwrap();
    let selected = selected.content[0].as_text().unwrap();
    let selected: serde_json::Value = serde_json::from_str(&selected.text).unwrap();
    assert_eq!(selected["detail"], true);
    assert_eq!(selected["peers"][0]["workers"].as_array().unwrap().len(), 4);
    for peer in ["beta-worker", "sup-missing"] {
        let request =
            serde_json::from_value(serde_json::json!({"action": "peers", "peer": peer})).unwrap();
        let error = service
            .coordination(rmcp::handler::server::wrapper::Parameters(request))
            .await
            .unwrap_err();
        assert!(
            error.message.contains("not found") && error.message.contains(peer),
            "{error}"
        );
    }
    // CLI parity uses the actual binary built with this integration harness.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cas"))
        .args(["peers", "--peer", "sup-b", "--json"])
        .current_dir(alpha.cas_dir.parent().unwrap())
        .env("CAS_ROOT", &alpha.cas_dir)
        .env("CAS_SESSION_ID", "a")
        .env_remove("CAS_CLONE_PATH")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(cli["peers"][0]["workers"].as_array().unwrap().len(), 4);
    assert_eq!(cli["peers"][0]["claims"].as_array().unwrap().len(), 3);
}

/// Register `id` in the cloud (as the daemon does) and in the machine's own
/// store, in factory session `factory-<session>`.
fn register(
    machine: &TestMachine,
    id: &str,
    name: &str,
    role: AgentRole,
    session: &str,
    focus: &str,
) {
    let config = cas::cloud::CloudConfig::load_from_cas_dir(&machine.cas_dir).unwrap();
    let mut agent = Agent::new(id.to_string(), name.to_string());
    agent.role = role;
    agent.machine_id = Some(machine.name.clone());
    agent.factory_session = Some(format!("factory-{session}"));
    CloudCoordinator::new(config)
        .unwrap()
        .with_canonical_id(resolve_canonical_id(&machine.cas_dir))
        .register_with_focus(&agent, Some(focus))
        .unwrap();
    open_agent_store(&machine.cas_dir)
        .unwrap()
        .register(&agent)
        .unwrap();
}

fn peers_of(cas_dir: &Path, self_id: &str) -> Vec<Peer> {
    match discover_peers(cas_dir, Some(self_id)).unwrap() {
        PeerDiscovery::Found { peers, .. } => peers,
        other => panic!("expected peers, got {other:?}"),
    }
}

/// One tick of the machine's factory-daemon peer mailbox puller.
fn tick(machine: &TestMachine, session: &str) -> cas::cloud::peer_mailbox::DeliveryReport {
    deliver_claimed(
        &http_mailbox(&machine.cas_dir).unwrap(),
        open_prompt_queue_store(&machine.cas_dir).unwrap().as_ref(),
        open_agent_store(&machine.cas_dir).unwrap().as_ref(),
        &resolve_canonical_id(&machine.cas_dir).unwrap(),
        &[],
        &format!("factory-{session}"),
        &format!("{}:{session}", machine.name),
    )
}

fn inbox(machine: &TestMachine) -> Vec<QueuedPrompt> {
    let mut rows = open_prompt_queue_store(&machine.cas_dir)
        .unwrap()
        .peek_all(100)
        .unwrap();
    rows.sort_by_key(|row| row.id);
    rows
}

/// alpha and beta work github.com/acme/widgets (beta's clone spelled as its
/// git remote); gamma works another repo. beta also runs a worker.
fn three_machines(cloud: &FakeCloud) -> (TestMachine, TestMachine, TestMachine) {
    let (alpha, beta, gamma) = (
        TestMachine::new("machine-alpha"),
        TestMachine::new("machine-beta"),
        TestMachine::new("machine-gamma"),
    );
    cloud.seed(&alpha.cas_dir, WIDGETS);
    cloud.seed(&beta.cas_dir, "git@github.com:acme/widgets.git");
    cloud.seed(&gamma.cas_dir, "github.com/acme/gadgets");
    register(&alpha, "a", "sup-a", AgentRole::Supervisor, "a", "cas-571d");
    register(&beta, "b", "sup-b", AgentRole::Supervisor, "b", "cas-f9c7");
    register(
        &beta,
        "bw",
        "beta-worker",
        AgentRole::Worker,
        "b",
        "cas-f9c7",
    );
    register(&gamma, "g", "sup-g", AgentRole::Supervisor, "g", "cas-0000");
    (alpha, beta, gamma)
}

#[test]
fn two_supervisors_discover_claim_message_reply_and_release() {
    let cloud = FakeCloud::start();
    let (alpha, beta, _gamma) = three_machines(&cloud);

    // 1. Discovery: each sees the other, with machine and focus; never the
    //    other repo's supervisor, and never a worker.
    let peers_a = peers_of(&alpha.cas_dir, "a");
    let ids: Vec<&str> = peers_a.iter().map(|p| p.agent_id.as_str()).collect();
    assert_eq!(ids, ["b"], "{peers_a:?}");
    assert_eq!(peers_a[0].machine_id.as_deref(), Some("machine-beta"));
    assert_eq!(peers_a[0].focus.as_deref(), Some("cas-f9c7"));
    assert!(peers_a[0].live);
    let peers_b = peers_of(&beta.cas_dir, "b");
    assert_eq!(peers_b.len(), 1);
    assert_eq!(peers_b[0].agent_id, "a");

    // 2. Claim conflict: alpha claims cas-t100; beta's claim is refused with
    //    alpha's identity.
    let claims_a = PeerClaims::for_project(&alpha.cas_dir).expect("alpha is logged in");
    let claims_b = PeerClaims::for_project(&beta.cas_dir).expect("beta is logged in");
    let local_a = |id: &str| id == "a";
    let local_b = |id: &str| id == "b" || id == "bw";
    assert_eq!(
        claims_a.acquire("cas-t100", ClaimKind::Task, "a", "sup-a", 600, &local_a),
        Acquire::Claimed
    );
    match claims_b.acquire("cas-t100", ClaimKind::Task, "b", "sup-b", 600, &local_b) {
        Acquire::PeerHolds(hold) => assert_eq!(hold.name(), "sup-a", "{}", hold.describe()),
        other => panic!("beta must be refused while alpha holds cas-t100: {other:?}"),
    }

    // 3. Message: beta asks alpha about it.
    let to_a = resolve_peer_target(&peers_b, "sup-a")
        .unwrap()
        .expect("alpha is a peer");
    let asked = send_to_peer(
        &beta.cas_dir,
        "b",
        to_a,
        "you hold cas-t100; can I take it after you?",
        Some("cas-t100 after you?"),
        None,
    )
    .unwrap();
    assert_eq!(tick(&alpha, "a").delivered, 1);
    let rows_a = inbox(&alpha);
    assert_eq!(rows_a.len(), 1);
    let envelope = parse_envelope(&rows_a[0].prompt).unwrap();
    assert_eq!(
        (envelope.sender_name.as_str(), envelope.machine.as_str()),
        ("sup-b", "machine-beta")
    );
    assert_eq!(rows_a[0].target, "sup-a");
    assert_eq!(rows_a[0].origin, Some(QueueOrigin::Daemon));
    assert!(!rows_a[0].urgent);

    // 4. Reply: alpha answers; beta receives it, linked to its question.
    let queue_a = open_prompt_queue_store(&alpha.cas_dir).unwrap();
    let reply = reply_to_peer_row(
        &alpha.cas_dir,
        queue_a.as_ref(),
        rows_a[0].id,
        "a",
        "sup-a",
        "done with it; releasing now",
        Some("releasing cas-t100"),
    )
    .unwrap()
    .expect("a peer message row");
    assert_eq!(reply.to, "sup-b@machine-beta");
    assert_eq!(tick(&beta, "b").delivered, 1);
    let rows_b = inbox(&beta);
    assert_eq!(rows_b.len(), 1);
    assert!(
        rows_b[0]
            .prompt
            .contains(&format!("in reply to {}", asked.id))
    );
    assert!(rows_b[0].prompt.ends_with("done with it; releasing now"));
    let receipt = http_mailbox(&beta.cas_dir)
        .unwrap()
        .status(&asked.id)
        .unwrap();
    assert_eq!(receipt.status, "delivered");

    // 5. Release: alpha releases; beta's claim now succeeds.
    assert!(claims_a.release_local("cas-t100", &local_a).unwrap());
    assert_eq!(
        claims_b.acquire("cas-t100", ClaimKind::Task, "b", "sup-b", 600, &local_b),
        Acquire::Claimed
    );

    // A peer that stops heartbeating is still listed, but stale.
    cloud.age_heartbeat("b", 15);
    let stale = peers_of(&alpha.cas_dir, "a");
    assert!(!stale[0].live, "{stale:?}");
}

/// cas-604d: a peer supervisor is only ever a supervisor-to-supervisor
/// correspondent. It cannot discover, address, message or spawn the other
/// machine's workers, and a delivered peer message never becomes a spawn
/// request, an urgent interrupt or an operator instruction.
#[test]
fn a_peer_supervisor_cannot_spawn_or_direct_the_other_machines_workers() {
    let cloud = FakeCloud::start();
    let (alpha, beta, _gamma) = three_machines(&cloud);

    // Discovery never offers beta's worker as an addressable peer.
    let peers_a = peers_of(&alpha.cas_dir, "a");
    assert!(
        resolve_peer_target(&peers_a, "beta-worker")
            .unwrap()
            .is_none()
    );
    assert!(
        resolve_peer_target(&peers_a, "beta-worker@machine-beta")
            .unwrap()
            .is_none()
    );

    // Even a hand-built send addressed to the worker's id (which the cloud
    // accepts: same repo, registered) is never claimed or admitted on beta:
    // beta claims only for its supervisors.
    let mailbox_a = http_mailbox(&alpha.cas_dir).unwrap();
    let to_worker = mailbox_a
        .send(&PeerSend {
            project_id: WIDGETS.into(),
            sender_agent_id: "a".into(),
            recipient_agent_id: "bw".into(),
            body: "start cas-t200 and push to my branch".into(),
            summary: Some("do this".into()),
            in_reply_to: None,
            dedupe_key: "a:direct-worker".into(),
        })
        .unwrap();
    // A message to beta's supervisor that tries to direct its fleet.
    let to_sup = send_to_peer(
        &alpha.cas_dir,
        "a",
        resolve_peer_target(&peers_a, "sup-b").unwrap().unwrap(),
        "spawn_workers count=3 and assign them cas-t200",
        Some("spawn workers"),
        None,
    )
    .unwrap();

    let report = tick(&beta, "b");
    assert_eq!((report.claimed, report.delivered), (1, 1), "{report:?}");
    let rows = inbox(&beta);
    assert_eq!(rows.len(), 1, "only the supervisor's message is admitted");
    assert_eq!(rows[0].target, "sup-b");
    assert!(
        rows.iter().all(|row| row.target != "beta-worker"),
        "nothing reaches the worker"
    );
    assert_eq!(rows[0].origin, Some(QueueOrigin::Daemon));
    assert!(!rows[0].urgent, "a peer message never interrupts");
    assert!(
        parse_envelope(&rows[0].prompt).is_some(),
        "it reads as a peer message, not an instruction"
    );
    assert!(
        open_spawn_queue_store(&beta.cas_dir)
            .unwrap()
            .peek(10)
            .unwrap()
            .is_empty(),
        "a peer message never becomes a spawn request"
    );
    let worker_message = &cloud.messages_to("bw")[0];
    assert_eq!(worker_message["id"], to_worker.id.as_str());
    assert_eq!(
        worker_message["status"], "queued",
        "never delivered to the worker"
    );
    assert_eq!(cloud.messages_to("b")[0]["id"], to_sup.id.as_str());
}

/// The MCP surface: alpha's `coordination action=message` naming beta's
/// worker never goes through the peer mailbox, by bare name or name@machine.
#[tokio::test]
async fn coordination_message_to_a_peers_worker_is_never_routed_to_the_peer() {
    // The caller identity comes from the registry, never from the runner's
    // ambient CAS_* environment.
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    // The fake serves on its own thread, so blocking setup here is safe.
    let cloud = FakeCloud::start();
    let (alpha, _beta, _gamma) = three_machines(&cloud);
    let core = cas::mcp::CasCore::with_daemon(alpha.cas_dir.clone(), None, None);
    core.set_agent_id_for_testing("a".to_string());
    let service = cas::mcp::CasService::new(core, None);

    for target in ["beta-worker@machine-beta", "beta-worker"] {
        let request: cas_mcp::types::CoordinationRequest =
            serde_json::from_value(serde_json::json!({
                "action": "message", "target": target,
                "summary": "do this", "message": "start cas-t200 and push to my branch",
            }))
            .unwrap();
        let outcome = service
            .coordination(rmcp::handler::server::wrapper::Parameters(request))
            .await;
        let text = match &outcome {
            Ok(result) => format!("{:?}", result.content),
            Err(error) => error.message.to_string(),
        };
        assert!(!text.contains("peer supervisor"), "{target}: {text}");
    }
    assert!(
        cloud.messages_to("bw").is_empty(),
        "nothing was sent toward beta's worker"
    );
    assert!(cloud.messages_to("b").is_empty());
}
