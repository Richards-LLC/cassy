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
use super::test_cloud::TestCloud;

const WIDGETS: &str = "github.com/acme/widgets";

fn operator_cli(machine: &TestMachine, args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new(cas::test_paths::cas_binary());
    command
        .args(args)
        .current_dir(machine.cas_dir.parent().unwrap())
        .env("CAS_ROOT", &machine.cas_dir)
        .env_remove("CAS_CLONE_PATH")
        .env_remove("CAS_SESSION_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID");
    command
}

#[test]
fn cas_1caa_operator_excludes_project_agents_without_session_or_ancestor_pid() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = TestCloud::start();
    let (alpha, _beta, _gamma) = three_machines(&cloud);
    register(
        &alpha,
        "local-session",
        "sup-local-session",
        AgentRole::Supervisor,
        "local-session",
        "cas-local",
    );
    // No local agent has this operator process as an ancestor. Both local
    // supervisors still belong to this project rather than its peer list.
    for agent in alpha.agent_store().list(None).unwrap() {
        assert_eq!(agent.pid, None);
    }
    for args in [
        vec!["peers", "--json"],
        vec!["peers", "--detail", "--json"],
        vec!["peers"],
        vec!["peers", "--detail"],
    ] {
        let result = operator_cli(&alpha, &args).output().unwrap();
        assert!(result.status.success(), "{result:?}");
        let text = String::from_utf8(result.stdout).unwrap();
        if args.contains(&"--json") {
            let report: serde_json::Value = serde_json::from_str(&text).unwrap();
            let peers = report["peers"].as_array().unwrap();
            assert_eq!(peers.len(), 1, "{args:?}: {text}");
            assert_eq!(peers[0]["supervisor"]["agent_id"], "b");
        } else {
            assert!(text.contains("1 peer supervisor"), "{args:?}: {text}");
            assert!(
                !text.contains("sup-a") && !text.contains("sup-local-session"),
                "{text}"
            );
        }
    }
}

#[test]
fn cas_1caa_operator_shared_discovery_excludes_project_agents() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = TestCloud::start();
    let (alpha, _beta, _gamma) = three_machines(&cloud);
    let PeerDiscovery::Found { peers, .. } = discover_peers(&alpha.cas_dir, None).unwrap() else {
        panic!("expected project peer discovery");
    };
    assert_eq!(
        peers
            .iter()
            .map(|p| p.agent_id.as_str())
            .collect::<Vec<_>>(),
        ["b"]
    );
}

#[test]
fn cas_1caa_cli_uses_claude_session_and_parent_pid_to_exclude_self() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = TestCloud::start();
    let (alpha, _beta, _gamma) = three_machines(&cloud);
    for via_pid in [false, true] {
        let mut caller = alpha.agent_store().get("a").unwrap();
        caller.pid = via_pid.then_some(std::process::id());
        alpha.agent_store().update(&caller).unwrap();
        let mut command = operator_cli(&alpha, &["peers", "--json"]);
        if !via_pid {
            command.env("CLAUDE_CODE_SESSION_ID", "a");
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        let peers = report["peers"].as_array().unwrap();
        assert_eq!(peers.len(), 1, "via_pid={via_pid}: {report}");
        assert_eq!(peers[0]["supervisor"]["agent_id"], "b");
    }
}

