//! Data loading for the director panel.
//!
//! This module provides DirectorData aggregation from CAS stores,
//! without any TUI/rendering dependencies.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use cas_store::{
    AgentStore, EventStore, PromptQueueStore, QueuedPrompt, Reminder, ReminderStore,
    SqliteAgentStore, SqliteEventStore, SqlitePromptQueueStore, SqliteReminderStore,
    SqliteTaskStore, SqliteWorktreeStore, WorktreeStore,
};
use cas_types::{
    AgentRole, AgentStatus, Event, EventType, Priority, Task, TaskStatus, TaskType, WorktreeStatus,
};

use crate::changes::{FileChangeInfo, GitFileStatus, SourceChangesInfo};

/// cas-e98e: lightweight PID liveness for FACTORY pane reconciliation.
/// Linux: `/proc/<pid>` exists. Other platforms: conservatively false
/// (no false "live" without a real probe).
fn agent_pid_looks_alive(pid: Option<u32>) -> bool {
    let Some(pid) = pid else {
        return false;
    };
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

/// Cached store handles to avoid re-opening on every refresh cycle.
///
/// Each `open()` call does path canonicalization + global hashmap lookup.
/// Caching the handles in the caller (e.g., FactoryApp) eliminates that overhead.
pub struct DirectorStores {
    pub task_store: SqliteTaskStore,
    pub event_store: SqliteEventStore,
    pub agent_store: SqliteAgentStore,
    pub worktree_store: Option<SqliteWorktreeStore>,
    pub reminder_store: Option<SqliteReminderStore>,
    /// Best-effort handle for per-agent pending-message counts. Used by the
    /// idle detector to suppress `WorkerIdle` while a worker has unread
    /// messages in the prompt queue (spawn-race fix, cas-afb7).
    pub prompt_queue_store: Option<SqlitePromptQueueStore>,
}

impl DirectorStores {
    /// Open all stores for a CAS directory. Worktree, reminder, and
    /// prompt-queue stores are best-effort (None on failure) since they are
    /// not critical.
    pub fn open(cas_dir: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            task_store: SqliteTaskStore::open(cas_dir)?,
            event_store: SqliteEventStore::open(cas_dir)?,
            agent_store: SqliteAgentStore::open(cas_dir)?,
            worktree_store: SqliteWorktreeStore::open(cas_dir).ok(),
            reminder_store: SqliteReminderStore::open(cas_dir).ok(),
            prompt_queue_store: SqlitePromptQueueStore::open(cas_dir).ok(),
        })
    }
}

/// A summary of a task for display
#[derive(Debug, Clone)]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
    pub priority: Priority,
    pub assignee: Option<String>,
    pub task_type: TaskType,
    /// Parent epic ID (if this is a subtask)
    pub epic: Option<String>,
    /// The branch that is authoritative for this summary's delivery boundary:
    /// an epic's branch, or a task's declared WorkTarget branch.
    ///
    /// A task-level WorkTarget deliberately wins over its parent epic. The
    /// factory relay must advertise the same destination that close enforces.
    pub branch: Option<String>,
    /// Last-updated timestamp from the task store. Drives recency ordering in
    /// the TASKS panel so the supervisor's current focus surfaces above a
    /// stale, high-subtask-count epic (cas-2fb6). `None` when the source is a
    /// hand-built summary that does not carry timing (e.g. some tests).
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Agent id/name responsible for epic verification (epics only; cas-9fff).
    /// Used to route epic-completion prompts to the owning supervisor instead
    /// of every concurrent factory session's supervisor.
    pub epic_verification_owner: Option<String>,
}

/// Task state carried by an active lease even when the agent currently appears
/// idle to the director.
#[derive(Debug, Clone)]
pub struct ActiveLeaseSummary {
    pub task_id: String,
    pub task_title: String,
    pub task_status: TaskStatus,
    pub close_rejected_reason: Option<String>,
}

