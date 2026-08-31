//! Detection and one-time repair for the legacy daemon Tantivy root.
//!
//! # Concurrency contract (cas-25a9)
//!
//! Repair runs on two callers with very different tolerances: `cas doctor
//! --fix`, which may block a human for a while, and the MCP daemon's
//! maintenance `select!` arm, which must never block — anything slow there
//! stalls agent reaping, lease reclaim and worktree cleanup for every session
//! on the box.
//!
//! Three rules fall out of that, and each is pinned by a test:
//!
//! 1. **Never block on a lock.** Tantivy's [`META_LOCK`] declares
//!    `is_blocking: true`, so `acquire_lock(&META_LOCK)` is an *unbounded*
//!    `flock`. A pre-fix daemon still writing to the legacy root would hang
//!    `doctor --fix` and wedge the whole daemon loop. This module acquires a
//!    non-blocking variant with a short deadline and reports
//!    [`LegacyRepairOutcome::Busy`] instead.
//! 2. **Snapshot under the lock.** Ids are read *after* the writer lock is
//!    held, so a concurrent old daemon cannot write an entry into the window
//!    between snapshot and retirement and have it deleted un-requeued.
//! 3. **Retire so that a partial failure is recoverable.** Ids are durably
//!    re-queued first, then `meta.json` goes *first* — after that the root can
//!    no longer strand anything — and `.managed.json` goes *last*, so a run
//!    that dies mid-delete leaves a resumable sweep rather than an
//!    un-inspectable root.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use tantivy::collector::DocSetCollector;
use tantivy::directory::error::LockError;
use tantivy::directory::{Directory, DirectoryLock, INDEX_WRITER_LOCK, Lock, META_LOCK};
use tantivy::query::AllQuery;
use tantivy::schema::Value;

use crate::error::{CasError, Result};
use crate::hybrid_search::{BackgroundIndexer, IndexingConfig};
use crate::store::Store;

/// How long to keep retrying a busy legacy root before reporting it busy.
///
/// Deliberately short: the daemon calls this on its maintenance arm, and a
/// skipped cycle costs nothing (the next one retries) whereas a slow cycle
/// costs every other maintenance job behind it.
const LOCK_DEADLINE: Duration = Duration::from_secs(3);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// Read-only description of an index accidentally written directly below
/// `<cas_root>/index` by BackgroundIndexer before cas-bc42.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyIndexState {
    pub documents: usize,
    pub entry_ids: Vec<String>,
}

/// Outcome of migrating a legacy root into the canonical Tantivy index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyRepairResult {
    pub legacy_documents: usize,
    pub requeued_entries: usize,
    pub indexed_entries: usize,
    /// Per-entry indexing failures. Carried rather than raised: the entries are
    /// already durably re-queued, so a later cycle retries them, and turning
    /// this into an `Err` is what let one bad entry disable all background
    /// indexing (cas-25a9 P1-B).
    pub errors: Vec<(String, String)>,
    /// Files the retirement sweep could not remove this run. Non-fatal: ids are
    /// re-queued before any deletion, so the residue is stray bytes, and
    /// `.managed.json` is retained so the next run resumes the sweep.
    pub unswept_files: Vec<String>,
}

/// What one repair attempt did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyRepairOutcome {
    /// Nothing to do — no stray Tantivy root below `<cas_root>/index`.
    NoLegacyRoot,
    /// Another process holds the legacy root. The caller should retry later:
    /// doctor renders this as a Warning, the daemon skips the cycle.
    Busy { reason: String },
    Repaired(LegacyRepairResult),
}

/// Per-call bounds on how much work one repair may do.
///
/// The daemon passes its configured budget so a huge legacy root cannot hold
/// the maintenance arm for a full re-index; `doctor --fix` passes
/// [`LegacyRepairLimits::unbounded`] because a human asked for it and is
/// waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyRepairLimits {
    pub batch_size: usize,
    pub max_per_run: usize,
}

impl Default for LegacyRepairLimits {
    fn default() -> Self {
        Self {
            batch_size: 256,
            max_per_run: 2_000,
        }
    }
}

impl LegacyRepairLimits {
    /// Drain everything in one call. Only for interactive `doctor --fix`.
    pub fn unbounded() -> Self {
        Self {
            batch_size: 256,
            max_per_run: usize::MAX,
        }
    }
}

