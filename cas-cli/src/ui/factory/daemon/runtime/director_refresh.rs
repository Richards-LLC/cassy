//! The factory director's two-second refresh reads, off the loop thread
//! (cas-ee9ab).
//!
//! Measured on a 1.6 GB store at N=16 (cas-98b24): 13.9% of daemon passes
//! took 100 ms or more, on the refresh cadence. Each refresh tick read every
//! task, agent and recent event on the loop thread (the full director
//! snapshot), and the delivery stage read the same snapshot again before it
//! revalidated events. Those reads now run on one background thread with its
//! own SQLite connection. The loop starts a read, keeps passing, and applies
//! the finished snapshot on a later pass; panels are stale by at most one
//! refresh.

use std::time::Duration;

/// Background refresh reads for one daemon loop.
pub(crate) struct DirectorRefresh {
    /// Test hook: sleep this long inside every background read.
    pub(crate) read_delay: Duration,
}

impl DirectorRefresh {
    pub(crate) fn new() -> Self {
        Self {
            read_delay: Duration::ZERO,
        }
    }

    /// Model a slow store: every read sleeps `delay` first.
    #[cfg(test)]
    pub(crate) fn with_read_delay(mut self, delay: Duration) -> Self {
        self.read_delay = delay;
        self
    }
}

#[cfg(test)]
#[path = "director_refresh_tests.rs"]
mod tests;
