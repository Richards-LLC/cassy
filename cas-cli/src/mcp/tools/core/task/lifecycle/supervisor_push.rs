//! Centralized task lifecycle → owning-supervisor push (cas-062d / cas-17e4 / cas-ecff).
//!
//! Single transition-to-event seam so start / blocked / ready / close-rejected /
//! awaiting-merge / closed cannot drift. Events are durable in
//! `supervisor_queue` (idempotent by **occurrence** identity) and delivered via
//! `prompt_queue` as an outbox step:
//! - prompt enqueue is **required** when an owning supervisor exists (open failure
//!   leaves durable pending, never stamps delivered)
//! - prompt rows use a unique `dedupe_key` so stamp-failure replay cannot duplicate
//! - [`drain_lifecycle_outbox`] repairs pending rows without re-running task mutations

use chrono::{DateTime, Utc};
use serde_json::json;

use cas_store::{
    AgentStore, NotificationPriority, NotifyIdempotentResult, PromptQueueStore,
    SupervisorNotification, SupervisorQueueStore,
};
use cas_types::{AgentRole, TaskStatus};
use std::path::Path;

use super::TaskLifecycleGateError;
use crate::mcp::server::CasCore;
use crate::store::{open_prompt_queue_store, open_supervisor_queue_store};

/// Named lifecycle transitions that must push to the owning supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleTransition {
    Started,
    Blocked,
    ReadyReopened,
    CloseRejected,
    AwaitingMerge,
    Closed,
}

impl LifecycleTransition {
    pub fn as_event_type(self) -> &'static str {
        match self {
            Self::Started => "task_started",
            Self::Blocked => "task_blocked",
            Self::ReadyReopened => "task_ready",
            Self::CloseRejected => "task_close_rejected",
            Self::AwaitingMerge => "task_awaiting_merge",
            Self::Closed => "task_closed",
        }
    }

    pub fn priority(self) -> NotificationPriority {
        match self {
            Self::CloseRejected
            | Self::Blocked
            | Self::AwaitingMerge
            => NotificationPriority::High,
            Self::Started | Self::ReadyReopened | Self::Closed => NotificationPriority::Normal,
        }
    }

    /// Whether this transition may wake an IDLE supervisor pane (cas-f02b /
    /// GH #101).
    ///
    /// True exactly for the transitions that park a worker behind supervisor
    /// action: the work is finished or stopped, and nothing in the factory can
    /// proceed until the supervisor merges, re-reviews, or unblocks. Delivered
    /// to a Claude supervisor in teams mode, these are inbox FILE writes —
    /// Claude Code polls its inbox only at turn boundaries, and an idle
    /// supervisor has no upcoming boundary, so the signal sits unread until
    /// something external creates a turn. That is the reported failure: fleets
    /// parked in `awaiting_merge` idling silently until a cron sweep, with
    /// every drain discovered by poll and never by push.
    ///
    /// `Started` / `ReadyReopened` / `Closed` stay false — they are progress
    /// FYI, and waking a supervisor for them would re-create the noise the
    /// idle-nudge exclusion (cas-dab2) was added to stop.
    pub fn wakes_idle_supervisor(self) -> bool {
        match self {
            Self::CloseRejected
            | Self::AwaitingMerge
            | Self::Blocked
            => true,
            Self::Started | Self::ReadyReopened | Self::Closed => false,
        }
    }
}

/// Marker prefix on a `prompt_queue.source` whose row may wake an idle
/// supervisor pane (cas-f02b).
///
/// The delivery lane needs to tell "a worker sent the supervisor a message"
/// (inbox-only — see cas-dab2) from "the factory is stalled behind the
/// supervisor" (wake-eligible). Rather than have the daemon sniff prompt text,
/// the emitting side states the intent in the one field it already synthesizes.
pub const LIFECYCLE_WAKE_SOURCE_PREFIX: &str = "lifecycle-wake:";

/// Non-waking counterpart of [`LIFECYCLE_WAKE_SOURCE_PREFIX`].
pub const LIFECYCLE_SOURCE_PREFIX: &str = "lifecycle:";

/// `prompt_queue.source` for one lifecycle notification, encoding whether the
/// transition may wake an idle supervisor (cas-f02b).
///
/// Both the live emit path and the outbox drain build the source here, so a
/// replayed row carries the same wake eligibility as the original.
pub fn lifecycle_prompt_source(kind: LifecycleTransition, notification_id: i64) -> String {
    let prefix = if kind.wakes_idle_supervisor() {
        LIFECYCLE_WAKE_SOURCE_PREFIX
    } else {
        LIFECYCLE_SOURCE_PREFIX
    };
    format!("{prefix}{notification_id}")
}

/// Whether a queued prompt's `source` marks it as a supervisor wake signal.
pub fn is_lifecycle_wake_source(source: &str) -> bool {
    source.starts_with(LIFECYCLE_WAKE_SOURCE_PREFIX)
}

/// Result of a lifecycle push attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecyclePushResult {
    /// New durable event enqueued and prompt delivery completed (or no prompt path).
    Enqueued { notification_id: i64 },
    /// Durable row already present; prompt delivery was completed (or re-stamped) without
    /// inserting a second durable event.
    Recovered { notification_id: i64 },
    /// Same occurrence fully complete (durable + prompt) — no new side effects.
    AlreadyComplete { notification_id: i64 },
    /// No owning supervisor found for the factory session (non-factory or empty).
    NoSupervisor,
}

/// Build occurrence-scoped transition identity for idempotency (cas-17e4).
///
/// Includes:
/// - factory_session so concurrent factories never collide/leak
/// - occurrence_id (typically post-mutation `task.updated_at`) so two legitimate
///   Open→InProgress cycles (start → block → ready → start) produce distinct events,
///   while retrying the *same* occurrence still dedupes
pub fn transition_key(
    task_id: &str,
    old_status: TaskStatus,
    new_status: TaskStatus,
    factory_session: Option<&str>,
    kind: LifecycleTransition,
    occurrence_id: &str,
) -> String {
    format!(
        "{task_id}:{old_status}:{new_status}:{}:{}:{occurrence_id}",
        factory_session.unwrap_or(""),
        kind.as_event_type()
    )
}

/// Format occurrence id from a post-mutation timestamp (stable for that write).
pub fn occurrence_from_updated_at(updated_at: DateTime<Utc>) -> String {
    updated_at.to_rfc3339()
}

/// Build the durable half of one lifecycle occurrence before a task mutation.
///
/// Closed-task reopen uses this value inside the same SQLite transaction as
/// the task/proof/dependency changes. Prompt delivery remains the recoverable
/// outbox step performed by [`emit_task_lifecycle_transition`] after commit.
#[allow(clippy::too_many_arguments)]
pub fn prepare_task_lifecycle_outbox(
    agent_store: &dyn AgentStore,
    task_id: &str,
    task_title: &str,
    old_status: TaskStatus,
    new_status: TaskStatus,
    actor: &str,
    reason: Option<&str>,
    kind: LifecycleTransition,
    occurrence_id: &str,
) -> Option<cas_store::TaskReopenLifecycleOutbox> {
    let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let supervisor = resolve_owning_supervisor(agent_store, factory_session.as_deref())?;
    let transition_key = transition_key(
        task_id,
        old_status,
        new_status,
        factory_session.as_deref(),
        kind,
        occurrence_id,
    );
    let now = Utc::now();
    let payload = json!({
        "task_id": task_id,
        "title": task_title,
        "old_status": old_status.to_string(),
        "new_status": new_status.to_string(),
        "actor": actor,
        "reason": reason,
        "transition": kind.as_event_type(),
        "factory_session": factory_session,
        "supervisor_id": supervisor.agent_id,
        "supervisor_name": supervisor.name,
        "occurrence_id": occurrence_id,
        "transition_key": transition_key,
        "timestamp": now.to_rfc3339(),
    })
    .to_string();
    let actor_is_owning_supervisor = actor == supervisor.agent_id || actor == supervisor.name;
    Some(cas_store::TaskReopenLifecycleOutbox {
        supervisor_id: supervisor.agent_id,
        payload,
        priority: kind.priority(),
        transition_key,
        prompt_delivered_at: actor_is_owning_supervisor.then_some(now),
    })
}

/// Stable prompt_queue dedupe key for one durable lifecycle notification (cas-ecff).
pub fn lifecycle_prompt_dedupe_key(notification_id: i64) -> String {
    format!("lifecycle-outbox:{notification_id}")
}

const AUTO_UNBLOCK_ASSIGNEE_STALE_SECS: i64 = 300;
const AUTO_UNBLOCK_SPAWN_RECEIPT_LIMIT: usize = 100;

/// Result of trying to wake the worker that owns an automatically unblocked
/// task. The worker prompt uses the normal, non-urgent coordination-message
/// queue path, while the idempotent key keeps one unblock occurrence from
/// producing duplicate turns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AutoUnblockWakeResult {
    Queued {
        worker_name: String,
        prompt_id: i64,
        dedupe_key: String,
    },
    Skipped {
        reason: String,
    },
}

enum AutoUnblockWorkerTarget {
    Worker(String),
    Skipped(String),
}

fn append_auto_unblock_wake_note(
    cas_root: &Path,
    task_id: &str,
    marker: &str,
    note: &str,
) -> Result<(), String> {
    let task_store = crate::store::open_task_store(cas_root).map_err(|error| {
        format!("task store open failed while recording auto-unblock wake: {error}")
    })?;
    let mut task = task_store
        .get(task_id)
        .map_err(|error| format!("task read failed while recording auto-unblock wake: {error}"))?;
    if task.notes.contains(marker) {
        return Ok(());
    }
    task.notes = if task.notes.is_empty() {
        note.to_string()
    } else {
        format!("{}\n\n{note}", task.notes)
    };
    task.updated_at = Utc::now();
    task_store.update(&task).map(|_| ()).map_err(|error| {
        format!("task note update failed while recording auto-unblock wake: {error}")
    })
}

