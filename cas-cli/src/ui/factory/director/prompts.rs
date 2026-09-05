//! Auto-prompting system for the Director
//!
//! Generates prompts based on detected Cassy state changes and injects them
//! into the appropriate agent's terminal.

use std::collections::HashSet;
use std::path::Path;

use crate::config::AutoPromptConfig;
use crate::mcp::tools::core::task::lifecycle::close_ops::{
    KnownUnmergedCount, fetch_parent_branch_best_effort, known_unmerged_factory_commits,
    resolve_ref_commit_sha,
};
use crate::ui::factory::director::data::{ActiveLeaseSummary, DirectorData, TaskSummary};
use crate::ui::factory::director::events::DirectorEvent;
use cas_mux::SupervisorCli;
use cas_types::{TaskStatus, TaskType};

/// Task ids that are Open but have at least one unmet `Blocks` dependency (a
/// blocker task whose status isn't Closed). Mirrors the exact semantics of
/// `TaskStore::list_ready()`'s SQL predicate (`crates/cas-store`), which
/// `DirectorData.ready_tasks` does NOT apply — that bucket only splits on
/// `task.status` (see `crates/cas-factory/src/director.rs`), so a
/// discussion-gated/dependency-blocked task can otherwise leak into
/// `dispatchable_ready_count` and get surfaced as "ready tasks exist —
/// assign" even though the live `task action=ready` query would correctly
/// exclude it (cas-09d0 bug report point 3).
///
/// `non_closed_task_ids` should be every task id NOT in a Closed state —
/// derivable from `ready_tasks ∪ in_progress_tasks ∪ epic_tasks`, since
/// together those three buckets exhaustively cover every non-closed
/// `TaskStatus` (see the bucketing switch in `director.rs::load_with_stores`).
/// A blocker id absent from that set is therefore closed (or no longer
/// exists), matching `list_ready()`'s `blocker.status != 'closed'` check.
pub fn compute_gated_task_ids(
    non_closed_task_ids: &HashSet<&str>,
    blocks_deps: &[cas_types::Dependency],
) -> HashSet<String> {
    blocks_deps
        .iter()
        .filter(|d| d.dep_type == cas_types::DependencyType::Blocks)
        .filter(|d| non_closed_task_ids.contains(d.to_id.as_str()))
        .map(|d| d.from_id.clone())
        .collect()
}

/// Count tasks that are actually dispatchable to an idle worker.
///
/// `DirectorData::ready_tasks` conflates `Open` and `Blocked` (see
/// `crates/cas-factory/src/director.rs`). Blocked tasks cannot be started, and
/// Closed tasks never appear in `ready_tasks` at all, but this count decides
/// whether the `WorkerIdle` / `AgentRegistered` prompts should offer an assign
/// command. Count only `Open`, unassigned, and not dependency-gated (cas-09d0)
/// tasks. See cas-177f.
fn dispatchable_ready_count(data: &DirectorData, gated_task_ids: &HashSet<String>) -> usize {
    data.ready_tasks
        .iter()
        .filter(|t| {
            t.status == TaskStatus::Open && t.assignee.is_none() && !gated_task_ids.contains(&t.id)
        })
        .count()
}

/// Render the instant a delivery-time snapshot was read, for notices that
/// report an absence ("no dispatchable tasks") — the one claim whose truth
/// decays (cas-ae6d, GH #100). Second-resolution UTC keeps it readable and
/// directly comparable to `task action=ready` output timestamps.
pub(crate) fn snapshot_stamp(snapshot_at: chrono::DateTime<chrono::Utc>) -> String {
    format!("snapshot {}", snapshot_at.format("%Y-%m-%d %H:%M:%SZ"))
}

fn live_worker_session_id(data: &DirectorData, worker_name: &str) -> Option<String> {
    data.agents
        .iter()
        .find(|agent| agent.name == worker_name)
        .map(|agent| agent.id.clone())
        .or_else(|| {
            data.agent_id_to_name
                .iter()
                .find_map(|(id, name)| (name == worker_name).then(|| id.clone()))
        })
}

fn task_assigned_to_worker(data: &DirectorData, task: &TaskSummary, worker: &str) -> bool {
    task.assignee.as_deref() == Some(worker)
        || data
            .agent_id_to_name
            .iter()
            .any(|(id, name)| name == worker && task.assignee.as_deref() == Some(id.as_str()))
}

/// cas-ed6c: `pub(crate)` re-export of the same lease-independent
/// assignment predicate the `WorkerIdle` delivery-time revalidation arm
/// already uses (`revalidate_event_for_delivery_with_context`), so the
/// inbox-retraction sweep (`TeamsManager::prune_stale_idle_alerts`, wired
/// in `lifecycle.rs`) can never disagree with it about what "this worker
/// is no longer idle" means. Checks the task store's InProgress +
/// open-Ready assignee fields directly — independent of the lease table,
/// which cas-d165 already established goes blind mid-task (leases expire
/// ~30 min in; a worker can hold a genuine assignment with no active
/// lease at all).
pub fn worker_now_has_real_assignment(data: &DirectorData, worker: &str) -> bool {
    worker_has_open_or_in_progress_assignment(data, worker)
}

/// Whether an `EpicAllSubtasksClosed` occurrence is still actionable.
///
/// Only positive, authoritative evidence makes an occurrence stale: the epic
/// is now Closed, or a non-closed subtask is present again. A missing epic is
/// unverifiable rather than stale, so it deliberately returns `true`. This
/// fail-open direction keeps transient/incomplete snapshots from suppressing
/// or retracting a legitimate supervisor notification.
pub(crate) fn epic_completion_is_current(data: &DirectorData, epic_id: &str) -> bool {
    let Some(epic) = data.epic_tasks.iter().find(|epic| epic.id == epic_id) else {
        return true;
    };
    if matches!(epic.status, TaskStatus::Closed | TaskStatus::Cancelled) {
        return false;
    }

    !data
        .ready_tasks
        .iter()
        .chain(data.in_progress_tasks.iter())
        .any(|task| {
            task.epic.as_deref() == Some(epic_id)
                && !matches!(task.status, TaskStatus::Closed | TaskStatus::Cancelled)
        })
}

fn worker_has_open_or_in_progress_assignment(data: &DirectorData, worker: &str) -> bool {
    data.in_progress_tasks
        .iter()
        .chain(
            data.ready_tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Open),
        )
        .any(|task| task_assigned_to_worker(data, task, worker))
}

/// How epic-completion ownership was resolved (cas-9fff).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpicCompletionOwnershipSource {
    /// `Task.epic_verification_owner` matched this supervisor.
    VerificationOwner,
    /// Inferred: this session's agents worked the epic (assignees) or the
    /// session is focused on it.
    SessionAffinity,
    /// Owner is known but not live here; deliver only as an explicit
    /// last-resort fallback (never silent).
    UnreachableOwnerFallback,
    /// No ownership signal at all — legacy single-session path.
    Unresolved,
}

/// Routing decision for `EpicAllSubtasksClosed` prompts (cas-9fff).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpicCompletionRoute {
    /// Deliver to this session's supervisor.
    Deliver {
        owner: String,
        source: EpicCompletionOwnershipSource,
        owner_session: Option<String>,
    },
    /// Suppress — another supervisor owns this epic.
    Suppress { reason: &'static str },
}

/// Decide whether this factory session's supervisor should receive an
/// epic-completion notification.
///
/// Preference order (per cas-9fff design):
/// 1. `epic_verification_owner` (exact agent id or display name)
/// 2. Session affinity (subtask assignees visible as agents in this session,
///    or `focused_epic_id` matches)
/// 3. Unreachable-owner fallback only when explicitly requested by the caller
///    (`allow_unreachable_fallback`) and this session has affinity
/// 4. Epic present but no affinity → suppress (foreign concurrent session)
/// 5. Epic absent from snapshot → deliver unresolved (legacy/tests)
///
/// Concurrent supervisors: a non-owning session always gets `Suppress`.
pub fn route_epic_completion(
    supervisor_name: &str,
    supervisor_id: Option<&str>,
    factory_session: Option<&str>,
    epic_verification_owner: Option<&str>,
    focused_on_epic: bool,
    session_has_epic_workers: bool,
    owner_live_in_this_session: bool,
    allow_unreachable_fallback: bool,
    epic_present_in_snapshot: bool,
) -> EpicCompletionRoute {
    let self_ids: Vec<&str> = std::iter::once(supervisor_name)
        .chain(supervisor_id)
        .collect();

    if let Some(owner) = epic_verification_owner
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let is_owner = self_ids.iter().any(|id| *id == owner);
        if is_owner {
            return EpicCompletionRoute::Deliver {
                owner: owner.to_string(),
                source: EpicCompletionOwnershipSource::VerificationOwner,
                owner_session: factory_session.map(str::to_string),
            };
        }
        // Owner is someone else. Only fall back if they are unreachable *and*
        // this session has affinity (worked the epic / focused it).
        if !owner_live_in_this_session
            && allow_unreachable_fallback
            && (session_has_epic_workers || focused_on_epic)
        {
            return EpicCompletionRoute::Deliver {
                owner: owner.to_string(),
                source: EpicCompletionOwnershipSource::UnreachableOwnerFallback,
                owner_session: None,
            };
        }
        return EpicCompletionRoute::Suppress {
            reason: "epic_verification_owner is a different supervisor",
        };
    }

    // No explicit owner — infer from this session's affinity.
    if session_has_epic_workers || focused_on_epic {
        return EpicCompletionRoute::Deliver {
            owner: supervisor_name.to_string(),
            source: EpicCompletionOwnershipSource::SessionAffinity,
            owner_session: factory_session.map(str::to_string),
        };
    }

    // Epic is visible but this session has no claim — concurrent foreign epic.
    if epic_present_in_snapshot {
        return EpicCompletionRoute::Suppress {
            reason: "no ownership affinity for epic in this session",
        };
    }

    // Epic not in snapshot (unit tests / degraded load): deliver with explicit
    // unresolved stamp so the recipient can still self-filter.
    EpicCompletionRoute::Deliver {
        owner: supervisor_name.to_string(),
        source: EpicCompletionOwnershipSource::Unresolved,
        owner_session: factory_session.map(str::to_string),
    }
}

/// Ownership inputs for an epic from a director snapshot.
#[derive(Debug, Clone)]
pub struct EpicCompletionContext {
    pub owner: Option<String>,
    pub session_has_epic_workers: bool,
    pub focused_on_epic: bool,
    pub supervisor_id: Option<String>,
    pub owner_live_in_this_session: bool,
    pub epic_present: bool,
}

/// Collect ownership inputs for an epic from a director snapshot.
pub fn epic_completion_context(
    data: &DirectorData,
    epic_id: &str,
    supervisor_name: &str,
    focused_epic_id: Option<&str>,
) -> EpicCompletionContext {
    let epic = data.epic_tasks.iter().find(|e| e.id == epic_id);
    let owner = epic
        .and_then(|e| e.epic_verification_owner.clone())
        .or_else(|| epic.and_then(|e| e.assignee.clone()));

    let session_agent_keys: HashSet<&str> = data
        .agents
        .iter()
        .flat_map(|a| [a.id.as_str(), a.name.as_str()])
        .chain(
            data.agent_id_to_name
                .iter()
                .flat_map(|(id, name)| [id.as_str(), name.as_str()]),
        )
        .chain(std::iter::once(supervisor_name))
        .collect();

    let session_has_epic_workers = data
        .ready_tasks
        .iter()
        .chain(data.in_progress_tasks.iter())
        .filter(|t| t.epic.as_deref() == Some(epic_id))
        .filter_map(|t| t.assignee.as_deref())
        .any(|assignee| session_agent_keys.contains(assignee));

    let owner_live_in_this_session = owner
        .as_deref()
        .map(|o| session_agent_keys.contains(o))
        .unwrap_or(false);

    let supervisor_id = data
        .agents
        .iter()
        .find(|a| a.name == supervisor_name)
        .map(|a| a.id.clone())
        .or_else(|| {
            data.agent_id_to_name
                .iter()
                .find_map(|(id, name)| (name == supervisor_name).then(|| id.clone()))
        });

    EpicCompletionContext {
        owner,
        session_has_epic_workers,
        focused_on_epic: focused_epic_id == Some(epic_id),
        supervisor_id,
        owner_live_in_this_session,
        epic_present: epic.is_some(),
    }
}

/// Revalidate an event with the session's focused epic so session-affinity
/// routing for epic completion can use it (cas-9fff).
pub fn revalidate_event_for_delivery_with_focus(
    event: &DirectorEvent,
    unfiltered_data: &DirectorData,
    supervisor_name: &str,
    focused_epic_id: Option<&str>,
) -> Option<DirectorEvent> {
    revalidate_event_for_delivery_with_context(
        event,
        unfiltered_data,
        supervisor_name,
        focused_epic_id,
        None,
    )
}

/// Delivery-time revalidation with the detector's idle-transition timestamp.
/// A supervisor message that arrived after event detection must win the race,
/// even if transport already marked the queue row processed.
pub fn revalidate_event_for_delivery_with_context(
    event: &DirectorEvent,
    unfiltered_data: &DirectorData,
    supervisor_name: &str,
    focused_epic_id: Option<&str>,
    idle_since: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<DirectorEvent> {
    match event {
        DirectorEvent::EpicAllSubtasksClosed { epic_id, .. } => {
            if !epic_completion_is_current(unfiltered_data, epic_id) {
                tracing::info!(
                    target: "cas::coordination",
                    epic_id = %epic_id,
                    "suppressing stale EpicAllSubtasksClosed after authoritative state changed"
                );
                return None;
            }
            let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
            let ctx =
                epic_completion_context(unfiltered_data, epic_id, supervisor_name, focused_epic_id);
            match route_epic_completion(
                supervisor_name,
                ctx.supervisor_id.as_deref(),
                factory_session.as_deref(),
                ctx.owner.as_deref(),
                ctx.focused_on_epic,
                ctx.session_has_epic_workers,
                ctx.owner_live_in_this_session,
                // Never auto-fallback at revalidation — wrong-session must
                // suppress; owner session (or explicit ops) owns recovery.
                false,
                ctx.epic_present,
            ) {
                EpicCompletionRoute::Deliver { .. } => Some(event.clone()),
                EpicCompletionRoute::Suppress { reason } => {
                    tracing::info!(
                        target: "cas::coordination",
                        epic_id = %epic_id,
                        supervisor = %supervisor_name,
                        reason,
                        "suppressing EpicAllSubtasksClosed for non-owning supervisor"
                    );
                    None
                }
            }
        }
        DirectorEvent::AgentRegistered {
            agent_id,
            agent_name,
        } => {
            let supervisor_already_contacted = unfiltered_data
                .agents
                .iter()
                .find(|agent| agent.name == *agent_name)
                .and_then(|agent| {
                    agent
                        .latest_supervisor_message_at
                        .map(|sent_at| sent_at >= agent.registered_at)
                })
                .unwrap_or(false);
            if supervisor_already_contacted {
                tracing::info!(
                    target: "cas::coordination",
                    worker = %agent_name,
                    "suppressing stale worker-ready nudge after supervisor contact"
                );
                return None;
            }
            if agent_name == supervisor_name
                || live_worker_session_id(unfiltered_data, agent_name).is_none()
                || worker_has_open_or_in_progress_assignment(unfiltered_data, agent_name)
            {
                None
            } else {
                Some(DirectorEvent::AgentRegistered {
                    agent_id: agent_id.clone(),
                    agent_name: agent_name.clone(),
                })
            }
        }
        DirectorEvent::WorkerIdle {
            worker,
            active_task: enqueued_active_task,
        } => {
            if worker == supervisor_name
                || live_worker_session_id(unfiltered_data, worker).is_none()
            {
                return None;
            }

            let active_task = unfiltered_data
                .agents
                .iter()
                .find(|agent| agent.name == *worker)
                .and_then(|agent| agent.active_lease.clone());

            let supervisor_already_contacted = idle_since
                .and_then(|idle_since| {
                    unfiltered_data
                        .agents
                        .iter()
                        .find(|agent| agent.name == *worker)
                        .and_then(|agent| agent.latest_supervisor_message_at)
                        .map(|sent_at| sent_at >= idle_since)
                })
                .unwrap_or(false);

            if enqueued_active_task.is_none()
                && worker_has_open_or_in_progress_assignment(unfiltered_data, worker)
            {
                return None;
            }
            if supervisor_already_contacted {
                tracing::info!(
                    target: "cas::coordination",
                    worker = %worker,
                    "suppressing stale WorkerIdle nudge after supervisor contact"
                );
                return None;
            }

            Some(DirectorEvent::WorkerIdle {
                worker: worker.clone(),
                active_task,
            })
        }
        DirectorEvent::WorkerStalled {
            worker,
            task_id,
            elapsed_secs,
            escalate,
        } => {
            if worker == supervisor_name
                || live_worker_session_id(unfiltered_data, worker).is_none()
            {
                return None;
            }

            let still_stalled_task = unfiltered_data
                .in_progress_tasks
                .iter()
                .chain(
                    unfiltered_data
                        .ready_tasks
                        .iter()
                        .filter(|task| task.status == TaskStatus::Open),
                )
                // cas-ef0a3: `in_progress_tasks` is a visibility bucket that
                // also contains AwaitingMerge.
                // Neither state is worker-actionable, so a stale stall event
                // must not survive a detect→park race and re-nudge the worker.
                .filter(|task| matches!(task.status, TaskStatus::Open | TaskStatus::InProgress))
                .any(|task| {
                    task.id == *task_id && task_assigned_to_worker(unfiltered_data, task, worker)
                });

            still_stalled_task.then(|| DirectorEvent::WorkerStalled {
                worker: worker.clone(),
                task_id: task_id.clone(),
                elapsed_secs: *elapsed_secs,
                escalate: *escalate,
            })
        }
        DirectorEvent::TaskBlocked {
            task_id,
            task_title,
            worker,
        } => unfiltered_data
            .ready_tasks
            .iter()
            .find(|task| task.id == *task_id)
            .filter(|task| task.status == TaskStatus::Blocked)
            .filter(|task| task_assigned_to_worker(unfiltered_data, task, worker))
            .map(|task| DirectorEvent::TaskBlocked {
                task_id: task.id.clone(),
                task_title: if task.title.is_empty() {
                    task_title.clone()
                } else {
                    task.title.clone()
                },
                worker: worker.clone(),
            }),
        // cas-2ca9: `TaskAssigned` used to fall through to the `_` catch-all
        // below with NO revalidation — unlike WorkerIdle/WorkerStalled/
        // TaskBlocked/AgentRegistered, which all re-check current task state
        // against this delivery-time `unfiltered_data` snapshot before
        // generating a prompt. `detect_changes_at` (events.rs) snapshots the
        // task as dispatchable+newly-assigned at *detection* time, but
        // `revalidate_and_prompt_for_delivery` (app/mod.rs) loads a SEPARATE,
        // later snapshot specifically to catch state that changed in the gap
        // between detection and delivery (see its doc comment). Without this
        // arm, a task that closed (or was reassigned to someone else) in that
        // gap still got the "You have been assigned a new task" prompt
        // delivered — the dedup guard in `detect_changes_at` only prevents
        // the SAME (task, assignee) pair from firing more than once; it does
        // nothing to stop a single already-emitted, now-stale event from
        // being delivered. This is the root cause of cas-2ca9 (director
        // re-dispatching already-Closed tasks): the terminal-status guard
        // added in cas-177f covers event *generation* but this delivery-time
        // gate was never extended to cover `TaskAssigned` when it was added
        // later (cas-627f).
        DirectorEvent::TaskAssigned {
            task_id,
            task_title,
            worker,
        } => unfiltered_data
            .in_progress_tasks
            .iter()
            .chain(
                unfiltered_data
                    .ready_tasks
                    .iter()
                    .filter(|task| task.status == TaskStatus::Open),
            )
            // cas-ef0a3: this bucket also carries supervisor-owned
            // AwaitingMerge tasks. Re-check the actual
            // status so a stale assignment event cannot redispatch completed
            // worker work after the merge gate parks it.
            .filter(|task| matches!(task.status, TaskStatus::Open | TaskStatus::InProgress))
            .find(|task| task.id == *task_id)
            .filter(|task| task_assigned_to_worker(unfiltered_data, task, worker))
            .map(|task| DirectorEvent::TaskAssigned {
                task_id: task.id.clone(),
                task_title: if task.title.is_empty() {
                    task_title.clone()
                } else {
                    task.title.clone()
                },
                worker: worker.clone(),
            }),
        _ => Some(event.clone()),
    }
}

/// A prompt to be injected into an agent's terminal
#[derive(Debug, Clone)]
pub struct Prompt {
    /// Target agent name (worker name or "supervisor")
    pub target: String,
    /// Prompt text to inject
    pub text: String,
    /// cas-ed6c/cas-38e3: `Some(worker)` when this prompt is a taskless-worker
    /// alert about `worker` (`WorkerIdle` or `AgentRegistered`) — threaded down
    /// to `deliver_to_worker` so the
    /// queued inbox row can be tagged for later retraction
    /// (`TeamsManager::prune_stale_idle_alerts`) if the worker gains a real
    /// assignment before the recipient ever reads it. `None` for every
    /// other prompt kind, INCLUDING MERGE REQUIRED alerts — those use
    /// `retract_task` instead (see its doc).
    pub retract_worker: Option<String>,
    /// cas-e48f: `Some(task_id)` when this prompt is the actionable MERGE
    /// REQUIRED / `AwaitingMerge` idle alert (`merge_required_idle_prompt_text`)
    /// — threaded down to `deliver_to_worker` so the queued inbox row can be
    /// tagged for later retraction (`TeamsManager::prune_stale_merge_alerts`)
    /// if the merge lands, or the task leaves `AwaitingMerge`, before the
    /// recipient ever reads it. Deliberately NOT `retract_worker`: a merge
    /// alert's staleness is about this task's live unmerged-commit count
    /// against the CURRENT epic tip, not about the named worker's assignment
    /// state — a worker can be reassigned elsewhere while its own merge is
    /// still genuinely outstanding, and a merge can land while the worker
    /// stays idle with no new assignment at all. `None` for every other
    /// prompt kind, including the plain informational (non-merge)
    /// close-rejected idle wording, which still uses `retract_worker`.
    pub retract_task: Option<String>,
    /// Epic id carried by an `EpicAllSubtasksClosed` occurrence. The same id
    /// drives the last-mile live-state check and best-effort retraction of an
    /// unread Teams inbox row if the epic closes or a subtask reopens.
    pub retract_epic: Option<String>,
    /// Taskless worker prompts must be checked once more against a fresh
    /// store snapshot immediately before their PTY/inbox injection. Epic
    /// prompts use `retract_epic` for the same last-mile race; this field is
    /// specific to worker-assignment currency.
    pub drop_if_worker_assigned: Option<String>,
    /// cas-ae6d (GH #100): this prompt is loss-intolerant — dropping it leaves
    /// a worker parked with an assignment it was never told about, and the
    /// event detector's `task_assigned_announced` guard guarantees no second
    /// chance. The daemon routes such a prompt through the durable
    /// `prompt_queue` (readiness-gated + retrying) whenever the recipient's
    /// channel is a PTY that cannot take it right now, and re-queues it if a
    /// direct attempt fails or is deferred. `false` for informational prompts,
    /// which keep their historical one-shot delivery.
    pub durable_retry: bool,
}

/// Last-mile predicate for a prompt that has already survived event-level
/// revalidation. The caller supplies a snapshot loaded immediately before
/// transport injection, not the earlier batch snapshot. Epic occurrence
/// identity, worker identity, and merge state are all checked here, through
/// their single shared predicates.
///
/// cas-6eab (GH #74): `retract_task` was previously the one state-bearing tag
/// with NO last-mile check. A MERGE REQUIRED alert was re-validated against
/// live git when its prompt was GENERATED (`check_merge_alert_freshness`, at
/// the top of the daemon tick) and could be retracted afterwards only if it
/// was still sitting unread in a Teams inbox (`prune_stale_merge_alerts`) —
/// so nothing covered the window in between, which is not idle time: the same
/// tick runs `handle_epic_change`, and that performs merges. An alert whose
/// premise was killed by the daemon's own merge, mid-tick, was still injected
/// quoting the pre-merge tip. A PTY-delivered factory (no Teams inbox) had no
/// retraction path at all, so for it this is the only check that ever runs.
///
/// `repo_root` is the main checkout all `factory/*` and `epic/*` branches live
/// in. Only `Stale` — positive evidence that the merge landed or the task left
/// `AwaitingMerge` — suppresses; `NotApplicable` (nothing to verify against)
/// delivers, matching the fail-open stance of every other predicate here.
pub(crate) fn prompt_is_still_deliverable(
    prompt: &Prompt,
    data: &DirectorData,
    repo_root: &Path,
) -> bool {
    let epic_is_current = prompt
        .retract_epic
        .as_deref()
        .is_none_or(|epic_id| epic_completion_is_current(data, epic_id));
    let merge_is_still_required = prompt.retract_task.as_deref().is_none_or(|task_id| {
        !matches!(
            check_merge_alert_freshness_for_task(task_id, data, repo_root),
            MergeAlertFreshness::Stale
        )
    });
    epic_is_current
        && merge_is_still_required
        && prompt
            .drop_if_worker_assigned
            .as_deref()
            .is_none_or(|worker| !worker_now_has_real_assignment(data, worker))
}

/// Wrap a message with response instructions
///
/// Appends instructions telling the agent how to respond using the MCP message tool.
/// The command prefix differs by harness:
/// - Claude: `mcp__cas__`
/// - Codex: `mcp__cs__`
///
/// # Arguments
/// * `message` - The original message text
/// * `respond_to` - The target agent name for responses (e.g., "supervisor", "swift-fox")
/// * `receiver_cli` - CLI harness for the agent receiving this message
///
/// # Returns
/// The message with response instructions appended at the end
pub fn with_response_instructions(
    message: &str,
    respond_to: &str,
    receiver_cli: SupervisorCli,
) -> String {
    let prefix = receiver_cli.backend().capabilities().tool_prefix;
    format!(
        "{message}\n\n---\nTo respond to this message, use: `{prefix}coordination action=message target={respond_to} message=\"...\"`"
    )
}

/// True when a WorkerIdle active-task payload is the merge-gate park path
/// (cas-c145): the task's CURRENT status is `AwaitingMerge`.
///
/// cas-6883: this used to also match when `close_rejected_reason` merely
/// *named* MERGE REQUIRED, even if `task_status` had since moved off
/// `AwaitingMerge`. Traced against `run_factory_branch_merge_gate` /
/// `park_task_awaiting_merge` (close_ops.rs): a MERGE REQUIRED rejection
/// ALWAYS parks the task to `AwaitingMerge` in the same call, so
/// `task_status != AwaitingMerge` while `close_rejected_reason` still says
/// MERGE REQUIRED can only mean the reason string is a stale echo of an
/// older activity event (`close_rejections` in `director.rs` scans the last
/// 50 activity rows and never expires an entry) — the task was reset or
/// reopened since. Gating strictly on live `task_status` stops that stale
/// echo from re-triggering the actionable merge-queue framing; the alert
/// falls back to the honest, generic idle wording instead (which still
/// surfaces `close_rejected_reason` for context — see
/// `BUG-stale-merge-required-alerts-refire-after-merge.md`, AC1).
fn is_merge_required_idle(task: &ActiveLeaseSummary) -> bool {
    task.task_status == TaskStatus::AwaitingMerge
}

/// Live evidence backing a MERGE REQUIRED idle alert, computed at send time
/// (cas-6883) so the alert is self-verifying instead of asserting stale
/// state. `epic_sha` is the epic branch's tip at the moment the unmerged
/// count was computed — if a supervisor's own `epic_status` shows a
/// different SHA, the branch has moved and this alert may already be
/// out of date again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeAlertEvidence {
    pub task_id: String,
    pub factory_branch: String,
    pub unmerged_count: u32,
    pub epic_sha: String,
    /// Exact local or remote-tracking epic ref used for the count.
    pub checked_epic_ref: String,
    /// Present when local and remote-tracking refs both resolved but did
    /// not describe the same tip/count.
    pub ref_disagreement: Option<String>,
    /// The local epic already contains the factory work, but the authoritative
    /// origin epic does not. The supervisor should push the epic, not repeat
    /// the local merge.
    pub push_required: bool,
}