/// A non-blocking twin of [`META_LOCK`].
///
/// `META_LOCK` is declared `is_blocking: true`, which makes
/// `MmapDirectory::acquire_lock` call `File::lock_exclusive()` and wait
/// forever. Same lock file, same mutual exclusion, but it returns
/// `LockError::LockBusy` immediately instead of hanging the caller.
fn non_blocking_meta_lock() -> Lock {
    Lock {
        filepath: META_LOCK.filepath.clone(),
        is_blocking: false,
    }
}

/// Try to take `lock` until `LOCK_DEADLINE` elapses.
///
/// `Ok(None)` means "held by someone else" — a normal, retryable outcome, not
/// an error.
fn acquire_bounded(directory: &dyn Directory, lock: &Lock) -> Result<Option<DirectoryLock>> {
    let deadline = Instant::now() + LOCK_DEADLINE;
    loop {
        match directory.acquire_lock(lock) {
            Ok(guard) => return Ok(Some(guard)),
            Err(LockError::LockBusy) => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(error) => {
                return Err(CasError::Other(format!(
                    "legacy index lock {}: {error}",
                    lock.filepath.display()
                )));
            }
        }
    }
}

/// Inspect the legacy root without creating an index when none exists.
pub fn inspect_legacy_index(cas_dir: &Path) -> Result<Option<LegacyIndexState>> {
    let legacy_dir = cas_dir.join("index");
    if !legacy_dir.join("meta.json").is_file() {
        return Ok(None);
    }

    let index = tantivy::Index::open_in_dir(&legacy_dir)?;
    read_index_state(&index).map(Some)
}

/// Read ids out of an already-open legacy index.
///
/// Split out of [`inspect_legacy_index`] so the repair path can take this
/// snapshot *while holding the writer lock* (cas-25a9 P2-A); the public
/// inspect entry point stays lock-free because doctor's read-only check must
/// never contend with a running daemon.
fn read_index_state(index: &tantivy::Index) -> Result<LegacyIndexState> {
    let schema = index.schema();
    let id_field = schema
        .get_field("id")
        .map_err(|_| CasError::Other("legacy Tantivy index has no `id` field".to_string()))?;
    let doc_type_field = schema
        .get_field("doc_type")
        .map_err(|_| CasError::Other("legacy Tantivy index has no `doc_type` field".to_string()))?;
    let reader = index.reader()?;
    let searcher = reader.searcher();
    let addresses = searcher.search(&AllQuery, &DocSetCollector)?;
    let documents = addresses.len();
    let mut entry_ids = Vec::new();
    for address in addresses {
        let document: tantivy::TantivyDocument = searcher.doc(address)?;
        let doc_type = document
            .get_first(doc_type_field)
            .and_then(|value| value.as_str());
        if doc_type != Some("entry") {
            continue;
        }
        if let Some(id) = document
            .get_first(id_field)
            .and_then(|value| value.as_str())
        {
            entry_ids.push(id.to_string());
        }
    }
    entry_ids.sort();
    entry_ids.dedup();

    Ok(LegacyIndexState {
        documents,
        entry_ids,
    })
}

