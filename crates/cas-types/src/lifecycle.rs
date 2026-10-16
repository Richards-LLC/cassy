//! Typed supervisor relay envelopes shared by transport and recipient drains.

use crate::TaskStatus;
use chrono::{DateTime, Utc};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleEnvelope {
    pub task_id: String,
    pub new_status: TaskStatus,
    pub occurrence: DateTime<Utc>,
    /// The lifecycle kind is optional for compatibility with envelopes from
    /// before the relay supersession contract was added.
    pub transition: Option<String>,
    /// Merge-boundary tip captured when an awaiting-merge/close-rejected relay
    /// was emitted. A later close cycle must not revive the old relay.
    pub branch_tip: Option<String>,
}

fn xml_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

/// Parse a lifecycle relay without interpreting its notification id as a task id.
pub fn parse_lifecycle_envelope(prompt: &str) -> Option<LifecycleEnvelope> {
    let tag_end = prompt.find('>')?;
    let tag = &prompt[..tag_end];
    if !tag.starts_with("<task-lifecycle ") {
        return None;
    }
    let task_id = xml_attribute(tag, "task_id")?.to_string();
    let new_status = TaskStatus::from_str(xml_attribute(tag, "new")?).ok()?;
    let occurrence = DateTime::parse_from_rfc3339(xml_attribute(tag, "occurrence")?)
        .ok()?
        .with_timezone(&Utc);
    Some(LifecycleEnvelope {
        task_id,
        new_status,
        occurrence,
        transition: xml_attribute(tag, "transition").map(str::to_string),
        branch_tip: xml_attribute(tag, "branch_tip").map(str::to_string),
    })
}

const VERIFICATION_DISPATCH_ENVELOPE_OPEN: &str = "<cas-verification-dispatch ";
const VERIFICATION_DISPATCH_ENVELOPE_CLOSE: &str = "</cas-verification-dispatch>";

/// CAS's own handoff for a close that just entered verification (cas-8725).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationDispatchEnvelope {
    pub dispatch_id: String,
    pub task_id: String,
    /// Agent id bound to record the verdict.
    pub owner: String,
    /// RFC3339 instant after which the dispatch times out.
    pub deadline: String,
    /// Git HEAD captured when this dispatch was created, when it is a
    /// repository-bound task cycle.
    pub bound_head: Option<String>,
    /// Verdict id from the immediately preceding cycle, when a close retry
    /// replaced a proof that had already been reviewed.
    pub approved_verdict_id: Option<String>,
}

/// Parse a verification-dispatch handoff, if this prompt is one.
///
/// Whole-prompt shaped, like `<task-lifecycle …>`: CAS composes the entire
/// row, so the envelope opens the prompt and closes it. A worker cannot
/// prepend it to its own free text and still parse.
pub fn parse_verification_dispatch_envelope(
    prompt: &str,
) -> Option<VerificationDispatchEnvelope> {
    if !prompt.starts_with(VERIFICATION_DISPATCH_ENVELOPE_OPEN)
        || !prompt.trim_end().ends_with(VERIFICATION_DISPATCH_ENVELOPE_CLOSE)
    {
        return None;
    }
    let tag_end = prompt.find('>')?;
    let tag = &prompt[..tag_end];
    let required = |name: &str| xml_attribute(tag, name).filter(|value| !value.is_empty());
    Some(VerificationDispatchEnvelope {
        dispatch_id: required("dispatch_id")?.to_string(),
        task_id: required("task_id")?.to_string(),
        owner: required("owner")?.to_string(),
        deadline: required("deadline")?.to_string(),
        bound_head: xml_attribute(tag, "bound_head")
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        approved_verdict_id: xml_attribute(tag, "approved_verdict_id")
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    })
}

/// A CI point-in-time alert; its branch can resolve a task locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiRedRunEnvelope {
    pub branch: String,
    pub task_id: Option<String>,
}

/// Parse exactly the CI envelope grammar accepted by the supervisor wake gate.
pub fn parse_ci_red_run_envelope(prompt: &str) -> Option<CiRedRunEnvelope> {
    let tag_end = prompt.find('>')?;
    let tag = &prompt[..tag_end];
    let valid = tag.starts_with("<ci-red-run ")
        && xml_attribute(tag, "branch").is_some_and(|value| !value.is_empty())
        && xml_attribute(tag, "head_sha").is_some_and(|value| !value.is_empty())
        && xml_attribute(tag, "run_id").is_some_and(|value| value.parse::<u64>().is_ok())
        && prompt.ends_with("</ci-red-run>");
    valid.then(|| CiRedRunEnvelope {
        branch: xml_attribute(tag, "branch").unwrap().to_string(),
        task_id: xml_attribute(tag, "task_id").filter(|id| !id.is_empty()).map(str::to_string),
    })
}

/// A worker-attention snapshot, optionally scoped to a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerAttentionEnvelope {
    pub task_id: Option<String>,
}

/// Parse exactly the attention grammar accepted by the supervisor wake gate.
pub fn parse_worker_attention_envelope(prompt: &str) -> Option<WorkerAttentionEnvelope> {
    let tag_end = prompt.find('>')?;
    let tag = &prompt[..tag_end];
    let valid = tag.starts_with("<worker-attention ")
        && matches!(
            xml_attribute(tag, "kind"),
            Some(
                "worker_idle"
                    | "worker_stalled"
                    | "worker_delivery_stalled"
                    | "worker_unavailable"
                    // cas-4143: a teammate permission parked for a lead
                    // nobody plays, past the wake threshold.
                    | "worker_approval_pending"
                    // cas-2ffe: simultaneous harness exits flagged as one.
                    | "workers_died_together"
                    | "supervisor_stalled"
                    | "merged_close_blocked"
                    | "pr_lane_failed"
                    | "delivery_pr_merge_timeout"
                    // cas-d9a8: CAS's own unread-backlog summary for the
                    // supervisor. Same envelope, same producer (the daemon
                    // relay), so it inherits the wake terms rather than
                    // inventing new ones.
                    | "supervisor_unread"
                    | "sweep_failed"
                    // GH #1006: the one report a deferred rolling
                    // integration sends when it finally runs green.
                    | "sweep_passed"
                    | "sweep_not_configured"
            )
        )
        && xml_attribute(tag, "worker").is_some_and(|value| !value.is_empty())
        && xml_attribute(tag, "notification_id").is_some_and(|value| value.parse::<i64>().is_ok())
        && prompt.ends_with("</worker-attention>");
    valid.then(|| WorkerAttentionEnvelope {
        task_id: xml_attribute(tag, "task_id").filter(|id| !id.is_empty()).map(str::to_string),
    })
}

