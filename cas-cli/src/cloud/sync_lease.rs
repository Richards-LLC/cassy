//! One cloud sync at a time per store (cas-f1fb).
//!
//! Background syncers take this lease: each `cas serve` daemon's sync tick,
//! and the hub's idle-store sweep through `cas cloud sync --background`.
//! Without it, a sweep that started just before a session opened would race
//! that session's own daemon sync on the same queue. Several serves open in
//! one project would also all sync on every tick.
//!
//! The lease is an advisory `flock` on `<cas_root>/cloud-sync.lock`. The
//! kernel releases it when the holder exits, so a killed or crashed syncer
//! never leaves a stale lock behind. A manual `cas cloud sync` without
//! `--background` does not take the lease; its behaviour is unchanged.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;

/// File name of the lease inside a store's `.cas` directory.
pub const SYNC_LEASE_FILE: &str = "cloud-sync.lock";

/// Exit code of `cas cloud sync --background` when another syncer holds the
/// lease (`EX_TEMPFAIL`): nothing ran, and the store will be retried later.
pub const SYNC_LEASE_BUSY_EXIT: i32 = 75;

/// The lease file for one store.
pub fn sync_lease_path(cas_root: &Path) -> PathBuf {
    cas_root.join(SYNC_LEASE_FILE)
}

/// A held sync lease. Dropping it releases the lock.
#[derive(Debug)]
pub struct SyncLease {
    file: File,
}

impl SyncLease {
    /// Take the lease without waiting. `Ok(None)` means another syncer holds
    /// it now.
    pub fn try_acquire(cas_root: &Path) -> io::Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(sync_lease_path(cas_root))?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(Self { file })),
            Err(error) if error.kind() == fs2::lock_contended_error().kind() => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl Drop for SyncLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_holder_is_refused_until_the_first_releases() {
        let dir = tempfile::tempdir().unwrap();
        let first = SyncLease::try_acquire(dir.path()).unwrap();
        assert!(first.is_some(), "an unheld lease is granted");
        assert!(
            SyncLease::try_acquire(dir.path()).unwrap().is_none(),
            "a held lease refuses a second syncer"
        );
        drop(first);
        assert!(
            SyncLease::try_acquire(dir.path()).unwrap().is_some(),
            "releasing the lease lets the next syncer in"
        );
    }

    #[test]
    fn leases_are_per_store() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let _held = SyncLease::try_acquire(a.path()).unwrap().unwrap();
        assert!(SyncLease::try_acquire(b.path()).unwrap().is_some());
    }
}
