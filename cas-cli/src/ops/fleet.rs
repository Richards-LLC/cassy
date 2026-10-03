//! Fleet operations (fleet-operations brief S1, cas-566b).
//!
//! The operator facade the MCP actions and the Commander hub both call:
//!
//! - O1, ask the supervisor to merge: one operator turn on the Commander
//!   lane ([`enqueue_commander_message`]), the same row a Commander
//!   `SendMessage` makes. MCP `message_send` is not reused: it derives its
//!   sender from the MCP caller's registered agent, which a hub device has not.
//! - O2, focus an epic: [`focus_epic`], the body of MCP `focus_epic`.
//!
//! Each hub operation states what the operator saw (`expected`); a mismatch
//! is [`OperationError::Stale`] and changes nothing.

use std::path::Path;

/// Why an operation did not run.
#[derive(Debug)]
pub(crate) enum OperationError {
    /// The fleet no longer matches `expected`; `current` is what it is now.
    Stale(serde_json::Value),
    /// A named task or session does not exist.
    NotFound(String),
    /// The request cannot run as asked; nothing changed.
    Invalid(String),
    /// The operation failed while running.
    Failed(String),
}

impl std::fmt::Display for OperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale(current) => write!(f, "stale: now {current}"),
            Self::NotFound(detail) | Self::Invalid(detail) | Self::Failed(detail) => {
                f.write_str(detail)
            }
        }
    }
}

/// What the operator saw when asking for a merge (brief: O1's precondition).
#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct RequestMergeExpected {
    pub status: String,
    #[serde(default)]
    pub tip: Option<String>,
}

/// O1: ask the session's supervisor to merge an awaiting-merge task, as an
/// operator turn carrying the device's verified attribution. Returns the
/// queued notification id.
pub(crate) fn request_merge(
    cas_dir: &Path,
    factory_session: &str,
    task_id: &str,
    expected: &RequestMergeExpected,
    attribution: &crate::ui::factory::protocol::MessageAttribution,
) -> Result<i64, OperationError> {
    let store = crate::store::open_task_store(cas_dir)
        .map_err(|error| OperationError::Failed(format!("task store unavailable: {error}")))?;
    let task = store
        .get(task_id)
        .map_err(|_| OperationError::NotFound(format!("task {task_id} not found")))?;
    let status = task.status.to_string();
    let tip = task.deliverables.factory_branch_anchor.clone();
    if status != expected.status || tip != expected.tip {
        return Err(OperationError::Stale(serde_json::json!({
            "status": status,
            "tip": tip,
        })));
    }
    if task.status != cas_types::TaskStatus::AwaitingMerge {
        return Err(OperationError::Invalid(format!(
            "{task_id} is {status}; only an awaiting_merge task can be sent to the supervisor to merge"
        )));
    }
    let text = request_merge_text(
        task_id,
        &task.title,
        task.deliverables.parked_branch.as_deref(),
        tip.as_deref(),
    );
    let summary = format!("Merge request from Commander: {task_id}");
    let outcome = enqueue_commander_message(
        cas_dir,
        factory_session,
        "supervisor",
        &text,
        Some(&summary),
        false,
        None,
        attribution,
    )
    .map_err(|error| OperationError::Failed(format!("could not queue the request: {error}")))?;
    if matches!(outcome, cas_store::EnqueueOutcome::Created(_)) {
        crate::ui::factory::daemon::runtime::delivery::wake_daemon_after_enqueue(cas_dir);
    }
    Ok(outcome.id())
}

/// The message an operator's "ask the supervisor to merge" sends: the task,
/// the branch it is parked on and the tip the operator saw.
pub(crate) fn request_merge_text(
    task_id: &str,
    title: &str,
    branch: Option<&str>,
    tip: Option<&str>,
) -> String {
    format!(
        "Operator request from Commander: please merge {task_id} ({title}).\n\
         Branch: {}\nTip: {}\n\
         It is awaiting merge. Merge it into its epic, or reply with what blocks it.",
        branch.unwrap_or("<not recorded>"),
        tip.unwrap_or("<not recorded>"),
    )
}