/// Outcome of the cas-6883 send-time freshness re-check for a MERGE
/// REQUIRED / AwaitingMerge idle alert.
#[derive(Debug)]
pub enum MergeAlertFreshness {
    /// Not a merge-required idle signal (wrong event shape, task no longer
    /// AwaitingMerge, or the epic branch can't be resolved from this
    /// snapshot). Caller falls back to the pre-cas-6883 path unaffected.
    NotApplicable,
    /// Confirmed stale: the factory branch already carries zero unmerged
    /// commits vs the epic branch. Drop the alert entirely (AC1).
    Stale,
    /// Confirmed live: evidence to embed in the alert text (AC2).
    Fresh(MergeAlertEvidence),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MergeRefObservation {
    epic_ref: String,
    commit_id: Option<String>,
    count: KnownUnmergedCount,
}

fn observe_merge_ref(
    repo_root: &Path,
    factory_commit: Option<&str>,
    epic_ref: &str,
) -> MergeRefObservation {
    let commit_id = resolve_ref_commit_sha(repo_root, epic_ref);
    let count = match (factory_commit, commit_id.as_deref()) {
        (Some(factory_commit), Some(epic_commit)) => {
            known_unmerged_factory_commits(repo_root, factory_commit, epic_commit)
        }
        _ => KnownUnmergedCount::Unknown,
    };
    MergeRefObservation {
        epic_ref: epic_ref.to_string(),
        commit_id,
        count,
    }
}

fn known_count(count: KnownUnmergedCount) -> Option<u32> {
    match count {
        KnownUnmergedCount::KnownZero => Some(0),
        KnownUnmergedCount::KnownPositive(count) => Some(count),
        KnownUnmergedCount::Unknown => None,
    }
}

fn short_commit_id(commit_id: &str) -> String {
    commit_id.chars().take(7).collect()
}

/// Classify one immutable local/origin observation pair. A known origin result
/// is authoritative; only when origin is Unknown do we fall back to local.
fn classify_merge_alert_observations(
    task_id: &str,
    factory_branch: &str,
    local: MergeRefObservation,
    origin: MergeRefObservation,
) -> MergeAlertFreshness {
    let chosen = match origin.count {
        KnownUnmergedCount::KnownZero => return MergeAlertFreshness::Stale,
        KnownUnmergedCount::KnownPositive(_) => &origin,
        KnownUnmergedCount::Unknown => match local.count {
            KnownUnmergedCount::KnownZero => return MergeAlertFreshness::Stale,
            KnownUnmergedCount::KnownPositive(_) => &local,
            KnownUnmergedCount::Unknown => return MergeAlertFreshness::NotApplicable,
        },
    };

    let KnownUnmergedCount::KnownPositive(unmerged_count) = chosen.count else {
        unreachable!("zero and unknown observations return before evidence construction");
    };
    let Some(epic_commit) = chosen.commit_id.as_deref() else {
        return MergeAlertFreshness::NotApplicable;
    };
    let push_required = local.count == KnownUnmergedCount::KnownZero
        && matches!(origin.count, KnownUnmergedCount::KnownPositive(_));

    let ref_disagreement = match (known_count(local.count), known_count(origin.count)) {
        (Some(local_count), Some(origin_count))
            if local.commit_id != origin.commit_id || local_count != origin_count =>
        {
            let local_sha = local
                .commit_id
                .as_deref()
                .map(short_commit_id)
                .unwrap_or_else(|| "<unknown>".to_string());
            let origin_sha = origin
                .commit_id
                .as_deref()
                .map(short_commit_id)
                .unwrap_or_else(|| "<unknown>".to_string());
            Some(format!(
                "{} at {} reports {} unmerged commit(s); {} at {} reports {} unmerged commit(s)",
                local.epic_ref, local_sha, local_count, origin.epic_ref, origin_sha, origin_count,
            ))
        }
        _ => None,
    };

    MergeAlertFreshness::Fresh(MergeAlertEvidence {
        task_id: task_id.to_string(),
        factory_branch: factory_branch.to_string(),
        unmerged_count,
        epic_sha: short_commit_id(epic_commit),
        checked_epic_ref: chosen.epic_ref.clone(),
        ref_disagreement,
        push_required,
    })
}

/// Fetch and re-read both the local epic ref and `origin/<epic>`, then
/// classify the factory branch against immutable commit IDs. Unknown Git
/// state never masquerades as zero.
fn fresh_merge_alert_git_evidence(
    repo_root: &Path,
    task_id: &str,
    factory_branch: &str,
    epic_branch: &str,
) -> MergeAlertFreshness {
    fetch_parent_branch_best_effort(repo_root, epic_branch);

    // Pin all three movable refs once. Every merge-base/rev-list below uses
    // these immutable IDs, so a concurrent fetch/branch update cannot combine
    // a SHA from one instant with a count from another.
    let factory_commit = resolve_ref_commit_sha(repo_root, factory_branch);
    let local = observe_merge_ref(repo_root, factory_commit.as_deref(), epic_branch);
    let origin_ref = format!("origin/{epic_branch}");
    let origin = observe_merge_ref(repo_root, factory_commit.as_deref(), &origin_ref);
    classify_merge_alert_observations(task_id, factory_branch, local, origin)
}

/// Re-validate a MERGE REQUIRED / AwaitingMerge `WorkerIdle` signal against
/// live git state immediately before it would be sent (cas-6883).
///
/// See `docs/requests/BUG-stale-merge-required-alerts-refire-after-merge.md`:
/// the task-status snapshot backing this alert can be accurate (the task
/// really is still `AwaitingMerge` in the DB) while stale in the sense that
/// matters — the branch was already merged and nobody has re-closed the
/// task yet. The check uses the close gate's success-bearing
/// `known_unmerged_factory_commits` helper against immutable snapshots of
/// both the local epic ref and `origin/<epic>`, after a bounded exact-ref
/// fetch. A known origin result is authoritative: a pushed merge suppresses
/// a stale local alert, while origin-positive state keeps an alert actionable
/// even if the local epic already contains an unpushed merge. Unknown Git
/// failures cannot masquerade as zero.
pub fn check_merge_alert_freshness(
    event: &DirectorEvent,
    data: &DirectorData,
    repo_root: &Path,
) -> MergeAlertFreshness {
    let DirectorEvent::WorkerIdle {
        worker,
        active_task: Some(task),
    } = event
    else {
        return MergeAlertFreshness::NotApplicable;
    };
    if task.task_status != TaskStatus::AwaitingMerge {
        return MergeAlertFreshness::NotApplicable;
    }
    let factory_branch = format!("factory/{worker}");
    let (_, epic_branch, _) = resolve_merge_target_for_task(data, &task.task_id);
    let Some(epic_branch) = epic_branch else {
        // No resolvable epic link in this snapshot — can't verify either
        // way, so don't silently drop a possibly-valid alert over a
        // data-linking gap unrelated to git state (pre-cas-6883 behavior).
        return MergeAlertFreshness::NotApplicable;
    };
    fresh_merge_alert_git_evidence(repo_root, &task.task_id, &factory_branch, &epic_branch)
}

/// Re-validate a MERGE REQUIRED / `AwaitingMerge` alert already SITTING in
/// the supervisor's inbox, keyed by task id (cas-e48f, follow-on to
/// `check_merge_alert_freshness` above).
///
/// `check_merge_alert_freshness` re-checks a signal against live git state
/// at the instant its prompt is GENERATED — but the row can then sit
/// unread in the Teams inbox file for minutes (Claude Code only polls its
/// inbox at its own turn boundaries; `read` is never flipped to `true` by
/// production code). This is the sweep-time counterpart: given only a
/// `task_id` pulled from a queued row's `retract_task` tag, look the task
/// up fresh in the CURRENT snapshot and re-run the same success-bearing
/// check against the CURRENT local and remote-tracking epic refs — never
/// the tip captured when the row was written.
///
/// Returns `Stale` (retract the row) when:
/// - the task is no longer tracked in `data.in_progress_tasks` at all
///   (closed, reset, or otherwise resolved since the alert was written), or
/// - the task is tracked but its status has moved off `AwaitingMerge`
///   (re-closed, reopened, reassigned), or
/// - the task is still `AwaitingMerge` but its factory branch now carries
///   zero unmerged commits against the live epic tip (the merge landed).
///
/// Returns `NotApplicable` (preserve the row; caller cannot verify either
/// way) when the task has no assignee or its epic branch can't be resolved
/// from this snapshot — the same "don't silently drop a possibly-valid
/// alert over a data-linking gap" stance `check_merge_alert_freshness`
/// takes.
///
/// Returns `Fresh` (preserve the row) when the merge is still genuinely
/// outstanding — this also covers the case where the epic tip moved for an
/// UNRELATED reason (another task's merge): the fresh evidence helper
/// re-diffs against whatever local/remote epic tips are visible now, so an
/// unrelated tip move that doesn't touch this task's commits still yields a
/// nonzero count and the alert correctly survives.
pub fn check_merge_alert_freshness_for_task(
    task_id: &str,
    data: &DirectorData,
    repo_root: &Path,
) -> MergeAlertFreshness {
    let Some(task) = data.in_progress_tasks.iter().find(|t| t.id == task_id) else {
        // No longer tracked as in-progress/awaiting-merge in the current
        // snapshot at all — whatever happened (closed, reset elsewhere),
        // the alert's premise no longer holds.
        return MergeAlertFreshness::Stale;
    };
    if task.status != TaskStatus::AwaitingMerge {
        return MergeAlertFreshness::Stale;
    }
    let Some(worker) = task.assignee.clone() else {
        return MergeAlertFreshness::NotApplicable;
    };
    let factory_branch = format!("factory/{worker}");
    let (_, epic_branch, _) = resolve_merge_target_for_task(data, task_id);
    let Some(epic_branch) = epic_branch else {
        return MergeAlertFreshness::NotApplicable;
    };
    fresh_merge_alert_git_evidence(repo_root, task_id, &factory_branch, &epic_branch)
}

/// Resolve the delivery target for a parked task from the current director
/// snapshot. A task WorkTarget, projected into its summary branch, wins over
/// the parent epic; the parent remains only as the legacy fallback.
fn resolve_merge_target_for_task(
    data: &DirectorData,
    task_id: &str,
) -> (Option<String>, Option<String>, bool) {
    // AwaitingMerge tasks live in `in_progress_tasks` (DirectorData
    // waiting/active bucket). Ready/open rows are chained as a fallback.
    let task = data
        .in_progress_tasks
        .iter()
        .chain(data.ready_tasks.iter())
        .find(|t| t.id == task_id);
    let declared_target = task
        .filter(|task| task.task_type != TaskType::Epic)
        .and_then(|task| task.branch.as_deref())
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(str::to_string);
    let epic_id = task.and_then(|task| task.epic.clone());
    if let Some(target_branch) = declared_target {
        return (epic_id, Some(target_branch), true);
    }
    let epic_branch = epic_id.as_ref().and_then(|eid| {
        data.epic_tasks
            .iter()
            .find(|e| e.id == *eid)
            .and_then(|e| e.branch.clone())
    });
    (epic_id, epic_branch, false)
}

/// Actionable merge-queue prompt for MERGE REQUIRED / AwaitingMerge idle
/// signals (cas-c145). Carries task, source factory branch, merge target,
/// and next action. Explicitly push-based (no polling loop).
///
/// `supervisor_prefix` is used for tools the **supervisor** runs
/// (epic_status, list awaiting_merge, show). `worker_prefix` is used only
/// for the worker re-close command the supervisor is told to relay — mixed
/// factories (e.g. Claude supervisor + Codex/Grok worker) must not leak the
/// supervisor's MCP alias into worker-facing tool strings (review P1).
///
/// Wording constraint: must not contain "assign" — the AwaitingMerge idle
/// path is not "idle needing work" (cas-09d0 / cas-728b).
///
/// `evidence` (cas-6883) is the live git evidence this alert was validated
/// against (see `check_merge_alert_freshness`) — `None` only when the
/// caller skipped that check (e.g. no resolvable epic branch, or a caller
/// that intentionally doesn't run it, such as most tests). When present, it
/// is embedded inline (AC2) so the alert is dismissible at a glance without
/// a supervisor having to run `epic_status` themselves.
fn merge_required_idle_prompt_text(
    worker: &str,
    task: &ActiveLeaseSummary,
    data: &DirectorData,
    supervisor_prefix: &str,
    worker_prefix: &str,
    evidence: Option<&MergeAlertEvidence>,
) -> String {
    let factory_branch = format!("factory/{worker}");
    let (epic_id, epic_branch, declared_target) =
        resolve_merge_target_for_task(data, &task.task_id);
    let target = epic_branch
        .as_deref()
        .unwrap_or("the task's resolved merge target");
    let epic_status = if declared_target {
        format!("`{supervisor_prefix}task action=show id={}`", task.task_id)
    } else {
        match epic_id.as_deref() {
            Some(id) => format!("`{supervisor_prefix}coordination action=epic_status id={id}`"),
            None => {
                format!("`{supervisor_prefix}coordination action=epic_status id=<focused-epic>")
            }
        }
    };
    let list_awaiting = format!("`{supervisor_prefix}task action=list status=awaiting_merge`");
    let show = format!("`{supervisor_prefix}task action=show id={}`", task.task_id);
    // Worker re-close uses the *worker's* harness prefix so the supervisor
    // relays a callable alias (cas-c145 review P1).
    let reclose = format!("`{worker_prefix}task action=close id={}`", task.task_id);
    let rejection = task
        .close_rejected_reason
        .as_deref()
        .unwrap_or("MERGE REQUIRED");
    let evidence_line = match evidence {
        Some(e) => {
            let disagreement = e
                .ref_disagreement
                .as_deref()
                .map(|details| {
                    format!(
                        "Git ref disagreement detected: {details}. Using {} after re-reading both refs.\n",
                        e.checked_epic_ref
                    )
                })
                .unwrap_or_default();
            format!(
                "Live evidence: {} unmerged commit(s) on {} vs {} (checked {} at {}).\n{}",
                e.unmerged_count,
                e.factory_branch,
                target,
                e.checked_epic_ref,
                e.epic_sha,
                disagreement
            )
        }
        None => String::new(),
    };
    let merge_step = if evidence.is_some_and(|e| e.push_required) {
        format!(
            "2. Push required: the local {target} already contains {factory_branch}, \
             but origin does not. Push {target} to origin; do not repeat the local merge."
        )
    } else {
        format!(
            "2. Merge {factory_branch} into {target} (FF preferred; else \
             `git merge --no-ff {factory_branch}` on {target})"
        )
    };

    format!(
        "⚠️ MERGE REQUIRED — supervisor action needed (not a task completion).\n\
         Worker {worker} is idle while task {} ({}) is {} (close rejected: {rejection}).\n\
         {evidence_line}\
         Source branch: {factory_branch}\n\
         Merge target: {target}\n\
         Next action — drain the merge queue before free-form user chat:\n\
         1. Confirm: {epic_status} and/or {list_awaiting}\n\
         {merge_step}\n\
         3. Push {target} if remote tracking applies\n\
         4. Tell {worker} to re-close with {reclose} (or use the supervisor escape-hatch close after merge if the worker is unresponsive)\n\
         5. Then clear context / hand the worker their next task if more work is ready\n\
         Live task state: {show}\n\
         This is a push-based WorkerIdle close-rejected signal — do not poll or sleep.",
        task.task_id, task.task_title, task.task_status
    )
}

/// Generate a prompt for a detected event
///
/// Returns `Some(Prompt)` if a prompt should be sent for this event,
/// or `None` if no prompt is needed or if the event type is disabled in config.
///
/// `data` may be epic-scoped (filtered to the currently-tracked epic, e.g. for
/// `WorkerIdle`'s ready-task counting — cas-405f). `unfiltered_data` must
/// always be the true, never-epic-filtered task snapshot; it backs
/// `TaskCompleted`'s render-time safety net (cas-6aaf / cas-dbbe), which needs
/// to see tasks outside the tracked epic to avoid confirming a false "has
/// closed" for a task that's merely out of the current epic's display scope.
/// Callers with only one snapshot available (e.g. most tests) may pass the
/// same value for both.
///
/// `merge_alert_evidence` (cas-6883) is the live git evidence a caller
/// already computed via `check_merge_alert_freshness` for THIS event, or
/// `None` when the caller skipped that check (most tests) or the event
/// isn't a merge-required idle signal. This function never runs git itself
/// (it stays a pure function over in-memory snapshots) — the freshness
/// re-check, including the decision to drop a stale alert entirely, is the
/// caller's responsibility (`revalidate_and_prompt_for_delivery`).
/// Thin shim over [`generate_prompt_at`] that stamps taskless-worker notices
/// with `Utc::now()`. Production callers use `generate_prompt_at` and pass the
/// instant the delivery-time snapshot was actually read, so the stamp names the
/// snapshot's age rather than the render's (cas-ae6d / GH #100). Mirrors the
/// `detect_changes` / `detect_changes_at` clock-injection pattern in events.rs.
///
/// Test-only: every production caller has a real snapshot instant to pass, so
/// keeping this out of non-test builds means no prompt can quietly stamp a
/// notice with render time instead of read time.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub fn generate_prompt(
    event: &DirectorEvent,
    data: &DirectorData,
    unfiltered_data: &DirectorData,
    supervisor_name: &str,
    config: &AutoPromptConfig,
    supervisor_cli: SupervisorCli,
    worker_cli: SupervisorCli,
    gated_task_ids: &HashSet<String>,
    merge_alert_evidence: Option<&MergeAlertEvidence>,
) -> Option<Prompt> {
    generate_prompt_at(
        event,
        data,
        unfiltered_data,
        supervisor_name,
        config,
        supervisor_cli,
        worker_cli,
        gated_task_ids,
        merge_alert_evidence,
        chrono::Utc::now(),
    )
}

