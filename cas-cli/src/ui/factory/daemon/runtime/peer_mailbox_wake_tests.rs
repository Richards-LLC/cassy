//! cas-f9c7: the wake gate for cloud-relayed peer supervisor messages.
use std::collections::HashMap;

use chrono::Utc;

use super::queue_and_events::{PaneWakeState, ToolCallEvidence, WakeSender};
use crate::cloud::peer_mailbox::{ClaimedPeerMessage, render_prompt};
use crate::ui::factory::daemon::FactoryDaemon;
use crate::ui::factory::director::{AgentSummary, DirectorData};

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
        sender_session: None,
        body: "can you take cas-1234?".into(),
        summary: None,
        in_reply_to: None,
        created_at: "2026-10-11T03:00:00Z".into(),
        attempts: 1,
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
}
