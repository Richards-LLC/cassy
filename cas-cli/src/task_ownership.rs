//! Proving that a bare task id belongs to this project (cas-6162).
//!
//! Legacy artifact directories (`~/.cas/artifacts/cas-1a2b/`) and simulator
//! names (`cas-1a2b-proof`) carry a task id but no project. Task ids are short,
//! so another project on the same host can have the same id. A bare id counts
//! as this project's only when no other known project's task store holds it;
//! a store that cannot be read makes every bare id unproven.
//!
//! Other projects' stores are never written. Their main database file is read
//! with SQLite's `immutable=1`, which takes no locks and creates, checkpoints
//! or deletes no `-wal`/`-shm` file. An immutable read does not see rows still
//! in the write-ahead log, so a non-empty `-wal` is also scanned, read-only,
//! for the candidate ids; any hit counts as holding the id (conservative). An
//! id that was never scanned against a store with a WAL is unproven.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Other projects' task stores, consulted read-only.
pub trait OtherProjects {
    /// The other projects whose store holds `task_id`. `Err` means ownership
    /// cannot be proven (a store is missing or unreadable).
    fn holders(&self, task_id: &str) -> Result<Vec<String>, String>;

    /// Look up many ids at once before calling [`Self::holders`]; a store's
    /// write-ahead log is then read once for all of them.
    fn prefetch(&self, _task_ids: &[String]) {}
}

/// One other project's store.
struct Store {
    label: String,
    ids: HashSet<String>,
    /// A non-empty `-wal` beside the database, scanned on demand.
    wal: Option<PathBuf>,
}

/// Task ids of every other known project.
pub struct KnownProjects {
    stores: Result<Vec<Store>, String>,
    /// Ids already scanned against every WAL, with the stores that hold them.
    wal_hits: RefCell<HashMap<String, Vec<String>>>,
    wal_error: RefCell<Option<String>>,
}

impl KnownProjects {
    /// Every repo in the host registry except the one owning `cas_root`.
    pub fn load(cas_root: &Path) -> Self {
        let repos = crate::store::known_repos::open_host_known_repo_store()
            .and_then(|store| cas_store::KnownRepoStore::list(&store).map_err(Into::into));
        match repos {
            Ok(repos) => Self::from_roots(cas_root, repos.into_iter().map(|repo| repo.path)),
            Err(error) => Self::failed(format!("known-repo registry unreadable: {error}")),
        }
    }

    fn failed(error: String) -> Self {
        Self { stores: Err(error), wal_hits: RefCell::default(), wal_error: RefCell::default() }
    }

    /// Other projects from explicit repo roots (each with a `.cas/` store).
    pub fn from_roots(cas_root: &Path, roots: impl IntoIterator<Item = PathBuf>) -> Self {
        let this = std::fs::canonicalize(cas_root).unwrap_or_else(|_| cas_root.to_path_buf());
        let mut stores = Vec::new();
        let mut unreadable = Vec::new();
        for root in roots {
            let other = root.join(".cas");
            if std::fs::canonicalize(&other).is_ok_and(|other| other == this) {
                continue;
            }
            let db = other.join("cas.db");
            match main_task_ids(&db) {
                Ok(ids) => {
                    let wal = PathBuf::from(format!("{}-wal", db.display()));
                    let wal = std::fs::metadata(&wal).is_ok_and(|meta| meta.len() > 0).then_some(wal);
                    stores.push(Store { label: root.display().to_string(), ids, wal });
                }
                Err(error) => unreadable.push(format!("{} ({error})", root.display())),
            }
        }
        if unreadable.is_empty() {
            Self { stores: Ok(stores), wal_hits: RefCell::default(), wal_error: RefCell::default() }
        } else {
            Self::failed(format!(
                "task stores of other known projects cannot be read: {}; forget missing repos with `cas known-repos` or restore them",
                unreadable.join(", ")
            ))
        }
    }
}

impl OtherProjects for KnownProjects {
    fn prefetch(&self, task_ids: &[String]) {
        let Ok(stores) = &self.stores else { return };
        let pending: Vec<&String> = {
            let hits = self.wal_hits.borrow();
            task_ids.iter().filter(|id| !hits.contains_key(*id)).collect()
        };
        if pending.is_empty() {
            return;
        }
        let mut found: HashMap<String, Vec<String>> =
            pending.iter().map(|id| ((*id).clone(), Vec::new())).collect();
        let pattern = pending.iter().map(|id| regex::escape(id)).collect::<Vec<_>>().join("|");
        let matcher = match regex::bytes::Regex::new(&pattern) {
            Ok(matcher) => matcher,
            Err(error) => {
                *self.wal_error.borrow_mut() = Some(format!("cannot scan write-ahead logs: {error}"));
                return;
            }
        };
        for store in stores {
            let Some(wal) = &store.wal else { continue };
            match std::fs::read(wal) {
                Ok(bytes) => {
                    for hit in matcher.find_iter(&bytes) {
                        let id = String::from_utf8_lossy(hit.as_bytes()).into_owned();
                        if let Some(holders) = found.get_mut(&id)
                            && !holders.contains(&store.label)
                        {
                            holders.push(store.label.clone());
                        }
                    }
                }
                Err(error) => {
                    *self.wal_error.borrow_mut() =
                        Some(format!("write-ahead log of {} unreadable: {error}", store.label));
                    return;
                }
            }
        }
        self.wal_hits.borrow_mut().extend(found);
    }

