use cas_types::{Task, TaskStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::str::FromStr;

const MERGE_ENVELOPE_OPEN: &str = "<cas-merge-request>";
const MERGE_ENVELOPE_CLOSE: &str = "</cas-merge-request>";

/// The task id an assignment-like prompt asks its recipient to start.
///
/// This is deliberately shared by daemon delivery and `inbox_poll`: an
/// assignment queued before a task reaches a terminal state must not become a
/// fresh-looking `task start` instruction merely because it took a different
/// transport. Spawn briefs use the same imperative shape and therefore belong
/// to this contract too.
pub(crate) fn assignment_solicited_task_id(prompt: &str) -> Option<String> {
    let lowered = prompt.to_lowercase();
    const ASSIGNMENT_PHRASES: [&str; 5] = [
        "you have been assigned",
        "you are assigned",
        "you're assigned",
        "assigned task",
        "you were spawned for task",
    ];
    if !ASSIGNMENT_PHRASES
        .iter()
        .any(|phrase| lowered.contains(phrase))
    {
        return None;
    }
    for marker in [
        "action=start id=",
        "task id:",
        "assigned task ",
        "spawned for task ",
    ] {
        if let Some(index) = lowered.find(marker)
            && let Some(id) = first_task_id_token(&prompt[index + marker.len()..])
        {
            return Some(id);
        }
    }
    first_task_id_token(prompt)
}

/// Terminal task states make an assignment's `task start` imperative stale.
/// Missing/unreadable state deliberately returns false: delivery must fail
/// open unless Cassy has positive terminal evidence.
pub(crate) fn assignment_targets_terminal_task(prompt: &str, status: TaskStatus) -> Option<String> {
    matches!(status, TaskStatus::Closed | TaskStatus::Cancelled)
        .then(|| assignment_solicited_task_id(prompt))
        .flatten()
}

/// A spawn-time assignment is stale once the addressed worker has already
/// moved its assigned task past `Open`. This is separate from terminal-task
/// suppression: an in-progress or parked task is still valid work, but its
/// original `task start` boilerplate is no longer an actionable instruction.
/// Require the same assignee so another worker's progress cannot suppress a
/// message that this recipient may still need.
pub(crate) fn assignment_targets_started_task(
    prompt: &str,
    status: TaskStatus,
    assignee: Option<&str>,
    recipient: &str,
) -> Option<String> {
    matches!(
        status,
        TaskStatus::InProgress
            | TaskStatus::Blocked
            | TaskStatus::AwaitingMerge
    )
    .then(|| assignment_solicited_task_id(prompt))
    .flatten()
    .filter(|_| assignee.is_some_and(|owner| owner.eq_ignore_ascii_case(recipient)))
}

fn first_task_id_token(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .find(|token| {
            let Some(suffix) = token
                .strip_prefix("cas-")
                .or_else(|| token.strip_prefix("Cassy-"))
            else {
                return false;
            };
            suffix.len() >= 4 && suffix.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .map(|token| format!("cas-{}", &token[4..]))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MergeRequestEnvelope {
    pub task_id: String,
    /// The tip the request is about, resolved LIVE at compose time.
    pub branch_tip: String,
    pub target_branch: String,
    pub target_branch_tip: String,
    /// The `factory_branch_anchor` recorded by the previous merge/close, when
    /// it differs from `branch_tip` (cas-b17c / GH #703).
    ///
    /// Present only on drift, and never used to decide suppression — it is
    /// carried so the supervisor can see that the worker has pushed past the
    /// last merge rather than having to infer it. Optional with a serde
    /// default so envelopes written by an older client still parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_tip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeRequestDecision {
    Pending { target_tip: String },
    AlreadyIntegrated { target_tip: String },
    Unverifiable,
}

/// What the daemon should do with a worker→supervisor merge request at
/// transport time (cas-6eab, GH #61).
///
/// The request is an instruction ("please merge `<tip>`"), so it is only worth
/// delivering while its premise holds. Two things can kill that premise
/// between the worker composing it and the supervisor reading it, and in the
/// reported sessions both routinely did — the supervisor had already merged
/// and already replied before the request arrived, on ~12 of ~20 closes:
///
/// - the branch tip is already an ancestor of the target branch (merged), or
/// - the task is no longer parked awaiting a merge (re-closed, reopened,
///   request_changes'd), so nothing is waiting on the supervisor at all, or
/// - the task's delivery anchor no longer names this request's immutable tip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeRequestDelivery {
    /// Premise still holds — deliver, and tag the row so it can be retracted
    /// if the merge lands before the supervisor reads it.
    Deliver,
    /// The merge already landed. Suppress and tell the WORKER instead.
    SuppressLanded { target_tip: String },
    /// The task left `AwaitingMerge` — the request is moot whatever git says.
    SuppressResolved { status: TaskStatus },
    /// The task may have entered a later merge cycle, but the immutable tip
    /// this message asked the supervisor to merge was invalidated. This is
    /// deliberately distinct from `SuppressResolved`: a task can return to
    /// `AwaitingMerge` after request_changes, reset, or a reopen.
    SuppressInvalidatedAnchor { current_anchor: Option<String> },
}

/// Decide a merge request's fate from live state only (cas-6eab).
///
/// `task` is read fresh at transport time; `None` means it could not be read
/// at all. `envelope.branch_tip` is the delivery anchor captured when the
/// message was queued. `git` is the live reachability check for that anchor.
///
/// Fails open in exactly one direction: uncertainty (unreadable task,
/// unverifiable git) delivers. A suppression requires positive evidence that
/// the request is already satisfied, because the cost of wrongly suppressing
/// a genuine merge request is a stalled task, while the cost of delivering a
/// stale one is a supervisor round-trip.
pub(crate) fn merge_request_delivery_decision(
    task: Option<&Task>,
    envelope: &MergeRequestEnvelope,
    git: &MergeRequestDecision,
) -> MergeRequestDelivery {
    if let Some(task) = task {
        if task.status != TaskStatus::AwaitingMerge {
            return MergeRequestDelivery::SuppressResolved {
                status: task.status,
            };
        }
        if task.deliverables.factory_branch_anchor.as_deref() != Some(&envelope.branch_tip) {
            return MergeRequestDelivery::SuppressInvalidatedAnchor {
                current_anchor: task.deliverables.factory_branch_anchor.clone(),
            };
        }
    }
    match git {
        MergeRequestDecision::AlreadyIntegrated { target_tip } => {
            MergeRequestDelivery::SuppressLanded {
                target_tip: target_tip.clone(),
            }
        }
        MergeRequestDecision::Pending { .. } | MergeRequestDecision::Unverifiable => {
            MergeRequestDelivery::Deliver
        }
    }
}

/// Guidance sent to the worker when the exact delivery anchor named by its
/// message has been invalidated. It may be tempting to describe an
/// `AwaitingMerge` task as still actionable here, but that can be a *later*
/// close cycle. Naming both anchors keeps the worker from asking the
/// supervisor to merge a declined or superseded tip.
pub(crate) fn merge_request_anchor_invalidated_guidance(
    task_id: &str,
    branch_tip: &str,
    current_anchor: Option<&str>,
) -> String {
    let current = current_anchor.unwrap_or("none (the prior delivery was invalidated)");
    format!(
        "Cassy suppressed your stale merge request for {task_id}: delivery anchor {branch_tip} \
         is no longer current (current anchor: {current}). Do not ask the supervisor to merge \
         the prior tip. Re-read the task with `task action=show id={task_id}` before sending \
         anything further about it."
    )
}

/// Guidance sent to the worker when its merge request is suppressed because
/// the task is no longer parked (cas-6eab).
pub(crate) fn merge_request_moot_guidance(task_id: &str, status: TaskStatus) -> String {
    format!(
        "Cassy suppressed your merge request for {task_id}: the task is no longer awaiting a \
         merge (current status: {status}). Nothing is queued for the supervisor. Re-read the \
         task with `task action=show id={task_id}` before sending anything further about it."
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LifecycleEnvelope {
    pub task_id: String,
    pub new_status: TaskStatus,
    pub occurrence: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LifecyclePromptDecision {
    Deliver,
    SuppressStale { task_id: String },
    Unstructured,
}

/// What the daemon should do with a lifecycle prompt whose task state has
/// moved on (cas-7787, GH #160).
///
/// [`revalidate_lifecycle_prompt`] answers only "is this payload still true?".
/// That question is necessary but not sufficient, and treating it as
/// sufficient is the reported defect: in session cas-src-fast-pelican-83 the
/// `task_awaiting_merge` relays for cas-fe23 (18:51), cas-d897 (19:00),
/// cas-b69a (19:17) and cas-edee (19:26) were each enqueued correctly,
/// written to the supervisor's inbox, never transported, and then stamped
/// `suppressed_idle` at the exact second their task went
/// awaiting_merge → closed. Suppression was the right call for the PAYLOAD —
/// it had genuinely expired — and the wrong call for the FACT that a relay
/// the factory depends on had failed to arrive. Nothing recorded the failure,
/// so a human became the delivery mechanism for three finished lanes.
///
/// So staleness alone no longer decides. A stale row that was delivered is a
/// benign suppression; a stale row that was never transported is a delivery
/// FAILURE, and a wake-eligible one is a failure the factory cannot function
/// without knowing about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LifecycleStaleOutcome {
    /// Payload still true — hand it to transport.
    Deliver,
    /// Payload expired and the recipient already got it. Quiet suppression.
    SuppressDelivered { task_id: String },
    /// Payload expired and the recipient NEVER got it. Terminate the row (a
    /// live re-write would storm an expired premise) but record it loudly.
    UndeliveredRelayFailure { task_id: String },
}

/// Decide a stale lifecycle row's fate from the payload verdict plus the two
/// facts the payload cannot know: did it ever reach the recipient, and was it
/// load-bearing (cas-7787, GH #160).
///
/// Pure so the honesty contract is testable without a daemon or a harness.
///
/// Fails toward NOISE, deliberately and in one direction only: when a
/// wake-eligible relay is stale and undelivered we report a failure rather
/// than assume it was harmless. A false alarm costs the supervisor one glance
/// at `worker_status`; a missed one costs a parked lane an unbounded stall,
/// which is exactly what happened.
pub(crate) fn lifecycle_stale_outcome(
    decision: &LifecyclePromptDecision,
    wake_eligible: bool,
    transport_delivered: bool,
) -> LifecycleStaleOutcome {
    let LifecyclePromptDecision::SuppressStale { task_id } = decision else {
        return LifecycleStaleOutcome::Deliver;
    };
    // A non-wake lifecycle row (task_started / task_ready / task_closed) is
    // progress FYI. Losing one costs nothing the factory needs, so it keeps
    // the quiet path and does not train anyone to ignore the banner.
    if wake_eligible && !transport_delivered {
        return LifecycleStaleOutcome::UndeliveredRelayFailure {
            task_id: task_id.clone(),
        };
    }
    LifecycleStaleOutcome::SuppressDelivered {
        task_id: task_id.clone(),
    }
}

/// Operator-facing description of one undelivered lifecycle relay
/// (cas-7787, GH #160).
///
/// States what did not happen and what to do, in that order, without
/// mentioning a queue stage or a prompt row — the reader is a supervisor
/// deciding whether a lane is waiting on them, not someone debugging Cassy.
pub(crate) fn undelivered_relay_notice(task_id: &str, summary: Option<&str>) -> String {
    let what = summary.unwrap_or("a task lifecycle transition");
    format!(
        "UNDELIVERED: the factory notified the supervisor that {task_id} needed them \
         ({what}) and that message never arrived. The task moved on before it could be \
         delivered, so it has been withdrawn rather than re-sent with an expired premise. \
         Check {task_id} directly — do not assume it was handled."
    )
}

/// Operator-facing description of one undelivered worker-death relay
/// (cas-3dcb, GH #168).
///
/// A death notice cannot be "stale" the way a task transition can — the worker
/// is still dead — so this says the opposite of the lifecycle notice: the fact
/// is still true, nobody was told, go look.
pub(crate) fn undelivered_worker_died_notice(worker_name: &str) -> String {
    format!(
        "UNDELIVERED: the factory tried to tell the supervisor that worker {worker_name} died \
         and that message never arrived. The worker is still gone and any work it held is \
         unattended. Run `coordination action=worker_status` and re-assign {worker_name}'s \
         tasks — do not assume this was handled."
    )
}

/// cas-3dcb (GH #168): the worker-death relay wire format.
///
/// Producer ([`format_worker_died_relay`], called by orphan recovery) and
/// classifier ([`parse_worker_died_envelope`], called by the daemon's delivery
/// loop) share one definition on purpose. The daemon corroborates a
/// `lifecycle-wake:` source against a self-identifying envelope before it will
/// type anything into the supervisor pane, so a death notice that does not
/// parse here is silently demoted to ordinary chatter — the exact silence this
/// fixes. Keep the two functions adjacent and change them together.
const WORKER_DIED_ENVELOPE_OPEN: &str = "<worker-died ";
const SPAWN_PREASSIGN_FAILED_ENVELOPE_OPEN: &str = "<spawn-preassign-failed ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerDiedEnvelope {
    pub worker_id: String,
    pub worker_name: String,
    /// Death-incident identity — one incident yields one notice (cas-3dcb).
    pub incident: String,
    /// Durable `supervisor_queue.id` exposed to the recipient.
    pub notification_id: i64,
    /// Every registration UUID represented by this relay/batch.
    pub forensic_worker_ids: Vec<String>,
    /// Every durable notification represented by this relay/batch.
    pub coalesced_notification_ids: Vec<i64>,
    /// Tasks actually held when this registration died.
    pub held_tasks: Vec<String>,
    /// Tasks parked back to Open by orphan recovery.
    pub recovered_tasks: Vec<String>,
}

/// Render a worker-death relay for PTY injection into the supervisor's session.
#[allow(clippy::too_many_arguments)]
pub(crate) fn format_worker_died_relay(
    worker_id: &str,
    worker_name: &str,
    incident: &str,
    reason: &str,
    held_tasks: &[String],
    recovered_tasks: &[String],
    notification_id: i64,
) -> String {
    let held = if held_tasks.is_empty() {
        "none".to_string()
    } else {
        held_tasks.join(", ")
    };
    let recovered = if recovered_tasks.is_empty() {
        "none".to_string()
    } else {
        recovered_tasks.join(", ")
    };
    format!(
        "<worker-died worker_id=\"{worker_id}\" worker_name=\"{worker_name}\" \
         incident=\"{incident}\" notification_id=\"{notification_id}\">\n\
         Worker {worker_name} died — {reason}.\n\
         Held at death: {held}\n\
         Parked back to Open: {recovered}\n\
         These tasks are unattended. Re-assign them or respawn a worker; \
         `coordination action=worker_status` shows the current fleet.\n\
         Acknowledge this relay with `coordination action=message_ack \
         notification_id={notification_id}`. (`queue_ack` accepts the same durable ID.)\n\
         </worker-died>"
    )
}

/// Render one bounded summary for duplicate registry rows that expired without
/// holding or parking any task. The first notification remains the actionable
/// durable row; the rest are retained here as forensic identities and marked
/// coalesced in their two queue lanes before transport.
pub(crate) fn format_coalesced_worker_died_relay(
    worker_name: &str,
    worker_ids: &[String],
    incidents: &[String],
    notification_ids: &[i64],
) -> Option<String> {
    let worker_id = worker_ids.first()?;
    let incident = incidents.first()?;
    let notification_id = *notification_ids.first()?;
    Some(format!(
        "<worker-died worker_id=\"{worker_id}\" worker_name=\"{worker_name}\" \
         incident=\"{incident}\" notification_id=\"{notification_id}\" \
         coalesced_count=\"{}\">\n\
         Worker {worker_name} had {} duplicate registry rows expire.\n\
         No tasks were held or parked; no reassignment is required.\n\
         Forensic worker IDs: {}\n\
         Coalesced durable notification IDs: {}\n\
         Acknowledge this batch with `coordination action=message_ack \
         notification_id={notification_id}`. (`queue_ack` accepts the same durable ID.)\n\
         </worker-died>",
        worker_ids.len(),
        worker_ids.len(),
        worker_ids.join(", "),
        notification_ids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    ))
}

fn worker_died_task_line(prompt: &str, prefix: &str) -> Vec<String> {
    let Some(value) = prompt
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(prefix))
    else {
        return Vec::new();
    };
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") || value.is_empty() {
        return Vec::new();
    }
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn worker_died_i64_line(prompt: &str, prefix: &str) -> Vec<i64> {
    worker_died_task_line(prompt, prefix)
        .into_iter()
        .filter_map(|value| value.parse().ok())
        .collect()
}

/// Parse a worker-death relay envelope, if this prompt is one.
pub(crate) fn parse_worker_died_envelope(prompt: &str) -> Option<WorkerDiedEnvelope> {
    let tag_end = prompt.find('>')?;
    let tag = &prompt[..tag_end];
    if !tag.starts_with(WORKER_DIED_ENVELOPE_OPEN) {
        return None;
    }
    let worker_id = xml_attribute(tag, "worker_id")?.to_string();
    let notification_id = xml_attribute(tag, "notification_id")?.parse().ok()?;
    let mut forensic_worker_ids = worker_died_task_line(prompt, "Forensic worker IDs:");
    if forensic_worker_ids.is_empty() {
        forensic_worker_ids.push(worker_id.clone());
    }
    let mut coalesced_notification_ids =
        worker_died_i64_line(prompt, "Coalesced durable notification IDs:");
    if coalesced_notification_ids.is_empty() {
        coalesced_notification_ids.push(notification_id);
    }
    Some(WorkerDiedEnvelope {
        worker_id,
        worker_name: xml_attribute(tag, "worker_name")?.to_string(),
        incident: xml_attribute(tag, "incident")?.to_string(),
        notification_id,
        forensic_worker_ids,
        coalesced_notification_ids,
        held_tasks: worker_died_task_line(prompt, "Held at death:"),
        recovered_tasks: worker_died_task_line(prompt, "Parked back to Open:"),
    })
}

/// Whether a prompt carries an envelope that authorizes a supervisor wake.
///
/// The `lifecycle-wake:` source marker states intent, but `prompt_queue.source`
/// is caller-settable, so the daemon corroborates it with the payload. Exactly
/// Factory health relays use the same constrained envelope boundary as task
/// lifecycle, worker-death, spawn, and CI alerts.
pub(crate) fn is_supervisor_wake_envelope(prompt: &str) -> bool {
    parse_lifecycle_envelope(prompt).is_some()
        || parse_worker_died_envelope(prompt).is_some()
        || parse_spawn_preassign_failed_envelope(prompt)
        || parse_ci_red_run_envelope(prompt)
        || parse_worker_attention_envelope(prompt)
}

/// Worker idle/stall relay emitted by the factory daemon (cas-d4ae).
pub(crate) fn parse_worker_attention_envelope(prompt: &str) -> bool {
    let Some(tag_end) = prompt.find('>') else {
        return false;
    };
    let tag = &prompt[..tag_end];
    tag.starts_with("<worker-attention ")
        && matches!(
            xml_attribute(tag, "kind"),
            Some(
                "worker_idle" | "worker_stalled" | "worker_delivery_stalled" | "worker_unavailable"
            )
        )
        && xml_attribute(tag, "worker").is_some_and(|value| !value.is_empty())
        && xml_attribute(tag, "notification_id").is_some_and(|value| value.parse::<i64>().is_ok())
        && prompt.ends_with("</worker-attention>")
}

/// Spawn pre-assignment failures leave a registered worker idle and need the
/// same supervisor wake/retry semantics as other factory lifecycle failures.
pub(crate) fn parse_spawn_preassign_failed_envelope(prompt: &str) -> bool {
    prompt
        .strip_prefix(SPAWN_PREASSIGN_FAILED_ENVELOPE_OPEN)
        .is_some_and(|rest| {
            rest.contains("task_id=\"")
                && rest.contains("worker_name=\"")
                && rest.contains("notification_id=\"")
                && prompt.contains("</spawn-preassign-failed>")
        })
}

/// CI failures are emitted only by the daemon's GitHub watcher.  Keeping this
/// small, strict envelope check beside the existing lifecycle/death checks
/// preserves the supervisor PTY wake boundary: a caller-settable source alone
/// can never type arbitrary content into the supervisor pane.
pub(crate) fn parse_ci_red_run_envelope(prompt: &str) -> bool {
    let Some(tag_end) = prompt.find('>') else {
        return false;
    };
    let tag = &prompt[..tag_end];
    tag.starts_with("<ci-red-run ")
        && xml_attribute(tag, "branch").is_some_and(|value| !value.is_empty())
        && xml_attribute(tag, "head_sha").is_some_and(|value| !value.is_empty())
        && xml_attribute(tag, "run_id").is_some_and(|value| value.parse::<u64>().is_ok())
        && prompt.ends_with("</ci-red-run>")
}

pub(crate) fn select_unambiguous_merge_task<'a>(
    parked_tasks: &'a [Task],
    worker: &str,
    explicit_task_id: Option<&str>,
) -> Option<&'a Task> {
    let mut matches = parked_tasks.iter().filter(|task| {
        task.status == TaskStatus::AwaitingMerge
            && task
                .assignee
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(worker))
            && explicit_task_id.is_none_or(|id| task.id == id)
    });
    let only = matches.next()?;
    matches.next().is_none().then_some(only)
}

