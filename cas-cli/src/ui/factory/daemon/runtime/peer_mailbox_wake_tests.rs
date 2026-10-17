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
            args: vec!["-c".into(), "printf ready; exec /bin/sleep 60".into()],
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
    tokio::time::sleep(Duration::from_millis(5100)).await;
    for pane in daemon.app.mux.panes_mut() {
        pane.drain_output();
    }
    let bytes = daemon.app.mux.pane_bytes_received(&name).unwrap();
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
    let imported = import(
        &daemon,
        vec![
            peer_message("pm-reply-1", true),
            peer_message("pm-reply-2", true),
        ],
    );
    assert_eq!(imported.delivered, 2, "{imported:?}");
    daemon.process_prompt_queue().await.unwrap();
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
            .is_none(),
        "second reply waits for a later pass"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn peer_fyi_real_queue_stays_inbox_only_cas_1f25() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = peer_fixture().await;
    let imported = import(&daemon, vec![peer_message("pm-fyi", false)]);
    assert_eq!(imported.delivered, 1);
    daemon.process_prompt_queue().await.unwrap();
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let report = queue
        .message_delivery_report(imported.admitted[0])
        .unwrap()
        .unwrap();
    assert_eq!(
        report.wake_attempt,
        cas_store::WakeAttempt::NotAttempted,
        "FYI must stay quiet: {report:?}"
    );
}

async fn action_wakes(kind: &str) {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = peer_fixture().await;
    let mut message = peer_message("pm-action", false);
    message.body = format!("<cas-peer-intent kind=\"{kind}\"/>\nPlease coordinate.");
    let report = import(&daemon, vec![message]);
    daemon.process_prompt_queue().await.unwrap();
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
    assert!(daemon.reason.contains("peer mailbox"), "{}", daemon.reason);
    for sender in [
        WakeSender::Unstamped,
        WakeSender::Unattributed,
        WakeSender::Unresolvable,
        WakeSender::Registered {
            role: cas_types::AgentRole::Worker,
            name: "gold-fox".into(),
        },
    ] {
        assert!(
            !decide(&sender, &prompt).allowed,
            "{sender:?} must not raise a cloud peer wake"
        );
    }
    assert!(!decide(&WakeSender::Daemon, &format!("relayed: {prompt}")).allowed);
    let fyi = render_prompt(&peer_message("pm-fyi", false));
    assert!(!decide(&WakeSender::Daemon, &fyi).allowed);
    for (kind, reply, allowed) in [
        ("ask", false, true),
        ("blocker", false, true),
        ("answer", false, false),
        ("answer", true, true),
    ] {
        let mut message = peer_message("pm-policy", reply);
        message.body =
            crate::cloud::peer_mailbox::attach_message_intent(&message.body, Some(kind), false);
        assert_eq!(
            decide(&WakeSender::Daemon, &render_prompt(&message)).allowed,
            allowed,
            "Cloud peer {kind}, reply={reply}"
        );
    }
    // pin: every shipped supervisor contract must describe the real peer wake policy above.
    let peer_policy = "Authenticated local peer supervisors may wake each other. Via Cloud, kind=ask/blocker or replies with nonempty in_reply_to may wake; kind=answer alone and peer FYIs stay inbox-only. Busy panes, in-flight tools and drafts defer wakes.";
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
        assert!(
            intro[0].prompt.contains(peer_policy),
            "{harness} queued intro omits peer wake policy"
        );
    }
    for (name, surface) in [
        (
            "cas-supervisor",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/builtins/skills/cas-supervisor.md"
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
    assert!(
        !decide(&WakeSender::Daemon, &quoted).allowed,
        "a peer FYI cannot borrow daemon blocker authority"
    );
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
