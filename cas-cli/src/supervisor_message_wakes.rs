//! Directed supervisor messages share the prompt queue's selection, claims and PTY delivery.
use std::{collections::HashMap, path::Path};
use chrono::{DateTime, Utc};
use cas_store::{PromptQueueStore, QueuedPrompt};
use crate::ui::factory::daemon::runtime::queue_and_events::RowFacts;

pub(crate) fn schedule(
    _cas: &Path, _session: &str, _supervisor: &str, _agents: &[cas_types::Agent],
    _rows: &mut Vec<QueuedPrompt>, _facts: &mut HashMap<i64, RowFacts>, _now: DateTime<Utc>,
) -> anyhow::Result<()> { Ok(()) }

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::QueueOrigin;
    use crate::ui::factory::daemon::runtime::queue_and_events::run_queue_select;
    use crate::ui::factory::daemon::runtime::prompt_queue_jobs::QueueSelectRequest;

    fn setup() -> (tempfile::TempDir, std::path::PathBuf, cas_types::Agent) {
        let root = tempfile::tempdir().unwrap();
        let cas = crate::store::init_cas_dir(root.path()).unwrap();
        let mut peer = cas_types::Agent::new_with_role("peer-id".into(), "peer".into(), cas_types::AgentRole::Supervisor);
        peer.factory_session = Some("other-session".into());
        crate::store::open_agent_store(&cas).unwrap().register(&peer).unwrap();
        (root, cas, peer)
    }
    fn enqueue(cas: &Path, peer: &cas_types::Agent, body: &str) -> i64 {
        crate::store::open_prompt_queue_store(cas).unwrap().enqueue_urgent_with_outcome(
            &peer.name, "supervisor", body, Some("wake-session"), Some("field report"),
            Some(cas_store::NotificationPriority::Normal), false,
            Some(&QueueOrigin::RegisteredAgent { agent_id: peer.id.clone() }),
        ).unwrap().id()
    }
    fn select(cas: &Path) -> crate::ui::factory::daemon::runtime::prompt_queue_jobs::QueueSelection {
        run_queue_select(&QueueSelectRequest {
            cas_dir: cas.into(), session_name: "wake-session".into(), supervisor_name: "wake-sup".into(),
            worker_names: vec![], dead_workers: Default::default(), limit: 10,
            operator_replies: false, deferred_operator_replies: Default::default(), adopt_operator_notices: false,
        }, 1).unwrap()
    }
    fn age(cas: &Path) {
        rusqlite::Connection::open(cas.join("cas.db")).unwrap().execute(
            "UPDATE prompt_queue SET created_at=?1", [(Utc::now()-chrono::Duration::seconds(21)).to_rfc3339()],
        ).unwrap();
    }
    #[test]
    fn cas_1e7e2_directed_burst_debounces_into_one_body_batch() {
        let (_root, cas, peer) = setup();
        let ids: Vec<_> = ["First field report", "Second field report", "Third field report"]
            .into_iter().map(|body| enqueue(&cas, &peer, body)).collect();
        assert!(select(&cas).rows.is_empty(), "a fresh burst waits for the debounce deadline");
        age(&cas);
        let selected = select(&cas);
        assert_eq!(selected.rows.iter().map(|row| row.id).collect::<Vec<_>>(), vec![ids[0]]);
        assert_eq!(selected.facts[&ids[0]].message_batch.iter().map(|row| row.id).collect::<Vec<_>>(), ids[1..]);
        assert_eq!(selected.facts[&ids[0]].message_senders, vec![peer.id]);
    }
    #[test]
    fn cas_1e7e2_acknowledgements_wait_readable_for_the_next_turn() {
        let (_root, cas, peer) = setup();
        let ack = enqueue(&cas, &peer, "thanks, received");
        let receipt = enqueue(&cas, &peer, "<cas-message-quiet/>\nAcknowledged.");
        age(&cas);
        assert!(select(&cas).rows.is_empty(), "acknowledgements must never start a turn");
        let queue = crate::store::open_prompt_queue_store(&cas).unwrap();
        let readable = queue.surface_unseen_for_recipient("supervisor", Some("wake-session"), 20).unwrap();
        assert_eq!(readable.iter().map(|row|row.id).collect::<Vec<_>>(), vec![ack, receipt]);
    }
}