/// The branch a task's merge request is about: the parked branch it was
/// delivered on, else the worker's own `factory/<assignee>`.
///
/// Shared by the compose path and the queued-delivery path (cas-b17c) so the
/// two cannot disagree about which branch to resolve.
pub(crate) fn merge_request_branch(task: Option<&cas_types::Task>) -> Option<String> {
    let task = task?;
    task.deliverables
        .parked_branch
        .clone()
        .or_else(|| task.assignee.as_ref().map(|name| format!("factory/{name}")))
}

/// Resolve the tip a merge request must actually be judged against
/// (cas-b17c / GH #703).
///
/// The recorded `factory_branch_anchor` is evidence about the *previous* merge
/// cycle, not about the branch now. Preferring it meant that after a merge at
/// A, the commits B and C a worker pushed next were revalidated as A — A is an
/// ancestor of the target with its content present, so the request was
/// suppressed with "Merge already landed" and the worker was told an unmerged
/// fix had shipped. That is the reported defect, and it is the one direction
/// this check must never fail in.
///
/// So the tip is resolved live, from the branch itself: the remote ref
/// (`refs/remotes/origin/<branch>`) and the local ref, whichever is newer — the
/// one that has the other as an ancestor. If neither contains the other the
/// branch has diverged and there is no single "the tip", so this returns the
/// remote one and the caller's ancestor check decides on real evidence; if
/// nothing resolves it returns `None`, and the caller must treat that as
/// unverifiable rather than falling back to the anchor. The anchor is kept by
/// callers only as a reported datum so the drift is visible.
pub(crate) fn resolve_live_branch_tip(
    repo_path: &Path,
    branch: &str,
    _recorded_anchor: Option<&str>,
) -> Option<String> {
    use crate::mcp::tools::core::task::lifecycle::close_ops::{
        git_commit_is_ancestor, resolve_branch_sha,
    };

    let remote = resolve_branch_sha(repo_path, &format!("refs/remotes/origin/{branch}"));
    let local = resolve_branch_sha(repo_path, branch);

    match (remote, local) {
        (Some(remote), Some(local)) => {
            if remote == local {
                return Some(remote);
            }
            // Newer wins: the tip that already contains the other is the one a
            // merge request is really about.
            if git_commit_is_ancestor(repo_path, &local, &remote) {
                Some(remote)
            } else if git_commit_is_ancestor(repo_path, &remote, &local) {
                Some(local)
            } else {
                // Diverged. Report the published side; the caller's ancestor
                // and content checks then decide from evidence, and neither
                // side can be declared integrated on the other's behalf.
                Some(remote)
            }
        }
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    }
}

