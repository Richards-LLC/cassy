//! Event detection for the Director
//!
//! Detects state changes in Cassy data by comparing snapshots.
//! Used to trigger auto-prompting and activity logging.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::ui::factory::director::data::{ActiveLeaseSummary, DirectorData, TaskSummary};
use cas_types::TaskStatus;
use chrono::{DateTime, Utc};

/// Minimum cadence between repeated supervisor forward-motion wakes.
///
/// The configurable silence threshold controls when an episode first becomes
/// actionable; once it does, repeated wakes stay capped at one per ten minutes
/// so a silent supervisor is nudged without producing a notification storm.
pub(crate) const SUPERVISOR_STALL_REFIRE_SECS: i64 = 10 * 60;

/// Concrete next action the supervisor owns while a focused epic is stalled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorActionableState {
    MergeBranches {
        /// `(task id, delivery branch, live tip)` in deterministic task order.
        branches: Vec<(String, String, String)>,
    },
    AssignReadyWork {
        task_ids: Vec<String>,
        idle_workers: Vec<String>,
    },
    AssembleGatePipeline {
        epic_id: String,
    },
}

impl SupervisorActionableState {
    /// Render an impact-first instruction suitable for the supervisor wake.
    pub(crate) fn next_step_text(&self) -> String {
        match self {
            Self::MergeBranches { branches } => {
                let rows = branches
                    .iter()
                    .map(|(task, branch, tip)| format!("- {task}: {branch} @ {tip}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "Drive to the exit: merge the ready delivery branch(es) into the focused epic now:\n{rows}"
                )
            }
            Self::AssignReadyWork {
                task_ids,
                idle_workers,
            } => format!(
                "Drive to the exit: assign ready/open task(s) {} to idle worker(s) {} now.",
                task_ids.join(", "),
                idle_workers.join(", ")
            ),
            Self::AssembleGatePipeline { epic_id } => format!(
                "Drive to the exit: all children of {epic_id} are terminal -> assemble the epic branch, run the integration gate, and queue the release/PR pipeline."
            ),
        }
    }
}

/// Persistable per-factory-session accounting for supervisor stalls.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SupervisorStallTracker {
    /// Completed time spent under the full detector predicate.
    #[serde(default)]
    pub actionable_idle_secs: u64,
    /// Start of the currently-active detector predicate, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actionable_idle_started_at: Option<DateTime<Utc>>,
    /// Last wake accepted for delivery in this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_wake_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SupervisorStallObservation {
    pub wake: Option<SupervisorActionableState>,
    pub actionable_idle_secs: u64,
}

impl SupervisorStallTracker {
    /// Advance one detector sample. The metric counts only while the complete
    /// predicate is true: actionable epic state, supervisor silence beyond the
    /// configured threshold, and no covering reminder.
    pub(crate) fn observe(
        &mut self,
        actionable: Option<SupervisorActionableState>,
        last_supervisor_mcp_call_at: Option<DateTime<Utc>>,
        covering_reminder: bool,
        now: DateTime<Utc>,
        stall_after_secs: u64,
    ) -> SupervisorStallObservation {
        let silent = last_supervisor_mcp_call_at
            .map(|last| (now - last).num_seconds() >= stall_after_secs as i64)
            .unwrap_or(false);
        let condition_true = actionable.is_some() && silent && !covering_reminder;

        if condition_true {
            self.actionable_idle_started_at.get_or_insert(now);
        } else if let Some(started_at) = self.actionable_idle_started_at.take() {
            self.actionable_idle_secs = self.actionable_idle_secs.saturating_add(
                (now - started_at).num_seconds().max(0) as u64,
            );
        }

        let current_secs = self.actionable_idle_secs.saturating_add(
            self.actionable_idle_started_at
                .map(|started_at| (now - started_at).num_seconds().max(0) as u64)
                .unwrap_or(0),
        );
        let refire_due = self
            .last_wake_at
            .map(|last| (now - last).num_seconds() >= SUPERVISOR_STALL_REFIRE_SECS)
            .unwrap_or(true);
        let wake = if condition_true && refire_due {
            self.last_wake_at = Some(now);
            actionable
        } else {
            None
        };

        SupervisorStallObservation {
            wake,
            actionable_idle_secs: current_secs,
        }
    }

    pub fn actionable_idle_minutes_at(&self, now: DateTime<Utc>) -> u64 {
        self.actionable_idle_secs
            .saturating_add(
                self.actionable_idle_started_at
                    .map(|started_at| (now - started_at).num_seconds().max(0) as u64)
                    .unwrap_or(0),
            )
            / 60
    }
}

/// Compute the highest-priority concrete next step for the focused epic.
///
/// Branch tips are resolved by the caller so tests stay pure and production
/// can consult the live repository immediately before constructing the wake.
pub(crate) fn supervisor_actionable_state(
    data: &DirectorData,
    focused_epic_id: Option<&str>,
    supervisor_name: &str,
    held_workers: &HashSet<String>,
    now: DateTime<Utc>,
    idle_after_secs: u64,
    mut resolve_branch_tip: impl FnMut(&str) -> Option<String>,
) -> Option<SupervisorActionableState> {
    let epic_id = focused_epic_id?;
    let epic_is_open = data.epic_tasks.iter().any(|epic| {
        epic.id == epic_id
            && matches!(
                epic.status,
                TaskStatus::Open | TaskStatus::InProgress | TaskStatus::Blocked
            )
    });
    if !epic_is_open {
        return None;
    }

    let mut mergeable = data
        .in_progress_tasks
        .iter()
        .filter(|task| {
            task.epic.as_deref() == Some(epic_id) && task.status == TaskStatus::AwaitingMerge
        })
        .filter_map(|task| {
            let assignee = task.assignee.as_deref()?;
            let worker = data
                .agent_id_to_name
                .get(assignee)
                .map(String::as_str)
                .unwrap_or(assignee);
            let branch = format!("factory/{worker}");
            let tip = resolve_branch_tip(&branch).unwrap_or_else(|| "tip-unresolved".to_string());
            Some((task.id.clone(), branch, tip))
        })
        .collect::<Vec<_>>();
    mergeable.sort_by(|left, right| left.0.cmp(&right.0));
    if !mergeable.is_empty() {
        return Some(SupervisorActionableState::MergeBranches {
            branches: mergeable,
        });
    }

    let active_children = data
        .ready_tasks
        .iter()
        .chain(data.in_progress_tasks.iter())
        .filter(|task| task.epic.as_deref() == Some(epic_id))
        .count();
    if active_children == 0
        && data.epic_closed_counts.get(epic_id).copied().unwrap_or(0) > 0
    {
        return Some(SupervisorActionableState::AssembleGatePipeline {
            epic_id: epic_id.to_string(),
        });
    }

    let mut task_ids = data
        .ready_tasks
        .iter()
        .filter(|task| {
            task.epic.as_deref() == Some(epic_id)
                && task.status == TaskStatus::Open
                && task.assignee.is_none()
        })
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    task_ids.sort();
    if task_ids.is_empty() {
        return None;
    }

    let assigned_workers = data
        .ready_tasks
        .iter()
        .chain(data.in_progress_tasks.iter())
        .filter_map(|task| task.assignee.as_deref())
        .flat_map(|assignee| {
            std::iter::once(assignee.to_string()).chain(
                data.agent_id_to_name
                    .get(assignee)
                    .cloned()
                    .into_iter(),
            )
        })
        .collect::<HashSet<_>>();
    let mut idle_workers = data
        .agents
        .iter()
        .filter(|agent| {
            agent.name != supervisor_name
                && agent.current_task.is_none()
                && !held_workers.contains(&agent.name)
                && !assigned_workers.contains(&agent.name)
                && !assigned_workers.contains(&agent.id)
        })
        .filter(|agent| {
            let baseline = agent
                .latest_activity
                .as_ref()
                .map(|(_, at)| *at)
                .unwrap_or(agent.registered_at)
                .max(agent.registered_at);
            (now - baseline).num_seconds() >= idle_after_secs as i64
        })
        .map(|agent| agent.name.clone())
        .collect::<Vec<_>>();
    idle_workers.sort();
    if idle_workers.is_empty() {
        return None;
    }

    Some(SupervisorActionableState::AssignReadyWork {
        task_ids,
        idle_workers,
    })
}

/// Debounce duration for events (don't emit same event within this window)
const DEBOUNCE_DURATION: Duration = Duration::from_secs(30);

/// Rate limit for WorkerIdle events — at most one per worker per 5 minutes.
/// Idle notifications are low-priority and flood the supervisor when multiple
/// workers idle simultaneously.
const IDLE_RATE_LIMIT: Duration = Duration::from_secs(300);

/// A worker whose `last_heartbeat` is within this many seconds of `now_utc` is
/// considered "recently alive" for the purposes of the idle gate (cas-4038).
/// CC agents heartbeat on every tool call, so a 60s window covers one full
/// turn without generating a false-idle notification.
pub(crate) const FRESH_HEARTBEAT_SECS: i64 = 60;

/// A worker whose `latest_activity` timestamp is within this many seconds of
/// `now_utc` is considered "recently active" (cas-4038). Combined with the
/// fresh-heartbeat gate: BOTH must be true to suppress a WorkerIdle tick.
/// 120s gives one comfortable "between tasks" turn window at the 2s refresh
/// rate without masking a genuinely stalled worker.
pub(crate) const RECENT_ACTIVITY_SECS: i64 = 120;

