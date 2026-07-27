//! Task type definitions
//!
//! Tasks are work items tracked by CAS

// Dead code check enabled - all items used

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::TypeError;
use crate::scope::Scope;

/// Status of a task in its lifecycle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Not yet started
    #[default]
    Open,
    /// Currently being worked on
    InProgress,
    /// Waiting on something
    Blocked,
    /// Completed
    Closed,
    /// Worker close ran the lightweight gate successfully; awaiting
    /// supervisor code-review dispatch. Only reachable when
    /// `[code_review] owner = "supervisor"` is set (cas-b51a).
    PendingSupervisorReview,
    /// Worker close reached the merge-state guard and the worker's factory
    /// branch is not on the target branch yet. The worker has no further
    /// action until the supervisor merges the branch, but the task is still
    /// open and closeable once the merge guard passes.
    AwaitingMerge,
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskStatus::Open => write!(f, "open"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::Blocked => write!(f, "blocked"),
            TaskStatus::Closed => write!(f, "closed"),
            TaskStatus::PendingSupervisorReview => write!(f, "pending_supervisor_review"),
            TaskStatus::AwaitingMerge => write!(f, "awaiting_merge"),
        }
    }
}

impl FromStr for TaskStatus {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("open") {
            Ok(TaskStatus::Open)
        } else if s.eq_ignore_ascii_case("in_progress")
            || s.eq_ignore_ascii_case("in-progress")
            || s.eq_ignore_ascii_case("inprogress")
        {
            Ok(TaskStatus::InProgress)
        } else if s.eq_ignore_ascii_case("blocked") {
            Ok(TaskStatus::Blocked)
        } else if s.eq_ignore_ascii_case("closed") {
            Ok(TaskStatus::Closed)
        } else if s.eq_ignore_ascii_case("pending_supervisor_review")
            || s.eq_ignore_ascii_case("pending-supervisor-review")
        {
            Ok(TaskStatus::PendingSupervisorReview)
        } else if s.eq_ignore_ascii_case("awaiting_merge")
            || s.eq_ignore_ascii_case("awaiting-merge")
        {
            Ok(TaskStatus::AwaitingMerge)
        } else {
            Err(TypeError::InvalidTaskStatus(s.to_string()))
        }
    }
}

/// Type of task
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    /// Standard work item
    #[default]
    Task,
    /// Defect or problem
    Bug,
    /// New functionality
    Feature,
    /// Large work with subtasks
    Epic,
    /// Maintenance or cleanup
    Chore,
    /// Investigation or research (produces understanding, not code)
    Spike,
}

impl fmt::Display for TaskType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskType::Task => write!(f, "task"),
            TaskType::Bug => write!(f, "bug"),
            TaskType::Feature => write!(f, "feature"),
            TaskType::Epic => write!(f, "epic"),
            TaskType::Chore => write!(f, "chore"),
            TaskType::Spike => write!(f, "spike"),
        }
    }
}

impl FromStr for TaskType {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("task") {
            Ok(TaskType::Task)
        } else if s.eq_ignore_ascii_case("bug") {
            Ok(TaskType::Bug)
        } else if s.eq_ignore_ascii_case("feature") {
            Ok(TaskType::Feature)
        } else if s.eq_ignore_ascii_case("epic") {
            Ok(TaskType::Epic)
        } else if s.eq_ignore_ascii_case("chore") {
            Ok(TaskType::Chore)
        } else if s.eq_ignore_ascii_case("spike") {
            Ok(TaskType::Spike)
        } else {
            Err(TypeError::Parse(format!("invalid task type: {s}")))
        }
    }
}

/// Execution depth of a task (EPIC cas-1255 — per-task speed mode).
///
/// Controls the speed-vs-rigor tradeoff for feel-driven iteration. `Deep`
/// is the default and preserves full execution rigor; `Light` signals a
/// fast, feel-driven pass. Rows created before this field existed read back
/// as `Deep` (NULL maps to the default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskDepth {
    /// Full execution rigor (default)
    #[default]
    Deep,
    /// Fast, feel-driven pass
    Light,
}

