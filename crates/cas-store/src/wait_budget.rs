//! Thread-scoped bounds on store waits (GH #1165).
//!
//! The factory daemon renders the TUI and forwards keystrokes on one thread.
//! Before this module, a store call on that thread could wait out a foreign
//! SQLite write lock (5 s busy handler, then up to ~31 s of retries) or a
//! cross-process flock, and the operator's terminal froze for minutes.
//!
//! Two thread-local scopes let such a thread bound or forbid those waits
//! without changing any other thread's behaviour:
//!
//! - [`bound_waits_until`] sets a deadline. Every wait the store layer runs
//!   on this thread checks it: the SQLite busy handler, the
//!   `begin_immediate_with_retry` and `with_write_retry` backoffs, the
//!   in-process connection mutex, and file locks taken through
//!   [`lock_file_exclusive_within_budget`]. Past the deadline they fail fast
//!   with a `SQLITE_BUSY`-shaped error, so callers' existing busy handling
//!   applies and the work is retried on a later pass.
//! - [`forbid_store_access`] marks a region that must not touch the store at
//!   all. A store access inside it is counted (see
//!   [`store_access_violations`]) and logged, so a test can prove a code path
//!   is store-free.
//!
//! Both are plain thread-locals. They only work for code that stays on the
//! thread that set them: a future driven by `Runtime::block_on`, a std thread,
//! or synchronous code. A Tokio task that can migrate between worker threads
//! must not hold a guard across an `.await`.

use std::cell::Cell;
use std::fs::File;
use std::time::{Duration, Instant};

use rusqlite::ffi;

use crate::StoreError;

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    static FORBID_DEPTH: Cell<u32> = const { Cell::new(0) };
    static VIOLATIONS: Cell<u64> = const { Cell::new(0) };
    static LAST_VIOLATION: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// Restores the previous deadline when dropped.
#[must_use = "the wait bound lasts only while the guard is alive"]
pub struct WaitBudgetGuard {
    previous: Option<Instant>,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for WaitBudgetGuard {
    fn drop(&mut self) {
        DEADLINE.with(|cell| cell.set(self.previous));
    }
}

/// Bound every store wait on this thread until `deadline`.
///
/// Nested guards keep the earlier deadline, so an inner scope can tighten the
/// bound but never extend it.
pub fn bound_waits_until(deadline: Instant) -> WaitBudgetGuard {
    let previous = DEADLINE.with(Cell::get);
    let effective = previous.map_or(deadline, |prev| prev.min(deadline));
    DEADLINE.with(|cell| cell.set(Some(effective)));
    WaitBudgetGuard {
        previous,
        _not_send: std::marker::PhantomData,
    }
}

/// [`bound_waits_until`] `now + budget`.
pub fn bound_waits_for(budget: Duration) -> WaitBudgetGuard {
    bound_waits_until(Instant::now() + budget)
}

/// The deadline in force on this thread, if any.
pub fn wait_deadline() -> Option<Instant> {
    DEADLINE.with(Cell::get)
}

/// How long this thread may still wait: `None` when unbounded.
pub fn remaining_wait() -> Option<Duration> {
    wait_deadline().map(|deadline| deadline.saturating_duration_since(Instant::now()))
}

/// True when this thread has a deadline and it has passed.
pub fn wait_budget_exhausted() -> bool {
    remaining_wait().is_some_and(|left| left.is_zero())
}

/// Clamp a planned sleep to the remaining budget. `None` means the budget is
/// spent and the caller must give up instead of sleeping.
pub fn clamp_wait(planned: Duration) -> Option<Duration> {
    match remaining_wait() {
        None => Some(planned),
        Some(left) if left.is_zero() => None,
        Some(left) => Some(planned.min(left)),
    }
}

/// A `SQLITE_BUSY` error that names the spent budget, so callers that already
/// treat busy as "retry later" do so here too.
pub fn budget_exhausted_error(what: &str) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        ffi::Error::new(ffi::SQLITE_BUSY),
        Some(format!(
            "{what}: this thread's store wait budget is spent; retry on a later pass"
        )),
    )
}

/// [`budget_exhausted_error`] as a store error.
pub fn budget_exhausted_store_error(what: &str) -> StoreError {
    StoreError::Database(budget_exhausted_error(what))
}