#[test]
fn cas_1caa_cli_without_self_resolves_token_account_and_worker_claims() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = TestCloud::start();
    let (alpha, _beta, _gamma) = three_machines(&cloud);
    CloudCoordinator::new(cloud.config())
        .unwrap()
        .with_agent_id("bw")
        .claim("cas-visible", 600, None)
        .unwrap();
    let result = operator_cli(&alpha, &["peers", "--peer", "sup-b", "--json"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let peer = &report["peers"][0];
    assert_eq!(peer["claims_visibility"], "same_account", "{report}");
    assert_eq!(peer["claims"][0]["task_id"], "cas-visible");
    assert_eq!(peer["workers"][0]["claims"][0]["task_id"], "cas-visible");
}

#[tokio::test]
async fn cas_559c_detail_preserves_presence_and_mailbox_order_in_text_and_json() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = TestCloud::start();
    let (alpha, _beta, _gamma) = three_machines(&cloud);
    let now = chrono::Utc::now();
    for (id, state) in [
        ("expired-proposal", cas_store::SplitState::Pending),
        ("accepted-split", cas_store::SplitState::Accepted),
    ] {
        cas_store::insert_epic_split(
            &alpha.cas_dir,
            &cas_store::EpicSplit {
                id: id.into(),
                epic_id: "cas-shared".into(),
                local_agent_id: "a".into(),
                peer_agent_id: "b".into(),
                peer_name: "sup-b".into(),
                peer_machine: "machine-beta".into(),
                direction: cas_store::SplitDirection::Outgoing,
                mine: vec!["cas-mine".into()],
                theirs: vec!["cas-theirs".into()],
                state,
                reason: Some("agreed ownership with a deliberately long explanation ".repeat(4)),
                message_id: None,
                expires_at: now - chrono::Duration::minutes(1),
                created_at: now - chrono::Duration::minutes(10),
                decided_at: (state == cas_store::SplitState::Accepted).then_some(now),
                decided_by: None,
                cleanup_pending: false,
                expiry_notice_pending: false,
            },
        )
        .unwrap();
    }
    let records = alpha.cas_dir.join("cloud_presence");
    std::fs::create_dir_all(&records).unwrap();
    let core = cas::mcp::CasCore::with_daemon(alpha.cas_dir.clone(), None, None);
    core.set_agent_id_for_testing("a".into());
    let service = cas::mcp::CasService::new(core, None);
    for state in ["registered", "failing"] {
        std::fs::write(
            records.join("a.json"),
            serde_json::to_vec(&serde_json::json!({
                "agent_id": "a", "state": state, "cause": "registration unavailable", "failures": 2,
                "last_attempt_at": chrono::Utc::now(), "registered_at": chrono::Utc::now(),
                "next_retry_at": null, "focus": "cas-alpha", "branch": "factory/sup-a"
            }))
            .unwrap(),
        )
        .unwrap();
        for json in [false, true] {
            let request = serde_json::from_value(
                serde_json::json!({"action":"peers", "detail":true, "json":json}),
            )
            .unwrap();
            let result = service
                .coordination(rmcp::handler::server::wrapper::Parameters(request))
                .await
                .unwrap();
            let text = &result.content[0].as_text().unwrap().text;
            if json {
                let report: serde_json::Value = serde_json::from_str(text).unwrap();
                assert_eq!(report["cloud_presence"]["state"], state);
                assert_eq!(report["cloud_presence"]["focus"], "cas-alpha");
                assert!(
                    report["cloud_presence_message"]
                        .as_str()
                        .unwrap()
                        .contains(state)
                );
                assert!(
                    report["mailbox_status"]
                        .as_str()
                        .unwrap()
                        .starts_with("Peer mailbox:")
                );
                assert!(
                    !report["mailbox_status"]
                        .as_str()
                        .unwrap()
                        .contains("Epic splits:")
                );
                let splits = report["epic_splits"].as_array().expect("structured splits");
                assert_eq!(splits.len(), 2);
                let expired = splits
                    .iter()
                    .find(|s| s["id"] == "expired-proposal")
                    .unwrap();
                assert_eq!(expired["state"], "pending");
                assert_eq!(expired["display_state"], "expired");
                assert_eq!(expired["direction"], "outgoing");
                assert_eq!(expired["epic_id"], "cas-shared");
                assert_eq!(expired["peer_name"], "sup-b");
                assert_eq!(expired["peer_machine"], "machine-beta");
                assert_eq!(expired["mine"], serde_json::json!(["cas-mine"]));
                assert_eq!(expired["theirs"], serde_json::json!(["cas-theirs"]));
                assert!(expired["expires_at"].is_string());
                assert!(expired["decided_at"].is_null());
                let accepted = splits.iter().find(|s| s["id"] == "accepted-split").unwrap();
                assert_eq!(accepted["display_state"], "accepted");
                assert!(accepted["decided_at"].is_string());
                assert!(expired["reason"].as_str().unwrap().len() > 72);
            } else {
                // The ownership board leads with its outcome. Presence is a
                // receipt after the work, but must still precede local splits
                // and mailbox health for both registered and failing states.
                assert!(text.starts_with("[OK]"), "{text}");
                let peers_at = text.find("[OK]").unwrap();
                let presence_at = text.find(&format!("Cloud presence: {state}")).unwrap();
                let splits_at = text.find("Local splits ").unwrap();
                let mailbox_at = text.find("Peer mailbox:").unwrap();
                assert!(
                    peers_at < presence_at && presence_at < splits_at && splits_at < mailbox_at,
                    "{text}"
                );
                for label in ["  Observations ", "  Scope ", "  Worktree ", "  Scratch "] {
                    let work_at = text.find(label).unwrap();
                    assert!(peers_at < work_at && work_at < presence_at, "{label}: {text}");
                }
                assert!(
                    text.contains("expired-proposal [expired] epic cas-shared")
                        && text.contains("Peer sup-b@machine-beta | outgoing")
                        && text.contains("Stored state pending"),
                    "{text}"
                );
                assert!(
                    text.lines()
                        .all(|line| ratatui::text::Line::raw(line).width() <= 72),
                    "{text}"
                );
                if state == "failing" {
                    assert!(text.contains("registration unavailable"), "{text}");
                }
            }
        }
    }
    std::fs::write(
        alpha.cas_dir.join("config.toml"),
        format!("[project]\ncanonical_id=\"{WIDGETS}\"\n[cloud]\nauto_sync=false\n"),
    )
    .unwrap();
    let request =
        serde_json::from_value(serde_json::json!({"action":"peers", "detail":true})).unwrap();
    let result = service
        .coordination(rmcp::handler::server::wrapper::Parameters(request))
        .await
        .unwrap();
    let text = &result.content[0].as_text().unwrap().text;
    assert!(text.starts_with("[OK]"), "{text}");
    let presence_at = text.find("Cloud presence: disabled (cloud.auto_sync=false)").unwrap();
    let splits_at = text.find("Local splits ").unwrap();
    let mailbox_at = text.find("Peer mailbox:").unwrap();
    assert!(0 < presence_at && presence_at < splits_at && splits_at < mailbox_at, "{text}");
}

