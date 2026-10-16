//! Blocking-call guard for the factory daemon's lanes (factory loop v2 §6.6,
//! cas-6931, C6 cas-9db0b).
//!
//! The daemon's io and control lanes must never block on a store, a child
//! process, `/proc` or a transcript; that work belongs on the jobs lane. A
//! lane thread marks itself with [`enter_lane`] or [`enforce_lane`]. Blocking
//! call sites report themselves through [`note_blocking_call`]: store
//! connection acquisition does so from
//! [`crate::wait_budget::note_store_access`], and the daemon's git, `/proc`
//! and transcript helpers call it directly.
//!
//! A lane runs in one of two [`GuardMode`]s:
//!
//! - **Report** ([`enter_lane`]; the daemon loop's attachment): the call is
//!   counted, recorded per call site with its step and logged once per site,
//!   and then runs as before. The daemon loop reports until its residual
//!   loop I/O has moved to the jobs lane (factory loop v2 C2c, cas-4491f).
//! - **Enforce** ([`enforce_lane`], or [`enforce_here`] for a scope of the
//!   current lane; factory loop v2 C6, cas-9db0b): the call is recorded the
//!   same way. Under a test harness (cas-store's own `cfg(test)`, or a
//!   process that called [`panic_on_enforced_violations_for_tests`]) it then panics,
//!   naming the call, lane, step and site. Anywhere else, debug builds
//!   included, it logs one error per site with the step and runs: a debug
//!   daemon that supervisors and QA run for real must never lose its worker
//!   panes to a guard.
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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

/// What a blocking call on a lane does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardMode {
    /// Count, record and log the call once per site, then run it.
    Report,
    /// Panic under a test harness; anywhere else record the call, log one
    /// error per site and run it.
    Enforce,
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
    /// The lane step the call ran in, when the lane names its steps
    /// ([`set_step`]).
    pub step: Option<&'static str>,
    /// Calls from this site on a lane so far.
    pub count: u64,
}

