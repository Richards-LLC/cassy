//! Process-level shared SQLite connection pool.
//!
//! All SQLite stores in a process share ONE connection per database file,
//! dramatically reducing connection count and eliminating intra-process
//! write lock contention when many store types access the same `cas.db`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::Connection;

#[cfg(test)]
use crate::SQLITE_BUSY_TIMEOUT;
use crate::{Result, StoreError};

/// Acquire a shared SQLite connection, converting a poisoned mutex into a
/// recoverable store error instead of panicking the caller.
///
/// Every store in this crate takes its connection through this (or
/// [`lock_connection_recovering`]); a guard test refuses a raw `conn.lock()`
/// in non-test code (GH #1165 / cas-06ef).
///
/// Under a thread wait budget ([`crate::wait_budget`]) the in-process mutex is
/// polled only until the deadline: another thread of this process holding the
/// connection across its own busy wait must not stall a UI thread (GH #1165).
/// Without a budget this is a plain blocking lock.
pub(crate) fn lock_connection(conn: &Mutex<Connection>) -> Result<MutexGuard<'_, Connection>> {
    lock_connection_with(conn, false)
}

/// [`lock_connection`] for stores that recover a poisoned mutex rather than
/// fail on it. Fails only when the thread's wait budget runs out.
pub(crate) fn lock_connection_recovering(
    conn: &Mutex<Connection>,
) -> Result<MutexGuard<'_, Connection>> {
    lock_connection_with(conn, true)
}

/// [`lock_connection`] for the pooled write path's bare mutex.
pub(crate) fn lock_connection_mutex(conn: &Mutex<Connection>) -> Result<MutexGuard<'_, Connection>> {
    lock_connection(conn)
}

fn lock_connection_with(
    conn: &Mutex<Connection>,
    recover_poison: bool,
) -> Result<MutexGuard<'_, Connection>> {
    let poisoned = || StoreError::Other("shared SQLite connection lock poisoned".to_string());
    crate::wait_budget::note_store_access("sqlite connection");
    if crate::wait_budget::wait_deadline().is_none() {
        return match conn.lock() {
            Ok(guard) => Ok(guard),
            Err(error) if recover_poison => Ok(error.into_inner()),
            Err(_) => Err(poisoned()),
        };
    }
    let mut pause = Duration::from_micros(200);
    loop {
        match conn.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(error)) if recover_poison => {
                return Ok(error.into_inner());
            }
            Err(std::sync::TryLockError::Poisoned(_)) => return Err(poisoned()),
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        let Some(sleep) = crate::wait_budget::clamp_wait(pause) else {
            return Err(crate::wait_budget::budget_exhausted_store_error(
                "shared SQLite connection held by another thread",
            ));
        };
        std::thread::sleep(sleep);
        pause = (pause * 2).min(Duration::from_millis(5));
    }
}

/// Process-global pool of shared SQLite connections, keyed by canonical DB path.
///
/// Each database has its own [`PoolSlot`]. `POOL` itself is held only to find
/// or insert a slot and never across SQLite I/O: before cas-e335 it was held
/// while `Connection::open` ran, so one open stalled inside SQLite blocked
/// every opener of every database in the process.
static POOL: Mutex<Option<Pool>> = Mutex::new(None);

/// How long a connection no caller holds stays open before the pool closes it.
pub const IDLE_CONNECTION_TTL: Duration = Duration::from_secs(60);

/// Most connections no caller holds that the pool keeps open at once. Beyond
/// it the least recently used close first, so a process that touches many
/// short-lived databases keeps a bounded number of file descriptors.
pub const MAX_IDLE_CONNECTIONS: usize = 16;

/// Minimum spacing between the idle sweeps `shared_connection` runs.
const IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Pool {
    slots: HashMap<PathBuf, Arc<PoolSlot>>,
    last_sweep: Option<Instant>,
}

/// One database's entry in [`POOL`].
///
/// The slot owns the connection's lifetime: it keeps a strong reference, so a
/// caller dropping the last store never closes the database, and it closes a
/// connection only under this slot's lock. Opens take the same lock, so a
/// close and an open of one database can never overlap. They did before
/// cas-e335, when the pool kept only a `Weak` and the close ran wherever the
/// last `Arc` dropped: `sqlite3WalClose` racing `sqlite3BtreeOpen` on the same
/// file deadlocked inside SQLite and wedged the macOS hub.
#[derive(Default)]
struct PoolSlot {
    state: Mutex<SlotState>,
}

#[derive(Default)]
struct SlotState {
    connection: Option<PooledConnection>,
}

struct PooledConnection {
    shared: Arc<Mutex<Connection>>,
    /// Identity of the database file when it was opened. An idle connection
    /// whose file was deleted or replaced is closed and reopened rather than
    /// handed out still pointing at the old file.
    identity: Option<FileIdentity>,
    last_used: Instant,
}

impl PooledConnection {
    /// No caller holds the connection; only the pool does.
    fn idle(&self) -> bool {
        Arc::strong_count(&self.shared) == 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

fn file_identity(path: &Path) -> Option<FileIdentity> {
    let metadata = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Some(FileIdentity {
            device: 0,
            inode: 0,
        })
    }
}

fn lock_pool() -> MutexGuard<'static, Option<Pool>> {
    POOL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_slot(slot: &PoolSlot) -> MutexGuard<'_, SlotState> {
    slot.state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Close idle pooled connections: every one unused for `ttl`, then the least
/// recently used beyond `cap`. Only databases for which `include` is true are
/// considered. Each close runs under that database's slot lock; a slot busy
/// right now (an open in progress) is skipped until the next sweep.
fn sweep_idle_connections(
    now: Instant,
    ttl: Duration,
    cap: usize,
    include: impl Fn(&Path) -> bool,
) {
    let slots: Vec<Arc<PoolSlot>> = lock_pool()
        .as_ref()
        .map(|pool| {
            pool.slots
                .iter()
                .filter(|(path, _)| include(path))
                .map(|(_, slot)| Arc::clone(slot))
                .collect()
        })
        .unwrap_or_default();

    let mut idle: Vec<(Instant, Arc<PoolSlot>)> = slots
        .into_iter()
        .filter_map(|slot| {
            let last_used = {
                let state = slot.state.try_lock().ok()?;
                let pooled = state.connection.as_ref()?;
                pooled.idle().then_some(pooled.last_used)?
            };
            Some((last_used, slot))
        })
        .collect();
    idle.sort_by_key(|(last_used, _)| *last_used);
    let over_cap = idle.len().saturating_sub(cap);

    for (index, (seen_last_used, slot)) in idle.into_iter().enumerate() {
        let expired = now.saturating_duration_since(seen_last_used) >= ttl;
        if !expired && index >= over_cap {
            continue;
        }
        let Ok(mut state) = slot.state.try_lock() else {
            continue;
        };
        let still_idle = state
            .connection
            .as_ref()
            .is_some_and(|pooled| pooled.idle() && pooled.last_used == seen_last_used);
        if still_idle {
            // Close while this database's slot lock is held.
            drop(state.connection.take());
        }
    }

    prune_empty_slots();
}

/// Forget slots with no connection that no caller is using.
fn prune_empty_slots() {
    if let Some(pool) = lock_pool().as_mut() {
        pool.slots.retain(|_, slot| {
            Arc::strong_count(slot) > 1
                || slot
                    .state
                    .try_lock()
                    .map(|state| state.connection.is_some())
                    .unwrap_or(true)
        });
    }
}

/// Close every pooled connection no caller holds.
///
/// A process calls this on its way out: the pool lives in a static that is
/// never dropped, so without it a CLI run would exit with its idle
/// connections un-closed and its WAL un-checkpointed, where before the pool
/// owned closes the last store drop checkpointed it.
pub fn close_idle_connections() {
    sweep_idle_connections(Instant::now(), Duration::ZERO, 0, |_| true);
}

/// Close the pooled connection for `db_path` now, if no caller holds it.
///
/// Call this before touching the database file directly (copying a backup
/// over it, for example): the pool otherwise keeps an idle connection, and
/// its WAL, open for up to [`IDLE_CONNECTION_TTL`]. Returns true when a
/// connection was closed.
pub fn close_idle_connection(db_path: &Path) -> bool {
    let canonical = canonical_db_path(db_path);
    let Some(slot) = lock_pool()
        .as_ref()
        .and_then(|pool| pool.slots.get(&canonical).cloned())
    else {
        return false;
    };
    let closed = {
        let mut state = lock_slot(&slot);
        if state
            .connection
            .as_ref()
            .is_some_and(PooledConnection::idle)
        {
            drop(state.connection.take());
            true
        } else {
            false
        }
    };
    drop(slot);
    prune_empty_slots();
    closed
}

/// Environment variable naming databases a test run must never open.
///
/// Colon-separated list of absolute paths. Each entry is either a `cas.db`
/// file or a `.cas` directory (in which case `<dir>/cas.db` is protected).
/// Set by the test harness — see `scripts/check-real-store-untouched.sh` —
/// and honoured by every production store open, because [`shared_connection`]
/// is the single choke point they all funnel through.
///
/// This exists because the integration suite silently wrote 994 fixture
/// memories into the developer's real `~/.cas/cas.db` and the cas-src project
/// database over several months (cas-78c8 / GH #156). A test that escapes its
/// sandbox now aborts loudly at the moment of the escape instead of quietly
/// corrupting a real corpus.
pub const PROTECTED_DBS_ENV: &str = "CAS_TEST_PROTECTED_DBS";

/// Normalize a database path the same way the pool keys it: canonicalize the
/// parent (which always exists) and rejoin the file name, because the file
/// itself may not exist yet and macOS symlinks (`/var` → `/private/var`)
/// otherwise produce key mismatches.
fn canonical_db_path(db_path: &Path) -> PathBuf {
    match db_path.parent().and_then(|p| p.canonicalize().ok()) {
        Some(parent) => parent.join(db_path.file_name().unwrap_or_default()),
        None => db_path.to_path_buf(),
    }
}

/// Expand one `CAS_TEST_PROTECTED_DBS` entry to the database file it protects.
///
/// A `.cas` directory protects `<dir>/cas.db`; anything else is taken as the
/// database path itself.
fn protected_entry_to_db(entry: &str) -> Option<PathBuf> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    let path = PathBuf::from(entry);
    let db = if path.is_dir() {
        path.join("cas.db")
    } else {
        path
    };
    Some(canonical_db_path(&db))
}

