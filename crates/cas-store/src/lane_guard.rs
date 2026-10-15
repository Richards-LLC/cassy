//! Blocking-call guard for the factory daemon's lanes (factory loop v2 §6.6,
//! cas-6931).
//!
//! The daemon's io and control lanes must never block on a store, a child
//! process, `/proc` or a transcript; that work belongs on the jobs lane. A
//! lane thread marks itself with [`enter_lane`]. Blocking call sites report
//! themselves through [`note_blocking_call`]: store connection acquisition
//! does so from [`crate::wait_budget::note_store_access`], and the daemon's
//! git, `/proc` and transcript helpers call it directly.
//!
//! This is **report mode**: a call on a lane is counted, recorded per call
//! site and logged once per site, and then runs as before. Enforce mode
//! (an assertion in debug and test builds) is switched on by
//! [`ENFORCE_ON_LANE`] once the work has moved off the lanes (C6).
//!
//! The flag lives here because this is the lowest crate shared by the store
//! chokepoints and the daemon. It is a plain thread-local, so it holds only
//! for code that stays on the lane's thread: a future driven by
//! `Runtime::block_on`, or synchronous code. Work handed to another thread is
//! off the lane by construction.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::panic::Location;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// When true, a blocking call on a lane panics in debug and test builds
/// instead of being reported. C6 of factory loop v2 flips this.
pub const ENFORCE_ON_LANE: bool = false;

/// Red-first stub (cas-6931): reporting is not wired yet.
const RED_STUB: bool = true;

/// Most distinct call sites the process-wide report keeps.
const SITES_KEPT: usize = 64;

/// A daemon lane that must not block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lane {
    /// Client input, PTY reads and writes, rendering.
    Io,
    /// Decisions, director state, orchestration.
    Control,
}

impl Lane {
    pub fn name(self) -> &'static str {
        match self {
            Lane::Io => "io",
            Lane::Control => "control",
        }
    }
}

/// The kind of blocking call a site makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BlockingKind {
    /// A store connection open or acquisition.
    Store,
    /// A `std::process::Command` (git, gh, ps).
    Command,
    /// A read under `/proc`.
    Proc,
    /// A transcript read.
    Transcript,
}

impl BlockingKind {
    pub fn name(self) -> &'static str {
        match self {
            BlockingKind::Store => "store",
            BlockingKind::Command => "command",
            BlockingKind::Proc => "proc",
            BlockingKind::Transcript => "transcript",
        }
    }
}

/// One call site that made a blocking call on a lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneViolation {
    pub lane: Lane,
    pub kind: BlockingKind,
    /// What the call was, e.g. `shared_connection` or `git rev-parse`.
    pub what: &'static str,
    /// `file:line` of the reporting call.
    pub site: String,
    /// Calls from this site on a lane so far.
    pub count: u64,
}

impl LaneViolation {
    /// `lane/kind what at file:line (xN)`, the form `loop.json` keeps.
    pub fn describe(&self) -> String {
        format!(
            "{}/{} {} at {} (x{})",
            self.lane.name(),
            self.kind.name(),
            self.what,
            self.site,
            self.count
        )
    }
}

