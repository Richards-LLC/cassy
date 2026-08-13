//! Local projection of cloud-owned cross-project task dependencies.
//!
//! These rows are signals about foreign tasks, not replicas of those tasks.
//! The cloud proposal/dependency row remains authoritative.

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};

use crate::{Result, shared_db};

pub const EXTERNAL_TASK_DEPENDENCY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS external_task_dependencies (
    origin_task_id TEXT NOT NULL,
    proposal_id TEXT NOT NULL,
    target_project_canonical_id TEXT NOT NULL DEFAULT '',
    target_task_id TEXT NOT NULL,
    proposal_state TEXT NOT NULL,
    target_task_status TEXT,
    resolution_state TEXT NOT NULL,
    resolved_at TEXT,
    suppressed_at TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (origin_task_id, proposal_id)
);
CREATE INDEX IF NOT EXISTS idx_external_task_dependencies_origin_state
    ON external_task_dependencies(origin_task_id, resolution_state);
CREATE TABLE IF NOT EXISTS external_task_dependency_sync_state (
    origin_project_canonical_id TEXT PRIMARY KEY,
    cursor TEXT,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS task_proposal_request_keys (
    request_fingerprint TEXT PRIMARY KEY,
    client_request_id TEXT NOT NULL,
    created_at TEXT NOT NULL
);
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalTaskDependencyProjection {
    pub origin_task_id: String,
    pub proposal_id: String,
    pub target_project_canonical_id: String,
    pub target_task_id: String,
    pub proposal_state: String,
    pub target_task_status: Option<String>,
    pub resolution_state: String,
    pub resolved_at: Option<String>,
}

impl ExternalTaskDependencyProjection {
    pub fn is_blocking(&self) -> bool {
        self.resolution_state != "resolved"
    }
}

pub struct ExternalTaskDependencyStore {
    conn: Arc<Mutex<Connection>>,
}

impl ExternalTaskDependencyStore {
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let conn = shared_db::shared_connection(&cas_dir.join("cas.db"))?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    pub fn init(&self) -> Result<()> {
        self.conn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .execute_batch(EXTERNAL_TASK_DEPENDENCY_SCHEMA)?;
        Ok(())
    }

    pub fn upsert(&self, dependency: &ExternalTaskDependencyProjection) -> Result<()> {
        self.conn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .execute(
                "INSERT INTO external_task_dependencies
                 (origin_task_id, proposal_id, target_project_canonical_id, target_task_id,
                  proposal_state, target_task_status, resolution_state, resolved_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(origin_task_id, proposal_id) DO UPDATE SET
                   target_project_canonical_id = excluded.target_project_canonical_id,
                   target_task_id = excluded.target_task_id,
                   proposal_state = excluded.proposal_state,
                   target_task_status = excluded.target_task_status,
                   resolution_state = excluded.resolution_state,
                   resolved_at = excluded.resolved_at,
                   updated_at = excluded.updated_at
                 WHERE external_task_dependencies.suppressed_at IS NULL",
                params![
                    dependency.origin_task_id,
                    dependency.proposal_id,
                    dependency.target_project_canonical_id,
                    dependency.target_task_id,
                    dependency.proposal_state,
                    dependency.target_task_status,
                    dependency.resolution_state,
                    dependency.resolved_at,
                    Utc::now().to_rfc3339(),
                ],
            )?;
        Ok(())
    }

    pub fn list_blocking_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<ExternalTaskDependencyProjection>> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        let mut statement = conn.prepare(
            "SELECT origin_task_id, proposal_id, target_project_canonical_id, target_task_id,
                    proposal_state, target_task_status, resolution_state, resolved_at
             FROM external_task_dependencies
             WHERE origin_task_id = ?1
               AND resolution_state != 'resolved'
               AND suppressed_at IS NULL
             ORDER BY proposal_id",
        )?;
        let rows = statement.query_map(params![task_id], |row| {
            Ok(ExternalTaskDependencyProjection {
                origin_task_id: row.get(0)?,
                proposal_id: row.get(1)?,
                target_project_canonical_id: row.get(2)?,
                target_task_id: row.get(3)?,
                proposal_state: row.get(4)?,
                target_task_status: row.get(5)?,
                resolution_state: row.get(6)?,
                resolved_at: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Suppress an external handoff rather than deleting it. Cloud feeds
    /// replay a short safety window, so physical deletion would let a rejected
    /// handoff reappear and silently re-block a task after the operator had
    /// explicitly removed it.
    pub fn remove(&self, origin_task_id: &str, target_task_id: &str) -> Result<bool> {
        let suppressed = self
            .conn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .execute(
                "UPDATE external_task_dependencies
                 SET suppressed_at = COALESCE(suppressed_at, ?3), updated_at = ?3
                 WHERE origin_task_id = ?1 AND target_task_id = ?2",
                params![origin_task_id, target_task_id, Utc::now().to_rfc3339()],
            )?;
        Ok(suppressed > 0)
    }

    pub fn cursor(&self, origin_project: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .query_row(
                "SELECT cursor FROM external_task_dependency_sync_state
                 WHERE origin_project_canonical_id = ?1",
                params![origin_project],
                |row| row.get(0),
            )
            .optional()?
            .flatten())
    }

    pub fn set_cursor(&self, origin_project: &str, cursor: Option<&str>) -> Result<()> {
        self.conn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .execute(
                "INSERT INTO external_task_dependency_sync_state
                 (origin_project_canonical_id, cursor, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(origin_project_canonical_id) DO UPDATE SET
                   cursor = excluded.cursor, updated_at = excluded.updated_at",
                params![origin_project, cursor, Utc::now().to_rfc3339()],
            )?;
        Ok(())
    }

    /// Return the durable idempotency key for one logical proposal request.
    /// A transport failure after cloud commit is ambiguous; retaining this key
    /// makes the caller's retry converge on the cloud's unique proposal row.
    pub fn client_request_id(
        &self,
        request_fingerprint: &str,
        generated_request_id: &str,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        conn.execute(
            "INSERT OR IGNORE INTO task_proposal_request_keys
             (request_fingerprint, client_request_id, created_at) VALUES (?1, ?2, ?3)",
            params![
                request_fingerprint,
                generated_request_id,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(conn.query_row(
            "SELECT client_request_id FROM task_proposal_request_keys WHERE request_fingerprint = ?1",
            params![request_fingerprint],
            |row| row.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SqliteTaskStore, TaskStore};

    #[test]
    fn rejected_handoff_remains_blocking_until_operator_removes_it() {
        let temp = tempfile::tempdir().unwrap();
        let store = ExternalTaskDependencyStore::open(temp.path()).unwrap();
        let mut dependency = ExternalTaskDependencyProjection {
            origin_task_id: "cas-origin".into(),
            proposal_id: "proposal-1".into(),
            target_project_canonical_id: "target".into(),
            target_task_id: "cas-0123456789abcdef".into(),
            proposal_state: "proposed".into(),
            target_task_status: None,
            resolution_state: "unresolved".into(),
            resolved_at: None,
        };
        store.upsert(&dependency).unwrap();
        dependency.proposal_state = "rejected".into();
        dependency.resolution_state = "handoff_rejected".into();
        store.upsert(&dependency).unwrap();
        assert_eq!(
            store.list_blocking_for_task("cas-origin").unwrap(),
            vec![dependency.clone()]
        );
        assert!(store.remove("cas-origin", "cas-0123456789abcdef").unwrap());
        assert!(
            store
                .list_blocking_for_task("cas-origin")
                .unwrap()
                .is_empty()
        );
        // The feed replays the same rejected row after an operator removes
        // it. The durable tombstone must win over that replay.
        store.upsert(&dependency).unwrap();
        assert!(
            store
                .list_blocking_for_task("cas-origin")
                .unwrap()
                .is_empty(),
            "a replayed rejected handoff must remain operator-suppressed"
        );
    }

    #[test]
    fn resolved_projection_stops_blocking_and_cursor_is_opaque() {
        let temp = tempfile::tempdir().unwrap();
        let store = ExternalTaskDependencyStore::open(temp.path()).unwrap();
        store
            .upsert(&ExternalTaskDependencyProjection {
                origin_task_id: "cas-origin".into(),
                proposal_id: "proposal-1".into(),
                target_project_canonical_id: "target".into(),
                target_task_id: "cas-0123456789abcdef".into(),
                proposal_state: "accepted".into(),
                target_task_status: Some("closed".into()),
                resolution_state: "resolved".into(),
                resolved_at: Some("2026-08-13T12:00:00Z".into()),
            })
            .unwrap();
        assert!(
            store
                .list_blocking_for_task("cas-origin")
                .unwrap()
                .is_empty()
        );
        store
            .set_cursor("origin", Some("opaque:cursor/value"))
            .unwrap();
        assert_eq!(
            store.cursor("origin").unwrap().as_deref(),
            Some("opaque:cursor/value")
        );
    }

    /// Cloud contract: closing an accepted target task resolves the edge;
    /// reopening it returns the edge to `unresolved` with a null `resolved_at`
    /// (never contradictory fields), and a later close re-resolves with a fresh
    /// timestamp. The local projection must track that round trip, including
    /// clearing a previously stored `resolved_at`.
    #[test]
    fn reopened_target_returns_edge_to_unresolved_and_reclose_reresolves() {
        let temp = tempfile::tempdir().unwrap();
        let store = ExternalTaskDependencyStore::open(temp.path()).unwrap();
        let mut dependency = ExternalTaskDependencyProjection {
            origin_task_id: "cas-origin".into(),
            proposal_id: "proposal-1".into(),
            target_project_canonical_id: "target".into(),
            target_task_id: "cas-0123456789abcdef".into(),
            proposal_state: "accepted".into(),
            target_task_status: Some("closed".into()),
            resolution_state: "resolved".into(),
            resolved_at: Some("2026-08-13T12:00:00Z".into()),
        };
        store.upsert(&dependency).unwrap();
        assert!(
            store
                .list_blocking_for_task("cas-origin")
                .unwrap()
                .is_empty(),
            "resolved edge does not block"
        );

        // Target reopened: back to unresolved, resolved_at cleared.
        dependency.target_task_status = Some("open".into());
        dependency.resolution_state = "unresolved".into();
        dependency.resolved_at = None;
        store.upsert(&dependency).unwrap();
        let blocking = store.list_blocking_for_task("cas-origin").unwrap();
        assert_eq!(blocking, vec![dependency.clone()], "reopen re-blocks");
        assert!(
            blocking[0].resolved_at.is_none(),
            "stale resolved_at must be cleared, not retained"
        );

        // Closed again: re-resolves with a fresh timestamp.
        dependency.target_task_status = Some("closed".into());
        dependency.resolution_state = "resolved".into();
        dependency.resolved_at = Some("2026-08-13T18:30:00Z".into());
        store.upsert(&dependency).unwrap();
        assert!(
            store
                .list_blocking_for_task("cas-origin")
                .unwrap()
                .is_empty(),
            "re-close resolves again"
        );
    }

    #[test]
    fn ready_and_blocked_queries_consider_external_projection() {
        let temp = tempfile::tempdir().unwrap();
        let tasks = SqliteTaskStore::open(temp.path()).unwrap();
        tasks.init().unwrap();
        tasks
            .add(&cas_types::Task::new(
                "cas-origin".into(),
                "Origin work".into(),
            ))
            .unwrap();
        let external = ExternalTaskDependencyStore::open(temp.path()).unwrap();
        external
            .upsert(&ExternalTaskDependencyProjection {
                origin_task_id: "cas-origin".into(),
                proposal_id: "proposal-1".into(),
                target_project_canonical_id: "target".into(),
                target_task_id: "cas-0123456789abcdef".into(),
                proposal_state: "accepted".into(),
                target_task_status: Some("open".into()),
                resolution_state: "unresolved".into(),
                resolved_at: None,
            })
            .unwrap();
        assert!(tasks.list_ready().unwrap().is_empty());
        let blocked = tasks.list_blocked().unwrap();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].0.id, "cas-origin");
        assert!(blocked[0].1.is_empty(), "foreign tasks are not replicas");
    }
}
