//! Orphan recovery when a worker vanishes mid-task (cas-2e81).
//!
//! `mark_stale` / lease reclaim revoke leases but historically left the task
//! row as `InProgress` with no supervisor signal. This module parks eligible
//! tasks Open with an audit note, records a `WorkerDied` event, and queues a
//! critical `worker_died` notification for active supervisors.
//!
//! cas-3dcb (GH #168): that notification used to be `supervisor_queue`-only, so
//! 2,044 critical death alerts — 100% of them — never entered a supervisor turn
//! and deaths surfaced only if someone polled `worker_status`. It now follows
//! the cas-ecff outbox: durable row keyed by death incident, then a prompt-path
//! relay the daemon can inject, wake an idle pane with, and report as a lost
//! relay if it never lands. The incident key also bounds re-emission — the same
//! corpse re-detected on every maintenance tick used to yield a fresh critical
//! notice each time (1,452 for a single agent over ten days).

use std::path::Path;
use std::sync::Arc;

use cas_types::{Agent, AgentRole, AgentStatus, Event, EventEntityType, EventType, TaskStatus};
use chrono::Utc;

use crate::mcp::tools::core::task::lifecycle::supervisor_push::LIFECYCLE_WAKE_SOURCE_PREFIX;
use crate::store::{
    AgentStore, NotificationPriority, TaskStore, open_event_store, open_prompt_queue_store,
    open_supervisor_queue_store, open_task_store,
};
use cas_store::{NotifyIdempotentResult, PromptQueueStore, SupervisorQueueStore};

/// Maximum pending worker-death relays examined in one daemon pass.
///
/// This is deliberately much larger than the ordinary ten-row delivery batch:
/// duplicate registry cleanup must collapse before it can monopolize turns,
/// while the finite cap keeps one malformed queue from creating an unbounded
/// allocation on the hot path.
pub(crate) const WORKER_DEATH_COALESCE_SCAN_LIMIT: usize = 4_096;

/// Summary of a single recovery pass.
#[derive(Debug, Default, Clone)]
pub struct OrphanRecoverySummary {
    pub recovered_task_ids: Vec<String>,
    pub held_task_ids: Vec<String>,
}

/// Statuses that must NOT be auto-parked while work is already parked or done.
fn is_protected_status(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Closed
            | TaskStatus::Cancelled
            | TaskStatus::Open
            | TaskStatus::AwaitingMerge
    )
}

/// Park orphaned working tasks for a dead agent and emit supervisor signals.
///
/// `held_task_ids` should be the leases observed *before* `mark_stale` /
/// reclaim (those calls clear active leases). Also recovers InProgress/
/// Blocked tasks still assigned to the agent by name or id.
pub fn recover_worker_vanished(
    cas_root: &Path,
    agent_store: &dyn AgentStore,
    agent: &Agent,
    held_task_ids: &[String],
    reason: &str,
) -> OrphanRecoverySummary {
    let mut summary = OrphanRecoverySummary {
        held_task_ids: held_task_ids.to_vec(),
        ..Default::default()
    };

    let task_store = match open_task_store(cas_root) {
        Ok(s) => s,
        Err(_) => {
            emit_worker_died_signals(cas_root, agent_store, agent, &summary, reason);
            return summary;
        }
    };

    let mut candidate_ids: Vec<String> = held_task_ids.to_vec();

    // Also pick up every non-terminal task still assigned to this worker.
    // Shutdown/crash paths can lose the lease before recovery runs; the death
    // relay must still enumerate that work rather than claim "none".
    if let Ok(tasks) = task_store.list(None) {
        for t in tasks {
            if !matches!(t.status, TaskStatus::Closed | TaskStatus::Cancelled)
                && task_assigned_to_agent(&t.assignee, agent)
            {
                candidate_ids.push(t.id);
            }
        }
    }
    candidate_ids.sort();
    candidate_ids.dedup();
    summary.held_task_ids = candidate_ids.clone();

    for task_id in &candidate_ids {
        if park_orphaned_task(&task_store, task_id, agent, reason) {
            summary.recovered_task_ids.push(task_id.clone());
        }
    }

    emit_worker_died_signals(cas_root, agent_store, agent, &summary, reason);
    summary
}

/// Recover tasks whose leases just expired, but only when the holder is dead
/// or heartbeat-stale (worker vanished) — not when a live agent simply failed
/// to renew mid-turn.
pub fn recover_expired_leases_for_dead_holders(
    cas_root: &Path,
    agent_store: &dyn AgentStore,
    expired: &[(String, String)], // (task_id, agent_id)
    stale_threshold_secs: i64,
) -> Vec<OrphanRecoverySummary> {
    use std::collections::HashMap;

    let mut by_agent: HashMap<String, Vec<String>> = HashMap::new();
    for (task_id, agent_id) in expired {
        by_agent
            .entry(agent_id.clone())
            .or_default()
            .push(task_id.clone());
    }

    let mut out = Vec::new();
    for (agent_id, task_ids) in by_agent {
        let agent = match agent_store.get(&agent_id) {
            Ok(a) => a,
            Err(_) => continue,
        };
        if holder_is_alive(&agent, stale_threshold_secs) {
            continue;
        }
        let summary = recover_worker_vanished(
            cas_root,
            agent_store,
            &agent,
            &task_ids,
            "lease expired while holder gone",
        );
        out.push(summary);
    }
    out
}