impl LaneViolation {
    /// `lane/kind what at file:line in step (xN)`, the form `loop.json`
    /// keeps; ` in step` is left out when no step was named.
    pub fn describe(&self) -> String {
        let step = self
            .step
            .map(|step| format!(" in {step}"))
            .unwrap_or_default();
        format!(
            "{}/{} {} at {}{step} (x{})",
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
    static MODE: Cell<GuardMode> = const { Cell::new(GuardMode::Report) };
    static STEP: Cell<Option<&'static str>> = const { Cell::new(None) };
    static THREAD_VIOLATIONS: Cell<u64> = const { Cell::new(0) };
    static LAST_THREAD_VIOLATION: Cell<Option<(Lane, BlockingKind, &'static str)>> =
        const { Cell::new(None) };
}

static TOTAL_VIOLATIONS: AtomicU64 = AtomicU64::new(0);
static OVERFLOWED: AtomicU64 = AtomicU64::new(0);
static OVERFLOW_LOGGED: AtomicBool = AtomicBool::new(false);

/// Whether an enforced violation panics: cas-store's own tests, or a test
/// harness that asked for it ([`panic_on_enforced_violations_for_tests`]).
static PANIC_ON_ENFORCED: AtomicBool = AtomicBool::new(cfg!(test));

/// The explicit test-harness switch: from now on an enforced violation in
/// this process panics instead of logging. Test code calls this (cas-cli
/// through `loop_watchdog::enforce_test_lane`); production code never does.
#[doc(hidden)]
pub fn panic_on_enforced_violations_for_tests() {
    PANIC_ON_ENFORCED.store(true, Ordering::Relaxed);
}

#[cfg(test)]
thread_local! {
    /// This crate's tests of the non-test path: enforce without panicking.
    static TEST_LOG_ONLY: Cell<bool> = const { Cell::new(false) };
}

/// True when an enforced violation panics in this process.
pub fn enforced_violations_panic() -> bool {
    #[cfg(test)]
    if TEST_LOG_ONLY.with(Cell::get) {
        return false;
    }
    PANIC_ON_ENFORCED.load(Ordering::Relaxed)
}

/// A site is a call site on a lane in a step: the same call from another
/// step is another site, so the report attributes each call to its step.
type SiteKey = (
    Lane,
    BlockingKind,
    &'static str,
    &'static str,
    u32,
    Option<&'static str>,
);

fn sites() -> &'static Mutex<BTreeMap<SiteKey, LaneViolation>> {
    static SITES: std::sync::OnceLock<Mutex<BTreeMap<SiteKey, LaneViolation>>> =
        std::sync::OnceLock::new();
    SITES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Restores the previous lane, mode and step when dropped.
#[must_use = "the lane lasts only while the guard is alive"]
pub struct LaneGuard {
    previous: Option<Lane>,
    previous_mode: GuardMode,
    previous_step: Option<&'static str>,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for LaneGuard {
    fn drop(&mut self) {
        ON_LANE.with(|lane| lane.set(self.previous));
        MODE.with(|mode| mode.set(self.previous_mode));
        STEP.with(|step| step.set(self.previous_step));
    }
}

fn enter(lane: Lane, mode: GuardMode) -> LaneGuard {
    LaneGuard {
        previous: ON_LANE.with(|current| current.replace(Some(lane))),
        previous_mode: MODE.with(|current| current.replace(mode)),
        previous_step: STEP.with(|current| current.replace(None)),
        _not_send: std::marker::PhantomData,
    }
}

/// Mark this thread as running `lane` in report mode until the guard drops.
/// The daemon loop attaches its thread this way until its residual loop I/O
/// has moved to the jobs lane (C2c, cas-4491f).
pub fn enter_lane(lane: Lane) -> LaneGuard {
    enter(lane, GuardMode::Report)
}

/// Mark this thread as running `lane` in enforce mode until the guard drops
/// (factory loop v2 C6). See the module docs for what enforce does where.
pub fn enforce_lane(lane: Lane) -> LaneGuard {
    enter(lane, GuardMode::Enforce)
}

/// Restores the mode [`enforce_here`] or [`report_here`] replaced when
/// dropped.
#[must_use = "the mode lasts only while the guard is alive"]
pub struct ModeGuard {
    previous: Option<GuardMode>,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for ModeGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous {
            MODE.with(|mode| mode.set(previous));
        }
    }
}

fn set_mode_here(mode: GuardMode) -> ModeGuard {
    let previous = current_lane().map(|_| MODE.with(|current| current.replace(mode)));
    ModeGuard {
        previous,
        _not_send: std::marker::PhantomData,
    }
}

/// Enforce on the lane this thread already runs until the guard drops, for
/// steps proven clean inside a reporting lane (the daemon loop's client
/// input, PTY exchange and inject/queue pass). A no-op off a lane.
pub fn enforce_here() -> ModeGuard {
    set_mode_here(GuardMode::Enforce)
}

/// Report on the lane this thread already runs until the guard drops: a
/// named exception inside an enforced step, such as
/// [`crate::wait_budget::permit_store_access`]. A no-op off a lane.
pub fn report_here() -> ModeGuard {
    set_mode_here(GuardMode::Report)
}

/// Name the step this lane runs, for the violation report and the enforce
/// panic. A no-op off a lane.
pub fn set_step(step: &'static str) {
    if current_lane().is_some() {
        STEP.with(|current| current.set(Some(step)));
    }
}

/// The step this lane runs, if it named one.
pub fn current_step() -> Option<&'static str> {
    current_lane().and_then(|_| STEP.with(Cell::get))
}

/// The guard mode of this thread's lane, if it runs one.
pub fn current_mode() -> Option<GuardMode> {
    current_lane().map(|_| MODE.with(Cell::get))
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

/// Report a blocking call. On a lane it is counted and recorded against the
/// caller's site and step. In enforce mode under a test harness it then
/// panics; otherwise it is logged once per site (an error in enforce mode, a
/// warning in report mode) and runs. Off a lane it costs a thread-local read.
#[track_caller]
pub fn note_blocking_call(kind: BlockingKind, what: &'static str) {
    let Some(lane) = current_lane() else {
        return;
    };
    let caller = Location::caller();
    let step = STEP.with(Cell::get);
    THREAD_VIOLATIONS.with(|count| count.set(count.get() + 1));
    LAST_THREAD_VIOLATION.with(|last| last.set(Some((lane, kind, what))));
    TOTAL_VIOLATIONS.fetch_add(1, Ordering::Relaxed);
    let key = (lane, kind, what, caller.file(), caller.line(), step);
    let recorded = {
        let mut sites = sites()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        record_site(&mut sites, key, || LaneViolation {
            lane,
            kind,
            what,
            site: format!("{}:{}", caller.file(), caller.line()),
            step,
            count: 1,
        })
    };
    if recorded == SiteRecord::Overflow {
        OVERFLOWED.fetch_add(1, Ordering::Relaxed);
        if !OVERFLOW_LOGGED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                target: "cas::loop_watchdog",
                kept = SITES_KEPT,
                "the lane violation report is full; calls from further new sites are counted \
                 (lane_guard::overflowed_violations) but not recorded or logged (logged once)"
            );
        }
    }
    let first = recorded == SiteRecord::First;
    let enforced = MODE.with(Cell::get) == GuardMode::Enforce;
    if enforced && enforced_violations_panic() {
        panic!(
            "blocking {} call `{what}` on the {} lane{} at {caller} (factory loop v2 §6.6 \
             enforce mode: move it to the jobs lane)",
            kind.name(),
            lane.name(),
            step.map(|step| format!(" in step `{step}`"))
                .unwrap_or_default(),
        );
    }
    if first && enforced {
        tracing::error!(
            target: "cas::loop_watchdog",
            lane = lane.name(),
            kind = kind.name(),
            call = what,
            site = %caller,
            step = step.unwrap_or(""),
            "blocking call on an enforced daemon lane (factory loop v2 §6.6; logged once per site)"
        );
    } else if first {
        tracing::warn!(
            target: "cas::loop_watchdog",
            lane = lane.name(),
            kind = kind.name(),
            call = what,
            site = %caller,
            step = step.unwrap_or(""),
            "blocking call on a daemon lane (factory loop v2 report mode; logged once per site)"
        );
    }
}