fn auto_unblock_worker_target(
    cas_root: &Path,
    task: &cas_types::Task,
) -> Result<AutoUnblockWorkerTarget, String> {
    let factory_session = std::env::var("CAS_FACTORY_SESSION")
        .ok()
        .filter(|session| !session.trim().is_empty());

    if let Some(assignee) = task.assignee.as_deref() {
        let agent_store = crate::store::open_agent_store(cas_root).map_err(|error| {
            format!("agent store open failed while resolving assignee: {error}")
        })?;
        let agents = agent_store
            .list(None)
            .map_err(|error| format!("agent list failed while resolving assignee: {error}"))?;
        let live_assignee = agents.into_iter().find(|agent| {
            agent.role == AgentRole::Worker
                && agent.visible_to_factory_session(factory_session.as_deref())
                && (agent.name == assignee || agent.id == assignee)
                && agent.is_alive()
                && !agent.is_heartbeat_expired(AUTO_UNBLOCK_ASSIGNEE_STALE_SECS)
        });
        return Ok(match live_assignee {
            Some(agent) => AutoUnblockWorkerTarget::Worker(agent.name),
            None => AutoUnblockWorkerTarget::Skipped(format!(
                "assignee '{assignee}' has no registered worker with a fresh heartbeat"
            )),
        });
    }

    let Some(factory_session) = factory_session.as_deref() else {
        return Ok(AutoUnblockWorkerTarget::Skipped(
            "no live assignee or matching pre-assignment receipt".to_string(),
        ));
    };
    let spawn_queue = crate::store::open_spawn_queue_store(cas_root).map_err(|error| {
        format!("spawn queue open failed while resolving pre-assignment: {error}")
    })?;
    let worker_name = spawn_queue
        .recent_spawn_lifecycle(factory_session, AUTO_UNBLOCK_SPAWN_RECEIPT_LIMIT)
        .map_err(|error| {
            format!("spawn receipt lookup failed while resolving pre-assignment: {error}")
        })?
        .into_iter()
        .filter(|receipt| receipt.task_id.as_deref() == Some(task.id.as_str()))
        .filter(|receipt| !receipt.state.is_terminal())
        .find_map(|receipt| {
            receipt
                .worker_name
                .filter(|name| !name.trim().is_empty())
                .or_else(|| {
                    (receipt.requested_names.len() == 1).then(|| receipt.requested_names[0].clone())
                })
        });
    Ok(match worker_name {
        Some(worker_name) => AutoUnblockWorkerTarget::Worker(worker_name),
        None => AutoUnblockWorkerTarget::Skipped(
            "no live assignee or matching pre-assignment receipt".to_string(),
        ),
    })
}

/// Queue the synthetic worker wake emitted by the blocked→open transition.
///
/// This deliberately uses the same prompt queue consumed by
/// `coordination action=message`, with `urgent = false`; the daemon therefore
/// applies its ordinary idle-gate PTY nudge. The dedupe key is scoped to the
/// task, worker, blocker, and post-update occurrence, so a replay cannot add a
/// second prompt while a later unblock cycle can still wake the same worker.
pub(crate) fn queue_auto_unblock_worker_wake(
    cas_root: &Path,
    task: &cas_types::Task,
    blocker_id: &str,
    occurrence_id: &str,
) -> Result<AutoUnblockWakeResult, String> {
    let target = auto_unblock_worker_target(cas_root, task)?;
    let base_key = format!(
        "task-unblocked:{}:{}:{}",
        task.id, blocker_id, occurrence_id
    );
    let (worker_name, prompt_id, dedupe_key) = match target {
        AutoUnblockWorkerTarget::Skipped(reason) => {
            let marker = format!("auto-unblock-wake-skip:{base_key}");
            let timestamp = Utc::now().format("%Y-%m-%d %H:%M");
            let note =
                format!("[{timestamp}] Auto-unblock wake skipped: {reason}. (marker={marker})");
            append_auto_unblock_wake_note(cas_root, &task.id, &marker, &note)?;
            return Ok(AutoUnblockWakeResult::Skipped { reason });
        }
        AutoUnblockWorkerTarget::Worker(worker_name) => {
            let dedupe_key = format!("{base_key}:worker:{worker_name}");
            let prompt = format!(
                "Task {} is now unblocked (blocker {} closed): run task start id={}",
                task.id, blocker_id, task.id
            );
            let summary = format!("Task unblocked: {}", task.id);
            let factory_session = std::env::var("CAS_FACTORY_SESSION")
                .ok()
                .filter(|session| !session.trim().is_empty());
            let queue = crate::store::open_prompt_queue_store(cas_root).map_err(|error| {
                format!("prompt queue open failed for auto-unblock wake: {error}")
            })?;
            let result = queue
                .enqueue_idempotent(
                    "supervisor",
                    &worker_name,
                    &prompt,
                    factory_session.as_deref(),
                    Some(&summary),
                    Some(cas_store::NotificationPriority::Normal),
                    &dedupe_key,
                    Some(&cas_store::QueueOrigin::Daemon),
                )
                .map_err(|error| format!("auto-unblock worker wake enqueue failed: {error}"))?;
            let prompt_id = match result {
                cas_store::EnqueueIdempotentResult::Created(id)
                | cas_store::EnqueueIdempotentResult::AlreadyExists(id) => id,
            };
            crate::ui::factory::daemon::runtime::delivery::wake_daemon_after_enqueue(cas_root);
            (worker_name, prompt_id, dedupe_key)
        }
    };

    let marker = format!("auto-unblock-wake:{dedupe_key}");
    let timestamp = Utc::now().format("%Y-%m-%d %H:%M");
    let note = format!(
        "[{timestamp}] Auto-unblock wake queued for worker '{worker_name}' (prompt_id={prompt_id}): Task {} is now unblocked (blocker {blocker_id} closed): run task start id={} (dedupe_key={dedupe_key}; marker={marker}).",
        task.id, task.id
    );
    append_auto_unblock_wake_note(cas_root, &task.id, &marker, &note)?;
    Ok(AutoUnblockWakeResult::Queued {
        worker_name,
        prompt_id,
        dedupe_key,
    })
}

/// Truthful repair guidance after task mutation succeeded but lifecycle push failed.
///
/// Never claims that re-running the task operation is safe — status may already
/// make that operation illegal/no-op. Names the **callable** outbox drain path.
pub fn lifecycle_push_failure_message(
    task_id: &str,
    current_status: TaskStatus,
    kind: LifecycleTransition,
    transition_key: &str,
    error: &str,
) -> String {
    format!(
        "Task {task_id} is already {current_status}; supervisor lifecycle push \
         for {} failed: {error}. \
         Task state was NOT rolled back. \
         Repair: call drain_lifecycle_outbox (CasCore / factory daemon auto-drain) \
         for transition_key={transition_key} — durable event may already exist with \
         prompt_delivered_at unmarked; drain re-delivers via idempotent prompt \
         dedupe_key and stamps delivery exactly once. \
         Do NOT re-run the original task operation solely to recover the event; \
         that operation may now be illegal or a no-op for status={current_status}.",
        kind.as_event_type()
    )
}

/// Result of draining pending lifecycle outbox rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleOutboxDrainReport {
    pub attempted: usize,
    pub recovered: usize,
    pub already_complete: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

/// Owning supervisor identity for lifecycle push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwningSupervisor {
    /// Agent id used as `supervisor_queue.supervisor_id` (matches worker_died /
    /// queue_poll conventions — supervisors poll by agent id).
    pub agent_id: String,
    /// Display/pane name (for payloads and diagnostics).
    pub name: String,
}

