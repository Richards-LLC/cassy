//! Proving that a bare task id belongs to this project (cas-6162).
//!
//! Legacy artifact directories (`~/.cas/artifacts/cas-1a2b/`) and simulator
//! names (`cas-1a2b-proof`) carry a task id but no project. Task ids are short,
//! so another project on the same host can have the same id. A bare id counts
//! as this project's only when no other known project's task store holds it;
//! a store that cannot be read makes every bare id unproven.
//!
//! Other projects' stores are never written. Each store is read in one
//! consistent pass (cas-6162): its `-wal` bytes first, then its main database
//! with SQLite's `immutable=1` (no locks; nothing created, checkpointed or
//! deleted), then both are re-checked. If the WAL header or size, or the
//! database size or mtime, changed during the pass (a checkpoint and WAL
//! restart in between would otherwise hide a row), the store is unproven.
//! Ids still only in the WAL are found by scanning its bytes: at every `cas-`
//! the run of hex digits after it yields every id length (4 to 16), so a
//! longer id is never shadowed by a shorter one and adjacent bytes can only
//! add holders, never remove one.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Other projects' task stores, consulted read-only.
pub trait OtherProjects {
    /// The other projects whose store holds `task_id`. `Err` means ownership
    /// cannot be proven (a store is missing, unreadable or changed while read).
    fn holders(&self, task_id: &str) -> Result<Vec<String>, String>;

    /// Kept for callers that batch lookups; reads happen at load.
    fn prefetch(&self, _task_ids: &[String]) {}
}

/// Task ids of every other known project, read once at load.
pub struct KnownProjects {
    stores: Result<Vec<(String, HashSet<String>)>, String>,
}

impl KnownProjects {
    /// Every repo in the host registry except the one owning `cas_root`.
    pub fn load(cas_root: &Path) -> Self {
        let repos = crate::store::known_repos::open_host_known_repo_store()
            .and_then(|store| cas_store::KnownRepoStore::list(&store).map_err(Into::into));
        match repos {
            Ok(repos) => Self::from_roots(cas_root, repos.into_iter().map(|repo| repo.path)),
            Err(error) => Self { stores: Err(format!("known-repo registry unreadable: {error}")) },
        }
    }

    /// Other projects from explicit repo roots (each with a `.cas/` store).
    pub fn from_roots(cas_root: &Path, roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self::from_roots_with(cas_root, roots, &|| {})
    }

    /// [`Self::from_roots`] with a hook run between a store's WAL read and
    /// its main-database read (tests use it to race a checkpoint).
    fn from_roots_with(cas_root: &Path, roots: impl IntoIterator<Item = PathBuf>, between: &dyn Fn()) -> Self {
        let this = std::fs::canonicalize(cas_root).unwrap_or_else(|_| cas_root.to_path_buf());
        let mut stores = Vec::new();
        let mut unreadable = Vec::new();
        for root in roots {
            let other = root.join(".cas");
            if std::fs::canonicalize(&other).is_ok_and(|other| other == this) {
                continue;
            }
            match store_task_ids(&other.join("cas.db"), between) {
                Ok(ids) => stores.push((root.display().to_string(), ids)),
                Err(error) => unreadable.push(format!("{} ({error})", root.display())),
            }
        }
        if unreadable.is_empty() {
            Self { stores: Ok(stores) }
        } else {
            Self {
                stores: Err(format!(
                    "task stores of other known projects cannot be read: {}; forget missing repos with `cas known-repos` or restore them",
                    unreadable.join(", ")
                )),
            }
        }
    }
}

impl OtherProjects for KnownProjects {
    fn holders(&self, task_id: &str) -> Result<Vec<String>, String> {
        let stores = self.stores.as_ref().map_err(Clone::clone)?;
        let holders: BTreeMap<&str, ()> = stores
            .iter()
            .filter(|(_, ids)| ids.contains(task_id))
            .map(|(label, _)| (label.as_str(), ()))
            .collect();
        Ok(holders.into_keys().map(str::to_string).collect())
    }
}

/// What must not change while one store is read.
#[derive(Debug, PartialEq, Eq)]
struct Fingerprint {
    db_len: u64,
    db_mtime: Option<std::time::SystemTime>,
    wal_len: u64,
    /// The WAL header (magic, version, page size, checkpoint sequence,
    /// salts, checksums): a checkpoint restart rewrites it.
    wal_header: Vec<u8>,
}

