//! Durable supervisor succession. Registry liveness and lane history are separate:
//! a successor receives evidence; workers keep their identities and task leases.
use std::path::Path;

use cas_store::{AgentStore, NotificationPriority, QueueOrigin, SqliteAgentStore};
use cas_types::{Agent, AgentRole};

const SOURCE: &str = "supervisor-handoff";

fn preview(value: &serde_json::Value) -> serde_json::Value {
    let Some(fields) = value.as_object() else {
        return value.clone();
    };
    let mut result = serde_json::Map::new();
    for (key, value) in fields {
        if key == "notes" || value.is_null() || value.as_str() == Some("") {
            continue;
        }
        if value.as_object().is_some_and(|v| v.is_empty())
            || value.as_array().is_some_and(|v| v.is_empty())
        {
            continue;
        }
        let projected = if key == "deliverables" {
            let mut delivery = serde_json::Map::new();
            for key in ["parked_branch", "factory_branch_anchor"] {
                if let Some(value) = value.get(key) {
                    delivery.insert(key.into(), value.clone());
                }
            }
            serde_json::Value::Object(delivery)
        } else if let Some(text) = value.as_str() {
            let limit = if matches!(key.as_str(), "prompt" | "payload" | "message") {
                160
            } else {
                100
            };
            let mut brief: String = text.chars().take(limit).collect();
            if text.chars().count() > limit {
                brief.push_str("...");
            }
            serde_json::Value::String(brief)
        } else {
            value.clone()
        };
        result.insert(key.clone(), projected);
    }
    serde_json::Value::Object(result)
}

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
        #[cfg(unix)]
        std::fs::File::open(&dir)?.sync_all()?;
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
            let mut used = 0;
            let mut ordered: Vec<_> = rows.iter().collect();
            ordered.sort_by_key(|row| row["deliverables"]["parked_branch"].is_null());
            for (index, row) in ordered.iter().enumerate() {
                let line = if let Some(inherited) = row["inherited_handoff"].as_str() {
                    format!(
                        "- Earlier receipt: {}\n",
                        dir.join(format!("{inherited}.json")).display()
                    )
                } else {
                    format!("- {}\n", serde_json::to_string(&preview(row))?)
                };
                // Reserve startup space for all three groups and the exact
                // receipt path. Long timelines and large fleets stay in JSON.
                if used + line.len() > 650 {
                    detail.push_str(&format!(
                        "- {} additional row(s): see receipt.\n",
                        rows.len() - index
                    ));
                    break;
                }
                used += line.len();
                detail.push_str(&line);
            }
        }
        detail.push_str("Review the receipt and explicitly adopt owned epics. Worker rows are retirement-time evidence; verify current liveness before recovery.\n");
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
            1,
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
