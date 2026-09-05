use std::fs::{File, OpenOptions};

use chrono::Utc;
use fs2::FileExt;
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::cloud::sync_queue::queue_ops::{remove_legacy_team_upsert_row, upsert_queue_row};
use crate::cloud::sync_queue::{EntityType, SyncOperation, SyncQueue};
use crate::error::CasError;

const TASK_SYNC_INTENT_LOCK: &str = "task-sync-intents.lock";

pub(crate) struct TaskSyncMutationGuard(File);

impl Drop for TaskSyncMutationGuard {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.0) {
            tracing::error!(%error, "failed to release task-sync mutation lock");
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskSyncIntent {
    pub id: i64,
    pub mutation_id: String,
    pub entity_id: String,
    pub operation: String,
    pub previous_updated_at: Option<String>,
    pub previous_revision: i64,
    pub committed_revision: Option<i64>,
    pub team_id: Option<String>,
    pub previous_team_id: Option<String>,
    pub previous_project_id: Option<String>,
    pub global_scope: bool,
}

pub(crate) struct TaskSyncPayload {
    pub payload: String,
    pub current_project_id: Option<String>,
    pub current_team_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskSyncFulfillResult {
    Fulfilled,
    ProvenPreCommit,
    Superseded,
}

impl SyncQueue {
    /// Serialize wrapper mutations with reconciliation across processes. The
    /// operating system releases this lease after a crash.
    pub(crate) fn lock_task_sync_mutations(&self) -> Result<TaskSyncMutationGuard, CasError> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(self.cas_dir.join(TASK_SYNC_INTENT_LOCK))?;
        file.lock_exclusive()?;
        Ok(TaskSyncMutationGuard(file))
    }