/// Scale the base stall threshold by a worker's configured reasoning effort
/// (cas-09d0). A high/xhigh-effort worker's read-and-think phase routinely
/// runs longer before producing a checkpoint-class event (file edit, commit,
/// subagent spawn) than a low/minimal-effort worker's — using one flat
/// threshold for every worker means the workers most likely to have a long,
/// legitimate silent-reasoning phase are also the ones most likely to trip a
/// false stall. `None` (effort unknown/unset) scales as 1.0x, the pre-cas-09d0
/// behavior.
pub(crate) fn effective_stall_threshold_secs(
    base_secs: u64,
    effort: Option<cas_mux::Effort>,
) -> u64 {
    let multiplier = match effort {
        None | Some(cas_mux::Effort::Minimal) | Some(cas_mux::Effort::Low) => 1.0,
        Some(cas_mux::Effort::Medium) => 1.5,
        Some(cas_mux::Effort::High) => 2.0,
        Some(cas_mux::Effort::XHigh) => 3.0,
    };
    ((base_secs as f64) * multiplier).round() as u64
}

/// Number of consecutive refresh ticks an agent must appear idle before
/// WorkerIdle is emitted.
///
/// The daemon's `refresh_interval` is 2s (see
/// `cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs`), so this gives a
/// sustained-idle window of roughly `2 * refresh_interval = 4s`. The window
/// is long enough to absorb normal close-X → start-Y transitions (where a
/// worker finishes one task and immediately claims the next) without
/// emitting a spurious "worker idle" prompt to the supervisor, and short
/// enough that genuinely idle workers are still surfaced quickly.
///
/// Before this threshold existed, a single refresh landing inside the
/// sub-second gap between a worker closing task X and starting task Y would
/// emit `WorkerIdle` immediately, producing apparent out-of-order delivery
/// ("idle notification arrived before the claim") even though the worker
/// was already working. See task cas-f9e8.
const IDLE_CONSECUTIVE_TICKS: u32 = 2;

/// A newly registered worker is expected to be taskless while the supervisor
/// completes the spawn -> assign round trip. Do not begin the normal idle
/// debounce inside this window; otherwise two fast refreshes can enqueue an
/// alert before the assignment write lands.
const SPAWN_ASSIGN_GRACE_SECS: i64 = 10;

/// Events detected from Cassy state changes
#[derive(Debug, Clone)]
pub enum DirectorEvent {
    /// The focused epic has concrete supervisor-owned work while the
    /// supervisor has made no Cassy MCP call for the configured interval.
    SupervisorStalled {
        next_step: SupervisorActionableState,
        occurrence: String,
        actionable_idle_secs: u64,
    },
    /// A task was assigned to a worker
    TaskAssigned {
        task_id: String,
        task_title: String,
        worker: String,
    },
    /// A task was completed
    TaskCompleted {
        task_id: String,
        task_title: String,
        worker: String,
    },
    /// A task was blocked
    TaskBlocked {
        task_id: String,
        task_title: String,
        worker: String,
    },
    /// A worker became idle. `active_task` is present when the worker appears
    /// idle even though its active lease still points at an InProgress task.
    WorkerIdle {
        worker: String,
        active_task: Option<ActiveLeaseSummary>,
    },
    /// A worker has an in-progress task and a fresh heartbeat, but no
    /// observable activity (file edit, commit, subagent event, ...) for
    /// longer than the configured stall threshold (cas-9829). Heartbeat
    /// alone cannot distinguish "healthy" from "printed a plan and
    /// stopped" — this is the activity-based signal that fills that gap.
    ///
    /// `escalate = false` on first detection in a stall streak: the
    /// director auto-nudges the worker once (re-injects the task prompt)
    /// instead of paging the supervisor immediately, since a single
    /// re-poke often unsticks a stalled agent. `escalate = true` once the
    /// worker is still stalled after that nudge — the supervisor is
    /// notified at that point.
    WorkerStalled {
        worker: String,
        task_id: String,
        elapsed_secs: u64,
        escalate: bool,
    },
    /// A new agent registered
    AgentRegistered {
        agent_id: String,
        agent_name: String,
    },
    /// An epic was started (detected by new epic-type task)
    EpicStarted { epic_id: String, epic_title: String },
    /// All tasks in an epic are complete
    EpicCompleted { epic_id: String },
    /// All subtasks of an epic are closed but the epic itself is still open
    EpicAllSubtasksClosed { epic_id: String, epic_title: String },
}

impl DirectorEvent {
    /// Get the worker/agent this event targets (for prompt injection)
    pub fn target(&self) -> Option<&str> {
        match self {
            Self::SupervisorStalled { .. } => None,
            Self::TaskAssigned { worker, .. } => Some(worker),
            Self::TaskCompleted { worker, .. } => Some(worker),
            Self::TaskBlocked { worker, .. } => Some(worker),
            Self::WorkerIdle { worker, .. } => Some(worker),
            Self::WorkerStalled { worker, .. } => Some(worker),
            Self::AgentRegistered { agent_name, .. } => Some(agent_name),
            Self::EpicStarted { .. } => None, // Broadcast or supervisor
            Self::EpicCompleted { .. } => None,
            Self::EpicAllSubtasksClosed { .. } => None, // Targets supervisor
        }
    }

    /// Get a description of the event for logging
    pub fn description(&self) -> String {
        match self {
            Self::SupervisorStalled {
                next_step,
                actionable_idle_secs,
                ..
            } => format!(
                "Supervisor actionable-idle for {}m. {}",
                actionable_idle_secs / 60,
                next_step.next_step_text()
            ),
            Self::TaskAssigned {
                task_id,
                worker,
                task_title,
            } => {
                format!("{worker} assigned task {task_id} ({task_title})")
            }
            Self::TaskCompleted {
                task_id,
                worker,
                task_title,
            } => {
                format!("{worker} completed task {task_id} ({task_title})")
            }
            Self::TaskBlocked {
                task_id,
                worker,
                task_title,
            } => {
                format!("{worker} blocked on task {task_id} ({task_title})")
            }
            Self::WorkerIdle {
                worker,
                active_task: Some(task),
            } => {
                if let Some(reason) = task.close_rejected_reason.as_deref() {
                    format!(
                        "{worker} is idle — task {} {}, close rejected ({reason})",
                        task.task_id, task.task_status
                    )
                } else {
                    format!(
                        "{worker} is idle — task {} {}",
                        task.task_id, task.task_status
                    )
                }
            }
            Self::WorkerIdle { worker, .. } => {
                format!("{worker} is idle")
            }
            Self::WorkerStalled {
                worker,
                task_id,
                elapsed_secs,
                escalate,
            } => {
                if *escalate {
                    format!(
                        "{worker} still stalled on {task_id} after {elapsed_secs}s (nudged, escalating to supervisor)"
                    )
                } else {
                    format!("{worker} stalled on {task_id}: no activity for {elapsed_secs}s")
                }
            }
            Self::AgentRegistered { agent_name, .. } => {
                format!("{agent_name} registered")
            }
            Self::EpicStarted {
                epic_id,
                epic_title,
            } => {
                format!("Epic {epic_id} started: {epic_title}")
            }
            Self::EpicCompleted { epic_id } => {
                format!("Epic {epic_id} completed")
            }
            Self::EpicAllSubtasksClosed {
                epic_id,
                epic_title,
            } => {
                format!(
                    "All subtasks of epic '{epic_title}' ({epic_id}) are closed — ready to close epic"
                )
            }
        }
    }

    /// Get a unique key for debouncing this event
    ///
    /// Events with the same key are considered duplicates within the debounce window.
    pub fn debounce_key(&self) -> String {
        match self {
            Self::SupervisorStalled { occurrence, .. } => {
                format!("supervisor_stalled:{occurrence}")
            }
            Self::TaskAssigned {
                task_id, worker, ..
            } => {
                format!("assigned:{task_id}:{worker}")
            }
            Self::TaskCompleted {
                task_id, worker, ..
            } => {
                format!("completed:{task_id}:{worker}")
            }
            Self::TaskBlocked {
                task_id, worker, ..
            } => {
                format!("blocked:{task_id}:{worker}")
            }
            Self::WorkerIdle { worker, .. } => {
                format!("idle:{worker}")
            }
            Self::WorkerStalled {
                worker, escalate, ..
            } => {
                format!("stalled:{worker}:{escalate}")
            }
            Self::AgentRegistered { agent_id, .. } => {
                format!("registered:{agent_id}")
            }
            Self::EpicStarted { epic_id, .. } => {
                format!("epic_started:{epic_id}")
            }
            Self::EpicCompleted { epic_id } => {
                format!("epic_completed:{epic_id}")
            }
            Self::EpicAllSubtasksClosed { epic_id, .. } => {
                format!("epic_all_subtasks_closed:{epic_id}")
            }
        }
    }

    /// Get the event type as a string (for recording export)
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::SupervisorStalled { .. } => "supervisor_stalled",
            Self::TaskAssigned { .. } => "task_assigned",
            Self::TaskCompleted { .. } => "task_completed",
            Self::TaskBlocked { .. } => "task_blocked",
            Self::WorkerIdle { .. } => "worker_idle",
            Self::WorkerStalled { .. } => "worker_stalled",
            Self::AgentRegistered { .. } => "agent_registered",
            Self::EpicStarted { .. } => "epic_started",
            Self::EpicCompleted { .. } => "epic_completed",
            Self::EpicAllSubtasksClosed { .. } => "epic_all_subtasks_closed",
        }
    }

    /// Convert event data to JSON (for recording export)
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::SupervisorStalled {
                next_step,
                occurrence,
                actionable_idle_secs,
            } => serde_json::json!({
                "next_step": next_step.next_step_text(),
                "occurrence": occurrence,
                "actionable_idle_secs": actionable_idle_secs,
            }),
            Self::TaskAssigned {
                task_id,
                task_title,
                worker,
            } => serde_json::json!({
                "task_id": task_id,
                "task_title": task_title,
                "worker": worker,
            }),
            Self::TaskCompleted {
                task_id,
                task_title,
                worker,
            } => serde_json::json!({
                "task_id": task_id,
                "task_title": task_title,
                "worker": worker,
            }),
            Self::TaskBlocked {
                task_id,
                task_title,
                worker,
            } => serde_json::json!({
                "task_id": task_id,
                "task_title": task_title,
                "worker": worker,
            }),
            Self::WorkerIdle {
                worker,
                active_task,
            } => {
                let mut value = serde_json::json!({
                    "worker": worker,
                });
                if let Some(task) = active_task {
                    value["task_id"] = serde_json::Value::String(task.task_id.clone());
                    value["task_title"] = serde_json::Value::String(task.task_title.clone());
                    value["task_state"] = serde_json::Value::String(task.task_status.to_string());
                    if let Some(reason) = task.close_rejected_reason.as_deref() {
                        value["close_rejected"] = serde_json::Value::Bool(true);
                        value["close_rejected_reason"] =
                            serde_json::Value::String(reason.to_string());
                    }
                }
                value
            }
            Self::WorkerStalled {
                worker,
                task_id,
                elapsed_secs,
                escalate,
            } => serde_json::json!({
                "worker": worker,
                "task_id": task_id,
                "elapsed_secs": elapsed_secs,
                "escalate": escalate,
            }),
            Self::AgentRegistered {
                agent_id,
                agent_name,
            } => serde_json::json!({
                "agent_id": agent_id,
                "agent_name": agent_name,
            }),
            Self::EpicStarted {
                epic_id,
                epic_title,
            } => serde_json::json!({
                "epic_id": epic_id,
                "epic_title": epic_title,
            }),
            Self::EpicCompleted { epic_id } => serde_json::json!({
                "epic_id": epic_id,
            }),
            Self::EpicAllSubtasksClosed {
                epic_id,
                epic_title,
            } => serde_json::json!({
                "epic_id": epic_id,
                "epic_title": epic_title,
            }),
        }
    }
}

