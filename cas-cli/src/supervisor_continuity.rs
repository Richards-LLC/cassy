//! Durable supervisor succession. Registry liveness and lane history are separate:
//! a successor receives evidence; workers keep their identities and task leases.
use std::path::Path;

use cas_store::{AgentStore, NotificationPriority, QueueOrigin, SqliteAgentStore};
use cas_types::{Agent, AgentRole};

const SOURCE: &str = "supervisor-handoff";

pub(crate) fn record_end(root: &Path, id: &str, reason: Option<&str>) -> anyhow::Result<()> {
    let store = SqliteAgentStore::open(root)?;
    store.init()?;
    store.record_session_end(id, reason)?;
    Ok(())
}

/// Claim first, then publish to the durable inbox before marking the handoff
/// queued. Interrupted publication retries with the same prompt idempotency key.
pub(crate) fn resume(root: &Path, successor: &Agent) -> anyhow::Result<()> {
    if successor.role != AgentRole::Supervisor {
        return Ok(());
    }
    let store = SqliteAgentStore::open(root)?;
    store.init()?;
    for handoff in store.claim_supervisor_handoffs(successor)? {
        let dir = root.join("supervisor-handoffs");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", handoff.id));
        let temporary = tempfile::NamedTempFile::new_in(&dir)?;
        serde_json::to_writer_pretty(temporary.as_file(), &handoff)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&path)?;
        let mut detail = format!(
            "Supervisor handoff from {} ({}), end reason: {}.\nReceipt: {}\n",
            handoff.name,
            handoff.agent_id,
            handoff.reason,
            path.display()
        );
        // The complete immutable receipt remains readable when a large lane
        // exceeds startup's context budget. Each projection has an explicit count.
        for (label, rows) in [
            ("Tasks and parked deliveries", &handoff.tasks),
            ("Workers (original session retained)", &handoff.workers),
            ("Pending asks and blockers", &handoff.pending),
        ] {
            detail.push_str(&format!("{label} ({}):\n", rows.len()));
            for row in rows.iter().take(12) {
                let line = serde_json::to_string(row)?;
                let preview: String = line.chars().take(600).collect();
                detail.push_str(&format!(
                    "- {preview}{}\n",
                    if line.chars().count() > 600 {
                        " … see receipt"
                    } else {
                        ""
                    }
                ));
            }
            if rows.len() > 12 {
                detail.push_str("- Additional rows in receipt.\n");
            }
        }
        detail.push_str("Review the receipt and explicitly adopt owned epics; live workers and leases were preserved.\n");
        crate::store::open_prompt_queue_store(root)?.enqueue_idempotent(
            SOURCE,
            &successor.name,
            &detail,
            successor.factory_session.as_deref(),
            Some(SOURCE),
            Some(NotificationPriority::High),
            &format!("supervisor-handoff:{}", handoff.id),
            Some(&QueueOrigin::Daemon),
        )?;
        store.mark_supervisor_handoff_queued(&handoff.id, successor)?;
    }
    Ok(())
}

pub(crate) fn startup_context(root: &Path, id: &str) -> anyhow::Result<String> {
    let agents = crate::store::open_agent_store(root)?;
    let agent = agents.get(id)?;
    resume(root, &agent)?;
    let messages = crate::store::open_prompt_queue_store(root)?
        .surface_unseen_from_sources_for_recipient(
            &agent.name,
            agent.factory_session.as_deref(),
            &[SOURCE],
            20,
        )?;
    Ok(messages
        .into_iter()
        .map(|row| row.prompt)
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Local-only history for worker_status and peers, independent of Cloud.
pub(crate) fn status_section(root: &Path) -> String {
    let result = (|| -> anyhow::Result<String> {
        let store = SqliteAgentStore::open(root)?;
        let rows = store.recent_session_ends(&Agent::get_or_generate_machine_id(), 5)?;
        if rows.is_empty() {
            return Ok(String::new());
        }
        let mut text = "\nRecent supervisor session ends:\n".to_string();
        for row in rows {
            text.push_str(&format!(
                "- {} ({}): {}; handoff {}\n",
                row.name, row.agent_id, row.reason, row.id
            ));
        }
        Ok(text)
    })();
    result.unwrap_or_else(|error| format!("\nSession-end history unavailable: {error}\n"))
}
