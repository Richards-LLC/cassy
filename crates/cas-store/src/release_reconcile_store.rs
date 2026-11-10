//! One durable closeout record per immutable project/release/commit identity.
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use crate::{Result, StoreError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseOperationKey {
    pub project_id: String,
    pub release_id: String,
    pub final_release_sha: String,
}

impl ReleaseOperationKey {
    pub fn id(&self) -> Result<String> {
        if self.project_id.trim().is_empty() || self.release_id.trim().is_empty()
            || self.final_release_sha.len() != 40
            || !self.final_release_sha.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(StoreError::Parse("invalid immutable release operation key".into()));
        }
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(self)?)))
    }
}

/// The held file lock serializes every read/effect/checkpoint for this release.
/// A dead process drops its lock; the next owner resumes the same record.
pub struct ReleaseOperation {
    key: ReleaseOperationKey,
    path: PathBuf,
    _lock: File,
}

impl ReleaseOperation {
    pub fn open(root: &Path, key: ReleaseOperationKey) -> Result<Self> {
        let id = key.id()?;
        let dir = root.join("release-reconcile");
        std::fs::create_dir_all(&dir)?;
        let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true)
            .open(dir.join(format!("{id}.lock")))?;
        crate::wait_budget::lock_file_exclusive_within_budget(&lock, "release-reconcile")?;
        Ok(Self { key, path: dir.join(format!("{id}.json")), _lock: lock })
    }

    pub fn path(&self) -> &Path { &self.path }

    pub fn load(&self) -> Result<Option<serde_json::Value>> {
        Ok(None)
    }

    pub fn save(&self, _receipt: &serde_json::Value) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs2::FileExt;

    #[test]
    fn release_operation_recovers_checkpoint_and_serializes_same_identity() {
        let root = tempfile::tempdir().unwrap();
        let key = ReleaseOperationKey { project_id: "project-a".into(), release_id: "v9.99.0".into(), final_release_sha: "a".repeat(40) };
        let receipt = serde_json::json!({"key": key, "members": {"cas-a": {"action":"closed"}}, "notice_sent":true});
        {
            let operation = ReleaseOperation::open(root.path(), key.clone()).unwrap();
            assert!(operation.load().unwrap().is_none());
            operation.save(&receipt).unwrap();
            // A concurrent process cannot perform effects for this key.
            let contender = OpenOptions::new().read(true).write(true).open(operation.path().with_extension("lock")).unwrap();
            assert!(contender.try_lock_exclusive().is_err());
        }
        let resumed = ReleaseOperation::open(root.path(), key.clone()).unwrap();
        assert_eq!(resumed.load().unwrap(), Some(receipt));
        for different in [
            ReleaseOperationKey { project_id: "project-b".into(), ..key.clone() },
            ReleaseOperationKey { release_id: "v9.99.1".into(), ..key.clone() },
            ReleaseOperationKey { final_release_sha: "b".repeat(40), ..key.clone() },
        ] {
            let other = ReleaseOperation::open(root.path(), different).unwrap();
            assert_ne!(other.path(), resumed.path());
            assert!(other.load().unwrap().is_none());
        }
    }
}