/// State snapshot for comparison
#[derive(Debug, Clone, Default)]
struct DirectorState {
    /// Map of task_id -> (status, assignee)
    tasks: HashMap<String, (TaskStatus, Option<String>)>,
    /// Map of task_id -> title (for lookup when tasks disappear from active sets)
    task_titles: HashMap<String, String>,
    /// Set of active agent IDs
    active_agents: HashSet<String>,
    /// Map of epic_id -> (status, has_branch)
    epic_statuses: HashMap<String, (TaskStatus, bool)>,
    /// Map of epic_id -> count of active (non-closed) subtasks
    epic_active_subtask_counts: HashMap<String, usize>,
}

impl DirectorState {
    fn from_data(data: &DirectorData) -> Self {
        let mut tasks = HashMap::new();
        let mut task_titles = HashMap::new();

        // Add ready tasks
        for task in &data.ready_tasks {
            tasks.insert(task.id.clone(), (task.status, task.assignee.clone()));
            task_titles.insert(task.id.clone(), task.title.clone());
        }

        // Add in-progress tasks
        for task in &data.in_progress_tasks {
            tasks.insert(task.id.clone(), (task.status, task.assignee.clone()));
            task_titles.insert(task.id.clone(), task.title.clone());
        }

        let active_agents: HashSet<String> = data.agents.iter().map(|a| a.id.clone()).collect();

        // Track epic statuses and branch presence
        let epic_statuses: HashMap<String, (TaskStatus, bool)> = data
            .epic_tasks
            .iter()
            .map(|e| (e.id.clone(), (e.status, e.branch.is_some())))
            .collect();

        // Count active (non-closed) subtasks per epic.
        // Tasks in ready_tasks or in_progress_tasks are active by definition.
        let mut epic_active_subtask_counts: HashMap<String, usize> = HashMap::new();
        for task in data.ready_tasks.iter().chain(data.in_progress_tasks.iter()) {
            if let Some(ref epic_id) = task.epic {
                *epic_active_subtask_counts
                    .entry(epic_id.clone())
                    .or_insert(0) += 1;
            }
        }

        Self {
            tasks,
            task_titles,
            active_agents,
            epic_statuses,
            epic_active_subtask_counts,
        }
    }
}

/// Detects events by comparing Cassy state snapshots
pub struct DirectorEventDetector {
    /// Previous state snapshot
    last_state: DirectorState,
    /// Factory worker names (for filtering)
    worker_names: Vec<String>,
    /// Supervisor name
    supervisor_name: String,
    /// Last prompt times for debouncing (event key -> instant)
    last_prompt_times: HashMap<String, Instant>,
    /// Workers that have been removed (shutdown/crashed) — suppress their events
    removed_workers: HashSet<String>,
    /// Consecutive refresh ticks each factory agent has appeared idle.
    /// Used with `IDLE_CONSECUTIVE_TICKS` to debounce `WorkerIdle` so that
    /// sub-second close-X → start-Y transitions do not generate spurious
    /// idle prompts. Keyed by agent id.
    consecutive_idle_ticks: HashMap<String, u32>,
    /// Agents for whom `WorkerIdle` has already been emitted in the current
    /// idle streak. Cleared once the agent picks up a task again, so a fresh
    /// idle streak can trigger another emission (subject to `IDLE_RATE_LIMIT`
    /// in `debounce_events`). Keyed by agent id.
    idle_already_emitted: HashSet<String>,
    /// UTC baseline for the worker's current task-less streak, keyed by
    /// resolved worker name. Kept across prompt delivery so send-time
    /// revalidation can recognize supervisor contact newer than the streak.
    idle_transition_at: HashMap<String, DateTime<chrono::Utc>>,
    /// Workers whose current idle transition has already been handled by a
    /// supervisor message. This survives queue drainage and clears only when
    /// the worker resumes task/activity or leaves the session.
    idle_handled_by_supervisor: HashSet<String>,
    /// Tasks for which `TaskCompleted` has already been announced this session.
    ///
    /// **Never cleared on active-set reappearance.** When a task oscillates
    /// (lease expires → temporarily disappears → lease re-acquired → reappears)
    /// the reappearance is NOT a new assignment; it is the same task continuing.
    /// Clearing the guard on reappearance would cause a re-emission on every
    /// subsequent oscillation cycle — exactly the ~30s re-fire bug (cas-55dc).
    ///
    /// Keyed by task_id.
    task_completed_announced: HashSet<String>,
    /// Assignment pairs for which `TaskAssigned` has already been announced.
    ///
    /// Same never-clear-on-reappearance policy as `task_completed_announced`:
    /// if a task oscillates out and back in with the same assignee the
    /// assignment was already dispatched and must not re-fire.
    ///
    /// Key is `"{task_id}:{assignee_id}"`. A genuine reassignment to a
    /// *different* worker produces a new key and is therefore not suppressed.
    task_assigned_announced: HashSet<String>,
    /// Workers that have already received the one-shot stall auto-nudge in
    /// the current stall streak (cas-9829). Cleared once the worker's
    /// activity resumes (elapsed drops back under the threshold) or the
    /// worker leaves the active set, so a fresh stall streak nudges again.
    stall_nudged: HashSet<String>,
    /// Workers for whom the stall has already been escalated to the
    /// supervisor in the current streak. Cleared alongside `stall_nudged`.
    stall_escalated: HashSet<String>,
    /// Seconds of no observable activity (with a fresh heartbeat and an
    /// in-progress task) before a worker is flagged `WorkerStalled`.
    /// Defaults to `cas_factory::DEFAULT_STALL_THRESHOLD_SECS`; overridden
    /// via [`Self::set_stall_threshold_secs`] from `.cas/config.toml`
    /// `[factory] stall_threshold_secs`.
    stall_threshold_secs: u64,
    /// (cas-728b) UTC timestamp each agent's CURRENT `task_id` was first
    /// observed by the director, keyed by agent id: `(task_id, first_seen)`.
    /// Overwritten whenever `current_task` changes to a different id (or is
    /// seen for the first time) — never refreshed while the same task
    /// continues, so this is a one-shot grace baseline, not a sliding
    /// window.
    ///
    /// Used to treat the task-start transition itself as activity: a
    /// worker's first read/investigation turn on a dense task can run 5+
    /// minutes with zero file-edit/commit/subagent checkpoints (the exact
    /// false-positive class from the 2026-07-07 repros — two workers
    /// flagged stalled ~5m after task start while still read-only
    /// investigating). The stall predicate uses
    /// `max(latest_activity_ts, task_start_ts)` as its baseline instead of
    /// `latest_activity_ts` alone, so a stale-or-absent checkpoint
    /// timestamp from before this task started can't fire a false stall in
    /// the grace window right after start.
    task_start_observed: HashMap<String, (String, DateTime<chrono::Utc>)>,
    /// (cas-728b) Cassy root directory, set via [`Self::set_cas_root`]. When
    /// present, a checkpoint-age stall candidate is additionally confirmed
    /// against transcript mtime — the same liveness signal
    /// `cas factory is-wedged` reads (cas-4513) — before firing/escalating:
    /// a transcript written within the harness-specific
    /// [`activity_fresh_window`](crate::cli::factory::wedged::activity_fresh_window)
    /// (Codex 5m; Claude/Grok 60s; cas-ab80) means the worker is actively
    /// producing output even though no *checkpoint-class* event has landed
    /// yet, so the checkpoint-age-only signal alone is unreliable. `None`
    /// (e.g. in tests, or before the daemon has resolved a cas root) skips
    /// the confirmation step and falls back to the checkpoint-only
    /// predicate — preserves old behavior rather than going silent when the
    /// stronger signal is unavailable.
    cas_root: Option<std::path::PathBuf>,
    /// (cas-09d0) Test-only override for transcript-age resolution, keyed by
    /// resolved worker name. When `Some`, `transcript_confirms_stall` uses
    /// this map directly instead of doing real `/proc` + filesystem I/O via
    /// `cas_root` + `resolve_worker` — lets tests exercise the
    /// transcript-confirms-stall suppression path (AC a: "transcript mtime
    /// within the harness fresh window suppresses stall alert") without a
    /// full `SqliteAgentStore` + real transcript file round-trip. `None`
    /// (production default) leaves the existing `cas_root`-based path
    /// untouched.
    transcript_age_override: Option<HashMap<String, Option<Duration>>>,
    /// (cas-ab80) Test-only per-worker transcript freshness window used with
    /// `transcript_age_override`. Production resolves the window via
    /// `activity_fresh_window(resolved.cli)` (same helper as is-wedged).
    /// When a worker is absent from this map under age override, defaults to
    /// `TRANSCRIPT_FRESH_WINDOW` (Claude/Grok 60s).
    transcript_window_override: Option<HashMap<String, Duration>>,
    /// (cas-7e85) Test-only override for the "has an in-flight tool call"
    /// signal, keyed by resolved worker name, used alongside
    /// `transcript_age_override` so tests can exercise the suppression path
    /// without a real transcript file. `None` per-worker (missing from the
    /// map) means "no in-flight call" — production resolves this via
    /// `wedged::transcript_has_in_flight_tool_call`.
    transcript_in_flight_override: Option<HashMap<String, bool>>,
    /// (cas-09d0) Workers explicitly put on hold by the supervisor — a
    /// first-class primitive for "deliberately paused, not idle-needing-work"
    /// that doesn't require a task-status transition (unlike `AwaitingMerge`,
    /// which already gets informational-only `WorkerIdle` framing via
    /// `active_lease`, see `prompts.rs`). While a worker's name is in this
    /// set, the idle-tick counter never accumulates for them and no
    /// `WorkerIdle` is emitted, mirroring the existing `pending_messages > 0`
    /// short-circuit below. Set/cleared via [`Self::mark_worker_hold`] /
    /// [`Self::clear_worker_hold`].
    held_workers: HashSet<String>,
    /// (cas-6883) Evidence `(unmerged_count, epic_sha)` last actually sent
    /// for a MERGE REQUIRED alert, keyed by `(task_id, factory_branch)`.
    /// `IDLE_RATE_LIMIT` above only floors re-fire *frequency* (once per 5
    /// minutes); this floors re-fire *content* — AC3 requires the alert not
    /// re-emit for the same (task, branch) pair without an intervening
    /// state change, not merely "not more than once every 5 minutes" (a
    /// parked task can easily stay parked for 5+ minutes, and did in the
    /// reported six-stale-alerts session). See
    /// [`Self::merge_alert_should_emit`].
    merge_alert_last_evidence: HashMap<(String, String), (u32, String)>,
}