/// A summary of an agent for display
#[derive(Debug, Clone)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    pub status: AgentStatus,
    /// When this agent registered. Used as the ready/idle-transition baseline
    /// for suppressing a redundant director nudge after supervisor contact.
    pub registered_at: chrono::DateTime<chrono::Utc>,
    pub current_task: Option<String>,
    /// Latest activity (description, timestamp)
    pub latest_activity: Option<(String, chrono::DateTime<chrono::Utc>)>,
    /// Last heartbeat timestamp
    pub last_heartbeat: Option<chrono::DateTime<chrono::Utc>>,
    /// Number of currently delivery-eligible prompt-queue messages addressed
    /// to this agent. Used by the idle detector to suppress `WorkerIdle` when
    /// the worker has unread work it can poll now (spawn race mitigation,
    /// cas-afb7). Rows in a future retry window are excluded.
    pub pending_messages: u32,
    /// Delivery-eligible subset authored by a live supervisor in this factory
    /// session.
    pub pending_supervisor_messages: u32,
    /// Most recent prompt-queue message from this factory session's
    /// supervisor(s), including already-processed rows.
    pub latest_supervisor_message_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Active task lease state, resolved directly from task_leases. This stays
    /// separate from `current_task` because an agent can appear idle while a
    /// task row remains InProgress after a rejected close.
    pub active_lease: Option<ActiveLeaseSummary>,
    /// Configured reasoning effort, parsed from `Agent.metadata["worker_effort"]`
    /// (written at registration time from `CAS_FACTORY_WORKER_EFFORT`, see
    /// `apply_factory_worker_metadata` in `cas-cli/src/mcp/daemon.rs`). `None`
    /// when unset (legacy agent, or a role that doesn't carry an effort tier).
    /// Used by the director's stall detector to scale its threshold —
    /// a high/xhigh-effort worker's read-and-think phase routinely runs
    /// longer before producing a checkpoint-class activity event than a
    /// low-effort worker's (cas-09d0).
    pub effort: Option<cas_mux::Effort>,
}

/// A group of tasks under an epic
#[derive(Debug, Clone)]
pub struct EpicGroup {
    /// The epic task itself
    pub epic: TaskSummary,
    /// Subtasks belonging to this epic
    pub subtasks: Vec<TaskSummary>,
    /// Whether any subtask is in_progress
    pub has_active: bool,
}

/// Data for the director panel
#[derive(Debug, Clone)]
pub struct DirectorData {
    /// Ready tasks (open, not blocked)
    pub ready_tasks: Vec<TaskSummary>,
    /// In-progress tasks
    pub in_progress_tasks: Vec<TaskSummary>,
    /// Epic tasks (for tracking epic status)
    pub epic_tasks: Vec<TaskSummary>,
    /// Active agents
    pub agents: Vec<AgentSummary>,
    /// Recent activity events
    pub activity: Vec<Event>,
    /// Map of agent ID to name
    pub agent_id_to_name: HashMap<String, String>,
    /// Git changes by source (main repo + worktrees)
    pub changes: Vec<SourceChangesInfo>,
    /// Whether git changes have been loaded (for lazy loading)
    pub git_loaded: bool,
    /// Pending reminders (across all supervisors in this session)
    pub reminders: Vec<Reminder>,
    /// Count of closed subtasks per epic (epic_id -> closed_count)
    pub epic_closed_counts: HashMap<String, usize>,
}

impl DirectorData {
    /// Load data from CAS stores
    ///
    /// # Arguments
    /// * `cas_dir` - Path to the CAS directory
    /// * `worktree_root` - Optional path to the worktree directory for factory workers
    pub fn load(cas_dir: &Path, worktree_root: Option<&Path>) -> anyhow::Result<Self> {
        Self::load_with_git(cas_dir, worktree_root, true)
    }

    /// Fast load without git changes (for initial startup)
    ///
    /// Git changes are loaded lazily on first refresh to speed up daemon startup.
    pub fn load_fast(cas_dir: &Path) -> anyhow::Result<Self> {
        Self::load_with_git(cas_dir, None, false)
    }

    /// Load data with optional git change collection.
    ///
    /// This allows callers to refresh CAS data frequently while throttling expensive git ops.
    pub fn load_with_git(
        cas_dir: &Path,
        worktree_root: Option<&Path>,
        load_git: bool,
    ) -> anyhow::Result<Self> {
        Self::load_with_stores(cas_dir, worktree_root, load_git, None)
    }

    /// Refresh only git changes while preserving already-loaded task/agent/activity data.
    pub fn refresh_git_changes(
        &mut self,
        cas_dir: &Path,
        worktree_root: Option<&Path>,
    ) -> anyhow::Result<()> {
        self.changes = load_all_git_changes(cas_dir, worktree_root, &self.agent_id_to_name, None)?;
        self.git_loaded = true;
        Ok(())
    }

    /// Refresh only git changes using cached stores.
    pub fn refresh_git_changes_with_stores(
        &mut self,
        cas_dir: &Path,
        worktree_root: Option<&Path>,
        stores: Option<&DirectorStores>,
    ) -> anyhow::Result<()> {
        let wt_store = stores.and_then(|s| s.worktree_store.as_ref());
        self.changes =
            load_all_git_changes(cas_dir, worktree_root, &self.agent_id_to_name, wt_store)?;
        self.git_loaded = true;
        Ok(())
    }

