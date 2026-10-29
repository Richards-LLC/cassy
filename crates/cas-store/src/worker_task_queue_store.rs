//! Ordered task backlogs, independent of worker registration lifetimes.
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Result, StoreError};

pub const WORKER_TASK_QUEUE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS worker_task_queues (
    factory_session TEXT NOT NULL,
    worker_name TEXT NOT NULL,
    revision INTEGER NOT NULL,
    state TEXT NOT NULL,
    PRIMARY KEY(factory_session, worker_name)
);";

pub const SPAWN_TASK_QUEUE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS spawn_task_queues (
    request_id INTEGER PRIMARY KEY,
    task_ids TEXT NOT NULL,
    bound_worker TEXT
);";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerTaskQueue {
    pub revision: u64,
    pub current_task: Option<String>,
    pub pending: Vec<String>,
    /// An interrupted selection is retried by its predecessor, never popped twice.
    pub dispatch_from: Option<String>,
    pub starting: bool,
    #[serde(default)]
    pub dispatch_revision: u64,
    pub notes: Vec<String>,
}

pub struct SqliteWorkerTaskQueueStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteWorkerTaskQueueStore {
    pub fn open(root: &Path) -> Result<Self> {
        let conn = crate::shared_db::shared_connection(&root.join("cas.db"))?;
        crate::shared_db::lock_connection(&conn)?.execute_batch(WORKER_TASK_QUEUE_SCHEMA)?;
        crate::shared_db::lock_connection(&conn)?.execute_batch(SPAWN_TASK_QUEUE_SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn get(&self, session: &str, worker: &str) -> Result<WorkerTaskQueue> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let state: Option<String> = conn
            .query_row(
                "SELECT state FROM worker_task_queues WHERE factory_session=? AND worker_name=?",
                params![session, worker],
                |row| row.get(0),
            )
            .optional()?;
        state
            .map(|json| serde_json::from_str(&json).map_err(|e| StoreError::Other(e.to_string())))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    /// Compare-and-swap the complete queue, including selection/recovery state.
    pub fn save(&self, session: &str, worker: &str, state: &mut WorkerTaskQueue) -> Result<()> {
        let expected = state.revision;
        let mut next = state.clone();
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| StoreError::Other("Worker queue revision overflow".into()))?;
        let revision =
            i64::try_from(next.revision).map_err(|e| StoreError::Other(e.to_string()))?;
        let expected_db = i64::try_from(expected).map_err(|e| StoreError::Other(e.to_string()))?;
        let json = serde_json::to_string(&next).map_err(|e| StoreError::Other(e.to_string()))?;
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let changed = if expected == 0 {
            conn.execute(
                "INSERT OR IGNORE INTO worker_task_queues(factory_session,worker_name,revision,state) VALUES(?,?,?,?)",
                params![session, worker, revision, json],
            )?
        } else {
            conn.execute(
                "UPDATE worker_task_queues SET revision=?,state=? WHERE factory_session=? AND worker_name=? AND revision=?",
                params![revision, json, session, worker, expected_db],
            )?
        };
        if changed != 1 {
            return Err(StoreError::Other(
                "Worker task queue changed; reload before replacing it".into(),
            ));
        }
        *state = next;
        Ok(())
    }

    /// Bind a durable spawn payload once, after the worker name is known.
    pub fn bind_spawn(&self, request: i64, session: &str, worker: &str) -> Result<()> {
        let mut conn = crate::shared_db::lock_connection(&self.conn)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let json: Option<String> = tx.query_row(
            "SELECT task_ids FROM spawn_task_queues WHERE request_id=? AND bound_worker IS NULL",
            [request], |row| row.get(0),
        ).optional()?;
        if let Some(json) = json {
            let ids: Vec<String> =
                serde_json::from_str(&json).map_err(|e| StoreError::Other(e.to_string()))?;
            let mut state = WorkerTaskQueue::default();
            state.current_task = ids.first().cloned();
            state.pending = ids.into_iter().skip(1).collect();
            state.revision = 1;
            let json =
                serde_json::to_string(&state).map_err(|e| StoreError::Other(e.to_string()))?;
            tx.execute(
                "INSERT INTO worker_task_queues(factory_session,worker_name,revision,state) VALUES(?,?,1,?) ON CONFLICT(factory_session,worker_name) DO UPDATE SET revision=worker_task_queues.revision+1,state=excluded.state",
                params![session, worker, json],
            )?;
            // Keep the JSON and CAS revision equal on reuse of a named worker.
            let revision: i64 = tx.query_row(
                "SELECT revision FROM worker_task_queues WHERE factory_session=? AND worker_name=?",
                params![session, worker],
                |row| row.get(0),
            )?;
            state.revision =
                u64::try_from(revision).map_err(|e| StoreError::Other(e.to_string()))?;
            tx.execute(
                "UPDATE worker_task_queues SET state=? WHERE factory_session=? AND worker_name=?",
                params![
                    serde_json::to_string(&state).map_err(|e| StoreError::Other(e.to_string()))?,
                    session,
                    worker
                ],
            )?;
            tx.execute(
                "UPDATE spawn_task_queues SET bound_worker=? WHERE request_id=?",
                params![worker, request],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_survives_reopen_and_rejects_stale_replacement() {
        let root = tempfile::tempdir().unwrap();
        let store = SqliteWorkerTaskQueueStore::open(root.path()).unwrap();
        let mut first = store.get("session", "worker").unwrap();
        let mut stale = first.clone();
        first.pending = vec!["cas-b".into(), "cas-a".into()];
        store.save("session", "worker", &mut first).unwrap();
        stale.pending = vec!["cas-lost".into()];
        assert!(store.save("session", "worker", &mut stale).is_err());
        let reopened = SqliteWorkerTaskQueueStore::open(root.path()).unwrap();
        assert_eq!(reopened.get("session", "worker").unwrap(), first);
        assert!(
            reopened
                .get("another-session", "worker")
                .unwrap()
                .pending
                .is_empty()
        );
    }
}