fn fingerprint(db: &Path, wal: &Path) -> Result<Fingerprint, String> {
    let meta = std::fs::metadata(db).map_err(|error| error.to_string())?;
    let (wal_len, wal_header) = match std::fs::File::open(wal) {
        Ok(mut file) => {
            use std::io::Read;
            let len = file.metadata().map_err(|error| error.to_string())?.len();
            let mut header = vec![0; 32.min(len as usize)];
            file.read_exact(&mut header).map_err(|error| error.to_string())?;
            (len, header)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (0, Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    Ok(Fingerprint { db_len: meta.len(), db_mtime: meta.modified().ok(), wal_len, wal_header })
}

/// Every task id a store holds: WAL bytes first, then the immutable main
/// database, then a re-check that neither changed in between.
fn store_task_ids(db: &Path, between: &dyn Fn()) -> Result<HashSet<String>, String> {
    if !db.is_file() {
        return Err("no cas.db".into());
    }
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    let before = fingerprint(db, &wal)?;
    let wal_bytes = match std::fs::read(&wal) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("write-ahead log unreadable: {error}")),
    };
    between();
    let mut ids = main_task_ids(db)?;
    if fingerprint(db, &wal)? != before || wal_bytes.len() as u64 != before.wal_len {
        return Err("store changed while it was read".into());
    }
    ids.extend(wal_task_ids(&wal_bytes));
    Ok(ids)
}

/// Every possible task id in raw WAL bytes: at each `cas-`, the hex run
/// after it contributes each prefix of 4 to 16 digits.
fn wal_task_ids(bytes: &[u8]) -> HashSet<String> {
    let mut ids = HashSet::new();
    let mut at = 0;
    while let Some(offset) = bytes[at..].windows(4).position(|window| window == b"cas-") {
        let start = at + offset + 4;
        let run = bytes[start..].iter().take(16).take_while(|byte| byte.is_ascii_hexdigit()).count();
        for len in 4..=run {
            ids.insert(format!("cas-{}", String::from_utf8_lossy(&bytes[start..start + len])));
        }
        at = start;
    }
    ids
}

/// Every task id in a store's main database file, read with `immutable=1`
/// so the other project's files are never written, locked or checkpointed.
/// Any read anomaly is an error, which makes the store unproven.
fn main_task_ids(db: &Path) -> Result<HashSet<String>, String> {
    let uri = format!("file:{}?immutable=1", uri_path(db));
    let conn = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| error.to_string())?;
    // A store whose schema still lives only in the WAL has no tasks table in
    // the main file yet; its ids come from the WAL scan.
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

    /// A store with a WAL-only row, as an idle copy of a live store.
    fn store_with_wal(dir: &Path, checkpointed: &[&str], wal_only: &[&str]) -> PathBuf {
        let live = dir.join("live");
        std::fs::create_dir_all(&live).unwrap();
        let db = live.join("cas.db");
        let writer = rusqlite::Connection::open(&db).unwrap();
        writer
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT);")
            .unwrap();
        for id in checkpointed {
            writer.execute("INSERT INTO tasks VALUES (?1, 'deadbeef')", [id]).unwrap();
        }
        writer.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
        for id in wal_only {
            // A hex-looking neighbour column, so ids run into adjacent bytes.
            writer.execute("INSERT INTO tasks VALUES (?1, 'abcdef0123')", [id]).unwrap();
        }
        let root = dir.join("other");
        let store = root.join(".cas");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::copy(&db, store.join("cas.db")).unwrap();
        std::fs::copy(live.join("cas.db-wal"), store.join("cas.db-wal")).unwrap();
        root
    }

    /// cas-6162 (a): a shorter id never shadows a longer one in the WAL scan;
    /// both cas-02a7 and cas-02a7d rows are held.
    #[test]
    fn a_shorter_wal_id_never_shadows_a_longer_one_cas_6162() {
        let dir = tempfile::tempdir().unwrap();
        let this = tempfile::tempdir().unwrap();
        let root = store_with_wal(dir.path(), &[], &["cas-02a7", "cas-02a7d"]);
        let projects = KnownProjects::from_roots(&this.path().join(".cas"), vec![root.clone()]);
        let label = root.display().to_string();
        assert_eq!(projects.holders("cas-02a7d").unwrap(), vec![label.clone()]);
        assert_eq!(projects.holders("cas-02a7").unwrap(), vec![label]);
    }

    /// cas-6162 (b): a checkpoint and WAL restart between the WAL read and
    /// the main-database read makes the store unproven, never "not held".
    #[test]
    fn a_checkpoint_between_reads_makes_the_store_unproven_cas_6162() {
        let dir = tempfile::tempdir().unwrap();
        let this = tempfile::tempdir().unwrap();
        let root = store_with_wal(dir.path(), &["cas-aaaa"], &["cas-bbbb"]);
        let wal = root.join(".cas/cas.db-wal");
        // The race: the WAL is restarted (new header salts) after it was read.
        let restart = || {
            let mut bytes = std::fs::read(&wal).unwrap();
            for byte in &mut bytes[16..24] {
                *byte ^= 0xff;
            }
            std::fs::write(&wal, bytes).unwrap();
        };
        let projects = KnownProjects::from_roots_with(&this.path().join(".cas"), vec![root], &restart);
        let error = projects.holders("cas-bbbb").unwrap_err();
        assert!(error.contains("changed while it was read"), "{error}");
    }
}