impl DirectorEventDetector {
    /// Create a new event detector
    pub fn new(worker_names: Vec<String>, supervisor_name: String) -> Self {
        Self {
            last_state: DirectorState::default(),
            worker_names,
            supervisor_name,
            last_prompt_times: HashMap::new(),
            removed_workers: HashSet::new(),
            consecutive_idle_ticks: HashMap::new(),
            idle_already_emitted: HashSet::new(),
            idle_transition_at: HashMap::new(),
            idle_handled_by_supervisor: HashSet::new(),
            task_completed_announced: HashSet::new(),
            task_assigned_announced: HashSet::new(),
            stall_nudged: HashSet::new(),
            stall_escalated: HashSet::new(),
            stall_threshold_secs: cas_factory::DEFAULT_STALL_THRESHOLD_SECS,
            task_start_observed: HashMap::new(),
            cas_root: None,
            transcript_age_override: None,
            transcript_window_override: None,
            transcript_in_flight_override: None,
            held_workers: HashSet::new(),
            merge_alert_last_evidence: HashMap::new(),
        }
    }

    /// (cas-6883) Whether a MERGE REQUIRED alert for `(task_id,
    /// factory_branch)` carrying `(unmerged_count, epic_sha)` should
    /// actually be sent — `false` when this exact evidence was the last
    /// thing sent for this pair (AC3: no re-emit without an intervening
    /// state change). Records the evidence as "last sent" as a side effect
    /// whenever it returns `true`, so callers must only invoke this once
    /// per candidate emission (not use it as a read-only peek).
    pub(crate) fn merge_alert_should_emit(
        &mut self,
        task_id: &str,
        factory_branch: &str,
        unmerged_count: u32,
        epic_sha: &str,
    ) -> bool {
        let key = (task_id.to_string(), factory_branch.to_string());
        let evidence = (unmerged_count, epic_sha.to_string());
        if self.merge_alert_last_evidence.get(&key) == Some(&evidence) {
            return false;
        }
        self.merge_alert_last_evidence.insert(key, evidence);
        true
    }

    /// (cas-09d0) Put a worker on hold: suppress `WorkerIdle` for them
    /// entirely until [`Self::clear_worker_hold`] is called, regardless of
    /// how long they appear idle. Use for a deliberate supervisor pause that
    /// doesn't correspond to a task-status transition (e.g. "stand by while
    /// I sort out the merge base") — the task-level equivalent
    /// (`AwaitingMerge`) is already handled via `active_lease` informational
    /// framing in `prompts.rs` and doesn't need this.
    pub fn mark_worker_hold(&mut self, worker_name: &str) {
        self.held_workers.insert(worker_name.to_string());
    }

    /// Release a worker from hold — idle detection resumes normally on the
    /// next tick.
    pub fn clear_worker_hold(&mut self, worker_name: &str) {
        self.held_workers.remove(worker_name);
    }

    /// Whether `worker_name` is currently held (test/inspection helper).
    pub fn is_worker_held(&self, worker_name: &str) -> bool {
        self.held_workers.contains(worker_name)
    }

    /// (cas-09d0) Test-only seam: inject synthetic transcript ages instead of
    /// resolving them via real `/proc` + filesystem I/O. See
    /// `transcript_age_override` field doc.
    #[cfg(test)]
    pub(crate) fn set_transcript_age_override(&mut self, ages: HashMap<String, Option<Duration>>) {
        self.transcript_age_override = Some(ages);
    }

    /// (cas-ab80) Test-only seam: inject harness-specific transcript fresh
    /// windows alongside `set_transcript_age_override`. Production uses
    /// `activity_fresh_window(cli)` from the resolved worker.
    #[cfg(test)]
    pub(crate) fn set_transcript_window_override(&mut self, windows: HashMap<String, Duration>) {
        self.transcript_window_override = Some(windows);
    }

    /// (cas-7e85) Test-only seam: inject a synthetic "in-flight tool call"
    /// signal alongside `set_transcript_age_override`, so tests can prove
    /// an outstanding call suppresses the stall alert regardless of how
    /// stale the (overridden) transcript age is — without needing a real
    /// JSONL fixture file.
    #[cfg(test)]
    pub(crate) fn set_transcript_in_flight_override(&mut self, in_flight: HashMap<String, bool>) {
        self.transcript_in_flight_override = Some(in_flight);
    }

    /// Initialize with current state (call after first data load)
    pub fn initialize(&mut self, data: &DirectorData) {
        self.last_state = DirectorState::from_data(data);
    }

    /// Override the stall-detection threshold (default
    /// `cas_factory::DEFAULT_STALL_THRESHOLD_SECS`). Call once after
    /// construction, before the first `detect_changes`/`detect_changes_at`,
    /// with the value resolved from `.cas/config.toml`
    /// `[factory] stall_threshold_secs` (cas-9829).
    pub fn set_stall_threshold_secs(&mut self, secs: u64) {
        self.stall_threshold_secs = secs;
    }

    /// (cas-728b) Set the Cassy root directory so stall-candidate confirmation
    /// can consult transcript mtime the same way `cas factory is-wedged`
    /// does. Call once after construction. Leaving this unset skips the
    /// confirmation step (checkpoint-age-only predicate, the pre-cas-728b
    /// behavior) — safe default for tests and any caller that hasn't
    /// resolved a cas root yet.
    pub fn set_cas_root(&mut self, cas_root: std::path::PathBuf) {
        self.cas_root = Some(cas_root);
    }

    /// (cas-728b) Returns `true` if a checkpoint-age stall candidate should
    /// still be treated as stalled after consulting transcript mtime —
    /// `false` means the transcript was written to recently enough that the
    /// worker is evidently still producing output, and the checkpoint-age
    /// signal alone was a false positive.
    ///
    /// Freshness uses the same harness-specific window as
    /// `cas factory is-wedged` via
    /// [`activity_fresh_window`](crate::cli::factory::wedged::activity_fresh_window)
    /// (cas-ab80): Codex 5m, Claude/Grok 60s.
    ///
    /// Defaults to `true` (proceed with the checkpoint-only verdict, the
    /// pre-cas-728b behavior) whenever the stronger signal is unavailable:
    /// no `cas_root` set, the worker isn't resolvable in the agent store,
    /// or it has no resolvable transcript path. This mirrors the existing
    /// "when either signal is absent, stay inactive rather than guessing"
    /// philosophy already used for the heartbeat gate above — but inverted,
    /// since here the checkpoint-age signal is already the fallback and we
    /// must not let a *missing* confirmation signal silently suppress every
    /// stall alert.
    fn transcript_confirms_stall(&self, worker_name: &str) -> bool {
        if let Some(overrides) = &self.transcript_age_override {
            let age = overrides.get(worker_name).copied().flatten();
            let window = self
                .transcript_window_override
                .as_ref()
                .and_then(|m| m.get(worker_name).copied())
                .unwrap_or(crate::cli::factory::wedged::TRANSCRIPT_FRESH_WINDOW);
            // cas-7e85: missing from the override map means "no in-flight
            // call", matching production's fail-safe default.
            let in_flight = self
                .transcript_in_flight_override
                .as_ref()
                .and_then(|m| m.get(worker_name).copied())
                .unwrap_or(false);
            return transcript_confirms_stall_for_age(age, window, in_flight);
        }
        let Some(cas_root) = &self.cas_root else {
            return true;
        };
        let Ok(resolved) = crate::cli::factory::wedged::resolve_worker(cas_root, worker_name)
        else {
            return true;
        };
        let background_processes = crate::cli::factory::wedged::find_worker_pid(
            &crate::cli::factory::wedged::RealProcessTable,
            &resolved.name,
        )
        .or(resolved.pid)
        .map(crate::cli::factory::wedged::background_processes_for)
        .unwrap_or(crate::cli::factory::wedged::BackgroundProcessState::Unavailable);
        if background_processes.is_active() {
            transcript_confirms_stall_for_path_with_background(
                resolved.transcript_path.as_deref(),
                resolved.cli,
                &background_processes,
            )
        } else {
            transcript_confirms_stall_for_path(resolved.transcript_path.as_deref(), resolved.cli)
        }
    }