/// What recording one call did to the site report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SiteRecord {
    /// A new site, now recorded: log it.
    First,
    /// A recorded site, counted again.
    Repeat,
    /// A new site with the report full: counted in the overflow only, never
    /// logged per call (cas-9db0b review M1).
    Overflow,
}

fn record_site(
    sites: &mut BTreeMap<SiteKey, LaneViolation>,
    key: SiteKey,
    violation: impl FnOnce() -> LaneViolation,
) -> SiteRecord {
    if let Some(entry) = sites.get_mut(&key) {
        entry.count += 1;
        return SiteRecord::Repeat;
    }
    if sites.len() >= SITES_KEPT {
        return SiteRecord::Overflow;
    }
    sites.insert(key, violation());
    SiteRecord::First
}

/// Calls from new sites the full report could not keep, in this process.
pub fn overflowed_violations() -> u64 {
    OVERFLOWED.load(Ordering::Relaxed)
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
            assert_eq!(current_mode(), Some(GuardMode::Report));
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

    fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_default()
    }

    /// Factory loop v2 C6 (cas-9db0b): a lane entered with
    /// [`enforce_lane`] enforces. Under a test harness each blocking kind
    /// panics, naming the call, lane, step and site; the guard restores the
    /// thread when the panic unwinds; off a lane nothing is reported.
    #[test]
    fn flv2_enforce_mode_panics_on_each_blocking_kind_with_step_and_site() {
        assert!(enforced_violations_panic(), "cas-store's own tests panic");
        assert_eq!(current_mode(), None);
        let before = thread_violations();
        for kind in [
            BlockingKind::Store,
            BlockingKind::Command,
            BlockingKind::Proc,
            BlockingKind::Transcript,
        ] {
            note_blocking_call(kind, "off lane enforce site");
            let caught = std::panic::catch_unwind(|| {
                let _lane = enforce_lane(Lane::Io);
                assert_eq!(current_mode(), Some(GuardMode::Enforce));
                set_step("client input");
                note_blocking_call(kind, "enforce test site");
            });
            let message = panic_message(caught.expect_err("enforce mode panics"));
            assert!(
                message.starts_with(&format!(
                    "blocking {} call `enforce test site` on the io lane in step `client input` at ",
                    kind.name()
                )),
                "{message}"
            );
            assert!(message.contains("lane_guard.rs:"), "{message}");
            assert_eq!(current_lane(), None, "the guard unwinds with the panic");
            assert_eq!(current_mode(), None);
            assert_eq!(current_step(), None);
        }
        assert_eq!(thread_violations(), before + 4, "only the lane calls count");
        let site = violation_sites()
            .into_iter()
            .find(|site| site.what == "enforce test site")
            .expect("an enforced call is recorded before it panics");
        assert_eq!(site.step, Some("client input"));
        assert!(
            site.describe().contains(" in client input (x"),
            "{}",
            site.describe()
        );
    }

    /// Outside a test harness (a debug daemon run for QA, a release build)
    /// an enforced violation is recorded with its step and logged once as an
    /// error, and the call runs: it never panics.
    #[test]
    fn flv2_enforce_outside_a_test_harness_records_and_logs_without_panicking() {
        TEST_LOG_ONLY.with(|only| only.set(true));
        assert!(!enforced_violations_panic());
        let before = thread_violations();
        {
            let _lane = enforce_lane(Lane::Control);
            set_step("director apply");
            for _ in 0..2 {
                note_blocking_call(BlockingKind::Store, "enforced log-only site");
            }
        }
        TEST_LOG_ONLY.with(|only| only.set(false));
        assert_eq!(thread_violations(), before + 2);
        let site = violation_sites()
            .into_iter()
            .find(|site| site.what == "enforced log-only site")
            .expect("recorded");
        assert_eq!((site.count, site.step), (2, Some("director apply")));
    }

    /// Review M1: with the report full, a new site is an overflow. It is
    /// counted, never recorded and never "first", so it is not logged on
    /// every call; known sites keep counting.
    #[test]
    fn flv2_full_site_report_counts_overflow_without_logging_each_call() {
        let mut sites = BTreeMap::new();
        let key = |line: u32| -> SiteKey {
            (Lane::Io, BlockingKind::Proc, "cap test", "lane_guard.rs", line, Some("step"))
        };
        let violation = || LaneViolation {
            lane: Lane::Io,
            kind: BlockingKind::Proc,
            what: "cap test",
            site: "lane_guard.rs:1".to_string(),
            step: Some("step"),
            count: 1,
        };
        for line in 0..SITES_KEPT as u32 {
            assert_eq!(record_site(&mut sites, key(line), violation), SiteRecord::First);
        }
        for _ in 0..3 {
            assert_eq!(
                record_site(&mut sites, key(SITES_KEPT as u32), violation),
                SiteRecord::Overflow
            );
        }
        assert_eq!(record_site(&mut sites, key(0), violation), SiteRecord::Repeat);
        assert_eq!(sites.len(), SITES_KEPT);
        assert_eq!(sites[&key(0)].count, 2);
    }

    /// A report lane inside an enforcing one restores the outer lane's mode
    /// and step when it drops; [`enforce_here`] enforces a scope of a
    /// reporting lane and is a no-op off a lane.
    #[test]
    fn flv2_nested_lanes_restore_mode_and_step() {
        {
            let _off = enforce_here();
            assert_eq!(current_mode(), None);
        }
        let _outer = enforce_lane(Lane::Control);
        set_step("prompt queue");
        {
            let _inner = enter_lane(Lane::Io);
            assert_eq!(current_mode(), Some(GuardMode::Report));
            assert_eq!(current_step(), None, "a new lane starts unnamed");
            note_blocking_call(BlockingKind::Store, "nested report site");
            switch_lane(Lane::Control);
            assert_eq!(current_mode(), Some(GuardMode::Report), "switching keeps the mode");
            {
                let _clean = enforce_here();
                assert_eq!(current_mode(), Some(GuardMode::Enforce));
                {
                    let _exception = report_here();
                    assert_eq!(current_mode(), Some(GuardMode::Report));
                    note_blocking_call(BlockingKind::Store, "named exception site");
                }
                assert_eq!(current_mode(), Some(GuardMode::Enforce));
            }
            assert_eq!(current_mode(), Some(GuardMode::Report));
        }
        assert_eq!(current_lane(), Some(Lane::Control));
        assert_eq!(current_mode(), Some(GuardMode::Enforce));
        assert_eq!(current_step(), Some("prompt queue"));
        drop(_outer);
        set_step("off lane");
        assert_eq!(current_step(), None, "set_step is a no-op off a lane");
    }
}