/// Decide whether `db_path` is one of the databases listed in `protected`.
///
/// Split out from the env lookup so the comparison is unit-testable without
/// mutating process-global environment.
fn is_protected_db(db_path: &Path, protected: &str) -> bool {
    let canonical = canonical_db_path(db_path);
    protected
        .split(':')
        .filter_map(protected_entry_to_db)
        .any(|candidate| candidate == canonical)
}

/// Abort if this process is about to open a database the test harness declared
/// off-limits.
///
/// Deliberately a panic, not an error: a test that reaches a real store has
/// already proven its isolation is broken, and returning `Err` would let a
/// tolerant caller swallow the evidence. The env var is read per connection
/// open (not cached) because opens are rare and in-process tests set the
/// variable after start.
fn assert_not_protected(db_path: &Path) {
    let Some(protected) = std::env::var_os(PROTECTED_DBS_ENV) else {
        return;
    };
    let protected = protected.to_string_lossy();
    if is_protected_db(db_path, &protected) {
        panic!(
            "refusing to open protected database {}: this process is running under \
             {PROTECTED_DBS_ENV} and must use an isolated CAS store. Anchor the test (and \
             every `cas` subprocess it spawns) to a temp directory — see \
             cas-cli/tests/support/mod.rs::CasSandbox.",
            db_path.display()
        );
    }
}

/// Get or create a shared SQLite connection for the given database path.
///
/// All callers with the same canonical path share one underlying `Connection`.
/// PRAGMAs (WAL, busy_timeout, etc.) are configured exactly once per connection.
pub fn shared_connection(db_path: &Path) -> crate::Result<Arc<Mutex<Connection>>> {
    assert_not_protected(db_path);
    crate::wait_budget::note_store_access("shared_connection");

    let canonical = canonical_db_path(db_path);

    let (slot, sweep_due) = {
        let mut guard = lock_pool();
        let pool = guard.get_or_insert_with(Pool::default);
        let slot = Arc::clone(pool.slots.entry(canonical.clone()).or_default());
        let now = Instant::now();
        let sweep_due = pool
            .last_sweep
            .is_none_or(|last| now.saturating_duration_since(last) >= IDLE_SWEEP_INTERVAL);
        if sweep_due {
            pool.last_sweep = Some(now);
        }
        (slot, sweep_due)
    };

    let (shared, opened) = {
        // Only callers of this same database wait here.
        let mut state = lock_slot(&slot);
        let stale = state
            .connection
            .as_ref()
            .is_some_and(|pooled| pooled.idle() && pooled.identity != file_identity(&canonical));
        if stale {
            // The file was deleted or replaced since this idle connection
            // opened it: close it (under the slot lock) and open the new file.
            drop(state.connection.take());
        }
        match state.connection.as_mut() {
            Some(pooled) => {
                pooled.last_used = Instant::now();
                (Arc::clone(&pooled.shared), false)
            }
            None => {
                let shared = Arc::new(Mutex::new(open_configured(db_path)?));
                state.connection = Some(PooledConnection {
                    shared: Arc::clone(&shared),
                    identity: file_identity(&canonical),
                    last_used: Instant::now(),
                });
                (shared, true)
            }
        }
    };
    drop(slot);

    // A new open is also when the idle cap can be exceeded, so sweep then too.
    if sweep_due || opened {
        sweep_idle_connections(
            Instant::now(),
            IDLE_CONNECTION_TTL,
            MAX_IDLE_CONNECTIONS,
            |_| true,
        );
    }
    Ok(shared)
}

/// Open a database and apply the PRAGMAs every shared connection carries.
fn open_configured(db_path: &Path) -> crate::Result<Connection> {
    let conn = Connection::open(db_path)?;
    install_busy_handler(&conn)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;\
         PRAGMA synchronous=NORMAL;\
         PRAGMA foreign_keys=ON;\
         PRAGMA mmap_size=268435456;\
         PRAGMA cache_size=-8000;",
    )?;
    // Bound the WAL file so a long-lived writer fleet cannot leave a journal
    // orders of magnitude larger than its live frames (cas-759f).
    conn.pragma_update(None, "journal_size_limit", WAL_SIZE_LIMIT_BYTES)?;
    Ok(conn)
}

/// Install the pooled connections' busy handler: SQLite's [`SQLITE_BUSY_TIMEOUT`]
/// schedule, cut short by this thread's wait budget ([`crate::wait_budget`]).
///
/// Code that temporarily narrows a pooled connection with `busy_timeout` must
/// restore with this, not `busy_timeout(SQLITE_BUSY_TIMEOUT)`, which would
/// replace the budget-aware handler with SQLite's built-in one.
pub fn install_busy_handler(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_handler(Some(crate::wait_budget::busy_handler))
}

/// RAII guard for an IMMEDIATE transaction.
///
/// Unlike `rusqlite::Transaction` (which uses DEFERRED), this acquires the
/// write lock immediately, preventing the deadlock pattern where two readers
/// try to upgrade to writers simultaneously.
pub struct ImmediateTx<'a> {
    conn: &'a Connection,
    committed: bool,
}

impl<'a> ImmediateTx<'a> {
    /// Start a new IMMEDIATE transaction on the given connection.
    pub fn new(conn: &'a Connection) -> rusqlite::Result<Self> {
        conn.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Self {
            conn,
            committed: false,
        })
    }

    /// Commit the transaction.
    pub fn commit(mut self) -> rusqlite::Result<()> {
        self.conn.execute_batch("COMMIT")?;
        self.committed = true;
        Ok(())
    }
}

impl<'a> Drop for ImmediateTx<'a> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}

impl<'a> std::ops::Deref for ImmediateTx<'a> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn
    }
}

/// Cap on the WAL file left behind after a checkpoint (64 MiB).
///
/// SQLite only truncates the WAL when a limit is set; without one the file
/// grows to its historical high-water mark and stays there. The store on this
/// host was carrying a 389 MB WAL for 1,137 live frames (cas-759f). This is a
/// size cap, not a checkpoint policy: it costs nothing at runtime and cannot
/// stall a writer, which is why it is the half of the WAL problem worth fixing
/// on the connection-open path.
pub const WAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024;

/// Default backoff for a contended write transaction, in milliseconds.
const WRITE_TXN_BACKOFF_MS: &[u64] = &[50, 100, 200, 400, 800];

/// Acquire a `BEGIN IMMEDIATE` write transaction, waiting out a foreign write
/// lock instead of failing on the first collision.
///
/// Why this exists rather than `Connection::transaction()`: rusqlite's default
/// is DEFERRED, so a block that reads before it writes takes a read snapshot
/// and must later *upgrade* to a writer. SQLite answers that upgrade with
/// SQLITE_BUSY **without ever calling the busy handler** — blocking there could
/// deadlock two readers upgrading at once — so the connection's `busy_timeout`
/// is silently irrelevant and the caller fails in milliseconds under a burst of
/// contention. That is exactly how four consecutive verification writes failed
/// while single-statement writes in the same seconds succeeded (cas-759f).
///
/// Taking the write lock up front puts the wait somewhere the busy handler
/// applies, and the bounded jittered retry here covers the case where the
/// holder outlives one `busy_timeout` window.
///
/// Nothing is retried once the transaction is open: the caller's body runs
/// exactly once, so a body that consumes a single-use token cannot consume it
/// twice.
///
/// This form is for a connection the caller owns outright. A pooled
/// [`shared_connection`] must use [`begin_immediate_pooled`] instead: calling
/// this through a held `MutexGuard` keeps the process-wide mutex locked across
/// every wait (cas-3f65e).
pub fn begin_immediate_with_retry(conn: &Connection) -> crate::Result<ImmediateTx<'_>> {
    begin_immediate_with_retry_bounded(conn, WRITE_TXN_BACKOFF_MS)
}

/// [`begin_immediate_with_retry`] with an explicit backoff schedule, so tests
/// can exhaust the budget without waiting seconds for it.
pub fn begin_immediate_with_retry_bounded<'a>(
    conn: &'a Connection,
    backoff_ms: &[u64],
) -> crate::Result<ImmediateTx<'a>> {
    let started = Instant::now();
    let mut attempts = 0usize;
    let mut last_busy: Option<rusqlite::Error> = None;

    for base_ms in backoff_ms.iter().copied().chain(std::iter::once(0)) {
        attempts += 1;
        let is_final = attempts > backoff_ms.len();

        match ImmediateTx::new(conn) {
            Ok(tx) => return Ok(tx),
            Err(error) if is_busy_error(&error) => {
                last_busy = Some(error);
                if is_final {
                    break;
                }
                // ±50% jitter, matching `with_write_retry`: without it a fleet
                // of daemons wakes and collides on the same instant.
                let jitter_range = base_ms / 2;
                let jitter = cheap_random_u64() % (jitter_range * 2 + 1);
                let delay_ms = base_ms - jitter_range + jitter;
                // A thread with a wait budget (the factory UI loop) gives up
                // instead of sleeping past it (GH #1165).
                let Some(sleep) = crate::wait_budget::clamp_wait(Duration::from_millis(delay_ms))
                else {
                    break;
                };
                tracing::warn!(
                    base_ms,
                    delay_ms,
                    attempts,
                    "write lock held by another connection, retrying after backoff with jitter"
                );
                std::thread::sleep(sleep);
            }
            Err(error) => return Err(StoreError::Database(error)),
        }
    }

    if crate::wait_budget::wait_budget_exhausted() {
        // Keep the busy shape so callers that defer on busy do so here.
        return Err(crate::wait_budget::budget_exhausted_store_error(
            "BEGIN IMMEDIATE",
        ));
    }
    // The bare "database is locked" is what made the original report
    // un-triageable: it does not say whether anything waited. State it.
    Err(StoreError::Other(format!(
        "database busy for {:.1}s across {attempts} attempt(s); another connection held the \
         write lock for the whole wait{}",
        started.elapsed().as_secs_f64(),
        last_busy
            .map(|error| format!(" (last: {error})"))
            .unwrap_or_default(),
    )))
}