/// [`generate_prompt`] with the snapshot instant injected.
///
/// `snapshot_at` is when `unfiltered_data` was read from the store. Idle /
/// registration notices that report "nothing dispatchable" carry it verbatim so
/// the recipient can tell a genuinely-empty queue from a notice built against a
/// snapshot that has since been overtaken by an assignment (cas-ae6d, GH #100).
#[allow(clippy::too_many_arguments)]
pub fn generate_prompt_at(
    event: &DirectorEvent,
    data: &DirectorData,
    unfiltered_data: &DirectorData,
    supervisor_name: &str,
    config: &AutoPromptConfig,
    supervisor_cli: SupervisorCli,
    worker_cli: SupervisorCli,
    gated_task_ids: &HashSet<String>,
    merge_alert_evidence: Option<&MergeAlertEvidence>,
    snapshot_at: chrono::DateTime<chrono::Utc>,
) -> Option<Prompt> {
    // Check global enable flag first
    if !config.enabled {
        return None;
    }
    let supervisor_prefix = supervisor_cli.backend().capabilities().tool_prefix;
    let worker_prefix = worker_cli.backend().capabilities().tool_prefix;

    match event {
        DirectorEvent::SupervisorStalled { .. } => None,
        DirectorEvent::TaskAssigned {
            task_id,
            task_title,
            worker,
        } => {
            // A supervisor may own a gate task, but is never a worker to be
            // handed the worker assignment template. Keep this delivery-time
            // defence even though the detector also excludes supervisors: a
            // queued event can outlive detector state or be generated by a
            // direct caller.
            if !config.on_task_assigned || worker == supervisor_name {
                return None;
            }

            let text = format!(
                "You have been assigned a new task:\n\
                 Task ID: {task_id}\n\
                 Title: {task_title}\n\n\
                 View full details: {worker_prefix}task action=show id={task_id}\n\
                 Start working: {worker_prefix}task action=start id={task_id}\n\
                 Then send an ACK to supervisor with your execution plan.\n\
                 While working, post progress notes with {worker_prefix}task action=notes.\n\
                 If blocked, set status=blocked and explain the blocker."
            );

            Some(Prompt {
                target: worker.clone(),
                text: with_response_instructions(&text, supervisor_name, worker_cli),
                retract_worker: None,
                retract_task: None,
                retract_epic: None,
                drop_if_worker_assigned: None,
                // cas-ae6d (GH #100): the assignment wake-up is the one prompt
                // whose loss strands a worker — the detector announces a
                // (task, assignee) pair exactly once, so a swallowed PTY write
                // is permanent. Make it durable so a Codex/Grok worker whose
                // pane isn't ready still gets woken by the retrying queue lane.
                durable_retry: true,
            })
        }

        DirectorEvent::TaskCompleted {
            task_id,
            task_title,
            worker,
        } => {
            // See TaskAssigned above. A supervisor closing their own gate is
            // not a worker completion and must not receive advice to assign
            // another task to themselves or close a still-active epic.
            if !config.on_task_completed || worker == supervisor_name {
                return None;
            }

            // cas-6aaf: check current task state before emitting guidance.
            //
            // `TaskCompleted` fires when a task disappears from `in_progress_tasks`,
            // which happens when it transitions to `Closed`. However, lease churn
            // can also cause a task to temporarily regress to `Open` (lease expired
            // → status reset to Open). We check the current snapshot to distinguish
            // the two cases and avoid emitting "please close" guidance for a task
            // the worker has already closed.
            //
            // cas-dbbe: deliberately re-check against `unfiltered_data`, not
            // `data`. `data` may be epic-scoped to whatever epic the director
            // currently tracks; a task belonging to a SECOND epic being worked
            // concurrently in the same session would be absent from `data`'s
            // ready/in_progress lists regardless of its true status, which
            // would make this safety net rubber-stamp a false "has closed"
            // instead of catching it.
            //
            // State resolution:
            //   - task absent from ready+in_progress → closed (expected path)
            //   - task in ready_tasks as Open       → lease expired, still needs close
            //   - task in in_progress_tasks         → still being worked (edge case)
            let in_ready = unfiltered_data
                .ready_tasks
                .iter()
                .any(|t| t.id == *task_id && t.status == cas_types::TaskStatus::Open);
            let in_progress = unfiltered_data
                .in_progress_tasks
                .iter()
                .any(|t| t.id == *task_id);

            let text = if in_ready {
                // Task regressed to Open (lease expired) — worker needs to close it.
                format!(
                    "Worker {worker} was working on task {task_id} ({task_title}) but \
                     it is now Open (lease may have expired).\n\n\
                     Next steps:\n\
                     - Ask the worker to close: {worker_prefix}task action=close id={task_id}\n\
                     - If they have uncommitted work, they should commit first, then close\n\
                     - If close triggers verification, the worker handles it (not you)\n\n\
                     Remember: workers close their own tasks, supervisors close epics."
                )
            } else if in_progress {
                // Still in progress — stale event, nothing to do.
                return None;
            } else {
                // Task is already closed (the normal path after a successful close).
                // Do NOT instruct the supervisor to ask the worker to close it again.
                format!(
                    "Worker {worker} has closed task {task_id} ({task_title}).\n\n\
                     Next steps:\n\
                     - Assign another task to this worker, OR\n\
                     - If all subtasks are done, verify and close the epic\n\n\
                     Remember: workers close their own tasks, supervisors close epics."
                )
            };

            Some(Prompt {
                target: supervisor_name.to_string(),
                text: with_response_instructions(&text, worker, supervisor_cli),
                retract_worker: None,
                retract_task: None,
                retract_epic: None,
                drop_if_worker_assigned: None,
                durable_retry: false,
            })
        }

        DirectorEvent::TaskBlocked {
            task_id,
            task_title,
            worker,
        } => {
            if !config.on_task_blocked {
                return None;
            }

            let text = format!(
                "Worker {worker} is blocked on task {task_id} ({task_title}).\n\
                 They may need assistance or the blocker needs to be resolved."
            );

            Some(Prompt {
                target: supervisor_name.to_string(),
                text: with_response_instructions(&text, worker, supervisor_cli),
                retract_worker: None,
                retract_task: None,
                retract_epic: None,
                drop_if_worker_assigned: None,
                durable_retry: false,
            })
        }

        DirectorEvent::WorkerIdle {
            worker,
            active_task,
        } => {
            if !config.on_worker_idle {
                return None;
            }

            // Guard (cas-c790): supervisor / team-lead is never an idle worker.
            // The event detector filters this at the source (is_worker_agent_name),
            // but defense-in-depth here catches any path that bypasses the upstream
            // gate (e.g. supervisor name in worker_names on resume/reconnect — the
            // recurrence described in cas-c790 / cas-b67d).
            if worker == supervisor_name {
                return None;
            }

            // Defense-in-depth for stale queued events: only emit an idle nudge
            // when the current authoritative snapshot still contains this worker.
            // If the worker was shut down, crashed, or belonged to another session,
            // a stale WorkerIdle event must not tell the supervisor to assign into
            // the void.
            // Liveness gate only — the assignee interpolation below uses the
            // display name (`worker`), not this session ID. `task mine` matches
            // on display name, and `task update assignee=<session-id>` gets
            // silently normalized back to the display name (update.rs:176-186,
            // cas-dbbb). Advertising the session id here just adds a spurious
            // normalization warning on every assignment.
            let Some(_worker_session_id) = live_worker_session_id(data, worker) else {
                return None;
            };

            // Guard (cas-889d / cas-dbbb): suppress idle nudge if the worker already
            // has an active in_progress task OR an assigned-but-not-yet-started Open
            // task in the current snapshot. Checking in_progress_tasks alone misses
            // the window between `task.update assignee=<name>` (status stays Open)
            // and the worker calling `task start` (status becomes InProgress) — the
            // director would incorrectly re-fire WorkerIdle during that gap.
            //
            // Blocked tasks are EXCLUDED: `ready_tasks` contains both Open and Blocked
            // tasks, but a worker with only a Blocked task is genuinely stalled and may
            // still need an idle nudge. Including Blocked tasks here would suppress
            // WorkerIdle indefinitely for stalled workers.
            //
            // Checking by both display-name assignee (canonical DB path) and session-ID
            // assignee (legacy assignment path via agent_id_to_name) makes this robust
            // to either convention.
            //
            // cas-ae6d (GH #100): checked against `unfiltered_data` — the
            // delivery-time read — so an assignment outside the tracked epic
            // still counts as busy. Defense-in-depth only: the primary guard is
            // `revalidate_event_for_delivery_with_context`, which applies the
            // same predicate to the same snapshot before this function is
            // called. This arm is what remains reachable when the event carried
            // an `active_task` whose lease has since expired.
            let worker_is_busy = unfiltered_data
                .in_progress_tasks
                .iter()
                .chain(
                    unfiltered_data
                        .ready_tasks
                        .iter()
                        .filter(|t| t.status == TaskStatus::Open),
                )
                .any(|t| {
                    t.assignee.as_deref() == Some(worker.as_str())
                        || unfiltered_data
                            .agent_id_to_name
                            .iter()
                            .any(|(id, name)| name == worker && t.assignee.as_deref() == Some(id))
                });
            if worker_is_busy && active_task.is_none() {
                return None;
            }

            if let Some(task) = active_task {
                // cas-728b/cas-627f: Blocked and AwaitingMerge are
                // supervisor-parked states. This arm is NOT the
                // worker-assistance "please assign this idle worker
                // something" ping (that's the `ready_count` branch below,
                // reached only when `active_task` is `None`). An earlier
                // version of this fix unconditionally suppressed this arm
                // for Blocked/AwaitingMerge — that re-hid the flagship
                // close-rejected notification cas-627f spent real effort
                // making reachable again (park releases the lease, so
                // `active_lease` — and this `Some(task)` — was `None` for
                // every parked task until that fix). Tick-by-tick repetition
                // is already handled upstream: the event detector's
                // `idle_already_emitted` gate (events.rs) fires `WorkerIdle`
                // once per sustained idle streak, not every 2s tick, and
                // `IDLE_RATE_LIMIT` floors any streak-reset repeat to once
                // per 5 minutes. No additional suppression needed here.
                //
                // cas-c145: when the park is specifically MERGE REQUIRED /
                // AwaitingMerge, upgrade from vague "resolve the rejection"
                // to an actionable merge-queue prompt (task, factory branch,
                // epic target, next steps). Other close-rejection reasons
                // keep the informational wording.
                let text = if is_merge_required_idle(task) {
                    merge_required_idle_prompt_text(
                        worker,
                        task,
                        data,
                        supervisor_prefix,
                        worker_prefix,
                        merge_alert_evidence,
                    )
                } else {
                    let rejection = task
                        .close_rejected_reason
                        .as_deref()
                        .map(|reason| format!(", close rejected ({reason})"))
                        .unwrap_or_default();
                    format!(
                        "Worker {worker} is idle while task {} ({}) is still {}{}.\n\
                         This is a worker-lifecycle idle signal, not a task completion.\n\
                         Check live state: `{supervisor_prefix}task action=show id={}`\n\
                         If close was rejected, resolve the rejection before acting on the task as closed.",
                        task.task_id, task.task_title, task.task_status, rejection, task.task_id
                    )
                };

                // cas-e48f: MERGE REQUIRED alerts are tagged with
                // `retract_task` (this task's own live merge state), NOT
                // `retract_worker` (the named worker's assignment state) —
                // the two can diverge in both directions (see `Prompt::
                // retract_task` doc). The plain informational close-rejected
                // wording (the `else` branch above, e.g. Blocked parks) is
                // still genuinely about worker idleness, so it keeps
                // `retract_worker`.
                let (retract_worker, retract_task) = if is_merge_required_idle(task) {
                    (None, Some(task.task_id.clone()))
                } else {
                    (Some(worker.clone()), None)
                };

                return Some(Prompt {
                    target: supervisor_name.to_string(),
                    text: with_response_instructions(&text, worker, supervisor_cli),
                    retract_worker,
                    retract_task,
                    retract_epic: None,
                    drop_if_worker_assigned: None,
                    durable_retry: false,
                });
            }

            // Count only truly-dispatchable tasks (Open + unassigned). See
            // `dispatchable_ready_count` for why `ready_tasks.len()` is wrong.
            //
            // cas-ae6d (GH #100): deliberately still `data`, NOT
            // `unfiltered_data`. The epic/session scoping is load-bearing here:
            // `unfiltered_data` is the whole task DB, so counting from it makes
            // every backlog task in the store — other epics, abandoned earlier
            // sessions — read as dispatchable work for THIS worker, and the
            // stand-down branch below becomes unreachable in any repo with a
            // backlog. `data` is reloaded on this same tick whenever the DB
            // changed (`refresh_data`), and `snapshot_at` below names exactly
            // when that read happened.
            let ready_count = dispatchable_ready_count(data, gated_task_ids);
            let idle_summary = if data
                .agents
                .iter()
                .find(|agent| agent.name == *worker)
                .is_some_and(|agent| agent.latest_activity.is_some())
            {
                format!("Worker {worker} finished its task and is now free with no assigned tasks.")
            } else {
                format!(
                    "Worker {worker} has not started a task yet and is idle with no assigned tasks."
                )
            };

            let text = if ready_count > 0 {
                // D-3 (cas-405f): do NOT embed the snapshot count here.
                //
                // `ready_count` comes from the director's epic-filtered snapshot
                // (app::filter_director_agents_to_current_session), which tracks
                // only tasks visible to the current epic scope. The live global
                // `task action=ready` often shows more — confirmed mismatches of
                // "said 1, actual 10" and "said 14, actual 25" were traced to this
                // gap. Advertising a stale number causes the supervisor to
                // under-assign or over-assign, so we remove the specific count and
                // direct them to the live command instead.
                //
                format!(
                    "{idle_summary}\n\
                     Ready tasks exist — check live: `{supervisor_prefix}task action=ready`\n\
                     Assign work: {supervisor_prefix}task action=update id=<task-id> assignee={worker}"
                )
            } else {
                // Do NOT suggest "closing the epic" here — the task snapshot may
                // be stale (cas-b67d D-3): the director refresh window is 2s, and
                // recently-created tasks may not yet be visible. Obeying "close the
                // epic" advice from a stale snapshot would orphan live open work.
                // Direct the supervisor to verify with a live query instead.
                format!(
                    "{idle_summary}\n\
                     No dispatchable tasks in current snapshot ({}) — verify with \
                     `{supervisor_prefix}task action=ready` before acting.\n\
                     If genuinely idle, assign new work or stand down this worker.",
                    snapshot_stamp(snapshot_at)
                )
            };

            Some(Prompt {
                target: supervisor_name.to_string(),
                text: with_response_instructions(&text, worker, supervisor_cli),
                retract_worker: Some(worker.clone()),
                retract_task: None,
                retract_epic: None,
                drop_if_worker_assigned: Some(worker.clone()),
                durable_retry: false,
            })
        }

        DirectorEvent::WorkerStalled {
            worker,
            task_id,
            elapsed_secs,
            escalate,
        } => {
            if !config.on_worker_stalled {
                return None;
            }

            // Guard (cas-c790 pattern): supervisor is never a "worker" for
            // this purpose; and only nudge/escalate for a worker that's
            // still in the live snapshot (stale queued event otherwise).
            if worker == supervisor_name || live_worker_session_id(data, worker).is_none() {
                return None;
            }

            let elapsed_mins = elapsed_secs / 60;
            let assigned_but_unstarted = data.ready_tasks.iter().any(|task| {
                task.id == *task_id
                    && task.status == TaskStatus::Open
                    && task_assigned_to_worker(data, task, worker)
            });

            if assigned_but_unstarted {
                let text = format!(
                    "Worker {worker} has remained assigned-but-unstarted and inactive on task \
                     {task_id} for about {elapsed_mins}m — the worker is alive, but there is no \
                     recent transcript or task activity.\n\n\
                     Check the worker pane and delivery state, then either ask the worker to run \
                     `{worker_prefix}task action=start id={task_id}` or recover the worker if it \
                     is wedged. The normal just-assigned grace window has already elapsed."
                );

                return Some(Prompt {
                    target: supervisor_name.to_string(),
                    text: with_response_instructions(&text, worker, supervisor_cli),
                    retract_worker: None,
                    retract_task: None,
                    retract_epic: None,
                    drop_if_worker_assigned: None,
                    durable_retry: false,
                });
            }

            if !escalate {
                // First detection: auto-nudge the worker directly — a
                // single re-poke often unsticks a stalled agent (cas-9829).
                let text = format!(
                    "You have gone quiet on task {task_id} for about {elapsed_mins}m \
                     (heartbeat is fine, but no tool calls/file edits/commits observed).\n\n\
                     If you are still working, post a progress note now: \
                     {worker_prefix}task action=notes id={task_id} notes=\"...\" note_type=progress\n\
                     If you are blocked, report it: \
                     {worker_prefix}task action=notes id={task_id} notes=\"...\" note_type=blocker\n\
                     If you are done, close the task: {worker_prefix}task action=close id={task_id}"
                );

                Some(Prompt {
                    target: worker.clone(),
                    text: with_response_instructions(&text, supervisor_name, worker_cli),
                    retract_worker: None,
                    retract_task: None,
                    retract_epic: None,
                    drop_if_worker_assigned: None,
                    durable_retry: false,
                })
            } else {
                // Still stalled after the nudge — escalate to the supervisor.
                //
                // cas-728b: the old advice ("consider shutdown + respawn
                // (safe if the worktree is clean)") pointed at the exact
                // anti-pattern that destroyed in-flight work before
                // (silent-owl-56, 2026-04-23): a clean worktree mid-task
                // means un-persisted work + full in-flight context loss, not
                // "safe". Point at the actual triage triad instead —
                // `is-wedged` classifies before anyone kills anything.
                let text = format!(
                    "Worker {worker} has been stalled on task {task_id} for about \
                     {elapsed_mins}m — alive heartbeat, no activity, and an auto-nudge \
                     did not unstick it.\n\n\
                     Triage before acting:\n\
                     1. `cas factory is-wedged {worker}` — classifies Alive / Wedged / \
                     Starved / Dead from PID + transcript evidence.\n\
                     2. `cas factory debug {worker}` — tail the transcript to see the \
                     last in-flight tool call.\n\
                     3. Only `cas factory kill {worker}` if is-wedged reports Wedged or \
                     Dead — a clean worktree does NOT mean safe to kill: it means \
                     un-persisted work and full in-flight context loss if the worker \
                     was still genuinely working."
                );

                Some(Prompt {
                    target: supervisor_name.to_string(),
                    text: with_response_instructions(&text, worker, supervisor_cli),
                    retract_worker: None,
                    retract_task: None,
                    retract_epic: None,
                    drop_if_worker_assigned: None,
                    durable_retry: false,
                })
            }
        }

        DirectorEvent::AgentRegistered {
            agent_id,
            agent_name,
        } => {
            if !config.on_worker_ready {
                return None;
            }

            // Don't notify about supervisor registering
            if agent_name == supervisor_name {
                return None;
            }

            // Guard (cas-889d / cas-dbbb): suppress registration nudge if the
            // newly-registered worker already has an active in_progress task OR an
            // assigned-but-not-yet-started Open task (reconnect after session restart,
            // or assignment during the registration window). Check both ID-keyed and
            // name-keyed assignees for the same reason as WorkerIdle above.
            //
            // Blocked tasks are EXCLUDED (see WorkerIdle guard comment above).
            let worker_already_busy = data
                .in_progress_tasks
                .iter()
                .chain(
                    data.ready_tasks
                        .iter()
                        .filter(|t| t.status == TaskStatus::Open),
                )
                .any(|t| {
                    t.assignee.as_deref() == Some(agent_id.as_str())
                        || t.assignee.as_deref() == Some(agent_name.as_str())
                });
            if worker_already_busy {
                return None;
            }

            // cas-ae6d: same scoping rule as WorkerIdle above — session/epic
            // scope is load-bearing for dispatch advice; the notice is stamped
            // with when that snapshot was read instead.
            let ready_count = dispatchable_ready_count(data, gated_task_ids);
            let text = if ready_count > 0 {
                format!(
                    "Worker {agent_name} has registered and is awaiting its first task.\n\
                     Ready tasks exist — check live: `{supervisor_prefix}task action=ready`\n\
                     Assign work: {supervisor_prefix}task action=update id=<task-id> assignee={agent_name}"
                )
            } else {
                format!(
                    "Worker {agent_name} has registered and is awaiting its first task.\n\
                     No dispatchable tasks in current snapshot ({}) — verify with \
                     `{supervisor_prefix}task action=ready` before acting.",
                    snapshot_stamp(snapshot_at)
                )
            };

            Some(Prompt {
                target: supervisor_name.to_string(),
                text: with_response_instructions(&text, agent_name, supervisor_cli),
                retract_worker: Some(agent_name.clone()),
                retract_task: None,
                retract_epic: None,
                drop_if_worker_assigned: Some(agent_name.clone()),
                durable_retry: false,
            })
        }

        DirectorEvent::EpicStarted { .. } => {
            // No prompt needed - supervisor already knows since they started the epic
            None
        }

        DirectorEvent::EpicCompleted { .. } => {
            // No prompt needed - supervisor already knows since they orchestrated the epic
            // completion (closed tasks, merged branches, shut down workers)
            None
        }

        DirectorEvent::EpicAllSubtasksClosed {
            epic_id,
            epic_title,
        } => {
            if !config.on_epic_completed {
                return None;
            }

            // Direct callers (and a queued event that crossed a state change)
            // bypass the normal revalidation wrapper. Refuse to render an
            // all-subtasks-closed template unless the snapshot still proves
            // that claim; in particular an AwaitingMerge child is neither
            // terminal nor merged and makes closing the epic false advice.
            if !epic_completion_is_current(unfiltered_data, epic_id) {
                return None;
            }

            // cas-9fff: stamp ownership in the payload. Hard suppress only when
            // epic_verification_owner is an explicit other agent — full
            // session-affinity / focus routing lives in
            // `revalidate_event_for_delivery_with_focus` (which has focus
            // context). generate_prompt may be called without focus, so it
            // must not re-suppress session-affinity deliveries that already
            // passed revalidation.
            let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
            let ctx = epic_completion_context(unfiltered_data, epic_id, supervisor_name, None);
            if let Some(ref owner) = ctx.owner {
                let self_ids: Vec<&str> = std::iter::once(supervisor_name)
                    .chain(ctx.supervisor_id.as_deref())
                    .collect();
                if !self_ids.iter().any(|id| *id == owner.as_str()) {
                    tracing::info!(
                        target: "cas::coordination",
                        epic_id = %epic_id,
                        supervisor = %supervisor_name,
                        owner = %owner,
                        "generate_prompt suppressed EpicAllSubtasksClosed for non-owner"
                    );
                    return None;
                }
            }
            let route = route_epic_completion(
                supervisor_name,
                ctx.supervisor_id.as_deref(),
                factory_session.as_deref(),
                ctx.owner.as_deref(),
                // Prefer delivering a stamped prompt once revalidation (or a
                // direct test) admitted the event — force affinity so we get
                // a Deliver route for stamping rather than Suppress.
                true,
                true,
                ctx.owner_live_in_this_session,
                false,
                ctx.epic_present,
            );
            let (owner_label, source, owner_session) = match route {
                EpicCompletionRoute::Suppress { .. } => {
                    // Should be unreachable given the force-affinity flags
                    // above; keep a safe unresolved stamp if it happens.
                    (
                        supervisor_name.to_string(),
                        EpicCompletionOwnershipSource::Unresolved,
                        factory_session.clone(),
                    )
                }
                EpicCompletionRoute::Deliver {
                    owner,
                    source,
                    owner_session,
                } => (owner, source, owner_session),
            };

            let source_label = match source {
                EpicCompletionOwnershipSource::VerificationOwner => "epic_verification_owner",
                EpicCompletionOwnershipSource::SessionAffinity => "session_affinity",
                EpicCompletionOwnershipSource::UnreachableOwnerFallback => {
                    "unreachable_owner_fallback"
                }
                EpicCompletionOwnershipSource::Unresolved => "unresolved",
            };
            let session_label = owner_session
                .as_deref()
                .or(factory_session.as_deref())
                .unwrap_or("(unknown session)");

            let ownership_banner = match source {
                EpicCompletionOwnershipSource::UnreachableOwnerFallback => {
                    format!(
                        "OWNERSHIP: owner={owner_label} (UNREACHABLE — fallback delivery) \
                         session={session_label} source={source_label}\n\
                         Do NOT close this epic or shutdown_workers unless you confirm you own it.\n\n"
                    )
                }
                EpicCompletionOwnershipSource::Unresolved => {
                    format!(
                        "OWNERSHIP: owner={owner_label} session={session_label} source={source_label}\n\
                         Owner could not be verified — decline if this is not your epic.\n\n"
                    )
                }
                _ => {
                    format!(
                        "OWNERSHIP: owner={owner_label} session={session_label} source={source_label}\n\n"
                    )
                }
            };

            let text = format!(
                "{ownership_banner}\
                 All subtasks of epic '{epic_title}' ({epic_id}) are now closed.\n\n\
                 Next steps:\n\
                 - Verify the integrated result\n\
                 - Close the epic: {supervisor_prefix}task action=close id={epic_id} reason=\"All subtasks complete\"\n\
                 - Shut down idle workers if no more work"
            );

            Some(Prompt {
                target: supervisor_name.to_string(),
                text,
                retract_worker: None,
                retract_task: None,
                retract_epic: Some(epic_id.clone()),
                drop_if_worker_assigned: None,
                durable_retry: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::factory::director::data::{ActiveLeaseSummary, AgentSummary, TaskSummary};
    use crate::ui::factory::director::prompts::*;
    use cas_mux::SupervisorCli;
    use cas_types::{AgentStatus, Priority, TaskStatus, TaskType};
    use std::collections::HashMap;

    /// Repo root for prompts that carry no `retract_task` tag: the merge
    /// re-check is never reached, so no real checkout is needed. Merge-alert
    /// cases use a genuine git fixture (see `merge_alert_freshness_tests`).
    fn no_repo() -> &'static Path {
        Path::new("/nonexistent/cas-6eab")
    }

    fn make_data(ready_count: usize) -> DirectorData {
        let ready_tasks: Vec<TaskSummary> = (0..ready_count)
            .map(|i| TaskSummary {
                id: format!("task-{i}"),
                title: format!("Ready Task {i}"),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: None,
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            })
            .collect();

        DirectorData {
            ready_tasks,
            in_progress_tasks: vec![],
            epic_tasks: vec![],
            agents: vec![AgentSummary {
                id: "sess-id-abc123".to_string(),
                name: "swift-fox".to_string(),
                status: AgentStatus::Active,
                registered_at: chrono::Utc::now(),
                current_task: None,
                latest_activity: None,
                last_heartbeat: Some(chrono::Utc::now()),
                pending_messages: 0,
                pending_supervisor_messages: 0,
                latest_supervisor_message_at: None,
                active_lease: None,
                effort: None,
            }],
            activity: vec![],
            agent_id_to_name: [("sess-id-abc123".to_string(), "swift-fox".to_string())]
                .into_iter()
                .collect(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: HashMap::new(),
        }
    }

    fn open_task(id: &str, assignee: Option<&str>) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            title: format!("Task {id}"),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            assignee: assignee.map(str::to_string),
            task_type: TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        }
    }

    fn blocked_task(id: &str, assignee: Option<&str>) -> TaskSummary {
        TaskSummary {
            status: TaskStatus::Blocked,
            epic_verification_owner: None,
            ..open_task(id, assignee)
        }
    }

    fn task_with_status(id: &str, assignee: Option<&str>, status: TaskStatus) -> TaskSummary {
        TaskSummary {
            status,
            ..open_task(id, assignee)
        }
    }

    fn default_config() -> AutoPromptConfig {
        AutoPromptConfig::default()
    }

    fn codex() -> SupervisorCli {
        SupervisorCli::Codex
    }

    fn claude() -> SupervisorCli {
        SupervisorCli::Claude
    }

    /// cas-ae6d (GH #100): the assignment wake-up is the one prompt whose loss
    /// strands a worker — the detector announces each (task, assignee) pair
    /// exactly once — so it must be tagged for durable delivery. Every
    /// informational prompt keeps its historical one-shot behavior.
    #[test]
    fn task_assignment_prompt_is_marked_durable() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "cas-ae6d".to_string(),
            task_title: "Wake the codex worker".to_string(),
            worker: "swift-fox".to_string(),
        };
        let data = make_data(0);
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &default_config(),
            claude(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("assignment prompt is generated");

        assert_eq!(prompt.target, "swift-fox");
        assert!(
            prompt.durable_retry,
            "an assignment wake-up must survive a PTY pane that cannot take it yet"
        );

        // Contrast: an idle notice is informational and stays one-shot.
        let idle = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let idle_prompt = generate_prompt(
            &idle,
            &data,
            &data,
            "supervisor",
            &default_config(),
            claude(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("idle prompt is generated");
        assert!(!idle_prompt.durable_retry);
    }

    /// cas-ae6d (GH #100): "no dispatchable tasks" is an absence claim whose
    /// truth decays, so it must name the snapshot it was read from.
    #[test]
    fn idle_notice_carries_its_snapshot_timestamp() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let data = make_data(0);
        let snapshot_at = chrono::DateTime::parse_from_rfc3339("2026-08-05T22:41:07Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let prompt = generate_prompt_at(
            &event,
            &data,
            &data,
            "supervisor",
            &default_config(),
            claude(),
            codex(),
            &HashSet::new(),
            None,
            snapshot_at,
        )
        .expect("idle prompt is generated");

        assert!(
            prompt.text.contains("No dispatchable tasks"),
            "expected the no-work wording: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("snapshot 2026-08-05 22:41:07Z"),
            "the absence claim must be attributable to a known read: {}",
            prompt.text
        );
    }

    /// cas-ae6d: same stamp on the registration notice, which makes the
    /// identical absence claim.
    #[test]
    fn registration_notice_carries_its_snapshot_timestamp() {
        let event = DirectorEvent::AgentRegistered {
            agent_id: "sess-id-abc123".to_string(),
            agent_name: "swift-fox".to_string(),
        };
        let data = make_data(0);
        let snapshot_at = chrono::DateTime::parse_from_rfc3339("2026-08-05T22:41:07Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let prompt = generate_prompt_at(
            &event,
            &data,
            &data,
            "supervisor",
            &default_config(),
            claude(),
            codex(),
            &HashSet::new(),
            None,
            snapshot_at,
        )
        .expect("registration prompt is generated");

        assert!(
            prompt.text.contains("snapshot 2026-08-05 22:41:07Z"),
            "{}",
            prompt.text
        );
    }

    /// cas-ae6d (GH #100): the dispatchability verdict must stay scoped to
    /// this session's epic. `unfiltered_data` is the whole task DB — other
    /// epics, abandoned earlier sessions — so counting from it would make the
    /// notice advertise out-of-scope work and would make the stand-down
    /// wording unreachable in any repo with a backlog.
    #[test]
    fn idle_notice_keeps_dispatchable_counting_session_scoped() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        // Nothing dispatchable in this session's scope...
        let scoped = make_data(0);
        // ...while the unscoped store still holds unrelated open backlog.
        let mut whole_store = make_data(0);
        whole_store.ready_tasks = vec![open_task("cas-other-epic", None)];

        let prompt = generate_prompt(
            &event,
            &scoped,
            &whole_store,
            "supervisor",
            &default_config(),
            claude(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("idle prompt is generated");

        assert!(
            prompt.text.contains("No dispatchable tasks"),
            "out-of-scope backlog must not be advertised as this worker's work: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("stand down"),
            "the stand-down branch must stay reachable: {}",
            prompt.text
        );
    }

    /// cas-ae6d: the in-function busy guard reads the delivery-time snapshot,
    /// so an assignment outside the tracked epic still suppresses the notice.
    /// (The primary guard is the earlier delivery revalidation, which this
    /// direct call deliberately bypasses to exercise the fallback arm.)
    #[test]
    fn idle_notice_is_suppressed_by_an_assignment_only_the_fresh_snapshot_sees() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let stale = make_data(0);
        let mut fresh = make_data(0);
        fresh.ready_tasks = vec![open_task("cas-assigned", Some("swift-fox"))];

        let prompt = generate_prompt(
            &event,
            &stale,
            &fresh,
            "supervisor",
            &default_config(),
            claude(),
            codex(),
            &HashSet::new(),
            None,
        );

        assert!(
            prompt.is_none(),
            "an idle notice must never contradict a live assignee field"
        );
    }

    #[test]
    fn test_delivery_recheck_drops_worker_idle_after_assignment() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let mut data = make_data(0);
        data.ready_tasks = vec![open_task("cas-next", Some("swift-fox"))];

        let rechecked = revalidate_event_for_delivery_with_focus(&event, &data, "supervisor", None);

        assert!(
            rechecked.is_none(),
            "WorkerIdle generated before assignment must be dropped when delivery sees assigned work"
        );
    }

    #[test]
    fn test_delivery_recheck_drops_worker_idle_after_newer_supervisor_message() {
        let idle_since = chrono::Utc::now();
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let mut data = make_data(0);
        data.agents[0].registered_at = idle_since - chrono::Duration::minutes(5);
        data.agents[0].latest_supervisor_message_at =
            Some(idle_since + chrono::Duration::seconds(1));

        assert!(
            revalidate_event_for_delivery_with_context(
                &event,
                &data,
                "supervisor",
                None,
                Some(idle_since),
            )
            .is_none(),
            "supervisor contact that wins the detection/delivery race must suppress the idle nudge"
        );
    }

    #[test]
    fn test_delivery_recheck_drops_ready_nudge_after_supervisor_contact() {
        let event = DirectorEvent::AgentRegistered {
            agent_id: "sess-id-abc123".to_string(),
            agent_name: "swift-fox".to_string(),
        };
        let mut data = make_data(0);
        let registered_at = data.agents[0].registered_at;
        data.agents[0].latest_supervisor_message_at =
            Some(registered_at + chrono::Duration::seconds(1));

        assert!(
            revalidate_event_for_delivery_with_focus(&event, &data, "supervisor", None).is_none(),
            "a supervisor message after registration makes the ready nudge stale"
        );
    }

    #[test]
    fn test_delivery_recheck_drops_taskless_idle_after_active_task_assignment() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let mut data = make_data(0);
        data.in_progress_tasks = vec![task_with_status(
            "cas-merge",
            Some("swift-fox"),
            TaskStatus::AwaitingMerge,
        )];
        data.agents[0].active_lease = Some(ActiveLeaseSummary {
            task_id: "cas-merge".to_string(),
            task_title: "Merge gated task".to_string(),
            task_status: TaskStatus::AwaitingMerge,
            close_rejected_reason: Some("MERGE REQUIRED: commit not on epic".to_string()),
        });

        assert!(
            revalidate_event_for_delivery_with_focus(&event, &data, "supervisor", None).is_none(),
            "an event enqueued while taskless must be dropped when any assignment lands"
        );
    }

    #[test]
    fn test_delivery_recheck_drops_stale_ready_and_blocked_signals() {
        let ready_event = DirectorEvent::AgentRegistered {
            agent_id: "sess-id-abc123".to_string(),
            agent_name: "swift-fox".to_string(),
        };
        let mut assigned_data = make_data(0);
        assigned_data.ready_tasks = vec![open_task("cas-next", Some("sess-id-abc123"))];

        assert!(
            revalidate_event_for_delivery_with_focus(
                &ready_event,
                &assigned_data,
                "supervisor",
                None,
            )
            .is_none(),
            "ready notification must drop when delivery sees assigned work"
        );

        let blocked_event = DirectorEvent::TaskBlocked {
            task_id: "cas-block".to_string(),
            task_title: "Old blocked title".to_string(),
            worker: "swift-fox".to_string(),
        };
        let mut unblocked_data = make_data(0);
        unblocked_data.ready_tasks = vec![open_task("cas-block", Some("swift-fox"))];

        assert!(
            revalidate_event_for_delivery_with_focus(
                &blocked_event,
                &unblocked_data,
                "supervisor",
                None,
            )
            .is_none(),
            "blocked notification must drop when delivery sees the task is no longer blocked"
        );

        let mut blocked_data = make_data(0);
        blocked_data.ready_tasks = vec![blocked_task("cas-block", Some("swift-fox"))];
        assert!(
            matches!(
                revalidate_event_for_delivery_with_focus(
                    &blocked_event,
                    &blocked_data,
                    "supervisor",
                    None,
                ),
                Some(DirectorEvent::TaskBlocked { .. })
            ),
            "blocked notification should remain when delivery still sees the blocked task"
        );
    }

    /// Regression test for cas-2ca9: a director re-dispatching an
    /// already-Closed task. `detect_changes_at` legitimately emitted
    /// `TaskAssigned` while `cas-9789` was still Open+assigned (dedup guard
    /// means this fires at most once per detector lifetime), but the task
    /// closed in the gap before `revalidate_and_prompt_for_delivery` loaded
    /// its fresh delivery-time snapshot. Before the fix, `TaskAssigned` fell
    /// through the `_` catch-all in `revalidate_event_for_delivery_with_focus` with no
    /// recheck, so the stale "You have been assigned a new task" prompt
    /// still went out for a task the worker (or supervisor) already closed.
    #[test]
    fn test_delivery_recheck_drops_task_assigned_for_already_closed_task() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "cas-9789".to_string(),
            task_title: "Stale assignment".to_string(),
            worker: "swift-fox".to_string(),
        };
        // Delivery-time snapshot: cas-9789 is Closed, so it's absent from
        // both `ready_tasks` and `in_progress_tasks` (the only two buckets
        // `DirectorData` uses for non-terminal tasks).
        let data = make_data(0);

        assert!(
            revalidate_event_for_delivery_with_focus(&event, &data, "supervisor", None).is_none(),
            "TaskAssigned must be dropped when delivery sees the task is no longer active"
        );
    }

    /// Regression for cas-ef0a3: `AwaitingMerge` shares the director's
    /// visibility-oriented `in_progress_tasks` bucket, but worker work is done.
    /// A stale assignment event must be dropped even when a different, genuinely
    /// Open task remains available for dispatch.
    #[test]
    fn test_delivery_recheck_drops_task_assigned_after_awaiting_merge_park() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "cas-merge".to_string(),
            task_title: "Already finished".to_string(),
            worker: "swift-fox".to_string(),
        };
        let mut data = make_data(0);
        data.in_progress_tasks = vec![task_with_status(
            "cas-merge",
            Some("swift-fox"),
            TaskStatus::AwaitingMerge,
        )];
        data.ready_tasks = vec![open_task("cas-next", None)];

        assert!(
            revalidate_event_for_delivery_with_focus(&event, &data, "supervisor", None).is_none(),
            "AwaitingMerge is supervisor-owned merge work, never a worker assignment"
        );

        let open_event = DirectorEvent::TaskAssigned {
            task_id: "cas-next".to_string(),
            task_title: "Genuinely ready".to_string(),
            worker: "swift-fox".to_string(),
        };
        data.ready_tasks[0].assignee = Some("swift-fox".to_string());
        assert!(
            matches!(
                revalidate_event_for_delivery_with_focus(&open_event, &data, "supervisor", None,),
                Some(DirectorEvent::TaskAssigned { .. })
            ),
            "a genuinely Open assignment must remain dispatchable"
        );
    }

    /// The same detect-to-deliver status race applies to the director's
    /// worker-directed stalled/rescue nudge. Once close parks the task
    /// AwaitingMerge, only the supervisor merge prompt is valid.
    #[test]
    fn test_delivery_recheck_drops_worker_stalled_after_awaiting_merge_park() {
        let event = DirectorEvent::WorkerStalled {
            worker: "swift-fox".to_string(),
            task_id: "cas-merge".to_string(),
            elapsed_secs: 600,
            escalate: false,
        };
        let mut data = make_data(0);
        data.in_progress_tasks = vec![task_with_status(
            "cas-merge",
            Some("swift-fox"),
            TaskStatus::AwaitingMerge,
        )];

        assert!(
            revalidate_event_for_delivery_with_focus(&event, &data, "supervisor", None).is_none(),
            "AwaitingMerge must never receive a worker-directed rescue nudge"
        );
    }

    /// Companion regression: the task was reassigned to a DIFFERENT worker
    /// by delivery time (e.g. supervisor force-transfer). The original
    /// worker's stale `TaskAssigned` must not be delivered either — only a
    /// fresh event carrying the new assignee would be correct, and that's a
    /// different (task_id, assignee) key handled by `detect_changes_at`'s own
    /// dedup guard, not this revalidation layer.
    #[test]
    fn test_delivery_recheck_drops_task_assigned_reassigned_to_other_worker() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "cas-9789".to_string(),
            task_title: "Reassigned elsewhere".to_string(),
            worker: "swift-fox".to_string(),
        };
        let mut data = make_data(0);
        data.ready_tasks = vec![open_task("cas-9789", Some("other-worker"))];

        assert!(
            revalidate_event_for_delivery_with_focus(&event, &data, "supervisor", None).is_none(),
            "TaskAssigned must be dropped when delivery sees a different assignee"
        );
    }

    /// Positive control: a genuinely still-valid assignment (task still Open
    /// and assigned to the same worker at delivery time) must survive
    /// revalidation and deliver normally — the fix must not over-suppress.
    #[test]
    fn test_delivery_recheck_keeps_task_assigned_when_still_valid() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "cas-9789".to_string(),
            task_title: "Stale title from detection time".to_string(),
            worker: "swift-fox".to_string(),
        };
        let mut data = make_data(0);
        data.ready_tasks = vec![open_task("cas-9789", Some("swift-fox"))];

        let rechecked = revalidate_event_for_delivery_with_focus(&event, &data, "supervisor", None)
            .expect("still-assigned task must survive revalidation");

        match rechecked {
            DirectorEvent::TaskAssigned {
                task_id, worker, ..
            } => {
                assert_eq!(task_id, "cas-9789");
                assert_eq!(worker, "swift-fox");
            }
            other => panic!("expected TaskAssigned to survive, got {other:?}"),
        }
    }

    /// Positive control: an in-progress task (rather than ready/Open) must
    /// also survive — `TaskAssigned` can be delivered slightly after the
    /// worker already called `task start`, moving the task to InProgress.
    #[test]
    fn test_delivery_recheck_keeps_task_assigned_when_in_progress() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "cas-4321".to_string(),
            task_title: "Already started".to_string(),
            worker: "swift-fox".to_string(),
        };
        let mut data = make_data(0);
        data.in_progress_tasks = vec![TaskSummary {
            id: "cas-4321".to_string(),
            title: "Already started".to_string(),
            status: TaskStatus::InProgress,
            priority: Priority::MEDIUM,
            assignee: Some("swift-fox".to_string()),
            task_type: TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        }];

        assert!(
            matches!(
                revalidate_event_for_delivery_with_focus(&event, &data, "supervisor", None),
                Some(DirectorEvent::TaskAssigned { .. })
            ),
            "TaskAssigned must survive when delivery sees the task now InProgress"
        );
    }

    #[test]
    fn test_task_assigned_prompt() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "task-123".to_string(),
            task_title: "Implement feature X".to_string(),
            worker: "swift-fox".to_string(),
        };
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert_eq!(prompt.target, "swift-fox");
        assert!(prompt.text.contains("task-123"));
        assert!(prompt.text.contains("Implement feature X"));
        assert!(prompt.text.contains("mcp__cs__task action=start"));
        // Response instructions should be appended
        assert!(prompt.text.contains("To respond to this message, use:"));
        assert!(prompt.text.contains("target=supervisor"));
    }

    /// A supervisor may close a gate task they own, but that transition must
    /// never be rendered as a worker assignment or worker completion for the
    /// same supervisor (GH #302).
    #[test]
    fn cas_9d40_supervisor_owned_gate_never_gets_worker_lifecycle_templates() {
        let data = make_data(0);
        let config = default_config();
        let assignment = DirectorEvent::TaskAssigned {
            task_id: "cas-gate".to_string(),
            task_title: "Supervisor gate".to_string(),
            worker: "supervisor".to_string(),
        };
        let completion = DirectorEvent::TaskCompleted {
            task_id: "cas-gate".to_string(),
            task_title: "Supervisor gate".to_string(),
            worker: "supervisor".to_string(),
        };

        for event in [&assignment, &completion] {
            assert!(
                generate_prompt(
                    event,
                    &data,
                    &data,
                    "supervisor",
                    &config,
                    SupervisorCli::Grok,
                    SupervisorCli::Codex,
                    &HashSet::new(),
                    None,
                )
                .is_none(),
                "a supervisor-owned gate must not receive worker lifecycle guidance: {event:?}"
            );
        }
    }

    /// Every injected template must take its tool namespace from the receiving
    /// harness, rather than retaining a literal from a previous CLI flavor.
    #[test]
    fn cas_9d40_injected_templates_only_render_the_live_tool_prefix() {
        let data = make_data(0);
        let events = [
            DirectorEvent::TaskAssigned {
                task_id: "cas-prefix".to_string(),
                task_title: "Prefix guard".to_string(),
                worker: "swift-fox".to_string(),
            },
            DirectorEvent::TaskCompleted {
                task_id: "cas-prefix".to_string(),
                task_title: "Prefix guard".to_string(),
                worker: "swift-fox".to_string(),
            },
        ];
        for cli in [
            SupervisorCli::Claude,
            SupervisorCli::Codex,
            SupervisorCli::Grok,
        ] {
            let prefix = cli.backend().capabilities().tool_prefix;
            for event in &events {
                let prompt = generate_prompt(
                    event,
                    &data,
                    &data,
                    "supervisor",
                    &default_config(),
                    cli,
                    cli,
                    &HashSet::new(),
                    None,
                )
                .expect("worker lifecycle template");
                assert!(
                    prompt.text.contains(&format!("{prefix}task"))
                        || prompt.text.contains(&format!("{prefix}coordination")),
                    "{cli:?}: {}",
                    prompt.text
                );
                for stale in ["mcp__cas__", "mcp__cs__", "cas__"] {
                    if stale != prefix {
                        assert!(
                            !prompt.text.contains(&format!(" {stale}task"))
                                && !prompt.text.contains(&format!(" {stale}coordination")),
                            "{cli:?} template leaked stale prefix {stale}: {}",
                            prompt.text
                        );
                    }
                }
            }
        }
    }

    /// cas-6aaf: TaskCompleted with task already closed (the normal path).
    /// The prompt must NOT instruct the supervisor to ask the worker to close
    /// the task — it was already closed when the event fired.
    #[test]
    fn test_task_completed_prompt_already_closed() {
        let event = DirectorEvent::TaskCompleted {
            task_id: "task-123".to_string(),
            task_title: "Implement feature X".to_string(),
            worker: "swift-fox".to_string(),
        };
        // Task not present in any active set = already closed.
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert_eq!(prompt.target, "supervisor");
        assert!(prompt.text.contains("swift-fox"));
        assert!(prompt.text.contains("task-123"));
        // Must say "closed" not "completed" — reflects actual final state.
        assert!(
            prompt.text.contains("closed"),
            "cas-6aaf: TaskCompleted prompt must say 'closed' (task is already closed): {}",
            prompt.text
        );
        // Must NOT instruct supervisor to close an already-closed task.
        assert!(
            !prompt.text.to_lowercase().contains("task action=close"),
            "cas-6aaf: TaskCompleted must not emit close instruction for already-closed task: {}",
            prompt.text
        );
        // Should clarify verification ownership.
        assert!(prompt.text.contains("workers close their own tasks"));
        assert!(prompt.text.contains("supervisors close epics"));
        // Response instructions should point to the worker.
        assert!(prompt.text.contains("To respond to this message, use:"));
        assert!(prompt.text.contains("target=swift-fox"));
    }

    /// cas-6aaf: TaskCompleted when task regressed to Open (lease expired).
    /// The supervisor SHOULD be asked to have the worker close it — the task
    /// is still open and needs attention.
    #[test]
    fn test_task_completed_prompt_lease_expired_still_open() {
        let event = DirectorEvent::TaskCompleted {
            task_id: "task-123".to_string(),
            task_title: "Implement feature X".to_string(),
            worker: "swift-fox".to_string(),
        };
        // Task is in ready_tasks as Open — lease expired, not yet closed.
        let task = TaskSummary {
            id: "task-123".to_string(),
            title: "Implement feature X".to_string(),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            assignee: Some("swift-fox".to_string()),
            task_type: TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        };
        let data = DirectorData {
            ready_tasks: vec![task],
            in_progress_tasks: vec![],
            epic_tasks: vec![],
            agents: vec![],
            activity: vec![],
            agent_id_to_name: HashMap::new(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: HashMap::new(),
        };
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        // When lease expired and task regressed to Open, supervisor should ask worker to close.
        assert!(
            prompt.text.to_lowercase().contains("task action=close"),
            "cas-6aaf: TaskCompleted for lease-expired Open task must include close instruction: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("task-123"),
            "Prompt must identify the task: {}",
            prompt.text
        );
    }

    /// cas-6aaf: TaskCompleted when task is still InProgress returns None
    /// (stale event, nothing actionable).
    #[test]
    fn test_task_completed_prompt_still_in_progress_suppressed() {
        let event = DirectorEvent::TaskCompleted {
            task_id: "task-123".to_string(),
            task_title: "Implement feature X".to_string(),
            worker: "swift-fox".to_string(),
        };
        // Task is still in in_progress — stale event.
        let task = TaskSummary {
            id: "task-123".to_string(),
            title: "Implement feature X".to_string(),
            status: TaskStatus::InProgress,
            priority: Priority::MEDIUM,
            assignee: Some("swift-fox".to_string()),
            task_type: TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        };
        let data = DirectorData {
            ready_tasks: vec![],
            in_progress_tasks: vec![task],
            epic_tasks: vec![],
            agents: vec![],
            activity: vec![],
            agent_id_to_name: HashMap::new(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: HashMap::new(),
        };
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );
        assert!(
            prompt.is_none(),
            "cas-6aaf: TaskCompleted must be suppressed when task is still in_progress: {:?}",
            prompt.map(|p| p.text)
        );
    }

    #[test]
    fn test_worker_idle_with_ready_tasks() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let data = make_data(3); // 3 ready tasks in snapshot
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert_eq!(prompt.target, "supervisor");
        assert!(prompt.text.contains("swift-fox"));
        assert!(prompt.text.contains("idle"));
        // D-3 (cas-405f): the specific count is intentionally NOT included — the
        // snapshot count diverges from the live global `task action=ready` result
        // because the director filters tasks to the current epic scope. We verify
        // that the prompt directs the supervisor to the live command instead.
        assert!(
            !prompt.text.contains("3 ready tasks"),
            "Prompt must not embed stale snapshot count (D-3): {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("task action=ready"),
            "Prompt must direct supervisor to live task action=ready (D-3): {}",
            prompt.text
        );
        // cas-ed6c: every WorkerIdle prompt must tag its worker so a stale
        // queued copy can be retracted later if the worker gets assigned
        // real work before the supervisor ever reads this row.
        assert_eq!(
            prompt.retract_worker.as_deref(),
            Some("swift-fox"),
            "WorkerIdle prompt must carry retract_worker so prune_stale_idle_alerts \
             can find and retract it later"
        );
    }

    #[test]
    fn queued_worker_idle_is_dropped_if_assignment_lands_before_injection() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let idle_data = make_data(0);
        let prompt = generate_prompt(
            &event,
            &idle_data,
            &idle_data,
            "supervisor",
            &default_config(),
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("taskless worker should initially enqueue an idle prompt");

        let mut assigned_data = make_data(0);
        assigned_data.in_progress_tasks = vec![task_with_status(
            "cas-next",
            Some("swift-fox"),
            TaskStatus::InProgress,
        )];
        assigned_data.agents[0].current_task = Some("cas-next".to_string());

        assert!(
            !prompt_is_still_deliverable(&prompt, &assigned_data, no_repo()),
            "last-mile delivery must drop an already-enqueued WorkerIdle after assignment"
        );
    }

    #[test]
    fn queued_worker_ready_is_dropped_if_assignment_lands_before_injection() {
        let event = DirectorEvent::AgentRegistered {
            agent_id: "sess-id-abc123".to_string(),
            agent_name: "swift-fox".to_string(),
        };
        let idle_data = make_data(0);
        let prompt = generate_prompt(
            &event,
            &idle_data,
            &idle_data,
            "supervisor",
            &default_config(),
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("newly registered taskless worker should enqueue a ready prompt");
        assert!(prompt.text.contains("awaiting its first task"));
        assert_eq!(prompt.retract_worker.as_deref(), Some("swift-fox"));
        assert_eq!(prompt.drop_if_worker_assigned.as_deref(), Some("swift-fox"));

        let mut assigned_data = make_data(0);
        assigned_data.in_progress_tasks = vec![task_with_status(
            "cas-next",
            Some("swift-fox"),
            TaskStatus::InProgress,
        )];
        assigned_data.agents[0].current_task = Some("cas-next".to_string());

        assert!(
            !prompt_is_still_deliverable(&prompt, &assigned_data, no_repo()),
            "last-mile delivery must drop an already-enqueued ready alert after assignment"
        );
    }

    #[test]
    fn worker_idle_text_distinguishes_never_started_from_finished_and_free() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let config = default_config();
        let never_started = make_data(0);
        let never_prompt = generate_prompt(
            &event,
            &never_started,
            &never_started,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();
        assert!(never_prompt.text.contains("has not started a task yet"));

        let mut finished = make_data(0);
        finished.agents[0].registered_at -= chrono::Duration::minutes(5);
        finished.agents[0].latest_activity = Some((
            "closed cas-done".to_string(),
            chrono::Utc::now() - chrono::Duration::seconds(30),
        ));
        let finished_prompt = generate_prompt(
            &event,
            &finished,
            &finished,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();
        assert!(
            finished_prompt
                .text
                .contains("finished its task and is now free")
        );
    }

    #[test]
    fn test_worker_idle_no_ready_tasks() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let data = make_data(0); // No ready tasks
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert_eq!(prompt.target, "supervisor");
        let lower = prompt.text.to_lowercase();
        assert!(
            lower.contains("no ready tasks") || lower.contains("no dispatchable"),
            "Expected 'no ready tasks' or 'no dispatchable' in: {}",
            prompt.text
        );
    }

    #[test]
    fn test_worker_idle_with_close_rejected_task_is_not_completion_worded() {
        // cas-6883: `task_status: InProgress` with a `close_rejected_reason`
        // that still names MERGE REQUIRED is exactly the stale-echo shape
        // from BUG-stale-merge-required-alerts-refire-after-merge.md — a
        // fresh MERGE REQUIRED rejection ALWAYS parks the task to
        // `AwaitingMerge` in the same call (see
        // `run_factory_branch_merge_gate` / `park_task_awaiting_merge` in
        // close_ops.rs), so `InProgress` here means the task was
        // reset/reopened since and the reason string is a leftover echo
        // from `director.rs`'s 50-event activity scan. Before cas-6883 this
        // still got the actionable "factory/swift-fox … merge …" framing
        // (asserted below to have been removed); it must now fall back to
        // the generic, honest idle wording instead.
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: Some(ActiveLeaseSummary {
                task_id: "cas-1234".to_string(),
                task_title: "Fix close gate".to_string(),
                task_status: TaskStatus::InProgress,
                close_rejected_reason: Some("MERGE REQUIRED".to_string()),
            }),
        };
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();
        let lower = prompt.text.to_lowercase();

        assert_eq!(prompt.target, "supervisor");
        assert!(prompt.text.contains("cas-1234"));
        assert!(prompt.text.contains("in_progress"));
        assert!(prompt.text.contains("MERGE REQUIRED"));
        assert!(prompt.text.contains("not a task completion"));
        assert!(
            !lower.contains("done") && !lower.contains("finished"),
            "idle close-rejection prompt must not use completion-flavored wording: {}",
            prompt.text
        );
        // cas-6883: `task_status` is InProgress, not AwaitingMerge, so this
        // must NOT get the actionable merge-queue framing (that would be
        // exactly the stale "instructing a merge that already happened"
        // bug this task fixes) — it stays on the generic informational
        // wording, which doesn't name a factory source branch.
        assert!(
            !prompt.text.contains("factory/swift-fox"),
            "InProgress + stale MERGE REQUIRED echo must NOT get the actionable \
             merge-queue framing (cas-6883): {}",
            prompt.text
        );
        assert!(
            !prompt
                .text
                .contains("MERGE REQUIRED — supervisor action needed"),
            "InProgress + stale MERGE REQUIRED echo must not use the actionable \
             alert header (cas-6883): {}",
            prompt.text
        );
    }

    /// cas-c145: AwaitingMerge idle must be an actionable merge-queue event
    /// (task + factory branch + epic target + next action), not a vague
    /// "resolve the rejection" hint. Push-based — no polling loop wording.
    #[test]
    fn test_c145_awaiting_merge_idle_is_actionable_merge_queue_prompt() {
        let event = DirectorEvent::WorkerIdle {
            worker: "recipe-be".to_string(),
            active_task: Some(ActiveLeaseSummary {
                task_id: "cas-8eff".to_string(),
                task_title: "Backend recipes API".to_string(),
                task_status: TaskStatus::AwaitingMerge,
                close_rejected_reason: Some("MERGE REQUIRED".to_string()),
            }),
        };
        let mut data = make_data(0);
        // make_data seeds a single agent named swift-fox; re-point it so the
        // live-worker session-id guard accepts recipe-be.
        data.agents[0].name = "recipe-be".to_string();
        data.agent_id_to_name
            .insert("sess-id-abc123".to_string(), "recipe-be".to_string());
        data.in_progress_tasks = vec![TaskSummary {
            id: "cas-8eff".to_string(),
            title: "Backend recipes API".to_string(),
            status: TaskStatus::AwaitingMerge,
            priority: Priority::MEDIUM,
            assignee: Some("recipe-be".to_string()),
            task_type: TaskType::Task,
            epic: Some("cas-4c77".to_string()),
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        }];
        data.epic_tasks = vec![TaskSummary {
            id: "cas-4c77".to_string(),
            title: "Dosha recipes epic".to_string(),
            status: TaskStatus::InProgress,
            priority: Priority::HIGH,
            assignee: None,
            task_type: TaskType::Epic,
            epic: None,
            branch: Some(
                "epic/general-dosha-recipes-dual-mode-generation-standal-cas-4c77".to_string(),
            ),
            updated_at: None,
            epic_verification_owner: None,
        }];
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            SupervisorCli::Grok,
            SupervisorCli::Grok,
            &HashSet::new(),
            None,
        )
        .expect("AwaitingMerge idle must produce a supervisor prompt");

        assert_eq!(prompt.target, "supervisor");
        assert!(prompt.text.contains("cas-8eff"), "{}", prompt.text);
        assert!(
            prompt.text.contains("factory/recipe-be"),
            "must name source factory branch: {}",
            prompt.text
        );
        assert!(
            prompt
                .text
                .contains("epic/general-dosha-recipes-dual-mode-generation-standal-cas-4c77"),
            "must name merge target epic branch: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains(
                "git merge --no-ff factory/recipe-be` on epic/general-dosha-recipes-dual-mode-generation-standal-cas-4c77"
            ),
            "epic-target relay must retain its epic branch merge command: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains(
                "Push epic/general-dosha-recipes-dual-mode-generation-standal-cas-4c77 if remote tracking applies"
            ),
            "epic-target relay must retain its epic branch push instruction: {}",
            prompt.text
        );
        assert!(
            prompt
                .text
                .contains("cas__coordination action=epic_status id=cas-4c77")
                || prompt.text.contains("epic_status"),
            "must direct supervisor to epic_status with cas__ prefix for Grok: {}",
            prompt.text
        );
        assert!(
            prompt
                .text
                .contains("cas__task action=list status=awaiting_merge")
                || prompt.text.contains("status=awaiting_merge"),
            "must surface awaiting_merge list: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("git merge --no-ff factory/recipe-be")
                || prompt
                    .text
                    .to_lowercase()
                    .contains("merge factory/recipe-be"),
            "must include merge next action: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("cas__task action=close id=cas-8eff"),
            "homogeneous Grok: worker re-close must use cas__ prefix: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("cas__task action=show id=cas-8eff"),
            "homogeneous Grok: supervisor show must use cas__ prefix: {}",
            prompt.text
        );
        let lower = prompt.text.to_lowercase();
        assert!(
            !lower.contains("poll") || lower.contains("do not poll"),
            "must not introduce a polling loop: {}",
            prompt.text
        );
        assert!(
            !lower.contains("assign"),
            "AwaitingMerge must not be worded as idle-needing-assign: {}",
            prompt.text
        );
        assert!(
            !prompt.text.contains("resolve the rejection before acting"),
            "must not keep the pre-cas-c145 vague wording: {}",
            prompt.text
        );
    }

    /// cas-c145 review P1: mixed harness — Claude supervisor + Codex worker.
    /// Supervisor actions use `mcp__cas__`; the worker re-close command the
    /// supervisor is told to relay must use `mcp__cs__` (never the supervisor
    /// alias). Same shape for Grok workers (`cas__`).
    #[test]
    fn test_c145_mixed_harness_awaiting_merge_uses_worker_prefix_for_reclose() {
        let event = DirectorEvent::WorkerIdle {
            worker: "codex-worker".to_string(),
            active_task: Some(ActiveLeaseSummary {
                task_id: "cas-mix1".to_string(),
                task_title: "Mixed factory merge park".to_string(),
                task_status: TaskStatus::AwaitingMerge,
                close_rejected_reason: Some("MERGE REQUIRED".to_string()),
            }),
        };
        let mut data = make_data(0);
        data.agents[0].name = "codex-worker".to_string();
        data.agent_id_to_name
            .insert("sess-id-abc123".to_string(), "codex-worker".to_string());
        data.in_progress_tasks = vec![TaskSummary {
            id: "cas-mix1".to_string(),
            title: "Mixed factory merge park".to_string(),
            status: TaskStatus::AwaitingMerge,
            priority: Priority::MEDIUM,
            assignee: Some("codex-worker".to_string()),
            task_type: TaskType::Task,
            epic: Some("cas-epic1".to_string()),
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        }];
        data.epic_tasks = vec![TaskSummary {
            id: "cas-epic1".to_string(),
            title: "Epic".to_string(),
            status: TaskStatus::InProgress,
            priority: Priority::HIGH,
            assignee: None,
            task_type: TaskType::Epic,
            epic: None,
            branch: Some("epic/mixed-cas-epic1".to_string()),
            updated_at: None,
            epic_verification_owner: None,
        }];
        let config = default_config();

        // Claude supervisor, Codex worker
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            claude(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("mixed-harness AwaitingMerge must produce a supervisor prompt");

        // Split body from with_response_instructions footer so a Claude
        // footer `mcp__cas__coordination action=message` cannot false-pass
        // supervisor body-command assertions (cas-c145 review follow-up).
        let body = prompt.text.split("\n---\n").next().unwrap_or(&prompt.text);

        // Supervisor-facing body tools: exact Claude alias (not footer-only).
        assert!(
            body.contains("mcp__cas__coordination action=epic_status id=cas-epic1"),
            "supervisor body epic_status must use exact Claude command: {}",
            body
        );
        assert!(
            body.contains("mcp__cas__task action=list status=awaiting_merge"),
            "supervisor body list must use exact Claude command: {}",
            body
        );
        assert!(
            body.contains("mcp__cas__task action=show id=cas-mix1"),
            "supervisor body show must use exact Claude command: {}",
            body
        );
        // Worker prefix must not appear on supervisor body actions.
        assert!(
            !body.contains("mcp__cs__coordination action=epic_status"),
            "supervisor epic_status must not use worker (Codex) prefix: {}",
            body
        );
        assert!(
            !body.contains("mcp__cs__task action=list status=awaiting_merge"),
            "supervisor list must not use worker (Codex) prefix: {}",
            body
        );
        assert!(
            !body.contains("mcp__cs__task action=show id=cas-mix1"),
            "supervisor show must not use worker (Codex) prefix: {}",
            body
        );
        // Worker re-close: Codex alias only
        assert!(
            body.contains("mcp__cs__task action=close id=cas-mix1"),
            "worker re-close must use Codex prefix mcp__cs__: {}",
            body
        );
        assert!(
            !body.contains("mcp__cas__task action=close id=cas-mix1"),
            "worker re-close must NOT use Claude supervisor prefix: {}",
            body
        );

        // Claude supervisor + Grok worker: re-close uses cas__
        let grok_worker_event = DirectorEvent::WorkerIdle {
            worker: "grok-worker".to_string(),
            active_task: Some(ActiveLeaseSummary {
                task_id: "cas-mix2".to_string(),
                task_title: "Grok worker merge park".to_string(),
                task_status: TaskStatus::AwaitingMerge,
                close_rejected_reason: Some("MERGE REQUIRED".to_string()),
            }),
        };
        let mut grok_data = make_data(0);
        grok_data.agents[0].name = "grok-worker".to_string();
        grok_data
            .agent_id_to_name
            .insert("sess-id-abc123".to_string(), "grok-worker".to_string());
        let grok_prompt = generate_prompt(
            &grok_worker_event,
            &grok_data,
            &grok_data,
            "supervisor",
            &config,
            claude(),
            SupervisorCli::Grok,
            &HashSet::new(),
            None,
        )
        .expect("Claude+Grok AwaitingMerge must produce a prompt");
        let grok_body = grok_prompt
            .text
            .split("\n---\n")
            .next()
            .unwrap_or(&grok_prompt.text);

        // Supervisor body commands: exact Claude prefix (not footer `mcp__cas__`).
        assert!(
            grok_body.contains("mcp__cas__coordination action=epic_status id=<focused-epic>"),
            "Claude+Grok supervisor body epic_status must be exact Claude command: {}",
            grok_body
        );
        assert!(
            grok_body.contains("mcp__cas__task action=list status=awaiting_merge"),
            "Claude+Grok supervisor body list must be exact Claude command: {}",
            grok_body
        );
        assert!(
            grok_body.contains("mcp__cas__task action=show id=cas-mix2"),
            "Claude+Grok supervisor body show must be exact Claude command: {}",
            grok_body
        );
        // Negative: bare Grok `cas__` tool calls on supervisor actions.
        // Match the leading backtick so Claude's `mcp__cas__` (which
        // contains the substring `cas__`) does not false-fail the check.
        assert!(
            !grok_body.contains("`cas__coordination action=epic_status"),
            "supervisor epic_status must not use bare worker (Grok) prefix: {}",
            grok_body
        );
        assert!(
            !grok_body.contains("`cas__task action=list status=awaiting_merge"),
            "supervisor list must not use bare worker (Grok) prefix: {}",
            grok_body
        );
        assert!(
            !grok_body.contains("`cas__task action=show id=cas-mix2"),
            "supervisor show must not use bare worker (Grok) prefix: {}",
            grok_body
        );
        // Worker re-close: Grok alias only
        assert!(
            grok_body.contains("cas__task action=close id=cas-mix2"),
            "Grok worker re-close must use cas__ prefix: {}",
            grok_body
        );
        assert!(
            !grok_body.contains("mcp__cas__task action=close id=cas-mix2"),
            "Grok worker re-close must NOT use Claude supervisor prefix: {}",
            grok_body
        );
    }

    /// cas-c145 characterization: non-merge close rejections keep the
    /// informational wording (not the merge-queue template).
    #[test]
    fn test_c145_non_merge_close_rejection_stays_informational() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: Some(ActiveLeaseSummary {
                task_id: "cas-9999".to_string(),
                task_title: "Lint gate".to_string(),
                task_status: TaskStatus::InProgress,
                close_rejected_reason: Some("CODE REVIEW REQUIRED".to_string()),
            }),
        };
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert!(prompt.text.contains("CODE REVIEW REQUIRED"));
        assert!(
            prompt.text.contains("resolve the rejection before acting"),
            "non-merge rejections keep informational wording: {}",
            prompt.text
        );
        assert!(
            !prompt.text.contains("factory/swift-fox"),
            "non-merge rejection must not use the merge-queue template: {}",
            prompt.text
        );
    }

    /// cas-627f: the flagship close-rejected notification, exercised end to
    /// end through BOTH pipeline steps a live director tick actually runs:
    /// `revalidate_event_for_delivery_with_focus` (delivery-time recheck) THEN
    /// `generate_prompt`. Before the cas-627f fix, `active_lease` for a
    /// parked `AwaitingMerge` task resolved to `None` once
    /// `park_task_awaiting_merge` released the lease (confirmed P1,
    /// docs/reviews/2026-07-07-cas-b646-epic.md). The current detector carries
    /// that resolved parked-task state in the event. This distinction matters:
    /// an event enqueued with no task must now be dropped if an assignment
    /// lands later, while a close-rejected event that already named its parked
    /// task must survive and remain actionable.
    #[test]
    fn test_worker_idle_awaiting_merge_close_rejected_survives_revalidate_and_names_task() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: Some(ActiveLeaseSummary {
                task_id: "cas-1234".to_string(),
                task_title: "Fix close gate".to_string(),
                task_status: TaskStatus::AwaitingMerge,
                close_rejected_reason: Some("MERGE REQUIRED".to_string()),
            }),
        };
        let mut data = make_data(0);
        data.in_progress_tasks = vec![TaskSummary {
            id: "cas-1234".to_string(),
            title: "Fix close gate".to_string(),
            status: TaskStatus::AwaitingMerge,
            priority: Priority::MEDIUM,
            assignee: Some("swift-fox".to_string()),
            task_type: TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        }];
        data.agents[0].active_lease = Some(ActiveLeaseSummary {
            task_id: "cas-1234".to_string(),
            task_title: "Fix close gate".to_string(),
            task_status: TaskStatus::AwaitingMerge,
            close_rejected_reason: Some("MERGE REQUIRED".to_string()),
        });
        let config = default_config();

        let revalidated =
            revalidate_event_for_delivery_with_focus(&event, &data, "supervisor", None)
                .expect("close-rejected WorkerIdle must survive delivery-time revalidation");

        let prompt = generate_prompt(
            &revalidated,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("close-rejected WorkerIdle must produce an operator notification");

        assert_eq!(prompt.target, "supervisor");
        assert!(prompt.text.contains("cas-1234"), "{}", prompt.text);
        assert!(
            prompt.text.to_lowercase().contains("awaiting_merge")
                || prompt.text.contains("AwaitingMerge"),
            "notification must name the AwaitingMerge status: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("MERGE REQUIRED"),
            "notification must carry the close-rejected reason: {}",
            prompt.text
        );
        // cas-e48f (deliberately updated from cas-ed6c's original assertion
        // here, which expected `retract_worker`): a MERGE-REQUIRED alert's
        // staleness is about THIS TASK's live merge state, not the named
        // worker's assignment state — `worker_now_has_real_assignment`
        // would wrongly retract a still-outstanding merge alert if the
        // worker got reassigned elsewhere, and would wrongly PRESERVE a
        // stale one if the merge landed with the worker still idle (the
        // literal live incident cas-e48f fixes). It must now carry
        // `retract_task` instead, and must NOT also carry `retract_worker`
        // (a merge alert tagged with the worker-assignment predicate would
        // silence itself for the wrong reason).
        assert_eq!(
            prompt.retract_task.as_deref(),
            Some("cas-1234"),
            "MERGE-REQUIRED WorkerIdle prompt must carry retract_task, keyed on task_id"
        );
        assert_eq!(
            prompt.retract_worker, None,
            "MERGE-REQUIRED WorkerIdle prompt must NOT also carry retract_worker — \
             that predicate is wrong for this alert class (see cas-e48f)"
        );
    }

    /// Regression for cas-b67d D-3: the zero-ready-task nudge must NOT instruct
    /// the supervisor to close the epic. The director snapshot may be stale; the
    /// epic may have open children that just aren't visible in this refresh cycle.
    /// Obeying "close the epic" advice would orphan live work.
    #[test]
    fn test_worker_idle_no_close_epic_advice() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let data = make_data(0); // No ready tasks in snapshot
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        // Must never suggest closing the epic — the snapshot may be stale and
        // the epic might have live open children not visible in this refresh.
        assert!(
            !prompt.text.to_lowercase().contains("closing the epic")
                && !prompt.text.to_lowercase().contains("close the epic"),
            "WorkerIdle nudge must not advise closing the epic (stale-snapshot risk): {:?}",
            prompt.text
        );
    }

    #[test]
    fn test_worker_idle_suppressed_when_worker_absent_from_live_snapshot() {
        let event = DirectorEvent::WorkerIdle {
            worker: "stale-worker".to_string(),
            active_task: None,
        };
        let data = make_data(2);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );

        assert!(
            prompt.is_none(),
            "WorkerIdle must not emit for a worker absent from current DirectorData: {:?}",
            prompt.map(|p| p.text)
        );
    }

    #[test]
    fn test_epic_completed_no_prompt() {
        let event = DirectorEvent::EpicCompleted {
            epic_id: "epic-456".to_string(),
        };
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );

        assert!(
            prompt.is_none(),
            "EpicCompleted should not generate a prompt"
        );
    }

    #[test]
    fn test_epic_all_subtasks_closed_has_no_branch_or_main_instructions() {
        let event = DirectorEvent::EpicAllSubtasksClosed {
            epic_id: "epic-456".to_string(),
            epic_title: "Test Epic".to_string(),
        };
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();
        let lower = prompt.text.to_lowercase();

        assert!(
            !lower.contains("cherry-pick") && !lower.contains("main"),
            "Epic completion prompt must not prescribe branch/merge/main instructions: {}",
            prompt.text
        );
        assert!(prompt.text.contains("task action=close id=epic-456"));
    }

    #[test]
    fn test_worker_ready_prompt() {
        let event = DirectorEvent::AgentRegistered {
            agent_id: "agent-123".to_string(),
            agent_name: "calm-owl".to_string(),
        };
        let data = make_data(3);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert_eq!(prompt.target, "supervisor");
        assert!(prompt.text.contains("calm-owl"));
        assert!(prompt.text.contains("ready"));
        assert!(!prompt.text.contains("3 ready tasks"));
        assert!(prompt.text.contains("task action=ready"));
        assert!(prompt.text.contains("assignee=calm-owl"));
    }

    #[test]
    fn test_worker_ready_no_tasks() {
        let event = DirectorEvent::AgentRegistered {
            agent_id: "agent-123".to_string(),
            agent_name: "calm-owl".to_string(),
        };
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert_eq!(prompt.target, "supervisor");
        assert!(prompt.text.contains("calm-owl"));
        assert!(prompt.text.contains("ready"));
        assert!(prompt.text.contains("No dispatchable tasks"));
        assert!(prompt.text.contains("task action=ready"));
    }

    #[test]
    fn test_worker_ready_disabled() {
        let event = DirectorEvent::AgentRegistered {
            agent_id: "agent-123".to_string(),
            agent_name: "calm-owl".to_string(),
        };
        let data = make_data(0);
        let config = AutoPromptConfig {
            on_worker_ready: false,
            ..default_config()
        };

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );
        assert!(prompt.is_none());
    }

    #[test]
    fn test_supervisor_registered_no_prompt() {
        // Supervisor registering should not notify itself
        let event = DirectorEvent::AgentRegistered {
            agent_id: "agent-sup".to_string(),
            agent_name: "supervisor".to_string(),
        };
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );
        assert!(prompt.is_none());
    }

    #[test]
    fn test_config_disabled_globally() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "task-123".to_string(),
            task_title: "Implement feature X".to_string(),
            worker: "swift-fox".to_string(),
        };
        let data = make_data(0);
        let config = AutoPromptConfig {
            enabled: false,
            ..default_config()
        };

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );
        assert!(prompt.is_none());
    }

    #[test]
    fn test_config_task_assigned_disabled() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "task-123".to_string(),
            task_title: "Implement feature X".to_string(),
            worker: "swift-fox".to_string(),
        };
        let data = make_data(0);
        let config = AutoPromptConfig {
            on_task_assigned: false,
            ..default_config()
        };

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );
        assert!(prompt.is_none());
    }

    #[test]
    fn test_with_response_instructions() {
        let message = "Hello worker, please do X";
        let wrapped = with_response_instructions(message, "supervisor", codex());

        // Original message should be preserved
        assert!(wrapped.starts_with(message));
        // Response instructions should be at the end
        assert!(wrapped.contains("To respond to this message, use:"));
        assert!(wrapped.contains("mcp__cs__coordination action=message"));
        assert!(wrapped.contains("target=supervisor"));
    }

    #[test]
    fn test_claude_prefix_for_worker_and_supervisor() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "task-123".to_string(),
            task_title: "Implement feature X".to_string(),
            worker: "swift-fox".to_string(),
        };
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            claude(),
            claude(),
            &HashSet::new(),
            None,
        )
        .unwrap();
        assert!(prompt.text.contains("mcp__cas__task action=start"));
        assert!(
            prompt
                .text
                .contains("mcp__cas__coordination action=message")
        );
    }

    // ── cas-889d regression tests ─────────────────────────────────────────────

    /// Build a DirectorData with one in-progress task assigned to `assignee`.
    fn make_data_with_in_progress(assignee: &str) -> DirectorData {
        let task = TaskSummary {
            id: "task-active".to_string(),
            title: "Active Task".to_string(),
            status: TaskStatus::InProgress,
            priority: Priority::MEDIUM,
            assignee: Some(assignee.to_string()),
            task_type: TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        };
        DirectorData {
            ready_tasks: vec![],
            in_progress_tasks: vec![task],
            epic_tasks: vec![],
            agents: vec![],
            activity: vec![],
            agent_id_to_name: HashMap::new(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: HashMap::new(),
        }
    }

    /// WorkerIdle assignment guidance must use the worker's display name, not
    /// the session ID. `task mine` matches on display name, and
    /// `task update assignee=<session-id>` gets silently normalized back to
    /// the display name (update.rs:176-186, cas-dbbb) — so the session ID
    /// form just produces a spurious warning. The live-session-ID lookup
    /// (`live_worker_session_id`) still gates whether a prompt fires at all
    /// (cas-c790 defense-in-depth), it just isn't interpolated into the
    /// assignee field.
    #[test]
    fn test_worker_idle_assignee_uses_display_name() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };

        let data = make_data(2);

        let config = default_config();
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert!(
            prompt.text.contains("assignee=swift-fox"),
            "WorkerIdle must use the display name in assignee field, got: {}",
            prompt.text
        );
        assert!(
            !prompt.text.contains("assignee=sess-id-abc123"),
            "WorkerIdle must not use the session ID in assignee field, got: {}",
            prompt.text
        );
    }

    /// cas-889d: WorkerIdle must return None when the worker already has an
    /// in-progress task (ID-keyed assignee path). Prevents spurious idle nudges
    /// that race with actual work.
    #[test]
    fn test_889d_worker_idle_suppressed_when_busy_by_session_id() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };

        // in_progress task assigned by session ID; agent_id_to_name maps it.
        let mut data = make_data_with_in_progress("sess-id-abc123");
        data.agent_id_to_name
            .insert("sess-id-abc123".to_string(), "swift-fox".to_string());

        let config = default_config();
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );

        assert!(
            prompt.is_none(),
            "cas-889d: WorkerIdle must be suppressed when worker has active task (ID key), got: {:?}",
            prompt.map(|p| p.text)
        );
    }

    /// cas-889d: WorkerIdle must return None when the in-progress task uses the
    /// display-name as assignee (legacy manual assignment path).
    #[test]
    fn test_889d_worker_idle_suppressed_when_busy_by_display_name() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };

        // in_progress task assigned by display name (legacy manual path).
        let data = make_data_with_in_progress("swift-fox");
        let config = default_config();
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );

        assert!(
            prompt.is_none(),
            "cas-889d: WorkerIdle must be suppressed when worker has active task (name key), got: {:?}",
            prompt.map(|p| p.text)
        );
    }

    // --- cas-ed6c: worker_now_has_real_assignment + retract_worker tagging --
    //
    // Direct pin of the AC#1 finding: `worker_now_has_real_assignment` is
    // the SAME lease-independent predicate the WorkerIdle delivery-time
    // revalidation arm already uses (`worker_has_open_or_in_progress_assignment`),
    // so `prune_stale_idle_alerts` can never disagree with it. Also proves
    // the falsifiable hypothesis in the ticket's original framing ("keys on
    // lease presence") does NOT hold: a worker with a real InProgress
    // assignment and NO active_lease at all still reads as assigned here.

    /// The reclaimed-lease-but-still-InProgress-and-assigned shape from
    /// AC#4/AC#2: `make_data_with_in_progress` builds a task InProgress with
    /// an assignee, but attaches NO `active_lease` to any agent (there are
    /// no agents in this snapshot at all) — proving the predicate reads the
    /// task store directly and does not require a lease to recognize a real
    /// assignment.
    #[test]
    fn worker_now_has_real_assignment_true_for_in_progress_task_with_no_lease() {
        let data = make_data_with_in_progress("swift-fox");
        assert!(
            worker_now_has_real_assignment(&data, "swift-fox"),
            "an InProgress task assigned to swift-fox must count as a real \
             assignment even with zero lease data in this snapshot"
        );
    }

    /// Negative control: a worker with no matching task anywhere in the
    /// snapshot is not considered assigned.
    #[test]
    fn worker_now_has_real_assignment_false_for_unrelated_worker() {
        let data = make_data_with_in_progress("swift-fox");
        assert!(!worker_now_has_real_assignment(&data, "some-other-worker"));
    }

    /// An Open (not yet started) task assigned to a worker also counts —
    /// mirrors the WorkerIdle guard's own "assigned-but-not-yet-started"
    /// gap coverage (the window between `task update assignee=` and
    /// `task start`).
    #[test]
    fn worker_now_has_real_assignment_true_for_open_assigned_task() {
        let mut data = make_data_with_in_progress("someone-else");
        data.in_progress_tasks.clear();
        data.ready_tasks.push(TaskSummary {
            id: "task-open".to_string(),
            title: "Open Task".to_string(),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            assignee: Some("swift-fox".to_string()),
            task_type: TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        });
        assert!(worker_now_has_real_assignment(&data, "swift-fox"));
    }

    /// A non-WorkerIdle prompt (e.g. `TaskAssigned`) must never carry
    /// `retract_worker` — that tag is specific to WorkerIdle-class alerts,
    /// and accidentally tagging other prompt kinds would let
    /// `prune_stale_idle_alerts` retract the wrong thing.
    #[test]
    fn test_task_assigned_prompt_has_no_retract_worker() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "cas-1234".to_string(),
            task_title: "Some task".to_string(),
            worker: "swift-fox".to_string(),
        };
        let config = default_config();
        let data = make_data(0);
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("TaskAssigned must produce a prompt");
        assert_eq!(
            prompt.retract_worker, None,
            "only WorkerIdle-class prompts should carry retract_worker"
        );
        assert_eq!(
            prompt.retract_task, None,
            "only the MERGE-REQUIRED alert should carry retract_task (cas-e48f)"
        );
    }

    /// AgentRegistered assignment guidance must use the registered display
    /// name, not the session ID — same rationale as WorkerIdle above
    /// (cas-dbbb: `task mine` matches display name; session-id assignees get
    /// silently normalized back to it, so advertising the session id here
    /// just adds a spurious warning).
    #[test]
    fn test_agent_registered_assignee_uses_display_name() {
        let event = DirectorEvent::AgentRegistered {
            agent_id: "sess-id-abc123".to_string(),
            agent_name: "calm-owl".to_string(),
        };
        let data = make_data(2);
        let config = default_config();
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert!(
            prompt.text.contains("assignee=calm-owl"),
            "AgentRegistered must use display name in assignee field, got: {}",
            prompt.text
        );
        assert!(
            !prompt.text.contains("assignee=sess-id-abc123"),
            "AgentRegistered must not use the session ID in assignee field, got: {}",
            prompt.text
        );
    }

    /// cas-889d: AgentRegistered must return None when the worker already has an
    /// active in-progress task (reconnect after session restart).
    #[test]
    fn test_889d_agent_registered_suppressed_when_busy() {
        let event = DirectorEvent::AgentRegistered {
            agent_id: "sess-id-abc123".to_string(),
            agent_name: "calm-owl".to_string(),
        };

        // Busy by session ID.
        let data = make_data_with_in_progress("sess-id-abc123");
        let config = default_config();
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );

        assert!(
            prompt.is_none(),
            "cas-889d: AgentRegistered must be suppressed when worker already has active task, got: {:?}",
            prompt.map(|p| p.text)
        );
    }

    /// cas-dbbb: AgentRegistered and WorkerIdle must be suppressed when the worker
    /// has an assigned Open (not yet InProgress) task. Without this, the director
    /// fires idle/registration nudges in the window between `task update assignee=X`
    /// (task stays Open) and the worker calling `task start` (task becomes InProgress).
    #[test]
    fn test_dbbb_idle_suppressed_when_worker_has_assigned_ready_task() {
        // ready_tasks (Open) with worker as the assignee — simulates the post-assign,
        // pre-start window.
        let task = TaskSummary {
            id: "task-assigned".to_string(),
            title: "Assigned Task".to_string(),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            assignee: Some("swift-fox".to_string()),
            task_type: TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        };
        let data = DirectorData {
            ready_tasks: vec![task],
            in_progress_tasks: vec![],
            epic_tasks: vec![],
            agents: vec![],
            activity: vec![],
            agent_id_to_name: HashMap::new(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: HashMap::new(),
        };
        let config = default_config();

        // WorkerIdle must be suppressed.
        let idle_event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let prompt = generate_prompt(
            &idle_event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );
        assert!(
            prompt.is_none(),
            "cas-dbbb: WorkerIdle must be suppressed when worker has an assigned Open task \
             (post-assign, pre-start window): got {:?}",
            prompt.map(|p| p.text)
        );

        // AgentRegistered must also be suppressed.
        let reg_event = DirectorEvent::AgentRegistered {
            agent_id: "sess-id-xyz".to_string(),
            agent_name: "swift-fox".to_string(),
        };
        let prompt2 = generate_prompt(
            &reg_event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );
        assert!(
            prompt2.is_none(),
            "cas-dbbb: AgentRegistered must be suppressed when worker has an assigned Open task: \
             got {:?}",
            prompt2.map(|p| p.text)
        );
    }

    /// cas-dbbb P2: WorkerIdle must NOT be suppressed when the worker's only task
    /// is Blocked. A Blocked task means the worker is genuinely stalled; the
    /// supervisor still needs an idle nudge so they can resolve the blocker or
    /// assign new work. Including Blocked tasks in the busy-guard would suppress
    /// the nudge indefinitely.
    #[test]
    fn test_dbbb_idle_not_suppressed_when_worker_only_has_blocked_task() {
        let blocked_task = TaskSummary {
            id: "task-blocked".to_string(),
            title: "Blocked Task".to_string(),
            status: TaskStatus::Blocked,
            priority: Priority::MEDIUM,
            assignee: Some("swift-fox".to_string()),
            task_type: TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        };
        let data = DirectorData {
            // Blocked task is in ready_tasks (Open|Blocked both land here).
            ready_tasks: vec![blocked_task],
            in_progress_tasks: vec![],
            epic_tasks: vec![],
            agents: vec![AgentSummary {
                id: "sess-id-abc123".to_string(),
                name: "swift-fox".to_string(),
                status: AgentStatus::Active,
                registered_at: chrono::Utc::now(),
                current_task: None,
                latest_activity: None,
                last_heartbeat: Some(chrono::Utc::now()),
                pending_messages: 0,
                pending_supervisor_messages: 0,
                latest_supervisor_message_at: None,
                active_lease: None,
                effort: None,
            }],
            activity: vec![],
            agent_id_to_name: [("sess-id-abc123".to_string(), "swift-fox".to_string())]
                .into_iter()
                .collect(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: HashMap::new(),
        };

        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let config = default_config();
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );
        assert!(
            prompt.is_some(),
            "cas-dbbb P2: WorkerIdle must NOT be suppressed when worker has only a Blocked task \
             (blocked ≠ busy). Got: None"
        );
    }

    /// cas-dbbb P2: WorkerIdle must be suppressed when the worker has a session-ID
    /// assignee on an Open task in ready_tasks, with agent_id_to_name mapping the
    /// session ID to the worker's display name. This covers the chain()
    /// + session-ID path added in cas-dbbb.
    #[test]
    fn test_dbbb_idle_suppressed_via_session_id_in_ready_open_task() {
        let open_task = TaskSummary {
            id: "task-open-session-id".to_string(),
            title: "Session-ID assigned Open task".to_string(),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            assignee: Some("sess-id-abc123".to_string()),
            task_type: TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        };
        let mut data = DirectorData {
            ready_tasks: vec![open_task],
            in_progress_tasks: vec![],
            epic_tasks: vec![],
            agents: vec![],
            activity: vec![],
            agent_id_to_name: HashMap::new(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: HashMap::new(),
        };
        // The reverse-lookup maps session ID → display name.
        data.agent_id_to_name
            .insert("sess-id-abc123".to_string(), "swift-fox".to_string());

        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let config = default_config();
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );
        assert!(
            prompt.is_none(),
            "cas-dbbb P2: WorkerIdle must be suppressed when worker has a session-ID assigned \
             Open task in ready_tasks (agent_id_to_name reverse-lookup path). Got: {:?}",
            prompt.map(|p| p.text)
        );
    }

    /// cas-dbbb P2: AgentRegistered must be suppressed when the worker's session ID
    /// matches an assignee on an Open task in ready_tasks. This verifies the
    /// chain() + agent_id path added in cas-dbbb.
    #[test]
    fn test_dbbb_agent_registered_suppressed_via_session_id_in_ready_open_task() {
        let open_task = TaskSummary {
            id: "task-reg-session-id".to_string(),
            title: "Session-ID assigned for registration test".to_string(),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            // Assignee is the session UUID (agent_id), not the display name.
            assignee: Some("sess-id-abc123".to_string()),
            task_type: TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        };
        let data = DirectorData {
            ready_tasks: vec![open_task],
            in_progress_tasks: vec![],
            epic_tasks: vec![],
            agents: vec![],
            activity: vec![],
            agent_id_to_name: HashMap::new(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: HashMap::new(),
        };

        let event = DirectorEvent::AgentRegistered {
            agent_id: "sess-id-abc123".to_string(),
            agent_name: "calm-owl".to_string(),
        };
        let config = default_config();
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );
        assert!(
            prompt.is_none(),
            "cas-dbbb P2: AgentRegistered must be suppressed when session ID (agent_id) is the \
             assignee of an Open task in ready_tasks. Got: {:?}",
            prompt.map(|p| p.text)
        );
    }

    // ── cas-c790 regression tests ─────────────────────────────────────────────

    /// cas-c790: WorkerIdle must return None when the "worker" is actually the
    /// supervisor / team-lead. This is defense-in-depth at the prompt layer — the
    /// event detector already filters via is_worker_agent_name, but that gate can
    /// be bypassed when the supervisor's name ends up in worker_names on
    /// resume/reconnect paths (the recurrence pattern described in cas-c790).
    #[test]
    fn test_c790_worker_idle_never_fires_for_supervisor() {
        // The worker name in the event is the supervisor's name.
        let event = DirectorEvent::WorkerIdle {
            worker: "supervisor".to_string(),
            active_task: None,
        };
        let data = make_data(5); // 5 ready tasks — the worst-case scenario
        let config = default_config();

        // Pass "supervisor" as supervisor_name — the prompt must return None.
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );

        assert!(
            prompt.is_none(),
            "cas-c790: WorkerIdle for the supervisor must return None regardless of ready count. \
             Got: {:?}",
            prompt.map(|p| p.text)
        );
    }

    /// cas-c790: WorkerIdle for a legitimate worker must still fire (not
    /// accidentally suppressed by the supervisor-name guard).
    #[test]
    fn test_c790_worker_idle_still_fires_for_real_workers() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        // No in_progress tasks (so the busy guard doesn't suppress).
        let data = make_data(1);
        let config = default_config();

        // "supervisor" is distinct from "swift-fox" — nudge must fire.
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );

        assert!(
            prompt.is_some(),
            "cas-c790: WorkerIdle for a legitimate worker must still produce a prompt. \
             Got: None"
        );
    }

    // ── cas-efc4: Heterogeneous Claude+Codex smoke regression tests ───────────
    //
    // Verifies that `generate_prompt` routes MCP tool prefixes correctly when the
    // supervisor and worker use different CLI harnesses (AC3 + AC5).  All
    // homogeneous tests above use codex()+codex() or claude()+claude(); these
    // tests specifically exercise the mixed-harness surfaces identified in the
    // cas-efc4 scope: director assignment hints (cas-dbbb), harness-aware tool
    // aliases in prompts (cas-8aaf at the prompt layer), and stale-guidance
    // suppression for idle/completed events (cas-6aaf).

    /// cas-efc4 AC3 / cas-dbbb: TaskAssigned to a Codex worker from a Claude
    /// supervisor.  The prompt is sent TO the worker, so it must use the
    /// worker's MCP prefix (`mcp__cs__`).  The response instruction appended at
    /// the end must also use the Codex prefix so the worker can reply.
    #[test]
    fn test_efc4_task_assigned_codex_worker_claude_supervisor_uses_worker_prefix() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "cas-efc4-t1".to_string(),
            task_title: "Smoke test task".to_string(),
            worker: "codex-worker".to_string(),
        };
        let data = make_data(0);
        let config = default_config();

        // Claude supervisor, Codex worker
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            claude(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("TaskAssigned must produce a prompt");

        assert_eq!(
            prompt.target, "codex-worker",
            "cas-efc4 AC3: prompt must target the Codex worker"
        );
        assert!(
            prompt.text.contains("mcp__cs__task action=show"),
            "cas-efc4 AC3: show command must use Codex prefix mcp__cs__: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("mcp__cs__task action=start"),
            "cas-efc4 AC3: start command must use Codex prefix mcp__cs__: {}",
            prompt.text
        );
        // Response instruction: Codex worker replies to Claude supervisor using
        // its own coordination tool.
        assert!(
            prompt.text.contains("mcp__cs__coordination action=message"),
            "cas-efc4 AC3: response instruction must use Codex coordination tool: {}",
            prompt.text
        );
        assert!(
            !prompt.text.contains("mcp__cas__task action=start"),
            "cas-efc4 AC3: must NOT leak Claude prefix into Codex worker prompt: {}",
            prompt.text
        );
    }

    /// cas-efc4 AC3 (other direction): TaskAssigned to a Claude worker from a
    /// Codex supervisor.  Worker tools must be `mcp__cas__`, NOT `mcp__cs__`.
    #[test]
    fn test_efc4_task_assigned_claude_worker_codex_supervisor_uses_cas_prefix() {
        let event = DirectorEvent::TaskAssigned {
            task_id: "cas-efc4-t2".to_string(),
            task_title: "Another smoke task".to_string(),
            worker: "claude-worker".to_string(),
        };
        let data = make_data(0);
        let config = default_config();

        // Codex supervisor, Claude worker
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            claude(),
            &HashSet::new(),
            None,
        )
        .expect("TaskAssigned must produce a prompt");

        assert_eq!(
            prompt.target, "claude-worker",
            "cas-efc4 AC3: prompt must target the Claude worker"
        );
        assert!(
            prompt.text.contains("mcp__cas__task action=start"),
            "cas-efc4 AC3: start command must use Claude prefix mcp__cas__: {}",
            prompt.text
        );
        assert!(
            !prompt.text.contains("mcp__cs__task action=start"),
            "cas-efc4 AC3: must NOT use Codex prefix for Claude worker: {}",
            prompt.text
        );
        assert!(
            prompt
                .text
                .contains("mcp__cas__coordination action=message"),
            "cas-efc4 AC3: response instruction must use Claude coordination tool: {}",
            prompt.text
        );
    }

    /// cas-efc4 AC5 / cas-8aaf (prompt layer): TaskCompleted for a Codex worker
    /// reported to a Claude supervisor.
    ///
    /// cas-6aaf added state-aware routing for TaskCompleted:
    ///   - Task already closed (not in ready/in_progress) → "Worker has closed" path,
    ///     NO close instruction in body.  Regression guard: supervisor must NOT be
    ///     told to re-close a task the worker already closed.
    ///   - Task regressed to Open (lease expired) → "ask worker to close" path,
    ///     close instruction uses the worker's prefix (mcp__cs__task for Codex).
    ///
    /// The response-instruction footer always uses the supervisor's own prefix
    /// (mcp__cas__coordination for Claude supervisor) because it tells the
    /// RECIPIENT how to reply — the recipient always uses their own tools.
    ///
    /// Two sub-tests cover both branches.

    /// cas-efc4 AC5 normal (closed) path: TaskCompleted when task is already
    /// closed must NOT emit a close instruction. Verifies cas-6aaf stale-guidance
    /// suppression in the heterogeneous case (Claude sup + Codex worker).
    #[test]
    fn test_efc4_task_completed_already_closed_no_stale_close_instruction() {
        let event = DirectorEvent::TaskCompleted {
            task_id: "cas-efc4-t3".to_string(),
            task_title: "Done task".to_string(),
            worker: "codex-worker".to_string(),
        };
        // Task absent from both ready_tasks and in_progress_tasks → "already closed"
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            claude(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("TaskCompleted (closed path) must produce a prompt");

        assert_eq!(
            prompt.target, "supervisor",
            "cas-efc4 AC5: TaskCompleted prompt goes to supervisor"
        );
        // cas-6aaf: stale-guidance suppression — no "please close" for already-closed task
        assert!(
            !prompt.text.contains("action=close"),
            "cas-efc4 / cas-6aaf: already-closed path must NOT emit a close instruction: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("closed"),
            "cas-efc4: prompt must confirm the task is already closed: {}",
            prompt.text
        );
        // Response instruction: supervisor (Claude) uses its own coordination tool
        assert!(
            prompt
                .text
                .contains("mcp__cas__coordination action=message"),
            "cas-efc4 AC5: response instruction must use Claude supervisor prefix: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("target=codex-worker"),
            "cas-efc4 AC5: response instruction must address the Codex worker: {}",
            prompt.text
        );
    }

    /// cas-efc4 AC5 regressed-to-Open path: TaskCompleted when the task regressed
    /// to Open (lease expired) must emit a close instruction using the WORKER's
    /// prefix (mcp__cs__task for a Codex worker). Verifies heterogeneous prefix
    /// routing for the recovery branch.
    #[test]
    fn test_efc4_task_completed_regressed_open_close_uses_worker_prefix() {
        let event = DirectorEvent::TaskCompleted {
            task_id: "cas-efc4-t3".to_string(),
            task_title: "Done task".to_string(),
            worker: "codex-worker".to_string(),
        };
        // Put the task into ready_tasks as Open to trigger the "regressed" branch.
        let mut data = make_data(0);
        data.ready_tasks.push(TaskSummary {
            id: "cas-efc4-t3".to_string(),
            title: "Done task".to_string(),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            assignee: None,
            task_type: cas_types::TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        });
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            claude(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("TaskCompleted (regressed) must produce a prompt");

        assert_eq!(
            prompt.target, "supervisor",
            "cas-efc4 AC5: TaskCompleted (regressed) prompt goes to supervisor"
        );
        // Close instruction uses the worker's prefix (Codex → mcp__cs__)
        assert!(
            prompt.text.contains("mcp__cs__task action=close"),
            "cas-efc4 AC5: close instruction must use Codex worker prefix mcp__cs__: {}",
            prompt.text
        );
        assert!(
            !prompt.text.contains("mcp__cas__task action=close"),
            "cas-efc4 AC5: close instruction must NOT use Claude prefix for Codex worker: {}",
            prompt.text
        );
        // Response instruction: supervisor (Claude) uses its own coordination tool
        assert!(
            prompt
                .text
                .contains("mcp__cas__coordination action=message"),
            "cas-efc4 AC5: response instruction must use Claude supervisor prefix: {}",
            prompt.text
        );
    }

    /// cas-efc4 AC5 / cas-dbbb: WorkerIdle for a Codex worker with a Claude
    /// supervisor.
    ///
    /// Prefix routing in the heterogeneous case:
    /// - Body commands address the SUPERVISOR's actions (assigning tasks, checking
    ///   ready queue) → `supervisor_prefix` = `mcp__cas__` (Claude).
    /// - Response instruction tells the SUPERVISOR how to reply → `supervisor_cli`
    ///   = Claude → `mcp__cas__coordination`.
    /// - assignee= uses the worker's display name (cas-dbbb); the live session
    ///   ID lookup still gates whether the prompt fires at all.
    #[test]
    fn test_efc4_worker_idle_codex_worker_claude_supervisor_prefixes() {
        let event = DirectorEvent::WorkerIdle {
            worker: "codex-worker".to_string(),
            active_task: None,
        };
        // 2 ready tasks so the "ready tasks exist" branch fires (non-empty assign cmd).
        let mut data = make_data(2);
        data.agents = vec![AgentSummary {
            id: "sess-id-codex-worker".to_string(),
            name: "codex-worker".to_string(),
            status: AgentStatus::Active,
            registered_at: chrono::Utc::now(),
            current_task: None,
            latest_activity: None,
            last_heartbeat: Some(chrono::Utc::now()),
            pending_messages: 0,
            pending_supervisor_messages: 0,
            latest_supervisor_message_at: None,
            active_lease: None,
            effort: None,
        }];
        data.agent_id_to_name = [(
            "sess-id-codex-worker".to_string(),
            "codex-worker".to_string(),
        )]
        .into_iter()
        .collect();
        let config = default_config();

        // Claude supervisor, Codex worker
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            claude(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("WorkerIdle must produce a prompt");

        assert_eq!(
            prompt.target, "supervisor",
            "cas-efc4 AC5: WorkerIdle prompt goes to the supervisor"
        );
        // Assign command uses supervisor's prefix (Claude supervisor acts)
        assert!(
            prompt.text.contains("mcp__cas__task action=update"),
            "cas-efc4 AC5: assign command must use Claude supervisor prefix: {}",
            prompt.text
        );
        // Ready-check uses supervisor's prefix
        assert!(
            prompt.text.contains("mcp__cas__task action=ready"),
            "cas-efc4 AC5: ready-check must use Claude supervisor prefix: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("assignee=codex-worker"),
            "cas-efc4: assignee must use worker display name: {}",
            prompt.text
        );
        assert!(
            !prompt.text.contains("assignee=sess-id-codex-worker"),
            "cas-efc4: assignee must not use the worker session ID: {}",
            prompt.text
        );
        // Response instruction: supervisor (Claude) uses its own tool to reply
        assert!(
            prompt
                .text
                .contains("mcp__cas__coordination action=message"),
            "cas-efc4 AC5: response instruction (to supervisor) must use Claude coordination prefix: {}",
            prompt.text
        );
        assert!(
            !prompt.text.contains("mcp__cs__task action=update"),
            "cas-efc4 AC5: body assign command must NOT use Codex prefix (supervisor acts): {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("target=codex-worker"),
            "cas-efc4 AC5: response instruction must address the Codex worker: {}",
            prompt.text
        );
    }

    // -------------------------------------------------------------------
    // cas-9829: WorkerStalled prompt generation
    // -------------------------------------------------------------------

    /// First-detection (`escalate = false`) must nudge the worker directly,
    /// not the supervisor — a single re-poke often unsticks a stalled agent.
    #[test]
    fn test_9829_worker_stalled_nudge_targets_worker() {
        let event = DirectorEvent::WorkerStalled {
            worker: "swift-fox".to_string(),
            task_id: "cas-0b7d".to_string(),
            elapsed_secs: 310,
            escalate: false,
        };
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert_eq!(
            prompt.target, "swift-fox",
            "the one-shot auto-nudge must go straight to the stalled worker"
        );
        assert!(prompt.text.contains("cas-0b7d"));
        assert!(prompt.text.contains("5m")); // 310s -> 5m
    }

    /// Once escalated, the prompt must go to the supervisor and name the
    /// stalled worker/task so they can act (check status, respawn, etc.).
    #[test]
    fn test_9829_worker_stalled_escalation_targets_supervisor() {
        let event = DirectorEvent::WorkerStalled {
            worker: "swift-fox".to_string(),
            task_id: "cas-0b7d".to_string(),
            elapsed_secs: 620,
            escalate: true,
        };
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert_eq!(prompt.target, "supervisor");
        assert!(prompt.text.contains("swift-fox"));
        assert!(prompt.text.contains("cas-0b7d"));
    }

    #[test]
    fn test_78bf_assigned_unstarted_escalation_names_state_and_start_action() {
        let event = DirectorEvent::WorkerStalled {
            worker: "swift-fox".to_string(),
            task_id: "cas-unstarted".to_string(),
            elapsed_secs: 310,
            escalate: true,
        };
        let mut data = make_data(0);
        data.ready_tasks
            .push(open_task("cas-unstarted", Some("swift-fox")));
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert_eq!(prompt.target, "supervisor");
        assert!(prompt.text.contains("assigned-but-unstarted and inactive"));
        assert!(prompt.text.contains("task action=start id=cas-unstarted"));
        assert!(
            revalidate_event_for_delivery_with_focus(&event, &data, "supervisor", None).is_some(),
            "the escalation must survive delivery revalidation while the task remains Open and assigned"
        );
    }

    /// cas-728b: the escalation advice used to say "consider shutdown +
    /// respawn (safe if the worktree is clean)" — pointing supervisors at
    /// the exact anti-pattern that destroyed in-flight work before
    /// (silent-owl-56, 2026-04-23: a clean worktree mid-task means
    /// un-persisted work, not "safe"). It must now point at the
    /// `is-wedged` triage triad instead.
    #[test]
    fn test_728b_worker_stalled_escalation_points_at_is_wedged_triage_not_clean_worktree_shutdown()
    {
        let event = DirectorEvent::WorkerStalled {
            worker: "swift-fox".to_string(),
            task_id: "cas-0b7d".to_string(),
            elapsed_secs: 620,
            escalate: true,
        };
        let data = make_data(0);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        assert!(
            !prompt.text.contains("safe if the"),
            "the 'safe if the worktree is clean' anti-pattern must be gone: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("is-wedged swift-fox"),
            "must point at `cas factory is-wedged <worker>` for triage: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("debug swift-fox"),
            "must point at `cas factory debug <worker>` for transcript triage: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("kill swift-fox"),
            "must name the actual kill command, gated on is-wedged's verdict: {}",
            prompt.text
        );
    }

    /// `on_worker_stalled = false` must suppress both the nudge and the
    /// escalation — the master per-event kill switch other event types get.
    #[test]
    fn test_9829_worker_stalled_respects_config_toggle() {
        let mut config = default_config();
        config.on_worker_stalled = false;
        let data = make_data(0);

        for escalate in [false, true] {
            let event = DirectorEvent::WorkerStalled {
                worker: "swift-fox".to_string(),
                task_id: "cas-0b7d".to_string(),
                elapsed_secs: 400,
                escalate,
            };
            assert!(
                generate_prompt(
                    &event,
                    &data,
                    &data,
                    "supervisor",
                    &config,
                    codex(),
                    codex(),
                    &HashSet::new(),
                    None,
                )
                .is_none(),
                "on_worker_stalled=false must suppress WorkerStalled (escalate={escalate})"
            );
        }
    }

    /// A stale queued WorkerStalled event for a worker no longer in the live
    /// snapshot (shutdown/crashed/reassigned) must not fire — same
    /// defense-in-depth guard WorkerIdle uses.
    #[test]
    fn test_9829_worker_stalled_suppressed_for_unknown_worker() {
        let event = DirectorEvent::WorkerStalled {
            worker: "ghost-worker".to_string(),
            task_id: "cas-0b7d".to_string(),
            elapsed_secs: 400,
            escalate: false,
        };
        let data = make_data(0);
        let config = default_config();

        assert!(
            generate_prompt(
                &event,
                &data,
                &data,
                "supervisor",
                &config,
                codex(),
                codex(),
                &HashSet::new(),
                None,
            )
            .is_none(),
            "WorkerStalled must not fire for a worker absent from the live snapshot"
        );
    }

    // -----------------------------------------------------------------
    // cas-09d0: dependency-gated tasks excluded from assignable counts
    // -----------------------------------------------------------------

    fn dep(
        from_id: &str,
        to_id: &str,
        dep_type: cas_types::DependencyType,
    ) -> cas_types::Dependency {
        cas_types::Dependency {
            from_id: from_id.to_string(),
            to_id: to_id.to_string(),
            dep_type,
            created_at: chrono::Utc::now(),
            created_by: None,
        }
    }

    #[test]
    fn test_09d0_compute_gated_task_ids_flags_task_with_open_blocker() {
        let non_closed: HashSet<&str> = ["cas-1", "cas-2"].into_iter().collect();
        let deps = vec![dep("cas-1", "cas-2", cas_types::DependencyType::Blocks)];
        let gated = compute_gated_task_ids(&non_closed, &deps);
        assert!(gated.contains("cas-1"), "cas-1 is blocked by open cas-2");
    }

    #[test]
    fn test_09d0_compute_gated_task_ids_ignores_closed_blocker() {
        // "cas-2" (the blocker) is NOT in non_closed_task_ids, meaning it's
        // closed — matches list_ready()'s `blocker.status != 'closed'` check.
        let non_closed: HashSet<&str> = ["cas-1"].into_iter().collect();
        let deps = vec![dep("cas-1", "cas-2", cas_types::DependencyType::Blocks)];
        let gated = compute_gated_task_ids(&non_closed, &deps);
        assert!(
            !gated.contains("cas-1"),
            "a closed blocker must not gate the dependent task"
        );
    }

    #[test]
    fn test_09d0_compute_gated_task_ids_ignores_non_blocks_dep_types() {
        let non_closed: HashSet<&str> = ["cas-1", "cas-2"].into_iter().collect();
        let deps = vec![dep("cas-1", "cas-2", cas_types::DependencyType::Related)];
        let gated = compute_gated_task_ids(&non_closed, &deps);
        assert!(
            gated.is_empty(),
            "a Related (non-Blocks) dependency must not gate the task"
        );
    }

    #[test]
    fn test_09d0_worker_idle_no_ready_tasks_when_only_task_is_gated() {
        // Regression for the exact bug report point 3: a single Open task
        // exists in the snapshot, but it has an unmet Blocks dependency — the
        // "ready tasks exist — assign" message must NOT fire.
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: None,
        };
        let data = make_data(1); // one Open, unassigned task: "task-0"
        let config = default_config();
        let gated: HashSet<String> = ["task-0".to_string()].into_iter().collect();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &gated,
            None,
        )
        .unwrap();

        let lower = prompt.text.to_lowercase();
        assert!(
            lower.contains("no dispatchable"),
            "the only ready task is gated — must fall through to the \
             no-dispatchable-work message, not 'ready tasks exist': {}",
            prompt.text
        );
    }

    #[test]
    fn test_09d0_agent_registered_no_ready_tasks_when_only_task_is_gated() {
        let event = DirectorEvent::AgentRegistered {
            agent_id: "sess-id-abc123".to_string(),
            agent_name: "swift-fox".to_string(),
        };
        let data = make_data(1);
        let config = default_config();
        let gated: HashSet<String> = ["task-0".to_string()].into_iter().collect();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &gated,
            None,
        )
        .unwrap();

        let lower = prompt.text.to_lowercase();
        assert!(
            lower.contains("no dispatchable"),
            "a gated-only snapshot must not advertise ready tasks on \
             registration either: {}",
            prompt.text
        );
    }

    /// AC (c) hardening: an idle worker parked on an `AwaitingMerge` task must
    /// get the informational framing, never the "please assign" wording —
    /// this is the concrete "not idle-needing-work" requirement.
    #[test]
    fn test_09d0_worker_idle_awaiting_merge_is_not_worded_as_assignable() {
        let event = DirectorEvent::WorkerIdle {
            worker: "swift-fox".to_string(),
            active_task: Some(ActiveLeaseSummary {
                task_id: "cas-1234".to_string(),
                task_title: "Fix close gate".to_string(),
                task_status: TaskStatus::AwaitingMerge,
                close_rejected_reason: None,
            }),
        };
        // Ready tasks ALSO exist in the snapshot — proves the informational
        // branch takes priority over the ready-count branch entirely,
        // regardless of what else is dispatchable.
        let data = make_data(2);
        let config = default_config();

        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();

        let lower = prompt.text.to_lowercase();
        assert!(
            !lower.contains("assign"),
            "an AwaitingMerge-parked worker must never be worded as \
             assignable/idle-needing-work: {}",
            prompt.text
        );
        assert!(prompt.text.contains("not a task completion"));
    }

    // --- cas-9fff: epic-completion ownership routing ---

    #[test]
    fn test_9fff_route_prefers_epic_verification_owner() {
        let owner_route = route_epic_completion(
            "owner-sup",
            Some("owner-id"),
            Some("session-owner"),
            Some("owner-id"),
            false,
            false,
            true,
            false,
            true,
        );
        assert!(matches!(
            owner_route,
            EpicCompletionRoute::Deliver {
                source: EpicCompletionOwnershipSource::VerificationOwner,
                ..
            }
        ));

        let foreign = route_epic_completion(
            "other-sup",
            Some("other-id"),
            Some("session-other"),
            Some("owner-id"),
            false,
            false,
            false,
            false,
            true,
        );
        assert!(matches!(foreign, EpicCompletionRoute::Suppress { .. }));
    }

    #[test]
    fn test_9fff_route_session_affinity_without_owner() {
        let route = route_epic_completion(
            "owner-sup",
            None,
            Some("session-a"),
            None,
            true, // focused
            false,
            false,
            false,
            true,
        );
        assert!(matches!(
            route,
            EpicCompletionRoute::Deliver {
                source: EpicCompletionOwnershipSource::SessionAffinity,
                ..
            }
        ));

        let foreign = route_epic_completion(
            "other-sup",
            None,
            Some("session-b"),
            None,
            false,
            false,
            false,
            false,
            true,
        );
        assert!(matches!(foreign, EpicCompletionRoute::Suppress { .. }));
    }

    #[test]
    fn test_9fff_unreachable_owner_fallback_is_explicit() {
        let route = route_epic_completion(
            "fallback-sup",
            None,
            Some("session-fallback"),
            Some("dead-owner"),
            true,
            true,
            false, // owner not live
            true,  // allow fallback
            true,
        );
        match route {
            EpicCompletionRoute::Deliver {
                source: EpicCompletionOwnershipSource::UnreachableOwnerFallback,
                owner,
                ..
            } => assert_eq!(owner, "dead-owner"),
            other => panic!("expected unreachable fallback, got {other:?}"),
        }

        // Without allow_unreachable_fallback → suppress
        let suppressed = route_epic_completion(
            "fallback-sup",
            None,
            Some("session-fallback"),
            Some("dead-owner"),
            true,
            true,
            false,
            false,
            true,
        );
        assert!(matches!(suppressed, EpicCompletionRoute::Suppress { .. }));
    }

    #[test]
    fn test_9fff_two_supervisors_only_owner_gets_epic_complete_prompt() {
        let event = DirectorEvent::EpicAllSubtasksClosed {
            epic_id: "cas-f4ef".to_string(),
            epic_title: "EPIC: food visual remediation".to_string(),
        };

        // Owner session snapshot: epic owned by owner-sup (by name)
        let mut owner_data = make_data(0);
        owner_data.epic_tasks = vec![TaskSummary {
            id: "cas-f4ef".to_string(),
            title: "EPIC: food visual remediation".to_string(),
            status: TaskStatus::Open,
            priority: Priority::HIGH,
            assignee: None,
            task_type: TaskType::Epic,
            epic: None,
            branch: Some("epic/food".to_string()),
            updated_at: None,
            epic_verification_owner: Some("owner-sup".to_string()),
        }];
        owner_data.agents.push(AgentSummary {
            id: "owner-session-id".to_string(),
            name: "owner-sup".to_string(),
            status: AgentStatus::Active,
            registered_at: chrono::Utc::now(),
            current_task: None,
            latest_activity: None,
            last_heartbeat: Some(chrono::Utc::now()),
            pending_messages: 0,
            pending_supervisor_messages: 0,
            latest_supervisor_message_at: None,
            active_lease: None,
            effort: None,
        });

        // Foreign session sees the same epic (shared DB) but different supervisor
        let foreign_data = owner_data.clone();
        let config = default_config();

        // Revalidation: only owner delivers
        let owner_event =
            revalidate_event_for_delivery_with_focus(&event, &owner_data, "owner-sup", None);
        assert!(
            owner_event.is_some(),
            "owning supervisor must receive EpicAllSubtasksClosed"
        );
        let foreign_event =
            revalidate_event_for_delivery_with_focus(&event, &foreign_data, "other-sup", None);
        assert!(
            foreign_event.is_none(),
            "non-owning concurrent supervisor must NOT receive EpicAllSubtasksClosed"
        );

        // Prompt for owner includes ownership stamp + next steps
        let owner_prompt = generate_prompt(
            &event,
            &owner_data,
            &owner_data,
            "owner-sup",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("owner should get a prompt");
        assert_eq!(owner_prompt.target, "owner-sup");
        assert!(
            owner_prompt.text.contains("OWNERSHIP: owner=owner-sup"),
            "payload must stamp owner for self-filter: {}",
            owner_prompt.text
        );
        assert!(owner_prompt.text.contains("source=epic_verification_owner"));
        assert!(owner_prompt.text.contains("task action=close id=cas-f4ef"));

        // Prompt for foreign supervisor is suppressed
        let foreign_prompt = generate_prompt(
            &event,
            &foreign_data,
            &foreign_data,
            "other-sup",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        );
        assert!(
            foreign_prompt.is_none(),
            "foreign supervisor must not get epic-complete prompt"
        );
    }

    #[test]
    fn test_9fff_epic_complete_prompt_stamps_session_context() {
        let event = DirectorEvent::EpicAllSubtasksClosed {
            epic_id: "epic-456".to_string(),
            epic_title: "Test Epic".to_string(),
        };
        // No epic in data → unresolved deliver path (legacy/single-session)
        let data = make_data(0);
        let config = default_config();
        let prompt = generate_prompt(
            &event,
            &data,
            &data,
            "supervisor",
            &config,
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .unwrap();
        assert!(
            prompt.text.contains("OWNERSHIP:"),
            "must stamp ownership even when unresolved: {}",
            prompt.text
        );
        assert!(prompt.text.contains("task action=close id=epic-456"));
    }

    fn cas06ca_epic_summary(status: TaskStatus) -> TaskSummary {
        TaskSummary {
            id: "cas-epic".to_string(),
            title: "Epic completion currency".to_string(),
            status,
            priority: Priority::HIGH,
            assignee: None,
            task_type: TaskType::Epic,
            epic: None,
            branch: Some("epic/currency".to_string()),
            updated_at: None,
            epic_verification_owner: Some("supervisor".to_string()),
        }
    }

    fn cas06ca_reopened_subtask() -> TaskSummary {
        TaskSummary {
            id: "cas-child".to_string(),
            title: "Reopened work".to_string(),
            status: TaskStatus::Open,
            priority: Priority::HIGH,
            assignee: None,
            task_type: TaskType::Bug,
            epic: Some("cas-epic".to_string()),
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        }
    }

    #[test]
    fn cas06ca_epic_completion_revalidation_uses_authoritative_current_state() {
        let event = DirectorEvent::EpicAllSubtasksClosed {
            epic_id: "cas-epic".to_string(),
            epic_title: "Epic completion currency".to_string(),
        };

        let mut current = make_data(0);
        current.epic_tasks = vec![cas06ca_epic_summary(TaskStatus::InProgress)];
        assert!(
            revalidate_event_for_delivery_with_focus(&event, &current, "supervisor", None)
                .is_some(),
            "a still-open epic with no active subtasks remains actionable"
        );

        let mut closed = current.clone();
        closed.epic_tasks[0].status = TaskStatus::Closed;
        assert!(
            revalidate_event_for_delivery_with_focus(&event, &closed, "supervisor", None).is_none(),
            "an epic closed after detection must suppress its delayed completion prompt"
        );

        let mut reopened = current.clone();
        reopened.ready_tasks.push(cas06ca_reopened_subtask());
        assert!(
            revalidate_event_for_delivery_with_focus(&event, &reopened, "supervisor", None)
                .is_none(),
            "a newly actionable subtask makes the old all-subtasks-closed occurrence stale"
        );

        let missing = make_data(0);
        assert!(
            revalidate_event_for_delivery_with_focus(&event, &missing, "supervisor", None)
                .is_some(),
            "missing epic state is unverifiable and must deliver, never suppress"
        );
    }

    /// An awaiting-merge child remains non-terminal until its factory commits
    /// land. Direct prompt generation must not bypass the delivery wrapper and
    /// claim the epic is ready to close (GH #307).
    #[test]
    fn cas_9d40_epic_completion_template_is_suppressed_for_awaiting_merge_child() {
        let event = DirectorEvent::EpicAllSubtasksClosed {
            epic_id: "cas-epic".to_string(),
            epic_title: "Epic completion currency".to_string(),
        };
        let mut data = make_data(0);
        data.epic_tasks = vec![cas06ca_epic_summary(TaskStatus::InProgress)];
        let mut parked = open_task("cas-child", Some("subtle-cobra-80"));
        parked.status = TaskStatus::AwaitingMerge;
        parked.epic = Some("cas-epic".to_string());
        data.in_progress_tasks.push(parked);

        assert!(
            generate_prompt(
                &event,
                &data,
                &data,
                "supervisor",
                &default_config(),
                SupervisorCli::Grok,
                SupervisorCli::Grok,
                &HashSet::new(),
                None,
            )
            .is_none(),
            "an awaiting-merge child with unmerged work makes epic-close guidance false"
        );
    }

    #[test]
    fn cas06ca_last_mile_recheck_uses_the_same_epic_identity() {
        let event = DirectorEvent::EpicAllSubtasksClosed {
            epic_id: "cas-epic".to_string(),
            epic_title: "Epic completion currency".to_string(),
        };
        let mut current = make_data(0);
        current.epic_tasks = vec![cas06ca_epic_summary(TaskStatus::InProgress)];
        let prompt = generate_prompt(
            &event,
            &current,
            &current,
            "supervisor",
            &default_config(),
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("current epic completion should render");

        assert!(prompt_is_still_deliverable(&prompt, &current, no_repo()));

        let mut closed = current.clone();
        closed.epic_tasks[0].status = TaskStatus::Closed;
        assert!(
            !prompt_is_still_deliverable(&prompt, &closed, no_repo()),
            "the epic id carried by the prompt must stop close-before-transport delivery"
        );

        let mut reopened = current.clone();
        reopened.ready_tasks.push(cas06ca_reopened_subtask());
        assert!(
            !prompt_is_still_deliverable(&prompt, &reopened, no_repo()),
            "the same epic id must stop delivery if a subtask reopens"
        );

        assert!(
            prompt_is_still_deliverable(&prompt, &make_data(0), no_repo()),
            "unavailable epic state must preserve the prompt"
        );
    }

    /// cas-6eab / GH #74, third occurrence: "all subtasks are closed → close
    /// the epic" fired twice while subtasks were still being ADDED to that
    /// epic. A supervisor following it verbatim would have closed a
    /// half-finished epic.
    ///
    /// Distinct from the reopened-subtask case above: these children never
    /// existed when the occurrence was detected, and they arrive in every
    /// non-closed status. `DirectorData` files Open/Blocked into
    /// `ready_tasks` and InProgress/AwaitingMerge into
    /// `in_progress_tasks`, so the last-mile check has to see all five — a
    /// status landing in neither bucket would read as "no children left" and
    /// let the prompt through.
    #[test]
    fn epic_complete_is_dropped_when_a_new_subtask_appears_in_any_open_status() {
        let event = DirectorEvent::EpicAllSubtasksClosed {
            epic_id: "cas-epic".to_string(),
            epic_title: "Epic completion currency".to_string(),
        };
        let mut current = make_data(0);
        current.epic_tasks = vec![cas06ca_epic_summary(TaskStatus::InProgress)];
        let prompt = generate_prompt(
            &event,
            &current,
            &current,
            "supervisor",
            &default_config(),
            codex(),
            codex(),
            &HashSet::new(),
            None,
        )
        .expect("current epic completion should render");
        assert!(
            prompt_is_still_deliverable(&prompt, &current, no_repo()),
            "precondition: deliverable while every child really is closed"
        );

        for status in [
            TaskStatus::Open,
            TaskStatus::Blocked,
            TaskStatus::InProgress,
            TaskStatus::AwaitingMerge,
        ] {
            let mut with_new_child = current.clone();
            let mut child = cas06ca_reopened_subtask();
            child.id = format!("cas-new-{status}");
            child.title = "Subtask added after the occurrence".to_string();
            child.status = status;
            match status {
                TaskStatus::Open | TaskStatus::Blocked => with_new_child.ready_tasks.push(child),
                _ => with_new_child.in_progress_tasks.push(child),
            }
            assert!(
                !prompt_is_still_deliverable(&prompt, &with_new_child, no_repo()),
                "a subtask added at {status} must stop the epic-complete instruction"
            );
        }
    }

    /// cas-6883: send-time freshness re-check for MERGE REQUIRED alerts.
    /// Uses real git repos (same style as `close_ops.rs`'s
    /// `run_factory_branch_merge_gate` tests) since
    /// `check_merge_alert_freshness` shells out to the same
    /// `count_unmerged_factory_commits` helper the close-time gate and
    /// `epic_status` use.
    mod merge_alert_freshness_tests {
        use super::*;
        use std::process::Command;
        use tempfile::TempDir;

        fn git(dir: &std::path::Path, args: &[&str]) {
            let status = Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@test")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@test")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?} failed");
        }

        /// `epic/test-epic` seeded with one commit, `factory/<worker>`
        /// branched off it. Caller adds worker commits and/or merges on
        /// top; returns the tempdir positioned on `factory/<worker>`.
        fn init_repo(worker: &str) -> TempDir {
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path();
            git(p, &["init", "-q", "-b", "epic/test-epic"]);
            std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
            git(p, &["add", "seed.txt"]);
            git(p, &["commit", "-q", "-m", "seed"]);
            git(p, &["checkout", "-q", "-b", &format!("factory/{worker}")]);
            dir
        }

        fn commit_file(dir: &std::path::Path, name: &str) {
            std::fs::write(dir.join(name), "x\n").unwrap();
            git(dir, &["add", name]);
            git(dir, &["commit", "-q", "-m", &format!("feat: {name}")]);
        }

        fn init_bare_remote() -> TempDir {
            let remote = tempfile::tempdir().unwrap();
            git(remote.path(), &["init", "-q", "--bare"]);
            git(
                remote.path(),
                &["symbolic-ref", "HEAD", "refs/heads/epic/test-epic"],
            );
            remote
        }

        fn publish_branch(dir: &std::path::Path, remote: &TempDir, branch: &str) {
            let remote_path = remote.path().to_str().unwrap();
            if !dir.join(".git/refs/remotes/origin").exists() {
                git(dir, &["remote", "add", "origin", remote_path]);
            }
            git(dir, &["push", "-q", "-u", "origin", branch]);
        }

        fn clone_epic(remote: &TempDir) -> TempDir {
            let checkout = tempfile::tempdir().unwrap();
            git(
                checkout.path(),
                &["clone", "-q", remote.path().to_str().unwrap(), "."],
            );
            checkout
        }

        /// Merge `factory/<worker>` into `epic/test-epic` (fast-forward),
        /// leaving the repo checked out on the epic branch.
        fn merge_worker_into_epic(dir: &std::path::Path, worker: &str) {
            git(dir, &["checkout", "-q", "epic/test-epic"]);
            git(
                dir,
                &["merge", "-q", "--ff-only", &format!("factory/{worker}")],
            );
        }

        fn awaiting_merge_data(worker: &str) -> DirectorData {
            let mut data = make_data(0);
            data.agents[0].name = worker.to_string();
            data.in_progress_tasks = vec![TaskSummary {
                id: "cas-6883t".to_string(),
                title: "Freshness test task".to_string(),
                status: TaskStatus::AwaitingMerge,
                priority: Priority::MEDIUM,
                assignee: Some(worker.to_string()),
                task_type: TaskType::Task,
                epic: Some("cas-epic-t".to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }];
            data.epic_tasks = vec![TaskSummary {
                id: "cas-epic-t".to_string(),
                title: "Test epic".to_string(),
                status: TaskStatus::InProgress,
                priority: Priority::HIGH,
                assignee: None,
                task_type: TaskType::Epic,
                epic: None,
                branch: Some("epic/test-epic".to_string()),
                updated_at: None,
                epic_verification_owner: None,
            }];
            data
        }

        fn idle_event(worker: &str, task_status: TaskStatus) -> DirectorEvent {
            DirectorEvent::WorkerIdle {
                worker: worker.to_string(),
                active_task: Some(ActiveLeaseSummary {
                    task_id: "cas-6883t".to_string(),
                    task_title: "Freshness test task".to_string(),
                    task_status,
                    close_rejected_reason: Some("MERGE REQUIRED".to_string()),
                }),
            }
        }

        /// cas-26da / GH #322: the relay must use the task-owned delivery
        /// target, not the session's unrelated focused epic. `TaskSummary`
        /// projects WorkTarget into `branch`, the same target branch close_ops
        /// resolves before its merge gate runs.
        #[test]
        fn merge_required_relay_prefers_declared_work_target_over_parent_epic() {
            let mut data = awaiting_merge_data("recipe-be");
            data.in_progress_tasks[0].branch = Some("main".to_string());
            data.epic_tasks[0].branch = Some("epic/pinned-focus".to_string());
            let event = idle_event("recipe-be", TaskStatus::AwaitingMerge);

            let (parent_epic, target, declared_target) =
                resolve_merge_target_for_task(&data, "cas-6883t");
            assert_eq!(parent_epic.as_deref(), Some("cas-epic-t"));
            assert_eq!(target.as_deref(), Some("main"));
            assert!(declared_target);

            let prompt = generate_prompt(
                &event,
                &data,
                &data,
                "supervisor",
                &default_config(),
                SupervisorCli::Claude,
                SupervisorCli::Claude,
                &HashSet::new(),
                None,
            )
            .expect("AwaitingMerge task must render a relay");
            assert!(
                prompt.text.contains("Merge target: main"),
                "relay must name the WorkTarget branch: {}",
                prompt.text
            );
            assert!(
                prompt.text.contains("git merge --no-ff factory/recipe-be` on main"),
                "main-target relay must derive its merge command from the WorkTarget: {}",
                prompt.text
            );
            assert!(
                prompt.text.contains("Push main if remote tracking applies"),
                "main-target relay must derive its push instruction from the WorkTarget: {}",
                prompt.text
            );
            assert!(
                !prompt.text.contains("epic branch"),
                "main-target relay must not redirect the merge to an epic branch: {}",
                prompt.text
            );
            assert!(
                !prompt.text.contains("epic/pinned-focus"),
                "an unrelated focused epic must never appear as the merge destination: {}",
                prompt.text
            );
        }

        #[test]
        fn ac1_drops_when_branch_already_fully_merged() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");
            merge_worker_into_epic(repo.path(), "recipe-be");

            let data = awaiting_merge_data("recipe-be");
            let event = idle_event("recipe-be", TaskStatus::AwaitingMerge);

            let outcome = check_merge_alert_freshness(&event, &data, repo.path());
            assert!(
                matches!(outcome, MergeAlertFreshness::Stale),
                "already-merged branch must yield Stale (drop the alert): {outcome:?}"
            );
        }

        #[test]
        fn ac1_fetches_and_drops_after_merge_pushed_with_stale_local_epic_ref() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");
            let remote = init_bare_remote();
            publish_branch(repo.path(), &remote, "epic/test-epic");
            publish_branch(repo.path(), &remote, "factory/recipe-be");
            let local_epic_before =
                short_commit_id(&resolve_ref_commit_sha(repo.path(), "epic/test-epic").unwrap());

            let integrator = clone_epic(&remote);
            git(
                integrator.path(),
                &["merge", "-q", "--ff-only", "origin/factory/recipe-be"],
            );
            git(
                integrator.path(),
                &["push", "-q", "origin", "epic/test-epic"],
            );
            assert_eq!(
                short_commit_id(&resolve_ref_commit_sha(repo.path(), "epic/test-epic").unwrap(),),
                local_epic_before,
                "precondition: local epic ref remains stale"
            );
            assert!(
                matches!(
                    known_unmerged_factory_commits(
                        repo.path(),
                        "factory/recipe-be",
                        "origin/epic/test-epic",
                    ),
                    KnownUnmergedCount::KnownPositive(1)
                ),
                "precondition: remote-tracking ref has not observed the pushed merge"
            );

            let outcome = check_merge_alert_freshness(
                &idle_event("recipe-be", TaskStatus::AwaitingMerge),
                &awaiting_merge_data("recipe-be"),
                repo.path(),
            );
            assert!(
                matches!(outcome, MergeAlertFreshness::Stale),
                "pushed merge visible on origin must suppress stale-local alert: {outcome:?}"
            );
            assert!(
                matches!(
                    known_unmerged_factory_commits(
                        repo.path(),
                        "factory/recipe-be",
                        "origin/epic/test-epic",
                    ),
                    KnownUnmergedCount::KnownZero
                ),
                "production freshness check must fetch the pushed merge itself"
            );
        }

        #[test]
        fn ac1_drops_when_task_no_longer_awaiting_merge() {
            // Branch genuinely has unmerged commits, but the task's live
            // status has already moved off AwaitingMerge — must not be
            // treated as an actionable merge-required signal.
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");

            let data = awaiting_merge_data("recipe-be");
            let event = idle_event("recipe-be", TaskStatus::InProgress);

            let outcome = check_merge_alert_freshness(&event, &data, repo.path());
            assert!(
                matches!(outcome, MergeAlertFreshness::NotApplicable),
                "task no longer AwaitingMerge must not produce a merge alert: {outcome:?}"
            );
        }

        /// cas-6eab / GH #74, the load-bearing regression: a MERGE REQUIRED
        /// alert generated while the merge was genuinely outstanding must be
        /// dropped at the last mile if the merge lands before injection.
        ///
        /// Order matters here and mirrors production: the daemon tick
        /// generates prompts FIRST, then runs `handle_epic_change` (which
        /// performs merges), then injects. Before this fix `retract_task` was
        /// the only state-bearing tag with no last-mile check, so this alert
        /// was delivered quoting a tip the epic had already moved past.
        #[test]
        fn merge_alert_generated_before_the_merge_is_dropped_at_injection() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");

            let data = awaiting_merge_data("recipe-be");
            let event = idle_event("recipe-be", TaskStatus::AwaitingMerge);
            let evidence = match check_merge_alert_freshness(&event, &data, repo.path()) {
                MergeAlertFreshness::Fresh(evidence) => evidence,
                other => panic!("precondition: alert must be live when generated: {other:?}"),
            };
            let prompt = generate_prompt(
                &event,
                &data,
                &data,
                "supervisor",
                &default_config(),
                SupervisorCli::Claude,
                SupervisorCli::Claude,
                &HashSet::new(),
                Some(&evidence),
            )
            .expect("live merge alert must render");
            assert_eq!(
                prompt.retract_task.as_deref(),
                Some("cas-6883t"),
                "the alert must carry its task identity for the last-mile check"
            );
            assert!(
                prompt_is_still_deliverable(&prompt, &data, repo.path()),
                "precondition: still deliverable while the merge is outstanding"
            );

            // The merge lands between generation and injection.
            merge_worker_into_epic(repo.path(), "recipe-be");

            assert!(
                !prompt_is_still_deliverable(&prompt, &data, repo.path()),
                "an alert whose merge landed mid-tick must not be injected"
            );
        }

        /// The same last-mile check must not eat a legitimate alert: a task
        /// that is still parked with unmerged commits survives, and so does
        /// an unverifiable one (no assignee → nothing to diff).
        #[test]
        fn last_mile_check_preserves_live_and_unverifiable_merge_alerts() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");
            let data = awaiting_merge_data("recipe-be");
            let prompt = Prompt {
                target: "supervisor".to_string(),
                text: "⚠️ MERGE REQUIRED".to_string(),
                retract_worker: None,
                retract_task: Some("cas-6883t".to_string()),
                retract_epic: None,
                drop_if_worker_assigned: None,
                durable_retry: false,
            };
            assert!(
                prompt_is_still_deliverable(&prompt, &data, repo.path()),
                "a genuinely outstanding merge must still reach the supervisor"
            );

            let mut unverifiable = data.clone();
            unverifiable.in_progress_tasks[0].assignee = None;
            assert!(
                prompt_is_still_deliverable(&prompt, &unverifiable, repo.path()),
                "no assignee means nothing to diff — uncertainty must deliver, not suppress"
            );
        }

        /// GH #74's third occurrence, at the same last mile: the task left
        /// `AwaitingMerge` (re-closed after the supervisor merged) while the
        /// alert was queued behind other prompts in the injection loop.
        #[test]
        fn merge_alert_is_dropped_when_the_task_leaves_awaiting_merge() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");
            let mut data = awaiting_merge_data("recipe-be");
            let prompt = Prompt {
                target: "supervisor".to_string(),
                text: "⚠️ MERGE REQUIRED".to_string(),
                retract_worker: None,
                retract_task: Some("cas-6883t".to_string()),
                retract_epic: None,
                drop_if_worker_assigned: None,
                durable_retry: false,
            };

            data.in_progress_tasks[0].status = TaskStatus::InProgress;
            assert!(
                !prompt_is_still_deliverable(&prompt, &data, repo.path()),
                "a task no longer parked has no outstanding merge to demand"
            );

            data.in_progress_tasks.clear();
            assert!(
                !prompt_is_still_deliverable(&prompt, &data, repo.path()),
                "a task gone from the snapshot entirely is likewise resolved"
            );
        }

        #[test]
        fn ac2_fresh_evidence_carries_unmerged_count_and_epic_sha() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");
            commit_file(repo.path(), "b.rs");

            let data = awaiting_merge_data("recipe-be");
            let event = idle_event("recipe-be", TaskStatus::AwaitingMerge);

            let outcome = check_merge_alert_freshness(&event, &data, repo.path());
            let evidence = match outcome {
                MergeAlertFreshness::Fresh(e) => e,
                other => {
                    panic!("expected Fresh evidence for a genuinely unmerged branch: {other:?}")
                }
            };
            assert_eq!(evidence.task_id, "cas-6883t");
            assert_eq!(evidence.factory_branch, "factory/recipe-be");
            assert_eq!(evidence.unmerged_count, 2);
            assert!(!evidence.epic_sha.is_empty(), "epic SHA must be captured");

            // The alert text itself must embed the evidence inline (AC2).
            let config = default_config();
            let prompt = generate_prompt(
                &event,
                &data,
                &data,
                "supervisor",
                &config,
                SupervisorCli::Claude,
                SupervisorCli::Claude,
                &HashSet::new(),
                Some(&evidence),
            )
            .expect("AwaitingMerge idle with fresh evidence must produce a prompt");
            assert!(
                prompt.text.contains("2 unmerged commit"),
                "alert text must include the unmerged commit count: {}",
                prompt.text
            );
            assert!(
                prompt.text.contains(&evidence.epic_sha),
                "alert text must include the epic SHA the check ran against: {}",
                prompt.text
            );
        }

        #[test]
        fn ac2_genuine_unmerged_alert_discloses_local_origin_ref_split() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");
            commit_file(repo.path(), "b.rs");
            let remote = init_bare_remote();
            publish_branch(repo.path(), &remote, "epic/test-epic");
            publish_branch(repo.path(), &remote, "factory/recipe-be");

            let integrator = clone_epic(&remote);
            commit_file(integrator.path(), "unrelated.rs");
            git(
                integrator.path(),
                &["push", "-q", "origin", "epic/test-epic"],
            );
            let data = awaiting_merge_data("recipe-be");
            let event = idle_event("recipe-be", TaskStatus::AwaitingMerge);
            let evidence = match check_merge_alert_freshness(&event, &data, repo.path()) {
                MergeAlertFreshness::Fresh(evidence) => evidence,
                other => panic!("genuinely unmerged branch must still alert: {other:?}"),
            };
            assert_eq!(evidence.unmerged_count, 2);
            assert_eq!(evidence.checked_epic_ref, "origin/epic/test-epic");
            assert!(evidence.ref_disagreement.is_some());

            let prompt = generate_prompt(
                &event,
                &data,
                &data,
                "supervisor",
                &default_config(),
                SupervisorCli::Claude,
                SupervisorCli::Claude,
                &HashSet::new(),
                Some(&evidence),
            )
            .unwrap();
            assert!(prompt.text.contains("Git ref disagreement detected"));
            assert!(prompt.text.contains("origin/epic/test-epic"));
        }

        #[test]
        fn origin_positive_overrides_local_zero_and_keeps_alert_actionable() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");
            let remote = init_bare_remote();
            publish_branch(repo.path(), &remote, "epic/test-epic");
            publish_branch(repo.path(), &remote, "factory/recipe-be");

            // Merge only in this checkout. The local epic contains the work,
            // but the authoritative remote epic still does not.
            merge_worker_into_epic(repo.path(), "recipe-be");
            assert!(matches!(
                known_unmerged_factory_commits(repo.path(), "factory/recipe-be", "epic/test-epic",),
                KnownUnmergedCount::KnownZero
            ));
            assert!(matches!(
                known_unmerged_factory_commits(
                    repo.path(),
                    "factory/recipe-be",
                    "origin/epic/test-epic",
                ),
                KnownUnmergedCount::KnownPositive(1)
            ));

            let evidence = match check_merge_alert_freshness(
                &idle_event("recipe-be", TaskStatus::AwaitingMerge),
                &awaiting_merge_data("recipe-be"),
                repo.path(),
            ) {
                MergeAlertFreshness::Fresh(evidence) => evidence,
                other => panic!(
                    "origin-positive state needs an actionable push/merge alert, got {other:?}"
                ),
            };
            assert_eq!(evidence.checked_epic_ref, "origin/epic/test-epic");
            assert_eq!(evidence.unmerged_count, 1);
            assert!(evidence.push_required);
            assert!(
                evidence.ref_disagreement.is_some(),
                "local-zero/origin-positive divergence must be disclosed"
            );

            let prompt = generate_prompt(
                &idle_event("recipe-be", TaskStatus::AwaitingMerge),
                &awaiting_merge_data("recipe-be"),
                &awaiting_merge_data("recipe-be"),
                "supervisor",
                &default_config(),
                SupervisorCli::Claude,
                SupervisorCli::Claude,
                &HashSet::new(),
                Some(&evidence),
            )
            .unwrap();
            assert!(prompt.text.contains("Push required"));
            assert!(prompt.text.contains("do not repeat the local merge"));
        }

        fn observation(
            epic_ref: &str,
            commit_id: Option<&str>,
            count: KnownUnmergedCount,
        ) -> MergeRefObservation {
            MergeRefObservation {
                epic_ref: epic_ref.to_string(),
                commit_id: commit_id.map(str::to_string),
                count,
            }
        }

        #[test]
        fn unknown_observation_matrix_is_conservative() {
            let unknown_local = observation("epic/test-epic", None, KnownUnmergedCount::Unknown);
            let unknown_origin =
                observation("origin/epic/test-epic", None, KnownUnmergedCount::Unknown);
            assert!(matches!(
                classify_merge_alert_observations(
                    "cas-6883t",
                    "factory/recipe-be",
                    unknown_local.clone(),
                    unknown_origin.clone(),
                ),
                MergeAlertFreshness::NotApplicable
            ));

            let positive_local = observation(
                "epic/test-epic",
                Some("1111111111111111111111111111111111111111"),
                KnownUnmergedCount::KnownPositive(2),
            );
            let positive_origin = observation(
                "origin/epic/test-epic",
                Some("2222222222222222222222222222222222222222"),
                KnownUnmergedCount::KnownPositive(3),
            );

            match classify_merge_alert_observations(
                "cas-6883t",
                "factory/recipe-be",
                unknown_local,
                positive_origin,
            ) {
                MergeAlertFreshness::Fresh(evidence) => {
                    assert_eq!(evidence.unmerged_count, 3);
                    assert_eq!(evidence.checked_epic_ref, "origin/epic/test-epic");
                }
                other => panic!("Unknown/Positive must remain actionable: {other:?}"),
            }

            match classify_merge_alert_observations(
                "cas-6883t",
                "factory/recipe-be",
                positive_local,
                unknown_origin,
            ) {
                MergeAlertFreshness::Fresh(evidence) => {
                    assert_eq!(evidence.unmerged_count, 2);
                    assert_eq!(evidence.checked_epic_ref, "epic/test-epic");
                }
                other => panic!("Positive/Unknown must remain actionable: {other:?}"),
            }
        }

        #[test]
        fn not_applicable_when_epic_branch_unresolvable() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");

            let mut data = awaiting_merge_data("recipe-be");
            data.epic_tasks.clear(); // epic link present on the task, but epic row missing
            let event = idle_event("recipe-be", TaskStatus::AwaitingMerge);

            let outcome = check_merge_alert_freshness(&event, &data, repo.path());
            assert!(
                matches!(outcome, MergeAlertFreshness::NotApplicable),
                "unresolvable epic branch must not silently drop a possibly-valid alert: {outcome:?}"
            );
        }

        #[test]
        fn ac3_dedup_suppresses_identical_repeat_but_allows_changed_evidence() {
            let mut detector = crate::ui::factory::director::events::DirectorEventDetector::new(
                vec!["recipe-be".to_string()],
                "supervisor".to_string(),
            );

            assert!(
                detector.merge_alert_should_emit("cas-6883t", "factory/recipe-be", 2, "abc1234"),
                "first emission with this evidence must be allowed"
            );
            assert!(
                !detector.merge_alert_should_emit("cas-6883t", "factory/recipe-be", 2, "abc1234"),
                "identical repeat evidence must be suppressed (AC3)"
            );
            assert!(
                detector.merge_alert_should_emit("cas-6883t", "factory/recipe-be", 3, "abc1234"),
                "changed unmerged count is a real state change and must re-emit"
            );
            assert!(
                detector.merge_alert_should_emit("cas-6883t", "factory/recipe-be", 3, "def5678"),
                "changed epic SHA is a real state change and must re-emit"
            );
            assert!(
                !detector.merge_alert_should_emit("cas-6883t", "factory/recipe-be", 3, "def5678"),
                "repeat of the new evidence must again be suppressed"
            );
        }

        // --- cas-e48f: check_merge_alert_freshness_for_task — the sweep-time
        // counterpart keyed on task_id, re-checked against the live epic tip
        // at sweep time rather than an event snapshot from generation time.

        /// AC1: the alert's merge has since landed — retract.
        #[test]
        fn task_sweep_stale_when_branch_already_fully_merged() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");
            merge_worker_into_epic(repo.path(), "recipe-be");

            let data = awaiting_merge_data("recipe-be");
            let outcome = check_merge_alert_freshness_for_task("cas-6883t", &data, repo.path());
            assert!(
                matches!(outcome, MergeAlertFreshness::Stale),
                "landed merge must be Stale (retract the queued row): {outcome:?}"
            );
        }

        #[test]
        fn task_sweep_fetches_and_retracts_after_pushed_merge_with_stale_local_epic_ref() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");
            let remote = init_bare_remote();
            publish_branch(repo.path(), &remote, "epic/test-epic");
            publish_branch(repo.path(), &remote, "factory/recipe-be");
            let local_epic_before =
                short_commit_id(&resolve_ref_commit_sha(repo.path(), "epic/test-epic").unwrap());

            let integrator = clone_epic(&remote);
            git(
                integrator.path(),
                &["merge", "-q", "--ff-only", "origin/factory/recipe-be"],
            );
            git(
                integrator.path(),
                &["push", "-q", "origin", "epic/test-epic"],
            );
            assert_eq!(
                short_commit_id(&resolve_ref_commit_sha(repo.path(), "epic/test-epic").unwrap(),),
                local_epic_before,
                "precondition: queued-row sweep begins with stale local epic"
            );

            let outcome = check_merge_alert_freshness_for_task(
                "cas-6883t",
                &awaiting_merge_data("recipe-be"),
                repo.path(),
            );
            assert!(
                matches!(outcome, MergeAlertFreshness::Stale),
                "supervisor alert must retract using origin evidence: {outcome:?}"
            );
        }

        /// AC1 (task no longer tracked): if the task has fallen out of the
        /// current in_progress snapshot entirely (closed, reset elsewhere),
        /// the alert's premise no longer holds — retract.
        #[test]
        fn task_sweep_stale_when_task_no_longer_in_progress() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");

            let mut data = awaiting_merge_data("recipe-be");
            data.in_progress_tasks.clear();

            let outcome = check_merge_alert_freshness_for_task("cas-6883t", &data, repo.path());
            assert!(
                matches!(outcome, MergeAlertFreshness::Stale),
                "task absent from the current snapshot must be Stale: {outcome:?}"
            );
        }

        /// AC1 (task moved off AwaitingMerge): re-closed / reopened /
        /// reassigned since the alert was written — retract.
        #[test]
        fn task_sweep_stale_when_task_left_awaiting_merge() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");

            let mut data = awaiting_merge_data("recipe-be");
            data.in_progress_tasks[0].status = TaskStatus::InProgress;

            let outcome = check_merge_alert_freshness_for_task("cas-6883t", &data, repo.path());
            assert!(
                matches!(outcome, MergeAlertFreshness::Stale),
                "task no longer AwaitingMerge must be Stale: {outcome:?}"
            );
        }

        /// AC2: merge is still genuinely outstanding — preserve (Fresh).
        #[test]
        fn task_sweep_preserves_when_still_genuinely_outstanding() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");
            commit_file(repo.path(), "b.rs");

            let data = awaiting_merge_data("recipe-be");
            let outcome = check_merge_alert_freshness_for_task("cas-6883t", &data, repo.path());
            match outcome {
                MergeAlertFreshness::Fresh(evidence) => {
                    assert_eq!(evidence.unmerged_count, 2);
                    assert_eq!(evidence.task_id, "cas-6883t");
                }
                other => panic!("still-outstanding merge must be Fresh: {other:?}"),
            }
        }

        /// AC4: the epic tip moved for an UNRELATED reason (a second,
        /// unrelated worker's branch merged into the epic) but THIS task's
        /// own commits are still unmerged — must NOT be retracted. Proves
        /// the sweep re-diffs against the live tip rather than trusting any
        /// snapshot captured when the row was written.
        #[test]
        fn task_sweep_preserves_when_epic_tip_moved_for_unrelated_reason() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");

            // A second, unrelated worker merges into the epic branch first —
            // the epic tip moves, but recipe-be's own commit is still
            // unmerged.
            git(repo.path(), &["checkout", "-q", "epic/test-epic"]);
            git(
                repo.path(),
                &["checkout", "-q", "-b", "factory/other-worker"],
            );
            commit_file(repo.path(), "unrelated.rs");
            merge_worker_into_epic(repo.path(), "other-worker");

            let data = awaiting_merge_data("recipe-be");
            let outcome = check_merge_alert_freshness_for_task("cas-6883t", &data, repo.path());
            match outcome {
                MergeAlertFreshness::Fresh(evidence) => {
                    assert_eq!(
                        evidence.unmerged_count, 1,
                        "recipe-be's own unmerged commit must still be counted \
                         even though the epic tip moved for an unrelated reason"
                    );
                }
                other => panic!(
                    "an unrelated epic-tip move must not retract a still-outstanding \
                     alert: {other:?}"
                ),
            }
        }

        /// `NotApplicable` when the epic branch can't be resolved from this
        /// snapshot — mirrors `check_merge_alert_freshness`'s stance: don't
        /// silently drop a possibly-valid alert over a data-linking gap.
        #[test]
        fn task_sweep_not_applicable_when_epic_branch_unresolvable() {
            let repo = init_repo("recipe-be");
            commit_file(repo.path(), "a.rs");

            let mut data = awaiting_merge_data("recipe-be");
            data.epic_tasks.clear();

            let outcome = check_merge_alert_freshness_for_task("cas-6883t", &data, repo.path());
            assert!(
                matches!(outcome, MergeAlertFreshness::NotApplicable),
                "unresolvable epic branch must not silently retract: {outcome:?}"
            );
        }
    }
}