pub(crate) fn revalidate_merge_request(
    repo_path: &Path,
    branch_tip: &str,
    target_branch: &str,
) -> MergeRequestDecision {
    let Some(target_tip) = crate::mcp::tools::core::task::lifecycle::close_ops::resolve_branch_sha(
        repo_path,
        target_branch,
    ) else {
        return MergeRequestDecision::Unverifiable;
    };
    if crate::mcp::tools::core::task::lifecycle::close_ops::git_commit_is_ancestor(
        repo_path,
        branch_tip,
        &target_tip,
    ) {
        match crate::mcp::tools::core::task::lifecycle::close_ops::delivery_content_presence_on_target(
            repo_path,
            branch_tip,
            &target_tip,
        ) {
            crate::mcp::tools::core::task::lifecycle::close_ops::DeliveryContentPresence::Present { .. } => {
                MergeRequestDecision::AlreadyIntegrated { target_tip }
            }
            crate::mcp::tools::core::task::lifecycle::close_ops::DeliveryContentPresence::Superseded { .. } => {
                MergeRequestDecision::AlreadyIntegrated { target_tip }
            }
            // cas-b278: a reachable commit with absent hunks is still pending
            // delivery. Never suppress its supervisor alert as "landed".
            crate::mcp::tools::core::task::lifecycle::close_ops::DeliveryContentPresence::Dropped { .. } => {
                MergeRequestDecision::Pending { target_tip }
            }
            crate::mcp::tools::core::task::lifecycle::close_ops::DeliveryContentPresence::Unknown { .. } => {
                MergeRequestDecision::Unverifiable
            }
        }
    } else {
        MergeRequestDecision::Pending { target_tip }
    }
}

pub(crate) fn attach_merge_request_envelope(
    message: &str,
    envelope: &MergeRequestEnvelope,
) -> String {
    let encoded = serde_json::to_string(envelope)
        .expect("MergeRequestEnvelope contains only JSON-serializable strings");
    format!("{message}\n\n{MERGE_ENVELOPE_OPEN}{encoded}{MERGE_ENVELOPE_CLOSE}")
}

/// Rewrite the opaque merge-request envelope with the target tip observed at
/// delivery time.
///
/// The human body is the worker's original merge request and must remain
/// byte-for-byte intact. Only the machine-readable receipt is refreshed: a
/// queue delay can span several supervisor merges, so the tip captured when
/// the worker enqueued the request is evidence about then, not now.
pub(crate) fn refresh_merge_request_target_tip(prompt: &str, target_tip: &str) -> Option<String> {
    let start = prompt.rfind(MERGE_ENVELOPE_OPEN)? + MERGE_ENVELOPE_OPEN.len();
    let end = prompt[start..].find(MERGE_ENVELOPE_CLOSE)? + start;
    let mut envelope: MergeRequestEnvelope = serde_json::from_str(&prompt[start..end]).ok()?;
    envelope.target_branch_tip = target_tip.to_string();
    let encoded = serde_json::to_string(&envelope).ok()?;
    Some(format!("{}{}{}", &prompt[..start], encoded, &prompt[end..]))
}

pub(crate) fn merge_landed_guidance(
    task_id: &str,
    branch_tip: &str,
    target_branch: &str,
    target_tip: &str,
) -> String {
    format!(
        "Merge already landed; Cassy suppressed your stale merge request.\n\n\
         Task: {task_id}\nBranch tip: {branch_tip}\nTarget: {target_branch} at {target_tip}\n\n\
         Re-run task close for {task_id} now."
    )
}

