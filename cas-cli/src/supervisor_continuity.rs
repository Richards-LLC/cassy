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
        if matches!(key.as_str(), "notes" | "retired_lease")
            || value.is_null()
            || value.as_str() == Some("")
        {
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

// Keep native Codex's visible handoff readable. The immutable JSON receipt
// retains every field, including long notes, branches and earlier receipts.
fn codex_handoff_row(row: &serde_json::Value) -> String {
    fn text(value: &str, limit: usize) -> String {
        let clean = value.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut result: String = clean.chars().filter(|c| !c.is_control()).take(limit).collect();
        if clean.chars().count() > limit {
            result.push_str("...");
        }
        result
    }
    let field = |key: &str| row[key].as_str().unwrap_or_default();
    if !field("title").is_empty() {
        let mut line = format!("- {} ({})\n  {}", text(field("title"), 60), field("id"), field("status"));
        if !field("assignee").is_empty() {
            line.push_str(&format!("; assigned to {}", text(field("assignee"), 40)));
        }
        line.push('\n');
        if let Some(branch) = row["deliverables"]["parked_branch"].as_str() {
            line.push_str(&format!("  Parked: {}\n", text(branch, 68)));
        }
        if !field("external_ref").is_empty() {
            line.push_str(&format!("  Review: {}\n", text(field("external_ref"), 68)));
        }
        line
    } else if !field("name").is_empty() {
        format!("- {}: {}\n", text(field("name"), 45), text(field("status"), 20))
    } else {
        let message = ["prompt", "message", "event_type"].into_iter()
            .find_map(|key| row[key].as_str()).unwrap_or("Queued decision; see receipt");
        let summary = if let Some(status) = message.lines().next().unwrap_or_default().strip_prefix("status:") {
            format!("{}: delivery {}", field("source"), status.trim())
        } else {
            message.lines().next().unwrap_or(message).to_string()
        };
        format!("- {}\n", text(&summary, 72))
    }
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
    let claimed = store.claim_supervisor_handoffs(successor)?;
    // cas-d85c3: a phone message the predecessor never read belongs to the
    // claiming successor. Re-check every claimed handoff, not just new ones,
    // so a message left behind before this ran still moves; a moved row no
    // longer matches, so repeats are no-ops.
    let queue = crate::store::open_prompt_queue_store(root)?;
    let mut rerouted = std::collections::HashMap::new();
    for predecessor in store.supervisor_handoffs_claimed_by(successor)? {
        let moved = queue.retarget_operator_rows_to_successor(
            &predecessor.name,
            predecessor.factory_session.as_deref(),
            &successor.name,
            successor.factory_session.as_deref(),
        )?;
        rerouted.insert(predecessor.id, moved);
    }
    for handoff in claimed {
        let dir = root.join("supervisor-handoffs");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", handoff.id));
        let temporary = tempfile::NamedTempFile::new_in(&dir)?;
        serde_json::to_writer_pretty(temporary.as_file(), &handoff)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&path)?;
        #[cfg(unix)]
        std::fs::File::open(&dir)?.sync_all()?;
        let codex = cas_core::env_overlay::var("CAS_HOOK_HARNESS")
            .map(|h| h.eq_ignore_ascii_case("codex"))
            .unwrap_or_else(|_| crate::harness_policy::own_harness_from_env() == cas_mux::SupervisorCli::Codex);
        let mut detail = if codex {
            format!(
                "Supervisor handoff from {}: {}.\n{} tasks, {} worker records, {} pending records.\nReceipt: {}\n",
                handoff.name, handoff.reason, handoff.tasks.len(),
                handoff.workers.len(), handoff.pending.len(), path.display()
            )
        } else {
            format!(
                "Supervisor handoff from {} ({}), end reason: {}.\nReceipt: {}\n",
                handoff.name, handoff.agent_id, handoff.reason, path.display()
            )
        };
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
            ordered.sort_by_key(|row| {
                if codex && row["name"].is_string() {
                    !matches!(row["status"].as_str(), Some("active" | "idle"))
                } else {
                    row["deliverables"]["parked_branch"].is_null()
                }
            });
            for (index, row) in ordered.iter().enumerate() {
                let line = if let Some(inherited) = row["inherited_handoff"].as_str() {
                    format!(
                        "- Earlier receipt: {}\n",
                        dir.join(format!("{inherited}.json")).display()
                    )
                } else if codex {
                    codex_handoff_row(row)
                } else {
                    format!("- {}\n", serde_json::to_string(&preview(row))?)
                };
                // Reserve startup space for all three groups and the exact
                // receipt path. Long timelines and large fleets stay in JSON.
                if used + line.len() > 650 || (codex && index >= 2) {
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
        if let Some(moved) = rerouted.get(&handoff.id).filter(|ids| !ids.is_empty()) {
            let ids: Vec<String> = moved.iter().map(|id| format!("#{id}")).collect();
            detail.push_str(&format!(
                "Unread operator messages re-routed to you: {}. Answer them with target=operator.\n",
                ids.join(", ")
            ));
        }
        detail.push_str("Review the receipt and explicitly adopt owned epics. Worker rows are retirement-time evidence; verify current liveness before recovery.\n");
        queue.enqueue_idempotent(
            SOURCE,
            &successor.name,
            &detail,
            successor.factory_session.as_deref(),
            Some(SOURCE),
            Some(NotificationPriority::High),
            &format!(
                "supervisor-handoff:{}:{}:{}",
                handoff.id,
                successor.id,
                successor.registered_at.to_rfc3339()
            ),
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
                "- {}: {}\n  Handoff: {}\n",
                row.name, row.reason, row.id
            ));
        }
        Ok(text)
    })();
    result.unwrap_or_else(|error| format!("\nSession-end history unavailable: {error}\n"))
}
