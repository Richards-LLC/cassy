use std::fs::{File, OpenOptions};

use chrono::Utc;
use fs2::FileExt;
use rusqlite::{OptionalExtension, params};

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
    pub entity_id: String,
    pub operation: String,
    pub previous_updated_at: Option<String>,
    pub team_id: Option<String>,
    pub previous_team_id: Option<String>,
    pub previous_project_id: Option<String>,
    pub global_scope: bool,
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
                (entity_id, operation, previous_updated_at, team_id, previous_team_id,
                 previous_project_id, global_scope, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                entity_id,
                operation,
                previous_updated_at,
                team_id,
                previous_team_id,
                previous_project_id,
                global_scope,
                Utc::now().to_rfc3339(),
            ],
        )?;
        let intent = TaskSyncIntent {
            id: tx.last_insert_rowid(),
            entity_id: entity_id.to_string(),
            operation: operation.to_string(),
            previous_updated_at: previous_updated_at.map(ToOwned::to_owned),
            team_id: team_id.map(ToOwned::to_owned),
            previous_team_id,
            previous_project_id,
            global_scope,
        };
        tx.commit()?;
        Ok(intent)
    }

    pub(crate) fn cancel_task_sync_intent(&self, intent_id: i64) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM task_sync_intents WHERE id = ?1",
            params![intent_id],
        )?;
        Ok(())
    }

    pub(crate) fn cancel_task_sync_intents_for_entity(
        &self,
        entity_id: &str,
    ) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM task_sync_intents WHERE entity_id = ?1",
            params![entity_id],
        )?;
        Ok(())
    }

    pub(crate) fn pending_task_sync_intents(&self) -> Result<Vec<TaskSyncIntent>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            r#"
            SELECT id, entity_id, operation, previous_updated_at, team_id,
                   previous_team_id, previous_project_id, global_scope
            FROM task_sync_intents
            ORDER BY id ASC
            "#,
        )?;
        let rows = statement.query_map([], |row| {
            Ok(TaskSyncIntent {
                id: row.get(0)?,
                entity_id: row.get(1)?,
                operation: row.get(2)?,
                previous_updated_at: row.get(3)?,
                team_id: row.get(4)?,
                previous_team_id: row.get(5)?,
                previous_project_id: row.get(6)?,
                global_scope: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CasError::from)
    }

    /// Queue only the canonical payload under the current routing policy.
    /// An obsolete proven route receives a payload-free delete.
    pub(crate) fn fulfill_task_sync_intent(
        &self,
        intent: &TaskSyncIntent,
        payload: &str,
        current_project_id: Option<&str>,
        current_team_id: Option<&str>,
    ) -> Result<(), CasError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        upsert_queue_row(
            &tx,
            EntityType::Task,
            &intent.entity_id,
            SyncOperation::Upsert,
            Some(payload),
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
                Some(payload),
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
        tx.execute(
            "DELETE FROM task_sync_intents WHERE entity_id = ?1",
            params![intent.entity_id],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_task_mutations_keep_independent_repair_intents() {
        let temp = tempfile::TempDir::new().unwrap();
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
