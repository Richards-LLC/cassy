//! cas-f9c7: the wake gate for cloud-relayed peer supervisor messages.
use std::collections::HashMap;

use chrono::Utc;

use super::queue_and_events::{PaneWakeState, ToolCallEvidence, WakeSender};
use crate::cloud::peer_mailbox::{ClaimedPeerMessage, render_prompt};
use crate::ui::factory::daemon::FactoryDaemon;
use crate::ui::factory::director::{AgentSummary, DirectorData};

struct ClaimedMailbox(Vec<ClaimedPeerMessage>);

impl crate::cloud::peer_mailbox::PeerMailbox for ClaimedMailbox {
    fn send(
        &self,
        _: &crate::cloud::peer_mailbox::PeerSend,
    ) -> Result<crate::cloud::peer_mailbox::SendReceipt, String> {
        unreachable!()
    }
    fn claim(
        &self,
        _: &[String],
        _: &[String],
        _: &str,
    ) -> Result<Vec<ClaimedPeerMessage>, String> {
        Ok(self.0.clone())
    }
    fn ack(
        &self,
        _: &str,
        _: &[(String, crate::cloud::peer_mailbox::PeerAckOutcome)],
    ) -> Result<(), String> {
        Ok(())
    }
    fn status(&self, _: &str) -> Result<crate::cloud::peer_mailbox::PeerMessageStatus, String> {
        unreachable!()
    }
}

pub(super) async fn peer_fixture() -> (tempfile::TempDir, FactoryDaemon) {
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().unwrap();
    let cas = crate::store::init_cas_dir(root.path()).unwrap();
    let mut daemon = super::provisioning_tests::daemon(&cas);
    let name = daemon.app.supervisor_name().to_string();
    let mut supervisor = cas_types::Agent::new_with_role(
        "local-sup".into(),
        name.clone(),
        cas_types::AgentRole::Supervisor,
    );
    supervisor.factory_session = Some(daemon.session_name.clone());
    supervisor.status = cas_types::AgentStatus::Idle;
    crate::store::open_agent_store(&cas)
        .unwrap()
        .register(&supervisor)
        .unwrap();
    let pty = cas_mux::Pty::spawn(
        &name,
        cas_mux::PtyConfig {
            command: "/bin/sh".into(),
            // cas-ac6c: consume input like a harness. A sleeping child leaves
            // the cooked PTY buffer full when two peer replies are batched,
            // so payload/submit receipts cannot settle within the test bound.
            args: vec![
                "-c".into(),
                "stty raw -echo; printf ready; exec cat > \"$1\"".into(),
                "peer-fixture".into(),
                root.path().join("input").to_str().unwrap().into(),
            ],
            cwd: Some(root.path().into()),
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        },
    )
    .unwrap();
    daemon.app.mux.add_pane(
        cas_mux::Pane::with_pty(
            &name,
            cas_mux::PaneKind::Supervisor,
            pty,
            24,
            80,
            cas_mux::SupervisorCli::Claude,
        )
        .unwrap(),
    );
    let teams = super::teams::TeamsManager::new(&daemon.session_name);
    teams
        .init_team_config(&[], root.path(), &HashMap::new(), "local-sup")
        .unwrap();
    assert!(
        !teams.team_tree_missing(),
        "exercise Teams delivery, never its missing-tree PTY fallback"
    );
    daemon.teams = Some(teams);
    let bytes = super::provisioning_tests::await_ready_fixture_pane(&mut daemon, &name).await;
    assert!(bytes > 0);
    daemon.last_pane_output_bytes.insert(name.clone(), bytes);
    daemon
        .pane_silent_since
        .insert(name.clone(), Instant::now() - Duration::from_secs(600));
    daemon.app.refresh_data().unwrap();
    assert!(
        daemon
            .app
            .director_data()
            .agents
            .iter()
            .any(|agent| agent.name == name)
    );
    (root, daemon)
}

