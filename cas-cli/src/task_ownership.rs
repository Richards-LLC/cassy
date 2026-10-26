//! Proving that a bare task id belongs to this project (cas-6162).
//!
//! Legacy artifact directories (`~/.cas/artifacts/cas-1a2b/`) and simulator
//! names (`cas-1a2b-proof`) carry a task id but no project. Task ids are short,
//! so another project on the same host can have the same id. A bare id counts
//! as this project's only when no other known project's task store holds it;
//! a store that cannot be read makes every bare id unproven.
//!
//! The host's global store (`~/.cas/cas.db`, global-scope tasks) is one of
//! the other stores (cas-2191).
//!
//! Other projects' stores are never written. Raw reads, immutable SQLite and
//! lock probes run in fresh exec children: closing any inode descriptor in
//! this process could release another thread's SQLite POSIX locks (cas-5a02).
//! Each store is read in one
//! consistent pass (cas-6162): its `-wal` bytes first, then its main database
//! with SQLite's `immutable=1` (no locks; nothing created, checkpointed or
//! deleted), then both are re-checked. The check covers the WAL header and
//! size, the database size, mtime and header change counter, and the
//! `-shm` wal-index header and checkpoint progress (cas-2191). In WAL mode a
//! checkpoint leaves the change counter alone and may not move a coarse
//! mtime, but it always advances the wal-index. A store that changed during
//! the pass is read again, a bounded number of times, and is otherwise
//! unproven.
//! Ids still only in the WAL are found by scanning its bytes: at every `cas-`
//! the run of hex digits after it yields every id length (4 to 16), so a
//! longer id is never shadowed by a shorter one and adjacent bytes can only
//! add holders, never remove one.

use std::cell::OnceCell;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Other projects' task stores, consulted read-only.
pub trait OtherProjects {
    /// The other projects whose store holds `task_id`. `Err` means ownership
    /// cannot be proven (a store is missing, unreadable or changed while read).
    fn holders(&self, task_id: &str) -> Result<Vec<String>, String>;
}

type StoreIds = Result<Vec<(String, HashSet<String>)>, String>;

/// One store to consult: a known repo's `.cas/` (which must exist) or the
/// host's global store (absent means it holds nothing).
struct StoreSpec {
    label: String,
    cas_dir: PathBuf,
    required: bool,
}

/// How many times a store that changed while it was read is read again
/// before it counts as unproven (cas-2191).
const STORE_READ_ATTEMPTS: u32 = 3;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Task ids of every other store on the host, read on first lookup.
pub struct KnownProjects {
    cas_root: PathBuf,
    stores: OnceCell<StoreIds>,
}

impl KnownProjects {
    /// Every repo in the host registry and the host's global store, except
    /// the one owning `cas_root`. Nothing is read until the first lookup, so
    /// a caller that never needs an answer (simctl missing, no candidates)
    /// opens no other store (cas-2191).
    pub fn load(cas_root: &Path) -> Self {
        Self { cas_root: cas_root.to_path_buf(), stores: OnceCell::new() }
    }

    fn read_registry(cas_root: &Path) -> StoreIds {
        let repos = crate::store::known_repos::open_host_known_repo_store()
            .and_then(|store| cas_store::KnownRepoStore::list(&store).map_err(Into::into))
            .map_err(|error| format!("known-repo registry unreadable: {error}"))?;
        let mut specs: Vec<StoreSpec> = repos.into_iter().map(|repo| repo_spec(repo.path)).collect();
        specs.push(StoreSpec {
            label: "global store".into(),
            cas_dir: crate::store::known_repos::host_cas_dir(),
            required: false,
        });
        read_stores(cas_root, specs, &|| {})
    }

    /// Other projects from explicit repo roots (each with a `.cas/` store).
    pub fn from_roots(cas_root: &Path, roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self::from_specs_with(cas_root, roots.into_iter().map(repo_spec).collect(), &|| {})
    }