    /// Load data with configurable options and optional cached stores.
    ///
    /// When `stores` is provided, uses the cached handles instead of re-opening.
    pub fn load_with_stores(
        cas_dir: &Path,
        worktree_root: Option<&Path>,
        load_git: bool,
        stores: Option<&DirectorStores>,
    ) -> anyhow::Result<Self> {
        // Use cached stores or open fresh ones
        let owned_task;
        let task_store: &SqliteTaskStore = if let Some(s) = stores {
            &s.task_store
        } else {
            owned_task = SqliteTaskStore::open(cas_dir)?;
            &owned_task
        };
        // Atomically load tasks + parent-child deps in a single lock hold to prevent
        // read skew where a task exists but its epic link is invisible (causes panel flicker)
        let (tasks, parent_child_deps) = task_store.list_with_parent_deps()?;

        // Build assignee to task map for looking up current tasks
        let mut assignee_tasks: HashMap<String, String> = HashMap::new();
        for task in &tasks {
            if task.status == TaskStatus::InProgress
                && let Some(ref assignee) = task.assignee
            {
                assignee_tasks.insert(assignee.clone(), task.id.clone());
            }
        }
        let task_by_id: HashMap<String, &Task> =
            tasks.iter().map(|task| (task.id.clone(), task)).collect();

        // Build map: child_id -> parent_epic_id
        let child_to_epic: HashMap<String, String> = parent_child_deps
            .iter()
            .map(|dep| (dep.from_id.clone(), dep.to_id.clone()))
            .collect();

        // Helper to convert Task to TaskSummary with epic relationship
        let to_summary = |t: &Task| -> TaskSummary {
            TaskSummary {
                id: t.id.clone(),
                title: t.title.clone(),
                status: t.status,
                priority: t.priority,
                assignee: t.assignee.clone(),
                task_type: t.task_type,
                epic: child_to_epic.get(&t.id).cloned(),
                branch: t
                    .deliverables
                    .work_target
                    .as_ref()
                    .map(|target| target.target_branch.clone())
                    .or_else(|| t.branch.clone()),
                updated_at: Some(t.updated_at),
                epic_verification_owner: t.epic_verification_owner.clone(),
            }
        };

        // Single pass: partition tasks into ready, in_progress, epic, and count closed per epic
        let mut ready_tasks: Vec<TaskSummary> = Vec::new();
        let mut in_progress_tasks: Vec<TaskSummary> = Vec::new();
        let mut epic_tasks: Vec<TaskSummary> = Vec::new();
        let mut epic_closed_counts: HashMap<String, usize> = HashMap::new();

        for task in &tasks {
            if task.task_type == TaskType::Epic {
                epic_tasks.push(to_summary(task));
            } else {
                match task.status {
                    TaskStatus::Open | TaskStatus::Blocked => {
                        ready_tasks.push(to_summary(task));
                    }
                    TaskStatus::InProgress => {
                        in_progress_tasks.push(to_summary(task));
                    }
                    TaskStatus::Closed => {
                        if task.counts_as_delivered()
                            && let Some(epic_id) = child_to_epic.get(&task.id)
                        {
                            *epic_closed_counts.entry(epic_id.clone()).or_insert(0) += 1;
                        }
                    }
                    TaskStatus::Cancelled => {
                        // Terminal but intentionally not delivered. Keep it out
                        // of both active queues and delivered child counts.
                    }
                    // cas-b51a: show tasks awaiting supervisor review as a
                    // separate in-progress bucket so the TUI surfaces them
                    // for the supervisor to act on.
                    TaskStatus::PendingSupervisorReview => {
                        in_progress_tasks.push(to_summary(task));
                    }
                    // cas-8d5b: worker work is done, but the factory branch
                    // is waiting on supervisor merge. Keep it visible in the
                    // waiting/active panel without treating it as worker-
                    // actionable ready work.
                    TaskStatus::AwaitingMerge => {
                        in_progress_tasks.push(to_summary(task));
                    }
                }
            }
        }

        // Load recent activity first (needed for agent latest_activity)
        let owned_event;
        let event_store: &SqliteEventStore = if let Some(s) = stores {
            &s.event_store
        } else {
            owned_event = SqliteEventStore::open(cas_dir)?;
            &owned_event
        };
        let activity = event_store.list_recent(50)?; // Load more to find worker activities

        // Build map of agent_id -> latest worker activity
        let worker_activity_types = [
            EventType::WorkerSubagentSpawned,
            EventType::WorkerSubagentCompleted,
            EventType::WorkerFileEdited,
            EventType::WorkerGitCommit,
            EventType::WorkerVerificationBlocked,
            EventType::VerificationStarted,
            EventType::VerificationAdded,
            EventType::TaskNoteAdded,
            EventType::AuditTrailGap,
        ];
        let mut agent_latest_activity: HashMap<String, (String, chrono::DateTime<chrono::Utc>)> =
            HashMap::new();
        for event in &activity {
            if worker_activity_types.contains(&event.event_type)
                && let Some(session_id) = &event.session_id
            {
                // Only keep the latest (first encountered since list is sorted by time desc)
                agent_latest_activity
                    .entry(session_id.clone())
                    .or_insert_with(|| (event.summary.clone(), event.created_at));
            }
        }
        let mut close_rejections: HashMap<(String, String), String> = HashMap::new();
        for event in &activity {
            if event.event_type != EventType::WorkerVerificationBlocked {
                continue;
            }
            let Some(session_id) = event.session_id.as_ref() else {
                continue;
            };
            let Some(metadata) = event.metadata.as_ref() else {
                continue;
            };
            if metadata
                .get("close_rejected")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                if let Some(task_id) = metadata.get("task_id").and_then(|v| v.as_str()) {
                    let reason = metadata
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or(event.summary.as_str())
                        .to_string();
                    close_rejections
                        .entry((session_id.clone(), task_id.to_string()))
                        .or_insert(reason);
                }
            }
        }