fn holder_is_alive(agent: &Agent, stale_threshold_secs: i64) -> bool {
    if !matches!(agent.status, AgentStatus::Active | AgentStatus::Idle) {
        return false;
    }
    let elapsed = (Utc::now() - agent.last_heartbeat).num_seconds();
    elapsed <= stale_threshold_secs
}

fn task_assigned_to_agent(assignee: &Option<String>, agent: &Agent) -> bool {
    match assignee {
        Some(a) => a == &agent.name || a == &agent.id,
        None => false,
    }
}

/// Returns true if the task was parked to Open.
fn park_orphaned_task(
    task_store: &Arc<dyn TaskStore>,
    task_id: &str,
    agent: &Agent,
    reason: &str,
) -> bool {
    let mut task = match task_store.get(task_id) {
        Ok(t) => t,
        Err(_) => return false,
    };
    if is_protected_status(task.status) {
        return false;
    }

    let prior_status = task.status;
    let prior_assignee = task.assignee.clone();
    task.status = TaskStatus::Open;
    task.assignee = None;
    let ts = Utc::now().format("%Y-%m-%d %H:%M");
    let audit = format!(
        "[{ts}] ⚠ orphan recovery: worker vanished mid-task — parked {prior_status:?}→Open \
         (prior assignee: {}, worker: {} / {}, reason: {reason}). \
         Use `task action=reset` only if still stuck; task is now claimable.",
        prior_assignee.as_deref().unwrap_or("<none>"),
        agent.name,
        &agent.id[..8.min(agent.id.len())],
    );
    task.notes = if task.notes.is_empty() {
        audit
    } else {
        format!("{}\n\n{}", task.notes, audit)
    };
    task.updated_at = Utc::now();
    task_store.update(&task).is_ok()
}

fn emit_worker_died_signals(
    cas_root: &Path,
    agent_store: &dyn AgentStore,
    agent: &Agent,
    summary: &OrphanRecoverySummary,
    reason: &str,
) {
    // The maintenance callers operate on the generic agent registry.  A
    // supervisor row expiring after a restart is not a worker death and must
    // never create a self-directed critical relay (GH #678).
    if agent.role != AgentRole::Worker {
        return;
    }

    let held = summary.held_task_ids.join(",");
    let recovered = summary.recovered_task_ids.join(",");
    let payload = serde_json::json!({
        "worker_id": agent.id,
        "worker_name": agent.name,
        "held_tasks": summary.held_task_ids,
        "recovered_tasks": summary.recovered_task_ids,
        "reason": reason,
        "last_heartbeat": agent.last_heartbeat.to_rfc3339(),
        "factory_session": agent.factory_session,
    });

    // Activity feed event.
    if let Ok(event_store) = open_event_store(cas_root) {
        let summary_text = if summary.held_task_ids.is_empty() {
            format!("Worker {} died ({reason}); no held tasks", agent.name)
        } else {
            format!(
                "Worker {} died mid-task ({reason}); held=[{held}]; recovered=[{recovered}]",
                agent.name
            )
        };
        let event = Event::new(
            EventType::WorkerDied,
            EventEntityType::Agent,
            agent.id.clone(),
            summary_text,
        )
        .with_metadata(payload.clone())
        .with_session(agent.id.clone());
        let _ = event_store.record(&event);
    }

    // Supervisor queue — critical priority.
    let supervisors = match agent_store.list(None) {
        Ok(agents) => agents
            .into_iter()
            .filter(|a| {
                matches!(a.role, AgentRole::Supervisor | AgentRole::Director)
                    && matches!(a.status, AgentStatus::Active | AgentStatus::Idle)
                    && a.visible_to_factory_session(agent.factory_session.as_deref())
            })
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };

    if let Ok(queue) = open_supervisor_queue_store(cas_root) {
        let payload_str = payload.to_string();
        let prompt_queue = open_prompt_queue_store(cas_root).ok();
        let incident = death_incident_key(agent);

        let mut recipients: Vec<String> = supervisors.iter().map(|s| s.id.clone()).collect();
        // If no live supervisor rows, still notify parent_id when set.
        if recipients.is_empty() {
            if let Some(parent) = agent.parent_id.clone() {
                recipients.push(parent);
            }
        }

        for recipient in &recipients {
            deliver_worker_died_notice(
                queue.as_ref(),
                prompt_queue.as_deref(),
                recipient,
                agent,
                summary,
                reason,
                &incident,
                &payload_str,
            );
        }
    }
}