pub(crate) fn parse_merge_request_envelope(prompt: &str) -> Option<MergeRequestEnvelope> {
    let start = prompt.rfind(MERGE_ENVELOPE_OPEN)? + MERGE_ENVELOPE_OPEN.len();
    let end = prompt[start..].find(MERGE_ENVELOPE_CLOSE)? + start;
    serde_json::from_str(&prompt[start..end]).ok()
}

fn xml_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

pub(crate) fn parse_lifecycle_envelope(prompt: &str) -> Option<LifecycleEnvelope> {
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
    })
}

pub(crate) fn revalidate_lifecycle_prompt(
    prompt: &str,
    current_status: TaskStatus,
    current_updated_at: DateTime<Utc>,
) -> LifecyclePromptDecision {
    let Some(envelope) = parse_lifecycle_envelope(prompt) else {
        return LifecyclePromptDecision::Unstructured;
    };
    if current_status != envelope.new_status {
        return LifecyclePromptDecision::SuppressStale {
            task_id: envelope.task_id,
        };
    }
    // cas-f02b (GH #101) carved out parked states from the equality test below
    // because a parked task keeps accruing writes while it waits. cas-0147
    // (GH #167) measured what the equality test did to every OTHER state and
    // found the carve-out was not a special case — the test was unpassable.
    //
    // Occurrence identity is derived from the caller's `task.updated_at`
    // (`occurrence_from_updated_at`, set at `lifecycle.rs` before the write),
    // but `TaskStore::update` re-stamps `updated_at = Utc::now()` inside the
    // UPDATE and discards the caller's value. Two clock reads means the two
    // timestamps are NEVER equal, so `current_updated_at == occurrence` was
    // false even when revalidated microseconds after the write it describes.
    // Measured on the live queue: of 389 suppressed rows joinable to their
    // task, ZERO matched exactly, 379 had an occurrence strictly earlier than
    // the persisted `updated_at`, and 97 differed only below the millisecond
    // (e.g. cas-0147's own `task_started`: occurrence 00:25:48.953756486 vs
    // updated_at 00:25:48.953778358 — 21.9 microseconds apart). The result was
    // a four-day outage in which 353 of 361 supervisor lifecycle relays —
    // including 34 of 36 `task_awaiting_merge` and 34 of 36
    // `task_close_rejected` — were destroyed before transport.
    //
    // So staleness is the status question and only the status question, which
    // is the one already answered above: while the task is still IN the state
    // the envelope announces, the signal is still true, whatever else has been
    // written to the task since. The cost of relaxing this is at worst one
    // duplicate FYI when a task re-enters the same status while an older
    // notification is still queued; the cost of keeping it was near-total
    // signal loss. This module's stated policy is to fail toward noise.
    //
    // `current_updated_at` stays load-bearing for the one thing it can still
    // decide soundly: a task whose persisted state PREDATES the occurrence the
    // envelope claims cannot have produced that occurrence — the row describes
    // a write that is not in this task's history (replayed, rewound, or
    // addressed to a recycled id). That is genuinely stale.
    if current_updated_at < envelope.occurrence {
        return LifecyclePromptDecision::SuppressStale {
            task_id: envelope.task_id,
        };
    }
    LifecyclePromptDecision::Deliver
}

#[cfg(test)]
mod cas_8aee_assignment_delivery_tests {
    use super::{
        assignment_solicited_task_id, assignment_targets_started_task,
        assignment_targets_terminal_task,
    };
    use cas_types::TaskStatus;

    #[test]
    fn queued_assignment_closed_before_delivery_is_not_startable() {
        let prompt = "You have been assigned a new task:\n\
                      Task ID: cas-8aee\n\
                      Start working: mcp__cas__task action=start id=cas-8aee\n\
                      Then send an ACK to supervisor with your execution plan.";

        assert_eq!(
            assignment_targets_terminal_task(prompt, TaskStatus::Closed).as_deref(),
            Some("cas-8aee"),
            "the queued assignment must be withdrawn rather than render its stale start/ACK instructions"
        );
        assert!(
            assignment_targets_terminal_task(prompt, TaskStatus::InProgress).is_none(),
            "only positive terminal evidence may suppress a worker wake"
        );
    }

    #[test]
    fn queued_spawn_intro_closed_before_delivery_is_not_startable() {
        let prompt = "You were spawned for task cas-bc5c — \"Customer communications\" — and it is assigned to you now.\n\
                      Start with `mcp__cas__task action=show id=cas-bc5c`, then \
                      `mcp__cas__task action=start id=cas-bc5c` before you change any code.";

        assert_eq!(
            assignment_solicited_task_id(prompt).as_deref(),
            Some("cas-bc5c")
        );
        for status in [TaskStatus::Closed, TaskStatus::Cancelled] {
            assert_eq!(
                assignment_targets_terminal_task(prompt, status).as_deref(),
                Some("cas-bc5c"),
                "a terminal spawn intro must not reach any renderer with task-start guidance"
            );
        }
    }

    #[test]
    fn queued_spawn_intro_is_stale_after_the_addressed_worker_started_it() {
        let prompt = "You were spawned for task cas-bc5c — \"Customer communications\" — and it is assigned to you now.\n\
                      Start with `mcp__cas__task action=show id=cas-bc5c`, then \
                      `mcp__cas__task action=start id=cas-bc5c` before you change any code.";
        assert_eq!(
            assignment_targets_started_task(
                prompt,
                TaskStatus::InProgress,
                Some("worker-1"),
                "worker-1",
            )
            .as_deref(),
            Some("cas-bc5c")
        );
        assert!(
            assignment_targets_started_task(
                prompt,
                TaskStatus::InProgress,
                Some("worker-2"),
                "worker-1",
            )
            .is_none(),
            "another worker starting the task is not evidence for this recipient"
        );
        assert!(
            assignment_targets_started_task(
                prompt,
                TaskStatus::Open,
                Some("worker-1"),
                "worker-1",
            )
            .is_none(),
            "an open task still needs its assignment instruction"
        );
    }
}

#[cfg(test)]
mod cas_7787_relay_honesty_tests {
    use super::*;
    use chrono::TimeZone;

    fn awaiting_merge_prompt(task_id: &str, occurrence: DateTime<Utc>) -> String {
        format!(
            "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"{task_id}\" \
             old=\"in_progress\" new=\"awaiting_merge\" actor=\"smooth-octopus-84\" \
             notification_id=\"3386\" occurrence=\"{}\">\nparked\n</task-lifecycle>",
            occurrence.to_rfc3339()
        )
    }