/// Re-queue every legacy entry, retire only Tantivy-managed root files, and
/// drain the pending queue into the canonical index.
///
/// Never blocks on a held lock; see the module docs for the ordering contract.
pub fn repair_legacy_index(
    cas_dir: &Path,
    store: &dyn Store,
    limits: LegacyRepairLimits,
) -> Result<LegacyRepairOutcome> {
    let legacy_dir = cas_dir.join("index");
    let has_meta = legacy_dir.join("meta.json").is_file();
    let has_managed = legacy_dir.join(".managed.json").is_file();

    if !has_meta {
        // A previous run got past `meta.json` but died before finishing the
        // sweep. Nothing can be stranded (the ids were re-queued before any
        // deletion), so this is pure residue — finish it and report nothing.
        if has_managed {
            let unswept = sweep_managed_files(&legacy_dir)?;
            if !unswept.is_empty() {
                return Ok(LegacyRepairOutcome::Repaired(LegacyRepairResult {
                    unswept_files: unswept,
                    ..Default::default()
                }));
            }
        }
        return Ok(LegacyRepairOutcome::NoLegacyRoot);
    }

    let index = tantivy::Index::open_in_dir(&legacy_dir)?;

    // Coordinate with any still-running pre-fix daemon BEFORE reading ids:
    // deleting a live writer's segments would corrupt the repair source, and
    // snapshotting before the lock would lose anything written in between.
    // Probe the meta lock before opening any reader.
    //
    // `Index::reader()` acquires META_LOCK internally, and META_LOCK is
    // declared blocking — so if a pre-fix daemon holds it, the *read* below
    // would hang exactly the way the old `acquire_lock(&META_LOCK)` did. The
    // probe takes the non-blocking variant and immediately drops it, purely to
    // answer "is anyone holding this?" within a bounded time.
    //
    // Residual race, stated rather than papered over: another process can take
    // the meta lock between this probe and the reader below, in which case the
    // read still blocks. The window is microseconds against a lock held for the
    // length of a daemon write, so this converts a reliable hang into a rare
    // one; closing it fully needs a tantivy API that accepts a timeout on the
    // reader's own acquisition.
    match acquire_bounded(index.directory(), &non_blocking_meta_lock())? {
        Some(probe) => drop(probe),
        None => {
            return Ok(LegacyRepairOutcome::Busy {
                reason: "legacy index metadata lock is held by another process".to_string(),
            });
        }
    }

    // The WRITER lock is the one that makes the snapshot safe: it excludes the
    // concurrent old daemon that could otherwise add an entry between the
    // snapshot and the retirement and have it deleted un-requeued.
    let Some(_writer_lock) = acquire_bounded(index.directory(), &INDEX_WRITER_LOCK)? else {
        return Ok(LegacyRepairOutcome::Busy {
            reason: "legacy index writer lock is held by another process".to_string(),
        });
    };

    // Snapshot under the writer lock.
    //
    // Deliberately NOT holding META_LOCK here: `Index::reader()` acquires the
    // meta lock itself (that is what it is for — keeping segments from being
    // GC'd while a reader opens them), and META_LOCK is declared blocking, so
    // holding it across this call deadlocks the process against itself.
    let state = read_index_state(&index)?;
    let ids: Vec<&str> = state.entry_ids.iter().map(String::as_str).collect();

    // Durable re-queue happens before ANY deletion, so every later step is
    // safe to fail: the worst residue is stray bytes, never a lost id.
    store.mark_index_pending_batch(&ids)?;

    // Now — with no reader of ours open — take the meta lock for the deletion
    // phase, so a reader in another process cannot be mid-open on the segments
    // being removed. Busy here is still safe: the ids are already queued, so
    // the next run simply retries the retirement.
    let Some(_meta_lock) = acquire_bounded(index.directory(), &non_blocking_meta_lock())? else {
        return Ok(LegacyRepairOutcome::Busy {
            reason: "legacy index metadata lock is held by another process".to_string(),
        });
    };

    let unswept = retire_locked(&legacy_dir)?;

    drop(_meta_lock);
    drop(_writer_lock);
    // Only now that both guards are released can the lock files themselves go.
    remove_lock_files(&legacy_dir);

    let indexer = BackgroundIndexer::open(cas_dir)?;
    let indexed = indexer.process_pending(
        store,
        &IndexingConfig {
            batch_size: limits.batch_size,
            max_per_run: limits.max_per_run,
        },
    )?;

    Ok(LegacyRepairOutcome::Repaired(LegacyRepairResult {
        legacy_documents: state.documents,
        requeued_entries: state.entry_ids.len(),
        indexed_entries: indexed.indexed,
        errors: indexed.errors,
        unswept_files: unswept,
    }))
}

/// Delete the legacy root's files in an order that makes any partial failure
/// recoverable. Returns the files that could not be removed.
///
/// `meta.json` goes first: once it is gone the root can no longer be opened as
/// an index, so it can never strand ids again. `.managed.json` goes last, so a
/// run that dies mid-sweep leaves behind exactly the list the next run needs.
fn retire_locked(legacy_dir: &Path) -> Result<Vec<String>> {
    let managed_path = legacy_dir.join(".managed.json");
    let managed: Vec<String> = serde_json::from_slice(&std::fs::read(&managed_path)?)?;
    for filename in &managed {
        validate_managed_filename(filename)?;
    }

    remove_file_if_present(&legacy_dir.join("meta.json"))?;

    let mut unswept = Vec::new();
    for filename in &managed {
        if remove_file_if_present(&legacy_dir.join(filename)).is_err() {
            unswept.push(filename.clone());
        }
    }

    if unswept.is_empty() {
        remove_file_if_present(&managed_path)?;
    }
    Ok(unswept)
}