thread_local! {
    static ON_LANE: Cell<Option<Lane>> = const { Cell::new(None) };
    static THREAD_VIOLATIONS: Cell<u64> = const { Cell::new(0) };
    static LAST_THREAD_VIOLATION: Cell<Option<(Lane, BlockingKind, &'static str)>> =
        const { Cell::new(None) };
}

static TOTAL_VIOLATIONS: AtomicU64 = AtomicU64::new(0);

type SiteKey = (Lane, BlockingKind, &'static str, &'static str, u32);

fn sites() -> &'static Mutex<BTreeMap<SiteKey, LaneViolation>> {
    static SITES: std::sync::OnceLock<Mutex<BTreeMap<SiteKey, LaneViolation>>> =
        std::sync::OnceLock::new();
    SITES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Restores the previous lane when dropped.
#[must_use = "the lane lasts only while the guard is alive"]
pub struct LaneGuard {
    previous: Option<Lane>,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for LaneGuard {
    fn drop(&mut self) {
        ON_LANE.with(|lane| lane.set(self.previous));
    }
}

/// Mark this thread as running `lane` until the guard drops.
pub fn enter_lane(lane: Lane) -> LaneGuard {
    let previous = ON_LANE.with(|current| current.replace(Some(lane)));
    LaneGuard {
        previous,
        _not_send: std::marker::PhantomData,
    }
}

/// Switch the lane this thread runs, only when it already runs one. The
/// daemon's single loop runs io steps and control steps in turn.
pub fn switch_lane(lane: Lane) {
    ON_LANE.with(|current| {
        if current.get().is_some() {
            current.set(Some(lane));
        }
    });
}

/// The lane this thread runs, if any.
pub fn current_lane() -> Option<Lane> {
    ON_LANE.with(Cell::get)
}

/// Report a blocking call. On a lane it is counted, recorded against the
/// caller's site and logged once per site; with [`ENFORCE_ON_LANE`] it
/// panics in debug and test builds. Off a lane it costs a thread-local read.
#[track_caller]
pub fn note_blocking_call(kind: BlockingKind, what: &'static str) {
    let Some(lane) = current_lane() else {
        return;
    };
    if RED_STUB {
        return;
    }
    let caller = Location::caller();
    THREAD_VIOLATIONS.with(|count| count.set(count.get() + 1));
    LAST_THREAD_VIOLATION.with(|last| last.set(Some((lane, kind, what))));
    TOTAL_VIOLATIONS.fetch_add(1, Ordering::Relaxed);
    let key = (lane, kind, what, caller.file(), caller.line());
    let first = {
        let mut sites = sites()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match sites.get_mut(&key) {
            Some(entry) => {
                entry.count += 1;
                false
            }
            None => {
                if sites.len() < SITES_KEPT {
                    sites.insert(
                        key,
                        LaneViolation {
                            lane,
                            kind,
                            what,
                            site: format!("{}:{}", caller.file(), caller.line()),
                            count: 1,
                        },
                    );
                }
                true
            }
        }
    };
    if ENFORCE_ON_LANE && cfg!(debug_assertions) {
        panic!(
            "blocking {} call `{what}` on the {} lane at {caller}",
            kind.name(),
            lane.name()
        );
    }
    if first {
        tracing::warn!(
            target: "cas::loop_watchdog",
            lane = lane.name(),
            kind = kind.name(),
            call = what,
            site = %caller,
            "blocking call on a daemon lane (factory loop v2 report mode; logged once per site)"
        );
    }
}

/// Blocking calls reported on a lane by this thread so far.
pub fn thread_violations() -> u64 {
    THREAD_VIOLATIONS.with(Cell::get)
}

/// The most recent blocking call reported on a lane by this thread.
pub fn last_thread_violation() -> Option<(Lane, BlockingKind, &'static str)> {
    LAST_THREAD_VIOLATION.with(Cell::get)
}

/// Blocking calls reported on any lane in this process so far.
pub fn total_violations() -> u64 {
    TOTAL_VIOLATIONS.load(Ordering::Relaxed)
}

/// Every recorded call site (at most [`SITES_KEPT`]), most frequent first.
pub fn violation_sites() -> Vec<LaneViolation> {
    let mut sites: Vec<LaneViolation> = sites()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .cloned()
        .collect();
    sites.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.site.cmp(&b.site)));
    sites
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_lane_calls_are_free_and_lane_calls_are_reported_per_site() {
        let before = thread_violations();
        note_blocking_call(BlockingKind::Command, "off lane");
        assert_eq!(thread_violations(), before);

        {
            let _lane = enter_lane(Lane::Control);
            for _ in 0..3 {
                note_blocking_call(BlockingKind::Proc, "lane guard test site");
            }
            switch_lane(Lane::Io);
            assert_eq!(current_lane(), Some(Lane::Io));
            note_blocking_call(BlockingKind::Transcript, "lane guard io site");
        }
        assert_eq!(current_lane(), None);
        switch_lane(Lane::Io);
        assert_eq!(current_lane(), None, "switch never enters a lane");
        assert_eq!(thread_violations(), before + 4);
        assert_eq!(
            last_thread_violation(),
            Some((Lane::Io, BlockingKind::Transcript, "lane guard io site"))
        );
        let site = violation_sites()
            .into_iter()
            .find(|site| site.what == "lane guard test site")
            .expect("site recorded");
        assert_eq!(site.count, 3);
        assert_eq!(site.lane, Lane::Control);
        assert!(site.site.contains("lane_guard.rs:"), "{site:?}");
        assert!(
            site.describe()
                .starts_with("control/proc lane guard test site at ")
        );
    }
}