        // Load agents
        let owned_agent;
        let agent_store: &SqliteAgentStore = if let Some(s) = stores {
            &s.agent_store
        } else {
            owned_agent = SqliteAgentStore::open(cas_dir)?;
            &owned_agent
        };
        let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
        let agents_list: Vec<_> = AgentStore::list(agent_store, None)?
            .into_iter()
            .filter(|a| a.visible_to_factory_session(factory_session.as_deref()))
            .collect();

        // Compute per-agent delivery-eligible message counts (best-effort,
        // non-fatal).
        // These are used by the idle detector to suppress `WorkerIdle` when a
        // freshly spawned worker has unread messages waiting in the queue before
        // it has had a chance to poll them (spawn race fix, cas-afb7). A row in
        // a future retry window does not suppress idle: repeated delivery
        // failures are themselves evidence that the worker needs attention.
        let agent_names_for_pq: Vec<&str> = agents_list.iter().map(|a| a.name.as_str()).collect();
        let supervisor_sources: Vec<&str> = agents_list
            .iter()
            .filter(|a| a.role == AgentRole::Supervisor)
            .flat_map(|a| [a.name.as_str(), a.id.as_str()])
            .collect();
        let pending_msgs: Vec<QueuedPrompt> =
            if let Some(pq) = stores.and_then(|s| s.prompt_queue_store.as_ref()) {
                pq.peek_for_targets(&agent_names_for_pq, factory_session.as_deref(), 500)
                    .unwrap_or_default()
            } else {
                SqlitePromptQueueStore::open(cas_dir)
                    .ok()
                    .and_then(|pq| {
                        pq.peek_for_targets(&agent_names_for_pq, factory_session.as_deref(), 500)
                            .ok()
                    })
                    .unwrap_or_default()
            };
        let mut pending_counts: HashMap<String, u32> = HashMap::new();
        let mut pending_supervisor_counts: HashMap<String, u32> = HashMap::new();
        for msg in pending_msgs {
            let target = msg.target;
            if supervisor_sources.contains(&msg.source.as_str()) {
                *pending_supervisor_counts.entry(target.clone()).or_insert(0) += 1;
            }
            *pending_counts.entry(target).or_insert(0) += 1;
        }
        let latest_supervisor_messages =
            if let Some(pq) = stores.and_then(|s| s.prompt_queue_store.as_ref()) {
                pq.latest_created_at_for_targets_from_sources(
                    &supervisor_sources,
                    &agent_names_for_pq,
                    factory_session.as_deref(),
                )
                .unwrap_or_default()
            } else {
                SqlitePromptQueueStore::open(cas_dir)
                    .ok()
                    .and_then(|pq| {
                        pq.latest_created_at_for_targets_from_sources(
                            &supervisor_sources,
                            &agent_names_for_pq,
                            factory_session.as_deref(),
                        )
                        .ok()
                    })
                    .unwrap_or_default()
            };