/// Resume an interrupted sweep: `meta.json` is already gone, `.managed.json`
/// still lists what is left.
fn sweep_managed_files(legacy_dir: &Path) -> Result<Vec<String>> {
    let managed_path = legacy_dir.join(".managed.json");
    let managed: Vec<String> = serde_json::from_slice(&std::fs::read(&managed_path)?)?;
    for filename in &managed {
        validate_managed_filename(filename)?;
    }

    let mut unswept = Vec::new();
    for filename in &managed {
        if remove_file_if_present(&legacy_dir.join(filename)).is_err() {
            unswept.push(filename.clone());
        }
    }
    if unswept.is_empty() {
        remove_file_if_present(&managed_path)?;
        remove_lock_files(legacy_dir);
    }
    Ok(unswept)
}

/// Reject anything that is not a bare filename inside the legacy root.
fn validate_managed_filename(filename: &str) -> Result<()> {
    let path = Path::new(filename);
    if path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err(CasError::Other(format!(
            "refusing unsafe legacy Tantivy managed path `{filename}`"
        )));
    }
    Ok(())
}

/// Best-effort removal of the lock files retirement itself created, so a
/// retired root does not linger looking half-alive. Never fatal.
fn remove_lock_files(legacy_dir: &Path) {
    for lock in [&INDEX_WRITER_LOCK.filepath, &META_LOCK.filepath] {
        let path: PathBuf = legacy_dir.join(lock);
        let _ = std::fs::remove_file(path);
    }
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hybrid_search::{
        DocType, HybridSearch, HybridSearchOptions, SearchIndex, SearchOptions,
    };
    use crate::store::open_store;
    use crate::types::Entry;
    use std::sync::mpsc;

    /// Build a `.cas` root holding one entry that exists ONLY in the legacy
    /// Tantivy index — the exact state cas-bc42 repairs.
    fn legacy_root_with_one_stranded_entry(temp: &Path) -> (PathBuf, Entry) {
        let cas_root = temp.join(".cas");
        std::fs::create_dir_all(&cas_root).expect("create .cas");
        let store = open_store(&cas_root).expect("store");
        let entry = Entry::new(
            "legacy-daemon-only".to_string(),
            "legacyquasar background repair target".to_string(),
        );
        store.add(&entry).expect("add entry");
        {
            let legacy = SearchIndex::open(&cas_root.join("index")).expect("legacy index");
            legacy.index_entry(&entry).expect("index legacy entry");
        }
        store
            .mark_indexed(&entry.id)
            .expect("mark incorrectly indexed");
        (cas_root, entry)
    }

    // ----------------------------------------------------------------------
    // P1-A: a held lock must bound, never hang.
    // ----------------------------------------------------------------------

    #[test]
    fn a_held_meta_lock_returns_busy_instead_of_hanging() {
        // Regression for the review's load-bearing finding: `META_LOCK` is
        // declared `is_blocking: true`, so the pre-fix code called an unbounded
        // `flock` here. With another holder this test hung forever, and in
        // production it wedged the daemon's whole maintenance loop.
        //
        // The repair runs on a worker thread and the assertion is a
        // `recv_timeout`, so a regression fails this test in ~30s instead of
        // hanging the suite.
        let temp = tempfile::tempdir().expect("tempdir");
        let (cas_root, _entry) = legacy_root_with_one_stranded_entry(temp.path());

        // Hold the meta lock the way a pre-fix daemon would.
        let holder = tantivy::Index::open_in_dir(cas_root.join("index")).expect("open legacy");
        let _held = holder
            .directory()
            .acquire_lock(&META_LOCK)
            .expect("hold meta lock");

        let (tx, rx) = mpsc::channel();
        let probe_root = cas_root.clone();
        std::thread::spawn(move || {
            let store = open_store(&probe_root).expect("store");
            let outcome = repair_legacy_index(
                &probe_root,
                store.as_ref(),
                LegacyRepairLimits::unbounded(),
            );
            let _ = tx.send(outcome);
        });

        let outcome = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("repair must return while the meta lock is held, not hang")
            .expect("repair call itself must not error");
        assert!(
            matches!(outcome, LegacyRepairOutcome::Busy { .. }),
            "expected Busy while the meta lock is held, got {outcome:?}"
        );
    }

    #[test]
    fn a_busy_legacy_root_requeues_nothing() {
        // P2-A as an observable ordering contract. The pre-fix code snapshotted
        // and re-queued ids BEFORE taking the lock, so a contended run left the
        // queue mutated; post-fix nothing is touched until the lock is held.
        // The same ordering is what stops a concurrent old daemon's writes from
        // being deleted un-requeued.
        let temp = tempfile::tempdir().expect("tempdir");
        let (cas_root, _entry) = legacy_root_with_one_stranded_entry(temp.path());
        let store = open_store(&cas_root).expect("store");
        assert!(
            store.list_pending_index(10).expect("pending").is_empty(),
            "precondition: the stranded entry is marked indexed, so nothing is pending"
        );

        let holder = tantivy::Index::open_in_dir(cas_root.join("index")).expect("open legacy");
        let _held = holder
            .directory()
            .acquire_lock(&INDEX_WRITER_LOCK)
            .expect("hold writer lock");

        let (tx, rx) = mpsc::channel();
        let probe_root = cas_root.clone();
        std::thread::spawn(move || {
            let store = open_store(&probe_root).expect("store");
            let _ = tx.send(repair_legacy_index(
                &probe_root,
                store.as_ref(),
                LegacyRepairLimits::unbounded(),
            ));
        });
        let outcome = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("must not hang")
            .expect("must not error");
        assert!(matches!(outcome, LegacyRepairOutcome::Busy { .. }), "{outcome:?}");

        assert!(
            store.list_pending_index(10).expect("pending").is_empty(),
            "a contended repair must not mutate the pending queue before it owns the lock"
        );
        assert!(
            inspect_legacy_index(&cas_root).expect("inspect").is_some(),
            "and it must leave the legacy root intact for the next attempt"
        );
    }

    // ----------------------------------------------------------------------
    // P2-B: partial retirement is recoverable.
    // ----------------------------------------------------------------------

    #[test]
    fn a_partial_retirement_is_recoverable_on_the_next_run() {
        // Inject a removal failure by replacing one managed file with a
        // DIRECTORY of the same name: `remove_file` then fails with EISDIR.
        // Pre-fix this aborted mid-loop with `meta.json` still present but
        // segments gone — an un-inspectable root that poisoned every later
        // repair. Post-fix the ids are already re-queued, `meta.json` goes
        // first, and `.managed.json` survives so the next run resumes.
        let temp = tempfile::tempdir().expect("tempdir");
        let (cas_root, entry) = legacy_root_with_one_stranded_entry(temp.path());
        let legacy_dir = cas_root.join("index");

        // Inject the failure into the SWEEP only, not into the index: add a
        // name to `.managed.json` that exists as a DIRECTORY, so `remove_file`
        // fails with EISDIR while every real segment still reads and deletes
        // normally. (Clobbering a real segment instead breaks the snapshot read
        // before retirement is ever reached, which tests nothing about
        // retirement.)
        let victim = "blocked-by-directory".to_string();
        let managed_path = legacy_dir.join(".managed.json");
        let mut managed: Vec<String> =
            serde_json::from_slice(&std::fs::read(&managed_path).unwrap()).unwrap();
        managed.push(victim.clone());
        std::fs::write(&managed_path, serde_json::to_vec(&managed).unwrap())
            .expect("rewrite managed list");
        std::fs::create_dir(legacy_dir.join(&victim)).expect("block removal with a directory");

        let store = open_store(&cas_root).expect("store");
        let first = match repair_legacy_index(
            &cas_root,
            store.as_ref(),
            LegacyRepairLimits::unbounded(),
        )
        .expect("first repair must not error")
        {
            LegacyRepairOutcome::Repaired(repair) => repair,
            other => panic!("expected Repaired, got {other:?}"),
        };
        assert!(
            !first.unswept_files.is_empty(),
            "the blocked file must be reported, not silently dropped"
        );
        assert_eq!(first.requeued_entries, 1, "ids are re-queued before any delete");
        assert!(
            legacy_dir.join(".managed.json").is_file(),
            "the sweep list must survive so the next run can resume"
        );
        assert!(
            !legacy_dir.join("meta.json").exists(),
            "meta.json goes first: the root can no longer strand ids"
        );

        // The obstruction clears (an operator removes it, or the transient
        // condition passes) and the next run finishes the job.
        std::fs::remove_dir(legacy_dir.join(&victim)).expect("clear the obstruction");
        let second = repair_legacy_index(
            &cas_root,
            store.as_ref(),
            LegacyRepairLimits::unbounded(),
        )
        .expect("second repair");
        match second {
            LegacyRepairOutcome::NoLegacyRoot => {}
            LegacyRepairOutcome::Repaired(repair) => {
                assert!(repair.unswept_files.is_empty(), "{repair:?}")
            }
            other => panic!("expected the sweep to finish, got {other:?}"),
        }
        assert!(
            !legacy_dir.join(".managed.json").exists(),
            "a completed sweep removes its own list"
        );
        assert!(
            !legacy_dir.join(&victim).exists(),
            "the previously blocked file is gone"
        );

        // The entry survived the whole ordeal: re-queued on run 1, indexed.
        assert!(
            store
                .list_pending_index(10)
                .expect("pending")
                .iter()
                .all(|pending| pending.id != entry.id),
            "the stranded entry must have been indexed, not left pending"
        );
    }

    // ----------------------------------------------------------------------
    // P2-C: per-run work is bounded by the caller's limits.
    // ----------------------------------------------------------------------

    #[test]
    fn repair_respects_the_callers_per_run_bound() {
        // The daemon's maintenance arm shares a thread with agent reaping and
        // lease reclaim, so repair must not drain an arbitrarily large root in
        // one cycle. Pre-fix this hardcoded `max_per_run: usize::MAX`.
        let temp = tempfile::tempdir().expect("tempdir");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("create .cas");
        let store = open_store(&cas_root).expect("store");

        let mut entries = Vec::new();
        for index in 0..12 {
            let entry = Entry::new(
                format!("legacy-bulk-{index:03}"),
                format!("legacybulk repair target number {index}"),
            );
            store.add(&entry).expect("add entry");
            entries.push(entry);
        }
        {
            let legacy = SearchIndex::open(&cas_root.join("index")).expect("legacy index");
            for entry in &entries {
                legacy.index_entry(entry).expect("index legacy entry");
            }
        }
        for entry in &entries {
            store.mark_indexed(&entry.id).expect("mark indexed");
        }

        let repair = match repair_legacy_index(
            &cas_root,
            store.as_ref(),
            LegacyRepairLimits {
                batch_size: 2,
                max_per_run: 5,
            },
        )
        .expect("repair")
        {
            LegacyRepairOutcome::Repaired(repair) => repair,
            other => panic!("expected Repaired, got {other:?}"),
        };

        assert_eq!(repair.requeued_entries, 12, "all ids are re-queued up front");
        assert!(
            repair.indexed_entries <= 5,
            "one cycle must not exceed max_per_run; indexed {}",
            repair.indexed_entries
        );
        assert!(
            !store.list_pending_index(20).expect("pending").is_empty(),
            "the remainder stays queued for the next cycle rather than being lost"
        );
    }

    #[test]
    fn repair_requeues_legacy_entries_into_the_canonical_index_and_preserves_siblings() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("create .cas");
        let store = open_store(&cas_root).expect("store");
        let entry = Entry::new(
            "legacy-daemon-only".to_string(),
            "legacyquasar background repair target".to_string(),
        );
        store.add(&entry).expect("add entry");

        {
            let legacy = SearchIndex::open(&cas_root.join("index")).expect("legacy index");
            legacy.index_entry(&entry).expect("index legacy entry");
        }
        store
            .mark_indexed(&entry.id)
            .expect("mark incorrectly indexed");
        std::fs::create_dir_all(cas_root.join("index/code")).expect("code dir");
        std::fs::write(cas_root.join("index/code/keep"), b"code-index-sibling")
            .expect("code marker");

        let state = inspect_legacy_index(&cas_root)
            .expect("inspect")
            .expect("legacy state");
        assert_eq!(state.documents, 1);
        assert_eq!(state.entry_ids, vec![entry.id.clone()]);

        let repair = match repair_legacy_index(
            &cas_root,
            store.as_ref(),
            LegacyRepairLimits::unbounded(),
        )
        .expect("repair")
        {
            LegacyRepairOutcome::Repaired(repair) => repair,
            other => panic!("expected a repair, got {other:?}"),
        };
        assert_eq!(repair.legacy_documents, 1);
        assert_eq!(repair.requeued_entries, 1);
        assert_eq!(repair.indexed_entries, 1);
        assert!(
            inspect_legacy_index(&cas_root)
                .expect("clean inspect")
                .is_none()
        );
        assert_eq!(
            std::fs::read(cas_root.join("index/code/keep")).expect("preserved marker"),
            b"code-index-sibling"
        );
        assert!(store.list_pending_index(10).expect("pending").is_empty());

        let search = HybridSearch::open(&cas_root).expect("canonical reader");
        let results = search
            .search(
                &HybridSearchOptions {
                    base: SearchOptions {
                        query: "legacyquasar".to_string(),
                        doc_types: vec![DocType::Entry],
                        ..Default::default()
                    },
                    enable_temporal: false,
                    enable_graph: false,
                    ..Default::default()
                },
                &store.list().expect("entries"),
            )
            .expect("search canonical index");
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some(entry.id.as_str())
        );
    }
}