/// Resolve the owning supervisor for a factory session.
///
/// Session isolation: only agents with `role == Supervisor` and matching
/// `factory_session` (via [`Agent::visible_to_factory_session`]) are considered.
/// Prefers Active/Idle over Stale/Shutdown, then stable name order.
pub fn resolve_owning_supervisor(
    agent_store: &dyn AgentStore,
    factory_session: Option<&str>,
) -> Option<OwningSupervisor> {
    let agents = agent_store.list(None).ok()?;
    let mut candidates: Vec<_> = agents
        .into_iter()
        .filter(|a| a.role == AgentRole::Supervisor)
        .filter(|a| a.visible_to_factory_session(factory_session))
        .collect();
    if candidates.is_empty() {
        return None;
    }
    // Prefer Active/Idle over stale; then the SUCCESSOR identity; then stable
    // name order.
    //
    // cas-7787 (GH #160): the recency tiebreak is load-bearing, not cosmetic.
    // A supervisor whose Claude session restarts mid-factory-session
    // re-registers under the same pane name with a NEW agent id (in the
    // reported session, `smooth-octopus-84` ended up with four rows —
    // d3556091 from before the restart, then 3f2b69fa and ad32fcde registered
    // 157ms apart at 18:45:07). The old ordering broke that tie on `name` then
    // `id`, i.e. lexicographically on a UUID — an arbitrary coin flip that can
    // hand every subsequent relay to the identity the operator has already
    // walked away from. Ordering by registration recency makes the successor
    // session win deterministically, so a relay emitted after a restart is
    // addressed to the supervisor that actually exists.
    candidates.sort_by(|a, b| {
        use cas_types::AgentStatus;
        let rank = |s: &AgentStatus| match s {
            AgentStatus::Active => 0,
            AgentStatus::Idle => 1,
            _ => 2,
        };
        rank(&a.status)
            .cmp(&rank(&b.status))
            .then_with(|| b.registered_at.cmp(&a.registered_at))
            .then_with(|| b.last_heartbeat.cmp(&a.last_heartbeat))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    let sup = &candidates[0];
    Some(OwningSupervisor {
        agent_id: sup.id.clone(),
        name: if sup.name.is_empty() {
            sup.id.clone()
        } else {
            sup.name.clone()
        },
    })
}

fn build_prompt_body(
    kind: LifecycleTransition,
    task_id: &str,
    task_title: &str,
    old_status: TaskStatus,
    new_status: TaskStatus,
    actor: &str,
    reason: Option<&str>,
    notification_id: i64,
    factory_session: Option<&str>,
    occurrence_id: &str,
) -> String {
    format!(
        "<task-lifecycle transition=\"{}\" task_id=\"{}\" old=\"{}\" new=\"{}\" actor=\"{}\" \
         notification_id=\"{}\" occurrence=\"{}\">\n\
         Task {} — {}\n\
         {}{}\
         </task-lifecycle>",
        kind.as_event_type(),
        task_id,
        old_status,
        new_status,
        actor,
        notification_id,
        occurrence_id,
        task_id,
        task_title,
        reason.map(|r| format!("Reason: {r}\n")).unwrap_or_default(),
        factory_session
            .map(|s| format!("Session: {s}\n"))
            .unwrap_or_default(),
    )
}

/// Emit one lifecycle transition to the owning supervisor (outbox workflow).
///
/// 1. **Durable:** `supervisor_queue.notify_idempotent` keyed by occurrence identity
/// 2. **Prompt:** if not yet marked `prompt_delivered`, enqueue to prompt_queue then stamp
///
/// Replaying the same occurrence after a prompt failure retries prompt delivery and
/// stamps delivery exactly once. Distinct occurrences (different `occurrence_id`) always
/// create distinct durable rows.
///
/// Task mutation must already have succeeded. Callers must surface errors with
/// [`lifecycle_push_failure_message`] — never claim the original task op retry is safe.
#[allow(clippy::too_many_arguments)]
pub fn emit_task_lifecycle_transition(
    supervisor_queue: &dyn SupervisorQueueStore,
    prompt_queue: Option<&dyn PromptQueueStore>,
    agent_store: &dyn AgentStore,
    task_id: &str,
    task_title: &str,
    old_status: TaskStatus,
    new_status: TaskStatus,
    actor: &str,
    reason: Option<&str>,
    kind: LifecycleTransition,
    occurrence_id: &str,
) -> Result<LifecyclePushResult, String> {
    let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let Some(supervisor) = resolve_owning_supervisor(agent_store, factory_session.as_deref())
    else {
        return Ok(LifecyclePushResult::NoSupervisor);
    };

    let key = transition_key(
        task_id,
        old_status,
        new_status,
        factory_session.as_deref(),
        kind,
        occurrence_id,
    );
    let now = Utc::now();
    let payload = json!({
        "task_id": task_id,
        "title": task_title,
        "old_status": old_status.to_string(),
        "new_status": new_status.to_string(),
        "actor": actor,
        "reason": reason,
        "transition": kind.as_event_type(),
        "factory_session": factory_session,
        "supervisor_id": supervisor.agent_id,
        "supervisor_name": supervisor.name,
        "occurrence_id": occurrence_id,
        "transition_key": key,
        "timestamp": now.to_rfc3339(),
    })
    .to_string();

    // Durable path keys by agent id so queue_poll / worker_died conventions match.
    let result = supervisor_queue
        .notify_idempotent(
            &supervisor.agent_id,
            "task_lifecycle",
            &payload,
            kind.priority(),
            &key,
        )
        .map_err(|e| format!("supervisor_queue write failed: {e}"))?;

    let (notification_id, already_existed, prompt_already_delivered) = match result {
        NotifyIdempotentResult::Created(id) => (id, false, false),
        NotifyIdempotentResult::AlreadyExists {
            id,
            prompt_delivered,
        } => (id, true, prompt_delivered),
    };

    // Fully complete occurrence — no side effects.
    if prompt_already_delivered {
        return Ok(LifecyclePushResult::AlreadyComplete { notification_id });
    }

    // The owning supervisor already knows about lifecycle mutations they performed.
    // Keep the durable row for audit/history, but complete its outbox state without
    // injecting a prompt that would wake the same supervisor for its own action.
    // `supervisor` is resolved within the current factory session above, so a
    // sibling factory's supervisor does not qualify for suppression.
    let actor_is_owning_supervisor = actor == supervisor.agent_id || actor == supervisor.name;
    if actor_is_owning_supervisor {
        supervisor_queue
            .mark_prompt_delivered(notification_id)
            .map_err(|e| {
                format!(
                    "failed to stamp self-actor lifecycle notification as delivered \
                     (notification_id={notification_id}, transition_key={key}): {e}"
                )
            })?;
        return if already_existed {
            Ok(LifecyclePushResult::Recovered { notification_id })
        } else {
            Ok(LifecyclePushResult::Enqueued { notification_id })
        };
    }

    // cas-ecff: with an owning supervisor, prompt delivery store is required.
    // Missing/failed open must leave durable pending (no stamp) and surface repair.
    let Some(pq) = prompt_queue else {
        return Err(format!(
            "prompt_queue unavailable after durable enqueue \
             (notification_id={notification_id}, transition_key={key}); \
             durable event left pending (prompt_delivered_at unmarked). \
             Repair: drain_lifecycle_outbox once prompt_queue is available"
        ));
    };

    deliver_prompt_for_notification(
        supervisor_queue,
        pq,
        notification_id,
        &key,
        kind,
        task_id,
        task_title,
        old_status,
        new_status,
        actor,
        reason,
        factory_session.as_deref(),
        occurrence_id,
    )
    .map_err(|error| error.to_string())?;

    if already_existed {
        Ok(LifecyclePushResult::Recovered { notification_id })
    } else {
        Ok(LifecyclePushResult::Enqueued { notification_id })
    }
}

/// Source marker for the verification-dispatch handoff (cas-8725).
///
/// Deliberately NOT a `lifecycle-wake:` source: this row is not a task
/// lifecycle transition, and borrowing that prefix would make the lifecycle
/// wake's own corroboration rule mean two different things. The wake gate
/// classifies this row from its envelope and its Daemon origin stamp; the
/// source is for operators reading the queue.
pub const VERIFICATION_DISPATCH_SOURCE_PREFIX: &str = "verification-dispatch:";

/// Hand a just-created verification dispatch to the supervisor (cas-8725).
///
/// The close path already knows the dispatch id, its bound owner and its
/// deadline at the moment it refuses the close — so CAS emits the handoff
/// itself instead of printing instructions and hoping the worker relays them.
/// That relay is what measurably failed: the worker's hand-typed dispatch-id
/// message was free text, so it never woke the supervisor, and the close it
/// blocked stayed blocked until someone polled.
///
/// Stamped [`cas_store::QueueOrigin::Daemon`] because CAS composed every byte
/// of it. Idempotent on the dispatch id: a retried close for the same dispatch
/// re-uses the row rather than queueing a second copy.
pub fn emit_verification_dispatch_handoff(
    prompt_queue: &dyn PromptQueueStore,
    dispatch_id: &str,
    task_id: &str,
    owner_agent_id: &str,
    deadline: DateTime<Utc>,
    worker: &str,
    close_reason: Option<&str>,
) -> Result<(), String> {
    let body = crate::prompt_revalidation::verification_dispatch_envelope(
        dispatch_id,
        task_id,
        owner_agent_id,
        &deadline.to_rfc3339(),
        worker,
        close_reason,
    );
    let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let source = format!("{VERIFICATION_DISPATCH_SOURCE_PREFIX}{dispatch_id}");
    let summary = format!("Verification required: {task_id} (dispatch {dispatch_id})");
    prompt_queue
        .enqueue_idempotent(
            &source,
            "supervisor",
            &body,
            factory_session.as_deref(),
            Some(&summary),
            Some(NotificationPriority::High),
            &source,
            Some(&cas_store::QueueOrigin::Daemon),
        )
        .map(|_| ())
        .map_err(|error| format!("verification-dispatch handoff enqueue failed: {error}"))
}

/// Idempotent prompt handoff + stamp for one durable notification (cas-ecff).
///
/// Uses `lifecycle-outbox:{notification_id}` as prompt_queue dedupe_key so a
/// successful enqueue followed by stamp failure cannot produce a second prompt
/// row on replay.
#[allow(clippy::too_many_arguments)]
fn deliver_prompt_for_notification(
    supervisor_queue: &dyn SupervisorQueueStore,
    prompt_queue: &dyn PromptQueueStore,
    notification_id: i64,
    transition_key: &str,
    kind: LifecycleTransition,
    task_id: &str,
    task_title: &str,
    old_status: TaskStatus,
    new_status: TaskStatus,
    actor: &str,
    reason: Option<&str>,
    factory_session: Option<&str>,
    occurrence_id: &str,
) -> Result<(), TaskLifecycleGateError> {
    let reject = |message: String| TaskLifecycleGateError::PromptDelivery { message };
    let body = build_prompt_body(
        kind,
        task_id,
        task_title,
        old_status,
        new_status,
        actor,
        reason,
        notification_id,
        factory_session,
        occurrence_id,
    );
    let summary = format!("{}: {} ({})", kind.as_event_type(), task_id, occurrence_id);
    let source = lifecycle_prompt_source(kind, notification_id);
    let dedupe = lifecycle_prompt_dedupe_key(notification_id);

    prompt_queue
        .enqueue_idempotent(
            &source,
            "supervisor",
            &body,
            factory_session,
            Some(&summary),
            Some(kind.priority()),
            &dedupe,
            Some(&cas_store::QueueOrigin::Daemon),
        )
        .map_err(|e| {
            reject(format!(
                "prompt_queue write failed after durable enqueue \
                 (notification_id={notification_id}, transition_key={transition_key}): {e}"
            ))
        })?;

    supervisor_queue
        .mark_prompt_delivered(notification_id)
        .map_err(|e| {
            reject(format!(
                "failed to stamp prompt_delivered_at for notification_id={notification_id} \
                 (prompt may already be enqueued under dedupe_key={dedupe}; \
                 drain_lifecycle_outbox is safe): {e}"
            ))
        })?;
    Ok(())
}

/// Required payload fields for truthful outbox recovery (cas-3a47).
///
/// Missing/malformed fields **fail closed** — never fabricate Started / Open→InProgress.
fn require_payload_str<'a>(
    payload: &'a serde_json::Value,
    field: &str,
    notification_id: i64,
) -> Result<&'a str, String> {
    payload
        .get(field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!(
                "incomplete lifecycle payload id={notification_id}: missing or empty required field `{field}` \
                 (row left pending; will not fabricate defaults)"
            )
        })
}