/// Store one Commander semantic message in the exact prompt queue drained by
/// coordination delivery. Split from the daemon method so parity tests can
/// compare a Commander row with an MCP coordination row in one isolated DB.
///
/// `in_reply_to` (cas-a8ea8) names the supervisor's Commander turn — an ask —
/// that this message answers. It gets the same treatment as a worker reply's
/// `in_reply_to` in the coordination `message` tool: the referenced row must
/// exist and be a supervisor→operator turn of this factory session, the
/// queued text carries the explicit reply reference the supervisor's inbox
/// renders, and the ask is confirmed (`acked_via = 'explicit_ack'`) so it
/// stops reading as unanswered.
#[allow(clippy::too_many_arguments)]
pub(crate) fn enqueue_commander_message(
    cas_dir: &std::path::Path,
    factory_session: &str,
    target: &str,
    text: &str,
    summary: Option<&str>,
    urgent: bool,
    in_reply_to: Option<i64>,
    attribution: &crate::ui::factory::protocol::MessageAttribution,
) -> anyhow::Result<cas_store::EnqueueOutcome> {
    use anyhow::Context as _;

    let queue = crate::store::open_prompt_queue_store(cas_dir)?;
    let attribution_json = serde_json::to_value(attribution)?;
    let priority = urgent.then_some(cas_store::NotificationPriority::Critical);
    let operator = operator_stamp(attribution);
    let bound_text;
    let text = if let Some(notification_id) = in_reply_to {
        let prior = queue
            .message_delivery_report(notification_id)
            .with_context(|| format!("failed to inspect in_reply_to message {notification_id}"))?
            .ok_or_else(|| anyhow::anyhow!("in_reply_to notification {notification_id} does not exist"))?;
        if !prior.target.eq_ignore_ascii_case(OPERATOR_TARGET) {
            anyhow::bail!(
                "in_reply_to notification {notification_id} is {} -> {}, not a supervisor turn addressed to the operator",
                prior.source,
                prior.target
            );
        }
        if prior
            .factory_session
            .as_deref()
            .is_some_and(|session| session != factory_session)
        {
            anyhow::bail!(
                "in_reply_to notification {notification_id} belongs to factory session {}, not {factory_session}",
                prior.factory_session.as_deref().unwrap_or_default()
            );
        }
        bound_text = commander_reply_text(notification_id, text);
        bound_text.as_str()
    } else {
        text
    };
    let outcome = queue.enqueue_operator_message(
        &attribution.queue_source(),
        target,
        text,
        Some(factory_session),
        summary,
        priority,
        urgent,
        Some(&attribution_json),
        &operator,
    )?;
    if let Some(notification_id) = in_reply_to {
        queue.ack(notification_id).with_context(|| {
            format!(
                "Commander reply {} queued but ask {notification_id} could not be confirmed",
                outcome.id()
            )
        })?;
    }
    Ok(outcome)
}

/// Recipient name of supervisor→operator Commander turns (`message.rs`).
const OPERATOR_TARGET: &str = "operator";

/// The explicit reply reference a supervisor's inbox renders — byte-identical
/// to the one the coordination `message` tool prefixes on a worker reply.
pub(crate) fn commander_reply_text(notification_id: i64, text: &str) -> String {
    format!("[CAS reply: explicitly acknowledges notification_id={notification_id}]\n{text}")
}

/// The durable operator columns for one Commander row (cas-e8df).
///
/// `verified` is true only when the hub said so AND named both the device and
/// its credential; a frame that claims verification without a principal is
/// treated as a client claim, and the row's origin becomes `Unattributed`.
pub(crate) fn operator_stamp(
    attribution: &crate::ui::factory::protocol::MessageAttribution,
) -> cas_store::OperatorStamp {
    let verified = attribution.operator_verified
        && attribution.device_id.as_deref().is_some_and(|id| !id.is_empty())
        && attribution
            .credential_id
            .as_deref()
            .is_some_and(|id| !id.is_empty());
    cas_store::OperatorStamp {
        operator: attribution.operator_label.clone().unwrap_or_default(),
        device_id: attribution.device_id.clone().unwrap_or_default(),
        device_label: attribution.device_label.clone().unwrap_or_default(),
        scopes: attribution.scopes.clone(),
        verified,
    }
}


/// O2's request: pin the session to an epic, or clear the pin.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FocusEpic<'a> {
    Pin {
        epic_id: &'a str,
        delivery_mode: Option<cas_types::DeliveryMode>,
    },
    Clear,
}