        let mut agent_id_to_name = HashMap::new();
        let agents: Vec<AgentSummary> = agents_list
            .into_iter()
            // Factory-relevant agents only. cas-e98e: also keep Stale rows
            // whose registered PID is still live so FACTORY does not show
            // "not registered" for a mid-turn agent that lost its heartbeat
            // (same dual-signal as worker_status process-alive suppress).
            .filter(|a| {
                let role_ok = a.role == AgentRole::Supervisor
                    || a.role == AgentRole::Worker
                    || a.role == AgentRole::Director;
                if !role_ok {
                    return false;
                }
                match a.status {
                    AgentStatus::Active | AgentStatus::Idle => true,
                    AgentStatus::Stale => agent_pid_looks_alive(a.pid),
                    _ => false,
                }
            })
            .map(|a| {
                agent_id_to_name.insert(a.id.clone(), a.name.clone());
                // Task assignees store agent names (not IDs), so look up by name
                let current_task = assignee_tasks.get(&a.name).cloned();
                let latest_activity = agent_latest_activity.get(&a.id).cloned();
                let pending_messages = pending_counts.get(&a.name).copied().unwrap_or(0);
                let pending_supervisor_messages =
                    pending_supervisor_counts.get(&a.name).copied().unwrap_or(0);
                let latest_supervisor_message_at = latest_supervisor_messages.get(&a.name).copied();
                let effort = a
                    .metadata
                    .get("worker_effort")
                    .and_then(|s| s.parse::<cas_mux::Effort>().ok());
                // cas-627f: `list_agent_leases` returns only status='active'
                // rows, but `park_task_awaiting_merge` (cas-8d5b) deliberately
                // RELEASES the worker's lease as part of parking — so the
                // one-task gate doesn't block the worker's next `task start`.
                // That means a parked AwaitingMerge task can never be found
                // via the lease table once parked, which made the flagship
                // close-rejected `WorkerIdle` notification unreachable
                // (confirmed P1, docs/reviews/2026-07-07-cas-b646-epic.md).
                // Fall back to resolving by assignee + AwaitingMerge status
                // directly from the task table — `park_task_awaiting_merge`
                // preserves `assignee`, so this is a reliable second source
                // for exactly the one status the lease table can't carry.
                let active_lease = agent_store
                    .list_agent_leases(&a.id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|lease| task_by_id.get(&lease.task_id).copied())
                    .find(|task| {
                        matches!(
                            task.status,
                            TaskStatus::InProgress | TaskStatus::AwaitingMerge
                        )
                    })
                    .or_else(|| {
                        tasks.iter().find(|task| {
                            task.status == TaskStatus::AwaitingMerge
                                && task.assignee.as_deref() == Some(a.name.as_str())
                        })
                    })
                    .map(|task| ActiveLeaseSummary {
                        task_id: task.id.clone(),
                        task_title: task.title.clone(),
                        task_status: task.status,
                        close_rejected_reason: close_rejections
                            .get(&(a.id.clone(), task.id.clone()))
                            .cloned(),
                    });
                // Surface process-alive Stale rows as Active so icons/counts
                // match worker_status dual-signal (cas-e98e).
                let display_status =
                    if a.status == AgentStatus::Stale && agent_pid_looks_alive(a.pid) {
                        AgentStatus::Active
                    } else {
                        a.status
                    };
                AgentSummary {
                    id: a.id,
                    name: a.name,
                    status: display_status,
                    registered_at: a.registered_at,
                    current_task,
                    latest_activity,
                    last_heartbeat: Some(a.last_heartbeat),
                    pending_messages,
                    pending_supervisor_messages,
                    latest_supervisor_message_at,
                    active_lease,
                    effort,
                }
            })
            .collect();

        // Trim activity to 20 for display
        let activity: Vec<Event> = activity.into_iter().take(20).collect();

        // Load git changes (optionally skip for fast startup)
        let worktree_store_ref = stores.and_then(|s| s.worktree_store.as_ref());
        let (changes, git_loaded) = if load_git {
            let changes = load_all_git_changes(
                cas_dir,
                worktree_root,
                &agent_id_to_name,
                worktree_store_ref,
            )?;
            (changes, true)
        } else {
            (Vec::new(), false)
        };

        // Load pending + recently fired reminders (best-effort, non-fatal)
        let reminders = if let Some(store) = stores.and_then(|s| s.reminder_store.as_ref()) {
            load_reminders(store)
        } else {
            SqliteReminderStore::open(cas_dir)
                .map(|store| {
                    store.init().ok();
                    load_reminders(&store)
                })
                .unwrap_or_default()
        };

        Ok(Self {
            ready_tasks,
            in_progress_tasks,
            epic_tasks,
            agents,
            activity,
            agent_id_to_name,
            changes,
            git_loaded,
            reminders,
            epic_closed_counts,
        })
    }

    /// Get all tasks (in_progress + ready) grouped by epic
    ///
    /// Returns (epic_groups, standalone_tasks) where:
    /// - epic_groups: Tasks grouped under their parent epic
    /// - standalone_tasks: Tasks without a parent epic
    pub fn tasks_by_epic(&self) -> (Vec<EpicGroup>, Vec<TaskSummary>) {
        // Build a map of epic_id -> subtasks
        let mut epic_subtasks: HashMap<String, Vec<TaskSummary>> = HashMap::new();
        let mut standalone: Vec<TaskSummary> = Vec::new();

        // Collect all tasks (in_progress first, then ready)
        for task in self.in_progress_tasks.iter().chain(self.ready_tasks.iter()) {
            if let Some(epic_id) = &task.epic {
                epic_subtasks
                    .entry(epic_id.clone())
                    .or_default()
                    .push(task.clone());
            } else {
                standalone.push(task.clone());
            }
        }

        // Build epic groups from epic_tasks
        let mut groups: Vec<EpicGroup> = self
            .epic_tasks
            .iter()
            .filter_map(|epic| {
                let subtasks = epic_subtasks.remove(&epic.id).unwrap_or_default();
                // Only include epics that have subtasks in the current view
                if subtasks.is_empty() {
                    return None;
                }
                let has_active = subtasks.iter().any(|t| t.status == TaskStatus::InProgress);
                Some(EpicGroup {
                    epic: epic.clone(),
                    subtasks,
                    has_active,
                })
            })
            .collect();

        // Sort groups by recency of activity so the supervisor's CURRENT focus
        // surfaces at the top of the panel, rather than a stale epic that merely
        // happens to carry more subtasks or a higher static priority (cas-2fb6).
        //
        // Recency = newest `updated_at` across the epic row and its subtasks.
        // This naturally follows focus: starting/closing/noting a subtask bumps
        // its `updated_at`, so the panel re-sorts within one refresh. Ties fall
        // back to the historical ordering (active-first, then epic priority,
        // then id) so single-epic and timing-less (test) data are unaffected.
        groups.sort_by(|a, b| {
            group_recency(b)
                .cmp(&group_recency(a))
                .then_with(|| a.has_active.cmp(&b.has_active).reverse())
                .then_with(|| a.epic.priority.0.cmp(&b.epic.priority.0))
                .then_with(|| a.epic.id.cmp(&b.epic.id))
        });

        // Standalone tasks: most-recently-updated first, then by priority, then
        // id, so a freshly-touched standalone task is not buried under older
        // ones either.
        standalone.sort_by(|a, b| {
            a.updated_at
                .cmp(&b.updated_at)
                .reverse()
                .then_with(|| a.priority.0.cmp(&b.priority.0))
                .then_with(|| a.id.cmp(&b.id))
        });

        (groups, standalone)
    }
}