    fn holders(&self, task_id: &str) -> Result<Vec<String>, String> {
        let stores = self.stores.as_ref().map_err(Clone::clone)?;
        if stores.iter().any(|store| store.wal.is_some()) {
            self.prefetch(&[task_id.to_string()]);
        }
        if let Some(error) = self.wal_error.borrow().clone() {
            return Err(error);
        }
        let mut holders: BTreeMap<String, ()> = stores
            .iter()
            .filter(|store| store.ids.contains(task_id))
            .map(|store| (store.label.clone(), ()))
            .collect();
        if let Some(from_wal) = self.wal_hits.borrow().get(task_id) {
            holders.extend(from_wal.iter().map(|label| (label.clone(), ())));
        }
        Ok(holders.into_keys().collect())
    }
}

/// Every task id in a store's main database file, read with `immutable=1`
/// so the other project's files are never written, locked or checkpointed.
fn main_task_ids(db: &Path) -> Result<HashSet<String>, String> {
    if !db.is_file() {
        return Err("no cas.db".into());
    }
    let uri = format!("file:{}?immutable=1", uri_path(db));
    let conn = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| error.to_string())?;
    // A store whose schema still lives only in the WAL has no tasks table in
    // the main file yet; its ids are found by the WAL scan.
    let has_tasks: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='tasks')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_tasks {
        return Ok(HashSet::new());
    }
    let mut statement = conn.prepare("SELECT id FROM tasks").map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|error| error.to_string())
}

/// A path as an SQLite URI path (`%`, `?` and `#` escaped).
fn uri_path(path: &Path) -> String {
    path.display().to_string().replace('%', "%25").replace('?', "%3f").replace('#', "%23")
}

/// Whether `name` is exactly a task id (`cas-` and 4 to 16 hex digits).
pub fn is_task_id(name: &str) -> bool {
    name.strip_prefix("cas-")
        .is_some_and(|hex| (4..=16).contains(&hex.len()) && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cas-6162: another project's store with a leftover write-ahead log is
    /// never written (database, WAL and the absent -shm are unchanged), and
    /// an id that lives only in that WAL still counts as held.
    #[test]
    fn a_leftover_wal_in_another_store_is_read_without_writing_cas_6162() {
        let live = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let this = tempfile::tempdir().unwrap();
        let db = live.path().join("cas.db");
        let writer = rusqlite::Connection::open(&db).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE tasks (id TEXT PRIMARY KEY);
                 INSERT INTO tasks VALUES ('cas-aaaa');
                 PRAGMA wal_checkpoint(TRUNCATE);
                 INSERT INTO tasks VALUES ('cas-bbbb');",
            )
            .unwrap();
        // Copy the files while the writer is open: the copy has a leftover WAL
        // holding cas-bbbb and no live connection, like a crashed project.
        let store = other.path().join(".cas");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::copy(&db, store.join("cas.db")).unwrap();
        std::fs::copy(live.path().join("cas.db-wal"), store.join("cas.db-wal")).unwrap();
        drop(writer);
        let before = (std::fs::read(store.join("cas.db")).unwrap(), std::fs::read(store.join("cas.db-wal")).unwrap());
        assert!(!before.1.is_empty(), "fixture keeps a leftover WAL");

        let projects = KnownProjects::from_roots(&this.path().join(".cas"), vec![other.path().to_path_buf()]);
        projects.prefetch(&["cas-aaaa".into(), "cas-bbbb".into(), "cas-cccc".into()]);
        let label = other.path().display().to_string();
        assert_eq!(projects.holders("cas-aaaa").unwrap(), vec![label.clone()], "checkpointed row");
        assert_eq!(projects.holders("cas-bbbb").unwrap(), vec![label], "row only in the WAL");
        assert!(projects.holders("cas-cccc").unwrap().is_empty());
        drop(projects);

        let after = (std::fs::read(store.join("cas.db")).unwrap(), std::fs::read(store.join("cas.db-wal")).unwrap());
        assert!(before == after, "the other store's database and WAL are unchanged");
        assert!(!store.join("cas.db-shm").exists(), "no -shm is created");
    }
}