    pub(crate) fn stage_task_sync_intent(
        &self,
        entity_id: &str,
        operation: &str,
        previous_updated_at: Option<&str>,
        team_id: Option<&str>,
        fallback_previous_team_id: Option<&str>,
        fallback_previous_project_id: Option<&str>,
        global_scope: bool,
    ) -> Result<TaskSyncIntent, CasError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mutation_id = uuid::Uuid::new_v4().to_string();
        let previous_revision = tx
            .query_row(
                "SELECT revision FROM task_mutation_revisions WHERE entity_id = ?1",
                params![entity_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        let previous_route = tx
            .query_row(
                "SELECT team_id, project_id FROM task_sync_routes WHERE entity_id = ?1",
                params![entity_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()?;
        let (previous_team_id, previous_project_id) = previous_route.unwrap_or_else(|| {
            (
                fallback_previous_team_id.map(ToOwned::to_owned),
                fallback_previous_project_id.map(ToOwned::to_owned),
            )
        });
        tx.execute(
            r#"
            INSERT INTO task_sync_intents
                (mutation_id, entity_id, operation, previous_updated_at, previous_revision,
                 team_id, previous_team_id, previous_project_id, global_scope, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                mutation_id,
                entity_id,
                operation,
                previous_updated_at,
                previous_revision,
                team_id,
                previous_team_id,
                previous_project_id,
                global_scope,
                Utc::now().to_rfc3339(),
            ],
        )?;
        let intent = TaskSyncIntent {
            id: tx.last_insert_rowid(),
            mutation_id,
            entity_id: entity_id.to_string(),
            operation: operation.to_string(),
            previous_updated_at: previous_updated_at.map(ToOwned::to_owned),
            previous_revision,
            committed_revision: None,
            team_id: team_id.map(ToOwned::to_owned),
            previous_team_id,
            previous_project_id,
            global_scope,
        };
        tx.commit()?;
        Ok(intent)
    }

    pub(crate) fn cancel_task_sync_intent(&self, intent_id: i64) -> Result<(), CasError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM task_mutation_receipts WHERE receipt_id IN
             (SELECT mutation_id FROM task_sync_intents WHERE id = ?1)",
            params![intent_id],
        )?;
        tx.execute(
            "DELETE FROM task_sync_intents WHERE id = ?1",
            params![intent_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn pending_task_sync_intents(&self) -> Result<Vec<TaskSyncIntent>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            r#"
            SELECT i.id, i.mutation_id, i.entity_id, i.operation, i.previous_updated_at,
                   i.previous_revision, r.revision, i.team_id, i.previous_team_id,
                   i.previous_project_id, i.global_scope
            FROM task_sync_intents i
            LEFT JOIN task_mutation_receipts r ON r.receipt_id = i.mutation_id
            ORDER BY i.id ASC
            "#,
        )?;
        let rows = statement.query_map([], |row| {
            Ok(TaskSyncIntent {
                id: row.get(0)?,
                mutation_id: row.get(1)?,
                entity_id: row.get(2)?,
                operation: row.get(3)?,
                previous_updated_at: row.get(4)?,
                previous_revision: row.get(5)?,
                committed_revision: row.get(6)?,
                team_id: row.get(7)?,
                previous_team_id: row.get(8)?,
                previous_project_id: row.get(9)?,
                global_scope: row.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CasError::from)
    }

    /// Queue only the canonical payload under the current routing policy.
    /// An obsolete proven route receives a payload-free delete.
    pub(crate) fn fulfill_task_sync_intent<F, H>(
        &self,
        intent: &TaskSyncIntent,
        after_validation: H,
        load_canonical: F,
    ) -> Result<TaskSyncFulfillResult, CasError>
    where
        F: FnOnce() -> Result<TaskSyncPayload, CasError>,
        H: FnOnce(),
    {
        let mut conn = self.conn.lock().unwrap();
        // BEGIN IMMEDIATE is load-bearing: after revision validation, the
        // read-only callback loads canonical state through the task store's
        // separate connection while this transaction excludes every bypass
        // writer until the outbox rows commit.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current_revision = tx
            .query_row(
                "SELECT revision, present FROM task_mutation_revisions WHERE entity_id = ?1",
                params![intent.entity_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? == 1)),
            )
            .optional()?;
        let committed_revision = tx
            .query_row(
                "SELECT revision FROM task_mutation_receipts WHERE receipt_id = ?1 AND entity_id = ?2",
                params![intent.mutation_id, intent.entity_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;

        match (committed_revision, current_revision) {
            (Some(committed), Some((current, true))) if committed == current => {}
            (Some(committed), Some((current, _))) if current > committed => {
                retire_task_sync_evidence(&tx, &intent.entity_id)?;
                tx.commit()?;
                return Ok(TaskSyncFulfillResult::Superseded);
            }
            (None, Some((current, _))) if current == intent.previous_revision => {
                retire_one_task_sync_intent(&tx, intent)?;
                tx.commit()?;
                return Ok(TaskSyncFulfillResult::ProvenPreCommit);
            }
            (None, None) if intent.previous_revision == 0 => {
                retire_one_task_sync_intent(&tx, intent)?;
                tx.commit()?;
                return Ok(TaskSyncFulfillResult::ProvenPreCommit);
            }
            (committed, current) => {
                return Err(CasError::Other(format!(
                    "task sync recovery blocked for {}: intent revision is unclassified (previous={}, committed={committed:?}, current={current:?}); durable evidence retained",
                    intent.entity_id, intent.previous_revision
                )));
            }
        }

        after_validation();
        let canonical = load_canonical()?;
        let payload = canonical.payload;
        let current_project_id = canonical.current_project_id.as_deref();
        let current_team_id = canonical.current_team_id.as_deref();
        upsert_queue_row(
            &tx,
            EntityType::Task,
            &intent.entity_id,
            SyncOperation::Upsert,
            Some(&payload),
            "",
            None,
        )?;

        let previous_route = intent
            .previous_team_id
            .as_deref()
            .map(|team_id| (team_id, intent.previous_project_id.as_deref()));
        let current_route = current_team_id.map(|team_id| (team_id, current_project_id));
        if previous_route != current_route
            && let Some((team_id, project_id)) = previous_route
        {
            remove_legacy_team_upsert_row(&tx, EntityType::Task, &intent.entity_id, team_id)?;
            upsert_queue_row(
                &tx,
                EntityType::Task,
                &intent.entity_id,
                SyncOperation::Delete,
                None,
                team_id,
                project_id,
            )?;
        }
        if let Some((team_id, project_id)) = current_route {
            upsert_queue_row(
                &tx,
                EntityType::Task,
                &intent.entity_id,
                SyncOperation::Upsert,
                Some(&payload),
                team_id,
                project_id,
            )?;
        }

        tx.execute(
            r#"
            INSERT INTO task_sync_routes (entity_id, team_id, project_id, updated_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(entity_id) DO UPDATE SET
                team_id = excluded.team_id,
                project_id = excluded.project_id,
                updated_at = excluded.updated_at
            "#,
            params![
                intent.entity_id,
                current_team_id,
                current_project_id,
                Utc::now().to_rfc3339(),
            ],
        )?;
        retire_task_sync_evidence(&tx, &intent.entity_id)?;
        tx.commit()?;
        Ok(TaskSyncFulfillResult::Fulfilled)
    }
}

fn retire_one_task_sync_intent(
    conn: &rusqlite::Connection,
    intent: &TaskSyncIntent,
) -> Result<(), CasError> {
    conn.execute(
        "DELETE FROM task_mutation_receipts WHERE receipt_id = ?1",
        params![intent.mutation_id],
    )?;
    conn.execute(
        "DELETE FROM task_sync_intents WHERE id = ?1",
        params![intent.id],
    )?;
    Ok(())
}

fn retire_task_sync_evidence(conn: &rusqlite::Connection, entity_id: &str) -> Result<(), CasError> {
    conn.execute(
        "DELETE FROM task_mutation_receipts WHERE receipt_id IN
         (SELECT mutation_id FROM task_sync_intents WHERE entity_id = ?1)",
        params![entity_id],
    )?;
    conn.execute(
        "DELETE FROM task_sync_intents WHERE entity_id = ?1",
        params![entity_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{SqliteTaskStore, TaskStore};

    #[test]
    fn legacy_intent_migrates_to_recovery_blocked_evidence() {
        let temp = tempfile::TempDir::new().unwrap();
        let tasks = SqliteTaskStore::open(temp.path()).unwrap();
        tasks.init().unwrap();
        let task = crate::types::Task::new("task-legacy-intent".into(), "canonical".into());
        tasks.add(&task).unwrap();
        let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE task_sync_intents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_id TEXT NOT NULL,
                operation TEXT NOT NULL,
                previous_updated_at TEXT,
                team_id TEXT,
                previous_team_id TEXT,
                previous_project_id TEXT,
                global_scope INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            INSERT INTO task_sync_intents
                (entity_id, operation, global_scope, created_at)
            VALUES ('task-legacy-intent', 'update', 0, '2026-09-05T00:00:00Z');
            "#,
        )
        .unwrap();

        let queue = SyncQueue::open(temp.path()).unwrap();
        queue.init().unwrap();
        let intent = queue.pending_task_sync_intents().unwrap().pop().unwrap();
        assert_eq!(intent.previous_revision, -1);
        assert!(intent.mutation_id.starts_with("legacy-unbound-"));

        let error = queue
            .fulfill_task_sync_intent(
                &intent,
                || panic!("unclassified intent must not reach payload loading"),
                || panic!("unclassified intent must not load canonical payload"),
            )
            .unwrap_err();
        assert!(error.to_string().contains("recovery blocked"), "{error}");
        assert_eq!(queue.pending_task_sync_intents().unwrap(), vec![intent]);
    }

    #[test]
    fn concurrent_task_mutations_keep_independent_repair_intents() {
        let temp = tempfile::TempDir::new().unwrap();
        let tasks = SqliteTaskStore::open(temp.path()).unwrap();
        tasks.init().unwrap();
        let queue = SyncQueue::open(temp.path()).unwrap();
        queue.init().unwrap();
        let first = queue
            .stage_task_sync_intent("cas-concurrent", "update", None, None, None, None, false)
            .unwrap();
        let second = queue
            .stage_task_sync_intent("cas-concurrent", "update", None, None, None, None, false)
            .unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(queue.pending_task_sync_intents().unwrap().len(), 2);
        queue.cancel_task_sync_intent(first.id).unwrap();
        assert_eq!(queue.pending_task_sync_intents().unwrap(), vec![second]);
    }

    #[test]
    fn task_sync_lock_releases_for_an_independent_handle() {
        let temp = tempfile::TempDir::new().unwrap();
        let queue = SyncQueue::open(temp.path()).unwrap();
        let held = queue.lock_task_sync_mutations().unwrap();
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(temp.path().join(TASK_SYNC_INTENT_LOCK))
            .unwrap();
        assert!(contender.try_lock_exclusive().is_err());
        drop(held);
        contender.try_lock_exclusive().unwrap();
        FileExt::unlock(&contender).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn task_sync_lock_descriptor_is_close_on_exec() {
        use std::os::fd::AsRawFd;

        let temp = tempfile::TempDir::new().unwrap();
        let queue = SyncQueue::open(temp.path()).unwrap();
        let held = queue.lock_task_sync_mutations().unwrap();
        let flags = unsafe { libc::fcntl(held.0.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }
}