/// Newest `updated_at` across an epic group (the epic row and all of its
/// subtasks). Used to order the TASKS panel by recency of activity so the
/// supervisor's current focus stays on top (cas-2fb6). Returns `None` only when
/// neither the epic nor any subtask carries timing information, in which case
/// the caller falls back to the historical active-first/priority ordering.
fn group_recency(group: &EpicGroup) -> Option<chrono::DateTime<chrono::Utc>> {
    std::iter::once(group.epic.updated_at)
        .chain(group.subtasks.iter().map(|t| t.updated_at))
        .flatten()
        .max()
}

/// Load pending + recently fired reminders from a reminder store.
fn load_reminders(store: &SqliteReminderStore) -> Vec<Reminder> {
    let init_result = store.init();
    if init_result.is_err() {
        return Vec::new();
    }
    let mut all = store.list_all_pending().unwrap_or_default();
    all.extend(store.list_recently_fired(60).unwrap_or_default());
    all
}

/// A repo to check for git changes
struct RepoToCheck {
    path: PathBuf,
    name: String,
    agent_name: Option<String>,
}

/// Load git changes from main repo and factory worktrees
///
/// Uses rayon for parallel git operations to support 1000+ workers.
fn load_all_git_changes(
    cas_dir: &Path,
    worktree_root: Option<&Path>,
    agent_id_to_name: &HashMap<String, String>,
    worktree_store: Option<&SqliteWorktreeStore>,
) -> anyhow::Result<Vec<SourceChangesInfo>> {
    use rayon::prelude::*;
    use std::collections::HashSet;

    let repo_root = cas_dir.parent().unwrap_or(cas_dir);

    // Collect all repos to check (fast, no I/O)
    let mut repos_to_check: Vec<RepoToCheck> = Vec::new();
    // Track paths to deduplicate between DB worktrees and filesystem scan
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();

    // 1. Main repo
    seen_paths.insert(repo_root.to_path_buf());
    repos_to_check.push(RepoToCheck {
        path: repo_root.to_path_buf(),
        name: "main".to_string(),
        agent_name: None,
    });

    // 2. Worktrees (from CAS database)
    let owned_wt_store;
    let wt_store = if let Some(s) = worktree_store {
        Some(s)
    } else {
        owned_wt_store = SqliteWorktreeStore::open(cas_dir).ok();
        owned_wt_store.as_ref()
    };
    if let Some(store) = wt_store
        && let Ok(worktrees) = store.list_by_status(WorktreeStatus::Active)
    {
        for wt in worktrees {
            let path = PathBuf::from(&wt.path);
            if !seen_paths.insert(path.clone()) {
                continue; // Already tracked
            }
            let name = wt
                .branch
                .split('/')
                .next_back()
                .unwrap_or(&wt.branch)
                .to_string();
            let agent_name = wt
                .created_by_agent
                .as_ref()
                .and_then(|id| agent_id_to_name.get(id).cloned());
            repos_to_check.push(RepoToCheck {
                path,
                name,
                agent_name,
            });
        }
    }

    // 3. Worktree directories (for factory workers) - only check worktrees matching active agents
    if let Some(wt_dir) = worktree_root
        && wt_dir.exists()
        && wt_dir.is_dir()
    {
        for agent_name in agent_id_to_name.values() {
            let path = wt_dir.join(agent_name);
            if path.is_dir() && path.join(".git").exists() && seen_paths.insert(path.clone()) {
                repos_to_check.push(RepoToCheck {
                    path,
                    name: agent_name.clone(),
                    agent_name: Some(agent_name.clone()),
                });
            }
        }
    }

    // Process all repos in parallel using rayon
    let mut sources: Vec<SourceChangesInfo> = repos_to_check
        .into_par_iter()
        .filter_map(|repo| get_source_changes(&repo.path, &repo.name, repo.agent_name))
        .collect();

    // Sort by total changes descending
    sources.sort_by(|a, b| {
        let a_total = a.total_added + a.total_removed;
        let b_total = b.total_added + b.total_removed;
        b_total.cmp(&a_total)
    });

    Ok(sources)
}