fn peer_message(id: &str, reply: bool) -> ClaimedPeerMessage {
    ClaimedPeerMessage {
        id: id.into(),
        project_id: "github.com/acme/widgets".into(),
        recipient_agent_id: "local-sup".into(),
        sender_agent_id: "remote-sup".into(),
        sender_name: "far-heron-3".into(),
        sender_machine_id: Some("other-box".into()),
        sender_session: Some("other-factory".into()),
        sender_label: None,
        body: "peer coordination".into(),
        summary: None,
        in_reply_to: reply.then(|| "pm-question".into()),
        created_at: Utc::now().to_rfc3339(),
        attempts: 1,
        scope: Default::default(),
    }
}

fn age_imported(daemon: &FactoryDaemon) {
    rusqlite::Connection::open(daemon.app.cas_dir().join("cas.db")).unwrap().execute(
        "UPDATE prompt_queue SET created_at=?1,next_attempt_at=?1",
        [(Utc::now()-chrono::Duration::seconds(21)).to_rfc3339()]).unwrap();
}

fn import(
    daemon: &FactoryDaemon,
    messages: Vec<ClaimedPeerMessage>,
) -> crate::cloud::peer_mailbox::DeliveryReport {
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let agents = crate::store::open_agent_store(daemon.app.cas_dir()).unwrap();
    crate::cloud::peer_mailbox::deliver_claimed(
        &ClaimedMailbox(messages),
        queue.as_ref(),
        agents.as_ref(),
        "github.com/acme/widgets",
        &[],
        &daemon.session_name,
        "local-consumer",
    )
}

#[tokio::test(flavor = "current_thread")]
async fn peer_reply_real_queue_wakes_idle_supervisor_once_per_pass_cas_1f25() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = peer_fixture().await;
    let replies = [None, Some("answer")].into_iter().enumerate().map(|(i, kind)| {
        let mut reply = peer_message(&format!("pm-reply-{i}"), true);
        if let Some(kind) = kind {
            reply.body = crate::cloud::peer_mailbox::attach_message_intent(&reply.body, Some(kind), false).unwrap();
        }
        reply
    }).collect();
    let imported = import(&daemon, replies);
    assert_eq!(imported.delivered, 2, "{imported:?}");
    age_imported(&daemon);
    daemon.process_prompt_queue().await.unwrap();
    super::supervisor_single_turn_tests::settle_injected_writes(&mut daemon).await;
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let reports: Vec<_> = imported
        .admitted
        .iter()
        .map(|id| queue.message_delivery_report(*id).unwrap().unwrap())
        .collect();
    assert_eq!(
        reports
            .iter()
            .filter(|report| report.wake_attempt == cas_store::WakeAttempt::Fired)
            .count(),
        1,
        "{reports:?}"
    );
    assert!(
        queue
            .queued_prompt(imported.admitted[1])
            .unwrap()
            .unwrap()
            .processed_at
            .is_some(),
        "both replies are carried by the same turn"
    );
}