/// The epic a factory session is pinned to now, from its metadata.
pub(crate) fn pinned_epic(factory_session: &str) -> Option<String> {
    let data = std::fs::read_to_string(crate::ui::factory::metadata_path(factory_session)).ok()?;
    serde_json::from_str::<crate::ui::factory::SessionMetadata>(&data)
        .ok()?
        .pinned_epic_id
}

/// O2: focus a factory session on an epic, or clear the focus. The body of
/// MCP `factory action=focus_epic`; its success text is that action's reply.
pub(crate) fn focus_epic(
    cas_root: &Path,
    factory_session: &str,
    request: FocusEpic<'_>,
) -> Result<String, OperationError> {
    use crate::store::open_task_store;
    use crate::ui::factory::{
        metadata_path, persist_session_metadata_delivery_mode_at,
        persist_session_metadata_pinned_epic_id_at,
    };
    use cas_types::{TaskStatus, TaskType};

    let metadata_path = metadata_path(factory_session);
    let (epic_id, requested_delivery_mode) = match request {
        FocusEpic::Clear => {
            persist_session_metadata_pinned_epic_id_at(&metadata_path, None).map_err(|e| {
                OperationError::Failed(format!("Failed to clear pinned epic focus: {e}"))
            })?;
            record_focus_epic_event(cas_root, factory_session, None, None);
            return Ok(format!(
                "Cleared pinned epic focus for factory session {factory_session}"
            ));
        }
        FocusEpic::Pin {
            epic_id,
            delivery_mode,
        } => (epic_id, delivery_mode),
    };

    let task_store = open_task_store(cas_root)
        .map_err(|e| OperationError::Failed(format!("Failed to open task store: {e}")))?;

    let mut epic = task_store
        .get(epic_id)
        .map_err(|e| OperationError::NotFound(format!("Task not found: {epic_id}: {e}")))?;

    if epic.task_type != TaskType::Epic {
        return Err(OperationError::Invalid(format!(
            "focus_epic: task {epic_id} is not an Epic (task_type={:?}). \
             This action only operates on Epic-type tasks.",
            epic.task_type
        )));
    }
    if epic.status == TaskStatus::Closed {
        return Err(OperationError::Invalid(format!(
            "focus_epic: task {epic_id} is Closed. \
             Closed epics cannot be pinned as the active factory focus.",
        )));
    }

    let delivery_mode = requested_delivery_mode.unwrap_or(epic.delivery_mode);
    if requested_delivery_mode.is_some() {
        epic.delivery_mode = delivery_mode;
        task_store.update(&epic).map_err(|error| {
            OperationError::Failed(format!("Failed to persist epic delivery mode: {error}"))
        })?;
    }

    persist_session_metadata_pinned_epic_id_at(&metadata_path, Some(epic_id))
        .map_err(|e| OperationError::Failed(format!("Failed to persist pinned epic focus: {e}")))?;
    persist_session_metadata_delivery_mode_at(&metadata_path, delivery_mode).map_err(|e| {
        OperationError::Failed(format!("Failed to persist factory delivery mode: {e}"))
    })?;
    record_focus_epic_event(cas_root, factory_session, Some(epic_id), Some(delivery_mode));

    Ok(format!(
        "Pinned epic focus to {epic_id} for factory session {factory_session} (delivery_mode={delivery_mode})"
    ))
}

fn record_focus_epic_event(
    cas_root: &Path,
    factory_session: &str,
    epic_id: Option<&str>,
    delivery_mode: Option<cas_types::DeliveryMode>,
) {
    use crate::store::open_event_store;
    use cas_types::{Event, EventEntityType, EventType};

    let Ok(event_store) = open_event_store(cas_root) else {
        return;
    };

    let summary = match epic_id {
        Some(epic_id) => format!("Pinned factory epic focus to {epic_id}"),
        None => "Cleared factory epic focus pin".to_string(),
    };
    let entity_type = if epic_id.is_some() {
        EventEntityType::Task
    } else {
        EventEntityType::Session
    };
    let entity_id = epic_id.unwrap_or(factory_session);
    let metadata = serde_json::json!({
        "factory_session": factory_session,
        "epic_id": epic_id,
        "delivery_mode": delivery_mode.map(|mode| mode.to_string()),
    });
    let event = Event::new(
        EventType::SupervisorInjected,
        entity_type,
        entity_id,
        summary,
    )
    .with_metadata(metadata)
    .with_session(factory_session);
    let _ = event_store.record(&event);
}