fn parse_lifecycle_kind(
    transition: &str,
    notification_id: i64,
) -> Result<LifecycleTransition, String> {
    match transition {
        "task_started" => Ok(LifecycleTransition::Started),
        "task_blocked" => Ok(LifecycleTransition::Blocked),
        "task_ready" => Ok(LifecycleTransition::ReadyReopened),
        "task_close_rejected" => Ok(LifecycleTransition::CloseRejected),
        "task_awaiting_merge" => Ok(LifecycleTransition::AwaitingMerge),
        "task_closed" => Ok(LifecycleTransition::Closed),
        other => Err(format!(
            "incomplete lifecycle payload id={notification_id}: unknown transition `{other}` \
             (row left pending; will not fabricate Started)"
        )),
    }
}

fn parse_required_status(
    payload: &serde_json::Value,
    field: &str,
    notification_id: i64,
) -> Result<TaskStatus, String> {
    let s = require_payload_str(payload, field, notification_id)?;
    s.parse().map_err(|_| {
        format!(
            "incomplete lifecycle payload id={notification_id}: invalid {field}=`{s}` \
             (row left pending; will not fabricate Open/InProgress)"
        )
    })
}

/// Deliver prompt for a persisted durable outbox row (drain / restart recovery).
///
/// Fail-closed on corrupt/incomplete payloads (cas-3a47): leaves
/// `prompt_delivered_at` unmarked so a later fix can re-drain.
pub fn deliver_lifecycle_outbox_row(
    supervisor_queue: &dyn SupervisorQueueStore,
    prompt_queue: &dyn PromptQueueStore,
    notification: &SupervisorNotification,
) -> Result<LifecyclePushResult, String> {
    if notification.event_type != "task_lifecycle" {
        return Err(format!(
            "not a task_lifecycle row: event_type={}",
            notification.event_type
        ));
    }
    if notification.prompt_delivered_at.is_some() {
        return Ok(LifecyclePushResult::AlreadyComplete {
            notification_id: notification.id,
        });
    }

    let payload: serde_json::Value = serde_json::from_str(&notification.payload).map_err(|e| {
        format!(
            "corrupt lifecycle payload id={}: {e} (row left pending)",
            notification.id
        )
    })?;

    let task_id = require_payload_str(&payload, "task_id", notification.id)?;
    // title/actor may be empty string for legacy rows, but keys must be present as strings.
    let task_title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            format!(
                "incomplete lifecycle payload id={}: missing field `title` (row left pending)",
                notification.id
            )
        })?;
    let actor = require_payload_str(&payload, "actor", notification.id)?;
    let occurrence_id = require_payload_str(&payload, "occurrence_id", notification.id)?;
    let key = payload
        .get("transition_key")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or(notification
            .transition_key
            .as_deref()
            .filter(|s| !s.is_empty()))
        .ok_or_else(|| {
            format!(
                "incomplete lifecycle payload id={}: missing transition_key \
                 (row left pending)",
                notification.id
            )
        })?;
    let factory_session = payload.get("factory_session").and_then(|v| {
        // null is ok (non-factory); wrong type is not
        match v {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    });
    let reason = payload.get("reason").and_then(|v| v.as_str());
    let transition = require_payload_str(&payload, "transition", notification.id)?;
    let kind = parse_lifecycle_kind(transition, notification.id)?;
    let old_status = parse_required_status(&payload, "old_status", notification.id)?;
    let new_status = parse_required_status(&payload, "new_status", notification.id)?;

    deliver_prompt_for_notification(
        supervisor_queue,
        prompt_queue,
        notification.id,
        key,
        kind,
        task_id,
        task_title,
        old_status,
        new_status,
        actor,
        reason,
        factory_session,
        occurrence_id,
    )
    .map_err(|error| error.to_string())?;

    Ok(LifecyclePushResult::Recovered {
        notification_id: notification.id,
    })
}

/// Drain all pending lifecycle outbox rows (callable repair + daemon auto-drain).
///
/// Does **not** re-run task mutations. Safe after process restart.
pub fn drain_lifecycle_outbox(
    supervisor_queue: &dyn SupervisorQueueStore,
    prompt_queue: &dyn PromptQueueStore,
    limit: usize,
) -> Result<LifecycleOutboxDrainReport, String> {
    let pending = supervisor_queue
        .list_pending_lifecycle_outbox(limit)
        .map_err(|e| format!("list pending lifecycle outbox: {e}"))?;

    let mut report = LifecycleOutboxDrainReport {
        attempted: pending.len(),
        ..Default::default()
    };

    for n in pending {
        match deliver_lifecycle_outbox_row(supervisor_queue, prompt_queue, &n) {
            Ok(LifecyclePushResult::Recovered { .. })
            | Ok(LifecyclePushResult::Enqueued { .. }) => {
                report.recovered += 1;
            }
            Ok(LifecyclePushResult::AlreadyComplete { .. }) => {
                report.already_complete += 1;
            }
            Ok(LifecyclePushResult::NoSupervisor) => {
                report.failed += 1;
                report
                    .errors
                    .push(format!("id={}: unexpected NoSupervisor during drain", n.id));
            }
            Err(e) => {
                report.failed += 1;
                report.errors.push(format!("id={}: {e}", n.id));
            }
        }
    }
    Ok(report)
}