/// Identity of ONE death of ONE agent (cas-3dcb, GH #168).
///
/// `last_heartbeat` is the discriminator, and it is the right one: `mark_stale`
/// deliberately does not touch it, so every re-detection of the same corpse —
/// by daemon maintenance, `agent_cleanup`, or expired-lease recovery — computes
/// the same key and collapses onto the same notice. Only a genuine revive →
/// heartbeat → die-again cycle moves the heartbeat and earns a second notice.
/// This is what makes the reported 1,452-notices-for-one-agent class impossible
/// rather than merely unlikely.
fn death_incident_key(agent: &Agent) -> String {
    format!(
        "worker_died:{}:{}",
        agent.id,
        agent.last_heartbeat.timestamp_millis()
    )
}

/// Durable notice + prompt-path injection for one recipient (cas-3dcb).
///
/// Mirrors the cas-ecff task-lifecycle outbox exactly:
///   1. `notify_idempotent` under the death-incident key (durable, deduped)
///   2. if the prompt was not already handed off, `enqueue_idempotent` into
///      `prompt_queue` under the notification's own key
///   3. stamp `prompt_delivered_at`
///
/// Every step is replay-safe, so a crash between 2 and 3 costs at most a
/// repeated no-op enqueue — never a second prompt row.
#[allow(clippy::too_many_arguments)]
fn deliver_worker_died_notice(
    queue: &dyn SupervisorQueueStore,
    prompt_queue: Option<&dyn PromptQueueStore>,
    recipient: &str,
    agent: &Agent,
    summary: &OrphanRecoverySummary,
    reason: &str,
    incident: &str,
    payload_str: &str,
) {
    let transition_key = format!("{incident}:{recipient}");
    let (notification_id, prompt_already_delivered) = match queue.notify_idempotent(
        recipient,
        "worker_died",
        payload_str,
        NotificationPriority::Critical,
        &transition_key,
    ) {
        Ok(NotifyIdempotentResult::Created(id)) => (id, false),
        Ok(NotifyIdempotentResult::AlreadyExists {
            id,
            prompt_delivered,
        }) => (id, prompt_delivered),
        Err(error) => {
            tracing::error!(
                target: "cas::coordination",
                stage = "worker_died_durable_enqueue_failed",
                worker = %agent.name,
                recipient = %recipient,
                %error,
                "cas-3dcb: could not record a worker death for the supervisor"
            );
            return;
        }
    };

    if prompt_already_delivered {
        return;
    }

    let Some(prompt_queue) = prompt_queue else {
        tracing::error!(
            target: "cas::coordination",
            stage = "worker_died_prompt_queue_unavailable",
            worker = %agent.name,
            recipient = %recipient,
            notification_id,
            "cas-3dcb: worker death recorded but prompt_queue is unavailable; the durable row \
             is left unstamped so a later pass retries injection"
        );
        return;
    };

    let body = crate::prompt_revalidation::format_worker_died_relay(
        &agent.id,
        &agent.name,
        incident,
        reason,
        &summary.held_task_ids,
        &summary.recovered_task_ids,
        notification_id,
    );
    // `lifecycle-wake:` is what makes the daemon corroborate, wake an idle
    // supervisor pane, bound re-nudges, and surface the row in
    // `list_undelivered_lifecycle_relays` if it never lands. The dead worker's
    // own name must NOT be the source: `is_dead_worker_source` drops those.
    let source = format!(
        "{}worker-died:{notification_id}",
        LIFECYCLE_WAKE_SOURCE_PREFIX
    );
    let display = format!("worker died: {}", agent.name);

    if let Err(error) = prompt_queue.enqueue_idempotent(
        &source,
        "supervisor",
        &body,
        agent.factory_session.as_deref(),
        Some(&display),
        Some(NotificationPriority::Critical),
        &format!("worker-died-outbox:{notification_id}"),
        Some(&cas_store::QueueOrigin::Daemon),
    ) {
        tracing::error!(
            target: "cas::coordination",
            stage = "worker_died_prompt_enqueue_failed",
            worker = %agent.name,
            recipient = %recipient,
            notification_id,
            %error,
            "cas-3dcb: worker death recorded but could not be injected into the supervisor's \
             session; durable row left unstamped for retry"
        );
        return;
    }

    if let Err(error) = queue.mark_prompt_delivered(notification_id) {
        // The enqueue is idempotent under its dedupe key, so an unstamped row
        // costs a repeated no-op on the next pass, not a duplicate prompt.
        tracing::warn!(
            target: "cas::coordination",
            stage = "worker_died_stamp_failed",
            notification_id,
            %error,
            "cas-3dcb: failed to stamp prompt_delivered_at for a worker-death notice"
        );
    }
}

/// Result of coalescing task-free duplicate worker-death relays.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkerDeathCoalesceReport {
    /// Logical no-action incident families replaced by a batch.
    pub families: usize,
    /// Duplicate prompt/durable rows made terminal behind their canonical row.
    pub duplicates: usize,
}