/// Run `body` inside a write transaction acquired by
/// [`begin_immediate_with_retry`], committing on success.
pub fn with_immediate_write_txn<T, F>(conn: &Connection, body: F) -> crate::Result<T>
where
    F: FnOnce(&ImmediateTx<'_>) -> crate::Result<T>,
{
    with_immediate_write_txn_bounded(conn, WRITE_TXN_BACKOFF_MS, body)
}

/// [`with_immediate_write_txn`] with an explicit backoff schedule.
pub fn with_immediate_write_txn_bounded<T, F>(
    conn: &Connection,
    backoff_ms: &[u64],
    body: F,
) -> crate::Result<T>
where
    F: FnOnce(&ImmediateTx<'_>) -> crate::Result<T>,
{
    let tx = begin_immediate_with_retry_bounded(conn, backoff_ms)?;
    let value = body(&tx)?;
    tx.commit()?;
    Ok(value)
}

/// Longest one pooled attempt waits inside SQLite's busy handler while it holds
/// the process-wide connection mutex.
const POOLED_ATTEMPT_BUSY: Duration = Duration::from_millis(100);

/// Total wait a pooled writer spends on a foreign write lock before it fails.
/// Matches the unpooled schedule's worst case (six 5s busy waits plus ~1.5s of
/// backoff), so moving a caller onto the pooled path does not make it give up
/// sooner.
const POOLED_WRITE_BUDGET: Duration = Duration::from_millis(31_500);

/// Sleep between pooled attempts, in milliseconds; the last entry repeats.
/// Short because the attempts themselves already wait in the busy handler, and
/// a long sleep would lose the lock to writers that poll faster.
const POOLED_BACKOFF_MS: &[u64] = &[5, 10, 20, 40, 80];

/// An IMMEDIATE transaction on a pooled connection that owns the connection's
/// mutex guard for the transaction's lifetime.
///
/// Built only by [`begin_immediate_pooled`], which takes the guard per attempt
/// and drops it while it sleeps; see that function for why.
pub struct PooledImmediateTx<'a> {
    conn: MutexGuard<'a, Connection>,
    committed: bool,
}

impl PooledImmediateTx<'_> {
    /// Commit the transaction, then release the connection mutex.
    pub fn commit(mut self) -> rusqlite::Result<()> {
        self.conn.execute_batch("COMMIT")?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PooledImmediateTx<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}

impl std::ops::Deref for PooledImmediateTx<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

/// Acquire a `BEGIN IMMEDIATE` write transaction on a pooled connection without
/// holding the connection mutex while it waits for a foreign writer.
///
/// [`begin_immediate_with_retry`] takes a `&Connection`, so a pooled caller
/// must already hold the process-wide `Mutex<Connection>` and keeps holding it
/// through every busy wait and backoff sleep, ~31.6s at worst. If another
/// thread in the same process holds the SQLite write lock on a different
/// connection and needs this pooled connection to finish, neither can proceed
/// until the budget runs out: the cas-0e57 task-sync incident.
///
/// Here each attempt takes the mutex, waits at most [`POOLED_ATTEMPT_BUSY`] in
/// the busy handler, and on SQLITE_BUSY drops the mutex before sleeping, so an
/// in-process lock holder that needs the connection gets it within one
/// attempt. The overall wait budget is unchanged. The connection's
/// budget-aware busy handler ([`install_busy_handler`]) is restored before the
/// mutex is released, which is what every pooled connection is opened with. A
/// thread under a store wait budget ([`crate::wait_budget`]) also stops at its
/// deadline (GH #1165).
///
/// As with the unpooled form, nothing runs inside the transaction until it is
/// open, so the caller's body executes exactly once.
pub fn begin_immediate_pooled(shared: &Mutex<Connection>) -> crate::Result<PooledImmediateTx<'_>> {
    begin_immediate_pooled_bounded(shared, POOLED_ATTEMPT_BUSY, POOLED_WRITE_BUDGET)
}

/// [`begin_immediate_pooled`] with an explicit per-attempt busy wait and total
/// budget, so tests can exhaust the budget quickly.
pub fn begin_immediate_pooled_bounded(
    shared: &Mutex<Connection>,
    attempt_busy: Duration,
    budget: Duration,
) -> crate::Result<PooledImmediateTx<'_>> {
    let started = Instant::now();
    let mut attempts = 0usize;

    let last_busy = loop {
        attempts += 1;
        let conn = lock_connection_mutex(shared)?;
        // GH #1165: a thread under a store wait budget never waits past it.
        let attempt_busy = match crate::wait_budget::remaining_wait() {
            Some(left) if left.is_zero() => {
                drop(conn);
                return Err(crate::wait_budget::budget_exhausted_store_error(
                    "BEGIN IMMEDIATE",
                ));
            }
            Some(left) => attempt_busy.min(left),
            None => attempt_busy,
        };
        conn.busy_timeout(attempt_busy)?;
        let begun = conn.execute_batch("BEGIN IMMEDIATE");
        // Restore the pool's budget-aware handler, not SQLite's built-in one.
        let restored = install_busy_handler(&conn);
        match begun {
            Ok(()) => {
                let tx = PooledImmediateTx {
                    conn,
                    committed: false,
                };
                restored?;
                return Ok(tx);
            }
            Err(error) if is_busy_error(&error) => {
                restored?;
                // Release the process mutex before sleeping: this is the point
                // of the pooled form.
                drop(conn);
                if started.elapsed() >= budget || crate::wait_budget::wait_budget_exhausted() {
                    break error;
                }
                let base_ms = POOLED_BACKOFF_MS
                    .get(attempts - 1)
                    .or(POOLED_BACKOFF_MS.last())
                    .copied()
                    .unwrap_or(0);
                let jitter_range = base_ms / 2;
                let jitter = cheap_random_u64() % (jitter_range * 2 + 1);
                let delay_ms = base_ms - jitter_range + jitter;
                if attempts == 1 || attempts.is_power_of_two() {
                    tracing::warn!(
                        attempts,
                        delay_ms,
                        "write lock held by another connection, retrying with the pooled \
                         connection released"
                    );
                }
                let Some(sleep) =
                    crate::wait_budget::clamp_wait(Duration::from_millis(delay_ms))
                else {
                    break error;
                };
                std::thread::sleep(sleep);
            }
            Err(error) => {
                restored?;
                return Err(StoreError::Database(error));
            }
        }
    };

    if crate::wait_budget::wait_budget_exhausted() {
        // Keep the busy shape so callers that defer on busy do so here.
        return Err(crate::wait_budget::budget_exhausted_store_error(
            "BEGIN IMMEDIATE",
        ));
    }

    Err(StoreError::Other(format!(
        "database busy for {:.1}s across {attempts} attempt(s); another connection held the \
         write lock for the whole wait (last: {last_busy})",
        started.elapsed().as_secs_f64(),
    )))
}

/// Run `body` inside a write transaction acquired by
/// [`begin_immediate_pooled`], committing on success.
pub fn with_immediate_write_txn_pooled<T, F>(shared: &Mutex<Connection>, body: F) -> crate::Result<T>
where
    F: FnOnce(&Connection) -> crate::Result<T>,
{
    let tx = begin_immediate_pooled(shared)?;
    let value = body(&tx)?;
    tx.commit()?;
    Ok(value)
}

/// Atomically fetch-and-increment a named sequence, returning the next value.
///
/// Uses `INSERT ... ON CONFLICT DO UPDATE` for a single atomic statement.
/// If the table does not yet exist (fresh database before migration), it is
/// created lazily on first call.
pub fn next_sequence_val(conn: &Connection, name: &str) -> crate::Result<i64> {
    match next_sequence_val_inner(conn, name) {
        Ok(val) => Ok(val),
        Err(crate::error::StoreError::Database(ref e))
            if e.to_string().contains("no such table: id_sequences") =>
        {
            // Table hasn't been created via migration yet — bootstrap it
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS id_sequences (
                    name TEXT PRIMARY KEY,
                    next_val INTEGER NOT NULL DEFAULT 1
                )",
            )?;
            next_sequence_val_inner(conn, name)
        }
        Err(e) => Err(e),
    }
}

fn next_sequence_val_inner(conn: &Connection, name: &str) -> crate::Result<i64> {
    let val: i64 = conn.query_row(
        "INSERT INTO id_sequences (name, next_val) VALUES (?1, 1)
         ON CONFLICT(name) DO UPDATE SET next_val = next_val + 1
         RETURNING next_val",
        rusqlite::params![name],
        |row| row.get(0),
    )?;
    Ok(val)
}

/// Check if a `rusqlite::Error` is a SQLITE_BUSY error.
pub fn is_busy_error(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::DatabaseBusy,
                ..
            },
            _
        )
    )
}

/// Whether an error message is SQLite's concurrent "duplicate column name" race.
pub fn is_duplicate_column_error(e: &rusqlite::Error) -> bool {
    let msg = e.to_string().to_lowercase();
    msg.contains("duplicate column")
}

/// True when `table` has a column named `column` (via `PRAGMA table_info`).
///
/// More reliable than `SELECT col FROM table LIMIT 0` for schema probes.
/// `table` / `column` must be trusted identifiers (not user input).
pub fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info(\"{table}\")")) else {
        return false;
    };
    let Ok(rows) = stmt.query_map([], |row| {
        let name: String = row.get(1)?;
        Ok(name)
    }) else {
        return false;
    };
    rows.flatten().any(|name| name == column)
}

/// Ensure `column` exists on `table` by running `alter_sql` only when missing.
///
/// **Concurrency (cas-88d8):** two processes can both observe a missing column
/// and race on `ALTER TABLE ... ADD COLUMN`. The loser gets "duplicate column
/// name". We recheck presence and treat that as success **only** when the
/// column now exists. All other migration errors still surface.
///
/// Note: SQLite auto-commits DDL, so do **not** wrap ADD COLUMN in a larger
/// multi-statement ImmediateTx that also creates indexes — the transaction
/// state becomes inconsistent after ALTER.
///
/// `table` / `column` must be trusted identifiers (not user input).
pub fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    alter_sql: &str,
) -> crate::Result<()> {
    if column_exists(conn, table, column) {
        return Ok(());
    }
    match conn.execute_batch(alter_sql.trim()) {
        Ok(()) => Ok(()),
        Err(e) if is_duplicate_column_error(&e) => {
            // Authoritative recheck: another initializer won the race.
            if column_exists(conn, table, column) {
                Ok(())
            } else {
                Err(crate::error::StoreError::Database(e))
            }
        }
        Err(e) => Err(crate::error::StoreError::Database(e)),
    }
}