impl fmt::Display for TaskDepth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskDepth::Deep => write!(f, "deep"),
            TaskDepth::Light => write!(f, "light"),
        }
    }
}

impl FromStr for TaskDepth {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("deep") {
            Ok(TaskDepth::Deep)
        } else if s.eq_ignore_ascii_case("light") {
            Ok(TaskDepth::Light)
        } else {
            Err(TypeError::Parse(format!("invalid task depth: {s}")))
        }
    }
}

/// Priority level (0 = highest, 4 = lowest)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct Priority(pub i32);

impl Priority {
    pub const CRITICAL: Priority = Priority(0);
    pub const HIGH: Priority = Priority(1);
    pub const MEDIUM: Priority = Priority(2);
    pub const LOW: Priority = Priority(3);
    pub const BACKLOG: Priority = Priority(4);

    pub fn label(&self) -> &'static str {
        match self.0 {
            0 => "P0 (critical)",
            1 => "P1 (high)",
            2 => "P2 (medium)",
            3 => "P3 (low)",
            _ => "P4 (backlog)",
        }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "P{}", self.0)
    }
}

impl From<i32> for Priority {
    fn from(v: i32) -> Self {
        Priority(v.clamp(0, 4))
    }
}

/// Deliverables captured when closing a task
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskDeliverables {
    /// Files changed (excluding deletions)
    #[serde(default)]
    pub files_changed: Vec<String>,
    /// Commit hash created during auto-commit (if any)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_hash: Option<String>,
    /// Merge commit hash for associated worktree (if any)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_commit: Option<String>,
    /// Persisted review envelope captured on verification-jail close so a later
    /// supervisor close can forward the prior code-review outcome without
    /// re-running the gate. Serialized as a JSON string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_envelope: Option<String>,
    /// cas-4b3f: the worker's `factory/<assignee>` branch commit sha, snapshotted
    /// the FIRST time the close-time merge-state guard (`run_factory_branch_merge_gate`)
    /// rejects this task with "MERGE REQUIRED". Anchors later merge-state checks to
    /// THIS task's own commit range instead of the branch's current HEAD.
    ///
    /// Without this anchor, a worker who starts a second task on the same
    /// `factory/<assignee>` branch before the first task's commits are merged
    /// re-strands the first task on every retry: branch HEAD now includes the
    /// second task's unmerged commits, so the gate can't tell "this task's own
    /// work is still unmerged" from "a *later* task's work is unmerged" and
    /// rejects the first task's close even after its commits landed on the
    /// parent branch. See BUG-close-guard-branch-head-not-task-commits.md.
    ///
    /// Stored as JSON inside the existing `deliverables` column (no migration
    /// needed — serde `default` keeps old rows deserializing as `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub factory_branch_anchor: Option<String>,

    /// cas-a844: the `factory/<assignee>` branch name recorded the first
    /// time a close is rejected with MERGE REQUIRED (same call site as
    /// `factory_branch_anchor`, but the branch NAME rather than a commit
    /// sha). `factory_branch_anchor` is only useful if you already know
    /// which branch to resolve it against; when the original assignee is
    /// lost (fleet restart, reassignment), the branch name itself is the
    /// only thing that still points at the orphaned commits. Never
    /// overwritten once set — see `park_task_awaiting_merge`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parked_branch: Option<String>,

    /// cas-a844: set when a supervisor `worktree_merge` attempt against this
    /// task's parked branch fails with an actual git merge conflict (as
    /// opposed to simply "not merged yet"). Distinguishes a clean
    /// `awaiting_merge` (mergeable, just queued for the supervisor) from a
    /// conflicted one (NOT complete, needs a worker to resolve it) — the two
    /// used to be indistinguishable in task status output.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub merge_conflicted: bool,
}