/// Get git changes for a single source directory
fn get_source_changes(
    path: &Path,
    name: &str,
    agent_name: Option<String>,
) -> Option<SourceChangesInfo> {
    if !path.exists() {
        return None;
    }

    let changes = get_git_changes(path);
    if changes.is_empty() {
        return None;
    }

    let total_added: usize = changes.iter().map(|c| c.lines_added).sum();
    let total_removed: usize = changes.iter().map(|c| c.lines_removed).sum();

    Some(SourceChangesInfo {
        source_name: name.to_string(),
        source_path: path.to_path_buf(),
        agent_name,
        changes,
        total_added,
        total_removed,
    })
}

/// Get git changes for a directory
fn get_git_changes(repo_path: &Path) -> Vec<FileChangeInfo> {
    // Run git status --porcelain
    let status_output = match Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_path)
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    if !status_output.status.success() {
        return Vec::new();
    }

    let status_str = String::from_utf8_lossy(&status_output.stdout);

    // Get line counts from combined staged + unstaged diff against HEAD
    let mut line_counts: HashMap<String, (usize, usize)> = HashMap::new();

    if let Ok(output) = Command::new("git")
        .args(["diff", "HEAD", "--numstat"])
        .current_dir(repo_path)
        .output()
        && output.status.success()
    {
        parse_diff_numstat(&String::from_utf8_lossy(&output.stdout), &mut line_counts);
    }

    // Parse status output
    let mut changes: Vec<FileChangeInfo> = Vec::new();

    for line in status_str.lines() {
        if line.len() < 3 {
            continue;
        }

        let status_code = &line[0..2];
        let file_path = line[3..].trim().to_string();

        // Handle renamed files
        let file_path = if file_path.contains(" -> ") {
            file_path
                .split(" -> ")
                .last()
                .unwrap_or(&file_path)
                .to_string()
        } else {
            file_path
        };

        let status = match status_code {
            "M " | " M" | "MM" | "AM" => GitFileStatus::Modified,
            "A " | " A" => GitFileStatus::Added,
            "D " | " D" => GitFileStatus::Deleted,
            "R " | " R" => GitFileStatus::Renamed,
            "??" => GitFileStatus::Untracked,
            _ => continue,
        };

        let first_char = status_code.chars().next().unwrap_or(' ');
        let staged = first_char != ' ' && first_char != '?';

        // Get line counts from diff (untracked files won't appear in diff, use 0)
        let (lines_added, lines_removed) = line_counts.get(&file_path).copied().unwrap_or((0, 0));

        changes.push(FileChangeInfo {
            file_path,
            lines_added,
            lines_removed,
            status,
            staged,
        });
    }

    // Sort by status then path
    changes.sort_by(|a, b| {
        let status_order = |s: &GitFileStatus| match s {
            GitFileStatus::Modified => 0,
            GitFileStatus::Added => 1,
            GitFileStatus::Deleted => 2,
            GitFileStatus::Renamed => 3,
            GitFileStatus::Untracked => 4,
        };
        status_order(&a.status)
            .cmp(&status_order(&b.status))
            .then_with(|| a.file_path.cmp(&b.file_path))
    });

    changes
}