/// Execute a fallible closure with retry on SQLITE_BUSY errors.
///
/// Uses exponential backoff with jitter: base delays of 50ms, 100ms, 200ms,
/// 400ms, 800ms plus ±50% random jitter (5 retries). The jitter breaks convoy
/// patterns where multiple agents wake up and retry at the same instant.
/// Combined with the 5s busy_timeout, this gives a total max wait of ~28s
/// before giving up.
pub fn with_write_retry<T, F>(f: F) -> crate::Result<T>
where
    F: Fn() -> crate::Result<T>,
{
    let base_delays_ms: [u64; 5] = [50, 100, 200, 400, 800];

    for base_ms in &base_delays_ms {
        match f() {
            Ok(val) => return Ok(val),
            Err(crate::error::StoreError::Database(ref e)) if is_busy_error(e) => {
                // Add ±50% jitter: actual delay is in [base/2, base*3/2]
                let jitter_range = base_ms / 2;
                let jitter = cheap_random_u64() % (jitter_range * 2 + 1);
                let delay_ms = base_ms - jitter_range + jitter;
                // A thread with a wait budget returns the busy error once the
                // budget is spent instead of sleeping (GH #1165).
                let Some(sleep) = crate::wait_budget::clamp_wait(Duration::from_millis(delay_ms))
                else {
                    return f();
                };
                tracing::warn!(
                    base_ms,
                    delay_ms,
                    "SQLite busy, retrying after backoff with jitter"
                );
                std::thread::sleep(sleep);
            }
            Err(e) => return Err(e),
        }
    }

    // Final attempt (no retry)
    f()
}