    /// Add a worker to the tracked list (call when spawning workers dynamically)
    pub fn add_worker(&mut self, name: String) {
        // cas-c790: guard against the supervisor's name being silently added to
        // the worker list on resume/reconnect paths, which would cause
        // is_worker_agent_name to return true for the lead — leaking WorkerIdle
        // events for the supervisor (recurrence of cas-b67d).
        if name == self.supervisor_name {
            return;
        }
        if !self.worker_names.contains(&name) {
            self.worker_names.push(name);
        }
    }

    /// Start of a worker's currently-observed idle streak, for the delivery
    /// race recheck performed after event detection.
    pub fn worker_idle_since(&self, worker_name: &str) -> Option<DateTime<chrono::Utc>> {
        self.idle_transition_at.get(worker_name).copied()
    }

    /// Remove a worker from the tracked list (call when shutting down workers)
    pub fn remove_worker(&mut self, name: &str) {
        self.worker_names.retain(|n| n != name);
        self.held_workers.remove(name);
        self.removed_workers.insert(name.to_string());
    }

    /// Detect changes between the last state and new data.
    ///
    /// Thin shim: captures `Instant::now()` and `Utc::now()` and delegates to
    /// [`detect_changes_at`]. Production callers use this; tests that need to
    /// isolate the state-guard from the 30s debounce window or the heartbeat
    /// freshness gate call `detect_changes_at` directly with synthetic clocks.
    pub fn detect_changes(
        &mut self,
        data: &DirectorData,
        current_epic_id: Option<&str>,
    ) -> Vec<DirectorEvent> {
        self.detect_changes_at(data, current_epic_id, Instant::now(), chrono::Utc::now())
    }