/// Collapse pending no-action deaths for duplicate registrations of one
/// logical factory worker before any of them reaches the supervisor turn.
///
/// A death holding or parking even one task is never eligible: those failures
/// remain separate, critical, actionable relays. Task-free deaths group by
/// target, factory session and logical worker name. The canonical prompt lists
/// every registration UUID and durable notification ID for forensics; sibling
/// prompt rows are suppressed and sibling durable rows are processed so both
/// `message_*` and `queue_*` views expose one batch.
pub(crate) fn coalesce_pending_worker_deaths(
    prompt_queue: &dyn PromptQueueStore,
    supervisor_queue: Option<&dyn SupervisorQueueStore>,
    rows: &[cas_store::QueuedPrompt],
) -> Result<WorkerDeathCoalesceReport, cas_store::StoreError> {
    use std::collections::BTreeMap;

    type FamilyKey = (String, Option<String>, String);
    let mut families: BTreeMap<
        FamilyKey,
        Vec<(
            cas_store::QueuedPrompt,
            crate::prompt_revalidation::WorkerDiedEnvelope,
        )>,
    > = BTreeMap::new();

    for row in rows {
        let Some(envelope) = crate::prompt_revalidation::parse_worker_died_envelope(&row.prompt)
        else {
            continue;
        };
        if !envelope.held_tasks.is_empty() || !envelope.recovered_tasks.is_empty() {
            continue;
        }
        families
            .entry((
                row.target.clone(),
                row.factory_session.clone(),
                envelope.worker_name.clone(),
            ))
            .or_default()
            .push((row.clone(), envelope));
    }

    let mut report = WorkerDeathCoalesceReport::default();
    for ((_target, _session, worker_name), mut family) in families {
        if family.len() < 2 {
            continue;
        }
        family.sort_by_key(|(row, _)| row.id);
        let (canonical, canonical_envelope) = &family[0];
        let mut worker_ids = family
            .iter()
            .flat_map(|(_, envelope)| envelope.forensic_worker_ids.clone())
            .collect::<Vec<_>>();
        worker_ids.sort();
        worker_ids.dedup();
        let incidents = family
            .iter()
            .map(|(_, envelope)| envelope.incident.clone())
            .collect::<Vec<_>>();
        let mut notification_ids = family
            .iter()
            .flat_map(|(_, envelope)| envelope.coalesced_notification_ids.clone())
            .collect::<Vec<_>>();
        notification_ids.sort_unstable();
        notification_ids.dedup();
        // The visible notification ID must identify the canonical prompt row's
        // dedupe key. The two queues have independent sequences, so sorting
        // durable IDs can otherwise expose a suppressed sibling's ID and make
        // message_ack leave the canonical replay row pending.
        notification_ids.retain(|id| *id != canonical_envelope.notification_id);
        notification_ids.insert(0, canonical_envelope.notification_id);
        let Some(prompt) = crate::prompt_revalidation::format_coalesced_worker_died_relay(
            &worker_name,
            &worker_ids,
            &incidents,
            &notification_ids,
        ) else {
            continue;
        };
        let summary = format!(
            "worker died: {worker_name} ({} duplicate no-task registrations)",
            family.len()
        );

        // Do not retire siblings unless the canonical row was safely rewritten.
        if !prompt_queue.rewrite_pending(canonical.id, &prompt, Some(&summary))? {
            continue;
        }

        for (duplicate, envelope) in family.iter().skip(1) {
            prompt_queue.mark_suppressed(
                duplicate.id,
                Some(&format!(
                    "coalesced into worker-death batch prompt {} for logical worker {}",
                    canonical.id, worker_name
                )),
            )?;
            if let Some(queue) = supervisor_queue
                && let Ok(Some(durable)) = queue.get(envelope.notification_id)
                && durable.processed_at.is_none()
            {
                let _ = queue.ack(envelope.notification_id);
            }
            report.duplicates += 1;
        }
        report.families += 1;
    }

    Ok(report)
}