/// cas-7dd1d: replay gabber's three overnight untyped asks to prowl (rows
/// 2581417-2581419, Nov 5) through the real sender stamp. They were stamped
/// kind=receipt and parked quiet for 7 h. Each must now fire a wake, whether
/// it comes from a same-repo peer on another machine, another project on
/// another machine, or another project on this machine; a legacy body with
/// no intent line still wakes too.
#[tokio::test(flavor = "current_thread")]
async fn untyped_peer_mail_wakes_idle_supervisor_prowl_rows_cas_7dd1d() {
    use crate::cloud::peer_mailbox::{PeerScope, attach_message_intent};
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let stamp = |body: &str| attach_message_intent(body, None, false).unwrap();
    // cas-35563: a published landing wakes, so the supervisor sees its train
    // move. cas-9cda: #4240970 (Nov 12) rendered it as kind=ask with a reply
    // instruction; it is a status that asks for no answer.
    let landing = format!("CAS_PEER_PREMERGE_V1 {}", serde_json::to_string(&cas_types::PeerPremergeReceipt {
        status: cas_types::VerificationStatus::Approved,
        project_id: "github.com/acme/widgets".into(),
        supervisor_id: "remote-sup".into(),
        task_id: "cas-30b0".into(),
        source_branch: "factory/peer".into(),
        source_sha: "a".repeat(40),
        target_branch: "epic/release".into(),
        target_sha: "b".repeat(40),
        candidate_tree: "c".repeat(40),
        landed_sha: "d".repeat(40),
        dispatch_id: "dispatch-landing".into(),
        verdict_id: "ver-landing".into(),
        plan_digest: "e".repeat(64),
    }).unwrap());
    let cases = [
        ("pm-9b7b9e0b", PeerScope::Project, "other-box", stamp("Release frozen and green. Please start the review now."), true),
        ("pm-8e716afb", PeerScope::User, "other-box", stamp("What's its ETA?"), true),
        ("pm-9c915946", PeerScope::User, "this-box", stamp("Reply right away: the release is waiting on you."), true),
        ("pm-legacy", PeerScope::Project, "other-box", "Substantive field report".to_string(), true),
        ("pm-landing-4240970", PeerScope::Project, "other-box", landing.clone(), true),
        ("pm-explicit-receipt", PeerScope::Project, "other-box", attach_message_intent(&landing, Some("receipt"), false).unwrap(), false),
        ("pm-explicit-quiet", PeerScope::Project, "other-box", format!("<cas-message-quiet/>\n{landing}"), false),
    ];
    for (id, scope, machine, body, wakes) in cases {
        let (_root, mut daemon) = peer_fixture().await;
        let mut message = peer_message(id, false);
        (message.scope, message.body) = (scope, body);
        message.sender_machine_id = Some(machine.into());
        if scope == PeerScope::User {
            message.project_id = "github.com/acme/gabber".into();
        }
        let imported = import(&daemon, vec![message]);
        assert_eq!(imported.delivered, 1, "{id}: {imported:?}");
        age_imported(&daemon);
        daemon.process_prompt_queue().await.unwrap();
        super::supervisor_single_turn_tests::settle_injected_writes(&mut daemon).await;
        let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
        let report = queue
            .message_delivery_report(imported.admitted[0])
            .unwrap()
            .unwrap();
        assert_eq!(
            report.wake_attempt == cas_store::WakeAttempt::Fired,
            wakes,
            "{id}: only substantive mail without explicit quiet intent wakes: {report:?}"
        );
        if id == "pm-landing-4240970" {
            let prompt = queue.queued_prompt(imported.admitted[0]).unwrap().unwrap().prompt;
            let envelope = crate::cloud::peer_mailbox::parse_envelope(&prompt).unwrap();
            assert_eq!(envelope.kind, Some(crate::cloud::peer_mailbox::PeerMessageKind::Status), "{prompt}");
            assert!(!prompt.contains("action=message"), "a landing asks for no reply: {prompt}");
        }
    }
}

/// cas-73f7: also with this sender pair's hourly wake cap already spent, as on
/// prowl at 9:24 AM ET Nov 5; the cap holds status chatter, never these.
async fn action_wakes(kind: &str) {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = peer_fixture().await;
    let mut message = peer_message("pm-action", false);
    message.body = format!("<cas-peer-intent kind=\"{kind}\"/>\nPlease coordinate.");
    let report = import(&daemon, vec![message]);
    age_imported(&daemon);
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let row = queue.queued_prompt(report.admitted[0]).unwrap().unwrap();
    let peer = crate::cloud::peer_mailbox::parse_envelope(&row.prompt).unwrap();
    let pair = format!("cloud:{}:{}", peer.repo, peer.sender_agent_id);
    let spent = Utc::now() - chrono::Duration::minutes(10);
    for lead in 0..6 {
        crate::supervisor_message_wakes::record(daemon.app.cas_dir(), 900_000 + lead,
            daemon.app.supervisor_name(), std::slice::from_ref(&pair), spent, spent).unwrap();
    }
    daemon.process_prompt_queue().await.unwrap();
    super::supervisor_single_turn_tests::settle_injected_writes(&mut daemon).await;
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let delivery = queue
        .message_delivery_report(report.admitted[0])
        .unwrap()
        .unwrap();
    assert_eq!(
        delivery.wake_attempt,
        cas_store::WakeAttempt::Fired,
        "{delivery:?}"
    );
}