    /// Core implementation of change detection with injectable clocks.
    ///
    /// Returns a list of detected events. Call after each refresh.
    ///
    /// `now` — `Instant` used for debounce bookkeeping (`last_prompt_times`).
    /// Pass `Instant::now()` in production; inject a synthetic value in tests
    /// to isolate state-guards from the 30s `DEBOUNCE_DURATION` window.
    ///
    /// `now_utc` — `DateTime<Utc>` used for heartbeat / activity freshness
    /// comparisons. Pass `Utc::now()` in production; inject a synthetic value
    /// in tests to exercise the `FRESH_HEARTBEAT_SECS` / `RECENT_ACTIVITY_SECS`
    /// gates without actually sleeping.
    ///
    /// `current_epic_id` is the factory app's currently-tracked epic (pass
    /// `None` at init time before any epic has been resolved). When `Some`,
    /// `EpicStarted` for an Open-with-branch epic is only emitted if the
    /// candidate is **strictly better** than the active epic under the shared
    /// subtask-count heuristic (see [`pick_best_open_branch_epic`]). This
    /// prevents a fresh zero-subtask Open-with-branch epic from overwriting
    /// the active `epic_state` mid-session (see task cas-4181).
    /// `InProgress` epic transitions still emit unconditionally.
    pub fn detect_changes_at(
        &mut self,
        data: &DirectorData,
        current_epic_id: Option<&str>,
        now: Instant,
        now_utc: DateTime<chrono::Utc>,
    ) -> Vec<DirectorEvent> {
        let new_state = DirectorState::from_data(data);
        let mut events = Vec::new();

        // Build lookup maps for task info
        let task_info: HashMap<&str, &TaskSummary> = data
            .ready_tasks
            .iter()
            .chain(data.in_progress_tasks.iter())
            .map(|t| (t.id.as_str(), t))
            .collect();

        // Detect task assignments (task now has assignee that it didn't before).
        //
        // Terminal-status guard (cas-177f): only emit `TaskAssigned` when the
        // new status is actionable. Closed and Blocked tasks must never
        // generate dispatch prompts, even if they somehow leak into
        // `new_state.tasks` via a data-loading bug or future refactor. This
        // also supersedes the older
        // `bugfix_director_dispatches_blocked_tasks` memory — the `ready_tasks`
        // bucket in `crates/cas-factory/src/director.rs` still conflates
        // `Open | Blocked`, so without this guard blocked assignments would
        // still be dispatched.
        for (task_id, (new_status, new_assignee)) in &new_state.tasks {
            if let Some(assignee) = new_assignee {
                let dispatchable = matches!(new_status, TaskStatus::Open | TaskStatus::InProgress);

                // Check if this is a new assignment
                let was_assigned = self
                    .last_state
                    .tasks
                    .get(task_id)
                    .map(|(_, old_assignee)| old_assignee.as_ref() == Some(assignee))
                    .unwrap_or(false);

                if dispatchable && !was_assigned && self.is_factory_worker(assignee, data) {
                    // State-guard (cas-55dc): suppress re-emission if this
                    // (task, assignee) pair was already announced. Oscillation
                    // (lease churn causes the task to temporarily leave and
                    // re-enter active sets with the same assignee) must not
                    // re-fire TaskAssigned. A genuine reassignment to a
                    // *different* worker produces a different key and is not
                    // suppressed.
                    let announced_key = format!("{task_id}:{assignee}");
                    if !self.task_assigned_announced.contains(&announced_key) {
                        self.task_assigned_announced.insert(announced_key);
                        let task_title = task_info
                            .get(task_id.as_str())
                            .map(|t| t.title.clone())
                            .unwrap_or_default();

                        events.push(DirectorEvent::TaskAssigned {
                            task_id: task_id.clone(),
                            task_title,
                            worker: self.resolve_agent_name(assignee, data),
                        });
                    }
                }
            }

            // Detect task blocked
            if *new_status == TaskStatus::Blocked {
                let was_blocked = self
                    .last_state
                    .tasks
                    .get(task_id)
                    .map(|(old_status, _)| *old_status == TaskStatus::Blocked)
                    .unwrap_or(false);

                if !was_blocked {
                    if let Some(assignee) = new_assignee {
                        if self.is_factory_worker(assignee, data) {
                            let task_title = task_info
                                .get(task_id.as_str())
                                .map(|t| t.title.clone())
                                .unwrap_or_default();

                            events.push(DirectorEvent::TaskBlocked {
                                task_id: task_id.clone(),
                                task_title,
                                worker: self.resolve_agent_name(assignee, data),
                            });
                        }
                    }
                }
            }
        }

        // Detect task completions (task disappeared from active sets).
        //
        // State-guard (cas-55dc): `task_completed_announced` is a per-session
        // HashSet that records every task_id for which TaskCompleted has been
        // emitted. The guard is NEVER cleared on active-set reappearance because
        // reappearance is the oscillation we are defending against: a task whose
        // lease expires and then is re-acquired temporarily disappears from and
        // reappears in the active sets, and without this guard every subsequent
        // disappearance would re-fire TaskCompleted (observed at ~30-second
        // intervals, the DEBOUNCE_DURATION). By recording the announcement at the
        // HashSet level (not the debounce map), the guard remains in force across
        // the debounce window and indefinitely thereafter.
        //
        // Genuine completions are not suppressed: the first time a task_id
        // disappears while InProgress the announcement fires; subsequent
        // disappearances for the same ID are no-ops.
        let completed_task_ids: Vec<(String, String, String)> = self
            .last_state
            .tasks
            .iter()
            .filter_map(|(task_id, (old_status, old_assignee))| {
                let removed_from_active_sets = !new_state.tasks.contains_key(task_id);
                if removed_from_active_sets
                    && *old_status == TaskStatus::InProgress
                    && !self.task_completed_announced.contains(task_id)
                {
                    if let Some(assignee) = old_assignee {
                        if self.is_factory_agent(assignee, data) {
                            let title = self
                                .last_state
                                .task_titles
                                .get(task_id)
                                .cloned()
                                .unwrap_or_default();
                            let worker = self.resolve_agent_name(assignee, data);
                            return Some((task_id.clone(), title, worker));
                        }
                    }
                }
                None
            })
            .collect();
        for (task_id, task_title, worker) in completed_task_ids {
            // Mark before pushing so the borrow on self.last_state is released.
            self.task_completed_announced.insert(task_id.clone());
            events.push(DirectorEvent::TaskCompleted {
                task_id,
                task_title,
                worker,
            });
        }

        // Detect idle workers using consecutive-tick debouncing.
        //
        // Previous logic emitted `WorkerIdle` the moment a worker transitioned
        // from having a task to having none. In practice that window is often
        // sub-second (worker closes task X, immediately calls `task start Y`),
        // and if the 2s director refresh landed inside the gap it emitted a
        // spurious idle prompt that the supervisor saw as "idle arrived before
        // the claim." See cas-f9e8.
        //
        // We now track how many consecutive refresh ticks each factory agent
        // has appeared idle and only emit once the count reaches
        // `IDLE_CONSECUTIVE_TICKS`. A single "has task" observation resets the
        // streak, so transient None states never accumulate. `idle_already_emitted`
        // prevents re-emission on every tick of a sustained idle streak; the
        // existing `IDLE_RATE_LIMIT` debounce at `debounce_events` handles the
        // cross-streak cooldown.
        let mut seen_factory_agents: HashSet<String> = HashSet::new();
        let mut seen_factory_agent_names: HashSet<String> = HashSet::new();
        for agent in &data.agents {
            if !self.is_factory_agent(&agent.id, data) {
                continue;
            }

            // WorkerIdle must never fire for the supervisor / team-lead / primary
            // agent (cas-b67d). `is_factory_agent` deliberately includes the
            // supervisor so that task-assignment and completion events can
            // reference work done by the lead; but for idle tracking we only want
            // to surface genuine workers. A supervisor with current_task=None is
            // just waiting between decisions — not idle in the worker sense.
            let resolved_name = self.resolve_agent_name(&agent.id, data);
            if !self.is_worker_agent_name(&resolved_name) {
                continue;
            }

            seen_factory_agents.insert(agent.id.clone());
            seen_factory_agent_names.insert(resolved_name.clone());

            if self.held_workers.contains(&resolved_name) {
                // cas-09d0: a deliberately held worker is never
                // idle-needing-work and shouldn't stall-nudge either. Reset
                // any partially-accumulated state so a fresh streak starts
                // once released — mirrors the `pending_messages` gate below.
                self.consecutive_idle_ticks.remove(&agent.id);
                self.idle_already_emitted.remove(&agent.id);
                self.idle_transition_at.remove(&resolved_name);
                self.idle_handled_by_supervisor.remove(&resolved_name);
                self.stall_nudged.remove(&agent.id);
                self.stall_escalated.remove(&agent.id);
                continue;
            }

            // cas-78bf: an assigned Open task is normally a brief dispatch
            // window, and cas-dbbb intentionally suppresses WorkerIdle while
            // the worker has not called `task start` yet. That suppression
            // must not hide the same state forever. Once the configurable
            // stall threshold has elapsed with a live heartbeat and no
            // activity newer than the assignment, use the existing
            // WorkerStalled supervisor-escalation path. Transcript
            // confirmation remains the final gate, so harness-specific
            // liveness and in-flight tool-call evidence stay centralized in
            // `transcript_confirms_stall`.
            let assigned_open_task = data.ready_tasks.iter().find(|task| {
                task.status == TaskStatus::Open
                    && (task.assignee.as_deref() == Some(resolved_name.as_str())
                        || task.assignee.as_deref() == Some(agent.id.as_str()))
            });
            // An active InProgress task is the stronger signal: let the
            // existing stall block below evaluate it instead of allowing a
            // secondary assigned-Open task to shadow it.
            if let Some(task) = assigned_open_task.filter(|_| agent.current_task.is_none()) {
                let has_fresh_heartbeat = agent
                    .last_heartbeat
                    .map(|hb| {
                        let age_secs = (now_utc - hb).num_seconds();
                        age_secs >= 0 && age_secs < FRESH_HEARTBEAT_SECS
                    })
                    .unwrap_or(false);
                let effective_threshold =
                    effective_stall_threshold_secs(self.stall_threshold_secs, agent.effort);
                let elapsed = task.updated_at.and_then(|assigned_at| {
                    let activity_baseline = agent
                        .latest_activity
                        .as_ref()
                        .map(|(_, activity_at)| assigned_at.max(*activity_at))
                        .unwrap_or(assigned_at);
                    let quiet_age_secs = (now_utc - activity_baseline).num_seconds();
                    let assigned_age_secs = (now_utc - assigned_at).num_seconds();
                    (quiet_age_secs >= effective_threshold as i64 && assigned_age_secs >= 0)
                        .then_some(assigned_age_secs as u64)
                });

                if has_fresh_heartbeat
                    && elapsed.is_some()
                    && self.transcript_confirms_stall(&resolved_name)
                {
                    if !self.stall_escalated.contains(&agent.id) {
                        events.push(DirectorEvent::WorkerStalled {
                            worker: resolved_name.clone(),
                            task_id: task.id.clone(),
                            elapsed_secs: elapsed.unwrap_or_default(),
                            escalate: true,
                        });
                        self.stall_escalated.insert(agent.id.clone());
                    }
                    // Do not also generate a generic WorkerIdle event for the
                    // same sustained assigned-Open state.
                    continue;
                }

                self.stall_nudged.remove(&agent.id);
                self.stall_escalated.remove(&agent.id);
            }

            if let Some(task_id) = &agent.current_task {
                // Agent is working — reset the idle streak. The next time this
                // agent's `current_task` goes to `None`, the counter starts
                // again from zero, which is exactly what we want: sustained idle
                // from THIS point on, not a stale count from an earlier streak.
                self.consecutive_idle_ticks.remove(&agent.id);
                self.idle_already_emitted.remove(&agent.id);
                self.idle_transition_at.remove(&resolved_name);
                self.idle_handled_by_supervisor.remove(&resolved_name);

                // Stall detection (cas-9829): heartbeat alone cannot tell a
                // healthy in-progress worker from one that printed a plan and
                // stopped — a worker can heartbeat every tick while producing
                // zero tool calls/file edits/commits for the task it holds.
                // Require BOTH signals to diverge: a fresh heartbeat (the
                // worker process is alive) AND a `latest_activity` timestamp
                // older than `stall_threshold_secs` (it has genuinely gone
                // quiet, not just mid-turn). When either signal is absent
                // (no heartbeat data, no activity ever recorded) the gate
                // stays inactive rather than guessing.
                let has_fresh_heartbeat = agent
                    .last_heartbeat
                    .map(|hb| {
                        let age_secs = (now_utc - hb).num_seconds();
                        age_secs >= 0 && age_secs < FRESH_HEARTBEAT_SECS
                    })
                    .unwrap_or(false);

                // cas-728b: treat a genuine task-start transition as
                // activity. `latest_activity` only tracks checkpoint-class
                // events (file edit, commit, subagent, verification) — a
                // worker's first read/investigation turn on a dense task
                // can run 5+ minutes producing none of those while
                // `latest_activity` still holds a stale timestamp from
                // before this task started (or none at all).
                //
                // "Just transitioned" is detected the same way
                // `TaskAssigned` above detects an Open→InProgress edge:
                // diffing `self.last_state` (the PREVIOUS tick's status)
                // against this task's current status, not "first time this
                // detector instance has observed the task_id" — that
                // would incorrectly reset the grace baseline on every
                // fresh `DirectorEventDetector` even for a task that's been
                // InProgress for the same session's entire history (e.g.
                // right after `initialize()` snapshots already-in-progress
                // state).
                let just_transitioned_to_in_progress = self
                    .last_state
                    .tasks
                    .get(task_id)
                    .map(|(old_status, _)| *old_status != TaskStatus::InProgress)
                    .unwrap_or(true);
                if just_transitioned_to_in_progress {
                    self.task_start_observed
                        .insert(agent.id.clone(), (task_id.clone(), now_utc));
                }
                let task_start_ts = self
                    .task_start_observed
                    .get(&agent.id)
                    .filter(|(observed_task_id, _)| observed_task_id == task_id)
                    .map(|(_, observed_at)| *observed_at);

                // Baseline = whichever is MORE RECENT — real activity or
                // task-start. Once a task has genuinely run past the
                // threshold with zero activity since start, the grace
                // period has naturally elapsed and the predicate applies
                // exactly as before. Preserves the "no signal at all ⇒ gate
                // stays inactive" behavior when NEITHER is available (task
                // has been InProgress since before this detector started
                // watching, and no checkpoint event was ever recorded).
                let effective_activity_ts = match (
                    agent.latest_activity.as_ref().map(|(_, ts)| *ts),
                    task_start_ts,
                ) {
                    (Some(activity_ts), Some(start_ts)) => Some(activity_ts.max(start_ts)),
                    (Some(activity_ts), None) => Some(activity_ts),
                    (None, Some(start_ts)) => Some(start_ts),
                    (None, None) => None,
                };
                // cas-09d0: scale the threshold by this worker's configured
                // effort before comparing — a high/xhigh worker gets a longer
                // grace window before the same elapsed time counts as stalled.
                let effective_threshold =
                    effective_stall_threshold_secs(self.stall_threshold_secs, agent.effort);
                let stalled_elapsed_secs = effective_activity_ts.and_then(|ts| {
                    let age_secs = (now_utc - ts).num_seconds();
                    (age_secs >= effective_threshold as i64).then_some(age_secs)
                });

                if has_fresh_heartbeat {
                    if let Some(elapsed) = stalled_elapsed_secs {
                        // cas-728b / cas-ab80: confirm against transcript
                        // mtime — the same liveness signal `cas factory
                        // is-wedged` reads, using the same harness-specific
                        // activity_fresh_window — before firing/escalating.
                        // A transcript written within that window means the
                        // worker is actively producing output (reading,
                        // reasoning, tool calls) even though no
                        // checkpoint-class event has landed; the
                        // checkpoint-age signal alone can't tell that apart
                        // from a genuine stall. Re-consulted every tick a
                        // candidate is seen, so it also debounces repeat
                        // alerts for a worker that's confirmed alive —
                        // it never re-enters the `stalled_elapsed_secs`
                        // branch while its transcript stays fresh.
                        if !self.transcript_confirms_stall(&resolved_name) {
                            self.stall_nudged.remove(&agent.id);
                            self.stall_escalated.remove(&agent.id);
                        } else if !self.stall_nudged.contains(&agent.id) {
                            // First detection in this streak: auto-nudge the
                            // worker (re-inject the task prompt) before
                            // paging the supervisor — a single re-poke often
                            // unsticks these (see bug report cas-9829).
                            events.push(DirectorEvent::WorkerStalled {
                                worker: resolved_name.clone(),
                                task_id: task_id.clone(),
                                elapsed_secs: elapsed as u64,
                                escalate: false,
                            });
                            self.stall_nudged.insert(agent.id.clone());
                        } else if !self.stall_escalated.contains(&agent.id) {
                            // Still stalled after the nudge — escalate to
                            // the supervisor.
                            events.push(DirectorEvent::WorkerStalled {
                                worker: resolved_name.clone(),
                                task_id: task_id.clone(),
                                elapsed_secs: elapsed as u64,
                                escalate: true,
                            });
                            self.stall_escalated.insert(agent.id.clone());
                        }
                    } else {
                        // Activity is fresh (or was never observed) — clear
                        // any prior streak so a future stall re-nudges from
                        // scratch instead of silently staying suppressed.
                        self.stall_nudged.remove(&agent.id);
                        self.stall_escalated.remove(&agent.id);
                    }
                }

                continue;
            }

            let was_already_active = self.last_state.active_agents.contains(&agent.id);
            let idle_since = *self
                .idle_transition_at
                .entry(resolved_name.clone())
                .or_insert_with(|| {
                    if was_already_active {
                        now_utc
                    } else {
                        agent.registered_at
                    }
                });
            let registration_age_secs = (now_utc - agent.registered_at).num_seconds();
            if (0..SPAWN_ASSIGN_GRACE_SECS).contains(&registration_age_secs) {
                self.consecutive_idle_ticks.remove(&agent.id);
                continue;
            }
            let supervisor_message_handles_idle = agent
                .latest_supervisor_message_at
                .map(|sent_at| sent_at >= idle_since)
                .unwrap_or(false)
                // If the director observes the row while it is still pending,
                // preserve the existing suppression after it drains. This
                // covers a message queued between refreshes, before the first
                // task-less snapshot establishes its local baseline.
                || (agent.pending_supervisor_messages > 0
                    && agent.latest_supervisor_message_at.is_some());
            if supervisor_message_handles_idle {
                self.idle_handled_by_supervisor
                    .insert(resolved_name.clone());
            }

            if self.idle_handled_by_supervisor.contains(&resolved_name) {
                self.consecutive_idle_ticks.remove(&agent.id);
                continue;
            }

            if agent.pending_messages > 0 {
                // Worker has unread messages in the prompt queue — don't count
                // this tick as idle. A freshly spawned worker appears task-less
                // before it has polled its first assignment; firing `WorkerIdle`
                // here would cause the supervisor to re-assign on top of the
                // queued message (spawn race, cas-afb7). Reset the streak so the
                // counter only starts accumulating after the queue is drained.
                self.consecutive_idle_ticks.remove(&agent.id);
                continue;
            }

            // Fresh-heartbeat + recent-activity gate (cas-4038).
            //
            // A CC agent sends heartbeats on every tool call. Between turns the
            // agent has `current_task = None` (no active lease) but is still
            // alive and may have uncommitted work. If the worker's heartbeat is
            // fresh AND it had recent activity, the current task-less state is
            // almost certainly a between-turns gap, not a genuine idle. Reset the
            // idle streak so WorkerIdle only fires after a truly sustained window
            // where BOTH signals are cold.
            //
            // The gate requires BOTH conditions (AND logic):
            //  - fresh heartbeat: a live worker always heartbeats; stale = dead/stalled
            //  - recent activity: guards against a worker that heartbeats as a
            //    daemon alive-check but hasn't actually done any work lately
            //
            // When either signal is absent (no heartbeat data, no activity) the
            // gate is inactive and normal consecutive-tick debounce governs.
            let has_fresh_heartbeat = agent
                .last_heartbeat
                .map(|hb| {
                    let age_secs = (now_utc - hb).num_seconds();
                    age_secs >= 0 && age_secs < FRESH_HEARTBEAT_SECS
                })
                .unwrap_or(false);
            let has_recent_activity = agent
                .latest_activity
                .as_ref()
                .map(|(_, ts)| {
                    let age_secs = (now_utc - *ts).num_seconds();
                    age_secs >= 0 && age_secs < RECENT_ACTIVITY_SECS
                })
                .unwrap_or(false);
            if has_fresh_heartbeat && has_recent_activity {
                // Worker is alive and recently active between turns — do not count
                // this tick and reset any partial idle streak so a genuine idle
                // that follows has to accumulate from zero.
                self.consecutive_idle_ticks.remove(&agent.id);
                self.idle_transition_at.remove(&resolved_name);
                self.idle_handled_by_supervisor.remove(&resolved_name);
                continue;
            }

            let count = self
                .consecutive_idle_ticks
                .entry(agent.id.clone())
                .or_insert(0);
            *count += 1;

            if *count >= IDLE_CONSECUTIVE_TICKS && !self.idle_already_emitted.contains(&agent.id) {
                // `resolved_name` is guaranteed to be a worker (supervisor
                // was excluded above). Re-use it directly — no re-resolve.
                events.push(DirectorEvent::WorkerIdle {
                    worker: resolved_name.clone(),
                    active_task: agent.active_lease.clone(),
                });
                self.idle_already_emitted.insert(agent.id.clone());
            }
        }

        // Stop tracking idle state for agents that have left the active set
        // (shutdown, crash, reassigned out of this factory). Without this the
        // maps would grow unbounded across long sessions.
        self.consecutive_idle_ticks
            .retain(|id, _| seen_factory_agents.contains(id));
        self.idle_already_emitted
            .retain(|id| seen_factory_agents.contains(id));
        self.idle_transition_at
            .retain(|name, _| seen_factory_agent_names.contains(name));
        self.idle_handled_by_supervisor
            .retain(|name| seen_factory_agent_names.contains(name));
        self.stall_nudged
            .retain(|id| seen_factory_agents.contains(id));
        self.stall_escalated
            .retain(|id| seen_factory_agents.contains(id));

        // Detect new agent registrations
        for agent_id in &new_state.active_agents {
            if !self.last_state.active_agents.contains(agent_id) {
                let agent_name = self.resolve_agent_name(agent_id, data);
                let already_contacted = data
                    .agents
                    .iter()
                    .find(|agent| agent.id == *agent_id)
                    .and_then(|agent| {
                        agent
                            .latest_supervisor_message_at
                            .map(|sent_at| sent_at >= agent.registered_at)
                    })
                    .unwrap_or(false);
                if self.is_factory_agent_name(&agent_name) && !already_contacted {
                    events.push(DirectorEvent::AgentRegistered {
                        agent_id: agent_id.clone(),
                        agent_name,
                    });
                }
            }
        }

        // Detect epic state changes
        // EpicStarted fires when:
        // 1. An epic transitions to InProgress (highest priority)
        // 2. A newly-appearing Open-with-branch epic is strictly better than
        //    the currently-active epic under the shared subtask-count
        //    heuristic. The picker and the init-time `detect_epic_state`
        //    share `pick_best_open_branch_epic` so they cannot diverge.
        {
            let mut in_progress_started: Option<(&str, &str)> = None;
            let mut saw_new_open_branch = false;

            for epic in &data.epic_tasks {
                if epic.status == TaskStatus::InProgress {
                    let was_in_progress = self
                        .last_state
                        .epic_statuses
                        .get(&epic.id)
                        .map(|(s, _)| *s == TaskStatus::InProgress)
                        .unwrap_or(false);

                    if !was_in_progress {
                        in_progress_started = Some((&epic.id, &epic.title));
                    }
                } else if epic.status == TaskStatus::Open && epic.branch.is_some() {
                    let was_open_with_branch = self
                        .last_state
                        .epic_statuses
                        .get(&epic.id)
                        .map(|(s, had_branch)| *s == TaskStatus::Open && *had_branch)
                        .unwrap_or(false);

                    if !was_open_with_branch {
                        saw_new_open_branch = true;
                    }
                }
            }

            // InProgress transitions always fire.
            if let Some((id, title)) = in_progress_started {
                events.push(DirectorEvent::EpicStarted {
                    epic_id: id.to_string(),
                    epic_title: title.to_string(),
                });
            } else if saw_new_open_branch {
                // Pick the best Open-with-branch epic using the shared
                // heuristic (subtasks, then lex ID). Applies the
                // strict-improvement gate when a current epic is known.
                if let Some(candidate) = pick_best_open_branch_epic(
                    &data.epic_tasks,
                    &data.in_progress_tasks,
                    &data.ready_tasks,
                ) {
                    // A tracked epic that has since been closed/deleted is
                    // treated as vacant so a legitimate new Open-with-branch
                    // epic can take over instead of the UI freezing on a
                    // ghost id (cas-4181 adversarial finding).
                    let cur_still_exists = current_epic_id
                        .map(|cur| data.epic_tasks.iter().any(|e| e.id == cur))
                        .unwrap_or(false);
                    let effective_current = if cur_still_exists {
                        current_epic_id
                    } else {
                        None
                    };
                    let should_fire = match effective_current {
                        // No active epic yet — any valid candidate wins.
                        None => true,
                        // Same epic already active — no change to announce.
                        Some(cur) if cur == candidate.id => false,
                        // Different epic — only announce if it is strictly
                        // better than the currently-active epic under the
                        // shared heuristic. A zero-subtask fresh epic cannot
                        // hijack an active one that has subtasks.
                        Some(cur) => {
                            let cand_score = open_branch_epic_score(
                                &candidate.id,
                                &data.in_progress_tasks,
                                &data.ready_tasks,
                            );
                            let cur_score = open_branch_epic_score(
                                cur,
                                &data.in_progress_tasks,
                                &data.ready_tasks,
                            );
                            cand_score > cur_score
                        }
                    };

                    if should_fire {
                        events.push(DirectorEvent::EpicStarted {
                            epic_id: candidate.id.clone(),
                            epic_title: candidate.title.clone(),
                        });
                    }
                }
            }
        }

        // EpicCompleted: Epic status changed to Closed
        for epic in &data.epic_tasks {
            if epic.status == TaskStatus::Closed {
                let was_closed = self
                    .last_state
                    .epic_statuses
                    .get(&epic.id)
                    .map(|(s, _)| *s == TaskStatus::Closed)
                    .unwrap_or(false);

                if !was_closed {
                    events.push(DirectorEvent::EpicCompleted {
                        epic_id: epic.id.clone(),
                    });
                }
            }
        }

        // EpicAllSubtasksClosed: All subtasks of a non-closed epic just became closed.
        // Detected when active subtask count drops to 0 from a previous count > 0.
        for epic in &data.epic_tasks {
            if epic.status != TaskStatus::Closed {
                let current_count = new_state
                    .epic_active_subtask_counts
                    .get(&epic.id)
                    .copied()
                    .unwrap_or(0);
                let previous_count = self
                    .last_state
                    .epic_active_subtask_counts
                    .get(&epic.id)
                    .copied()
                    .unwrap_or(0);

                if current_count == 0 && previous_count > 0 {
                    events.push(DirectorEvent::EpicAllSubtasksClosed {
                        epic_id: epic.id.clone(),
                        epic_title: epic.title.clone(),
                    });
                }
            }
        }

        // Update state for next comparison
        self.last_state = new_state;

        // Apply debouncing - filter out events emitted recently
        self.debounce_events(events, now)
    }