impl CasCore {
    /// Push a lifecycle transition after a successful task mutation
    /// (cas-062d / cas-17e4 / cas-ecff).
    ///
    /// `occurrence_id` must identify this mutation (typically
    /// [`occurrence_from_updated_at`] of the post-write `updated_at`).
    ///
    /// When an owning supervisor exists, prompt_queue **must** open successfully;
    /// open failure surfaces after durable insert without stamping delivered.
    /// Returns `Err` when durable write or prompt outbox step fails — callers must
    /// surface via [`lifecycle_push_failure_message`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_task_lifecycle(
        &self,
        task_id: &str,
        task_title: &str,
        old_status: TaskStatus,
        new_status: TaskStatus,
        actor: &str,
        reason: Option<&str>,
        kind: LifecycleTransition,
        occurrence_id: &str,
    ) -> Result<LifecyclePushResult, String> {
        let agent_store = self
            .open_agent_store()
            .map_err(|e| format!("agent store: {e}"))?;
        let sq = open_supervisor_queue_store(&self.cas_root)
            .map_err(|e| format!("supervisor_queue open: {e}"))?;
        // Open is fallible: pass None so emit leaves durable pending when a
        // supervisor exists. Never map open-fail to "skip prompt and stamp success".
        let pq_open = open_prompt_queue_store(&self.cas_root);
        let open_err = pq_open.as_ref().err().map(|e| e.to_string());
        let pq_store = pq_open.ok();
        let pq = pq_store
            .as_ref()
            .map(|a| a.as_ref() as &dyn PromptQueueStore);
        match emit_task_lifecycle_transition(
            sq.as_ref(),
            pq,
            agent_store.as_ref(),
            task_id,
            task_title,
            old_status,
            new_status,
            actor,
            reason,
            kind,
            occurrence_id,
        ) {
            Err(e) if e.contains("prompt_queue unavailable") => {
                if let Some(open_err) = open_err {
                    Err(format!("{e}; open error: {open_err}"))
                } else {
                    Err(e)
                }
            }
            other => other,
        }
    }

    /// Callable repair: drain pending lifecycle outbox to exactly-once prompt delivery.
    ///
    /// Safe after process restart. Factory daemon also invokes this automatically.
    pub fn drain_lifecycle_outbox(&self) -> Result<LifecycleOutboxDrainReport, String> {
        let sq = open_supervisor_queue_store(&self.cas_root)
            .map_err(|e| format!("supervisor_queue open: {e}"))?;
        let pq = open_prompt_queue_store(&self.cas_root)
            .map_err(|e| format!("prompt_queue open: {e}"))?;
        drain_lifecycle_outbox(sq.as_ref(), pq.as_ref(), 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use cas_store::{
        PromptQueueStore, SpawnQueueStore, SqliteAgentStore, SqlitePromptQueueStore,
        SqliteSpawnQueueStore, SqliteSupervisorQueueStore, SupervisorQueueStore, TaskStore,
    };
    use cas_types::{Agent, AgentRole, AgentStatus, Task, TaskStatus};
    use tempfile::TempDir;

    // cas-acb4: `CAS_FACTORY_SESSION` mutations here go through `TestEnvGuard`,
    // which owns the same process-wide lock the old local `env_lock()` helper
    // took AND restores every variable on drop, including during unwind. Do not
    // reintroduce a bare lock + manual save/restore pair: a panic between the
    // set and the restore leaked the session into every later test in this
    // binary, and the tests that then failed were in unrelated modules.

    #[test]
    fn transition_key_includes_session_and_occurrence() {
        let a = transition_key(
            "cas-1",
            TaskStatus::InProgress,
            TaskStatus::Closed,
            Some("sess-a"),
            LifecycleTransition::Closed,
            "occ-1",
        );
        let b = transition_key(
            "cas-1",
            TaskStatus::InProgress,
            TaskStatus::Closed,
            Some("sess-b"),
            LifecycleTransition::Closed,
            "occ-1",
        );
        let c = transition_key(
            "cas-1",
            TaskStatus::InProgress,
            TaskStatus::Closed,
            Some("sess-a"),
            LifecycleTransition::Closed,
            "occ-2",
        );
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(a.contains("sess-a"));
        assert!(a.contains("occ-1"));
    }

    #[test]
    fn transition_key_stable_for_same_occurrence() {
        let a = transition_key(
            "cas-1",
            TaskStatus::Open,
            TaskStatus::InProgress,
            Some("s"),
            LifecycleTransition::Started,
            "t1",
        );
        let b = transition_key(
            "cas-1",
            TaskStatus::Open,
            TaskStatus::InProgress,
            Some("s"),
            LifecycleTransition::Started,
            "t1",
        );
        assert_eq!(a, b);
    }

    #[test]
    fn two_start_cycles_get_distinct_keys() {
        let start1 = transition_key(
            "cas-1",
            TaskStatus::Open,
            TaskStatus::InProgress,
            Some("s"),
            LifecycleTransition::Started,
            "t-start-1",
        );
        let start2 = transition_key(
            "cas-1",
            TaskStatus::Open,
            TaskStatus::InProgress,
            Some("s"),
            LifecycleTransition::Started,
            "t-start-2",
        );
        assert_ne!(start1, start2);
    }

    #[test]
    fn event_types_are_stable_strings() {
        assert_eq!(LifecycleTransition::Started.as_event_type(), "task_started");
        assert_eq!(LifecycleTransition::Blocked.as_event_type(), "task_blocked");
        assert_eq!(
            LifecycleTransition::ReadyReopened.as_event_type(),
            "task_ready"
        );
        assert_eq!(
            LifecycleTransition::CloseRejected.as_event_type(),
            "task_close_rejected"
        );
        assert_eq!(
            LifecycleTransition::AwaitingMerge.as_event_type(),
            "task_awaiting_merge"
        );
        assert_eq!(LifecycleTransition::Closed.as_event_type(), "task_closed");
    }

    /// cas-f02b (GH #101): the transitions that park a worker behind supervisor
    /// action are exactly the ones allowed to wake an idle supervisor. Progress
    /// FYI must not — that noise is what cas-dab2's exclusion stopped.
    #[test]
    fn only_parked_transitions_wake_an_idle_supervisor() {
        assert!(LifecycleTransition::AwaitingMerge.wakes_idle_supervisor());
        assert!(LifecycleTransition::CloseRejected.wakes_idle_supervisor());
        assert!(LifecycleTransition::Blocked.wakes_idle_supervisor());
        assert!(!LifecycleTransition::Started.wakes_idle_supervisor());
        assert!(!LifecycleTransition::ReadyReopened.wakes_idle_supervisor());
        assert!(!LifecycleTransition::Closed.wakes_idle_supervisor());
    }

    /// cas-f02b: wake eligibility travels on the row's `source`, so the daemon
    /// never has to sniff prompt text — and a row replayed by the outbox drain
    /// carries the same eligibility as the original.
    #[test]
    fn lifecycle_prompt_source_encodes_wake_eligibility() {
        let wake = lifecycle_prompt_source(LifecycleTransition::AwaitingMerge, 7);
        assert_eq!(wake, "lifecycle-wake:7");
        assert!(is_lifecycle_wake_source(&wake));

        let fyi = lifecycle_prompt_source(LifecycleTransition::Closed, 7);
        assert_eq!(fyi, "lifecycle:7");
        assert!(!is_lifecycle_wake_source(&fyi));
        // The non-waking form must not be a prefix-match false positive.
        assert!(!is_lifecycle_wake_source("lifecycle:70"));
    }

    #[test]
    fn auto_unblock_wakes_fresh_idle_assignee_once_and_records_note() {
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-auto-wake")]);
        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        let mut worker = agent_in_session(
            "worker-auto-id",
            "quiet-worker",
            AgentRole::Worker,
            "sess-auto-wake",
        );
        worker.status = AgentStatus::Idle;
        agents.register(&worker).unwrap();

        let task_store = cas_store::SqliteTaskStore::open(temp.path()).unwrap();
        task_store.init().unwrap();
        let mut task = Task::new("cas-auto-wake".into(), "Wake me after blocker close".into());
        task.status = TaskStatus::Blocked;
        task.assignee = Some("quiet-worker".into());
        task_store.add(&task).unwrap();

        let result = queue_auto_unblock_worker_wake(
            temp.path(),
            &task,
            "cas-blocker",
            "unblock-occurrence-1",
        )
        .unwrap();
        assert!(matches!(result, AutoUnblockWakeResult::Queued { .. }));

        // A repeated attempt represents a replay of the same unblock event;
        // the idempotent queue row is the only prompt.
        queue_auto_unblock_worker_wake(temp.path(), &task, "cas-blocker", "unblock-occurrence-1")
            .unwrap();
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "supervisor");
        assert_eq!(rows[0].target, "quiet-worker");
        assert!(!rows[0].urgent);
        assert_eq!(
            rows[0].prompt,
            "Task cas-auto-wake is now unblocked (blocker cas-blocker closed): run task start id=cas-auto-wake"
        );
        assert!(
            task_store
                .get("cas-auto-wake")
                .unwrap()
                .notes
                .contains("Auto-unblock wake queued for worker 'quiet-worker'")
        );
    }

    #[test]
    fn auto_unblock_without_assignee_does_not_queue_a_message() {
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-auto-none")]);
        let temp = TempDir::new().unwrap();
        let task_store = cas_store::SqliteTaskStore::open(temp.path()).unwrap();
        task_store.init().unwrap();
        let mut task = Task::new("cas-auto-none".into(), "No worker yet".into());
        task.status = TaskStatus::Blocked;
        task_store.add(&task).unwrap();

        let result = queue_auto_unblock_worker_wake(
            temp.path(),
            &task,
            "cas-blocker",
            "unblock-occurrence-none",
        )
        .unwrap();
        assert!(matches!(result, AutoUnblockWakeResult::Skipped { .. }));
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        assert_eq!(queue.pending_count().unwrap(), 0);
        assert!(task_store.get("cas-auto-none").unwrap().notes.contains(
            "Auto-unblock wake skipped: no live assignee or matching pre-assignment receipt"
        ));
    }

    #[test]
    fn auto_unblock_stale_assignee_does_not_queue_a_message_and_records_note() {
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-auto-stale")]);
        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        let mut worker = agent_in_session(
            "worker-stale-id",
            "stale-worker",
            AgentRole::Worker,
            "sess-auto-stale",
        );
        worker.status = AgentStatus::Idle;
        worker.last_heartbeat = chrono::Utc::now() - chrono::Duration::seconds(301);
        agents.register(&worker).unwrap();

        let task_store = cas_store::SqliteTaskStore::open(temp.path()).unwrap();
        task_store.init().unwrap();
        let mut task = Task::new("cas-auto-stale".into(), "Stale worker".into());
        task.status = TaskStatus::Blocked;
        task.assignee = Some("stale-worker".into());
        task_store.add(&task).unwrap();

        let result = queue_auto_unblock_worker_wake(
            temp.path(),
            &task,
            "cas-blocker",
            "unblock-occurrence-stale",
        )
        .unwrap();
        assert!(matches!(result, AutoUnblockWakeResult::Skipped { .. }));
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        assert_eq!(queue.pending_count().unwrap(), 0);
        let notes = task_store.get("cas-auto-stale").unwrap().notes;
        assert!(
            notes.contains("stale-worker")
                && notes.contains("no registered worker with a fresh heartbeat")
        );
    }

    #[test]
    fn auto_unblock_uses_named_spawn_receipt_when_task_is_unassigned() {
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-auto-preassign")]);
        let temp = TempDir::new().unwrap();
        let task_store = cas_store::SqliteTaskStore::open(temp.path()).unwrap();
        task_store.init().unwrap();
        let mut task = Task::new("cas-auto-preassign".into(), "Spawn receipt target".into());
        task.status = TaskStatus::Blocked;
        task_store.add(&task).unwrap();

        let spawn_queue = SqliteSpawnQueueStore::open(temp.path()).unwrap();
        spawn_queue.init().unwrap();
        spawn_queue
            .enqueue_spawn(
                1,
                &["spawned-worker".into()],
                false,
                None,
                Some("sess-auto-preassign"),
                Some("cas-auto-preassign"),
            )
            .unwrap();

        let result = queue_auto_unblock_worker_wake(
            temp.path(),
            &task,
            "cas-blocker",
            "unblock-occurrence-preassign",
        )
        .unwrap();
        assert!(matches!(result, AutoUnblockWakeResult::Queued { .. }));
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, "spawned-worker");
    }

    /// cas-f02b end to end: parking a task as AwaitingMerge enqueues a
    /// supervisor-targeted prompt row that the dispatch layer will recognise
    /// as a wake — the push the factory prompt promises, instead of a file
    /// write nobody is polling.
    #[test]
    fn awaiting_merge_push_enqueues_a_wake_eligible_supervisor_row() {
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-wake")]);

        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        agents
            .register(&agent_in_session(
                "sup-wake",
                "cosmic-bear-43",
                AgentRole::Supervisor,
                "sess-wake",
            ))
            .unwrap();
        let sq = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        sq.init().unwrap();
        let pq = SqlitePromptQueueStore::open(temp.path()).unwrap();
        pq.init().unwrap();

        let result = emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-f02b",
            "MERGE REQUIRED close rejections never reach the supervisor",
            TaskStatus::InProgress,
            TaskStatus::AwaitingMerge,
            "swift-fox",
            Some("MERGE REQUIRED"),
            LifecycleTransition::AwaitingMerge,
            "occ-wake",
        )
        .expect("awaiting_merge push succeeds");
        assert!(matches!(result, LifecyclePushResult::Enqueued { .. }));

        let rows = pq.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1, "exactly one supervisor push");
        assert_eq!(rows[0].target, "supervisor");
        assert!(
            is_lifecycle_wake_source(&rows[0].source),
            "the dispatch layer must be able to tell this apart from ordinary \
             supervisor-addressed traffic: source={}",
            rows[0].source
        );
        assert_eq!(rows[0].priority, NotificationPriority::High);
        assert!(
            rows[0].prompt.contains("task_awaiting_merge") && rows[0].prompt.contains("cas-f02b"),
            "body must self-identify as a lifecycle signal (it is injected into \
             the supervisor pane, so it can never read as operator input): {}",
            rows[0].prompt
        );
    }

    #[test]
    fn failure_message_never_claims_task_op_retry_is_safe() {
        let msg = lifecycle_push_failure_message(
            "cas-x",
            TaskStatus::InProgress,
            LifecycleTransition::Started,
            "key",
            "prompt failed",
        );
        assert!(msg.contains("already in_progress"));
        assert!(msg.contains("Do NOT re-run"));
        assert!(!msg.to_lowercase().contains("retry is safe"));
        assert!(msg.contains("transition_key=key"));
        assert!(
            msg.contains("drain_lifecycle_outbox"),
            "must name callable repair path: {msg}"
        );
    }

    #[test]
    fn missing_prompt_queue_leaves_durable_unmarked() {
        // cas-acb4: one guard owns the shared env lock AND restores on
        // unwind. The previous save/restore pair leaked the variable to
        // every later test in this binary whenever an assertion panicked
        // between the two halves.
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-no-pq")]);

        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        agents
            .register(&agent_in_session(
                "sup-np",
                "sup-np",
                AgentRole::Supervisor,
                "sess-no-pq",
            ))
            .unwrap();
        let sq = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        sq.init().unwrap();

        let err = emit_task_lifecycle_transition(
            &sq,
            None, // prompt store open failed / unavailable
            &agents,
            "cas-np",
            "NP",
            TaskStatus::Open,
            TaskStatus::InProgress,
            "w",
            None,
            LifecycleTransition::Started,
            "occ-np",
        )
        .expect_err("must fail when prompt_queue missing with supervisor");
        assert!(err.contains("prompt_queue unavailable"), "{err}");
        assert!(err.contains("drain_lifecycle_outbox"), "{err}");

        let key = "cas-np:open:in_progress:sess-no-pq:task_started:occ-np";
        let row = sq.get_by_transition_key(key).unwrap().expect("durable row");
        assert!(
            row.prompt_delivered_at.is_none(),
            "must NOT stamp success when prompt path missing"
        );

        // Drain with real prompt store repairs exactly once.
        let pq = SqlitePromptQueueStore::open(temp.path()).unwrap();
        pq.init().unwrap();
        let report = drain_lifecycle_outbox(&sq, &pq, 10).unwrap();
        assert_eq!(report.recovered, 1);
        assert_eq!(report.failed, 0);
        assert!(
            sq.get_by_transition_key(key)
                .unwrap()
                .unwrap()
                .prompt_delivered_at
                .is_some()
        );
        assert_eq!(pq.pending_count().unwrap(), 1);

        // Second drain: no duplicate prompt.
        let report2 = drain_lifecycle_outbox(&sq, &pq, 10).unwrap();
        assert_eq!(report2.attempted, 0);
        assert_eq!(pq.pending_count().unwrap(), 1);
    }

    /// Stamp fails after enqueue: replay must not create a second prompt row.
    #[test]
    fn stamp_failure_replay_does_not_duplicate_prompt() {
        // cas-acb4: one guard owns the shared env lock AND restores on
        // unwind. The previous save/restore pair leaked the variable to
        // every later test in this binary whenever an assertion panicked
        // between the two halves.
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-stamp")]);

        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        agents
            .register(&agent_in_session(
                "sup-st",
                "sup-st",
                AgentRole::Supervisor,
                "sess-stamp",
            ))
            .unwrap();
        let sq = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        sq.init().unwrap();
        let pq = SqlitePromptQueueStore::open(temp.path()).unwrap();
        pq.init().unwrap();

        // Durable insert only (simulate after enqueue-before-stamp crash by
        // enqueue_idempotent then leaving stamp unset).
        let key = "cas-st:open:in_progress:sess-stamp:task_started:occ-st";
        let id = match sq
            .notify_idempotent(
                "sup-st",
                "task_lifecycle",
                r#"{"task_id":"cas-st","title":"ST","old_status":"open","new_status":"in_progress","actor":"w","transition":"task_started","factory_session":"sess-stamp","occurrence_id":"occ-st","transition_key":"cas-st:open:in_progress:sess-stamp:task_started:occ-st"}"#,
                NotificationPriority::Normal,
                key,
            )
            .unwrap()
        {
            NotifyIdempotentResult::Created(id) => id,
            other => panic!("expected Created: {other:?}"),
        };

        // First enqueue succeeds; stamp intentionally skipped (partial failure).
        pq.enqueue_idempotent(
            &format!("lifecycle:{id}"),
            "supervisor",
            "body",
            Some("sess-stamp"),
            Some("sum"),
            None,
            &lifecycle_prompt_dedupe_key(id),
            None,
        )
        .unwrap();
        assert_eq!(pq.pending_count().unwrap(), 1);
        assert!(
            sq.get_by_transition_key(key)
                .unwrap()
                .unwrap()
                .prompt_delivered_at
                .is_none()
        );

        // Drain/replay: must stamp without second prompt row.
        let report = drain_lifecycle_outbox(&sq, &pq, 10).unwrap();
        assert_eq!(report.recovered, 1, "errors={:?}", report.errors);
        assert_eq!(pq.pending_count().unwrap(), 1, "exactly-once prompt");
        assert!(
            sq.get_by_transition_key(key)
                .unwrap()
                .unwrap()
                .prompt_delivered_at
                .is_some()
        );

        // emit same occurrence: AlreadyComplete, still one prompt.
        let r = emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-st",
            "ST",
            TaskStatus::Open,
            TaskStatus::InProgress,
            "w",
            None,
            LifecycleTransition::Started,
            "occ-st",
        )
        .unwrap();
        assert!(matches!(r, LifecyclePushResult::AlreadyComplete { .. }));
        assert_eq!(pq.pending_count().unwrap(), 1);
    }

    /// cas-3a47 AC4: malformed payload stays pending; never fabricates Started/Open→InProgress.
    #[test]
    fn corrupt_payload_stays_pending_with_specific_error() {
        let temp = TempDir::new().unwrap();
        let sq = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        sq.init().unwrap();
        let pq = SqlitePromptQueueStore::open(temp.path()).unwrap();
        pq.init().unwrap();

        let id_bad = match sq
            .notify_idempotent(
                "sup",
                "task_lifecycle",
                "not-json",
                NotificationPriority::Normal,
                "key-corrupt",
            )
            .unwrap()
        {
            NotifyIdempotentResult::Created(id) => id,
            other => panic!("{other:?}"),
        };
        let row = sq
            .list_pending_lifecycle_outbox(10)
            .unwrap()
            .into_iter()
            .find(|n| n.id == id_bad)
            .unwrap();
        let err = deliver_lifecycle_outbox_row(&sq, &pq, &row).unwrap_err();
        assert!(err.contains("corrupt lifecycle payload"), "{err}");
        assert!(err.contains("left pending"), "{err}");
        assert!(
            sq.get_by_transition_key("key-corrupt")
                .unwrap()
                .unwrap()
                .prompt_delivered_at
                .is_none()
        );
        assert_eq!(pq.pending_count().unwrap(), 0);

        let id_inc = match sq
            .notify_idempotent(
                "sup",
                "task_lifecycle",
                r#"{"task_id":"cas-x"}"#,
                NotificationPriority::Normal,
                "key-incomplete",
            )
            .unwrap()
        {
            NotifyIdempotentResult::Created(id) => id,
            other => panic!("{other:?}"),
        };
        let row = sq
            .list_pending_lifecycle_outbox(10)
            .unwrap()
            .into_iter()
            .find(|n| n.id == id_inc)
            .unwrap();
        let err = deliver_lifecycle_outbox_row(&sq, &pq, &row).unwrap_err();
        assert!(
            err.contains("incomplete lifecycle payload") || err.contains("missing"),
            "{err}"
        );
        assert!(
            sq.get_by_transition_key("key-incomplete")
                .unwrap()
                .unwrap()
                .prompt_delivered_at
                .is_none()
        );
        assert_eq!(
            pq.pending_count().unwrap(),
            0,
            "no fabricated prompt delivery"
        );

        let id_unk = match sq
            .notify_idempotent(
                "sup",
                "task_lifecycle",
                r#"{"task_id":"cas-x","title":"t","actor":"a","occurrence_id":"o","transition":"task_magic","old_status":"open","new_status":"closed","transition_key":"key-unk"}"#,
                NotificationPriority::Normal,
                "key-unk",
            )
            .unwrap()
        {
            NotifyIdempotentResult::Created(id) => id,
            other => panic!("{other:?}"),
        };
        let row = sq
            .list_pending_lifecycle_outbox(10)
            .unwrap()
            .into_iter()
            .find(|n| n.id == id_unk)
            .unwrap();
        let err = deliver_lifecycle_outbox_row(&sq, &pq, &row).unwrap_err();
        assert!(err.contains("unknown transition"), "{err}");
        assert!(err.contains("will not fabricate Started"), "{err}");
        assert!(
            sq.get_by_transition_key("key-unk")
                .unwrap()
                .unwrap()
                .prompt_delivered_at
                .is_none()
        );
    }

    /// cas-3a47: valid drains; malformed stays pending; concurrent drain no dup.
    #[test]
    fn drain_mixed_valid_and_corrupt_rows_exactly_once() {
        let temp = TempDir::new().unwrap();
        let sq = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        sq.init().unwrap();
        let pq = SqlitePromptQueueStore::open(temp.path()).unwrap();
        pq.init().unwrap();

        let good_payload = r#"{
            "task_id":"cas-good",
            "title":"Good",
            "old_status":"open",
            "new_status":"in_progress",
            "actor":"w",
            "transition":"task_started",
            "factory_session":"s",
            "occurrence_id":"occ-g",
            "transition_key":"key-good"
        }"#;
        sq.notify_idempotent(
            "sup",
            "task_lifecycle",
            good_payload,
            NotificationPriority::Normal,
            "key-good",
        )
        .unwrap();
        sq.notify_idempotent(
            "sup",
            "task_lifecycle",
            r#"{"task_id":"cas-bad"}"#,
            NotificationPriority::Normal,
            "key-bad",
        )
        .unwrap();

        let report = drain_lifecycle_outbox(&sq, &pq, 20).unwrap();
        assert_eq!(report.recovered, 1, "errors={:?}", report.errors);
        assert_eq!(report.failed, 1, "corrupt must fail closed");
        assert_eq!(pq.pending_count().unwrap(), 1);
        assert!(
            sq.get_by_transition_key("key-good")
                .unwrap()
                .unwrap()
                .prompt_delivered_at
                .is_some()
        );
        assert!(
            sq.get_by_transition_key("key-bad")
                .unwrap()
                .unwrap()
                .prompt_delivered_at
                .is_none()
        );

        let report2 = drain_lifecycle_outbox(&sq, &pq, 20).unwrap();
        assert_eq!(report2.recovered, 0);
        assert_eq!(report2.failed, 1);
        assert_eq!(pq.pending_count().unwrap(), 1);

        sq.notify_idempotent(
            "sup",
            "task_lifecycle",
            r#"{
            "task_id":"cas-c2",
            "title":"C2",
            "old_status":"blocked",
            "new_status":"open",
            "actor":"w",
            "transition":"task_ready",
            "occurrence_id":"occ-c2",
            "transition_key":"key-c2"
        }"#,
            NotificationPriority::High,
            "key-c2",
        )
        .unwrap();

        let path = temp.path().to_path_buf();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let sq = SqliteSupervisorQueueStore::open(&path).unwrap();
                    sq.init().unwrap();
                    let pq = SqlitePromptQueueStore::open(&path).unwrap();
                    pq.init().unwrap();
                    drain_lifecycle_outbox(&sq, &pq, 20).unwrap()
                })
            })
            .collect();
        for h in handles {
            let _ = h.join().unwrap();
        }
        assert_eq!(
            pq.pending_count().unwrap(),
            2,
            "concurrent drain must not duplicate"
        );
        assert!(
            sq.get_by_transition_key("key-c2")
                .unwrap()
                .unwrap()
                .prompt_delivered_at
                .is_some()
        );
    }

    fn agent_in_session(id: &str, name: &str, role: AgentRole, session: &str) -> Agent {
        let mut a = Agent::new(id.to_string(), name.to_string());
        a.role = role;
        a.status = AgentStatus::Active;
        a.factory_session = Some(session.to_string());
        a
    }

    /// cas-7787 (GH #160), acceptance criterion 2: a supervisor whose harness
    /// session id changes mid-factory-session must still receive
    /// `awaiting_merge` relays — they have to land at the SUCCESSOR session.
    ///
    /// Reproduces the identity shape from the reported incident: the pane
    /// `smooth-octopus-84` restarts and re-registers under the same name with
    /// a new agent id, leaving two live rows in one factory session. Ids are
    /// chosen so the pre-restart identity sorts FIRST lexicographically —
    /// which is exactly how the old `name`-then-`id` tiebreak used to pick a
    /// winner, and would have addressed every later relay to the session the
    /// operator had already left.
    #[test]
    fn relays_follow_the_supervisor_across_a_mid_session_restart() {
        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();

        let mut before_restart = agent_in_session(
            "a-pre-restart-session",
            "smooth-octopus-84",
            AgentRole::Supervisor,
            "cas-src-fast-pelican-83",
        );
        before_restart.registered_at = chrono::Utc::now() - chrono::Duration::minutes(30);
        agents.register(&before_restart).unwrap();

        // The restart registered TWO rows 157ms apart under the same pane
        // name (3f2b69fa then ad32fcde in the real store). Reproduce that
        // duplicate exactly — a registry cleaned of the dups would not be the
        // failure shape, and tolerating them is half of what makes this pass.
        let restart_at = chrono::Utc::now() - chrono::Duration::seconds(1);
        let mut restart_first = agent_in_session(
            "m-restart-row-one",
            "smooth-octopus-84",
            AgentRole::Supervisor,
            "cas-src-fast-pelican-83",
        );
        restart_first.registered_at = restart_at;
        agents.register(&restart_first).unwrap();

        let mut after_restart = agent_in_session(
            "z-post-restart-session",
            "smooth-octopus-84",
            AgentRole::Supervisor,
            "cas-src-fast-pelican-83",
        );
        after_restart.registered_at = restart_at + chrono::Duration::milliseconds(157);
        agents.register(&after_restart).unwrap();

        // Three live rows share the name, exactly as the incident store did.
        assert_eq!(
            agents
                .list(None)
                .unwrap()
                .iter()
                .filter(|a| a.name == "smooth-octopus-84")
                .count(),
            3,
            "the duplicate same-name rows must be present — that IS the failure shape"
        );

        let resolved = resolve_owning_supervisor(&agents, Some("cas-src-fast-pelican-83")).unwrap();
        assert_eq!(
            resolved.agent_id, "z-post-restart-session",
            "an awaiting_merge relay emitted after the restart must be addressed to the \
             successor session — not to the identity the supervisor left behind, and not \
             to whichever duplicate row happens to sort first"
        );
        assert_eq!(resolved.name, "smooth-octopus-84");

        // End-to-end: emitting a real awaiting_merge transition must put the
        // durable row and its prompt on the SUCCESSOR, not merely avoid
        // suppressing it.
        let temp_q = TempDir::new().unwrap();
        let sq = SqliteSupervisorQueueStore::open(temp_q.path()).unwrap();
        sq.init().unwrap();
        let pq = SqlitePromptQueueStore::open(temp_q.path()).unwrap();
        pq.init().unwrap();

        unsafe { std::env::set_var("CAS_FACTORY_SESSION", "cas-src-fast-pelican-83") };
        let result = emit_task_lifecycle_transition(
            &sq,
            Some(&pq),
            &agents,
            "cas-fe23",
            "a parked lane",
            TaskStatus::InProgress,
            TaskStatus::AwaitingMerge,
            "happy-spider-96",
            Some("ready to merge"),
            LifecycleTransition::AwaitingMerge,
            "2026-08-07T18:51:51+00:00",
        )
        .unwrap();
        unsafe { std::env::remove_var("CAS_FACTORY_SESSION") };

        let notification_id = match result {
            LifecyclePushResult::Enqueued { notification_id } => notification_id,
            other => panic!("expected a fresh enqueue, got {other:?}"),
        };
        let successor_queue = sq.list_pending("z-post-restart-session").unwrap();
        assert!(
            successor_queue.iter().any(|row| row.id == notification_id),
            "the durable relay must be addressed to the supervisor that exists after the \
             restart, got {successor_queue:?}"
        );
        assert!(
            sq.list_pending("a-pre-restart-session").unwrap().is_empty(),
            "nothing may be queued at the identity the supervisor abandoned"
        );
        let stored = sq
            .get_by_transition_key(&transition_key(
                "cas-fe23",
                TaskStatus::InProgress,
                TaskStatus::AwaitingMerge,
                Some("cas-src-fast-pelican-83"),
                LifecycleTransition::AwaitingMerge,
                "2026-08-07T18:51:51+00:00",
            ))
            .unwrap()
            .expect("durable row exists for this occurrence");
        assert!(
            stored.prompt_delivered_at.is_some(),
            "the prompt half must have been handed to the queue, not left pending"
        );
        assert_eq!(
            pq.pending_count().unwrap(),
            1,
            "exactly one wake-eligible prompt row must be queued for the supervisor pane"
        );
    }

    /// The recency tiebreak must never outrank liveness: a freshly registered
    /// but dead identity must not steal relays from a live supervisor.
    #[test]
    fn a_newer_but_shutdown_supervisor_row_does_not_capture_relays() {
        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();

        // Keep names ordered opposite to registration recency so this still
        // proves the liveness rank rather than the legacy name tiebreak. The
        // store now retires active same-name supervisors on registration.
        let mut live = agent_in_session("live-id", "z-sup", AgentRole::Supervisor, "sess");
        live.registered_at = chrono::Utc::now() - chrono::Duration::minutes(10);
        live.status = AgentStatus::Active;
        agents.register(&live).unwrap();

        let mut newer_dead =
            agent_in_session("newer-dead-id", "a-sup", AgentRole::Supervisor, "sess");
        newer_dead.registered_at = chrono::Utc::now();
        newer_dead.status = AgentStatus::Shutdown;
        agents.register(&newer_dead).unwrap();

        let resolved = resolve_owning_supervisor(&agents, Some("sess")).unwrap();
        assert_eq!(
            resolved.agent_id, "live-id",
            "liveness still outranks recency — a shut-down successor must not swallow relays"
        );
    }

    #[test]
    fn resolve_owning_supervisor_session_isolation() {
        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        agents
            .register(&agent_in_session(
                "sup-a-id",
                "sup-a",
                AgentRole::Supervisor,
                "sess-a",
            ))
            .unwrap();
        agents
            .register(&agent_in_session(
                "sup-b-id",
                "sup-b",
                AgentRole::Supervisor,
                "sess-b",
            ))
            .unwrap();
        agents
            .register(&agent_in_session(
                "worker-a",
                "worker-a",
                AgentRole::Worker,
                "sess-a",
            ))
            .unwrap();

        let a = resolve_owning_supervisor(&agents, Some("sess-a")).unwrap();
        assert_eq!(a.agent_id, "sup-a-id");
        assert_eq!(a.name, "sup-a");

        let b = resolve_owning_supervisor(&agents, Some("sess-b")).unwrap();
        assert_eq!(b.agent_id, "sup-b-id");

        assert!(resolve_owning_supervisor(&agents, Some("sess-empty")).is_none());
    }

    #[test]
    fn emit_enqueues_once_and_suppresses_same_occurrence() {
        // SAFETY: process-wide Cassy env lock held for this test body.
        // cas-acb4: one guard owns the shared env lock AND restores on
        // unwind. The previous save/restore pair leaked the variable to
        // every later test in this binary whenever an assertion panicked
        // between the two halves.
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-emit")]);

        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        agents
            .register(&agent_in_session(
                "sup-emit",
                "sup-emit-name",
                AgentRole::Supervisor,
                "sess-emit",
            ))
            .unwrap();
        let sq = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        sq.init().unwrap();
        let pq = SqlitePromptQueueStore::open(temp.path()).unwrap();
        pq.init().unwrap();

        let r1 = emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-t1",
            "Title",
            TaskStatus::Open,
            TaskStatus::InProgress,
            "worker-1",
            None,
            LifecycleTransition::Started,
            "occ-1",
        )
        .unwrap();
        let r2 = emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-t1",
            "Title",
            TaskStatus::Open,
            TaskStatus::InProgress,
            "worker-1",
            None,
            LifecycleTransition::Started,
            "occ-1",
        )
        .unwrap();

        match (r1, r2) {
            (
                LifecyclePushResult::Enqueued {
                    notification_id: id1,
                },
                LifecyclePushResult::AlreadyComplete {
                    notification_id: id2,
                },
            ) => assert_eq!(id1, id2),
            other => panic!("expected Enqueued then AlreadyComplete, got {other:?}"),
        }
        assert_eq!(sq.pending_count("sup-emit").unwrap(), 1);
        let pending = sq.peek("sup-emit", 10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].event_type, "task_lifecycle");
        assert!(pending[0].payload.contains("task_started"));
        assert_eq!(
            pending[0].transition_key.as_deref(),
            Some("cas-t1:open:in_progress:sess-emit:task_started:occ-1")
        );
        assert!(pending[0].prompt_delivered_at.is_some());
        assert_eq!(pq.pending_count().unwrap(), 1);

        // SAFETY: restore env under the process-wide Cassy env lock.
    }

    #[test]
    fn emit_supervisor_actor_keeps_durable_event_without_prompt_self_echo() {
        // cas-acb4: one guard owns the shared env lock AND restores on
        // unwind. The previous save/restore pair leaked the variable to
        // every later test in this binary whenever an assertion panicked
        // between the two halves.
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-self")]);

        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        agents
            .register(&agent_in_session(
                "sup-self-id",
                "sup-self-name",
                AgentRole::Supervisor,
                "sess-self",
            ))
            .unwrap();
        let sq = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        sq.init().unwrap();
        let pq = SqlitePromptQueueStore::open(temp.path()).unwrap();
        pq.init().unwrap();

        let result = emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-self",
            "Self close",
            TaskStatus::InProgress,
            TaskStatus::Closed,
            "sup-self-name",
            Some("done"),
            LifecycleTransition::Closed,
            "occ-self",
        )
        .unwrap();

        assert!(matches!(result, LifecyclePushResult::Enqueued { .. }));
        assert_eq!(sq.pending_count("sup-self-id").unwrap(), 1);
        let row = sq
            .get_by_transition_key("cas-self:in_progress:closed:sess-self:task_closed:occ-self")
            .unwrap()
            .expect("durable lifecycle event");
        assert!(
            row.prompt_delivered_at.is_some(),
            "self-actor row must be complete so outbox drain cannot wake the supervisor later"
        );
        assert_eq!(
            pq.pending_count().unwrap(),
            0,
            "supervisor must not receive a prompt for its own transition"
        );
    }

    #[test]
    fn emit_worker_actor_still_prompts_owning_supervisor() {
        // cas-acb4: one guard owns the shared env lock AND restores on
        // unwind. The previous save/restore pair leaked the variable to
        // every later test in this binary whenever an assertion panicked
        // between the two halves.
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-worker")]);

        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        agents
            .register(&agent_in_session(
                "sup-worker-id",
                "sup-worker-name",
                AgentRole::Supervisor,
                "sess-worker",
            ))
            .unwrap();
        let sq = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        sq.init().unwrap();
        let pq = SqlitePromptQueueStore::open(temp.path()).unwrap();
        pq.init().unwrap();

        emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-worker",
            "Worker close",
            TaskStatus::InProgress,
            TaskStatus::Closed,
            "worker-name",
            Some("done"),
            LifecycleTransition::Closed,
            "occ-worker",
        )
        .unwrap();

        assert_eq!(
            pq.pending_count().unwrap(),
            1,
            "worker transition must still wake the owning supervisor"
        );
    }

    #[test]
    fn emit_two_start_cycles_create_two_events() {
        // cas-acb4: one guard owns the shared env lock AND restores on
        // unwind. The previous save/restore pair leaked the variable to
        // every later test in this binary whenever an assertion panicked
        // between the two halves.
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-cycle")]);

        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        agents
            .register(&agent_in_session(
                "sup-c",
                "sup-c",
                AgentRole::Supervisor,
                "sess-cycle",
            ))
            .unwrap();
        let sq = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        sq.init().unwrap();
        let pq = SqlitePromptQueueStore::open(temp.path()).unwrap();
        pq.init().unwrap();

        // start₁
        emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-c",
            "C",
            TaskStatus::Open,
            TaskStatus::InProgress,
            "w",
            None,
            LifecycleTransition::Started,
            "t1",
        )
        .unwrap();
        // block
        emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-c",
            "C",
            TaskStatus::InProgress,
            TaskStatus::Blocked,
            "w",
            None,
            LifecycleTransition::Blocked,
            "t2",
        )
        .unwrap();
        // ready
        emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-c",
            "C",
            TaskStatus::Blocked,
            TaskStatus::Open,
            "w",
            None,
            LifecycleTransition::ReadyReopened,
            "t3",
        )
        .unwrap();
        // start₂ — same old/new/kind as start₁ but different occurrence
        emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-c",
            "C",
            TaskStatus::Open,
            TaskStatus::InProgress,
            "w",
            None,
            LifecycleTransition::Started,
            "t4",
        )
        .unwrap();

        assert_eq!(sq.pending_count("sup-c").unwrap(), 4);
        let pending = sq.peek("sup-c", 20).unwrap();
        let started: Vec<_> = pending
            .iter()
            .filter(|n| n.payload.contains("task_started"))
            .collect();
        assert_eq!(started.len(), 2, "two legitimate starts must both emit");
        assert_eq!(pq.pending_count().unwrap(), 4);
    }

    #[test]
    fn emit_does_not_cross_factory_sessions() {
        // cas-acb4: one guard owns the shared env lock AND restores on
        // unwind. The previous save/restore pair leaked the variable to
        // every later test in this binary whenever an assertion panicked
        // between the two halves.
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-a")]);

        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        agents
            .register(&agent_in_session(
                "sup-a",
                "sup-a",
                AgentRole::Supervisor,
                "sess-a",
            ))
            .unwrap();
        agents
            .register(&agent_in_session(
                "sup-b",
                "sup-b",
                AgentRole::Supervisor,
                "sess-b",
            ))
            .unwrap();
        let sq = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        sq.init().unwrap();
        let pq = SqlitePromptQueueStore::open(temp.path()).unwrap();
        pq.init().unwrap();

        emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-x",
            "X",
            TaskStatus::InProgress,
            TaskStatus::Blocked,
            "worker",
            Some("waiting"),
            LifecycleTransition::Blocked,
            "occ-x",
        )
        .unwrap();

        assert_eq!(sq.pending_count("sup-a").unwrap(), 1);
        assert_eq!(sq.pending_count("sup-b").unwrap(), 0);
    }

    /// Simulate partial failure: durable insert without prompt stamp, then recover.
    #[test]
    fn durable_without_prompt_recovers_exactly_once_on_replay() {
        // cas-acb4: one guard owns the shared env lock AND restores on
        // unwind. The previous save/restore pair leaked the variable to
        // every later test in this binary whenever an assertion panicked
        // between the two halves.
        let _env = TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "sess-outbox")]);

        let temp = TempDir::new().unwrap();
        let agents = SqliteAgentStore::open(temp.path()).unwrap();
        agents.init().unwrap();
        agents
            .register(&agent_in_session(
                "sup-o",
                "sup-o",
                AgentRole::Supervisor,
                "sess-outbox",
            ))
            .unwrap();
        let sq = SqliteSupervisorQueueStore::open(temp.path()).unwrap();
        sq.init().unwrap();
        let pq = SqlitePromptQueueStore::open(temp.path()).unwrap();
        pq.init().unwrap();

        let key = "cas-o:open:in_progress:sess-outbox:task_started:occ-outbox";
        // Inject partial failure: durable row exists, prompt not stamped.
        let created = sq
            .notify_idempotent(
                "sup-o",
                "task_lifecycle",
                r#"{"task_id":"cas-o"}"#,
                NotificationPriority::Normal,
                key,
            )
            .unwrap();
        let id = match created {
            NotifyIdempotentResult::Created(id) => id,
            other => panic!("expected Created, got {other:?}"),
        };
        assert!(
            sq.get_by_transition_key(key)
                .unwrap()
                .unwrap()
                .prompt_delivered_at
                .is_none()
        );

        // Replay via emit: must deliver prompt + stamp, not insert second durable row.
        let r2 = emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-o",
            "O",
            TaskStatus::Open,
            TaskStatus::InProgress,
            "w",
            None,
            LifecycleTransition::Started,
            "occ-outbox",
        )
        .expect("replay must succeed");
        assert!(
            matches!(r2, LifecyclePushResult::Recovered { notification_id } if notification_id == id),
            "got {r2:?}"
        );
        assert_eq!(sq.pending_count("sup-o").unwrap(), 1);
        assert!(
            sq.get_by_transition_key(key)
                .unwrap()
                .unwrap()
                .prompt_delivered_at
                .is_some()
        );
        assert_eq!(pq.pending_count().unwrap(), 1);

        // Third call: fully complete — no additional prompt row.
        let r3 = emit_task_lifecycle_transition(
            &sq,
            Some(&pq as &dyn PromptQueueStore),
            &agents,
            "cas-o",
            "O",
            TaskStatus::Open,
            TaskStatus::InProgress,
            "w",
            None,
            LifecycleTransition::Started,
            "occ-outbox",
        )
        .unwrap();
        assert!(matches!(r3, LifecyclePushResult::AlreadyComplete { .. }));
        assert_eq!(
            pq.pending_count().unwrap(),
            1,
            "exactly-once prompt delivery"
        );
    }
}