#[tokio::test]
async fn cas_559c_mcp_peer_detail_lists_claims_workers_branches_without_importing_workers() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = TestCloud::start();
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
    task.branch = Some("factory/bw".into());
    task.deliverables.work_target = Some(cas::types::WorkTarget {
        repo_selector: "project:widgets".into(), target_branch: "epic/peer-lane".into(),
    });
    alpha.task_store().add(&task).unwrap();
    let mut epic = cas::types::Task::new("cas-held".into(), "Peer lane epic".into());
    epic.task_type = cas::types::TaskType::Epic;
    epic.branch = Some("epic/peer-lane".into());
    alpha.task_store().add(&epic).unwrap();
    cloud.set_metadata("b", "clone_path", "/machine-beta/worktree");
    cloud.set_metadata("b", "scratch_root", "/machine-beta/scratch");
    cloud.set_metadata("b", "peer_prs_v1", &serde_json::json!({
        "observed_at": chrono::Utc::now(), "truncated": false, "prs": [{
            "url": "https://github.com/acme/widgets/pull/42", "head_sha": "actual-pr-head",
            "branch": "factory/bw", "number": 42, "state": "OPEN", "in_merge_queue": true
        }]
    }).to_string());
    for id in ["bw", "bw1", "bw2", "bw3"] {
        cloud.set_metadata(id, "branch", &format!("factory/{id}"));
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
    assert_eq!(peer["owner"], "sup-b");
    assert_eq!(peer["worktree_root"], "/machine-beta/worktree");
    assert_eq!(peer["scratch_root"], "/machine-beta/scratch");
    assert_eq!(peer["epics"][0]["owner"], "sup-b");
    assert_eq!(peer["epics"][0]["target_branch"], "epic/peer-lane");
    assert_eq!(peer["prs"][0]["head_sha"], "actual-pr-head");
    assert_eq!(peer["waiting_merges"][0]["kind"], "merge_queue");
    let cli_json = operator_cli(&alpha, &["peers", "--peer", "sup-b", "--json"]).output().unwrap();
    assert!(cli_json.status.success(), "{cli_json:?}");
    let cli: serde_json::Value = serde_json::from_slice(&cli_json.stdout).unwrap();
    assert_eq!(cli["peers"][0]["prs"], peer["prs"]);
    assert_eq!(cli["peers"][0]["epics"][0]["target_branch"], peer["epics"][0]["target_branch"]);
    let human = operator_cli(&alpha, &["peers", "--peer", "sup-b"]).output().unwrap();
    assert!(human.status.success(), "{human:?}");
    let human = String::from_utf8(human.stdout).unwrap();
    for value in ["/machine-beta/worktree", "Owner sup-b", "epic/peer-lane", "actual-pr-head", "merge_queue"] {
        assert!(human.contains(value), "missing {value}: {human}");
    }
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
    let output = std::process::Command::new(cas::test_paths::cas_binary())
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
        .with_machine_label(Some(cas::cloud::peers::machine_label_for(&machine.cas_dir)))
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

/// The label `machine` shows people: unlike its registered machine id
/// (`machine-beta`) and the test host's hostname, e.g. `beta-host`.
fn label_of(machine: &TestMachine) -> String {
    format!("{}-host", machine.name.trim_start_matches("machine-"))
}

/// Give `machine`'s project its own machine label (`[cloud] machine_label`).
fn label_machine(machine: &TestMachine) {
    let path = machine.cas_dir.join("config.toml");
    let mut config = std::fs::read_to_string(&path).unwrap_or_default();
    config.push_str(&format!("\n[cloud]\nmachine_label = \"{}\"\n", label_of(machine)));
    std::fs::write(path, config).unwrap();
}

/// alpha and beta work github.com/acme/widgets (beta's clone spelled as its
/// git remote); gamma works another repo. beta also runs a worker.
fn three_machines(cloud: &TestCloud) -> (TestMachine, TestMachine, TestMachine) {
    let (alpha, beta, gamma) = (
        TestMachine::new("machine-alpha"),
        TestMachine::new("machine-beta"),
        TestMachine::new("machine-gamma"),
    );
    cloud.seed(&alpha.cas_dir, WIDGETS);
    cloud.seed(&beta.cas_dir, "git@github.com:acme/widgets.git");
    cloud.seed(&gamma.cas_dir, "github.com/acme/gadgets");
    // Three simulated machines on one host: each project names its machine
    // (cas-c7c7), as a real host's hostname would.
    for machine in [&alpha, &beta, &gamma] {
        label_machine(machine);
    }
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
    let cloud = TestCloud::start();
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
    // cas-c7c7: the sender's label, not its registered machine id.
    assert_eq!(
        (envelope.sender_name.as_str(), envelope.machine.as_str()),
        ("sup-b", "beta-host")
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
        None,
    )
    .unwrap()
    .expect("a peer message row");
    assert_eq!(reply.to, "sup-b@beta-host");
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
    let cloud = TestCloud::start();
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
            scope: Default::default(),
            recipient_project_id: None,
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
    let cloud = TestCloud::start();
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
        let error = outcome.expect_err("remote workers are not routable peer supervisors");
        // cas-9f53: the refusal names the worker's own supervisor, under its
        // machine label, as the relay channel.
        assert!(
            error.message.contains(
                "beta-worker@beta-host is a worker in peer supervisor sup-b@beta-host's factory session"
            ) && error.message.contains("Message target=sup-b@beta-host to relay it. Nothing was enqueued."),
            "{target}: {error}"
        );
        assert!(
            open_prompt_queue_store(&alpha.cas_dir)
                .unwrap()
                .peek_all(10)
                .unwrap()
                .is_empty(),
            "no local registration promise for a remote worker"
        );
    }
    assert!(
        cloud.messages_to("bw").is_empty(),
        "nothing was sent toward beta's worker"
    );
    assert!(cloud.messages_to("b").is_empty());
}