    /// Filter out events that were emitted recently (within debounce window)
    ///
    /// WorkerIdle events use a longer rate limit (5 minutes) to prevent flooding
    /// the supervisor when multiple workers idle simultaneously.
    /// Events from removed (shutdown/crashed) workers are suppressed entirely.
    fn debounce_events(&mut self, events: Vec<DirectorEvent>, now: Instant) -> Vec<DirectorEvent> {
        // Clean up old entries (use the longer idle rate limit as max TTL)
        self.last_prompt_times
            .retain(|_, time| now.duration_since(*time) < IDLE_RATE_LIMIT);

        // Filter events and update timestamps
        events
            .into_iter()
            .filter(|event| {
                // Suppress all events from removed (shutdown/crashed) workers
                if let Some(target) = event.target() {
                    if self.removed_workers.contains(target) {
                        return false;
                    }
                }

                let key = event.debounce_key();
                let window = if matches!(event, DirectorEvent::WorkerIdle { .. }) {
                    IDLE_RATE_LIMIT
                } else {
                    DEBOUNCE_DURATION
                };
                let should_emit = self
                    .last_prompt_times
                    .get(&key)
                    .map(|last_time| now.duration_since(*last_time) >= window)
                    .unwrap_or(true);

                if should_emit {
                    self.last_prompt_times.insert(key, now);
                }
                should_emit
            })
            .collect()
    }