/// Restores the previous forbid depth when dropped.
#[must_use = "the forbidden region lasts only while the guard is alive"]
pub struct ForbidStoreGuard {
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for ForbidStoreGuard {
    fn drop(&mut self) {
        FORBID_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Mark a region that must not touch the store. Accesses inside it are
/// counted and logged but still run, so production never fails on them.
pub fn forbid_store_access() -> ForbidStoreGuard {
    FORBID_DEPTH.with(|depth| depth.set(depth.get() + 1));
    ForbidStoreGuard {
        _not_send: std::marker::PhantomData,
    }
}

/// Restores the forbid depth that [`permit_store_access`] cleared.
#[must_use = "the permitted region lasts only while the guard is alive"]
pub struct PermitStoreGuard {
    previous_depth: u32,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for PermitStoreGuard {
    fn drop(&mut self) {
        FORBID_DEPTH.with(|depth| depth.set(self.previous_depth));
    }
}

/// Lift [`forbid_store_access`] for a named exception inside a store-free
/// region, such as an operator command that arrives on the input path but is
/// not keystroke forwarding. The wait budget still applies.
pub fn permit_store_access() -> PermitStoreGuard {
    let previous_depth = FORBID_DEPTH.with(|depth| depth.replace(0));
    PermitStoreGuard {
        previous_depth,
        _not_send: std::marker::PhantomData,
    }
}

/// True inside a [`forbid_store_access`] region on this thread.
pub fn store_access_forbidden() -> bool {
    FORBID_DEPTH.with(Cell::get) > 0
}

/// Record a store access. Inside a forbidden region it counts as a violation.
pub fn note_store_access(what: &'static str) {
    if store_access_forbidden() {
        VIOLATIONS.with(|count| count.set(count.get() + 1));
        LAST_VIOLATION.with(|last| last.set(Some(what)));
        tracing::warn!(
            access = what,
            "store access inside a store-free region (GH #1165): the input/draw path must not \
             touch the store"
        );
    }
}

/// Number of forbidden-region store accesses on this thread so far.
pub fn store_access_violations() -> u64 {
    VIOLATIONS.with(Cell::get)
}

/// The most recent forbidden-region access on this thread.
pub fn last_store_access_violation() -> Option<&'static str> {
    LAST_VIOLATION.with(Cell::get)
}

/// Lock an in-process mutex, honouring this thread's wait budget.
///
/// Without a budget this is a plain blocking `lock` (a poisoned mutex is
/// recovered). With one, it polls `try_lock` until the deadline and then
/// returns `None`, so a UI thread never waits on another thread that holds
/// the mutex across its own store wait.
pub fn lock_mutex_within_budget<'a, T>(
    mutex: &'a std::sync::Mutex<T>,
    what: &'static str,
) -> Option<std::sync::MutexGuard<'a, T>> {
    note_store_access(what);
    if wait_deadline().is_none() {
        return Some(
            mutex
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
    }
    let mut pause = Duration::from_micros(200);
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                return Some(poisoned.into_inner());
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        let sleep = clamp_wait(pause)?;
        std::thread::sleep(sleep);
        pause = (pause * 2).min(Duration::from_millis(5));
    }
}

/// Take an exclusive `flock` on `file`, honouring this thread's wait budget.
///
/// Without a budget this is a plain blocking `lock_exclusive`. With one, it
/// polls `try_lock_exclusive` until the deadline and then returns
/// `WouldBlock`, so a UI thread never queues behind another process's lock.
pub fn lock_file_exclusive_within_budget(file: &File, what: &'static str) -> std::io::Result<()> {
    use fs2::FileExt;

    note_store_access(what);
    if wait_deadline().is_none() {
        return file.lock_exclusive();
    }
    let mut pause = Duration::from_millis(1);
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {}
            Err(error) => return Err(error),
        }
        let Some(sleep) = clamp_wait(pause) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!("{what} is held by another process; this thread's wait budget is spent"),
            ));
        };
        std::thread::sleep(sleep);
        pause = (pause * 2).min(Duration::from_millis(10));
    }
}