/// Parse git diff --numstat output
fn parse_diff_numstat(output: &str, line_counts: &mut HashMap<String, (usize, usize)>) {
    for line in output.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(added_s), Some(removed_s), Some(file_s)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let added = added_s.parse().unwrap_or(0);
        let removed = removed_s.parse().unwrap_or(0);
        let entry = line_counts.entry(file_s.to_string()).or_insert((0, 0));
        entry.0 += added;
        entry.1 += removed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{PendingReason, TaskStore};
    use cas_types::{Agent, EventEntityType, WorkTarget};
    use chrono::{Duration, Utc};

    #[test]
    fn task_note_activity_counts_as_recent_worker_activity() {
        let temp_dir = tempfile::tempdir().unwrap();
        let stores = DirectorStores::open(temp_dir.path()).unwrap();
        stores.task_store.init().unwrap();
        stores.agent_store.init().unwrap();
        stores.event_store.init().unwrap();

        let now = Utc::now();
        let worker_id = "worker-session-1";
        let worker_name = "lively-worker-1";

        let mut worker = Agent::new_with_role(
            worker_id.to_string(),
            worker_name.to_string(),
            AgentRole::Worker,
        );
        worker.last_heartbeat = now;
        stores.agent_store.register(&worker).unwrap();

        let mut task = Task::new("cas-note-activity".to_string(), "Note activity".to_string());
        task.status = TaskStatus::InProgress;
        task.assignee = Some(worker_name.to_string());
        task.updated_at = now;
        stores.task_store.add(&task).unwrap();

        let mut stale_file_event = Event::new(
            EventType::WorkerFileEdited,
            EventEntityType::Task,
            &task.id,
            "old file edit",
        )
        .with_session(worker_id);
        stale_file_event.created_at =
            now - Duration::seconds(crate::config::DEFAULT_STALL_THRESHOLD_SECS as i64 + 30);
        stores.event_store.record(&stale_file_event).unwrap();

        let mut recent_note_event = Event::new(
            EventType::TaskNoteAdded,
            EventEntityType::Task,
            &task.id,
            "recent progress note",
        )
        .with_session(worker_id);
        recent_note_event.created_at =
            now - Duration::seconds(crate::config::DEFAULT_STALL_THRESHOLD_SECS as i64 / 2);
        stores.event_store.record(&recent_note_event).unwrap();

        let data =
            DirectorData::load_with_stores(temp_dir.path(), None, false, Some(&stores)).unwrap();
        let worker_summary = data
            .agents
            .iter()
            .find(|agent| agent.id == worker_id)
            .expect("worker is present in director data");

        let (summary, latest_activity_at) = worker_summary
            .latest_activity
            .as_ref()
            .expect("task notes are loaded as worker activity");
        assert_eq!(summary, "recent progress note");
        assert_eq!(*latest_activity_at, recent_note_event.created_at);

        let activity_age = now.signed_duration_since(*latest_activity_at).num_seconds();
        assert!(
            activity_age < crate::config::DEFAULT_STALL_THRESHOLD_SECS as i64,
            "recent task-note activity must keep the stall predicate below threshold"
        );
    }

    #[test]
    fn task_summary_projects_declared_work_target_branch() {
        let temp_dir = tempfile::tempdir().unwrap();
        let stores = DirectorStores::open(temp_dir.path()).unwrap();
        stores.task_store.init().unwrap();
        stores.agent_store.init().unwrap();
        stores.event_store.init().unwrap();

        let mut task = Task::new("cas-work-target".to_string(), "Declared target".to_string());
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: "remote:example.invalid/acme/cas".to_string(),
            target_branch: "main".to_string(),
        });
        stores.task_store.add(&task).unwrap();

        let data =
            DirectorData::load_with_stores(temp_dir.path(), None, false, Some(&stores)).unwrap();
        assert_eq!(
            data.in_progress_tasks[0].branch.as_deref(),
            Some("main"),
            "the director relay must receive the WorkTarget branch rather than infer a focus"
        );
    }

    #[test]
    fn backed_off_prompt_is_not_counted_as_immediately_deliverable() {
        let temp_dir = tempfile::tempdir().unwrap();
        let stores = DirectorStores::open(temp_dir.path()).unwrap();
        stores.task_store.init().unwrap();
        stores.agent_store.init().unwrap();
        stores.event_store.init().unwrap();
        let prompt_queue = stores.prompt_queue_store.as_ref().unwrap();
        prompt_queue.init().unwrap();

        let supervisor = Agent::new_with_role(
            "supervisor-session".to_string(),
            "supervisor".to_string(),
            AgentRole::Supervisor,
        );
        stores.agent_store.register(&supervisor).unwrap();
        let worker = Agent::new_with_role(
            "worker-session".to_string(),
            "backed-off-worker".to_string(),
            AgentRole::Worker,
        );
        stores.agent_store.register(&worker).unwrap();

        let prompt_id = prompt_queue
            .enqueue("supervisor", "backed-off-worker", "assigned work")
            .unwrap();
        for _ in 0..6 {
            prompt_queue
                .record_retry(
                    prompt_id,
                    PendingReason::TargetUnavailable,
                    Some("pane unavailable"),
                )
                .unwrap();
        }
        assert!(
            prompt_queue
                .peek_for_targets(&["backed-off-worker"], None, 10)
                .unwrap()
                .is_empty(),
            "delivery selection must not immediately retry the row"
        );

        let data =
            DirectorData::load_with_stores(temp_dir.path(), None, false, Some(&stores)).unwrap();
        let worker_summary = data
            .agents
            .iter()
            .find(|agent| agent.id == "worker-session")
            .expect("worker is present in director data");

        assert_eq!(
            worker_summary.pending_messages, 0,
            "the idle detector must not treat a 5s retry as immediately deliverable"
        );
        assert_eq!(
            worker_summary.pending_supervisor_messages, 0,
            "a backed-off supervisor row must not permanently latch the idle transition"
        );
    }
}