impl TaskDeliverables {
    pub fn is_empty(&self) -> bool {
        self.files_changed.is_empty()
            && self.commit_hash.is_none()
            && self.merge_commit.is_none()
            && self.review_envelope.is_none()
            && self.factory_branch_anchor.is_none()
            && self.parked_branch.is_none()
            && !self.merge_conflicted
    }
}

/// A task (work item) tracked by CAS
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique identifier (e.g., cas-a1b2)
    pub id: String,

    /// Storage scope (global or project)
    /// Project scope is the default for tasks
    #[serde(default)]
    pub scope: Scope,

    /// Task title
    pub title: String,

    /// Problem statement, context (immutable after creation)
    #[serde(default)]
    pub description: String,

    /// Technical approach, architecture decisions
    #[serde(default)]
    pub design: String,

    /// Concrete deliverables checklist
    #[serde(default)]
    pub acceptance_criteria: String,

    /// Session handoff notes (COMPLETED/IN_PROGRESS/NEXT)
    #[serde(default)]
    pub notes: String,

    /// Current status
    #[serde(default)]
    pub status: TaskStatus,

    /// Priority level (0-4)
    #[serde(default)]
    pub priority: Priority,

    /// Type of task
    #[serde(default)]
    pub task_type: TaskType,

    /// Who is working on this
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,

    /// Optional labels for categorization
    #[serde(default)]
    pub labels: Vec<String>,

    /// When the task was created
    pub created_at: DateTime<Utc>,

    /// When the task was last updated
    pub updated_at: DateTime<Utc>,

    /// When the task was closed (if closed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,

    /// Why the task was closed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_reason: Option<String>,

    /// Link to external tracker (GitHub, JIRA, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_ref: Option<String>,

    /// Content hash for deduplication
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,

    /// Git branch this task is scoped to (None = visible from all branches)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,

    /// Worktree this task was created in (for auto-cleanup)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,

    /// Whether this task is awaiting verification before close
    /// When true, the agent is "jailed" - only task-verifier can run
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending_verification: bool,

    /// Whether this task (epic) is awaiting worktree merge before close
    /// When true, the agent is "jailed" - only worktree-merger can run
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending_worktree_merge: bool,

    /// Agent ID responsible for epic verification (supervisor in factory mode)
    /// When set, this agent (not the task closer) gets jailed for epic verification
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epic_verification_owner: Option<String>,

    /// Task deliverables captured on close
    #[serde(default, skip_serializing_if = "TaskDeliverables::is_empty")]
    pub deliverables: TaskDeliverables,

    /// Team ID this task belongs to (None = personal/not shared with team)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,

    /// Per-task team-promotion override (T5). See `Rule.share` for
    /// semantics. Dormant — no CLI currently writes this field for
    /// tasks — but present to match Entry's shape end-to-end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<crate::scope::ShareScope>,

    /// What can be demonstrated when this task is complete
    /// e.g., "Type a query, results filter live"
    #[serde(default)]
    pub demo_statement: String,

    /// Execution methodology for this task. One of `test-first`,
    /// `characterization-first`, or `additive-only`. Validated at the MCP
    /// tool layer rather than the database. None = no methodology declared.
    /// See cas-7fc1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_note: Option<String>,

    /// Execution depth (EPIC cas-1255). `Deep` (default) preserves full
    /// rigor; `Light` signals a fast, feel-driven pass. Defaults to `Deep`
    /// when absent so existing tasks read as deep.
    #[serde(default)]
    pub depth: TaskDepth,
}

impl Task {
    /// Create a new task with the given ID and title
    pub fn new(id: String, title: String) -> Self {
        let now = Utc::now();
        Self {
            id,
            scope: Scope::default(), // Project scope by default
            title,
            description: String::new(),
            design: String::new(),
            acceptance_criteria: String::new(),
            notes: String::new(),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            task_type: TaskType::Task,
            assignee: None,
            labels: Vec::new(),
            created_at: now,
            updated_at: now,
            closed_at: None,
            close_reason: None,
            external_ref: None,
            content_hash: None,
            branch: None,
            worktree_id: None,
            pending_verification: false,
            pending_worktree_merge: false,
            epic_verification_owner: None,
            deliverables: TaskDeliverables::default(),
            team_id: None,
            share: None,
            demo_statement: String::new(),
            execution_note: None,
            depth: TaskDepth::Deep,
        }
    }