    /// Check if an agent ID belongs to this factory session
    fn is_factory_agent(&self, agent_id: &str, data: &DirectorData) -> bool {
        // Resolve agent ID to name first
        let name = data
            .agent_id_to_name
            .get(agent_id)
            .map(|s| s.as_str())
            .unwrap_or(agent_id);

        // Check if name matches any worker or supervisor
        self.worker_names.contains(&name.to_string()) || name == self.supervisor_name
    }

    /// True only for a worker in this factory, never for the supervisor. Task
    /// assignment/completion templates are worker-directed lifecycle traffic;
    /// accepting the supervisor here turns a supervisor-owned gate into a
    /// self-assignment followed by a bogus worker-complete notice.
    fn is_factory_worker(&self, agent_id: &str, data: &DirectorData) -> bool {
        let name = data
            .agent_id_to_name
            .get(agent_id)
            .map(|name| name.as_str())
            .unwrap_or(agent_id);
        self.is_worker_agent_name(name)
    }

    /// Check if an agent name belongs to this factory session
    fn is_factory_agent_name(&self, name: &str) -> bool {
        self.worker_names.contains(&name.to_string()) || name == self.supervisor_name
    }

    /// Check if an agent name is a **worker** in this factory session.
    ///
    /// Unlike `is_factory_agent_name`, this explicitly excludes the supervisor /
    /// primary agent. Use this wherever the intent is "this is one of MY workers"
    /// and the supervisor receiving the event would be wrong — e.g. the WorkerIdle
    /// path (cas-b67d / cas-c790).
    ///
    /// The explicit `!= supervisor_name` guard is defense-in-depth: even if the
    /// supervisor's name ends up in `worker_names` via a resume/reconnect path that
    /// doesn't go through `add_worker`, this check prevents a spurious WorkerIdle
    /// from propagating to the prompt layer.
    fn is_worker_agent_name(&self, name: &str) -> bool {
        self.worker_names.contains(&name.to_string()) && name != self.supervisor_name
    }

    /// Resolve agent ID to display name
    fn resolve_agent_name(&self, agent_id: &str, data: &DirectorData) -> String {
        data.agent_id_to_name
            .get(agent_id)
            .cloned()
            .unwrap_or_else(|| agent_id.to_string())
    }
}

/// (cas-728b / cas-de95) Pure decision given an already-resolved transcript path
/// and the worker's harness: `false` means do **not** fire a stalled nudge
/// (worker still producing output, **or** transcript telemetry is unavailable
/// so absence is not evidence); `true` means a **resolved, cold** transcript
/// corroborates the checkpoint-age stall candidate.
///
/// cas-de95: missing/unresolvable path returns `false` — treating telemetry
/// absence as a stall confirmation was the live false-nudge failure mode for
/// Codex/Claude workers with unresolved rollouts/transcripts.
///
/// cas-ab80: freshness window is harness-specific via
/// [`activity_fresh_window`](crate::cli::factory::wedged::activity_fresh_window),
/// matching `is-wedged`.
///
/// cas-7e85: also consults
/// [`transcript_has_in_flight_tool_call`](crate::cli::factory::wedged::transcript_has_in_flight_tool_call)
/// — the SAME function `cas factory is-wedged` uses — so an outstanding
/// tool call (e.g. a worker sleeping on a backgrounded `cargo test`) never
/// confirms a stall here while is-wedged would call the worker Alive.
fn transcript_confirms_stall_for_path(
    transcript_path: Option<&std::path::Path>,
    cli: cas_mux::SupervisorCli,
) -> bool {
    transcript_confirms_stall_for_path_with_background(
        transcript_path,
        cli,
        &crate::cli::factory::wedged::BackgroundProcessState::Unavailable,
    )
}

fn transcript_confirms_stall_for_path_with_background(
    transcript_path: Option<&std::path::Path>,
    cli: cas_mux::SupervisorCli,
    background_processes: &crate::cli::factory::wedged::BackgroundProcessState,
) -> bool {
    let Some(path) = transcript_path else {
        return false;
    };
    let window = crate::cli::factory::wedged::activity_fresh_window(cli);
    let in_flight = crate::cli::factory::wedged::transcript_has_in_flight_tool_call(path, cli);
    transcript_confirms_stall_for_age_with_background(
        crate::cli::factory::wedged::transcript_mtime_age(path),
        window,
        in_flight,
        background_processes.is_active(),
    )
}

/// (cas-09d0 / cas-de95 / cas-ab80 / cas-7e85) Pure core of the confirmation
/// decision.
///
/// - `in_flight_tool_call == true` → **never** confirm (AC1: an outstanding
///   call proves the worker is actively waiting on real work, regardless of
///   transcript age — checked first, short-circuiting the age comparison).
/// - `Some(age < fresh_window)` → not stalled (fresh transcript)
/// - `Some(age ≥ fresh_window)` → confirm stall (cold transcript is positive evidence)
/// - `None` → **do not confirm** (unresolved/missing telemetry is not starvation)
///
/// `fresh_window` must come from
/// [`activity_fresh_window`](crate::cli::factory::wedged::activity_fresh_window)
/// (or the same constants) so director and is-wedged agree.
fn transcript_confirms_stall_for_age(
    age: Option<Duration>,
    fresh_window: Duration,
    in_flight_tool_call: bool,
) -> bool {
    transcript_confirms_stall_for_age_with_background(age, fresh_window, in_flight_tool_call, false)
}

fn transcript_confirms_stall_for_age_with_background(
    age: Option<Duration>,
    fresh_window: Duration,
    in_flight_tool_call: bool,
    background_process_active: bool,
) -> bool {
    if in_flight_tool_call || background_process_active {
        return false;
    }
    match age {
        Some(age) if age < fresh_window => false,
        Some(_) => true,
        None => false,
    }
}

/// Score an Open-with-branch epic by active-subtask counts.
///
/// Returns `(in_progress_count, ready_count)` for subtasks whose `epic`
/// field matches `epic_id`. The tuple compares lexicographically: an
/// epic with more in-progress subtasks always outranks one with fewer,
/// regardless of ready-count. Used by both the init-time picker and the
/// runtime EpicStarted strict-improvement gate.
pub(crate) fn open_branch_epic_score(
    epic_id: &str,
    in_progress_tasks: &[TaskSummary],
    ready_tasks: &[TaskSummary],
) -> (usize, usize) {
    let ip = in_progress_tasks
        .iter()
        .filter(|t| t.epic.as_deref() == Some(epic_id))
        .count();
    let ready = ready_tasks
        .iter()
        .filter(|t| t.epic.as_deref() == Some(epic_id))
        .count();
    (ip, ready)
}

/// Pick the best Open-with-branch epic from `epic_tasks` using the shared
/// heuristic: highest in-progress subtask count wins; then highest ready
/// subtask count; then lexicographically greatest ID as a deterministic
/// final tiebreak.
///
/// Used by both `ui::factory::app::detect_epic_state` (init-time epic
/// resolution) and `DirectorEventDetector::detect_changes` (runtime
/// `EpicStarted` detection) so the two paths cannot disagree on which
/// Open-with-branch epic should own the factory panel.
///
/// Returns `None` if no epic in `epic_tasks` is `Open` with a branch set.
pub(crate) fn pick_best_open_branch_epic<'a>(
    epic_tasks: &'a [TaskSummary],
    in_progress_tasks: &[TaskSummary],
    ready_tasks: &[TaskSummary],
) -> Option<&'a TaskSummary> {
    epic_tasks
        .iter()
        .filter(|e| e.status == TaskStatus::Open && e.branch.is_some())
        .max_by(|a, b| {
            let a_score = open_branch_epic_score(&a.id, in_progress_tasks, ready_tasks);
            let b_score = open_branch_epic_score(&b.id, in_progress_tasks, ready_tasks);
            a_score
                .cmp(&b_score)
                // Deterministic final tiebreak: greatest lex ID wins.
                .then_with(|| a.id.cmp(&b.id))
        })
}

#[cfg(test)]
#[path = "events_tests/tests.rs"]
mod tests;
