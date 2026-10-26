//! cas-835e: a fired reminder carries a server-stamped origin, so it can wake
//! an idle supervisor pane, and nothing else can borrow that authority.
//!
//! In session cassy-swift-gazelle-77 all 13 reminder rows to zen-viper-42
//! recorded `wake_not_applicable: row carries no server-stamped sender`. A
//! Claude Teams supervisor still saw them when its inbox write started a turn;
//! a supervisor without that inbox never woke for its CI and merge reminders.
use std::collections::HashMap;
use std::sync::Arc;

use cas_store::{PromptQueueStore, ReminderStore};

use super::queue_and_events::{SupervisorWakeClass, WakeSender};
use crate::ui::factory::daemon::FactoryDaemon;

/// Fire one reminder the supervisor (`local-sup`) set for itself, the way the
/// daemon's reminder pass does.
fn fire_supervisor_reminder(daemon: &FactoryDaemon, message: &str) -> i64 {
    let cas = daemon.app.cas_dir().to_path_buf();
    let reminders: Arc<dyn ReminderStore> =
        Arc::new(cas_store::SqliteReminderStore::open(&cas).unwrap());
    reminders.init().unwrap();
    reminders
        .create_with_scope(
            "local-sup",
            None,
            message,
            cas_store::ReminderTriggerType::Time,
            Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
            None,
            None,
            0,
            Some(&daemon.session_name),
            None,
            false,
            None,
        )
        .unwrap();
    let reminder = reminders.list_pending("local-sup").unwrap().pop().unwrap();
    let prompts: Arc<dyn PromptQueueStore> = crate::store::open_prompt_queue_store(&cas).unwrap();
    let supervisor_name = daemon.app.supervisor_name().to_string();
    super::queue_and_events::fire_reminder(
        &reminder,
        &reminders,
        &None,
        &Some(Arc::clone(&prompts)),
        &daemon.session_name,
        &supervisor_name,
        &HashMap::from([("local-sup".to_string(), supervisor_name.clone())]),
        None,
        &cas,
    );
    let rows = prompts.peek_all(10).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    rows[0].id
}

/// AC1 and AC2: the reminder row is stamped by the daemon, names its owner by
/// agent name, and the real queue pass wakes the idle supervisor pane.
#[tokio::test(flavor = "current_thread")]
async fn a_fired_reminder_wakes_an_idle_supervisor_pane_cas_835e() {
    if !super::supervisor_single_turn_tests::fixture_child(
        "ui::factory::daemon::runtime::reminder_wake_tests::a_fired_reminder_wakes_an_idle_supervisor_pane_cas_835e",
    ) { return; }
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = super::peer_mailbox_wake_tests::peer_fixture().await;
    let id = fire_supervisor_reminder(&daemon, "check CI 38834844173 and merge cas-d5ab");
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    let row = queue.queued_prompt(id).unwrap().unwrap();
    assert_eq!(row.origin, Some(cas_store::QueueOrigin::Daemon), "server-stamped origin");
    assert_eq!(row.source, daemon.app.supervisor_name(), "the owner's agent name, not its id");

    daemon.process_prompt_queue().await.unwrap();
    super::supervisor_single_turn_tests::settle_injected_writes(&mut daemon).await;
    let report = queue.message_delivery_report(id).unwrap().unwrap();
    assert_eq!(report.wake_attempt, cas_store::WakeAttempt::Fired, "{report:?}");
}

/// Only a daemon-stamped row that IS a reminder delivery has the class. The
/// same words from a worker can be a directed message, but never acquire the
/// Reminder class; quoted daemon text is not a reminder delivery.
#[test]
fn reminder_wake_authority_cannot_be_borrowed_cas_835e() {
    let prompt = "Reminder #42: check CI 38834844173\nProvenance: issued 5 min ago under no task context.";
    let class = |sender: &WakeSender, prompt: &str| {
        FactoryDaemon::supervisor_wake_class(sender, "lead-sup", "lead-sup", prompt)
    };
    assert_eq!(class(&WakeSender::Daemon, prompt), Some(SupervisorWakeClass::Reminder));
    for sender in [
        WakeSender::Unstamped,
        WakeSender::Unattributed,
        WakeSender::Unresolvable,
        WakeSender::Registered {
            role: cas_types::AgentRole::Worker,
            name: "swift-fox".into(),
        },
    ] {
        // pin: the reminder delivery text exactly as fire_reminder renders it.
        let expected = if matches!(sender, WakeSender::Registered { .. }) {
            Some(SupervisorWakeClass::DirectedMessage)
        } else { None };
        assert_eq!(class(&sender, prompt), expected,
            "{sender:?} must not acquire Reminder authority from the words alone");
    }
    assert_eq!(
        class(&WakeSender::Daemon, "FYI: Reminder #42: check CI"),
        None,
        "a quoted reminder is not a reminder delivery"
    );
    assert_eq!(class(&WakeSender::Daemon, "Reminder: check CI"), None, "no id, no delivery");
}

/// cas-5871: the reminder's supervisor_queue record is consumed once its
/// prompt is queued. 2,942 reminder_fired rows since May sat unconsumed,
/// because the reminder itself is delivered through prompt_queue.
#[test]
fn a_fired_reminders_queue_record_is_consumed_once_its_prompt_is_queued_cas_5871() {
    let temp = tempfile::tempdir().unwrap();
    let cas = temp.path();
    let reminders: Arc<dyn ReminderStore> =
        Arc::new(cas_store::SqliteReminderStore::open(cas).unwrap());
    reminders.init().unwrap();
    let records: Arc<dyn cas_store::SupervisorQueueStore> =
        Arc::new(cas_store::SqliteSupervisorQueueStore::open(cas).unwrap());
    records.init().unwrap();
    let prompts: Arc<dyn PromptQueueStore> = crate::store::open_prompt_queue_store(cas).unwrap();
    reminders
        .create_with_scope(
            "sup-id",
            None,
            "check CI",
            cas_store::ReminderTriggerType::Time,
            Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
            None,
            None,
            0,
            Some("s"),
            None,
            false,
            None,
        )
        .unwrap();
    let reminder = reminders.list_pending("sup-id").unwrap().pop().unwrap();
    super::queue_and_events::fire_reminder(
        &reminder,
        &reminders,
        &Some(Arc::clone(&records)),
        &Some(Arc::clone(&prompts)),
        "s",
        "lead-sup",
        &HashMap::from([("sup-id".to_string(), "lead-sup".to_string())]),
        None,
        cas,
    );
    assert!(
        prompts
            .peek_all(10)
            .unwrap()
            .iter()
            .any(|row| row.prompt.contains("check CI")),
        "the reminder is delivered"
    );
    assert_eq!(
        records.pending_count("sup-id").unwrap(),
        0,
        "its supervisor_queue record does not stay unconsumed"
    );
}