    /// Create a new task with a specific scope
    pub fn new_with_scope(id: String, title: String, scope: Scope) -> Self {
        let mut task = Self::new(id, title);
        task.scope = scope;
        task
    }

    /// Check if the task is open (not closed)
    pub fn is_open(&self) -> bool {
        self.status != TaskStatus::Closed
    }

    /// Check if the task is ready to work on. Waiting states like
    /// PendingSupervisorReview and AwaitingMerge are intentionally excluded:
    /// the task cannot be picked up again by a worker until the supervisor
    /// resolves the waiting condition.
    pub fn is_ready(&self) -> bool {
        self.status == TaskStatus::Open
    }

    /// Get a short preview of the title
    pub fn preview(&self, max_len: usize) -> String {
        let char_count = self.title.chars().count();
        if char_count <= max_len {
            self.title.clone()
        } else {
            let truncated: String = self.title.chars().take(max_len.saturating_sub(3)).collect();
            format!("{truncated}...")
        }
    }
}

impl Default for Task {
    fn default() -> Self {
        Self {
            id: String::new(),
            scope: Scope::default(),
            title: String::new(),
            description: String::new(),
            design: String::new(),
            acceptance_criteria: String::new(),
            notes: String::new(),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            task_type: TaskType::Task,
            assignee: None,
            labels: Vec::new(),
            created_at: DateTime::<Utc>::default(),
            updated_at: DateTime::<Utc>::default(),
            closed_at: None,
            close_reason: None,
            external_ref: None,
            content_hash: None,
            branch: None,
            worktree_id: None,
            pending_verification: false,
            pending_worktree_merge: false,
            epic_verification_owner: None,
            deliverables: TaskDeliverables::default(),
            team_id: None,
            share: None,
            demo_statement: String::new(),
            execution_note: None,
            depth: TaskDepth::Deep,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::task::*;

    #[test]
    fn test_task_status_from_str() {
        assert_eq!(TaskStatus::from_str("open").unwrap(), TaskStatus::Open);
        assert_eq!(
            TaskStatus::from_str("in_progress").unwrap(),
            TaskStatus::InProgress
        );
        assert_eq!(
            TaskStatus::from_str("in-progress").unwrap(),
            TaskStatus::InProgress
        );
        assert_eq!(
            TaskStatus::from_str("blocked").unwrap(),
            TaskStatus::Blocked
        );
        assert_eq!(TaskStatus::from_str("closed").unwrap(), TaskStatus::Closed);
        assert_eq!(
            TaskStatus::from_str("pending_supervisor_review").unwrap(),
            TaskStatus::PendingSupervisorReview
        );
        assert_eq!(
            TaskStatus::from_str("pending-supervisor-review").unwrap(),
            TaskStatus::PendingSupervisorReview
        );
        assert_eq!(
            TaskStatus::from_str("awaiting_merge").unwrap(),
            TaskStatus::AwaitingMerge
        );
        assert_eq!(
            TaskStatus::from_str("awaiting-merge").unwrap(),
            TaskStatus::AwaitingMerge
        );
        assert!(TaskStatus::from_str("invalid").is_err());
    }

    #[test]
    fn test_pending_supervisor_review_display_roundtrip() {
        let s = TaskStatus::PendingSupervisorReview.to_string();
        assert_eq!(s, "pending_supervisor_review");
        assert_eq!(
            TaskStatus::from_str(&s).unwrap(),
            TaskStatus::PendingSupervisorReview
        );
    }

    #[test]
    fn test_pending_supervisor_review_is_open_not_ready() {
        let mut task = Task::new("cas-test".to_string(), "Test".to_string());
        task.status = TaskStatus::PendingSupervisorReview;
        // Still "open" (not closed) so dependents remain unblocked logic is sensible
        assert!(task.is_open());
        // But NOT ready — worker should not pick it up again until supervisor decides
        assert!(!task.is_ready());
    }

    #[test]
    fn test_awaiting_merge_display_roundtrip() {
        let s = TaskStatus::AwaitingMerge.to_string();
        assert_eq!(s, "awaiting_merge");
        assert_eq!(TaskStatus::from_str(&s).unwrap(), TaskStatus::AwaitingMerge);
    }

    #[test]
    fn test_awaiting_merge_is_open_not_ready() {
        let mut task = Task::new("cas-test".to_string(), "Test".to_string());
        task.status = TaskStatus::AwaitingMerge;
        assert!(task.is_open());
        assert!(!task.is_ready());
    }

    #[test]
    fn test_task_type_from_str() {
        assert_eq!(TaskType::from_str("task").unwrap(), TaskType::Task);
        assert_eq!(TaskType::from_str("bug").unwrap(), TaskType::Bug);
        assert_eq!(TaskType::from_str("feature").unwrap(), TaskType::Feature);
        assert_eq!(TaskType::from_str("epic").unwrap(), TaskType::Epic);
        assert_eq!(TaskType::from_str("chore").unwrap(), TaskType::Chore);
        assert_eq!(TaskType::from_str("spike").unwrap(), TaskType::Spike);
    }

    #[test]
    fn test_spike_task_type() {
        let spike = TaskType::Spike;
        assert_eq!(spike.to_string(), "spike");
        assert_eq!(TaskType::from_str("spike").unwrap(), TaskType::Spike);

        // Verify round-trip
        let s = spike.to_string();
        assert_eq!(TaskType::from_str(&s).unwrap(), TaskType::Spike);
    }

    #[test]
    fn test_priority() {
        assert!(Priority::CRITICAL < Priority::HIGH);
        assert!(Priority::HIGH < Priority::MEDIUM);
        assert_eq!(Priority::from(5), Priority(4)); // Clamped to max
        assert_eq!(Priority::from(-1), Priority(0)); // Clamped to min
    }

    #[test]
    fn test_task_new() {
        let task = Task::new("cas-a1b2".to_string(), "Test task".to_string());
        assert_eq!(task.id, "cas-a1b2");
        assert_eq!(task.title, "Test task");
        assert_eq!(task.status, TaskStatus::Open);
        assert_eq!(task.priority, Priority::MEDIUM);
        assert!(task.is_open());
        assert!(task.is_ready());
    }

    #[test]
    fn test_task_depth_default_is_deep() {
        assert_eq!(TaskDepth::default(), TaskDepth::Deep);
        let task = Task::new("cas-d3p1".to_string(), "Test".to_string());
        assert_eq!(task.depth, TaskDepth::Deep);
        assert_eq!(Task::default().depth, TaskDepth::Deep);
    }

    #[test]
    fn test_task_depth_from_str() {
        assert_eq!(TaskDepth::from_str("deep").unwrap(), TaskDepth::Deep);
        assert_eq!(TaskDepth::from_str("light").unwrap(), TaskDepth::Light);
        assert_eq!(TaskDepth::from_str("LIGHT").unwrap(), TaskDepth::Light);
        assert!(TaskDepth::from_str("medium").is_err());
        assert!(TaskDepth::from_str("").is_err());
    }

    #[test]
    fn test_task_depth_display_roundtrip() {
        for d in [TaskDepth::Deep, TaskDepth::Light] {
            assert_eq!(TaskDepth::from_str(&d.to_string()).unwrap(), d);
        }
        assert_eq!(TaskDepth::Light.to_string(), "light");
        assert_eq!(TaskDepth::Deep.to_string(), "deep");
    }
}