    /// The reported incident, reduced to its decision.
    ///
    /// Replays the exact cas-fe23 shape from session cas-src-fast-pelican-83:
    /// a `task_awaiting_merge` relay enqueued at 18:51:51, never transported,
    /// and revalidated at 18:53:58 after the task had gone
    /// awaiting_merge → closed. The old code called that a plain stale
    /// suppression and the supervisor was never told anything had been lost.
    #[test]
    fn an_awaiting_merge_relay_that_expires_undelivered_is_reported_as_a_failure() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 7, 18, 51, 51).unwrap();
        let closed_at = Utc.with_ymd_and_hms(2026, 8, 7, 18, 53, 58).unwrap();
        let prompt = awaiting_merge_prompt("cas-fe23", occurrence);

        let decision = revalidate_lifecycle_prompt(&prompt, TaskStatus::Closed, closed_at);
        assert!(
            matches!(decision, LifecyclePromptDecision::SuppressStale { .. }),
            "the payload really is stale once the task closed — that part was never wrong"
        );

        assert_eq!(
            lifecycle_stale_outcome(&decision, true, false),
            LifecycleStaleOutcome::UndeliveredRelayFailure {
                task_id: "cas-fe23".to_string()
            },
            "a wake-eligible relay that expired without transport is a delivery FAILURE, \
             not a benign suppression — this is the silence GH #160 reports"
        );
    }

    /// The honesty rule must not become a noise machine: a relay the recipient
    /// demonstrably received (explicit `message_ack`) is a quiet suppression,
    /// exactly as before.
    #[test]
    fn a_relay_the_supervisor_acknowledged_stays_a_quiet_suppression() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 7, 18, 51, 51).unwrap();
        let prompt = awaiting_merge_prompt("cas-fe23", occurrence);
        let decision = revalidate_lifecycle_prompt(
            &prompt,
            TaskStatus::Closed,
            occurrence + chrono::Duration::seconds(127),
        );

        assert_eq!(
            lifecycle_stale_outcome(&decision, true, true),
            LifecycleStaleOutcome::SuppressDelivered {
                task_id: "cas-fe23".to_string()
            },
            "delivered-then-expired must stay silent or the banner trains people to skip it"
        );
    }

    /// Progress FYI (`task_started` / `task_closed`) is not wake-eligible.
    /// Losing one costs the factory nothing, so it must not raise an alarm.
    #[test]
    fn a_non_wake_lifecycle_row_never_raises_the_alarm() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 7, 18, 46, 30).unwrap();
        let prompt = format!(
            "<task-lifecycle transition=\"task_started\" task_id=\"cas-fe23\" \
             old=\"open\" new=\"in_progress\" actor=\"worker\" notification_id=\"3382\" \
             occurrence=\"{}\">\nstarted\n</task-lifecycle>",
            occurrence.to_rfc3339()
        );
        let decision = revalidate_lifecycle_prompt(
            &prompt,
            TaskStatus::Closed,
            occurrence + chrono::Duration::seconds(60),
        );

        assert_eq!(
            lifecycle_stale_outcome(&decision, false, false),
            LifecycleStaleOutcome::SuppressDelivered {
                task_id: "cas-fe23".to_string()
            }
        );
    }

    /// A still-true payload is untouched by the new decision layer — the
    /// delivery path must not change shape for the healthy case.
    #[test]
    fn a_live_relay_is_still_delivered() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 7, 18, 51, 51).unwrap();
        let prompt = awaiting_merge_prompt("cas-fe23", occurrence);
        let decision = revalidate_lifecycle_prompt(
            &prompt,
            TaskStatus::AwaitingMerge,
            occurrence + chrono::Duration::seconds(90),
        );

        assert_eq!(decision, LifecyclePromptDecision::Deliver);
        assert_eq!(
            lifecycle_stale_outcome(&decision, true, false),
            LifecycleStaleOutcome::Deliver
        );
    }

    /// The notice a supervisor actually reads must name the task and refuse to
    /// imply the work was handled.
    #[test]
    fn the_undelivered_notice_names_the_task_and_refuses_to_imply_success() {
        let notice =
            undelivered_relay_notice("cas-edee", Some("task_awaiting_merge: cas-edee (19:26)"));
        assert!(notice.contains("cas-edee"), "must name the task");
        assert!(
            notice.contains("UNDELIVERED"),
            "must state the failure, not describe a queue stage"
        );
        assert!(
            notice.contains("do not assume it was handled"),
            "silence must not read as success"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// cas-f02b (GH #101): a parked task keeps accruing writes while it waits
    /// (a close retry setting `merge_conflicted`, a note, a dependency edit).
    /// Occurrence equality would call the park's own notification stale and
    /// drop it before transport — silently reproducing the stall the
    /// notification exists to prevent.
    #[test]
    fn awaiting_merge_notice_survives_writes_while_the_task_stays_parked() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 6, 2, 10, 0).unwrap();
        let prompt = format!(
            "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-f02b\" \
             old=\"in_progress\" new=\"awaiting_merge\" actor=\"swift-fox\" \
             notification_id=\"41\" occurrence=\"{}\">\nparked\n</task-lifecycle>",
            occurrence.to_rfc3339()
        );

        // Same occurrence: delivered, as before.
        assert!(matches!(
            revalidate_lifecycle_prompt(&prompt, TaskStatus::AwaitingMerge, occurrence),
            LifecyclePromptDecision::Deliver
        ));

        // Later write while STILL parked: still true, still delivered.
        let later = occurrence + chrono::Duration::seconds(90);
        assert!(
            matches!(
                revalidate_lifecycle_prompt(&prompt, TaskStatus::AwaitingMerge, later),
                LifecyclePromptDecision::Deliver
            ),
            "a write while the task stays parked must not silence the merge signal"
        );

        // Left the state: genuinely stale, suppressed.
        assert!(matches!(
            revalidate_lifecycle_prompt(&prompt, TaskStatus::Closed, later),
            LifecyclePromptDecision::SuppressStale { .. }
        ));
    }

    fn started_prompt(task_id: &str, occurrence: DateTime<Utc>) -> String {
        format!(
            "<task-lifecycle transition=\"task_started\" task_id=\"{task_id}\" old=\"open\" \
             new=\"in_progress\" actor=\"w\" notification_id=\"1\" occurrence=\"{}\">\n\
             </task-lifecycle>",
            occurrence.to_rfc3339()
        )
    }

    /// cas-0147 (GH #167) — THE REGRESSION GUARD, replayed from the live row.
    ///
    /// This test used to assert the opposite (`transient_transitions_still_
    /// require_matching_occurrence`). That assertion encoded the defect: it
    /// was written believing a `task_started` row could ever be revalidated
    /// with `updated_at == occurrence`, and it never can, because the
    /// occurrence is formatted from the caller's `task.updated_at` while
    /// `TaskStore::update` re-stamps the column from a second `Utc::now()`.
    ///
    /// The numbers below are cas-0147's own `task_started` row, copied from
    /// the live queue: 21.9 microseconds of drift between the notification and
    /// the task it describes. Under the old test's rule that row — and 353 of
    /// its 361 siblings across four days — was destroyed before transport.
    #[test]
    fn a_microsecond_of_clock_drift_no_longer_destroys_the_notification() {
        let occurrence = Utc
            .with_ymd_and_hms(2026, 8, 8, 0, 25, 48)
            .unwrap()
            .checked_add_signed(chrono::Duration::nanoseconds(953_756_486))
            .expect("occurrence");
        let persisted = Utc
            .with_ymd_and_hms(2026, 8, 8, 0, 25, 48)
            .unwrap()
            .checked_add_signed(chrono::Duration::nanoseconds(953_778_358))
            .expect("persisted updated_at");
        assert_ne!(
            occurrence, persisted,
            "the two clock reads are the whole defect — if they ever compare \
             equal this test has stopped reproducing it"
        );

        assert_eq!(
            revalidate_lifecycle_prompt(
                &started_prompt("cas-0147", occurrence),
                TaskStatus::InProgress,
                persisted,
            ),
            LifecyclePromptDecision::Deliver,
            "a notification revalidated 22 microseconds after the write it \
             describes is not stale"
        );
    }

    /// AC1 in its corrected form: the reason a row died was never that the
    /// recipient was idle, it was that every later write to the task disowned
    /// the notification. A row must stay deliverable across an idle stretch —
    /// re-offered on each poll — for as long as the task is still in the state
    /// it announces, so that it lands whenever the recipient next takes a turn.
    #[test]
    fn a_row_stays_deliverable_while_the_recipient_is_idle_and_the_task_accrues_writes() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 6, 2, 10, 0).unwrap();
        let prompt = started_prompt("cas-x", occurrence);

        // Every poll of a 15-minute idle stretch, with the task picking up
        // unrelated writes (progress notes, lease renewals, dependency edits)
        // the whole time. Not one of them may retire the row.
        for minutes in [0, 1, 5, 15, 60] {
            assert_eq!(
                revalidate_lifecycle_prompt(
                    &prompt,
                    TaskStatus::InProgress,
                    occurrence + chrono::Duration::minutes(minutes),
                ),
                LifecyclePromptDecision::Deliver,
                "poll at +{minutes}m retired a notification whose premise still holds"
            );
        }
    }

    /// The relaxation is not unconditional. `current_updated_at` still decides
    /// one case soundly: a task whose persisted state PREDATES the occurrence
    /// cannot have produced it, so that row describes a write which is not in
    /// this task's history.
    #[test]
    fn an_occurrence_from_the_future_of_the_task_is_still_stale() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 6, 2, 10, 0).unwrap();
        assert!(matches!(
            revalidate_lifecycle_prompt(
                &started_prompt("cas-x", occurrence),
                TaskStatus::InProgress,
                occurrence - chrono::Duration::seconds(1),
            ),
            LifecyclePromptDecision::SuppressStale { .. }
        ));
    }

    /// The status gate is what carries staleness now, so it must still bite.
    #[test]
    fn leaving_the_announced_status_is_still_stale() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 6, 2, 10, 0).unwrap();
        assert!(matches!(
            revalidate_lifecycle_prompt(
                &started_prompt("cas-x", occurrence),
                TaskStatus::Closed,
                occurrence + chrono::Duration::seconds(5),
            ),
            LifecyclePromptDecision::SuppressStale { .. }
        ));
    }

    use chrono::{TimeZone, Utc};
    use std::process::Command;

    fn git(repo: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// GH #703 (cas-b17c) — the reported defect, replayed.
    ///
    /// A merge lands at A, the worker pushes B, and the request for B was
    /// suppressed as "already landed" because revalidation ran against the
    /// recorded anchor A instead of the live branch tip. The tip under test
    /// must be resolved live; the anchor is evidence about the previous cycle.
    #[test]
    fn a_commit_pushed_after_the_merge_is_revalidated_against_the_live_tip() {
        let repo = tempfile::tempdir().expect("temp repo");
        git(repo.path(), &["init", "-b", "main"]);
        git(
            repo.path(),
            &["config", "user.email", "cas-test@example.invalid"],
        );
        git(repo.path(), &["config", "user.name", "Cassy Test"]);
        std::fs::write(repo.path().join("base"), "base\n").expect("base file");
        git(repo.path(), &["add", "base"]);
        git(repo.path(), &["commit", "-m", "base"]);
        git(repo.path(), &["checkout", "-b", "factory/test-worker"]);
        std::fs::write(repo.path().join("work"), "work\n").expect("work file");
        git(repo.path(), &["add", "work"]);
        git(repo.path(), &["commit", "-m", "first"]);
        let anchor = git(repo.path(), &["rev-parse", "factory/test-worker"]);

        // The supervisor merges A and records it as the branch anchor.
        git(repo.path(), &["checkout", "main"]);
        git(
            repo.path(),
            &["merge", "--no-ff", "factory/test-worker", "-m", "merge A"],
        );

        // The worker keeps working: B lands on the branch, unmerged.
        git(repo.path(), &["checkout", "factory/test-worker"]);
        std::fs::write(repo.path().join("more"), "more\n").expect("more file");
        git(repo.path(), &["add", "more"]);
        git(repo.path(), &["commit", "-m", "second"]);
        let live_tip = git(repo.path(), &["rev-parse", "factory/test-worker"]);
        assert_ne!(live_tip, anchor);

        // The anchor still looks merged — this is why anchor-first suppressed
        // the request.
        assert!(matches!(
            revalidate_merge_request(repo.path(), &anchor, "main"),
            MergeRequestDecision::AlreadyIntegrated { .. }
        ));

        // The resolver must return the live tip, not the recorded anchor.
        let resolved = resolve_live_branch_tip(
            repo.path(),
            "factory/test-worker",
            Some(anchor.as_str()),
        )
        .expect("live tip resolves");
        assert_eq!(resolved, live_tip);

        // And the request for it is Pending, i.e. delivered.
        assert!(matches!(
            revalidate_merge_request(repo.path(), &resolved, "main"),
            MergeRequestDecision::Pending { .. }
        ));
    }

    /// Once B is merged too, suppression is correct again.
    #[test]
    fn the_live_tip_suppresses_only_after_it_is_actually_merged() {
        let repo = tempfile::tempdir().expect("temp repo");
        git(repo.path(), &["init", "-b", "main"]);
        git(
            repo.path(),
            &["config", "user.email", "cas-test@example.invalid"],
        );
        git(repo.path(), &["config", "user.name", "Cassy Test"]);
        std::fs::write(repo.path().join("base"), "base\n").expect("base file");
        git(repo.path(), &["add", "base"]);
        git(repo.path(), &["commit", "-m", "base"]);
        git(repo.path(), &["checkout", "-b", "factory/test-worker"]);
        std::fs::write(repo.path().join("work"), "work\n").expect("work file");
        git(repo.path(), &["add", "work"]);
        git(repo.path(), &["commit", "-m", "first"]);
        let anchor = git(repo.path(), &["rev-parse", "factory/test-worker"]);
        std::fs::write(repo.path().join("more"), "more\n").expect("more file");
        git(repo.path(), &["add", "more"]);
        git(repo.path(), &["commit", "-m", "second"]);
        git(repo.path(), &["checkout", "main"]);
        git(
            repo.path(),
            &["merge", "--no-ff", "factory/test-worker", "-m", "merge B"],
        );

        let resolved =
            resolve_live_branch_tip(repo.path(), "factory/test-worker", Some(anchor.as_str()))
                .expect("live tip resolves");
        assert!(matches!(
            revalidate_merge_request(repo.path(), &resolved, "main"),
            MergeRequestDecision::AlreadyIntegrated { .. }
        ));
    }

    /// An anchor with no resolvable branch ref must never be treated as the
    /// live tip: suppressing on it is how an unmerged fix gets told it landed.
    #[test]
    fn an_unresolvable_branch_ref_yields_no_live_tip_rather_than_the_anchor() {
        let repo = tempfile::tempdir().expect("temp repo");
        git(repo.path(), &["init", "-b", "main"]);
        git(
            repo.path(),
            &["config", "user.email", "cas-test@example.invalid"],
        );
        git(repo.path(), &["config", "user.name", "Cassy Test"]);
        std::fs::write(repo.path().join("base"), "base\n").expect("base file");
        git(repo.path(), &["add", "base"]);
        git(repo.path(), &["commit", "-m", "base"]);
        let anchor = git(repo.path(), &["rev-parse", "main"]);

        assert_eq!(
            resolve_live_branch_tip(repo.path(), "factory/never-existed", Some(anchor.as_str())),
            None,
            "an absent branch must not fall back to the recorded anchor"
        );
        // With no live tip there is nothing to suppress on. The exact
        // non-suppressing verdict does not matter — being told "already
        // landed" for an unmerged fix is the one outcome this must never
        // produce, so that is what the test pins.
        assert!(
            !matches!(
                revalidate_merge_request(repo.path(), "factory/never-existed", "main"),
                MergeRequestDecision::AlreadyIntegrated { .. }
            ),
            "an unresolvable branch must never be reported as integrated"
        );
    }

    #[test]
    fn merge_then_stale_request_is_suppressed_with_current_target_tip() {
        let repo = tempfile::tempdir().expect("temp repo");
        git(repo.path(), &["init", "-b", "main"]);
        git(
            repo.path(),
            &["config", "user.email", "cas-test@example.invalid"],
        );
        git(repo.path(), &["config", "user.name", "Cassy Test"]);
        std::fs::write(repo.path().join("base"), "base\n").expect("base file");
        git(repo.path(), &["add", "base"]);
        git(repo.path(), &["commit", "-m", "base"]);
        git(repo.path(), &["checkout", "-b", "factory/test-worker"]);
        std::fs::write(repo.path().join("work"), "work\n").expect("work file");
        git(repo.path(), &["add", "work"]);
        git(repo.path(), &["commit", "-m", "work"]);
        let worker_tip = git(repo.path(), &["rev-parse", "factory/test-worker"]);

        // Reproduce the production ordering: the supervisor merges first, but
        // the worker's already-composed merge request reaches delivery later.
        git(repo.path(), &["checkout", "main"]);
        git(
            repo.path(),
            &["merge", "--no-ff", "factory/test-worker", "-m", "merge"],
        );
        let target_tip = git(repo.path(), &["rev-parse", "main"]);

        assert_eq!(
            revalidate_merge_request(repo.path(), &worker_tip, "main"),
            MergeRequestDecision::AlreadyIntegrated {
                target_tip: target_tip.clone(),
            }
        );

        let envelope = MergeRequestEnvelope {
            task_id: "cas-test".to_string(),
            branch_tip: worker_tip,
            target_branch: "main".to_string(),
            target_branch_tip: target_tip,
        anchor_tip: None,
        };
        assert_eq!(
            parse_merge_request_envelope(&attach_merge_request_envelope("please merge", &envelope)),
            Some(envelope)
        );
        let guidance = merge_landed_guidance(
            "cas-test",
            &git(repo.path(), &["rev-parse", "factory/test-worker"]),
            "main",
            &git(repo.path(), &["rev-parse", "main"]),
        );
        assert!(guidance.contains("Merge already landed"));
        assert!(guidance.contains("Re-run task close for cas-test now"));
    }

    #[test]
    fn reachable_request_with_dropped_content_is_not_suppressed_cas_b278() {
        let repo = tempfile::tempdir().expect("temp repo");
        git(repo.path(), &["init", "-b", "main"]);
        git(
            repo.path(),
            &["config", "user.email", "cas-test@example.invalid"],
        );
        git(repo.path(), &["config", "user.name", "Cassy Test"]);
        std::fs::write(repo.path().join("base"), "base\n").expect("base file");
        git(repo.path(), &["add", "base"]);
        git(repo.path(), &["commit", "-m", "base"]);
        let base = git(repo.path(), &["rev-parse", "HEAD"]);
        git(repo.path(), &["checkout", "-b", "factory/test-worker"]);
        std::fs::write(repo.path().join("credits.rs"), "restore grant\n").expect("delivery file");
        git(repo.path(), &["add", "credits.rs"]);
        git(repo.path(), &["commit", "-m", "restore grant"]);
        let worker_tip = git(repo.path(), &["rev-parse", "HEAD"]);

        git(repo.path(), &["checkout", "-b", "factory/other", &base]);
        std::fs::write(repo.path().join("credits.rs"), "competing credits work\n")
            .expect("competing file");
        git(repo.path(), &["add", "credits.rs"]);
        git(repo.path(), &["commit", "-m", "competing credits work"]);

        git(repo.path(), &["checkout", "main"]);
        git(
            repo.path(),
            &["merge", "--no-ff", "factory/test-worker", "-m", "merge"],
        );
        let conflict = std::process::Command::new("git")
            .args(["merge", "--no-ff", "factory/other"])
            .current_dir(repo.path())
            .status()
            .expect("start conflicting merge");
        assert!(!conflict.success(), "fixture must conflict");
        std::fs::write(repo.path().join("credits.rs"), "competing credits work\n")
            .expect("resolve without delivery");
        git(repo.path(), &["add", "credits.rs"]);
        git(repo.path(), &["commit", "-m", "merge drops restore"]);
        let target_tip = git(repo.path(), &["rev-parse", "main"]);

        assert!(
            crate::mcp::tools::core::task::lifecycle::close_ops::git_commit_is_ancestor(
                repo.path(),
                &worker_tip,
                "main"
            )
        );
        assert_eq!(
            revalidate_merge_request(repo.path(), &worker_tip, "main"),
            MergeRequestDecision::Pending { target_tip },
            "a reachable commit with absent content must keep the supervisor merge/correction relay live"
        );
    }

    #[test]
    fn genuinely_unmerged_request_remains_pending() {
        let repo = tempfile::tempdir().expect("temp repo");
        git(repo.path(), &["init", "-b", "main"]);
        git(
            repo.path(),
            &["config", "user.email", "cas-test@example.invalid"],
        );
        git(repo.path(), &["config", "user.name", "Cassy Test"]);
        std::fs::write(repo.path().join("base"), "base\n").expect("base file");
        git(repo.path(), &["add", "base"]);
        git(repo.path(), &["commit", "-m", "base"]);
        let target_tip = git(repo.path(), &["rev-parse", "main"]);
        git(repo.path(), &["checkout", "-b", "factory/test-worker"]);
        std::fs::write(repo.path().join("work"), "work\n").expect("work file");
        git(repo.path(), &["add", "work"]);
        git(repo.path(), &["commit", "-m", "work"]);
        let worker_tip = git(repo.path(), &["rev-parse", "factory/test-worker"]);

        assert_eq!(
            revalidate_merge_request(repo.path(), &worker_tip, "main"),
            MergeRequestDecision::Pending { target_tip }
        );
    }

    /// cas-e3be (GH #260): a merge request can wait in the queue while the
    /// target branch advances. The body sent at pop time must retain the
    /// worker's request but name the tip resolved then, rather than the
    /// enqueue-time snapshot.
    #[test]
    fn queued_merge_request_refreshes_target_tip_at_delivery() {
        let repo = tempfile::tempdir().expect("temp repo");
        git(repo.path(), &["init", "-b", "main"]);
        git(
            repo.path(),
            &["config", "user.email", "cas-test@example.invalid"],
        );
        git(repo.path(), &["config", "user.name", "Cassy Test"]);
        std::fs::write(repo.path().join("base"), "base\n").expect("base file");
        git(repo.path(), &["add", "base"]);
        git(repo.path(), &["commit", "-m", "base"]);
        let enqueue_target_tip = git(repo.path(), &["rev-parse", "main"]);

        git(repo.path(), &["checkout", "-b", "factory/test-worker"]);
        std::fs::write(repo.path().join("work"), "work\n").expect("work file");
        git(repo.path(), &["add", "work"]);
        git(repo.path(), &["commit", "-m", "work"]);
        let worker_tip = git(repo.path(), &["rev-parse", "factory/test-worker"]);
        let queued = attach_merge_request_envelope(
            "Please merge my conflict fix.",
            &MergeRequestEnvelope {
                task_id: "cas-e3be".to_string(),
                branch_tip: worker_tip.clone(),
                target_branch: "main".to_string(),
                target_branch_tip: enqueue_target_tip.clone(),
            anchor_tip: None,
            },
        );

        // The task is still awaiting merge, but unrelated work landed before
        // this queue row reaches the supervisor.
        git(repo.path(), &["checkout", "main"]);
        std::fs::write(repo.path().join("later"), "later\n").expect("later file");
        git(repo.path(), &["add", "later"]);
        git(repo.path(), &["commit", "-m", "advance target"]);
        let delivery_target_tip = git(repo.path(), &["rev-parse", "main"]);
        assert_ne!(enqueue_target_tip, delivery_target_tip);

        assert_eq!(
            revalidate_merge_request(repo.path(), &worker_tip, "main"),
            MergeRequestDecision::Pending {
                target_tip: delivery_target_tip.clone(),
            }
        );
        let injected = refresh_merge_request_target_tip(&queued, &delivery_target_tip)
            .expect("structured queued merge request refreshes at pop");
        assert!(injected.starts_with("Please merge my conflict fix."));
        assert_eq!(
            parse_merge_request_envelope(&injected),
            Some(MergeRequestEnvelope {
                task_id: "cas-e3be".to_string(),
                branch_tip: worker_tip,
                target_branch: "main".to_string(),
                target_branch_tip: delivery_target_tip,
            anchor_tip: None,
            })
        );
    }

    #[test]
    fn stale_blocked_occurrence_is_suppressed_after_task_resumes() {
        let occurrence = Utc
            .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
            .single()
            .expect("timestamp");
        let prompt = format!(
            "<task-lifecycle transition=\"task_blocked\" task_id=\"cas-test\" old=\"in_progress\" new=\"blocked\" actor=\"worker\" notification_id=\"1\" occurrence=\"{}\">\nTask blocked\n</task-lifecycle>",
            occurrence.to_rfc3339()
        );
        let current_updated_at = occurrence + chrono::Duration::seconds(1);

        assert_eq!(
            revalidate_lifecycle_prompt(&prompt, TaskStatus::InProgress, current_updated_at),
            LifecyclePromptDecision::SuppressStale {
                task_id: "cas-test".to_string(),
            }
        );
    }

    #[test]
    fn current_blocked_occurrence_is_delivered() {
        let occurrence = Utc
            .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
            .single()
            .expect("timestamp");
        let prompt = format!(
            "<task-lifecycle transition=\"task_blocked\" task_id=\"cas-test\" old=\"in_progress\" new=\"blocked\" actor=\"worker\" notification_id=\"1\" occurrence=\"{}\">\nTask blocked\n</task-lifecycle>",
            occurrence.to_rfc3339()
        );
        assert_eq!(
            revalidate_lifecycle_prompt(&prompt, TaskStatus::Blocked, occurrence),
            LifecyclePromptDecision::Deliver
        );
    }

    #[test]
    fn unstructured_messages_are_not_revalidated() {
        assert_eq!(
            parse_merge_request_envelope("ordinary free-form message"),
            None
        );
        assert_eq!(parse_lifecycle_envelope("ordinary free-form message"), None);
    }

    fn merge_envelope() -> MergeRequestEnvelope {
        MergeRequestEnvelope {
            task_id: "cas-test".to_string(),
            branch_tip: "worker-tip".to_string(),
            target_branch: "main".to_string(),
            target_branch_tip: "base-tip".to_string(),
        anchor_tip: None,
        }
    }

    fn merge_task(status: TaskStatus, anchor: Option<&str>) -> Task {
        let mut task = Task::new("cas-test".to_string(), "merge test".to_string());
        task.status = status;
        task.deliverables.factory_branch_anchor = anchor.map(str::to_string);
        task
    }

    /// cas-6eab / GH #61: the reported sequence — supervisor merges and
    /// replies, THEN the worker's already-composed request reaches transport.
    /// It must be suppressed rather than delivered as an actionable ask.
    #[test]
    fn merge_request_delivered_after_the_merge_is_suppressed() {
        let task = merge_task(TaskStatus::AwaitingMerge, Some("worker-tip"));
        assert_eq!(
            merge_request_delivery_decision(
                Some(&task),
                &merge_envelope(),
                &MergeRequestDecision::AlreadyIntegrated {
                    target_tip: "abc123".to_string(),
                },
            ),
            MergeRequestDelivery::SuppressLanded {
                target_tip: "abc123".to_string(),
            }
        );
    }

    /// The same class one step further along: the merge landed AND the task
    /// was already re-closed. Status alone settles it without trusting git.
    #[test]
    fn merge_request_for_a_task_that_left_awaiting_merge_is_moot() {
        for status in [
            TaskStatus::Closed,
            TaskStatus::InProgress,
            TaskStatus::Open,
        ] {
            let task = merge_task(status, Some("worker-tip"));
            assert_eq!(
                merge_request_delivery_decision(
                    Some(&task),
                    &merge_envelope(),
                    &MergeRequestDecision::Pending {
                        target_tip: "abc123".to_string(),
                    },
                ),
                MergeRequestDelivery::SuppressResolved { status },
                "a task at {status} has nothing queued for the supervisor"
            );
        }
        let guidance = merge_request_moot_guidance("cas-test", TaskStatus::Closed);
        assert!(guidance.contains("cas-test"));
        assert!(guidance.contains("no longer awaiting a merge"));
    }

    /// A genuinely outstanding merge must still reach the supervisor — and so
    /// must anything Cassy cannot verify. Suppression requires positive evidence.
    #[test]
    fn outstanding_and_unverifiable_merge_requests_are_delivered() {
        let task = merge_task(TaskStatus::AwaitingMerge, Some("worker-tip"));
        assert_eq!(
            merge_request_delivery_decision(
                Some(&task),
                &merge_envelope(),
                &MergeRequestDecision::Pending {
                    target_tip: "abc123".to_string(),
                },
            ),
            MergeRequestDelivery::Deliver
        );
        assert_eq!(
            merge_request_delivery_decision(
                Some(&task),
                &merge_envelope(),
                &MergeRequestDecision::Unverifiable,
            ),
            MergeRequestDelivery::Deliver,
            "unverifiable git state must never suppress a merge request"
        );
        assert_eq!(
            merge_request_delivery_decision(
                None,
                &merge_envelope(),
                &MergeRequestDecision::Unverifiable,
            ),
            MergeRequestDelivery::Deliver,
            "an unreadable task is uncertainty, not evidence of staleness"
        );
        assert_eq!(
            merge_request_delivery_decision(
                None,
                &merge_envelope(),
                &MergeRequestDecision::AlreadyIntegrated {
                    target_tip: "abc123".to_string(),
                },
            ),
            MergeRequestDelivery::SuppressLanded {
                target_tip: "abc123".to_string(),
            },
            "git reachability is positive evidence even when the task is unreadable"
        );
    }

    /// GH #340: request_changes clears the anchor before a queued worker
    /// message reaches the supervisor. Even if the task later returns to
    /// AwaitingMerge, the old request must never regain authority.
    #[test]
    fn invalidated_delivery_anchor_suppresses_a_queued_merge_request() {
        let task = merge_task(TaskStatus::AwaitingMerge, None);
        assert_eq!(
            merge_request_delivery_decision(
                Some(&task),
                &merge_envelope(),
                &MergeRequestDecision::Pending {
                    target_tip: "base-tip".to_string(),
                },
            ),
            MergeRequestDelivery::SuppressInvalidatedAnchor {
                current_anchor: None,
            }
        );
        let guidance = merge_request_anchor_invalidated_guidance("cas-test", "worker-tip", None);
        assert!(guidance.contains("worker-tip"));
        assert!(guidance.contains("no longer current"));
    }

    #[test]
    fn prior_cycle_request_stays_suppressed_after_a_new_anchor_is_parked() {
        let task = merge_task(TaskStatus::AwaitingMerge, Some("replacement-tip"));
        assert_eq!(
            merge_request_delivery_decision(
                Some(&task),
                &merge_envelope(),
                &MergeRequestDecision::Pending {
                    target_tip: "base-tip".to_string(),
                },
            ),
            MergeRequestDelivery::SuppressInvalidatedAnchor {
                current_anchor: Some("replacement-tip".to_string()),
            },
            "a later AwaitingMerge cycle must not revive the declined request"
        );
    }

    #[test]
    fn exactly_one_parked_task_is_inferred_but_ambiguity_is_left_unstructured() {
        let mut first = cas_types::Task::new("cas-one".to_string(), "one".to_string());
        first.status = TaskStatus::AwaitingMerge;
        first.assignee = Some("worker-a".to_string());
        let mut second = cas_types::Task::new("cas-two".to_string(), "two".to_string());
        second.status = TaskStatus::AwaitingMerge;
        second.assignee = Some("worker-a".to_string());

        assert_eq!(
            select_unambiguous_merge_task(&[first.clone()], "worker-a", None)
                .map(|task| task.id.as_str()),
            Some("cas-one")
        );
        assert!(
            select_unambiguous_merge_task(&[first.clone(), second], "worker-a", None).is_none(),
            "ambiguous implicit merge context must preserve free-form delivery"
        );
        assert_eq!(
            select_unambiguous_merge_task(&[first], "worker-a", Some("cas-one"))
                .map(|task| task.id.as_str()),
            Some("cas-one")
        );
    }
}