/// Format the "Recently died while leased" section for worker_status.
///
/// Pulls recent WorkerDied events (last `window_secs`) and any still-stale
/// workers that held leases at death. Returns empty string when nothing to show.
pub fn format_recently_died_while_leased(
    cas_root: &Path,
    agent_store: &dyn AgentStore,
    factory_session: Option<&str>,
    window_secs: i64,
    live_worker_names: &std::collections::HashSet<String>,
) -> String {
    let cutoff = Utc::now() - chrono::Duration::seconds(window_secs);
    let mut lines: Vec<String> = Vec::new();

    // Prefer structured WorkerDied events.
    if let Ok(event_store) = open_event_store(cas_root) {
        if let Ok(events) = event_store.list_since(cutoff, 100) {
            for ev in events {
                if ev.event_type != EventType::WorkerDied {
                    continue;
                }
                let meta = ev.metadata.as_ref();
                let name = meta
                    .and_then(|m| m.get("worker_name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(ev.entity_id.as_str());
                // A new registration for this logical worker is currently
                // live in the roster. The historical death is useful audit
                // data but presenting it beside that live row is a direct
                // contradiction and trains operators to distrust status.
                if live_worker_names.contains(name) {
                    continue;
                }
                let held = meta
                    .and_then(|m| m.get("held_tasks"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                if held.is_empty() {
                    // AC: died-while-leased — skip pure idle deaths.
                    continue;
                }
                let recovered = meta
                    .and_then(|m| m.get("recovered_tasks"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                let hb = meta
                    .and_then(|m| m.get("last_heartbeat"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let elapsed = chrono::DateTime::parse_from_rfc3339(hb)
                    .map(|dt| (Utc::now() - dt.with_timezone(&Utc)).num_seconds())
                    .unwrap_or(-1);
                let since = if elapsed >= 0 {
                    format!("{elapsed}s ago")
                } else {
                    hb.to_string()
                };
                let rec_note = if recovered.is_empty() {
                    String::new()
                } else {
                    format!(" → Open (orphaned: {recovered})")
                };
                lines.push(format!(
                    "  • {name} (last heartbeat: {since}) [stale]\n    held: {held}{rec_note}"
                ));
            }
        }
    }

    // Also surface currently-stale workers that still hold InProgress assignee
    // (recovery not yet run) — defensive second signal.
    if let Ok(stale) = agent_store.list(Some(AgentStatus::Stale)) {
        if let Ok(task_store) = open_task_store(cas_root) {
            for agent in stale {
                if !agent.visible_to_factory_session(factory_session) {
                    continue;
                }
                if agent.role != AgentRole::Worker {
                    continue;
                }
                if live_worker_names.contains(&agent.name) {
                    continue;
                }
                // Skip if already listed via WorkerDied event for this agent.
                if lines.iter().any(|l| l.contains(&agent.name)) {
                    continue;
                }
                let mut held = Vec::new();
                if let Ok(tasks) = task_store.list(Some(TaskStatus::InProgress)) {
                    for t in tasks {
                        if task_assigned_to_agent(&t.assignee, &agent) {
                            held.push(t.id);
                        }
                    }
                }
                if held.is_empty() {
                    continue;
                }
                let elapsed = (Utc::now() - agent.last_heartbeat).num_seconds();
                lines.push(format!(
                    "  • {} (last heartbeat: {elapsed}s ago) [stale]\n    held: {} (still InProgress — run agent_cleanup)",
                    agent.name,
                    held.join(", ")
                ));
            }
        }
    }

    if lines.is_empty() {
        return String::new();
    }
    // Dedupe by worker name line prefix.
    lines.sort();
    lines.dedup();
    format!(
        "\nRecently died while leased ({}):\n{}\n",
        lines.len(),
        lines.join("\n")
    )
}

#[cfg(test)]
mod cas_3dcb_death_relay_tests {
    use super::*;
    use crate::store::{open_agent_store, open_prompt_queue_store, open_task_store};
    use cas_types::{AgentRole, Task};

    struct Fixture {
        _dir: tempfile::TempDir,
        cas_root: std::path::PathBuf,
        agent_store: Arc<dyn AgentStore>,
        supervisor_id: String,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let cas_root = dir.path().to_path_buf();
            let agent_store = open_agent_store(&cas_root).expect("agent store");

            let supervisor_id = Agent::generate_fallback_id();
            let mut supervisor = Agent::new(supervisor_id.clone(), "sup".to_string());
            supervisor.role = AgentRole::Supervisor;
            agent_store.register(&supervisor).expect("register sup");

            Self {
                _dir: dir,
                cas_root,
                agent_store,
                supervisor_id,
            }
        }

        fn dead_worker(&self, name: &str, dead_for_secs: i64) -> Agent {
            let mut agent = Agent::new(Agent::generate_fallback_id(), name.to_string());
            agent.role = AgentRole::Worker;
            agent.last_heartbeat = Utc::now() - chrono::Duration::seconds(dead_for_secs);
            self.agent_store.register(&agent).expect("register worker");
            agent
        }

        fn emit(&self, agent: &Agent) {
            self.emit_summary(
                agent,
                OrphanRecoverySummary {
                    held_task_ids: vec!["cas-held1".to_string()],
                    recovered_task_ids: vec!["cas-held1".to_string()],
                },
            );
        }

        fn emit_summary(&self, agent: &Agent, summary: OrphanRecoverySummary) {
            emit_worker_died_signals(
                &self.cas_root,
                self.agent_store.as_ref(),
                agent,
                &summary,
                "daemon maintenance: heartbeat stale",
            );
        }

        fn prompt_relays(&self) -> Vec<cas_store::QueuedPrompt> {
            open_prompt_queue_store(&self.cas_root)
                .expect("prompt queue")
                .peek_all(200)
                .expect("peek")
                .into_iter()
                .filter(|row| row.prompt.starts_with("<worker-died "))
                .collect()
        }

        fn durable_notices(&self) -> usize {
            open_supervisor_queue_store(&self.cas_root)
                .expect("queue")
                .peek(&self.supervisor_id, 500)
                .expect("peek")
                .into_iter()
                .filter(|n| n.event_type == "worker_died")
                .count()
        }
    }

    /// The reported defect (GH #168): the emitter wrote to `supervisor_queue`
    /// only, so 2,044 critical death alerts never entered a supervisor turn.
    #[test]
    fn a_death_lands_on_the_prompt_path_with_wake_semantics() {
        let fixture = Fixture::new();
        let worker = fixture.dead_worker("lost-worker", 900);
        fixture.emit(&worker);

        let relays = fixture.prompt_relays();
        assert_eq!(relays.len(), 1, "one death, one prompt relay: {relays:?}");
        let relay = &relays[0];
        assert_eq!(relay.target, "supervisor");
        assert!(
            crate::mcp::tools::core::task::lifecycle::supervisor_push::is_lifecycle_wake_source(
                &relay.source
            ),
            "the relay must be wake-eligible or it neither wakes an idle supervisor nor \
             surfaces as a lost relay. source={}",
            relay.source
        );
        // `is_dead_worker_source` drops rows sourced from a dead worker's name.
        assert!(!relay.source.contains(&worker.name));
        let envelope = crate::prompt_revalidation::parse_worker_died_envelope(&relay.prompt)
            .expect("the daemon must be able to classify what we wrote");
        assert_eq!(envelope.worker_name, "lost-worker");
        assert!(relay.prompt.contains("cas-held1"));
    }

    /// GH #197: requested shutdown is still a worker termination. It must use
    /// the same durable supervisor relay as a crash rather than disappearing
    /// merely because the exit was intentional.
    #[test]
    fn intentional_shutdown_emits_supervisor_lifecycle_relay() {
        let fixture = Fixture::new();
        let worker = fixture.dead_worker("retired-worker", 0);
        let summary = OrphanRecoverySummary {
            held_task_ids: vec!["cas-active".to_string()],
            recovered_task_ids: vec!["cas-active".to_string()],
        };
        emit_worker_died_signals(
            &fixture.cas_root,
            fixture.agent_store.as_ref(),
            &worker,
            &summary,
            "worker terminated by shutdown request",
        );

        let relays = fixture.prompt_relays();
        assert_eq!(relays.len(), 1, "one termination, one relay: {relays:?}");
        assert!(relays[0].prompt.contains("retired-worker"));
        assert!(relays[0].prompt.contains("cas-active"));
        assert!(relays[0].prompt.contains("shutdown request"));
        assert_eq!(fixture.durable_notices(), 1);
    }

    #[test]
    fn stale_supervisor_expiry_emits_no_worker_died_relay() {
        let fixture = Fixture::new();
        let supervisor = fixture
            .agent_store
            .get(&fixture.supervisor_id)
            .expect("registered supervisor");

        let summary = recover_worker_vanished(
            &fixture.cas_root,
            fixture.agent_store.as_ref(),
            &supervisor,
            &[],
            "daemon maintenance: heartbeat stale",
        );

        assert!(summary.held_task_ids.is_empty());
        assert!(summary.recovered_task_ids.is_empty());
        assert!(
            fixture.prompt_relays().is_empty(),
            "supervisor expiry must not inject a worker_died relay back to itself"
        );
        assert_eq!(
            fixture.durable_notices(),
            0,
            "supervisor expiry must not create a critical worker_died notification"
        );
    }

    /// The 1,452-notices-for-one-agent class, proven at the emitter: however
    /// many times the same corpse is re-detected, one incident yields one
    /// notice on each channel.
    #[test]
    fn re_detecting_the_same_death_cannot_storm() {
        let fixture = Fixture::new();
        let worker = fixture.dead_worker("re-detected", 900);

        for _ in 0..50 {
            fixture.emit(&worker);
        }

        assert_eq!(
            fixture.prompt_relays().len(),
            1,
            "50 re-detections must not produce 50 prompt relays"
        );
        assert_eq!(
            fixture.durable_notices(),
            1,
            "50 re-detections must not produce 50 durable notices"
        );
    }

    #[test]
    fn recovery_enumerates_and_parks_assigned_in_progress_and_blocked_tasks() {
        let fixture = Fixture::new();
        let worker = fixture.dead_worker("task-holder", 900);
        let tasks = open_task_store(&fixture.cas_root).expect("task store");
        for (id, status) in [
            ("cas-in-progress", TaskStatus::InProgress),
            ("cas-blocked", TaskStatus::Blocked),
        ] {
            let mut task = Task::new(id.to_string(), id.to_string());
            task.status = status;
            task.assignee = Some(worker.name.clone());
            tasks.add(&task).expect("add task");
        }

        let summary = recover_worker_vanished(
            &fixture.cas_root,
            fixture.agent_store.as_ref(),
            &worker,
            &[],
            "test worker death",
        );
        assert_eq!(summary.held_task_ids, vec!["cas-blocked", "cas-in-progress"]);
        assert_eq!(summary.recovered_task_ids, vec!["cas-blocked", "cas-in-progress"]);
        for id in &summary.recovered_task_ids {
            let task = tasks.get(id).expect("recovered task");
            assert_eq!(task.status, TaskStatus::Open);
            assert_eq!(task.assignee, None);
            assert!(task.notes.contains("orphan recovery"));
        }
        let relay = fixture.prompt_relays().pop().expect("death relay");
        assert!(relay.prompt.contains("cas-in-progress") && relay.prompt.contains("cas-blocked"));
    }

    /// Dedup keys the death INCIDENT, not the agent: a worker that comes back,
    /// heartbeats, and dies again is a new fact the supervisor must hear.
    #[test]
    fn a_genuinely_separate_death_is_reported_again() {
        let fixture = Fixture::new();
        let mut worker = fixture.dead_worker("twice-dead", 900);
        fixture.emit(&worker);
        fixture.emit(&worker);
        assert_eq!(fixture.prompt_relays().len(), 1);

        // Revived, worked, died again — a later heartbeat is a later incident.
        worker.last_heartbeat = Utc::now() - chrono::Duration::seconds(60);
        fixture.emit(&worker);
        fixture.emit(&worker);

        assert_eq!(
            fixture.prompt_relays().len(),
            2,
            "the second death must be reported; dedup must not silence a live worker's \
             later death"
        );
        assert_eq!(fixture.durable_notices(), 2);
    }

    /// Existing `supervisor_queue` consumers must keep seeing the death, and
    /// the durable row must record that the prompt handoff completed.
    #[test]
    fn durable_row_survives_and_is_stamped_delivered() {
        let fixture = Fixture::new();
        let worker = fixture.dead_worker("stamped", 900);
        fixture.emit(&worker);

        let queue = open_supervisor_queue_store(&fixture.cas_root).expect("queue");
        let notices: Vec<_> = queue
            .peek(&fixture.supervisor_id, 50)
            .expect("peek")
            .into_iter()
            .filter(|n| n.event_type == "worker_died")
            .collect();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].priority, NotificationPriority::Critical);
        assert!(
            notices[0].payload.contains("stamped"),
            "existing consumers still read the same payload shape: {}",
            notices[0].payload
        );
        assert!(
            notices[0].prompt_delivered_at.is_some(),
            "prompt handoff must be stamped so the outbox does not retry forever"
        );
    }

    /// With no live supervisor row, the notice still goes to the parent agent
    /// rather than being dropped — and stays deduped there.
    #[test]
    fn falls_back_to_parent_when_no_supervisor_is_registered() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cas_root = dir.path().to_path_buf();
        let agent_store = open_agent_store(&cas_root).expect("agent store");

        let parent_id = Agent::generate_fallback_id();
        let mut worker = Agent::new(Agent::generate_fallback_id(), "orphan".to_string());
        worker.role = AgentRole::Worker;
        worker.parent_id = Some(parent_id.clone());
        worker.last_heartbeat = Utc::now() - chrono::Duration::seconds(900);
        agent_store.register(&worker).expect("register");

        for _ in 0..5 {
            emit_worker_died_signals(
                &cas_root,
                agent_store.as_ref(),
                &worker,
                &OrphanRecoverySummary::default(),
                "no supervisor present",
            );
        }

        let queue = open_supervisor_queue_store(&cas_root).expect("queue");
        let notices = queue.peek(&parent_id, 50).expect("peek");
        assert_eq!(
            notices
                .iter()
                .filter(|n| n.event_type == "worker_died")
                .count(),
            1,
            "the parent fallback must fire exactly once, not once per tick"
        );
    }

    /// cas-20ac live shape: duplicate nested registrations for one logical
    /// worker all expire in one cleanup pass. None held work, so thirty
    /// critical rows must become one explicit no-action forensic batch across
    /// both queue views before the director can inject them.
    #[test]
    fn thirty_duplicate_no_task_deaths_coalesce_to_one_bounded_batch() {
        let fixture = Fixture::new();
        let mut worker_ids = Vec::new();
        for _ in 0..30 {
            let worker = fixture.dead_worker("lt-codemap-refresh", 900);
            worker_ids.push(worker.id.clone());
            fixture.emit_summary(&worker, OrphanRecoverySummary::default());
        }

        let prompt_queue = open_prompt_queue_store(&fixture.cas_root).expect("prompt queue");
        let supervisor_queue =
            open_supervisor_queue_store(&fixture.cas_root).expect("supervisor queue");
        let before = fixture.prompt_relays();
        assert_eq!(before.len(), 30, "fixture must reproduce the flood first");

        let report = coalesce_pending_worker_deaths(
            prompt_queue.as_ref(),
            Some(supervisor_queue.as_ref()),
            &before,
        )
        .expect("coalesce");
        assert_eq!(
            report,
            WorkerDeathCoalesceReport {
                families: 1,
                duplicates: 29
            }
        );

        let after = fixture.prompt_relays();
        assert_eq!(after.len(), 1, "one logical worker must inject one turn");
        let batch = &after[0].prompt;
        assert!(batch.contains("30 duplicate registry rows expire"));
        assert!(batch.contains("No tasks were held or parked"));
        for worker_id in worker_ids {
            assert!(
                batch.contains(&worker_id),
                "forensic registration ID missing from batch: {worker_id}"
            );
        }
        assert_eq!(
            fixture.durable_notices(),
            1,
            "queue_poll/queue_peek must see the same single batch, not 30 rows"
        );
    }

    /// The durable and prompt queues use independent sequences. A coalesced
    /// batch must expose the durable ID linked to the canonical prompt, even
    /// when durable notification order and prompt insertion order disagree.
    #[test]
    fn coalesced_batch_ack_id_always_points_to_the_canonical_prompt() {
        let fixture = Fixture::new();
        let prompt_queue = open_prompt_queue_store(&fixture.cas_root).expect("prompt queue");
        let supervisor_queue =
            open_supervisor_queue_store(&fixture.cas_root).expect("supervisor queue");

        let durable_one = supervisor_queue
            .notify(
                &fixture.supervisor_id,
                "worker_died",
                "{}",
                NotificationPriority::Critical,
            )
            .expect("durable one");
        let durable_two = supervisor_queue
            .notify(
                &fixture.supervisor_id,
                "worker_died",
                "{}",
                NotificationPriority::Critical,
            )
            .expect("durable two");

        // Reverse the durable order in prompt_queue: durable_two owns the
        // canonical (lowest prompt ID), while durable_one is the sibling.
        for (worker_id, incident, notification_id) in [
            ("registration-two", "incident-two", durable_two),
            ("registration-one", "incident-one", durable_one),
        ] {
            prompt_queue
                .enqueue_idempotent(
                    &format!("lifecycle-wake:worker-died:{notification_id}"),
                    "supervisor",
                    &crate::prompt_revalidation::format_worker_died_relay(
                        worker_id,
                        "logical-worker",
                        incident,
                        "stale registration",
                        &[],
                        &[],
                        notification_id,
                    ),
                    None,
                    Some("worker died: logical-worker"),
                    Some(NotificationPriority::Critical),
                    &format!("worker-died-outbox:{notification_id}"),
                    None,
                )
                .expect("prompt relay");
        }

        let before = fixture.prompt_relays();
        coalesce_pending_worker_deaths(
            prompt_queue.as_ref(),
            Some(supervisor_queue.as_ref()),
            &before,
        )
        .expect("coalesce");

        let canonical = fixture.prompt_relays();
        assert_eq!(canonical.len(), 1);
        let envelope = crate::prompt_revalidation::parse_worker_died_envelope(&canonical[0].prompt)
            .expect("batch envelope");
        assert_eq!(
            envelope.notification_id, durable_two,
            "the visible ACK ID must resolve to the canonical prompt's dedupe key"
        );

        prompt_queue
            .ack_by_dedupe_key(&format!(
                "worker-died-outbox:{}",
                envelope.notification_id
            ))
            .expect("ack canonical dedupe key")
            .expect("canonical prompt");
        let replay = prompt_queue
            .poll_unseen_for_recipient("supervisor", None, 10)
            .expect("replay poll");
        assert!(
            replay.iter().all(|row| row.id != canonical[0].id),
            "acknowledging the visible batch ID must prevent canonical replay: {replay:?}"
        );
    }

    /// Task-bearing deaths are actionable incidents, not registry noise. Even
    /// when names match, different held tasks must retain separate urgent rows.
    #[test]
    fn distinct_task_holding_deaths_are_never_coalesced() {
        let fixture = Fixture::new();
        let first = fixture.dead_worker("logical-worker", 900);
        let second = fixture.dead_worker("logical-worker", 901);
        fixture.emit_summary(
            &first,
            OrphanRecoverySummary {
                held_task_ids: vec!["cas-task-a".to_string()],
                recovered_task_ids: vec!["cas-task-a".to_string()],
            },
        );
        fixture.emit_summary(
            &second,
            OrphanRecoverySummary {
                held_task_ids: vec!["cas-task-b".to_string()],
                recovered_task_ids: vec!["cas-task-b".to_string()],
            },
        );

        let prompt_queue = open_prompt_queue_store(&fixture.cas_root).expect("prompt queue");
        let supervisor_queue =
            open_supervisor_queue_store(&fixture.cas_root).expect("supervisor queue");
        let before = fixture.prompt_relays();
        let report = coalesce_pending_worker_deaths(
            prompt_queue.as_ref(),
            Some(supervisor_queue.as_ref()),
            &before,
        )
        .expect("coalesce");

        assert_eq!(report, WorkerDeathCoalesceReport::default());
        let after = fixture.prompt_relays();
        assert_eq!(after.len(), 2);
        assert!(after.iter().any(|row| row.prompt.contains("cas-task-a")));
        assert!(after.iter().any(|row| row.prompt.contains("cas-task-b")));
        assert_eq!(fixture.durable_notices(), 2);
    }

    #[test]
    fn recently_died_section_excludes_a_name_that_is_live_in_the_roster() {
        let fixture = Fixture::new();
        let worker = fixture.dead_worker("returned-worker", 900);
        let event = Event::new(
            EventType::WorkerDied,
            EventEntityType::Agent,
            worker.id.clone(),
            "returned worker died before respawn",
        )
        .with_metadata(serde_json::json!({
            "worker_name": worker.name,
            "held_tasks": ["cas-held1"],
            "last_heartbeat": worker.last_heartbeat.to_rfc3339(),
        }));
        open_event_store(&fixture.cas_root)
            .unwrap()
            .record(&event)
            .unwrap();

        let live_names = std::collections::HashSet::from(["returned-worker".to_string()]);
        let rendered = format_recently_died_while_leased(
            &fixture.cas_root,
            fixture.agent_store.as_ref(),
            None,
            3600,
            &live_names,
        );
        assert!(
            rendered.is_empty(),
            "a current live row and a historical death for the same name must not render together: {rendered}"
        );
    }
}