    /// Read `specs` now, running `between` between each store's WAL read and
    /// its main-database read (tests use it to race a checkpoint).
    fn from_specs_with(cas_root: &Path, specs: Vec<StoreSpec>, between: &dyn Fn()) -> Self {
        Self { cas_root: cas_root.to_path_buf(), stores: OnceCell::from(read_stores(cas_root, specs, between)) }
    }

    fn stores(&self) -> &StoreIds {
        self.stores.get_or_init(|| Self::read_registry(&self.cas_root))
    }
}

fn repo_spec(root: PathBuf) -> StoreSpec {
    StoreSpec { label: root.display().to_string(), cas_dir: root.join(".cas"), required: true }
}

fn read_stores(cas_root: &Path, specs: Vec<StoreSpec>, between: &dyn Fn()) -> StoreIds {
    let this = std::fs::canonicalize(cas_root).unwrap_or_else(|_| cas_root.to_path_buf());
    let mut stores = Vec::new();
    let mut unreadable = Vec::new();
    for spec in specs {
        if std::fs::canonicalize(&spec.cas_dir).is_ok_and(|other| other == this) {
            continue;
        }
        let db = spec.cas_dir.join("cas.db");
        // cas-69b5: only a store that is not there is absent; one that
        // cannot be looked at (permission denied) is unreadable, not empty.
        if !spec.required
            && std::fs::metadata(&db).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            continue;
        }
        match store_task_ids_retrying(&db, between) {
            Ok(ids) => stores.push((spec.label, ids)),
            Err(error) => unreadable.push(format!("{} ({error})", spec.label)),
        }
    }
    if unreadable.is_empty() {
        Ok(stores)
    } else {
        Err(format!(
            "task stores of other known projects cannot be read: {}; forget missing repos with `cas known-repos` or restore them",
            unreadable.join(", ")
        ))
    }
}