#[cfg(test)]
mod cas_3dcb_worker_died_relay_tests {
    use super::*;

    fn relay() -> String {
        format_worker_died_relay(
            "6f1b-agent-id",
            "mighty-kestrel-57",
            "worker_died:6f1b-agent-id:1754600000000",
            "daemon maintenance: heartbeat stale",
            &["cas-aaaa".to_string(), "cas-bbbb".to_string()],
            &["cas-aaaa".to_string()],
            4211,
        )
    }

    /// Producer and classifier must agree, or the daemon silently demotes the
    /// death notice to ordinary chatter — the GH #168 silence.
    #[test]
    fn producer_output_round_trips_through_the_classifier() {
        let envelope = parse_worker_died_envelope(&relay()).expect("relay must parse");
        assert_eq!(envelope.worker_id, "6f1b-agent-id");
        assert_eq!(envelope.worker_name, "mighty-kestrel-57");
        assert_eq!(envelope.incident, "worker_died:6f1b-agent-id:1754600000000");
    }

    /// The body must carry the facts a supervisor acts on, not just the tag.
    #[test]
    fn relay_body_names_the_worker_and_its_unattended_work() {
        let body = relay();
        assert!(body.contains("mighty-kestrel-57"));
        assert!(body.contains("heartbeat stale"));
        assert!(body.contains("cas-aaaa") && body.contains("cas-bbbb"));
    }

