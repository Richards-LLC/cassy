//! Process-cached task store for long-lived processes such as the factory
//! daemon (GH #1165).
//!
//! [`open_task_store`] builds a fresh wrapper stack on every call. When the
//! project is logged in to the cloud it also runs
//! `reconcile_pending_task_sync`, which takes an exclusive flock on
//! `task-sync-intents.lock`, even for a caller that only reads. The factory
//! daemon called it hundreds of times a minute from its UI loop and queued
//! behind other processes' flock holders for up to four minutes.
//!
//! [`open_task_store_cached`] opens the same wrapper stack once per CAS
//! directory, without that per-open reconcile, and hands out clones. Reads
//! through it take no flock. Mutations still serialise on the intents lock
//! inside `SyncingTaskStore`. The repair work runs through
//! [`reconcile_task_sync`], which the daemon calls from a background thread.
//!
//! The cache lives for the process. A cloud login or logout after the first
//! open takes effect when the process restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use cas_store::TaskStore;

use crate::cloud::{CloudConfig, SyncQueue};
use crate::error::CasError;
use crate::store::{SyncingTaskStore, open_task_store, open_task_store_local};

type Result<T> = std::result::Result<T, CasError>;

fn cache() -> &'static Mutex<HashMap<PathBuf, Arc<dyn TaskStore>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<dyn TaskStore>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The task store for `cas_dir`, opened once per process and never
/// reconciled on open. Use it on threads that must not block, and anywhere a
/// long-lived process would otherwise reopen the store per call.
pub fn open_task_store_cached(cas_dir: &Path) -> Result<Arc<dyn TaskStore>> {
    if let Some(store) = cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(cas_dir)
    {
        return Ok(Arc::clone(store));
    }
    // Open outside the cache lock: an open can do schema work. Two racing
    // first opens both succeed and the first insert wins.
    let opened = open_task_store_unreconciled(cas_dir)?;
    let mut cache = cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok(Arc::clone(
        cache.entry(cas_dir.to_path_buf()).or_insert(opened),
    ))
}

/// The wrapper stack [`open_task_store`] builds, minus its per-open reconcile.
fn open_task_store_unreconciled(cas_dir: &Path) -> Result<Arc<dyn TaskStore>> {
    let base_store = open_task_store_local(cas_dir)?;
    if let Ok(cloud_config) = CloudConfig::load_from_cas_dir(cas_dir)
        && cloud_config.is_logged_in()
    {
        let queue = SyncQueue::open(cas_dir)?;
        queue.init()?;
        let store = SyncingTaskStore::new(base_store, Arc::new(queue))
            .with_cloud_config(Arc::new(cloud_config));
        return Ok(Arc::new(store));
    }
    Ok(base_store)
}

/// Run the pending task-sync repair [`open_task_store`] performs on open.
///
/// It may wait on the intents flock and on SQLite, so call it from a
/// background thread, never from a UI loop.
pub fn reconcile_task_sync(cas_dir: &Path) -> Result<()> {
    open_task_store(cas_dir).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_store_is_shared_per_cas_dir() {
        let dir = tempfile::tempdir().unwrap();
        let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
        let first = open_task_store_cached(&cas_dir).unwrap();
        let second = open_task_store_cached(&cas_dir).unwrap();
        assert!(Arc::ptr_eq(&first, &second));

        let task = cas_types::Task::new("cas-c1a1".into(), "cached".into());
        first.add(&task).unwrap();
        assert_eq!(second.get("cas-c1a1").unwrap().title, "cached");
        // Writes through a fresh, reconciling open are visible too.
        let fresh = open_task_store(&cas_dir).unwrap();
        assert_eq!(fresh.get("cas-c1a1").unwrap().title, "cached");
        reconcile_task_sync(&cas_dir).unwrap();
    }
}