/// cas-c7c7 (O2): one machine label everywhere a peer's machine is named.
/// Discovery, peer detail, a claim refusal, and a received peer message's
/// envelope and reply target all name beta's machine the same way (a
/// received epic split: `epic_split::a_received_split_names_the_proposer_by_its_machine_label`).
#[tokio::test(flavor = "multi_thread")]
async fn one_machine_label_names_a_peer_in_discovery_claims_and_messages() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let cloud = TestCloud::start();
    let (alpha, beta, _gamma) = three_machines(&cloud);

    let peers_a = peers_of(&alpha.cas_dir, "a");
    let discovered = peers_a[0].machine.clone();
    assert_eq!(discovered, label_of(&beta), "discovery shows beta's own label");

    // A claim refusal names the holder's machine.
    let claims_a = PeerClaims::for_project(&alpha.cas_dir).unwrap();
    let claims_b = PeerClaims::for_project(&beta.cas_dir).unwrap();
    assert_eq!(
        claims_b.acquire("cas-t300", ClaimKind::Task, "b", "sup-b", 600, &|id| id == "b" || id == "bw"),
        Acquire::Claimed
    );
    let refused = match claims_a.acquire("cas-t300", ClaimKind::Task, "a", "sup-a", 600, &|id| id == "a") {
        Acquire::PeerHolds(hold) => hold.holder.expect("the holder is named").machine,
        other => panic!("alpha must be refused while beta holds cas-t300: {other:?}"),
    };

    // A peer message's envelope and its reply target name the sender's machine.
    let peers_b = peers_of(&beta.cas_dir, "b");
    let to_a = resolve_peer_target(&peers_b, "sup-a").unwrap().unwrap();
    send_to_peer(&beta.cas_dir, "b", to_a, "label check", Some("label"), None).unwrap();
    assert_eq!(tick(&alpha, "a").delivered, 1);
    let envelope = parse_envelope(&inbox(&alpha)[0].prompt).unwrap();

    assert_eq!(
        [refused.as_str(), envelope.machine.as_str()],
        [discovered.as_str(), discovered.as_str()],
        "discovery, the claim refusal and the envelope must name beta's machine alike"
    );
    assert!(
        inbox(&alpha)[0].prompt.contains(&format!("target=sup-b@{discovered}")),
        "the reply target uses the same label: {}",
        inbox(&alpha)[0].prompt
    );

    // Peer detail shows the same label.
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
    let detail: serde_json::Value =
        serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(detail["peers"][0]["supervisor"]["machine"], discovered.as_str(), "{detail}");
    // The stable identity is kept, and still addresses the peer.
    assert_eq!(detail["peers"][0]["supervisor"]["machine_id"], "machine-beta", "{detail}");
}