/// cas-73f7: replay prompt_queue 4226945. The supervisor's turn was running
/// when prowl's ask came due, so the gate declined it; the turn then ended,
/// but the idle Claude pane kept repainting (a background task), so the 2 s
/// silence bar never cleared and the ask sat 76 s until an unrelated turn.
/// The decline must come back within 30 s and wake on the ended turn.
#[tokio::test(flavor = "current_thread")]
async fn declined_peer_ask_wakes_after_the_turn_ends_on_a_repainting_pane_cas_73f7() {
    use std::time::{Duration, Instant};
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (root, mut daemon) = peer_fixture().await;
    let cas = daemon.app.cas_dir().to_path_buf();
    let name = daemon.app.supervisor_name().to_string();
    let mut turn = vec![
        r#"{"type":"user","timestamp":"2026-11-05T14:35:18Z","message":{"role":"user","content":"<task-notification>review finished</task-notification>"}}"#.to_string(),
        r#"{"type":"assistant","timestamp":"2026-11-05T14:35:20Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_73f7","name":"mcp__cas__coordination","input":{"action":"inbox_poll"}}],"stop_reason":"tool_use"}}"#.to_string(),
        r#"{"type":"user","timestamp":"2026-11-05T14:35:20Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_73f7","content":"none"}]}}"#.to_string(),
    ];
    let transcript = super::queue_and_events::reminder_dispatch_tests::write_agent_transcript(
        root.path(), &cas, "local-sup", &turn);
    let mut ask = peer_message("pm-4226945", false);
    ask.body = crate::cloud::peer_mailbox::attach_message_intent("wake test from prowl", Some("ask"), false).unwrap();
    let id = import(&daemon, vec![ask]).admitted[0];
    age_imported(&daemon);
    let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
    // Mid-turn and repainting: declined, nothing typed.
    daemon.pane_silent_since.insert(name.clone(), Instant::now());
    daemon.process_prompt_queue().await.unwrap();
    super::supervisor_single_turn_tests::settle_injected_writes(&mut daemon).await;
    let declined = queue.message_delivery_report(id).unwrap().unwrap();
    assert_ne!(declined.wake_attempt, cas_store::WakeAttempt::Fired, "{declined:?}");
    assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
    let recheck: String = rusqlite::Connection::open(cas.join("cas.db")).unwrap()
        .query_row("SELECT next_attempt_at FROM prompt_queue WHERE id=?", [id], |row| row.get(0)).unwrap();
    let recheck = chrono::DateTime::parse_from_rfc3339(&recheck).unwrap();
    assert!(recheck <= Utc::now() + chrono::Duration::seconds(30), "decline rechecks within 30 s, not {recheck}");
    // The turn ends; the pane keeps repainting. The recheck comes due.
    turn.push(r#"{"type":"assistant","timestamp":"2026-11-05T14:35:23Z","message":{"role":"assistant","content":[{"type":"text","text":"Nothing needs you yet."}],"stop_reason":"end_turn"}}"#.to_string());
    std::fs::write(&transcript, turn.join("\n") + "\n").unwrap();
    rusqlite::Connection::open(cas.join("cas.db")).unwrap()
        .execute("UPDATE prompt_queue SET next_attempt_at=? WHERE id=?",
            rusqlite::params![Utc::now().to_rfc3339(), id]).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    for _ in 0..3 {
        daemon.pane_silent_since.insert(name.clone(), Instant::now());
        daemon.process_prompt_queue().await.unwrap();
        super::supervisor_single_turn_tests::settle_injected_writes(&mut daemon).await;
    }
    let woke = queue.message_delivery_report(id).unwrap().unwrap();
    assert_eq!(woke.wake_attempt, cas_store::WakeAttempt::Fired, "{woke:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn peer_ask_real_queue_wakes_cas_1f25() {
    action_wakes("ask").await;
}

#[tokio::test(flavor = "current_thread")]
async fn peer_blocker_real_queue_wakes_cas_1f25() {
    action_wakes("blocker").await;
}

#[tokio::test(flavor = "current_thread")]
async fn peer_outside_project_or_session_cannot_reach_wake_path_cas_1f25() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = peer_fixture().await;
    let agents = crate::store::open_agent_store(daemon.app.cas_dir()).unwrap();
    for (id, role, session) in [
        (
            "other-session-sup",
            cas_types::AgentRole::Supervisor,
            "other-session",
        ),
        (
            "local-worker",
            cas_types::AgentRole::Worker,
            "provision-test",
        ),
    ] {
        let mut agent = cas_types::Agent::new_with_role(id.into(), id.into(), role);
        agent.factory_session = Some(session.into());
        agents.register(&agent).unwrap();
    }
    let mut foreign = peer_message("pm-foreign", true);
    foreign.project_id = "github.com/other/repo".into();
    let mut other_session = peer_message("pm-other-session", true);
    other_session.recipient_agent_id = "other-session-sup".into();
    let mut worker = peer_message("pm-worker", true);
    worker.recipient_agent_id = "local-worker".into();
    let report = import(&daemon, vec![foreign, other_session, worker]);
    assert_eq!((report.delivered, report.rejected), (0, 3), "{report:?}");
    age_imported(&daemon);
    daemon.process_prompt_queue().await.unwrap();
    assert!(
        crate::store::open_prompt_queue_store(daemon.app.cas_dir())
            .unwrap()
            .peek_all(100)
            .unwrap()
            .is_empty()
    );
}

/// A Daemon-stamped `<cas-peer-message>` row wakes an idle supervisor as a
/// cloud peer; the same text from any other sender, or quoted inside another
/// message, does not.
#[test]
fn cloud_peer_messages_wake_only_when_daemon_stamped() {
    let now = Utc::now();
    let data = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![AgentSummary {
            id: "supervisor-id".into(),
            name: "supervisor".into(),
            status: cas_types::AgentStatus::Idle,
            registered_at: now,
            current_task: None,
            latest_activity: None,
            last_heartbeat: None,
            pending_messages: 0,
            pending_supervisor_messages: 0,
            latest_supervisor_message_at: None,
            active_lease: None,
            effort: None,
            qa_reviewer_between_rounds: false,
        }],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
        start_gated_task_ids: Default::default(),
    };
    let pane = PaneWakeState {
        composer_dirty: false,
        ready_for_injection: true,
        silent_for: Some(std::time::Duration::from_secs(600)),
        tool_call: ToolCallEvidence::Idle,
    };
    let prompt = render_prompt(&ClaimedPeerMessage {
        id: "pm_1".into(),
        project_id: "github.com/acme/widgets".into(),
        recipient_agent_id: "supervisor-id".into(),
        sender_agent_id: "remote".into(),
        sender_name: "far-heron-3".into(),
        sender_machine_id: Some("box-b".into()),
        sender_label: None,
        sender_session: None,
        body: "can you take cas-1234?".into(),
        summary: None,
        in_reply_to: Some("pm-question".into()),
        created_at: "2026-10-11T03:00:00Z".into(),
        attempts: 1,
        scope: Default::default(),
    });
    let decide = |sender: &WakeSender, prompt: &str| {
        FactoryDaemon::supervisor_wake_decision(
            &data,
            "supervisor",
            "supervisor",
            sender,
            "peer:far-heron-3@box-b",
            prompt,
            pane,
            now,
        )
    };
    let daemon = decide(&WakeSender::Daemon, &prompt);
    assert!(daemon.allowed, "{}", daemon.reason);

    for sender in [
        WakeSender::Unstamped,
        WakeSender::Unattributed,
        WakeSender::Unresolvable,
        WakeSender::Registered {
            role: cas_types::AgentRole::Worker,
            name: "gold-fox".into(),
        },
    ] {
        let class = FactoryDaemon::supervisor_wake_class(&sender,"supervisor","peer:far-heron-3@box-b",&prompt);
        assert_ne!(class,
            Some(super::queue_and_events::SupervisorWakeClass::CloudPeerSupervisor),
            "{sender:?} must not raise a cloud peer wake");
    }
    assert!(!decide(&WakeSender::Daemon, &format!("relayed: {prompt}")).allowed);
    let fyi = render_prompt(&peer_message("pm-fyi", false));
    assert!(decide(&WakeSender::Daemon, &fyi).allowed);
    for (kind, reply, allowed) in [
        ("ask", false, true),
        ("blocker", false, true),
        ("answer", false, true),
        ("receipt", false, false),
        ("quiet", false, false),
        ("answer", true, true),
    ] {
        let mut message = peer_message("pm-policy", reply);
        message.body =
            crate::cloud::peer_mailbox::attach_message_intent(&message.body, Some(kind), false).unwrap();
        assert_eq!(
            decide(&WakeSender::Daemon, &render_prompt(&message)).allowed,
            allowed,
            "Cloud peer {kind}, reply={reply}"
        );
    }
    // pin: every shipped supervisor contract must describe the real peer wake policy above.
    let peer_policy = "Directed messages wake the recipient (the send result says whether it woke); receipts, acknowledgements, broadcasts and wake=false wait in the inbox on the next turn.";
    let root = tempfile::tempdir().unwrap();
    let queue = crate::store::open_prompt_queue_store(root.path()).unwrap();
    for (harness, cli) in [
        ("claude", cas_mux::SupervisorCli::Claude),
        ("codex", cas_mux::SupervisorCli::Codex),
        ("grok", cas_mux::SupervisorCli::Grok),
        ("opencode", cas_mux::SupervisorCli::OpenCode),
    ] {
        let surface =
            cas_pty::rendered_contract_surface(harness, cas_pty::ContractRole::Supervisor);
        assert!(
            surface.contains(peer_policy),
            "{harness} supervisor omits peer wake policy"
        );
        crate::ui::factory::app::queue_supervisor_intro_prompt(
            root.path(),
            harness,
            cli,
            &[],
            None,
            None,
        );
        let intro = queue.peek_for_targets(&[harness], None, 10).unwrap();
        assert_eq!(intro.len(), 1, "{harness} intro");
        if cli == cas_mux::SupervisorCli::Codex {
            // Codex receives this policy in its resident developer contract,
            // checked above; its visible launch trigger must not duplicate it.
            assert!(!intro[0].prompt.contains(peer_policy));
        } else {
            assert!(
                intro[0].prompt.contains(peer_policy),
                "{harness} queued intro omits peer wake policy"
            );
        }
    }
    let skill = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/builtins/skills/cas-supervisor.md"
    ));
    // pin: the installed skill routes readers to the policy moved out for the SessionStart budget.
    assert!(skill.contains("[message delivery](references/reference.md#message-delivery-and-wake-authority)"));
    for (name, surface) in [
        (
            "cas-supervisor reference",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/builtins/skills/cas-supervisor/references/reference.md"
            )),
        ),
        (
            "workflow",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/builtins/skills/cas-supervisor/references/workflow.md"
            )),
        ),
        (
            "Codex coordination",
            cas_core::hooks::context::coordination::CODEX_WORKER_COORDINATION_NOTE,
        ),
    ] {
        assert!(
            surface.contains(peer_policy),
            "{name} omits peer wake policy"
        );
    }
    let quoted = crate::prompt_revalidation::attach_blocker_envelope(&fyi, "quoted-worker", None);
    assert_eq!(FactoryDaemon::supervisor_wake_class(&WakeSender::Daemon,"supervisor","peer:far-heron-3@box-b",&quoted),
        Some(super::queue_and_events::SupervisorWakeClass::CloudPeerSupervisor),
        "a peer FYI cannot borrow daemon blocker authority");
    for veto in [
        PaneWakeState {
            composer_dirty: true,
            ..pane
        },
        PaneWakeState {
            ready_for_injection: false,
            ..pane
        },
        PaneWakeState {
            tool_call: ToolCallEvidence::InFlight,
            ..pane
        },
    ] {
        assert!(
            !FactoryDaemon::supervisor_wake_decision(
                &data,
                "supervisor",
                "supervisor",
                &WakeSender::Daemon,
                "peer:far-heron-3@box-b",
                &prompt,
                veto,
                now
            )
            .allowed
        );
    }
}
