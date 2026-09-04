//! Process-level shared SQLite connection pool.
//!
//! All SQLite stores in a process share ONE connection per database file,
//! dramatically reducing connection count and eliminating intra-process
//! write lock contention when many store types access the same `cas.db`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use rusqlite::Connection;

use crate::{Result, SQLITE_BUSY_TIMEOUT, StoreError};

/// Acquire a shared SQLite connection, converting a poisoned mutex into a
/// recoverable store error instead of panicking the caller.
pub(crate) fn lock_connection(conn: &Arc<Mutex<Connection>>) -> Result<MutexGuard<'_, Connection>> {
    conn.lock()
        .map_err(|_| StoreError::Other("shared SQLite connection lock poisoned".to_string()))
}

/// Process-global pool of shared SQLite connections, keyed by canonical DB path.
///
/// Uses `Weak` references so connections are cleaned up when all stores are dropped.
static POOL: Mutex<Option<HashMap<PathBuf, Weak<Mutex<Connection>>>>> = Mutex::new(None);

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

    let canonical = canonical_db_path(db_path);

    let mut guard = POOL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);

    // Try to upgrade existing weak reference
    if let Some(weak) = map.get(&canonical) {
        if let Some(strong) = weak.upgrade() {
            return Ok(strong);
        }
    }

    // Create new connection with all PRAGMAs
    let conn = Connection::open(db_path)?;
    conn.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
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

    let shared = Arc::new(Mutex::new(conn));
    map.insert(canonical, Arc::downgrade(&shared));
    Ok(shared)
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
                tracing::warn!(
                    base_ms,
                    delay_ms,
                    attempts,
                    "write lock held by another connection, retrying after backoff with jitter"
                );
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            Err(error) => return Err(StoreError::Database(error)),
        }
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
                tracing::warn!(
                    base_ms,
                    delay_ms,
                    "SQLite busy, retrying after backoff with jitter"
                );
                std::thread::sleep(Duration::from_millis(delay_ms));
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

    #[test]
    fn shared_connection_recreates_after_drop() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        let conn1 = shared_connection(&db_path).unwrap();
        let ptr1 = Arc::as_ptr(&conn1);
        drop(conn1);

        let conn2 = shared_connection(&db_path).unwrap();
        let ptr2 = Arc::as_ptr(&conn2);
        assert_ne!(ptr1, ptr2);
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

        // Drop all — next call creates a new connection
        drop(conn2);
        drop(conn3);
        let conn4 = shared_connection(&db_path).unwrap();
        assert_ne!(ptr, Arc::as_ptr(&conn4));
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
            let _map = guard.get_or_insert_with(HashMap::new);
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
}
