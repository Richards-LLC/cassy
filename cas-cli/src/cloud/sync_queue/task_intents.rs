use chrono::Utc;
use rusqlite::params;

use crate::cloud::sync_queue::queue_ops::{enqueue_team_move_rows, upsert_queue_row};
use crate::cloud::sync_queue::{EntityType, SyncOperation, SyncQueue};
use crate::error::CasError;

/// A durable handoff between a local task mutation and its outbox rows.
///
/// Each mutation gets its own row. A per-entity singleton would let one
/// concurrent writer delete another writer's crash-recovery evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskSyncIntent {
    pub id: i64,
    pub entity_id: String,
    pub operation: String,
    pub previous_updated_at: Option<String>,
    pub team_id: Option<String>,
    pub old_project_id: Option<String>,
    pub global_scope: bool,
}

impl SyncQueue {
    /// Persist task-sync intent before mutating the local task row.
    pub(crate) fn stage_task_sync_intent(
        &self,
        entity_id: &str,
        operation: &str,
        previous_updated_at: Option<&str>,
        team_id: Option<&str>,
        old_project_id: Option<&str>,
        global_scope: bool,
    ) -> Result<TaskSyncIntent, CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO task_sync_intents
                (entity_id, operation, previous_updated_at, team_id, old_project_id, global_scope, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                entity_id,
                operation,
                previous_updated_at,
                team_id,
                old_project_id,
                global_scope,
                Utc::now().to_rfc3339(),
            ],
        )?;

        Ok(TaskSyncIntent {
            id: conn.last_insert_rowid(),
            entity_id: entity_id.to_string(),
            operation: operation.to_string(),
            previous_updated_at: previous_updated_at.map(ToOwned::to_owned),
            team_id: team_id.map(ToOwned::to_owned),
            old_project_id: old_project_id.map(ToOwned::to_owned),
            global_scope,
        })
    }

    /// Remove an intent whose local mutation did not commit.
    pub(crate) fn cancel_task_sync_intent(&self, intent_id: i64) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM task_sync_intents WHERE id = ?1",
            params![intent_id],
        )?;
        Ok(())
    }

    /// List task intents retained by a failed or interrupted enqueue.
    pub(crate) fn pending_task_sync_intents(&self) -> Result<Vec<TaskSyncIntent>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            r#"
            SELECT id, entity_id, operation, previous_updated_at, team_id, old_project_id, global_scope
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
                old_project_id: row.get(5)?,
                global_scope: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CasError::from)
    }

    /// Materialize every outbox row for one task mutation and retire its
    /// durable intent in the same SQLite transaction.
    pub(crate) fn fulfill_task_sync_intent(
        &self,
        intent: &TaskSyncIntent,
        payload: &str,
        current_project_id: Option<&str>,
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

        if let Some(team_id) = intent.team_id.as_deref() {
            if let Some((old_project_id, new_project_id)) = intent
                .old_project_id
                .as_deref()
                .zip(current_project_id)
                .filter(|(old, current)| old != current)
            {
                enqueue_team_move_rows(
                    &tx,
                    EntityType::Task,
                    &intent.entity_id,
                    old_project_id,
                    new_project_id,
                    payload,
                    team_id,
                )?;
            } else {
                upsert_queue_row(
                    &tx,
                    EntityType::Task,
                    &intent.entity_id,
                    SyncOperation::Upsert,
                    Some(payload),
                    team_id,
                    current_project_id,
                )?;
            }
        }

        tx.execute(
            "DELETE FROM task_sync_intents WHERE id = ?1",
            params![intent.id],
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
            .stage_task_sync_intent("cas-concurrent", "update", None, None, None, false)
            .unwrap();
        let second = queue
            .stage_task_sync_intent("cas-concurrent", "update", None, None, None, false)
            .unwrap();

        assert_ne!(first.id, second.id);
        assert_eq!(queue.pending_task_sync_intents().unwrap().len(), 2);
        queue.cancel_task_sync_intent(first.id).unwrap();
        assert_eq!(queue.pending_task_sync_intents().unwrap(), vec![second]);
    }
}