/// Fast, non-cryptographic random u64 using thread-local xorshift state.
/// Seeded from thread ID + timestamp to avoid convoy patterns across agents.
fn cheap_random_u64() -> u64 {
    use std::cell::Cell;

    thread_local! {
        static STATE: Cell<u64> = Cell::new({
            let thread_id = std::thread::current().id();
            let tid_hash = format!("{thread_id:?}");
            let mut seed: u64 = 0;
            for b in tid_hash.bytes() {
                seed = seed.wrapping_mul(31).wrapping_add(b as u64);
            }
            // Mix in timestamp for cross-process uniqueness
            seed ^= std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            // Ensure non-zero
            if seed == 0 { 1 } else { seed }
        });
    }

    STATE.with(|cell| {
        let mut s = cell.get();
        // xorshift64
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        cell.set(s);
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic;
    use std::sync::Barrier;
    use tempfile::TempDir;

    // ── Protected-database tripwire (cas-78c8) ──────────────────────

    #[test]
    fn protected_list_matches_the_exact_database_file() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("cas.db");
        let other = temp.path().join("other.db");

        let protected = db.display().to_string();
        assert!(is_protected_db(&db, &protected));
        assert!(!is_protected_db(&other, &protected));
    }

    #[test]
    fn protected_list_accepts_a_cas_directory_and_protects_its_db() {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();

        let protected = cas_dir.display().to_string();
        assert!(is_protected_db(&cas_dir.join("cas.db"), &protected));
        // A sibling database inside the same directory is a different file and
        // must not be swept up by the directory form.
        assert!(!is_protected_db(&cas_dir.join("factory.db"), &protected));
    }

    #[test]
    fn protected_list_handles_multiple_entries_and_empty_segments() {
        let temp = TempDir::new().unwrap();
        let global = temp.path().join("global.db");
        let project = temp.path().join("project.db");
        let innocent = temp.path().join("temp.db");

        let protected = format!("{}::{}:", global.display(), project.display());
        assert!(is_protected_db(&global, &protected));
        assert!(is_protected_db(&project, &protected));
        assert!(!is_protected_db(&innocent, &protected));
    }

    #[test]
    fn protected_matching_is_not_a_prefix_match() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("cas.db");
        // A path that merely *starts with* a protected path must not match —
        // the earlier fixture leak was diagnosed with substring reasoning and
        // the guard must not repeat it.
        let decoy = temp.path().join("cas.db.backup");

        let protected = db.display().to_string();
        assert!(!is_protected_db(&decoy, &protected));
    }

    #[test]
    fn empty_protected_list_protects_nothing() {
        let temp = TempDir::new().unwrap();
        assert!(!is_protected_db(&temp.path().join("cas.db"), ""));
    }

    // ── Connection pool basics ──────────────────────────────────────

    #[test]
    fn shared_connection_returns_same_instance() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        let conn1 = shared_connection(&db_path).unwrap();
        let conn2 = shared_connection(&db_path).unwrap();

        assert!(Arc::ptr_eq(&conn1, &conn2));
    }

    #[test]
    fn shared_connection_different_paths_different_instances() {
        let temp = TempDir::new().unwrap();
        let db1 = temp.path().join("a.db");
        let db2 = temp.path().join("b.db");

        let conn1 = shared_connection(&db1).unwrap();
        let conn2 = shared_connection(&db2).unwrap();

        assert!(!Arc::ptr_eq(&conn1, &conn2));
    }

    /// Tag the connection behind `shared` with a per-connection TEMP table and
    /// report whether it was untagged, meaning SQLite opened it fresh. TEMP
    /// objects live and die with one connection, so unlike `Arc` addresses
    /// (which the allocator may reuse) the tag cannot survive a reopen.
    fn first_sight_of_connection(shared: &Arc<Mutex<Connection>>) -> bool {
        let conn = shared.lock().unwrap();
        let tagged: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM temp.sqlite_master WHERE name = 'e335_connection_tag'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        if tagged == 0 {
            conn.execute_batch("CREATE TEMP TABLE e335_connection_tag (x INTEGER);")
                .unwrap();
        }
        tagged == 0
    }

    /// The pool, not the last caller, owns the close (cas-e335): dropping every
    /// handle leaves the connection pooled, and only the pool closes it.
    #[test]
    fn shared_connection_is_reused_after_every_caller_drops() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        let conn1 = shared_connection(&db_path).unwrap();
        assert!(first_sight_of_connection(&conn1));
        drop(conn1);

        let conn2 = shared_connection(&db_path).unwrap();
        assert!(
            !first_sight_of_connection(&conn2),
            "dropping the last caller handle must not close the pooled connection"
        );
        assert!(
            !close_idle_connection(&db_path),
            "a connection a caller holds is never closed"
        );
        drop(conn2);

        assert!(close_idle_connection(&db_path));
        let conn3 = shared_connection(&db_path).unwrap();
        assert!(
            first_sight_of_connection(&conn3),
            "after the pool closes an idle connection the next caller gets a fresh one"
        );
    }

    #[test]
    fn shared_connection_keeps_alive_while_any_arc_exists() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        let conn1 = shared_connection(&db_path).unwrap();
        let conn2 = shared_connection(&db_path).unwrap();
        let ptr = Arc::as_ptr(&conn1);

        // Drop one clone — the other keeps the connection alive
        drop(conn1);
        let conn3 = shared_connection(&db_path).unwrap();
        assert_eq!(ptr, Arc::as_ptr(&conn3));
        assert!(!close_idle_connection(&db_path));

        // Drop all — the pool still holds it, so the next call reuses it
        drop(conn2);
        drop(conn3);
        let conn4 = shared_connection(&db_path).unwrap();
        assert_eq!(ptr, Arc::as_ptr(&conn4));
    }

    /// An idle pooled connection whose database file was deleted must not be
    /// handed out: writes would land in the unlinked file and vanish.
    #[test]
    fn an_idle_connection_to_a_deleted_database_is_reopened() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("replaced.db");

        let conn = shared_connection(&db_path).unwrap();
        assert!(first_sight_of_connection(&conn));
        drop(conn);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(temp.path().join(format!("replaced.db{suffix}")));
        }

        let reopened = shared_connection(&db_path).unwrap();
        assert!(
            first_sight_of_connection(&reopened),
            "a deleted database must be reopened, not served from the stale connection"
        );
        assert!(db_path.exists(), "the reopen recreates the database file");
    }

    #[test]
    fn shared_connection_pragmas_are_set() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        let conn = shared_connection(&db_path).unwrap();
        let guard = conn.lock().unwrap();

        let journal: String = guard
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal, "wal");

        let fk: i64 = guard
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);

        let sync: i64 = guard
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        // NORMAL = 1
        assert_eq!(sync, 1);
    }

    #[test]
    fn shared_connection_data_persists_across_callers() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        // First caller creates a table and inserts data
        {
            let conn = shared_connection(&db_path).unwrap();
            let guard = conn.lock().unwrap();
            guard
                .execute_batch("CREATE TABLE persist_test (val TEXT)")
                .unwrap();
            guard
                .execute("INSERT INTO persist_test VALUES ('hello')", [])
                .unwrap();
        }

        // Second caller (same connection) can read it
        {
            let conn = shared_connection(&db_path).unwrap();
            let guard = conn.lock().unwrap();
            let val: String = guard
                .query_row("SELECT val FROM persist_test", [], |r| r.get(0))
                .unwrap();
            assert_eq!(val, "hello");
        }
    }

    // ── Pool poisoning recovery ─────────────────────────────────────

    #[test]
    fn pool_recovers_from_poisoned_mutex() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("poison.db");

        // Poison the POOL mutex by panicking while holding it
        let _ = panic::catch_unwind(|| {
            let mut guard = POOL.lock().unwrap();
            let _pool = guard.get_or_insert_with(Pool::default);
            panic!("intentional poison");
        });

        // shared_connection should still work via unwrap_or_else(into_inner)
        let conn = shared_connection(&db_path).unwrap();
        let guard = conn.lock().unwrap();
        guard.execute_batch("SELECT 1").unwrap();
    }

    #[test]
    fn lock_connection_returns_error_for_poisoned_mutex() {
        let conn = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        let poisoned = Arc::clone(&conn);

        let _ = panic::catch_unwind(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("intentional poison");
        });

        assert!(
            matches!(lock_connection(&conn), Err(StoreError::Other(message)) if message == "shared SQLite connection lock poisoned")
        );
    }

    // ── Concurrent access ───────────────────────────────────────────

    #[test]
    fn concurrent_shared_connection_calls_return_same_instance() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("concurrent.db");

        let num_threads = 20;
        let barrier = Arc::new(Barrier::new(num_threads));
        let path = db_path.clone();

        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                let b = barrier.clone();
                let p = path.clone();
                std::thread::spawn(move || {
                    b.wait();
                    shared_connection(&p).unwrap()
                })
            })
            .collect();

        let conns: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All threads should get the same Arc
        for conn in &conns[1..] {
            assert!(Arc::ptr_eq(&conns[0], conn));
        }
    }

    #[test]
    fn concurrent_writers_through_shared_connection() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("writers.db");

        let conn = shared_connection(&db_path).unwrap();
        {
            let guard = conn.lock().unwrap();
            guard
                .execute_batch(
                    "CREATE TABLE counters (id INTEGER PRIMARY KEY, val INTEGER DEFAULT 0)",
                )
                .unwrap();
            guard
                .execute("INSERT INTO counters (id, val) VALUES (1, 0)", [])
                .unwrap();
        }

        let num_threads = 50;
        let barrier = Arc::new(Barrier::new(num_threads));

        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                let c = conn.clone();
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    let guard = c.lock().unwrap();
                    guard
                        .execute("UPDATE counters SET val = val + 1 WHERE id = 1", [])
                        .unwrap();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let guard = conn.lock().unwrap();
        let val: i64 = guard
            .query_row("SELECT val FROM counters WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(val, num_threads as i64);
    }

    #[test]
    fn concurrent_readers_dont_block() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("readers.db");

        let conn = shared_connection(&db_path).unwrap();
        {
            let guard = conn.lock().unwrap();
            guard
                .execute_batch("CREATE TABLE data (id INTEGER, val TEXT)")
                .unwrap();
            for i in 0..100 {
                guard
                    .execute(
                        "INSERT INTO data VALUES (?1, ?2)",
                        rusqlite::params![i, format!("value_{i}")],
                    )
                    .unwrap();
            }
        }

        // Open a second (separate) connection for reads — WAL allows concurrent reads
        let read_conn = Connection::open(&db_path).unwrap();
        read_conn.execute_batch("PRAGMA journal_mode=WAL").unwrap();

        let num_readers = 10;
        let barrier = Arc::new(Barrier::new(num_readers));
        let path = db_path.clone();

        let handles: Vec<_> = (0..num_readers)
            .map(|_| {
                let b = barrier.clone();
                let p = path.clone();
                std::thread::spawn(move || {
                    b.wait();
                    // Each reader opens its own connection (simulating separate processes)
                    let rc = Connection::open(&p).unwrap();
                    rc.execute_batch("PRAGMA journal_mode=WAL").unwrap();
                    let count: i64 = rc
                        .query_row("SELECT COUNT(*) FROM data", [], |r| r.get(0))
                        .unwrap();
                    assert_eq!(count, 100);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }

    // ── ImmediateTx ────────────────────────────────────────────────

    #[test]
    fn immediate_tx_commits() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE t (x INTEGER)").unwrap();

        {
            let tx = ImmediateTx::new(&conn).unwrap();
            tx.execute("INSERT INTO t VALUES (1)", []).unwrap();
            tx.commit().unwrap();
        }

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn immediate_tx_rolls_back_on_drop() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE t (x INTEGER)").unwrap();

        {
            let tx = ImmediateTx::new(&conn).unwrap();
            tx.execute("INSERT INTO t VALUES (1)", []).unwrap();
            // drop without commit
        }

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn immediate_tx_rolls_back_on_panic() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("panic.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE t (x INTEGER)").unwrap();

        let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            let tx = ImmediateTx::new(&conn).unwrap();
            tx.execute("INSERT INTO t VALUES (42)", []).unwrap();
            panic!("simulated error");
        }));

        // The row should NOT be present after panic-triggered rollback
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn immediate_tx_deref_allows_queries() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("deref.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE t (x INTEGER)").unwrap();
        conn.execute("INSERT INTO t VALUES (99)", []).unwrap();

        let tx = ImmediateTx::new(&conn).unwrap();
        // Use Deref to call Connection methods directly on tx
        let val: i64 = tx.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(val, 99);
        tx.commit().unwrap();
    }

    #[test]
    fn immediate_tx_sequential_transactions() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("seq.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE t (x INTEGER)").unwrap();

        // First transaction — commit
        {
            let tx = ImmediateTx::new(&conn).unwrap();
            tx.execute("INSERT INTO t VALUES (1)", []).unwrap();
            tx.commit().unwrap();
        }

        // Second transaction — rollback
        {
            let tx = ImmediateTx::new(&conn).unwrap();
            tx.execute("INSERT INTO t VALUES (2)", []).unwrap();
            // drop without commit
        }

        // Third transaction — commit
        {
            let tx = ImmediateTx::new(&conn).unwrap();
            tx.execute("INSERT INTO t VALUES (3)", []).unwrap();
            tx.commit().unwrap();
        }

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2); // Only rows 1 and 3

        let sum: i64 = conn
            .query_row("SELECT SUM(x) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sum, 4); // 1 + 3
    }

    #[test]
    fn immediate_tx_multi_statement() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("multi.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);\
             CREATE TABLE log (msg TEXT);",
        )
        .unwrap();

        {
            let tx = ImmediateTx::new(&conn).unwrap();
            tx.execute("INSERT INTO items VALUES (1, 'alpha')", [])
                .unwrap();
            tx.execute("INSERT INTO items VALUES (2, 'beta')", [])
                .unwrap();
            tx.execute("INSERT INTO log VALUES ('inserted 2 items')", [])
                .unwrap();
            tx.commit().unwrap();
        }

        let item_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap();
        let log_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(item_count, 2);
        assert_eq!(log_count, 1);
    }

    /// Prepare a WAL database with one row, plus a connection configured the
    /// way the store configures its own (5s busy timeout).
    fn contended_db(temp: &tempfile::TempDir) -> (std::path::PathBuf, Connection) {
        let db_path = temp.path().join("contended.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.busy_timeout(SQLITE_BUSY_TIMEOUT).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;\
             CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);\
             INSERT INTO t VALUES (1, 'seed');",
        )
        .unwrap();
        (db_path, conn)
    }

    /// Hold a write lock on `db_path` for `hold`, signalling once it is held.
    fn hold_write_lock(
        db_path: std::path::PathBuf,
        hold: Duration,
    ) -> (std::sync::mpsc::Receiver<()>, std::thread::JoinHandle<()>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let other = Connection::open(&db_path).unwrap();
            other.busy_timeout(SQLITE_BUSY_TIMEOUT).unwrap();
            other.execute_batch("BEGIN IMMEDIATE").unwrap();
            other.execute("INSERT INTO t VALUES (99, 'foreign')", []).unwrap();
            tx.send(()).unwrap();
            std::thread::sleep(hold);
            other.execute_batch("COMMIT").unwrap();
        });
        (rx, handle)
    }

    /// cas-759f / the GH-reported "database is locked": a DEFERRED transaction
    /// that reads before it writes holds a read snapshot, and SQLite refuses
    /// the upgrade to a writer WITHOUT consulting the busy handler — waiting
    /// there could deadlock. So a 5s busy timeout buys nothing and the caller
    /// fails in milliseconds. This test pins the mechanism, because the fix
    /// only makes sense against it.
    #[test]
    fn a_deferred_read_then_write_fails_instantly_despite_the_busy_timeout() {
        let temp = TempDir::new().unwrap();
        let (db_path, conn) = contended_db(&temp);

        conn.execute_batch("BEGIN").unwrap();
        // Take the read snapshot first, exactly as the verification handler did.
        let _: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
            .unwrap();

        let (ready, holder) = hold_write_lock(db_path, Duration::from_millis(50));
        ready.recv().unwrap();
        holder.join().unwrap();

        let started = Instant::now();
        let result = conn.execute("INSERT INTO t VALUES (2, 'ours')", []);
        let waited = started.elapsed();
        let _ = conn.execute_batch("ROLLBACK");

        let error = result.expect_err("the upgrade must be refused");
        assert!(is_busy_error(&error), "unexpected error: {error}");
        assert!(
            waited < Duration::from_millis(500),
            "the busy handler was expected to be skipped entirely, but it waited {waited:?}"
        );
    }

    /// The fix: take the write lock up front, where the busy handler DOES
    /// apply, and the same caller waits through a foreign lock instead of
    /// failing.
    #[test]
    fn immediate_write_txn_waits_through_a_one_second_foreign_lock() {
        let temp = TempDir::new().unwrap();
        let (db_path, conn) = contended_db(&temp);

        let (ready, holder) = hold_write_lock(db_path, Duration::from_millis(1_000));
        ready.recv().unwrap();

        let started = Instant::now();
        let rows: i64 = with_immediate_write_txn(&conn, |tx| {
            // Reads inside the write transaction are safe: the lock is ours.
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))?;
            tx.execute("INSERT INTO t VALUES (2, 'ours')", [])?;
            Ok(count)
        })
        .expect("must wait through the foreign lock, not fail");
        let waited = started.elapsed();

        holder.join().unwrap();
        assert!(
            waited >= Duration::from_millis(900),
            "expected to wait for the holder, waited only {waited:?}"
        );
        assert!(rows >= 1);
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 3, "seed + foreign + ours");
    }

    /// Eight long-lived connections is the real fleet shape on this host (six
    /// worker daemons, the supervisor, and the session launcher all share one
    /// store), so the policy is exercised at that width rather than at two.
    #[test]
    fn eight_concurrent_writers_all_commit() {
        let temp = TempDir::new().unwrap();
        let (db_path, _conn) = contended_db(&temp);

        let mut handles = Vec::new();
        for worker in 0..8 {
            let path = db_path.clone();
            handles.push(std::thread::spawn(move || {
                let conn = Connection::open(&path).unwrap();
                conn.busy_timeout(SQLITE_BUSY_TIMEOUT).unwrap();
                for round in 0..5 {
                    with_immediate_write_txn(&conn, |tx| {
                        let _: i64 = tx.query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))?;
                        tx.execute(
                            "INSERT INTO t (v) VALUES (?1)",
                            rusqlite::params![format!("w{worker}-r{round}")],
                        )?;
                        Ok(())
                    })
                    .unwrap_or_else(|e| panic!("worker {worker} round {round} failed: {e}"));
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let conn = Connection::open(&db_path).unwrap();
        let written: i64 = conn
            .query_row("SELECT COUNT(*) FROM t WHERE v LIKE 'w%'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(written, 40, "8 writers x 5 rounds must all land");
    }

    /// When the wait genuinely runs out, the caller is told what was attempted.
    /// "database is locked" alone told the supervisor nothing about whether
    /// anything had waited at all — which is why the original report needed a
    /// manual `BEGIN IMMEDIATE` probe to establish that the lock was a burst.
    #[test]
    fn giving_up_names_the_wait_that_was_attempted() {
        let temp = TempDir::new().unwrap();
        let (db_path, conn) = contended_db(&temp);
        // A tiny budget keeps the test fast; the shape of the message is what
        // is being asserted, not the specific duration.
        conn.busy_timeout(Duration::from_millis(20)).unwrap();

        let (ready, holder) = hold_write_lock(db_path, Duration::from_millis(3_000));
        ready.recv().unwrap();

        let result: crate::Result<()> = with_immediate_write_txn_bounded(
            &conn,
            &[10, 10],
            |tx| {
                tx.execute("INSERT INTO t VALUES (2, 'ours')", [])?;
                Ok(())
            },
        );
        let message = result.expect_err("the holder outlasts the budget").to_string();
        holder.join().unwrap();

        assert!(
            message.contains("database busy for"),
            "the error must state the wait attempted: {message}"
        );
        assert!(
            message.contains("attempt"),
            "the error must state how many attempts were made: {message}"
        );
    }

    /// A store connection must cap the WAL file so a long-lived writer fleet
    /// cannot leave a 389 MB journal behind for 1,137 live frames (cas-759f).
    #[test]
    fn shared_connections_cap_the_wal_file_size() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("cas.db");
        let shared = shared_connection(&db_path).unwrap();
        let conn = shared.lock().unwrap();
        let limit: i64 = conn
            .query_row("PRAGMA journal_size_limit", [], |row| row.get(0))
            .unwrap();
        assert_eq!(limit, WAL_SIZE_LIMIT_BYTES);
    }

    // ── is_busy_error ───────────────────────────────────────────────

    #[test]
    fn is_busy_error_detects_busy() {
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::DatabaseBusy,
                extended_code: 5,
            },
            Some("database is locked".to_string()),
        );
        assert!(is_busy_error(&busy));
    }

    #[test]
    fn is_busy_error_rejects_other_errors() {
        let not_busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                extended_code: 19,
            },
            None,
        );
        assert!(!is_busy_error(&not_busy));

        let query_err = rusqlite::Error::QueryReturnedNoRows;
        assert!(!is_busy_error(&query_err));
    }

    // ── with_write_retry ────────────────────────────────────────────

    #[test]
    fn ensure_column_is_idempotent_and_tolerates_duplicate() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("mig.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY);")
            .unwrap();

        ensure_column(&conn, "t", "c1", "ALTER TABLE t ADD COLUMN c1 TEXT;").unwrap();
        assert!(column_exists(&conn, "t", "c1"));
        // Second call is a no-op (column already present).
        ensure_column(&conn, "t", "c1", "ALTER TABLE t ADD COLUMN c1 TEXT;").unwrap();

        // Force ALTER to get a real duplicate-column error, then ensure_column
        // must still succeed via recheck (race-loser path).
        match conn.execute_batch("ALTER TABLE t ADD COLUMN c1 TEXT;") {
            Err(e) => {
                assert!(
                    is_duplicate_column_error(&e),
                    "expected duplicate column, got {e}"
                );
                ensure_column(&conn, "t", "c1", "ALTER TABLE t ADD COLUMN c1 TEXT;").unwrap();
            }
            Ok(()) => {
                // Some SQLite builds may no-op re-ADD; ensure_column still Ok.
                ensure_column(&conn, "t", "c1", "ALTER TABLE t ADD COLUMN c1 TEXT;").unwrap();
            }
        }
        assert!(column_exists(&conn, "t", "c1"));
    }

    #[test]
    fn ensure_column_surfaces_genuine_migration_errors() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("mig.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY);")
            .unwrap();
        // Table does not exist for this ALTER — not a duplicate-column race.
        let err = ensure_column(
            &conn,
            "missing_table",
            "c1",
            "ALTER TABLE missing_table ADD COLUMN c1 TEXT;",
        )
        .unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("no such table") || msg.contains("missing_table"),
            "expected real schema error, got {msg}"
        );
    }

    /// cas-88d8: concurrent ensure_column on a legacy table must all succeed.
    #[test]
    fn ensure_column_concurrent_add_all_succeed() {
        let temp = TempDir::new().unwrap();
        let db = temp.path().join("race.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t (id INTEGER PRIMARY KEY);")
                .unwrap();
        }

        let barrier = Arc::new(Barrier::new(8));
        let path = db.clone();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let conn = Connection::open(&path).unwrap();
                    conn.busy_timeout(Duration::from_secs(5)).unwrap();
                    barrier.wait();
                    // No ImmediateTx here — maximize chance of check/ALTER race.
                    ensure_column(
                        &conn,
                        "t",
                        "race_col",
                        "ALTER TABLE t ADD COLUMN race_col TEXT;",
                    )
                })
            })
            .collect();

        for h in handles {
            h.join()
                .unwrap()
                .expect("concurrent ensure_column must succeed");
        }
        let conn = Connection::open(&db).unwrap();
        assert!(conn.prepare("SELECT race_col FROM t LIMIT 0").is_ok());
    }

    #[test]
    fn with_write_retry_succeeds_on_first_try() {
        let call_count = Arc::new(Mutex::new(0u32));
        let cc = call_count.clone();

        let result = with_write_retry(|| {
            *cc.lock().unwrap() += 1;
            Ok(42)
        });

        assert_eq!(result.unwrap(), 42);
        assert_eq!(*call_count.lock().unwrap(), 1);
    }

    #[test]
    fn with_write_retry_retries_on_busy_then_succeeds() {
        let call_count = Arc::new(Mutex::new(0u32));
        let cc = call_count.clone();

        let result = with_write_retry(|| {
            let mut count = cc.lock().unwrap();
            *count += 1;
            if *count <= 3 {
                // Simulate SQLITE_BUSY for first 3 calls
                Err(crate::error::StoreError::Database(
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error {
                            code: rusqlite::ffi::ErrorCode::DatabaseBusy,
                            extended_code: 5,
                        },
                        Some("database is locked".to_string()),
                    ),
                ))
            } else {
                Ok("success")
            }
        });

        assert_eq!(result.unwrap(), "success");
        assert_eq!(*call_count.lock().unwrap(), 4); // 3 retries + 1 success
    }

    #[test]
    fn with_write_retry_gives_up_after_max_retries() {
        let call_count = Arc::new(Mutex::new(0u32));
        let cc = call_count.clone();

        let result: crate::Result<()> = with_write_retry(|| {
            *cc.lock().unwrap() += 1;
            Err(crate::error::StoreError::Database(
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error {
                        code: rusqlite::ffi::ErrorCode::DatabaseBusy,
                        extended_code: 5,
                    },
                    Some("database is locked".to_string()),
                ),
            ))
        });

        assert!(result.is_err());
        // 5 retries + 1 final attempt = 6 total calls
        assert_eq!(*call_count.lock().unwrap(), 6);
    }

    #[test]
    fn with_write_retry_does_not_retry_non_busy_errors() {
        let call_count = Arc::new(Mutex::new(0u32));
        let cc = call_count.clone();

        let result: crate::Result<()> = with_write_retry(|| {
            *cc.lock().unwrap() += 1;
            Err(crate::error::StoreError::Database(
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error {
                        code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                        extended_code: 19,
                    },
                    Some("UNIQUE constraint failed".to_string()),
                ),
            ))
        });

        assert!(result.is_err());
        // Should NOT retry — only 1 call
        assert_eq!(*call_count.lock().unwrap(), 1);
    }

    #[test]
    fn with_write_retry_does_not_retry_non_database_errors() {
        let call_count = Arc::new(Mutex::new(0u32));
        let cc = call_count.clone();

        let result: crate::Result<()> = with_write_retry(|| {
            *cc.lock().unwrap() += 1;
            Err(crate::error::StoreError::NotFound("gone".to_string()))
        });

        assert!(result.is_err());
        assert_eq!(*call_count.lock().unwrap(), 1);
    }

    // ── Cross-process write contention (simulated with separate connections) ──

    #[test]
    fn cross_connection_write_contention() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("contention.db");

        // Set up the database
        let setup_conn = Connection::open(&db_path).unwrap();
        setup_conn
            .execute_batch(
                "PRAGMA journal_mode=WAL;\
                 CREATE TABLE counter (id INTEGER PRIMARY KEY, val INTEGER)",
            )
            .unwrap();
        setup_conn
            .execute("INSERT INTO counter VALUES (1, 0)", [])
            .unwrap();
        drop(setup_conn);

        let num_threads = 20;
        let barrier = Arc::new(Barrier::new(num_threads));
        let successes = Arc::new(Mutex::new(0u32));

        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                let b = barrier.clone();
                let p = db_path.clone();
                let s = successes.clone();
                std::thread::spawn(move || {
                    // Each thread gets its own connection (simulating separate processes)
                    let conn = Connection::open(&p).unwrap();
                    conn.execute_batch("PRAGMA journal_mode=WAL").unwrap();
                    conn.busy_timeout(Duration::from_secs(5)).unwrap();

                    b.wait();

                    // Try to increment the counter
                    match conn.execute("UPDATE counter SET val = val + 1 WHERE id = 1", []) {
                        Ok(_) => *s.lock().unwrap() += 1,
                        Err(e) => panic!("Write failed: {e}"),
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // All writes should succeed thanks to busy_timeout + WAL
        assert_eq!(*successes.lock().unwrap(), num_threads as u32);

        let verify_conn = Connection::open(&db_path).unwrap();
        let val: i64 = verify_conn
            .query_row("SELECT val FROM counter WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(val, num_threads as i64);
    }

    // ── ImmediateTx under contention (separate connections) ─────────

    #[test]
    fn immediate_tx_contention_across_connections() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("imm_contention.db");

        let setup_conn = Connection::open(&db_path).unwrap();
        setup_conn
            .execute_batch(
                "PRAGMA journal_mode=WAL;\
                 CREATE TABLE ledger (account TEXT, balance INTEGER)",
            )
            .unwrap();
        setup_conn
            .execute("INSERT INTO ledger VALUES ('A', 1000)", [])
            .unwrap();
        setup_conn
            .execute("INSERT INTO ledger VALUES ('B', 1000)", [])
            .unwrap();
        drop(setup_conn);

        let num_threads = 10;
        let barrier = Arc::new(Barrier::new(num_threads));

        let handles: Vec<_> = (0..num_threads)
            .map(|i| {
                let b = barrier.clone();
                let p = db_path.clone();
                std::thread::spawn(move || {
                    let conn = Connection::open(&p).unwrap();
                    conn.execute_batch("PRAGMA journal_mode=WAL").unwrap();
                    conn.busy_timeout(Duration::from_secs(5)).unwrap();

                    b.wait();

                    // Transfer 10 from A to B using ImmediateTx
                    let tx = ImmediateTx::new(&conn).unwrap();
                    tx.execute(
                        "UPDATE ledger SET balance = balance - 10 WHERE account = 'A'",
                        [],
                    )
                    .unwrap();
                    tx.execute(
                        "UPDATE ledger SET balance = balance + 10 WHERE account = 'B'",
                        [],
                    )
                    .unwrap();
                    tx.commit().unwrap();

                    i // Return thread index for tracking
                })
            })
            .collect();

        let completed: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(completed.len(), num_threads);

        // Verify totals are consistent (no lost updates)
        let verify = Connection::open(&db_path).unwrap();
        let a: i64 = verify
            .query_row("SELECT balance FROM ledger WHERE account = 'A'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let b: i64 = verify
            .query_row("SELECT balance FROM ledger WHERE account = 'B'", [], |r| {
                r.get(0)
            })
            .unwrap();

        // Total should always be 2000 (no money created or destroyed)
        assert_eq!(a + b, 2000);
        // A should have lost 10 * num_threads
        assert_eq!(a, 1000 - (num_threads as i64 * 10));
        assert_eq!(b, 1000 + (num_threads as i64 * 10));
    }

    // ── Shared connection used by multiple "store-like" callers ─────

    #[test]
    fn multiple_stores_share_connection_and_operate_independently() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("multi_store.db");

        // Simulate two different stores both getting a shared connection
        let conn1 = shared_connection(&db_path).unwrap();
        let conn2 = shared_connection(&db_path).unwrap();
        assert!(Arc::ptr_eq(&conn1, &conn2));

        // "Store A" creates its table
        {
            let guard = conn1.lock().unwrap();
            guard
                .execute_batch("CREATE TABLE store_a (id INTEGER PRIMARY KEY, data TEXT)")
                .unwrap();
        }

        // "Store B" creates its table
        {
            let guard = conn2.lock().unwrap();
            guard
                .execute_batch("CREATE TABLE store_b (id INTEGER PRIMARY KEY, data TEXT)")
                .unwrap();
        }

        // Both stores write interleaved
        {
            let guard = conn1.lock().unwrap();
            guard
                .execute("INSERT INTO store_a VALUES (1, 'from_a')", [])
                .unwrap();
        }
        {
            let guard = conn2.lock().unwrap();
            guard
                .execute("INSERT INTO store_b VALUES (1, 'from_b')", [])
                .unwrap();
        }
        {
            let guard = conn1.lock().unwrap();
            guard
                .execute("INSERT INTO store_a VALUES (2, 'from_a_2')", [])
                .unwrap();
        }

        // Verify isolation between logical stores
        let guard = conn1.lock().unwrap();
        let a_count: i64 = guard
            .query_row("SELECT COUNT(*) FROM store_a", [], |r| r.get(0))
            .unwrap();
        let b_count: i64 = guard
            .query_row("SELECT COUNT(*) FROM store_b", [], |r| r.get(0))
            .unwrap();
        assert_eq!(a_count, 2);
        assert_eq!(b_count, 1);
    }

    // ── Edge case: empty/unusual paths ──────────────────────────────

    #[test]
    fn shared_connection_works_with_nested_path() {
        let temp = TempDir::new().unwrap();
        let nested = temp.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();
        let db_path = nested.join("deep.db");

        let conn1 = shared_connection(&db_path).unwrap();
        let conn2 = shared_connection(&db_path).unwrap();
        assert!(Arc::ptr_eq(&conn1, &conn2));
    }

    // ── Stress test: many threads, mixed reads and writes ───────────

    #[test]
    fn stress_mixed_read_write_through_shared_connection() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("stress.db");

        let conn = shared_connection(&db_path).unwrap();
        {
            let guard = conn.lock().unwrap();
            guard
                .execute_batch(
                    "CREATE TABLE stress (id INTEGER PRIMARY KEY, thread_id INTEGER, val TEXT)",
                )
                .unwrap();
        }

        let num_writers = 30;
        let num_readers = 20;
        let barrier = Arc::new(Barrier::new(num_writers + num_readers));

        let mut handles = Vec::new();

        // Writer threads
        for i in 0..num_writers {
            let c = conn.clone();
            let b = barrier.clone();
            handles.push(std::thread::spawn(move || {
                b.wait();
                let guard = c.lock().unwrap();
                guard
                    .execute(
                        "INSERT INTO stress (thread_id, val) VALUES (?1, ?2)",
                        rusqlite::params![i as i64, format!("data_{i}")],
                    )
                    .unwrap();
            }));
        }

        // Reader threads
        for _ in 0..num_readers {
            let c = conn.clone();
            let b = barrier.clone();
            handles.push(std::thread::spawn(move || {
                b.wait();
                let guard = c.lock().unwrap();
                let _count: i64 = guard
                    .query_row("SELECT COUNT(*) FROM stress", [], |r| r.get(0))
                    .unwrap();
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // Verify all writes landed
        let guard = conn.lock().unwrap();
        let count: i64 = guard
            .query_row("SELECT COUNT(*) FROM stress", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, num_writers as i64);
    }

    // ── Open stalls and close/open churn (cas-e335) ─────────────────

    /// An open stalled inside SQLite must hold up only callers of that same
    /// database. Before cas-e335 the pool lock was held across
    /// `Connection::open` and its PRAGMAs, so a macOS hub whose open of one
    /// agent registry deadlocked left every other opener in the process
    /// queued behind it. The stall here is real SQLite: a foreign connection
    /// holds an EXCLUSIVE lock on a rollback-journal database, so the pooled
    /// open waits out its busy timeout in `PRAGMA journal_mode=WAL`.
    #[test]
    fn a_stalled_open_does_not_block_other_databases() {
        let temp = TempDir::new().unwrap();
        let stalled_path = temp.path().join("stalled.db");
        let other_path = temp.path().join("other.db");

        let blocker = Connection::open(&stalled_path).unwrap();
        blocker
            .execute_batch(
                "PRAGMA journal_mode=DELETE;\
                 CREATE TABLE t (id INTEGER PRIMARY KEY);\
                 BEGIN EXCLUSIVE;\
                 INSERT INTO t VALUES (1);",
            )
            .unwrap();

        let (stalled_done_tx, stalled_done) = std::sync::mpsc::channel();
        let stalled = {
            let path = stalled_path.clone();
            std::thread::spawn(move || {
                let outcome = shared_connection(&path).map(|_| ());
                let _ = stalled_done_tx.send(());
                outcome
            })
        };
        // Give the pooled open time to reach SQLite and start waiting.
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            stalled_done.try_recv().is_err(),
            "test precondition: the open of the locked database must be waiting"
        );

        let (other_tx, other_rx) = std::sync::mpsc::channel();
        let other = std::thread::spawn(move || {
            let started = Instant::now();
            let outcome = shared_connection(&other_path).map(|_| started.elapsed());
            let _ = other_tx.send(outcome.is_ok());
            outcome
        });
        let answered = other_rx.recv_timeout(Duration::from_secs(2));

        // Always release the stall before asserting, so a failure cannot hang.
        blocker.execute_batch("ROLLBACK").unwrap();
        let _ = stalled.join().unwrap();
        let other_outcome = other.join().unwrap();

        assert_eq!(
            answered,
            Ok(true),
            "opening an unrelated database waited on a stalled open: {other_outcome:?}"
        );
    }

    /// Many threads acquiring and dropping the same database as fast as they
    /// can, alongside callers of a second database. Before cas-e335 the last
    /// handle's drop closed the connection outside any pool lock while another
    /// thread reopened the file, and that close/open race deadlocked inside
    /// SQLite (a full workspace run caught 3 of 16 threads wedged). Now the pool
    /// owns every close, so neither database is ever reopened: each is opened
    /// exactly once, and every caller finishes well inside the watchdog.
    #[test]
    fn concurrent_open_and_drop_of_one_database_never_wedges() {
        let temp = TempDir::new().unwrap();
        let hot = temp.path().join("hot.db");
        let side = temp.path().join("side.db");
        let threads = 24;
        let rounds = 2_000;
        let barrier = Arc::new(Barrier::new(threads));
        let hot_opens = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let side_opens = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        for index in 0..threads {
            let barrier = Arc::clone(&barrier);
            let (path, opens) = if index % 4 == 3 {
                (side.clone(), Arc::clone(&side_opens))
            } else {
                (hot.clone(), Arc::clone(&hot_opens))
            };
            let done = done_tx.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    for _ in 0..rounds {
                        let shared = shared_connection(&path).expect("pooled open");
                        if first_sight_of_connection(&shared) {
                            opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        // The handle drops here, often as the last caller one.
                    }
                }))
                .map_err(|_| "a pooled handle failed".to_string());
                let _ = done.send(outcome);
            });
        }
        drop(done_tx);

        let deadline = Instant::now() + Duration::from_secs(60);
        for finished in 0..threads {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match done_rx.recv_timeout(remaining) {
                Ok(outcome) => outcome.expect("every pooled handle must work"),
                Err(_) => panic!(
                    "{} of {threads} threads wedged in shared_connection open/close churn",
                    threads - finished
                ),
            }
        }
        assert_eq!(
            hot_opens.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the hot database must be opened once, never closed and reopened by a caller drop"
        );
        assert_eq!(side_opens.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Pooled slots under `parent`, and how many of them hold an open connection.
    fn pooled_under(parent: &Path) -> (usize, usize) {
        let guard = lock_pool();
        let Some(pool) = guard.as_ref() else {
            return (0, 0);
        };
        let ours: Vec<_> = pool
            .slots
            .iter()
            .filter(|(path, _)| path.parent() == Some(parent))
            .collect();
        let open = ours
            .iter()
            .filter(|(_, slot)| {
                slot.state
                    .try_lock()
                    .map(|state| state.connection.is_some())
                    .unwrap_or(true)
            })
            .count();
        (ours.len(), open)
    }

    /// Idle connections stay bounded: the least recently used beyond
    /// `MAX_IDLE_CONNECTIONS` close, every one idle past `IDLE_CONNECTION_TTL`
    /// closes and leaves the pool, and a connection a caller holds is never
    /// touched.
    #[test]
    fn idle_connections_are_capped_and_expire() {
        let temp = TempDir::new().unwrap();
        let live_path = temp.path().join("live.db");
        let parent = canonical_db_path(&live_path)
            .parent()
            .unwrap()
            .to_path_buf();
        let ours = |path: &Path| path.parent() == Some(parent.as_path());

        let live = shared_connection(&live_path).unwrap();
        assert!(first_sight_of_connection(&live));
        for index in 0..MAX_IDLE_CONNECTIONS + 8 {
            drop(shared_connection(&temp.path().join(format!("short-{index}.db"))).unwrap());
        }

        sweep_idle_connections(
            Instant::now(),
            IDLE_CONNECTION_TTL,
            MAX_IDLE_CONNECTIONS,
            &ours,
        );
        let (_, open) = pooled_under(&parent);
        assert!(
            open <= MAX_IDLE_CONNECTIONS + 1,
            "idle connections beyond the cap must close ({open} open, cap {MAX_IDLE_CONNECTIONS} + 1 live)"
        );

        sweep_idle_connections(
            Instant::now() + IDLE_CONNECTION_TTL + Duration::from_secs(1),
            IDLE_CONNECTION_TTL,
            MAX_IDLE_CONNECTIONS,
            &ours,
        );
        assert_eq!(
            pooled_under(&parent),
            (1, 1),
            "expired idle connections close and leave the pool; the live one stays"
        );
        assert!(
            !first_sight_of_connection(&live),
            "a connection a caller holds is never closed by a sweep"
        );
    }

    // ── Pooled write transactions (cas-3f65e) ───────────────────────

    fn busy_timeout_ms(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap()
    }

    /// The cas-0e57 inversion: an in-process thread holds the SQLite write
    /// lock on its own connection and needs the pooled connection's mutex to
    /// finish, while a pooled writer waits for that write lock. Holding the
    /// mutex through the wait (the unpooled retry used through a guard) left
    /// both stuck for the full ~31.6s budget, then failed the pooled writer.
    #[test]
    fn cas_3f65e_pooled_writer_lets_an_in_process_lock_holder_finish() {
        let temp = TempDir::new().unwrap();
        let (db_path, conn) = contended_db(&temp);
        let shared = Arc::new(Mutex::new(conn));

        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let holder = {
            let shared = Arc::clone(&shared);
            let db_path = db_path.clone();
            std::thread::spawn(move || {
                let own = Connection::open(&db_path).unwrap();
                own.busy_timeout(SQLITE_BUSY_TIMEOUT).unwrap();
                own.execute_batch("BEGIN IMMEDIATE").unwrap();
                own.execute("INSERT INTO t VALUES (99, 'holder')", []).unwrap();
                held_tx.send(()).unwrap();
                // Let the pooled writer start waiting on the write lock.
                std::thread::sleep(Duration::from_millis(150));
                let started = Instant::now();
                let rows: i64 = shared
                    .lock()
                    .unwrap()
                    .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
                    .unwrap();
                let mutex_wait = started.elapsed();
                own.execute_batch("COMMIT").unwrap();
                (rows, mutex_wait)
            })
        };
        held_rx.recv().unwrap();

        let started = Instant::now();
        let tx = begin_immediate_pooled(&shared).expect("the pooled writer must get the lock");
        tx.execute("INSERT INTO t VALUES (2, 'pooled')", []).unwrap();
        tx.commit().unwrap();
        let writer_wait = started.elapsed();

        let (rows_seen, mutex_wait) = holder.join().unwrap();
        assert_eq!(rows_seen, 1, "the holder read through the pooled connection");
        assert!(
            mutex_wait < Duration::from_millis(1_000),
            "the holder waited {mutex_wait:?} for the pooled mutex"
        );
        assert!(
            writer_wait < Duration::from_secs(3),
            "the pooled writer waited {writer_wait:?}"
        );
        let total: i64 = shared
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 3, "seed + holder + pooled");
        // The pool's budget-aware busy handler is back (GH #1165); SQLite
        // reports a custom handler as busy_timeout 0, never the short
        // per-attempt timeout.
        assert_eq!(busy_timeout_ms(&shared.lock().unwrap()), 0);
    }

    /// While a pooled writer waits, other callers keep getting the mutex
    /// between its attempts.
    #[test]
    fn cas_3f65e_pooled_writer_does_not_hold_the_mutex_across_retry_sleeps() {
        let temp = TempDir::new().unwrap();
        let (db_path, conn) = contended_db(&temp);
        let shared = Arc::new(Mutex::new(conn));

        let (ready, holder) = hold_write_lock(db_path, Duration::from_millis(1_200));
        ready.recv().unwrap();

        let writer = {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                let tx = begin_immediate_pooled(&shared)?;
                tx.execute("INSERT INTO t VALUES (2, 'pooled')", [])?;
                tx.commit()?;
                crate::Result::Ok(())
            })
        };
        std::thread::sleep(Duration::from_millis(100));

        let mut slowest = Duration::ZERO;
        for _ in 0..6 {
            let started = Instant::now();
            drop(shared.lock().unwrap());
            slowest = slowest.max(started.elapsed());
            std::thread::sleep(Duration::from_millis(100));
        }
        holder.join().unwrap();
        writer.join().unwrap().expect("the writer commits once the holder does");

        assert!(
            slowest < Duration::from_millis(500),
            "the pooled mutex was held for {slowest:?} while the writer waited"
        );
    }

    /// Fifteen workers' processes writing at once is the fleet-boot shape.
    /// Each simulated process has its own pooled connection; polling with the
    /// mutex released must not starve any of them.
    #[test]
    fn cas_3f65e_fifteen_pooled_writers_all_commit() {
        let temp = TempDir::new().unwrap();
        let (db_path, _conn) = contended_db(&temp);

        let mut handles = Vec::new();
        for worker in 0..15 {
            let path = db_path.clone();
            handles.push(std::thread::spawn(move || {
                let conn = Connection::open(&path).unwrap();
                conn.busy_timeout(SQLITE_BUSY_TIMEOUT).unwrap();
                let shared = Mutex::new(conn);
                let mut slowest = Duration::ZERO;
                for round in 0..5 {
                    let started = Instant::now();
                    with_immediate_write_txn_pooled(&shared, |tx| {
                        let _: i64 = tx.query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))?;
                        tx.execute(
                            "INSERT INTO t (v) VALUES (?1)",
                            rusqlite::params![format!("w{worker}-r{round}")],
                        )?;
                        // A short body, so the lock is genuinely contended.
                        std::thread::sleep(Duration::from_millis(5));
                        Ok(())
                    })
                    .unwrap_or_else(|e| panic!("worker {worker} round {round} failed: {e}"));
                    slowest = slowest.max(started.elapsed());
                }
                slowest
            }));
        }
        let slowest = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .max()
            .unwrap();

        let conn = Connection::open(&db_path).unwrap();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 1 + 15 * 5);
        assert!(slowest < Duration::from_secs(10), "slowest writer waited {slowest:?}");
    }

    /// GH #1165: a thread with a store wait budget gives up at its deadline
    /// on the pooled path too, with a busy-shaped error.
    #[test]
    fn pooled_writer_honours_the_thread_wait_budget() {
        let temp = TempDir::new().unwrap();
        let (db_path, conn) = contended_db(&temp);
        let shared = Mutex::new(conn);
        let (ready, holder) = hold_write_lock(db_path, Duration::from_millis(1_000));
        ready.recv().unwrap();

        let started = Instant::now();
        let error = {
            let _budget = crate::wait_budget::bound_waits_for(Duration::from_millis(40));
            match begin_immediate_pooled(&shared) {
                Ok(_) => panic!("the foreign lock outlives the budget"),
                Err(error) => error,
            }
        };
        let waited = started.elapsed();
        holder.join().unwrap();
        assert!(waited < Duration::from_millis(500), "waited {waited:?}");
        assert!(
            matches!(&error, StoreError::Database(e) if is_busy_error(e)),
            "{error}"
        );
        assert!(shared.try_lock().unwrap().is_autocommit());
    }

    #[test]
    fn cas_3f65e_pooled_budget_exhaustion_fails_with_the_wait_stated() {
        let temp = TempDir::new().unwrap();
        let (db_path, conn) = contended_db(&temp);
        let shared = Mutex::new(conn);

        let (ready, holder) = hold_write_lock(db_path, Duration::from_millis(1_000));
        ready.recv().unwrap();

        let started = Instant::now();
        let error = match begin_immediate_pooled_bounded(
            &shared,
            Duration::from_millis(20),
            Duration::from_millis(200),
        ) {
            Ok(_) => panic!("the foreign lock outlives the budget"),
            Err(error) => error,
        };
        let waited = started.elapsed();
        holder.join().unwrap();

        let message = error.to_string();
        assert!(message.contains("database busy for"), "{message}");
        assert!(message.contains("attempt(s)"), "{message}");
        assert!(waited < Duration::from_millis(800), "waited {waited:?}");
        let conn = shared.try_lock().expect("the mutex is released on failure");
        assert!(conn.is_autocommit(), "no transaction is left open");
        assert_eq!(
            busy_timeout_ms(&conn),
            0,
            "the budget-aware busy handler is restored (GH #1165)"
        );
    }

    #[test]
    fn cas_3f65e_pooled_tx_rolls_back_and_releases_on_drop() {
        let temp = TempDir::new().unwrap();
        let (_db_path, conn) = contended_db(&temp);
        let shared = Mutex::new(conn);

        {
            let tx = begin_immediate_pooled(&shared).unwrap();
            tx.execute("INSERT INTO t VALUES (2, 'discarded')", []).unwrap();
            assert!(shared.try_lock().is_err(), "the tx owns the mutex while open");
        }
        let conn = shared.try_lock().expect("dropping the tx releases the mutex");
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 1, "an uncommitted pooled tx rolls back");

        drop(conn);
        let value = with_immediate_write_txn_pooled(&shared, |tx| {
            tx.execute("INSERT INTO t VALUES (3, 'kept')", [])?;
            Ok(7)
        })
        .unwrap();
        assert_eq!(value, 7);
        let total: i64 = shared
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 2);
    }
}
