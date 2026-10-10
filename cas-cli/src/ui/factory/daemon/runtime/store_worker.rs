//! Store work the factory daemon loop hands off its own thread (GH #1165).
//!
//! The daemon loop forwards keystrokes and renders the TUI. A store call on
//! it could queue behind another process's `task-sync-intents.lock` flock or
//! SQLite write lock for minutes. Three pieces keep the loop moving:
//!
//! - [`StoreWorker`]: one ordered background thread for store writes whose
//!   result the loop does not need on the spot (CI watch results, attention
//!   relays, the lifecycle outbox drain). Jobs run in submission order and
//!   wait out contention there, never on the loop.
//! - [`PASS_STORE_WAIT_BUDGET`]: the loop sets a per-pass deadline with
//!   [`cas_store::wait_budget`]. Store calls that stay on the loop fail fast
//!   past it and are retried on a later pass; panels show the last snapshot.
//! - [`spawn_task_sync_reconcile`]: the pending task-sync repair that every
//!   uncached `open_task_store` used to run, on a background schedule.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Most time one loop pass may spend waiting on store contention: SQLite
/// busy waits, retries, the in-process connection mutex and the intents
/// flock, all together. Past it, waits fail fast.
pub(crate) const PASS_STORE_WAIT_BUDGET: Duration = Duration::from_millis(50);

/// How often the background thread runs the pending task-sync repair.
pub(crate) const TASK_SYNC_RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

type Job = Box<dyn FnOnce() + Send + 'static>;

/// One ordered background thread for daemon store writes.
pub(crate) struct StoreWorker {
    sender: Option<Sender<(Option<&'static str>, Job)>>,
    pending: Arc<AtomicUsize>,
    in_flight: Arc<Mutex<HashSet<&'static str>>>,
    handle: Option<JoinHandle<()>>,
}

impl StoreWorker {
    pub(crate) fn start() -> Self {
        let (sender, receiver) = channel::<(Option<&'static str>, Job)>();
        let pending = Arc::new(AtomicUsize::new(0));
        let in_flight = Arc::new(Mutex::new(HashSet::new()));
        let thread_pending = Arc::clone(&pending);
        let thread_in_flight = Arc::clone(&in_flight);
        let handle = std::thread::Builder::new()
            .name("factory-store-worker".into())
            .spawn(move || {
                for (key, job) in receiver {
                    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                    {
                        tracing::error!(?key, ?panic, "factory store job panicked");
                    }
                    if let Some(key) = key {
                        thread_in_flight
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .remove(key);
                    }
                    thread_pending.fetch_sub(1, Ordering::AcqRel);
                }
            });
        let (sender, handle) = match handle {
            Ok(handle) => (Some(sender), Some(handle)),
            Err(error) => {
                tracing::error!(%error, "could not start the factory store worker");
                (None, None)
            }
        };
        Self {
            sender,
            pending,
            in_flight,
            handle,
        }
    }

    /// Queue `job`. Returns false when the worker is gone. The job never runs
    /// on the caller's thread.
    pub(crate) fn submit(&self, label: &'static str, job: impl FnOnce() + Send + 'static) -> bool {
        self.send(None, label, Box::new(job))
    }

    /// Queue `job` unless a job with the same key is queued or running, so a
    /// periodic job never piles up behind a slow store.
    pub(crate) fn submit_unique(
        &self,
        key: &'static str,
        job: impl FnOnce() + Send + 'static,
    ) -> bool {
        if !self
            .in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key)
        {
            return false;
        }
        let queued = self.send(Some(key), key, Box::new(job));
        if !queued {
            self.in_flight
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(key);
        }
        queued
    }

    fn send(&self, key: Option<&'static str>, label: &'static str, job: Job) -> bool {
        let Some(sender) = self.sender.as_ref() else {
            tracing::warn!(label, "factory store worker unavailable; job dropped");
            return false;
        };
        self.pending.fetch_add(1, Ordering::AcqRel);
        if sender.send((key, job)).is_err() {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            tracing::warn!(label, "factory store worker stopped; job dropped");
            return false;
        }
        true
    }

    /// Jobs queued or running.
    pub(crate) fn pending(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }

    /// Stop accepting jobs and wait up to `timeout` for queued ones to finish.
    /// Used at shutdown, after the loop has stopped.
    pub(crate) fn drain(mut self, timeout: Duration) {
        drop(self.sender.take());
        let started = Instant::now();
        while self.pending() > 0 && started.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.pending() == 0
            && let Some(handle) = self.handle.take()
        {
            let _ = handle.join();
        } else if self.pending() > 0 {
            tracing::warn!(
                pending = self.pending(),
                "factory store worker still busy at shutdown; leaving it to finish"
            );
        }
    }
}

/// Run the pending task-sync repair every [`TASK_SYNC_RECONCILE_INTERVAL`]
/// on its own thread until `shutdown` is set. The daemon's cached task store
/// no longer runs it on open.
pub(crate) fn spawn_task_sync_reconcile(
    cas_dir: PathBuf,
    shutdown: Arc<AtomicBool>,
) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("factory-task-sync-reconcile".into())
        .spawn(move || {
            let mut next = Instant::now();
            while !shutdown.load(Ordering::Relaxed) {
                if Instant::now() >= next {
                    if let Err(error) = crate::store::reconcile_task_sync(&cas_dir) {
                        tracing::debug!(%error, "background task-sync reconcile failed; retrying");
                    }
                    next = Instant::now() + TASK_SYNC_RECONCILE_INTERVAL;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        })
        .map_err(|error| tracing::warn!(%error, "could not start task-sync reconcile thread"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_run_in_order_off_the_caller_thread() {
        let worker = StoreWorker::start();
        let caller = std::thread::current().id();
        let seen = Arc::new(Mutex::new(Vec::new()));
        for n in 0..5 {
            let seen = Arc::clone(&seen);
            assert!(worker.submit("order", move || {
                assert_ne!(std::thread::current().id(), caller);
                seen.lock().unwrap().push(n);
            }));
        }
        worker.drain(Duration::from_secs(5));
        assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn unique_jobs_do_not_pile_up() {
        let worker = StoreWorker::start();
        let (release, gate) = channel::<()>();
        assert!(worker.submit_unique("drain", move || {
            let _ = gate.recv_timeout(Duration::from_secs(5));
        }));
        assert!(
            !worker.submit_unique("drain", || {}),
            "a second drain is refused while the first is queued or running"
        );
        release.send(()).unwrap();
        let started = Instant::now();
        while worker.pending() > 0 && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(worker.submit_unique("drain", || {}));
        worker.drain(Duration::from_secs(5));
    }
}