    #[test]
    fn a_death_with_no_held_work_still_renders() {
        let body =
            format_worker_died_relay("id", "idle-worker", "incident", "shutdown", &[], &[], 1);
        assert!(parse_worker_died_envelope(&body).is_some());
        assert!(body.contains("Held at death: none"));
    }

    /// Wake eligibility is corroborated by the payload, so both producers must
    /// qualify and arbitrary text must not.
    #[test]
    fn wake_corroboration_accepts_both_envelopes_and_nothing_else() {
        assert!(is_supervisor_wake_envelope(&relay()));
        assert!(is_supervisor_wake_envelope(
            "<worker-attention kind=\"worker_stalled\" worker=\"calm-owl\" notification_id=\"42\">\nbody</worker-attention>"
        ));
        assert!(is_supervisor_wake_envelope(
            "<worker-attention kind=\"worker_delivery_stalled\" worker=\"calm-owl\" notification_id=\"42\">\nbody</worker-attention>"
        ));
        assert!(is_supervisor_wake_envelope(
            "<worker-attention kind=\"worker_unavailable\" worker=\"calm-owl\" notification_id=\"42\">\nbody</worker-attention>"
        ));
        assert!(is_supervisor_wake_envelope(
            "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-1\" old=\"in_progress\" \
             new=\"awaiting_merge\" actor=\"w\" notification_id=\"1\" \
             occurrence=\"2026-08-07T10:00:00+00:00\">\nbody</task-lifecycle>"
        ));
        assert!(!is_supervisor_wake_envelope("please merge my branch"));
        // A lookalike that omits required attributes must not qualify.
        assert!(!is_supervisor_wake_envelope("<worker-died >gotcha"));
        assert!(!is_supervisor_wake_envelope(
            "<worker-attention kind=\"unknown\" worker=\"calm-owl\" notification_id=\"42\">body</worker-attention>"
        ));
        assert!(!is_supervisor_wake_envelope(
            "<worker-diedish worker_id=\"a\" worker_name=\"b\" incident=\"c\">x"
        ));
    }

