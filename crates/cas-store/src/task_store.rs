//! SQLite-based task storage

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::error::StoreError;
use crate::event_store::record_event_with_conn;
use crate::recording_store::capture_task_event;
use crate::{Result, TaskStore};
use cas_types::{
    Dependency, DependencyType, Event, EventEntityType, EventType, Priority, RecordingEventType,
    Scope, Task, TaskDeliverables, TaskStatus, TaskType,
};

/// SQLite DDL for the `tasks` and `dependencies` tables.
///
/// Re-exported via `cas_store::TASK_SCHEMA` so the migration runner in
/// `cas-cli` can bootstrap the base tables before applying ALTER migrations.
/// See cas-bdb9 / EPIC cas-9fdb.
///
/// NOTE: `task_leases` lives exclusively in `AGENT_SCHEMA` (the agent
/// lifecycle owns lease semantics — FK to `agents(id)` with cascade delete,
/// `renewed_at NOT NULL`). A redundant slim `task_leases` definition used
/// to live here too; it was removed in cas-bdb9 fix-round-1 because the
/// `IF NOT EXISTS` no-op silently dropped the NOT-NULL + FK on
/// fresh-bootstrap DBs where `Subsystem::Tasks` ran before
/// `Subsystem::Agents`.
pub const TASK_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    design TEXT NOT NULL DEFAULT '',
    acceptance_criteria TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'open',
    priority INTEGER NOT NULL DEFAULT 2,
    task_type TEXT NOT NULL DEFAULT 'task',
    assignee TEXT,
    labels TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    closed_at TEXT,
    close_reason TEXT,
    external_ref TEXT,
    content_hash TEXT,
    branch TEXT,
    worktree_id TEXT,
    pending_verification INTEGER NOT NULL DEFAULT 0,
    pending_worktree_merge INTEGER NOT NULL DEFAULT 0,
    epic_verification_owner TEXT,
    team_id TEXT,
    deliverables TEXT NOT NULL DEFAULT '{}',
    demo_statement TEXT NOT NULL DEFAULT '',
    execution_note TEXT,
    owner_id TEXT,
    visibility TEXT NOT NULL DEFAULT 'private',
    collaborators TEXT NOT NULL DEFAULT '[]',
    share TEXT,
    depth TEXT
);

CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS idx_tasks_priority ON tasks(priority);
CREATE INDEX IF NOT EXISTS idx_tasks_created ON tasks(created_at DESC);

CREATE TABLE IF NOT EXISTS dependencies (
    from_id TEXT NOT NULL,
    to_id TEXT NOT NULL,
    dep_type TEXT NOT NULL DEFAULT 'blocks',
    created_at TEXT NOT NULL,
    created_by TEXT,
    PRIMARY KEY (from_id, to_id)
);

CREATE INDEX IF NOT EXISTS idx_deps_from ON dependencies(from_id);
CREATE INDEX IF NOT EXISTS idx_deps_to ON dependencies(to_id);
CREATE INDEX IF NOT EXISTS idx_deps_type ON dependencies(dep_type);
"#;