/// SQLite busy handler for every pooled connection.
///
/// Matches SQLite's own `busy_timeout` schedule up to
/// [`crate::SQLITE_BUSY_TIMEOUT`], and also stops at this thread's deadline.
/// `count` is the number of earlier calls for the same lock attempt.
pub(crate) fn busy_handler(count: i32) -> bool {
    const DELAYS_MS: [u64; 12] = [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100];
    let count = usize::try_from(count).unwrap_or(0);
    let delay = DELAYS_MS[count.min(DELAYS_MS.len() - 1)];
    let waited: u64 = if count < DELAYS_MS.len() {
        DELAYS_MS[..count].iter().sum()
    } else {
        DELAYS_MS.iter().sum::<u64>() + (count - DELAYS_MS.len()) as u64 * 100
    };
    let timeout_ms = crate::SQLITE_BUSY_TIMEOUT.as_millis() as u64;
    if waited >= timeout_ms {
        return false;
    }
    let planned = Duration::from_millis(delay.min(timeout_ms - waited));
    match clamp_wait(planned) {
        Some(sleep) => {
            std::thread::sleep(sleep);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadlines_nest_and_restore() {
        assert!(wait_deadline().is_none());
        let outer_deadline = Instant::now() + Duration::from_secs(10);
        let outer = bound_waits_until(outer_deadline);
        assert_eq!(wait_deadline(), Some(outer_deadline));
        {
            let _inner = bound_waits_until(outer_deadline + Duration::from_secs(5));
            assert_eq!(wait_deadline(), Some(outer_deadline), "inner never extends");
        }
        assert_eq!(wait_deadline(), Some(outer_deadline));
        drop(outer);
        assert!(wait_deadline().is_none());
    }

    #[test]
    fn spent_budget_refuses_to_sleep() {
        let _guard = bound_waits_until(Instant::now());
        assert!(wait_budget_exhausted());
        assert_eq!(clamp_wait(Duration::from_millis(5)), None);
        assert!(!busy_handler(0));
    }

    #[test]
    fn unbounded_busy_handler_gives_up_at_the_busy_timeout() {
        assert!(!busy_handler(i32::MAX));
    }

    #[test]
    fn forbidden_region_counts_accesses() {
        let before = store_access_violations();
        note_store_access("outside");
        assert_eq!(store_access_violations(), before);
        {
            let _forbid = forbid_store_access();
            note_store_access("inside");
        }
        assert_eq!(store_access_violations(), before + 1);
        assert_eq!(last_store_access_violation(), Some("inside"));
    }

    #[test]
    fn permitted_exception_is_not_a_violation() {
        let before = store_access_violations();
        let _forbid = forbid_store_access();
        {
            let _permit = permit_store_access();
            note_store_access("operator command");
        }
        assert_eq!(store_access_violations(), before);
        assert!(store_access_forbidden(), "the forbid scope resumes");
    }

    #[test]
    fn mutex_lock_gives_up_at_the_deadline() {
        let mutex = std::sync::Arc::new(std::sync::Mutex::new(0));
        let held = std::sync::Arc::clone(&mutex);
        let (locked_tx, locked) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _guard = held.lock().unwrap();
            locked_tx.send(()).unwrap();
            let _ = release_rx.recv();
        });
        locked.recv().unwrap();
        let started = Instant::now();
        {
            let _budget = bound_waits_for(Duration::from_millis(30));
            assert!(lock_mutex_within_budget(&mutex, "test mutex").is_none());
        }
        assert!(started.elapsed() < Duration::from_millis(500));
        release.send(()).unwrap();
        holder.join().unwrap();
        let _budget = bound_waits_for(Duration::from_millis(30));
        assert!(lock_mutex_within_budget(&mutex, "test mutex").is_some());
    }

    #[test]
    fn file_lock_gives_up_at_the_deadline() {
        use fs2::FileExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("held.lock");
        let holder = File::create(&path).unwrap();
        holder.lock_exclusive().unwrap();
        let contender = File::open(&path).unwrap();
        let started = Instant::now();
        let error = {
            let _guard = bound_waits_for(Duration::from_millis(30));
            lock_file_exclusive_within_budget(&contender, "test.lock").unwrap_err()
        };
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(started.elapsed() < Duration::from_millis(500));
        holder.unlock().unwrap();
        let _guard = bound_waits_for(Duration::from_millis(30));
        lock_file_exclusive_within_budget(&contender, "test.lock").unwrap();
    }
}