    #[test]
    fn undelivered_notice_names_the_worker_not_a_task() {
        let notice = undelivered_worker_died_notice("mighty-kestrel-57");
        assert!(notice.contains("mighty-kestrel-57"));
        assert!(notice.contains("UNDELIVERED"));
        assert!(!notice.contains("(unknown task)"));
    }
}

/// cas-ec74 — the producer half of cas-0147, proved end to end.
///
/// cas-0147 fixed the CONSUMER (this module's staleness gate) so a
/// microsecond of clock drift no longer destroyed a notification. cas-ec74
/// fixes the PRODUCER so the drift does not exist in the first place: the
/// store returns the stamp it persisted, and the occurrence is derived from
/// that return value instead of from a second `Utc::now()`.
///
/// These tests drive a real `SqliteTaskStore` rather than fabricated
/// timestamps, so they fail if either half regresses — a store that stops
/// returning its persisted stamp, or a gate that goes back to exact equality.
#[cfg(test)]
mod cas_ec74_producer_round_trip_tests {
    use super::*;
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::occurrence_from_updated_at;
    use crate::types::Task;
    use cas_store::{SqliteTaskStore, TaskStore};

    fn started_prompt(task_id: &str, occurrence: &str) -> String {
        format!(
            "<task-lifecycle transition=\"task_started\" task_id=\"{task_id}\" old=\"open\" \
             new=\"in_progress\" actor=\"w\" notification_id=\"1\" occurrence=\"{occurrence}\">\n\
             </task-lifecycle>"
        )
    }

    /// The full producer -> consumer round trip: an occurrence derived from
    /// `update()`'s RETURN value is accepted by the staleness gate, and the
    /// occurrence is byte-identical to the persisted `updated_at` — the exact
    /// match that was impossible for four days.
    #[test]
    fn an_occurrence_derived_from_the_returned_stamp_matches_the_stored_row() {
        let temp = tempfile::TempDir::new().unwrap();
        let store = SqliteTaskStore::open(temp.path()).unwrap();
        store.init().unwrap();

        let id = store.generate_id().unwrap();
        let mut task = Task::new(id.clone(), "producer round trip".to_string());
        store.add(&task).unwrap();

        // Exactly what lifecycle.rs does on start: stamp, write, adopt the
        // store's answer, derive the occurrence from it.
        task.status = TaskStatus::InProgress;
        task.updated_at = Utc::now();
        task.updated_at = store.update(&task).unwrap();
        let occurrence = occurrence_from_updated_at(task.updated_at);

        let persisted = store.get(&id).unwrap().updated_at;
        assert_eq!(
            occurrence,
            occurrence_from_updated_at(persisted),
            "producer and consumer must share ONE clock read — if these differ, \
             the second Utc::now() is back"
        );

        assert_eq!(
            revalidate_lifecycle_prompt(
                &started_prompt(&id, &occurrence),
                TaskStatus::InProgress,
                persisted,
            ),
            LifecyclePromptDecision::Deliver,
            "the notification describing this very write must survive the gate"
        );
    }

    /// AC4 restated as a guard: with the producer fixed, the rewound-occurrence
    /// check cas-0147 kept (`current_updated_at < occurrence`) must still bite.
    /// A correct producer must not make the gate toothless.
    #[test]
    fn a_rewound_occurrence_is_still_rejected_after_the_producer_fix() {
        let temp = tempfile::TempDir::new().unwrap();
        let store = SqliteTaskStore::open(temp.path()).unwrap();
        store.init().unwrap();

        let id = store.generate_id().unwrap();
        let mut task = Task::new(id.clone(), "rewound".to_string());
        store.add(&task).unwrap();
        task.status = TaskStatus::InProgress;
        let persisted = store.update(&task).unwrap();

        // An occurrence from a write that is not in this task's history.
        let from_the_future = occurrence_from_updated_at(persisted + chrono::Duration::seconds(30));

        assert_eq!(
            revalidate_lifecycle_prompt(
                &started_prompt(&id, &from_the_future),
                TaskStatus::InProgress,
                persisted,
            ),
            LifecyclePromptDecision::SuppressStale {
                task_id: id.clone()
            },
            "a task whose persisted state predates the announced occurrence \
             cannot have produced it"
        );
    }
}