/// SQLite-based task store
pub struct SqliteTaskStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteTaskStore {
    /// Open or create a SQLite task store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        Ok(Self { conn })
    }

    fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
            return Some(Utc.from_utc_datetime(&dt));
        }
        None
    }

    fn parse_labels(s: &str) -> Vec<String> {
        if s.is_empty() || s == "[]" {
            return Vec::new();
        }
        serde_json::from_str(s).unwrap_or_default()
    }

    fn parse_deliverables(s: &str) -> TaskDeliverables {
        if s.is_empty() || s == "{}" {
            return TaskDeliverables::default();
        }
        serde_json::from_str(s).unwrap_or_default()
    }

    fn labels_to_string(labels: &[String]) -> String {
        if labels.is_empty() {
            "[]".to_string()
        } else {
            serde_json::to_string(labels).unwrap_or_else(|_| "[]".to_string())
        }
    }

    fn deliverables_to_string(deliverables: &TaskDeliverables) -> String {
        serde_json::to_string(deliverables).unwrap_or_else(|_| "{}".to_string())
    }

    /// Generate a hash-based ID like cas-a1b2
    fn generate_hash_id(&self) -> Result<String> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        Utc::now().timestamp_nanos_opt().hash(&mut hasher);
        std::process::id().hash(&mut hasher);

        let hash = hasher.finish();
        let chars: Vec<char> = format!("{hash:016x}").chars().collect();

        // Try 4-char, then 5-char, then 6-char IDs
        let conn = self.conn.lock().unwrap();
        for len in 4..=6 {
            let id = format!("cas-{}", chars[..len].iter().collect::<String>());
            let exists: bool = conn
                .query_row("SELECT 1 FROM tasks WHERE id = ?", params![&id], |_| {
                    Ok(true)
                })
                .optional()?
                .unwrap_or(false);

            if !exists {
                return Ok(id);
            }
        }

        // Fallback to full hash
        Ok(format!("cas-{}", &chars[..8].iter().collect::<String>()))
    }

    fn task_from_row(row: &rusqlite::Row) -> rusqlite::Result<Task> {
        Ok(Task {
            id: row.get(0)?,
            scope: Scope::Project, // Tasks in project database are project-scoped
            title: row.get(1)?,
            description: row.get::<_, String>(2)?,
            design: row.get::<_, String>(3)?,
            acceptance_criteria: row.get::<_, String>(4)?,
            notes: row.get::<_, String>(5)?,
            status: row.get::<_, String>(6)?.parse().unwrap_or(TaskStatus::Open),
            priority: Priority(row.get::<_, i32>(7)?),
            task_type: row.get::<_, String>(8)?.parse().unwrap_or(TaskType::Task),
            assignee: row.get(9)?,
            labels: Self::parse_labels(&row.get::<_, String>(10)?),
            created_at: Self::parse_datetime(&row.get::<_, String>(11)?).unwrap_or_else(Utc::now),
            updated_at: Self::parse_datetime(&row.get::<_, String>(12)?).unwrap_or_else(Utc::now),
            closed_at: row
                .get::<_, Option<String>>(13)?
                .and_then(|s| Self::parse_datetime(&s)),
            close_reason: row.get(14)?,
            external_ref: row.get(15)?,
            content_hash: row.get(16)?,
            branch: row.get(17)?,
            worktree_id: row.get(18)?,
            pending_verification: row.get::<_, i32>(19).unwrap_or(0) == 1,
            pending_worktree_merge: row.get::<_, i32>(20).unwrap_or(0) == 1,
            epic_verification_owner: row.get(21)?,
            team_id: row.get(22)?,
            deliverables: Self::parse_deliverables(&row.get::<_, String>(23)?),
            demo_statement: row.get::<_, String>(24)?,
            execution_note: row.get::<_, Option<String>>(25)?,
            share: row
                .get::<_, Option<String>>(26)?
                .as_deref()
                .and_then(|s| s.parse().ok()),
            depth: row
                .get::<_, Option<String>>(27)?
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
        })
    }

    fn dep_from_row(row: &rusqlite::Row) -> rusqlite::Result<Dependency> {
        Ok(Dependency {
            from_id: row.get(0)?,
            to_id: row.get(1)?,
            dep_type: row
                .get::<_, String>(2)?
                .parse()
                .unwrap_or(DependencyType::Blocks),
            created_at: Self::parse_datetime(&row.get::<_, String>(3)?).unwrap_or_else(Utc::now),
            created_by: row.get(4)?,
        })
    }

    fn validate_task_exists_with_conn(conn: &Connection, task_id: &str) -> Result<()> {
        let exists: bool = conn
            .query_row("SELECT 1 FROM tasks WHERE id = ?", params![task_id], |_| {
                Ok(true)
            })
            .optional()?
            .unwrap_or(false);
        if exists {
            Ok(())
        } else {
            Err(StoreError::TaskNotFound(task_id.to_string()))
        }
    }

    fn add_with_conn(conn: &Connection, task: &Task) -> Result<()> {
        conn.execute(
            "INSERT INTO tasks (id, title, description, design, acceptance_criteria, notes,
             status, priority, task_type, assignee, labels, created_at, updated_at,
             closed_at, close_reason, external_ref, content_hash, branch, worktree_id,
             pending_verification, pending_worktree_merge, epic_verification_owner, team_id, deliverables, demo_statement, execution_note, share, depth)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)",
            params![
                task.id,
                task.title,
                task.description,
                task.design,
                task.acceptance_criteria,
                task.notes,
                task.status.to_string(),
                task.priority.0,
                task.task_type.to_string(),
                task.assignee,
                Self::labels_to_string(&task.labels),
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339(),
                task.closed_at.map(|t| t.to_rfc3339()),
                task.close_reason,
                task.external_ref,
                task.content_hash,
                task.branch,
                task.worktree_id,
                if task.pending_verification { 1 } else { 0 },
                if task.pending_worktree_merge { 1 } else { 0 },
                task.epic_verification_owner,
                task.team_id,
                Self::deliverables_to_string(&task.deliverables),
                task.demo_statement,
                task.execution_note,
                task.share.as_ref().map(|s| s.to_string()),
                task.depth.to_string(),
            ],
        )?;

        // Record event for sidecar activity feed
        let event = Event::new(
            EventType::TaskCreated,
            EventEntityType::Task,
            &task.id,
            format!("Task created: {}", task.title),
        );
        let _ = record_event_with_conn(conn, &event);

        // Capture event for recording playback
        let _ = capture_task_event(conn, RecordingEventType::TaskCreated, &task.id, None);

        Ok(())
    }

    fn add_dependency_with_conn(
        conn: &Connection,
        dep: &Dependency,
        check_cycle: bool,
    ) -> Result<()> {
        Self::validate_task_exists_with_conn(conn, &dep.from_id)?;
        Self::validate_task_exists_with_conn(conn, &dep.to_id)?;

        // Cycle checks only apply to "blocks" edges.
        // Uses a single recursive CTE instead of iterative BFS with N queries.
        if check_cycle && dep.dep_type == DependencyType::Blocks {
            let would_cycle: bool = conn.query_row(
                "WITH RECURSIVE reachable(node) AS (
                         SELECT ?1
                         UNION
                         SELECT d.to_id FROM dependencies d
                         JOIN reachable r ON d.from_id = r.node
                         WHERE d.dep_type = 'blocks'
                     )
                     SELECT COUNT(*) FROM reachable WHERE node = ?2",
                params![&dep.to_id, &dep.from_id],
                |row| Ok(row.get::<_, i64>(0)? > 0),
            )?;

            if would_cycle {
                return Err(StoreError::Parse(format!(
                    "adding dependency {} -> {} would create a cycle",
                    dep.from_id, dep.to_id
                )));
            }
        }

        conn.execute(
            "INSERT OR REPLACE INTO dependencies (from_id, to_id, dep_type, created_at, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                dep.from_id,
                dep.to_id,
                dep.dep_type.to_string(),
                dep.created_at.to_rfc3339(),
                dep.created_by,
            ],
        )?;
        Ok(())
    }

    /// Atomically load all tasks and parent-child dependencies in a single lock hold.
    ///
    /// Prevents read skew where a task exists but its epic link is invisible (or vice
    /// versa) due to a concurrent write between two separate queries.
    pub fn list_with_parent_deps(&self) -> Result<(Vec<Task>, Vec<Dependency>)> {
        let conn = self.conn.lock().unwrap();

        // Both queries run under the same mutex hold, guaranteeing a consistent snapshot
        let tasks = {
            let mut stmt = conn.prepare_cached(
                "SELECT id, title, description, design, acceptance_criteria, notes,
                 status, priority, task_type, assignee, labels, created_at, updated_at,
                 closed_at, close_reason, external_ref, content_hash, branch, worktree_id,
                 pending_verification, pending_worktree_merge, epic_verification_owner, team_id, deliverables, demo_statement, execution_note, share, depth
                 FROM tasks ORDER BY priority, created_at DESC",
            )?;
            stmt.query_map([], Self::task_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        let deps = {
            let dep_type_str = DependencyType::ParentChild.to_string();
            let mut stmt = conn.prepare_cached(
                "SELECT from_id, to_id, dep_type, created_at, created_by
                 FROM dependencies WHERE dep_type = ? ORDER BY created_at DESC",
            )?;
            stmt.query_map(params![dep_type_str], Self::dep_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        Ok((tasks, deps))
    }
}

/// Clear the pending_verification flag on a task using an existing connection.
///
/// For use in cross-store transactions (e.g., atomic unjail where verification
/// record and flag clear must happen in the same transaction).
pub fn clear_pending_verification_with_conn(conn: &Connection, task_id: &str) -> Result<()> {
    let now = Utc::now();
    let rows = conn.execute(
        "UPDATE tasks SET pending_verification = 0, updated_at = ?1 WHERE id = ?2 AND pending_verification = 1",
        params![now.to_rfc3339(), task_id],
    )?;
    if rows == 0 {
        // Either task doesn't exist or flag was already cleared (idempotent)
        let exists: bool = conn
            .query_row("SELECT 1 FROM tasks WHERE id = ?", params![task_id], |_| {
                Ok(true)
            })
            .optional()?
            .unwrap_or(false);
        if !exists {
            return Err(StoreError::TaskNotFound(task_id.to_string()));
        }
    }
    Ok(())
}

impl TaskStore for SqliteTaskStore {
    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(TASK_SCHEMA)?;
        // `task_leases` is owned by the agent lifecycle (`AGENT_SCHEMA`), but
        // `SqliteTaskStore::delete` issues `DELETE FROM task_leases WHERE
        // task_id = ?` for cleanup. To keep the task store usable without
        // requiring callers to also have constructed `SqliteAgentStore`,
        // we run the agent schema here too. The full AGENT_SCHEMA is the
        // single source of truth — running it from both store inits is
        // idempotent (`CREATE TABLE IF NOT EXISTS`). See cas-bdb9
        // fix-round-1 P1 for why a duplicated slim definition is dangerous.
        conn.execute_batch(crate::AGENT_SCHEMA)?;
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        self.generate_hash_id()
    }

    fn add(&self, task: &Task) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            Self::add_with_conn(&conn, task)
        })
    }

    fn create_atomic(
        &self,
        task: &Task,
        blocked_by: &[String],
        epic_id: Option<&str>,
        created_by: Option<&str>,
    ) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            let now = Utc::now();
            let epic_id = epic_id.map(str::trim).filter(|id| !id.is_empty());

            if let Some(epic_id) = epic_id {
                let epic_type = tx
                    .query_row(
                        "SELECT task_type FROM tasks WHERE id = ?",
                        params![epic_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                match epic_type {
                    Some(task_type) if task_type == "epic" => {}
                    Some(task_type) => {
                        return Err(StoreError::Parse(format!(
                            "Task {epic_id} is not an epic (type: {task_type})"
                        )));
                    }
                    None => {
                        return Err(StoreError::TaskNotFound(epic_id.to_string()));
                    }
                }
            }

            Self::add_with_conn(&tx, task)?;

            for blocker_id in blocked_by
                .iter()
                .map(|id| id.trim())
                .filter(|id| !id.is_empty())
            {
                let dep = Dependency {
                    from_id: task.id.clone(),
                    to_id: blocker_id.to_string(),
                    dep_type: DependencyType::Blocks,
                    created_at: now,
                    created_by: created_by.map(ToString::to_string),
                };
                Self::add_dependency_with_conn(&tx, &dep, false)?;
            }

            if let Some(epic_id) = epic_id {
                let dep = Dependency {
                    from_id: task.id.clone(),
                    to_id: epic_id.to_string(),
                    dep_type: DependencyType::ParentChild,
                    created_at: now,
                    created_by: created_by.map(ToString::to_string),
                };
                Self::add_dependency_with_conn(&tx, &dep, false)?;
            }

            tx.commit()?;
            Ok(())
        }) // with_write_retry
    }

    fn get(&self, id: &str) -> Result<Task> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, title, description, design, acceptance_criteria, notes,
             status, priority, task_type, assignee, labels, created_at, updated_at,
             closed_at, close_reason, external_ref, content_hash, branch, worktree_id,
             pending_verification, pending_worktree_merge, epic_verification_owner, team_id, deliverables, demo_statement, execution_note, share, depth
             FROM tasks WHERE id = ?",
            params![id],
            Self::task_from_row,
        )
        .optional()?
        .ok_or_else(|| StoreError::TaskNotFound(id.to_string()))
    }

    fn update(&self, task: &Task) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();

            // Combine the status read with the UPDATE: only SELECT the old status
            // when the new status differs from what's in the DB, avoiding the
            // pre-read on the common case where status hasn't changed.
            let new_status_str = task.status.to_string();
            let prev_status: Option<String> = conn
                .query_row(
                    "SELECT status FROM tasks WHERE id = ? AND status != ?",
                    params![task.id, new_status_str],
                    |row| row.get(0),
                )
                .optional()?;
            let mut persisted_deliverables = task.deliverables.clone();
            let reopening_closed =
                prev_status.as_deref() == Some("closed") && task.status != TaskStatus::Closed;
            let resuming_merge_conflict = prev_status.as_deref() == Some("awaiting_merge")
                && task.status == TaskStatus::InProgress;
            if reopening_closed || resuming_merge_conflict {
                // cas-ed9a: a close-cycle anchor is evidence for that completed
                // cycle only. Enforce invalidation at the persistence choke point
                // so every Closed -> non-Closed path (MCP update, reopen, UI,
                // recovery, and future callers) gets the same protection.
                // cas-5054 extends that invariant to conflict rework: the parked
                // anchor must not satisfy or false-reject the eventual re-close.
                persisted_deliverables.factory_branch_anchor = None;
            }
            if resuming_merge_conflict {
                // The decision note written by the lifecycle layer preserves the
                // diagnostic branch identity. These fields describe the prior
                // park cycle and must be rebuilt from the resolved branch on close.
                persisted_deliverables.parked_branch = None;
                persisted_deliverables.merge_conflicted = false;
            }

            let rows = conn.execute(
            "UPDATE tasks SET title = ?1, description = ?2, design = ?3,
             acceptance_criteria = ?4, notes = ?5, status = ?6, priority = ?7,
             task_type = ?8, assignee = ?9, labels = ?10, updated_at = ?11,
             closed_at = ?12, close_reason = ?13, external_ref = ?14, content_hash = ?15,
             branch = ?16, worktree_id = ?17,
             pending_verification = ?18, pending_worktree_merge = ?19, epic_verification_owner = ?20, team_id = ?21,
             deliverables = ?22, demo_statement = ?23, execution_note = ?24, share = ?25, depth = ?26
             WHERE id = ?27",
            params![
                task.title,
                task.description,
                task.design,
                task.acceptance_criteria,
                task.notes,
                new_status_str,
                task.priority.0,
                task.task_type.to_string(),
                task.assignee,
                Self::labels_to_string(&task.labels),
                Utc::now().to_rfc3339(),
                task.closed_at.map(|t| t.to_rfc3339()),
                task.close_reason,
                task.external_ref,
                task.content_hash,
                task.branch,
                task.worktree_id,
                if task.pending_verification { 1 } else { 0 },
                if task.pending_worktree_merge { 1 } else { 0 },
                task.epic_verification_owner,
                task.team_id,
                Self::deliverables_to_string(&persisted_deliverables),
                task.demo_statement,
                task.execution_note,
                task.share.as_ref().map(|s| s.to_string()),
                task.depth.to_string(),
                task.id,
            ],
        )?;
            if rows == 0 {
                return Err(StoreError::TaskNotFound(task.id.clone()));
            }

            // Emit status change events only when status actually changed
            // (prev_status is Some only when old status differs from new)
            if let Some(prev) = prev_status {
                let prev_status: TaskStatus = prev.parse().unwrap_or(TaskStatus::Open);
                if prev_status != task.status {
                    let (event_type, summary, recording_event_type) = match task.status {
                        TaskStatus::InProgress => (
                            EventType::TaskStarted,
                            format!("Task started: {}", task.title),
                            RecordingEventType::TaskStarted,
                        ),
                        TaskStatus::Closed => (
                            EventType::TaskCompleted,
                            format!("Task completed: {}", task.title),
                            RecordingEventType::TaskCompleted,
                        ),
                        TaskStatus::Blocked => (
                            EventType::TaskBlocked,
                            format!("Task blocked: {}", task.title),
                            RecordingEventType::TaskBlocked,
                        ),
                        TaskStatus::Open => (
                            EventType::TaskCreated,
                            format!("Task reopened: {}", task.title),
                            RecordingEventType::TaskCreated,
                        ),
                        // cas-b51a: supervisor-owned review mode — task awaits
                        // supervisor code-review dispatch before final close.
                        // Reuse TaskBlocked event type for TUI visibility since there
                        // is no dedicated supervisor-review event type yet.
                        TaskStatus::PendingSupervisorReview => (
                            EventType::TaskBlocked,
                            format!("Task pending supervisor review: {}", task.title),
                            RecordingEventType::TaskBlocked,
                        ),
                        TaskStatus::AwaitingMerge => (
                            EventType::TaskBlocked,
                            format!("Task awaiting merge: {}", task.title),
                            RecordingEventType::TaskBlocked,
                        ),
                    };
                    let event = Event::new(event_type, EventEntityType::Task, &task.id, summary);
                    let _ = record_event_with_conn(&conn, &event);

                    // Capture event for recording playback
                    let _ = capture_task_event(&conn, recording_event_type, &task.id, None);
                }
            }

            Ok(())
        }) // with_write_retry
    }

    fn delete(&self, id: &str) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;

            // Get task title before deleting for event summary
            let title: Option<String> = tx
                .query_row("SELECT title FROM tasks WHERE id = ?", params![id], |row| {
                    row.get(0)
                })
                .optional()?;

            // Delete associated dependencies first
            tx.execute(
                "DELETE FROM dependencies WHERE from_id = ? OR to_id = ?",
                params![id, id],
            )?;
            // Delete associated task leases
            tx.execute("DELETE FROM task_leases WHERE task_id = ?", params![id])?;
            let rows = tx.execute("DELETE FROM tasks WHERE id = ?", params![id])?;
            if rows == 0 {
                return Err(StoreError::TaskNotFound(id.to_string()));
            }

            // Record event for sidecar activity feed
            if let Some(title) = title {
                let event = Event::new(
                    EventType::TaskDeleted,
                    EventEntityType::Task,
                    id,
                    format!("Task deleted: {title}"),
                );
                let _ = record_event_with_conn(&tx, &event);
            }

            // Capture event for recording playback
            let _ = capture_task_event(&tx, RecordingEventType::TaskDeleted, id, None);

            tx.commit()?;
            Ok(())
        }) // with_write_retry
    }

    fn list(&self, status: Option<TaskStatus>) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();

        let (sql, params): (&str, Vec<String>) = match status {
            Some(s) => (
                "SELECT id, title, description, design, acceptance_criteria, notes,
                 status, priority, task_type, assignee, labels, created_at, updated_at,
                 closed_at, close_reason, external_ref, content_hash, branch, worktree_id,
                 pending_verification, pending_worktree_merge, epic_verification_owner, team_id, deliverables, demo_statement, execution_note, share, depth
                 FROM tasks WHERE status = ? ORDER BY priority, created_at DESC",
                vec![s.to_string()],
            ),
            None => (
                "SELECT id, title, description, design, acceptance_criteria, notes,
                 status, priority, task_type, assignee, labels, created_at, updated_at,
                 closed_at, close_reason, external_ref, content_hash, branch, worktree_id,
                 pending_verification, pending_worktree_merge, epic_verification_owner, team_id, deliverables, demo_statement, execution_note, share, depth
                 FROM tasks ORDER BY priority, created_at DESC",
                vec![],
            ),
        };

        let mut stmt = conn.prepare_cached(sql)?;
        let tasks = if params.is_empty() {
            stmt.query_map([], Self::task_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![params[0]], Self::task_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        Ok(tasks)
    }

    fn list_ready(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();

        // Ready = open tasks with no open blocking dependencies
        let mut stmt = conn.prepare_cached(
            "SELECT t.id, t.title, t.description, t.design, t.acceptance_criteria, t.notes,
             t.status, t.priority, t.task_type, t.assignee, t.labels, t.created_at, t.updated_at,
             t.closed_at, t.close_reason, t.external_ref, t.content_hash, t.branch, t.worktree_id,
             t.pending_verification, t.pending_worktree_merge, t.epic_verification_owner, t.team_id, t.deliverables, t.demo_statement, t.execution_note, t.share, t.depth
             FROM tasks t
             WHERE t.status = 'open'
             AND NOT EXISTS (
                 SELECT 1 FROM dependencies d
                 JOIN tasks blocker ON d.to_id = blocker.id
                 WHERE d.from_id = t.id
                 AND d.dep_type = 'blocks'
                 AND blocker.status != 'closed'
             )
             ORDER BY t.priority, t.created_at DESC",
        )?;

        let tasks = stmt
            .query_map([], Self::task_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(tasks)
    }

    fn list_blocked(&self) -> Result<Vec<(Task, Vec<Task>)>> {
        let conn = self.conn.lock().unwrap();

        // Fetch blocked tasks
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT t.id, t.title, t.description, t.design, t.acceptance_criteria, t.notes,
             t.status, t.priority, t.task_type, t.assignee, t.labels, t.created_at, t.updated_at,
             t.closed_at, t.close_reason, t.external_ref, t.content_hash, t.branch, t.worktree_id,
             t.pending_verification, t.pending_worktree_merge, t.epic_verification_owner, t.team_id, t.deliverables, t.demo_statement, t.execution_note, t.share, t.depth
             FROM tasks t
             JOIN dependencies d ON d.from_id = t.id
             JOIN tasks blocker ON d.to_id = blocker.id
             WHERE t.status != 'closed'
             AND d.dep_type = 'blocks'
             AND blocker.status != 'closed'
             ORDER BY t.priority, t.created_at DESC",
        )?;

        let blocked_tasks: Vec<Task> = stmt
            .query_map([], Self::task_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        if blocked_tasks.is_empty() {
            return Ok(Vec::new());
        }

        // Batch-fetch all blockers for all blocked tasks in a single query
        let task_ids: Vec<&str> = blocked_tasks.iter().map(|t| t.id.as_str()).collect();
        let placeholders: String = task_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "SELECT d.from_id,
             t.id, t.title, t.description, t.design, t.acceptance_criteria, t.notes,
             t.status, t.priority, t.task_type, t.assignee, t.labels, t.created_at, t.updated_at,
             t.closed_at, t.close_reason, t.external_ref, t.content_hash, t.branch, t.worktree_id,
             t.pending_verification, t.pending_worktree_merge, t.epic_verification_owner, t.team_id, t.deliverables, t.demo_statement, t.execution_note, t.share, t.depth
             FROM dependencies d
             JOIN tasks t ON d.to_id = t.id
             WHERE d.from_id IN ({placeholders})
             AND d.dep_type = 'blocks'
             AND t.status != 'closed'"
        );

        let mut blocker_stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = task_ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = blocker_stmt.query_map(params.as_slice(), |row| {
            let from_id: String = row.get(0)?;
            // task_from_row expects columns starting at index 0, but here they start at 1
            let task = Task {
                id: row.get(1)?,
                scope: Scope::Project,
                title: row.get(2)?,
                description: row.get::<_, String>(3)?,
                design: row.get::<_, String>(4)?,
                acceptance_criteria: row.get::<_, String>(5)?,
                notes: row.get::<_, String>(6)?,
                status: row.get::<_, String>(7)?.parse().unwrap_or(TaskStatus::Open),
                priority: Priority(row.get::<_, i32>(8)?),
                task_type: row.get::<_, String>(9)?.parse().unwrap_or(TaskType::Task),
                assignee: row.get(10)?,
                labels: Self::parse_labels(&row.get::<_, String>(11)?),
                created_at: Self::parse_datetime(&row.get::<_, String>(12)?)
                    .unwrap_or_else(Utc::now),
                updated_at: Self::parse_datetime(&row.get::<_, String>(13)?)
                    .unwrap_or_else(Utc::now),
                closed_at: row
                    .get::<_, Option<String>>(14)?
                    .and_then(|s| Self::parse_datetime(&s)),
                close_reason: row.get(15)?,
                external_ref: row.get(16)?,
                content_hash: row.get(17)?,
                branch: row.get(18)?,
                worktree_id: row.get(19)?,
                pending_verification: row.get::<_, i32>(20).unwrap_or(0) == 1,
                pending_worktree_merge: row.get::<_, i32>(21).unwrap_or(0) == 1,
                epic_verification_owner: row.get(22)?,
                team_id: row.get(23)?,
                deliverables: Self::parse_deliverables(&row.get::<_, String>(24)?),
                demo_statement: row.get::<_, String>(25)?,
                execution_note: row.get::<_, Option<String>>(26)?,
                share: row
                    .get::<_, Option<String>>(27)?
                    .as_deref()
                    .and_then(|s| s.parse().ok()),
                depth: row
                    .get::<_, Option<String>>(28)?
                    .as_deref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default(),
            };
            Ok((from_id, task))
        })?;

        // Group blockers by blocked task id
        let mut blockers_map: std::collections::HashMap<String, Vec<Task>> =
            std::collections::HashMap::new();
        for row in rows {
            let (from_id, blocker) = row?;
            blockers_map.entry(from_id).or_default().push(blocker);
        }

        let result = blocked_tasks
            .into_iter()
            .map(|task| {
                let blockers = blockers_map.remove(&task.id).unwrap_or_default();
                (task, blockers)
            })
            .collect();

        Ok(result)
    }

    fn list_pending_verification(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT id, title, description, design, acceptance_criteria, notes,
             status, priority, task_type, assignee, labels, created_at, updated_at,
             closed_at, close_reason, external_ref, content_hash, branch, worktree_id,
             pending_verification, pending_worktree_merge, epic_verification_owner, team_id, deliverables, demo_statement, execution_note, share, depth
             FROM tasks WHERE pending_verification = 1",
        )?;
        let tasks = stmt
            .query_map([], Self::task_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    fn list_pending_worktree_merge(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT id, title, description, design, acceptance_criteria, notes,
             status, priority, task_type, assignee, labels, created_at, updated_at,
             closed_at, close_reason, external_ref, content_hash, branch, worktree_id,
             pending_verification, pending_worktree_merge, epic_verification_owner, team_id, deliverables, demo_statement, execution_note, share, depth
             FROM tasks WHERE pending_worktree_merge = 1",
        )?;
        let tasks = stmt
            .query_map([], Self::task_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }

    // Dependency operations

    fn add_dependency(&self, dep: &Dependency) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::add_dependency_with_conn(&conn, dep, true)
    }

    fn remove_dependency(&self, from_id: &str, to_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM dependencies WHERE from_id = ? AND to_id = ?",
            params![from_id, to_id],
        )?;
        Ok(())
    }

    fn remove_dependency_of_type(
        &self,
        from_id: &str,
        to_id: &str,
        dep_type: DependencyType,
    ) -> Result<bool> {
        let dep_type_str = dep_type.to_string();
        let conn = self.conn.lock().unwrap();
        let rows_deleted = conn.execute(
            "DELETE FROM dependencies WHERE from_id = ? AND to_id = ? AND dep_type = ?",
            params![from_id, to_id, dep_type_str],
        )?;
        Ok(rows_deleted > 0)
    }

    fn get_dependencies(&self, task_id: &str) -> Result<Vec<Dependency>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT from_id, to_id, dep_type, created_at, created_by
             FROM dependencies WHERE from_id = ?",
        )?;

        let deps = stmt
            .query_map(params![task_id], Self::dep_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(deps)
    }

    fn get_dependents(&self, task_id: &str) -> Result<Vec<Dependency>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT from_id, to_id, dep_type, created_at, created_by
             FROM dependencies WHERE to_id = ?",
        )?;

        let deps = stmt
            .query_map(params![task_id], Self::dep_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(deps)
    }

    fn get_blockers(&self, task_id: &str) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "SELECT t.id, t.title, t.description, t.design, t.acceptance_criteria, t.notes,
             t.status, t.priority, t.task_type, t.assignee, t.labels, t.created_at, t.updated_at,
             t.closed_at, t.close_reason, t.external_ref, t.content_hash, t.branch, t.worktree_id,
             t.pending_verification, t.pending_worktree_merge, t.epic_verification_owner, t.team_id, t.deliverables, t.demo_statement, t.execution_note, t.share, t.depth
             FROM tasks t
             JOIN dependencies d ON d.to_id = t.id
             WHERE d.from_id = ? AND d.dep_type = 'blocks' AND t.status != 'closed'",
        )?;

        let tasks = stmt
            .query_map(params![task_id], Self::task_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(tasks)
    }

    fn would_create_cycle(&self, from_id: &str, to_id: &str) -> Result<bool> {
        // Use recursive CTE to check if to_id can reach from_id through blocking deps
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "WITH RECURSIVE reachable(node) AS (
                 SELECT ?1
                 UNION
                 SELECT d.to_id FROM dependencies d
                 JOIN reachable r ON d.from_id = r.node
                 WHERE d.dep_type = 'blocks'
             )
             SELECT COUNT(*) FROM reachable WHERE node = ?2",
            params![to_id, from_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn list_dependencies(&self, dep_type: Option<DependencyType>) -> Result<Vec<Dependency>> {
        let conn = self.conn.lock().unwrap();

        let (sql, params): (&str, Vec<String>) = match dep_type {
            Some(t) => (
                "SELECT from_id, to_id, dep_type, created_at, created_by
                 FROM dependencies WHERE dep_type = ? ORDER BY created_at DESC",
                vec![t.to_string()],
            ),
            None => (
                "SELECT from_id, to_id, dep_type, created_at, created_by
                 FROM dependencies ORDER BY created_at DESC",
                vec![],
            ),
        };

        let mut stmt = conn.prepare_cached(sql)?;
        let deps = if params.is_empty() {
            stmt.query_map([], Self::dep_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![params[0]], Self::dep_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        Ok(deps)
    }

    fn get_subtasks(&self, parent_id: &str) -> Result<Vec<Task>> {
        // Use recursive CTE to fetch all descendants in a single query
        // ParentChild dependency: from_id (child) -> to_id (parent)
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(
            "WITH RECURSIVE subtree(task_id) AS (
                 SELECT from_id FROM dependencies
                 WHERE to_id = ?1 AND dep_type = 'parent-child'
                 UNION
                 SELECT d.from_id FROM dependencies d
                 JOIN subtree s ON d.to_id = s.task_id
                 WHERE d.dep_type = 'parent-child'
             )
             SELECT t.id, t.title, t.description, t.design, t.acceptance_criteria, t.notes,
             t.status, t.priority, t.task_type, t.assignee, t.labels, t.created_at, t.updated_at,
             t.closed_at, t.close_reason, t.external_ref, t.content_hash, t.branch, t.worktree_id,
             t.pending_verification, t.pending_worktree_merge, t.epic_verification_owner, t.team_id, t.deliverables, t.demo_statement, t.execution_note, t.share, t.depth
             FROM tasks t
             JOIN subtree s ON t.id = s.task_id",
        )?;

        let tasks = stmt
            .query_map(params![parent_id], Self::task_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(tasks)
    }

    fn get_sibling_notes(
        &self,
        epic_id: &str,
        exclude_task_id: &str,
    ) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn.lock().unwrap();

        // Get direct subtasks of the epic that have non-empty notes
        // excluding the specified task
        let mut stmt = conn.prepare_cached(
            "SELECT t.id, t.title, t.notes
             FROM tasks t
             JOIN dependencies d ON d.from_id = t.id
             WHERE d.to_id = ? AND d.dep_type = 'parent-child'
               AND t.id != ?
               AND t.notes IS NOT NULL AND t.notes != ''
             ORDER BY t.updated_at DESC
             LIMIT 10",
        )?;

        let results: Vec<(String, String, String)> = stmt
            .query_map(params![epic_id, exclude_task_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(results)
    }

    fn get_parent_epic(&self, task_id: &str) -> Result<Option<Task>> {
        let conn = self.conn.lock().unwrap();

        // Find parent via ParentChild dependency where the parent is an epic
        let mut stmt = conn.prepare_cached(
            "SELECT t.id, t.title, t.description, t.design, t.acceptance_criteria, t.notes,
             t.status, t.priority, t.task_type, t.assignee, t.labels, t.created_at, t.updated_at,
             t.closed_at, t.close_reason, t.external_ref, t.content_hash, t.branch, t.worktree_id,
             t.pending_verification, t.pending_worktree_merge, t.epic_verification_owner, t.team_id, t.deliverables, t.demo_statement, t.execution_note, t.share, t.depth
             FROM tasks t
             JOIN dependencies d ON d.to_id = t.id
             WHERE d.from_id = ? AND d.dep_type = 'parent-child' AND t.task_type = 'epic'
             LIMIT 1",
        )?;

        let parent = stmt
            .query_map(params![task_id], Self::task_from_row)?
            .next()
            .transpose()?;

        Ok(parent)
    }
}

#[cfg(test)]
#[path = "task_store_tests/tests.rs"]
mod tests;