/// [`store_task_ids`], read again after a short pause while the store keeps
/// changing under the read, up to [`STORE_READ_ATTEMPTS`] times.
fn store_task_ids_retrying(db: &Path, between: &dyn Fn()) -> Result<HashSet<String>, String> {
    let mut attempt = 1;
    let deadline = crate::bounded_process::Deadline::after(PROBE_TIMEOUT);
    loop {
        match store_task_ids_with_deadline(db, between, deadline) {
            Err(ReadError::Changed) if attempt < STORE_READ_ATTEMPTS => {
                std::thread::sleep(std::time::Duration::from_millis(25 * u64::from(attempt)));
                attempt += 1;
            }
            Err(ReadError::Changed) => {
                return Err(format!("store changed while it was read, {STORE_READ_ATTEMPTS} times"));
            }
            Err(ReadError::Failed(error)) => return Err(error),
            Ok(ids) => return Ok(ids),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
enum ReadError {
    /// The store was written during the read.
    Changed,
    Failed(String),
}

impl From<String> for ReadError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

impl OtherProjects for KnownProjects {
    fn holders(&self, task_id: &str) -> Result<Vec<String>, String> {
        let stores = self.stores().as_ref().map_err(Clone::clone)?;
        let holders: BTreeMap<&str, ()> = stores
            .iter()
            .filter(|(_, ids)| ids.contains(task_id))
            .map(|(label, _)| (label.as_str(), ()))
            .collect();
        Ok(holders.into_keys().map(str::to_string).collect())
    }
}

/// What must not change while one store is read.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Fingerprint {
    db_len: u64,
    db_mtime: Option<std::time::SystemTime>,
    /// The database header's file change counter (bytes 24..28). A
    /// rollback-journal commit advances it; WAL mode does not.
    db_change_counter: Vec<u8>,
    wal_len: u64,
    /// The WAL header (magic, version, page size, checkpoint sequence,
    /// salts, checksums): a checkpoint restart rewrites it.
    wal_header: Vec<u8>,
    /// The `-shm` wal-index header (frame count, change count) and the
    /// checkpoint progress (backfilled frames). Every commit and every
    /// checkpoint moves one of them; reader marks are left out.
    wal_index: Vec<u8>,
    /// Another process holds SQLite's checkpointer lock. A checkpoint that
    /// spans the whole read can leave the wal-index unchanged. Backfill
    /// counters cannot establish activity: WAL recovery also raises
    /// nBackfillAttempted ahead of nBackfill (cas-593e).
    checkpointing: bool,
}

/// Up to `len` bytes of `path` from `start`; a missing file reads empty.
fn file_bytes(path: &Path, start: u64, len: usize) -> Result<(u64, Vec<u8>), String> {
    use std::io::{Read, Seek, SeekFrom};
    match std::fs::File::open(path) {
        Ok(mut file) => {
            let size = file.metadata().map_err(|error| error.to_string())?.len();
            let mut bytes = vec![0; len.min(size.saturating_sub(start) as usize)];
            file.seek(SeekFrom::Start(start)).map_err(|error| error.to_string())?;
            file.read_exact(&mut bytes).map_err(|error| error.to_string())?;
            Ok((size, bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((0, Vec::new())),
        Err(error) => Err(error.to_string()),
    }
}

/// Observe SQLite's exclusive WAL_CKPT_LOCK (shm byte 121), without
/// acquiring it or opening a SQLite connection that might recover the WAL.
/// This runs only in an exec child, so F_GETLK also observes the caller's
/// locks, and closing this descriptor cannot release them (cas-5a02).
#[cfg(unix)]
fn checkpoint_in_progress(shm: &Path) -> Result<bool, String> {
    use std::os::fd::AsRawFd;
    let file = match std::fs::File::open(shm) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    // SAFETY: flock is a plain C struct; all fields are initialized before
    // fcntl reads them, including platform-specific padding/extra fields.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    lock.l_start = 121;
    lock.l_len = 1;
    // SAFETY: file owns a live descriptor and lock is a valid output buffer.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut lock) } == -1 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(lock.l_type != libc::F_UNLCK as libc::c_short)
}

#[cfg(not(unix))]
fn checkpoint_in_progress(_shm: &Path) -> Result<bool, String> {
    Err("checkpoint lock inspection is unavailable on this platform".into())
}

fn fingerprint(db: &Path, wal: &Path) -> Result<Fingerprint, String> {
    let meta = std::fs::metadata(db).map_err(|error| error.to_string())?;
    let (_, db_change_counter) = file_bytes(db, 24, 4)?;
    let (wal_len, wal_header) = file_bytes(wal, 0, 32)?;
    let shm = PathBuf::from(format!("{}-shm", db.display()));
    let (_, header) = file_bytes(&shm, 0, 48)?;
    let (_, backfill) = file_bytes(&shm, 96, 4)?;
    let (_, attempted) = file_bytes(&shm, 128, 4)?;
    let checkpointing = checkpoint_in_progress(&shm)?;
    let wal_index = [header, backfill, attempted].concat();
    Ok(Fingerprint {
        db_len: meta.len(),
        db_mtime: meta.modified().ok(),
        db_change_counter,
        wal_len,
        wal_header,
        wal_index,
        checkpointing,
    })
}

/// Raw inspection is confined to a fresh exec process. In particular, even
/// an immutable SQLite read or a raw header read may close an inode on which
/// the caller owns POSIX locks; doing that in the caller releases those locks.
#[derive(serde::Serialize, serde::Deserialize)]
enum ProbeSnapshot {
    Before {
        fingerprint: Fingerprint,
        ids: HashSet<String>,
        wal_len: u64,
    },
    After {
        fingerprint: Fingerprint,
        ids: HashSet<String>,
    },
}

fn probe_snapshot(phase: &str, db: &Path) -> Result<ProbeSnapshot, ReadError> {
    if !db.is_file() {
        return Err(ReadError::Failed("no cas.db".into()));
    }
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    match phase {
        "before" => {
            let fingerprint = fingerprint(db, &wal)?;
            if fingerprint.checkpointing {
                return Err(ReadError::Changed);
            }
            let bytes = match std::fs::read(&wal) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => {
                    return Err(ReadError::Failed(format!(
                        "write-ahead log unreadable: {error}"
                    )));
                }
            };
            Ok(ProbeSnapshot::Before {
                fingerprint,
                ids: wal_task_ids(&bytes),
                wal_len: bytes.len() as u64,
            })
        }
        "after" => {
            let ids = main_task_ids(db)?;
            Ok(ProbeSnapshot::After {
                fingerprint: fingerprint(db, &wal)?,
                ids,
            })
        }
        _ => Err(ReadError::Failed("unknown ownership probe phase".into())),
    }
}

/// Internal executable entry point. Dispatch before logging/configuration or
/// normal store startup; the only written inode is the private response file.
#[doc(hidden)]
pub fn run_internal_ownership_probe(phase: &str, db: &Path, output: &Path) -> Result<(), String> {
    if !matches!(phase, "before" | "after") {
        return Err("ownership probe requires before or after".into());
    }
    let result = probe_snapshot(phase, db);
    let file = std::fs::File::create(output).map_err(|error| error.to_string())?;
    serde_json::to_writer(file, &result).map_err(|error| error.to_string())
}

#[cfg(test)]
const PROBE_TEST: &str = "task_ownership::tests::ownership_probe_child";

fn isolated_probe(
    phase: &str,
    db: &Path,
    deadline: crate::bounded_process::Deadline,
) -> Result<ProbeSnapshot, ReadError> {
    let response = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    let mut command =
        std::process::Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
    #[cfg(not(test))]
    command
        .arg("--internal-task-ownership-probe")
        .arg(phase)
        .arg(db)
        .arg(response.path());
    #[cfg(test)]
    command
        .args([
            "--exact",
            PROBE_TEST,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("CAS_OWNERSHIP_PROBE_PHASE", phase)
        .env("CAS_OWNERSHIP_PROBE_DB", db)
        .env("CAS_OWNERSHIP_PROBE_OUTPUT", response.path());
    let output = crate::bounded_process::run_command(&mut command, deadline, PROBE_TIMEOUT)
        .map_err(|error| ReadError::Failed(format!("ownership probe failed: {error:?}")))?;
    if !output.status.success() {
        return Err(ReadError::Failed(format!(
            "ownership probe exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    #[cfg(test)]
    crate::test_child::assert_passed(&String::from_utf8_lossy(&output.stdout), PROBE_TEST);
    // A truncated, absent or oversized response is uncertainty, never an empty
    // set. The response inode is distinct from every inspected store inode.
    if response
        .as_file()
        .metadata()
        .map_err(|error| error.to_string())?
        .len()
        > 64 * 1024 * 1024
    {
        return Err(ReadError::Failed(
            "ownership probe response exceeds 64 MiB".into(),
        ));
    }
    let bytes = std::fs::read(response.path()).map_err(|error| error.to_string())?;
    serde_json::from_slice::<Result<ProbeSnapshot, ReadError>>(&bytes)
        .map_err(|error| ReadError::Failed(format!("ownership probe response invalid: {error}")))?
}

#[cfg(test)]
fn store_task_ids(db: &Path, between: &dyn Fn()) -> Result<HashSet<String>, ReadError> {
    store_task_ids_with_deadline(
        db,
        between,
        crate::bounded_process::Deadline::after(PROBE_TIMEOUT),
    )
}

/// WAL snapshot first, then immutable main database and a fresh fingerprint.
/// Two exec phases retain the existing parent-side race callback without
/// running any Rust or SQLite code in an unexeced fork of the caller.
fn store_task_ids_with_deadline(
    db: &Path,
    between: &dyn Fn(),
    deadline: crate::bounded_process::Deadline,
) -> Result<HashSet<String>, ReadError> {
    let ProbeSnapshot::Before {
        fingerprint: before,
        mut ids,
        wal_len,
    } = isolated_probe("before", db, deadline)?
    else {
        return Err(ReadError::Failed(
            "ownership probe returned the wrong phase".into(),
        ));
    };
    between();
    let ProbeSnapshot::After {
        fingerprint: after,
        ids: main_ids,
    } = isolated_probe("after", db, deadline)?
    else {
        return Err(ReadError::Failed(
            "ownership probe returned the wrong phase".into(),
        ));
    };
    if after != before || wal_len != before.wal_len {
        return Err(ReadError::Changed);
    }
    ids.extend(main_ids);
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

    #[test]
    #[ignore = "exec entry point for isolated ownership inspection"]
    fn ownership_probe_child() {
        let phase = std::env::var("CAS_OWNERSHIP_PROBE_PHASE").unwrap();
        let db = std::env::var_os("CAS_OWNERSHIP_PROBE_DB").unwrap();
        let output = std::env::var_os("CAS_OWNERSHIP_PROBE_OUTPUT").unwrap();
        run_internal_ownership_probe(&phase, Path::new(&db), Path::new(&output)).unwrap();
    }

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
        let projects = KnownProjects::from_specs_with(&this.path().join(".cas"), vec![repo_spec(root)], &restart);
        let error = projects.holders("cas-bbbb").unwrap_err();
        assert!(error.contains("changed while it was read"), "{error}");
    }

    /// cas-2191: a checkpoint during the read that leaves the database
    /// size, mtime and change counter as they were (WAL mode, coarse mtime)
    /// still moves the wal-index, so the read is not trusted.
    #[test]
    fn a_checkpoint_hidden_from_mtime_is_still_detected_cas_2191() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cas.db");
        let writer = rusqlite::Connection::open(&db).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT);
                 INSERT INTO tasks VALUES ('cas-aaaa', 'one');
                 PRAGMA wal_checkpoint(TRUNCATE);
                 UPDATE tasks SET title = 'two' WHERE id = 'cas-aaaa';",
            )
            .unwrap();
        let mtime = std::fs::metadata(&db).unwrap().modified().unwrap();
        let checkpoint = || {
            writer.execute_batch("PRAGMA wal_checkpoint(PASSIVE);").unwrap();
            std::fs::File::options().write(true).open(&db).unwrap().set_modified(mtime).unwrap();
        };
        let before = std::fs::metadata(&db).unwrap().len();
        assert!(matches!(store_task_ids(&db, &checkpoint), Err(ReadError::Changed)));
        assert_eq!(std::fs::metadata(&db).unwrap().len(), before, "the checkpoint kept the size");
        assert_eq!(std::fs::metadata(&db).unwrap().modified().unwrap(), mtime, "and the mtime");
    }

    /// Recovery raises nBackfillAttempted without a checkpoint running. Keep
    /// the recovered connection open, as the daemon keeps its global store.
    #[test]
    fn a_recovered_idle_store_is_proven_cas_593e() {
        let dir = tempfile::tempdir().unwrap();
        let root = store_with_wal(dir.path(), &["cas-aaaa"], &["cas-bbbb"]);
        let db = root.join(".cas/cas.db");
        let recovered = rusqlite::Connection::open(&db).unwrap();
        recovered
            .execute_batch("PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        assert_eq!(
            recovered
                .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let shm = std::fs::read(root.join(".cas/cas.db-shm")).unwrap();
        let word = |offset| u32::from_ne_bytes(shm[offset..offset + 4].try_into().unwrap());
        assert_eq!(word(96), 0, "recovery has not backfilled");
        assert!(
            word(128) > word(96),
            "recovery leaves attempted ahead of backfill"
        );
        let ids = store_task_ids(&db, &|| {})
            .unwrap_or_else(|_| panic!("recovered idle store must be proven"));
        assert!(ids.contains("cas-aaaa") && ids.contains("cas-bbbb"));
        drop(recovered);
    }

    #[test]
    fn ownership_inspection_preserves_local_sqlite_locks_cas_5a02() {
        let dir = tempfile::tempdir().unwrap();
        let this = tempfile::tempdir().unwrap();
        let root = store_with_wal(dir.path(), &["cas-aaaa"], &["cas-bbbb"]);
        let db = root.join(".cas/cas.db");
        let writer = rusqlite::Connection::open(&db).unwrap();
        writer
            .execute_batch(
                "PRAGMA wal_autocheckpoint=0; BEGIN IMMEDIATE; UPDATE tasks SET title='pending';",
            )
            .unwrap();
        let observe = || {
            let output = std::process::Command::new("python3")
                .args([
                    "-I",
                    "-S",
                    "-c",
                    r#"
import fcntl, json, sys
held = []
for path, offset in [(sys.argv[1], 1073741826), (sys.argv[1] + '-shm', 120)]:
    with open(path, 'r+b') as file:
        try:
            fcntl.lockf(file, fcntl.LOCK_EX | fcntl.LOCK_NB, 1, offset)
        except BlockingIOError:
            held.append(True)
        else:
            held.append(False)
print(json.dumps(held))
"#,
                ])
                .arg(&db)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            serde_json::from_slice::<Vec<bool>>(&output.stdout).unwrap()
        };
        assert_eq!(
            observe(),
            vec![true, true],
            "real SQLite shared and WAL write locks are held"
        );
        let before = (
            std::fs::metadata(&db).unwrap().len(),
            std::fs::metadata(root.join(".cas/cas.db-wal"))
                .unwrap()
                .len(),
        );
        let projects = KnownProjects::from_roots(&this.path().join(".cas"), vec![root.clone()]);
        assert_eq!(
            projects.holders("cas-aaaa").unwrap(),
            vec![root.display().to_string()]
        );
        assert_eq!(
            observe(),
            vec![true, true],
            "inspection must preserve the caller's SQLite locks"
        );
        assert_eq!(
            before,
            (
                std::fs::metadata(&db).unwrap().len(),
                std::fs::metadata(root.join(".cas/cas.db-wal"))
                    .unwrap()
                    .len(),
            )
        );
        writer.execute_batch("ROLLBACK").unwrap();
    }

    /// cas-69b5: a checkpoint already copying frames when the read starts
    /// (and still copying when it ends) leaves every fingerprint field as it
    /// was, so the store is unproven rather than trusted.
    #[test]
    fn an_in_flight_checkpoint_makes_the_store_unproven_cas_69b5() {
        for cross_process in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let this = tempfile::tempdir().unwrap();
            let root = store_with_wal(dir.path(), &["cas-aaaa"], &["cas-bbbb"]);
            let mut shm = vec![0u8; 136];
            shm[96..100].copy_from_slice(&1u32.to_ne_bytes());
            shm[128..132].copy_from_slice(&1u32.to_ne_bytes());
            let path = root.join(".cas/cas.db-shm");
            std::fs::write(&path, shm).unwrap();
            // Exercise both a local SQLite connection and a separate checkpointer.
            // The real probe child must see either owner as a conflicting lock.
            use std::os::fd::AsRawFd;
            let held = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut child = if cross_process {
                let mut child = std::process::Command::new("python3")
                    .args(["-I", "-S", "-c", r#"
import fcntl, sys
with open(sys.argv[1], 'r+b') as shm:
    fcntl.lockf(shm, fcntl.LOCK_EX, 1, 121)
    print('locked', flush=True)
    sys.stdin.buffer.read(1)
"#]).arg(&path)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped()).spawn().unwrap();
                let mut ready = String::new();
                std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stdout.take().unwrap()), &mut ready).unwrap();
                assert_eq!(ready.trim(), "locked");
                Some(child)
            } else {
                let mut lock: libc::flock = unsafe { std::mem::zeroed() };
                lock.l_type = libc::F_WRLCK as _;
                lock.l_whence = libc::SEEK_SET as _;
                lock.l_start = 121;
                lock.l_len = 1;
                assert_eq!(unsafe { libc::fcntl(held.as_raw_fd(), libc::F_SETLK, &lock) }, 0);
                None
            };
            let projects = KnownProjects::from_specs_with(
                &this.path().join(".cas"),
                vec![repo_spec(root)],
                &|| {},
            );
            let result = projects.holders("cas-aaaa");
            if let Some(child) = &mut child {
                drop(child.stdin.take());
                assert!(child.wait().unwrap().success());
            }
            assert!(result.unwrap_err().contains("changed while it was read"), "cross_process={cross_process}");
        }
    }

    /// cas-69b5: a global store that cannot be looked at (permission
    /// denied) is unreadable, never absent, so no id counts as not held.
    #[test]
    fn an_unreadable_global_store_is_unproven_not_absent_cas_69b5() {
        use std::os::unix::fs::PermissionsExt;
        // Root bypasses mode bits, so mode 000 cannot prove unreadability.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let this = tempfile::tempdir().unwrap();
        let global = home.path().join(".cas");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(global.join("cas.db"), b"").unwrap();
        std::fs::set_permissions(&global, std::fs::Permissions::from_mode(0o000)).unwrap();
        let spec = StoreSpec { label: "global store".into(), cas_dir: global.clone(), required: false };
        let projects = KnownProjects::from_specs_with(&this.path().join(".cas"), vec![spec], &|| {});
        let result = projects.holders("cas-abcd");
        std::fs::set_permissions(&global, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.unwrap_err().contains("global store"));
        let missing = StoreSpec { label: "global store".into(), cas_dir: home.path().join("none"), required: false };
        let projects = KnownProjects::from_specs_with(&this.path().join(".cas"), vec![missing], &|| {});
        assert!(projects.holders("cas-abcd").unwrap().is_empty(), "a missing global store holds nothing");
    }

    /// cas-2191: a store written once during the read is read again and
    /// proven; one that keeps changing stays unproven after the bounded
    /// retries.
    #[test]
    fn a_busy_store_is_retried_before_it_is_unproven_cas_2191() {
        let dir = tempfile::tempdir().unwrap();
        let this = tempfile::tempdir().unwrap();
        let root = store_with_wal(dir.path(), &["cas-aaaa"], &["cas-bbbb"]);
        let wal = root.join(".cas/cas.db-wal");
        let calls = std::cell::Cell::new(0);
        let restart = |times: u32| {
            calls.set(calls.get() + 1);
            if calls.get() <= times {
                let mut bytes = std::fs::read(&wal).unwrap();
                for byte in &mut bytes[16..24] {
                    *byte ^= 0xff;
                }
                std::fs::write(&wal, bytes).unwrap();
            }
        };
        let label = root.display().to_string();
        let once = KnownProjects::from_specs_with(&this.path().join(".cas"), vec![repo_spec(root.clone())], &|| restart(1));
        assert_eq!(once.holders("cas-bbbb").unwrap(), vec![label]);
        assert_eq!(calls.get(), 2, "read again once");
        calls.set(0);
        let always = KnownProjects::from_specs_with(&this.path().join(".cas"), vec![repo_spec(root)], &|| restart(u32::MAX));
        assert!(always.holders("cas-bbbb").unwrap_err().contains("3 times"));
        assert_eq!(calls.get(), STORE_READ_ATTEMPTS);
    }

    /// cas-2191: the host's global store (`~/.cas/cas.db`) holds
    /// global-scope tasks, so its ids are held too.
    #[test]
    fn the_global_store_is_a_holder_cas_2191() {
        let _home = crate::test_support::TestEnvGuard::temp_home();
        let this = tempfile::tempdir().unwrap();
        let global = crate::store::known_repos::host_cas_dir();
        let home = global.parent().unwrap();
        assert_eq!(crate::store::init_cas_dir(home).unwrap(), global);
        crate::store::known_repos::ensure_host_schema().unwrap();
        crate::store::open_task_store(&global)
            .unwrap()
            .add(&cas_types::Task::new("cas-abcd".into(), "a global-scope task".into()))
            .unwrap();
        let projects = KnownProjects::load(&this.path().join(".cas"));
        assert_eq!(projects.holders("cas-abcd").unwrap(), vec!["global store".to_string()]);
        assert!(projects.holders("cas-dcba").unwrap().is_empty());
    }
}
