//! One durable closeout record per immutable project/release/commit identity.
use rusqlite::{OptionalExtension, params};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::{Result, StoreError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseOperationKey {
    pub project_id: String,
    pub release_id: String,
    pub final_release_sha: String,
}

impl ReleaseOperationKey {
    pub fn id(&self) -> Result<String> {
        if self.project_id.trim().is_empty()
            || self.release_id.trim().is_empty()
            || self.final_release_sha.len() != 40
            || !self
                .final_release_sha
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err(StoreError::Parse(
                "invalid immutable release operation key".into(),
            ));
        }
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(self)?)))
    }
}

/// The held file lock serializes every read/effect/checkpoint for this release.
/// A dead process drops its lock; the next owner resumes the same record.
pub struct ReleaseOperation {
    key: ReleaseOperationKey,
    path: PathBuf,
    root: PathBuf,
    _lock: File,
}

impl ReleaseOperation {
    pub fn open(root: &Path, key: ReleaseOperationKey) -> Result<Self> {
        let id = key.id()?;
        let dir = root.join("release-reconcile");
        std::fs::create_dir_all(&dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join(format!("{id}.lock")))?;
        crate::wait_budget::lock_file_exclusive_within_budget(&lock, "release-reconcile")?;
        Ok(Self {
            key,
            path: dir.join(format!("{id}.lock")),
            root: root.into(),
            _lock: lock,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn connection(&self) -> Result<std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>> {
        let conn = crate::shared_db::shared_connection(&self.root.join("cas.db"))?;
        crate::shared_db::lock_connection(&conn)?.execute(
            "CREATE TABLE IF NOT EXISTS release_reconcile_operations (
                project_id TEXT NOT NULL, release_id TEXT NOT NULL, final_release_sha TEXT NOT NULL,
                receipt_json TEXT NOT NULL,
                PRIMARY KEY (project_id, release_id, final_release_sha))",
            [],
        )?;
        Ok(conn)
    }

    pub fn load(&self) -> Result<Option<serde_json::Value>> {
        load_release_operation(&self.root, &self.key)
    }

    pub fn save(&self, receipt: &serde_json::Value) -> Result<()> {
        self.validate_receipt(receipt)?;
        let json = serde_json::to_string(receipt)?;
        let conn = self.connection()?;
        let mut guard = crate::shared_db::lock_connection(&conn)?;
        let tx = guard.transaction()?;
        tx.execute(
            "INSERT INTO release_reconcile_operations (project_id,release_id,final_release_sha,receipt_json)
             VALUES (?1,?2,?3,?4) ON CONFLICT(project_id,release_id,final_release_sha)
             DO UPDATE SET receipt_json=excluded.receipt_json",
            params![self.key.project_id, self.key.release_id, self.key.final_release_sha, json],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn validate_receipt(&self, receipt: &serde_json::Value) -> Result<()> {
        let key: ReleaseOperationKey = serde_json::from_value(receipt["key"].clone())?;
        if key != self.key {
            return Err(StoreError::Parse(
                "release receipt identity mismatch".into(),
            ));
        }
        Ok(())
    }
}

/// Read the atomic SQLite checkpoint without taking the effect lock. An
/// in-flight operation can continue while an adapter checks for completion.
pub fn load_release_operation(
    root: &Path,
    key: &ReleaseOperationKey,
) -> Result<Option<serde_json::Value>> {
    key.id()?;
    let path = root.join("cas.db");
    if !path.try_exists()? {
        return Ok(None);
    }
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    crate::shared_db::install_busy_handler(&conn)?;
    let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='release_reconcile_operations')", [], |r| r.get(0))?;
    if !exists {
        return Ok(None);
    }
    let json: Option<String> = conn.query_row(
        "SELECT receipt_json FROM release_reconcile_operations WHERE project_id=?1 AND release_id=?2 AND final_release_sha=?3",
        params![key.project_id, key.release_id, key.final_release_sha], |r| r.get(0),
    ).optional()?;
    json.map(|json| {
        let receipt: serde_json::Value = serde_json::from_str(&json)?;
        let saved: ReleaseOperationKey = serde_json::from_value(receipt["key"].clone())?;
        if &saved != key {
            return Err(StoreError::Parse(
                "release receipt identity mismatch".into(),
            ));
        }
        Ok(receipt)
    })
    .transpose()
}

/// A committed parent close makes its unfinished task-only verifier obsolete.
/// Retire it atomically without minting a verdict or modifying task state.
/// A newer delivery boundary or another SHA is never consumed by this cleanup.
pub fn retire_closed_parent_dispatch(root: &Path, task_id: &str, final_sha: &str) -> Result<bool> {
    let _schema = crate::SqliteVerificationStore::open(root)?;
    let conn = crate::shared_db::shared_connection(&root.join("cas.db"))?;
    let tx = crate::shared_db::begin_immediate_pooled(&conn)?;
    let status: Option<String> = tx
        .query_row("SELECT status FROM tasks WHERE id=?1", [task_id], |r| {
            r.get(0)
        })
        .optional()?;
    if status.as_deref() != Some("closed") {
        tx.commit()?;
        return Ok(false);
    }
    let Some(dispatch) = crate::get_latest_verification_dispatch_with_conn(&tx, task_id)? else {
        tx.commit()?;
        return Ok(false);
    };
    if dispatch.receipt().map_err(crate::StoreError::Parse)?.transaction().is_some()
        || !dispatch
            .repository
            .as_ref()
            .is_some_and(|r| r.head_commit == final_sha)
        || !matches!(
            dispatch.state,
            cas_types::VerificationDispatchState::Pending
                | cas_types::VerificationDispatchState::Claimed
                | cas_types::VerificationDispatchState::TimedOut
        )
    {
        tx.commit()?;
        return Ok(false);
    }
    let now = chrono::Utc::now().to_rfc3339();
    if let Some(capability) = &dispatch.capability_id {
        tx.execute(
            "UPDATE verification_capabilities SET consumed_at=COALESCE(consumed_at,?2) WHERE id=?1",
            params![capability, now],
        )?;
        tx.execute("UPDATE verification_handoffs SET state='consumed', consumed_at=COALESCE(consumed_at,?2) WHERE capability_id=?1 AND state IN ('pending','bound')", params![capability, now])?;
    }
    tx.execute("UPDATE verification_dispatches SET state='invalidated', resolved_at=?2 WHERE id=?1 AND state IN ('pending','claimed','timed_out')", params![dispatch.id, now])?;
    tx.commit()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs2::FileExt;

    #[test]
    fn release_operation_recovers_checkpoint_and_serializes_same_identity() {
        let root = tempfile::tempdir().unwrap();
        let key = ReleaseOperationKey {
            project_id: "project-a".into(),
            release_id: "v9.99.0".into(),
            final_release_sha: "a".repeat(40),
        };
        let receipt = serde_json::json!({"key": key, "members": {"cas-a": {"action":"closed"}}, "notice_sent":true});
        {
            let operation = ReleaseOperation::open(root.path(), key.clone()).unwrap();
            assert!(operation.load().unwrap().is_none());
            operation.save(&receipt).unwrap();
            assert_eq!(
                load_release_operation(root.path(), &key).unwrap(),
                Some(receipt.clone())
            );
            // A concurrent process cannot perform effects for this key.
            let contender = OpenOptions::new()
                .read(true)
                .write(true)
                .open(operation.path().with_extension("lock"))
                .unwrap();
            assert!(contender.try_lock_exclusive().is_err());
        }
        let resumed = ReleaseOperation::open(root.path(), key.clone()).unwrap();
        assert_eq!(resumed.load().unwrap(), Some(receipt));
        for different in [
            ReleaseOperationKey {
                project_id: "project-b".into(),
                ..key.clone()
            },
            ReleaseOperationKey {
                release_id: "v9.99.1".into(),
                ..key.clone()
            },
            ReleaseOperationKey {
                final_release_sha: "b".repeat(40),
                ..key.clone()
            },
        ] {
            let other = ReleaseOperation::open(root.path(), different).unwrap();
            assert_ne!(other.path(), resumed.path());
            assert!(other.load().unwrap().is_none());
        }
    }
}
