//! Daemon loop progress and its watchdog thread (cas-73b5, GH #970).
//!
//! The daemon loop records which step it is in and when it last completed a
//! pass. That costs a few atomic stores per pass. A plain OS thread publishes
//! the record through [`crate::factory_daemon_health`] every few seconds, so
//! `worker_status` still sees the loop when a pass blocks. The thread is not a
//! task on the loop's runtime, which a blocking call inside the loop would
//! also stall.
//!
//! While a pass is wedged and the supervisor has asked for a spawn-queue
//! restart, the watchdog also kills hung git/gh/ssh helper processes the
//! daemon started. Those are the blocking calls a pass can sit in. Harness
//! panes (the supervisor and workers) are never candidates.
//!
//! Factory loop v2 §6 (cas-6931) makes the watchdog name the step that is
//! slow. Each lane marks the [`Step`] it runs, including the prompt queue's
//! sub-steps ([`mark`]) and the director apply's `timed()` steps
//! ([`scoped`]). A step over its budget lands in a 32-event ring, a
//! `cas::loop_watchdog` WARN and the TUI status line, which holds the worst
//! one for 10 s (`slow: <lane>/<step> <n> ms`). The watchdog checks the
//! lanes every 250 ms; a pass that has not finished in 1 s, or a job pending
//! past its budget, is a wedge (lane, step, duration), written to
//! `loop.json` at once with the thread's wait channel. The TUI shows only
//! job wedges live (`wedged: jobs/<kind> for <n> s`); see
//! [`LoopProgress::status_line`] for why a loop wedge cannot be drawn yet.
//! An attached lane thread also reports blocking calls through
//! [`cas_store::lane_guard`] (report mode).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::factory_daemon_health::{
    DaemonLoopStatus, LANE_HEARTBEAT_MS, LANE_WEDGE_MS, LOOP_WEDGED_SECS, LaneWedge, SLOW_RING_CAP,
    SLOW_STATUS_SECS, SlowStepEvent, WATCHDOG_INTERVAL_SECS, pending_reset, status_path,
    write_status,
};

/// A helper must have run at least this long before a restart may kill it.
const HELPER_MIN_AGE_SECS: u64 = 30;
/// Helper commands the daemon runs in-line that can block a pass.
const HELPER_COMMANDS: &[&str] = &["git", "gh", "ssh", "git-remote-https", "git-remote-http"];
/// How many killed helpers the status file keeps.
const KILLED_HELPERS_KEPT: usize = 10;

/// Step budgets (factory loop v2 §6.2, cas-6931): an io step, a prompt-queue
/// decision step and every other control step.
pub(crate) const IO_STEP_BUDGET_MS: u64 = 5;
pub(crate) const PROMPT_QUEUE_STEP_BUDGET_MS: u64 = 20;
pub(crate) const CONTROL_STEP_BUDGET_MS: u64 = 10;
/// An over-budget step is logged at most this often per step name; the ring
/// still records every event.
const SLOW_LOG_INTERVAL: Duration = Duration::from_secs(1);
/// The status-line badge changes at most this often (cas-6931 review): a
/// redraw can itself be a slow io step, so a badge that followed every event
/// would feed itself.
const BADGE_MIN_INTERVAL: Duration = Duration::from_secs(1);

/// A lane of the daemon (factory loop v2 §3). Today one loop thread runs the
/// io and control steps in turn; jobs run on the store worker and pools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Lane {
    Io,
    Control,
    Jobs,
}

impl Lane {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Lane::Io => "io",
            Lane::Control => "control",
            Lane::Jobs => "jobs",
        }
    }

    /// The blocking-call guard's lane; the jobs lane may block.
    fn guarded(self) -> Option<cas_store::lane_guard::Lane> {
        match self {
            Lane::Io => Some(cas_store::lane_guard::Lane::Io),
            Lane::Control => Some(cas_store::lane_guard::Lane::Control),
            Lane::Jobs => None,
        }
    }
}

/// A named step of a lane and its budget. `None` is never timed (the idle
/// sleep between passes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Step {
    pub(crate) lane: Lane,
    pub(crate) name: &'static str,
    pub(crate) budget_ms: Option<u64>,
}

impl Step {
    pub(crate) const fn io(name: &'static str) -> Self {
        Self {
            lane: Lane::Io,
            name,
            budget_ms: Some(IO_STEP_BUDGET_MS),
        }
    }

    pub(crate) const fn control(name: &'static str) -> Self {
        Self {
            lane: Lane::Control,
            name,
            budget_ms: Some(CONTROL_STEP_BUDGET_MS),
        }
    }

    /// A step of the prompt-queue pass.
    pub(crate) const fn prompt_queue(name: &'static str) -> Self {
        Self {
            lane: Lane::Control,
            name,
            budget_ms: Some(PROMPT_QUEUE_STEP_BUDGET_MS),
        }
    }

    const fn idle() -> Self {
        Self::untimed("idle")
    }

    const fn untimed(name: &'static str) -> Self {
        Self {
            lane: Lane::Control,
            name,
            budget_ms: None,
        }
    }
}

/// The loop step running now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum LoopPhase {
    Start = 0,
    ClientInput,
    PtyOutput,
    Relay,
    PromptQueue,
    SpawnQueue,
    PendingSpawns,
    Refresh,
    Render,
    Idle,
    // cas-ee9ab: the refresh tick's steps, so a slow tick names its cause.
    RefreshSpawnVerify,
    RefreshDirector,
    RefreshDelivery,
    RefreshRelays,
    RefreshFollowUp,
    RefreshInject,
}

impl LoopPhase {
    pub(crate) fn name(self) -> &'static str {
        match self {
            LoopPhase::Start => "start",
            LoopPhase::ClientInput => "client input",
            LoopPhase::PtyOutput => "pty output",
            LoopPhase::Relay => "cloud relay",
            LoopPhase::PromptQueue => "prompt queue",
            LoopPhase::SpawnQueue => "spawn queue poll",
            LoopPhase::PendingSpawns => "pending spawns",
            LoopPhase::Refresh => "refresh",
            LoopPhase::Render => "render",
            LoopPhase::Idle => "idle",
            LoopPhase::RefreshSpawnVerify => "refresh: spawn verify",
            LoopPhase::RefreshDirector => "refresh: director data",
            LoopPhase::RefreshDelivery => "refresh: delivery revalidation",
            LoopPhase::RefreshRelays => "refresh: relays",
            LoopPhase::RefreshFollowUp => "refresh: follow-up",
            LoopPhase::RefreshInject => "refresh: prompt injection",
        }
    }

    /// The phase as a watchdog step: input, PTY output and render are io
    /// steps; the prompt queue has the decision budget (cas-6931).
    pub(crate) fn step(self) -> Step {
        match self {
            LoopPhase::Idle => Step::idle(),
            LoopPhase::Start => Step::untimed(self.name()),
            LoopPhase::ClientInput | LoopPhase::PtyOutput | LoopPhase::Render => {
                Step::io(self.name())
            }
            LoopPhase::PromptQueue => Step::prompt_queue(self.name()),
            _ => Step::control(self.name()),
        }
    }
}

/// The watchdog's time source; tests drive a fake one.
#[derive(Debug, Clone)]
pub(crate) enum LoopClock {
    Real,
    #[cfg_attr(not(test), allow(dead_code))]
    Fake(Arc<FakeClock>),
}

impl LoopClock {
    fn now(&self) -> Instant {
        match self {
            LoopClock::Real => Instant::now(),
            LoopClock::Fake(clock) => clock.now(),
        }
    }
}

/// A clock that moves only when told to.
#[derive(Debug)]
pub(crate) struct FakeClock {
    base: Instant,
    offset_ms: AtomicU64,
}

#[cfg_attr(not(test), allow(dead_code))]
impl FakeClock {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            base: Instant::now(),
            offset_ms: AtomicU64::new(0),
        })
    }

    pub(crate) fn advance(&self, by: Duration) {
        self.offset_ms
            .fetch_add(by.as_millis() as u64, Ordering::Relaxed);
    }

    fn now(&self) -> Instant {
        self.base + Duration::from_millis(self.offset_ms.load(Ordering::Relaxed))
    }
}

/// Jobs queued or running off the lanes, by kind, so the watchdog can name
/// the oldest (factory loop v2 §6.5). A [`JobTicket`] is taken when a job is
/// submitted and dropped when it finishes.
#[derive(Debug, Default)]
pub(crate) struct JobHealth {
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, PendingJob>>,
}

#[derive(Debug, Clone, Copy)]
struct PendingJob {
    kind: &'static str,
    budget: Duration,
    since: Instant,
}

/// The oldest pending job of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OldestJob {
    pub(crate) age: Duration,
    pub(crate) budget: Duration,
}

/// One pending job; dropping it marks the job finished.
#[derive(Debug)]
pub(crate) struct JobTicket {
    health: Arc<JobHealth>,
    id: u64,
}

impl Drop for JobTicket {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.health.pending.lock() {
            pending.remove(&self.id);
        }
    }
}

impl JobHealth {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The process's job registry, which the daemon's watchdog reads.
    pub(crate) fn global() -> Arc<Self> {
        static GLOBAL: std::sync::OnceLock<Arc<JobHealth>> = std::sync::OnceLock::new();
        Arc::clone(GLOBAL.get_or_init(|| Arc::new(JobHealth::default())))
    }

    /// Record a job of `kind` submitted now with its §4.4 `budget`.
    pub(crate) fn begin(self: &Arc<Self>, kind: &'static str, budget: Duration) -> JobTicket {
        self.begin_at(kind, budget, Instant::now())
    }

    pub(crate) fn begin_at(
        self: &Arc<Self>,
        kind: &'static str,
        budget: Duration,
        since: Instant,
    ) -> JobTicket {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(
                id,
                PendingJob {
                    kind,
                    budget,
                    since,
                },
            );
        }
        JobTicket {
            health: Arc::clone(self),
            id,
        }
    }

    /// The oldest pending job of each kind at `now`.
    pub(crate) fn oldest(&self, now: Instant) -> BTreeMap<&'static str, OldestJob> {
        let mut oldest: BTreeMap<&'static str, OldestJob> = BTreeMap::new();
        let Ok(pending) = self.pending.lock() else {
            return oldest;
        };
        for job in pending.values() {
            let age = now.saturating_duration_since(job.since);
            let entry = oldest.entry(job.kind).or_insert(OldestJob {
                age,
                budget: job.budget,
            });
            if age > entry.age {
                *entry = OldestJob {
                    age,
                    budget: job.budget,
                };
            }
        }
        oldest
    }
}

#[derive(Debug, Default)]
struct SpawnSnapshot {
    pending_spawns: usize,
    in_flight: Option<(String, DateTime<Utc>)>,
    last_reset: Option<String>,
}

/// The step a lane runs now and when its current segment began.
#[derive(Debug, Clone, Copy)]
struct CurrentStep {
    step: Step,
    started: Instant,
}

/// Over-budget steps: the last [`SLOW_RING_CAP`] events, the worst one in
/// the status-line window, and the per-step log throttle.
#[derive(Debug, Default)]
struct SlowSteps {
    ring: VecDeque<SlowStepEvent>,
    /// The worst event since it was taken, and when; a later event replaces
    /// it only when it is larger or the held one has expired.
    held: Option<(SlowStepEvent, Instant)>,
    logged: HashMap<&'static str, (Instant, u64)>,
}

/// The badge text the TUI shows and when it last changed. Each change marks
/// a frame dirty, so it changes at most once per [`BADGE_MIN_INTERVAL`].
#[derive(Debug, Default)]
struct Badge {
    shown: Option<String>,
    changed_at: Option<Instant>,
}

/// A wedge the watchdog has detected and not yet seen clear.
#[derive(Debug, Clone)]
struct WedgeState {
    lane: Lane,
    step: &'static str,
    since: Instant,
    detected_at: DateTime<Utc>,
}

/// A wedge found by [`LoopProgress::check_wedge`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WedgeCheck {
    pub(crate) lane: Lane,
    pub(crate) step: &'static str,
    pub(crate) duration: Duration,
    /// True the first time this lane/step wedge is seen.
    pub(crate) new: bool,
}

/// Progress the loop records and the watchdog publishes.
#[derive(Debug)]
pub(crate) struct LoopProgress {
    clock: LoopClock,
    jobs: Arc<JobHealth>,
    last_pass_ms: AtomicI64,
    passes: AtomicU64,
    loop_tid: AtomicI64,
    spawn: Mutex<SpawnSnapshot>,
    /// When the running pass began, and the pass durations since the last
    /// status snapshot (GH #1165).
    pass_started: Mutex<Option<Instant>>,
    window: Mutex<Vec<u64>>,
    /// The step marker (cas-6931): the step running now and when its segment
    /// began. Per-step latency since the last status snapshot (cas-ee9ab).
    /// Idle time is never timed.
    current: Mutex<CurrentStep>,
    phase_window:
        Mutex<std::collections::BTreeMap<&'static str, crate::factory_daemon_health::PhaseLatency>>,
    slow: Mutex<SlowSteps>,
    badge: Mutex<Badge>,
    wedge: Mutex<Option<WedgeState>>,
}

/// Most pass durations one snapshot window keeps; older ones are dropped
/// from the percentile but still counted.
const PASS_WINDOW_CAP: usize = 8192;
/// The pass latency #1165 sets as the loop's bound.
const SLOW_PASS_MS: u64 = 100;

/// Summary of one window of pass durations.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PassWindow {
    pub passes: u64,
    pub max_ms: Option<u64>,
    pub p99_ms: Option<u64>,
    pub over_100ms: u64,
}

impl PassWindow {
    fn from_durations(mut durations: Vec<u64>, passes: u64) -> Self {
        if durations.is_empty() {
            return Self {
                passes,
                ..Self::default()
            };
        }
        durations.sort_unstable();
        let rank = ((durations.len() * 99).div_ceil(100)).max(1) - 1;
        Self {
            passes,
            max_ms: durations.last().copied(),
            p99_ms: Some(durations[rank]),
            over_100ms: durations.iter().filter(|ms| **ms >= SLOW_PASS_MS).count() as u64,
        }
    }
}

thread_local! {
    /// The progress a lane thread reports its sub-steps to (cas-6931).
    static ATTACHED: RefCell<Option<Arc<LoopProgress>>> = const { RefCell::new(None) };
}

/// A lane thread attached to its progress; dropping it detaches.
#[must_use = "the lane lasts only while the attachment is alive"]
pub(crate) struct LaneAttachment {
    previous: Option<Arc<LoopProgress>>,
    _lane: Option<cas_store::lane_guard::LaneGuard>,
}

impl Drop for LaneAttachment {
    fn drop(&mut self) {
        let previous = self.previous.take();
        ATTACHED.with(|attached| *attached.borrow_mut() = previous);
    }
}

fn attached() -> Option<Arc<LoopProgress>> {
    ATTACHED.with(|attached| attached.borrow().clone())
}

/// Mark the attached lane as entering `step`. A no-op off a lane thread.
pub(crate) fn mark(step: Step) {
    if let Some(progress) = attached() {
        progress.enter_step(step);
    }
}

/// Run `step` until the returned scope drops, then return to the step that
/// was running. A no-op off a lane thread.
///
/// Timing is per segment: entering the scope closes the enclosing step's
/// current segment, and leaving it starts a fresh one. The enclosing step's
/// budget check, `phase_latency` entry count and max therefore apply to each
/// segment and exclude the nested step's time, which is charged to the
/// nested step alone. A step split by nested scopes can spend more than its
/// budget in total without being reported, as long as no single segment
/// does.
pub(crate) fn scoped(step: Step) -> Option<StepScope> {
    let progress = attached()?;
    let previous = progress.current_step();
    progress.enter_step(step);
    Some(StepScope { progress, previous })
}

/// Returns its lane to the enclosing step when dropped.
pub(crate) struct StepScope {
    progress: Arc<LoopProgress>,
    previous: Step,
}

impl Drop for StepScope {
    fn drop(&mut self) {
        self.progress.enter_step(self.previous);
    }
}

impl LoopProgress {
    pub(crate) fn new() -> Arc<Self> {
        Self::with_clock(LoopClock::Real, JobHealth::global())
    }

    pub(crate) fn with_clock(clock: LoopClock, jobs: Arc<JobHealth>) -> Arc<Self> {
        let now = clock.now();
        Arc::new(Self {
            clock,
            jobs,
            last_pass_ms: AtomicI64::new(Utc::now().timestamp_millis()),
            passes: AtomicU64::new(0),
            loop_tid: AtomicI64::new(current_tid()),
            spawn: Mutex::new(SpawnSnapshot::default()),
            pass_started: Mutex::new(None),
            window: Mutex::new(Vec::new()),
            current: Mutex::new(CurrentStep {
                step: LoopPhase::Start.step(),
                started: now,
            }),
            phase_window: Mutex::new(std::collections::BTreeMap::new()),
            slow: Mutex::new(SlowSteps::default()),
            badge: Mutex::new(Badge::default()),
            wedge: Mutex::new(None),
        })
    }

    /// Mark this thread as `lane`, so sub-steps marked with [`mark`] and
    /// [`scoped`] reach this progress and blocking calls are reported
    /// (factory loop v2 §6.6, report mode).
    pub(crate) fn attach(self: &Arc<Self>, lane: Lane) -> LaneAttachment {
        let lane_guard = lane.guarded().map(cas_store::lane_guard::enter_lane);
        let previous = ATTACHED.with(|attached| attached.replace(Some(Arc::clone(self))));
        LaneAttachment {
            previous,
            _lane: lane_guard,
        }
    }

    /// Mark the top of a pass, so [`Self::complete_pass`] can time it.
    pub(crate) fn begin_pass(&self) {
        if let Ok(mut started) = self.pass_started.lock() {
            *started = Some(self.clock.now());
        }
        self.loop_tid.store(current_tid(), Ordering::Relaxed);
    }

    /// Take the pass durations recorded since the last call.
    pub(crate) fn take_pass_window(&self) -> PassWindow {
        let durations = self
            .window
            .lock()
            .map(|mut window| std::mem::take(&mut *window))
            .unwrap_or_default();
        let passes = durations.len() as u64;
        PassWindow::from_durations(durations, passes)
    }

    /// Take the per-phase latency recorded since the last call (cas-ee9ab).
    pub(crate) fn take_phase_window(
        &self,
    ) -> std::collections::BTreeMap<String, crate::factory_daemon_health::PhaseLatency> {
        self.phase_window
            .lock()
            .map(|mut window| std::mem::take(&mut *window))
            .unwrap_or_default()
            .into_iter()
            .map(|(name, latency)| (name.to_string(), latency))
            .collect()
    }

    /// Record the loop entering `phase`.
    pub(crate) fn enter(&self, phase: LoopPhase) {
        self.enter_step(phase.step());
    }

    /// The step running now.
    pub(crate) fn current_step(&self) -> Step {
        self.current
            .lock()
            .map(|current| current.step)
            .unwrap_or_else(|_| Step::idle())
    }

    /// Record the lane entering `step`. The step it leaves is timed into the
    /// per-step window and, when over its budget, into the slow ring, the
    /// `cas::loop_watchdog` log and the status line. The idle sleep between
    /// passes is never timed.
    pub(crate) fn enter_step(&self, step: Step) {
        if let Some(lane) = step.lane.guarded() {
            cas_store::lane_guard::switch_lane(lane);
        }
        let now = self.clock.now();
        let left = match self.current.lock() {
            Ok(mut current) => std::mem::replace(&mut *current, CurrentStep { step, started: now }),
            Err(_) => return,
        };
        let Some(budget_ms) = left.step.budget_ms else {
            return;
        };
        let ms = now.saturating_duration_since(left.started).as_millis() as u64;
        if let Ok(mut window) = self.phase_window.lock() {
            let latency = window.entry(left.step.name).or_default();
            latency.entries += 1;
            latency.max_ms = latency.max_ms.max(ms);
            if ms >= SLOW_PASS_MS {
                latency.over_100ms += 1;
            }
        }
        if ms > budget_ms {
            self.record_slow(left.step, ms, budget_ms, now);
        }
    }

    fn record_slow(&self, step: Step, elapsed_ms: u64, budget_ms: u64, now: Instant) {
        let event = SlowStepEvent {
            lane: step.lane.name().to_string(),
            step: step.name.to_string(),
            elapsed_ms,
            budget_ms,
            at: Utc::now(),
        };
        let Ok(mut slow) = self.slow.lock() else {
            return;
        };
        if slow.ring.len() == SLOW_RING_CAP {
            slow.ring.pop_front();
        }
        slow.ring.push_back(event.clone());
        let window = Duration::from_secs(SLOW_STATUS_SECS);
        let replace = slow.held.as_ref().is_none_or(|(held, seen)| {
            now.saturating_duration_since(*seen) >= window || elapsed_ms > held.elapsed_ms
        });
        if replace {
            slow.held = Some((event, now));
        }
        let log = match slow.logged.get_mut(step.name) {
            Some((last, suppressed))
                if now.saturating_duration_since(*last) < SLOW_LOG_INTERVAL =>
            {
                *suppressed += 1;
                None
            }
            Some((last, suppressed)) => {
                *last = now;
                Some(std::mem::take(suppressed))
            }
            None => {
                slow.logged.insert(step.name, (now, 0));
                Some(0)
            }
        };
        drop(slow);
        if let Some(suppressed) = log {
            tracing::warn!(
                target: "cas::loop_watchdog",
                lane = %step.lane.name(),
                step = %step.name,
                elapsed_ms,
                budget_ms,
                suppressed,
                "daemon step over budget"
            );
        }
    }

    /// The last [`SLOW_RING_CAP`] over-budget steps, oldest first.
    pub(crate) fn slow_events(&self) -> Vec<SlowStepEvent> {
        self.slow
            .lock()
            .map(|slow| slow.ring.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Check the lanes' heartbeats (factory loop v2 §6.4). A pass running
    /// for [`LANE_WEDGE_MS`] wedges the lane of its current step; a job
    /// pending past both that and its kind's budget wedges the jobs lane.
    /// The watchdog calls this every [`LANE_HEARTBEAT_MS`].
    pub(crate) fn check_wedge(&self) -> Option<WedgeCheck> {
        let now = self.clock.now();
        let wedge_after = Duration::from_millis(LANE_WEDGE_MS);
        let pass = self
            .pass_started
            .lock()
            .ok()
            .and_then(|started| *started)
            .map(|started| now.saturating_duration_since(started))
            .filter(|elapsed| *elapsed >= wedge_after)
            .map(|elapsed| {
                let step = self.current_step();
                (step.lane, step.name, elapsed)
            });
        let found = pass.or_else(|| {
            self.jobs
                .oldest(now)
                .into_iter()
                .filter(|(_, job)| job.age >= wedge_after.max(job.budget))
                .max_by_key(|(_, job)| job.age)
                .map(|(kind, job)| (Lane::Jobs, kind, job.age))
        });
        let Ok(mut wedge) = self.wedge.lock() else {
            return None;
        };
        let Some((lane, step, duration)) = found else {
            *wedge = None;
            return None;
        };
        let new = wedge
            .as_ref()
            .is_none_or(|known| known.lane != lane || known.step != step);
        if new {
            *wedge = Some(WedgeState {
                lane,
                step,
                since: now.checked_sub(duration).unwrap_or(now),
                detected_at: Utc::now(),
            });
            drop(wedge);
            tracing::warn!(
                target: "cas::loop_watchdog",
                lane = %lane.name(),
                step = %step,
                elapsed_ms = duration.as_millis() as u64,
                wedge_ms = LANE_WEDGE_MS,
                "daemon lane wedged"
            );
        }
        Some(WedgeCheck {
            lane,
            step,
            duration,
            new,
        })
    }

    /// The TUI status-line badge.
    ///
    /// `slow: <lane>/<step> <n> ms` names the worst over-budget step of the
    /// last [`SLOW_STATUS_SECS`]; a smaller event never replaces it. The text
    /// changes at most once per [`BADGE_MIN_INTERVAL`], since each change
    /// redraws the frame.
    ///
    /// `wedged: jobs/<kind> for <n> s` shows only while a job is still
    /// pending past its budget. An io or control wedge is never shown: today
    /// the loop draws this badge itself, so whenever it can draw, its own
    /// wedge has already ended (the stalled step then shows as `slow:`). A
    /// live loop wedge is visible in `loop.json` (written by the watchdog
    /// thread within a heartbeat) and `worker_status`; live TUI display waits
    /// for the io lane on its own thread (C4b) or a client-side "daemon
    /// silent" signal.
    pub(crate) fn status_line(&self) -> Option<String> {
        let now = self.clock.now();
        let candidate = self.badge_candidate(now);
        let Ok(mut badge) = self.badge.lock() else {
            return candidate;
        };
        let may_change = badge
            .changed_at
            .is_none_or(|at| now.saturating_duration_since(at) >= BADGE_MIN_INTERVAL);
        if candidate != badge.shown && may_change {
            badge.shown = candidate;
            badge.changed_at = Some(now);
        }
        badge.shown.clone()
    }

    fn badge_candidate(&self, now: Instant) -> Option<String> {
        let job_wedge = self
            .wedge
            .lock()
            .ok()
            .and_then(|wedge| wedge.clone())
            .filter(|wedge| wedge.lane == Lane::Jobs);
        if let Some(wedge) = job_wedge {
            let wedge_after = Duration::from_millis(LANE_WEDGE_MS);
            let still_pending = self
                .jobs
                .oldest(now)
                .get(wedge.step)
                .is_some_and(|job| job.age >= wedge_after.max(job.budget));
            if still_pending {
                return Some(format!(
                    "wedged: jobs/{} for {} s",
                    wedge.step,
                    now.saturating_duration_since(wedge.since).as_secs()
                ));
            }
        }
        let slow = self.slow.lock().ok()?;
        let (event, seen) = slow.held.as_ref()?;
        (now.saturating_duration_since(*seen) < Duration::from_secs(SLOW_STATUS_SECS)).then(|| {
            format!(
                "slow: {}/{} {} ms",
                event.lane, event.step, event.elapsed_ms
            )
        })
    }

    /// Mark a pass complete. The loop's future may move between runtime
    /// threads, so the thread id is refreshed here too. A loop wedge ends
    /// with the pass.
    pub(crate) fn complete_pass(&self) {
        let now = self.clock.now();
        let elapsed = self
            .pass_started
            .lock()
            .ok()
            .and_then(|mut started| started.take())
            .map(|started| now.saturating_duration_since(started).as_millis() as u64);
        if let Some(ms) = elapsed
            && let Ok(mut window) = self.window.lock()
            && window.len() < PASS_WINDOW_CAP
        {
            window.push(ms);
        }
        if let Ok(mut wedge) = self.wedge.lock()
            && wedge.as_ref().is_some_and(|wedge| wedge.lane != Lane::Jobs)
            && let Some(cleared) = wedge.take()
        {
            tracing::info!(
                target: "cas::loop_watchdog",
                lane = %cleared.lane.name(),
                step = %cleared.step,
                elapsed_ms = elapsed.unwrap_or_default(),
                "daemon lane recovered"
            );
        }
        self.last_pass_ms
            .store(Utc::now().timestamp_millis(), Ordering::Relaxed);
        self.passes.fetch_add(1, Ordering::Relaxed);
        self.loop_tid.store(current_tid(), Ordering::Relaxed);
    }

    pub(crate) fn set_spawn_snapshot(
        &self,
        pending_spawns: usize,
        in_flight: Option<(String, DateTime<Utc>)>,
    ) {
        if let Ok(mut snapshot) = self.spawn.lock() {
            snapshot.pending_spawns = pending_spawns;
            snapshot.in_flight = in_flight;
        }
    }

    pub(crate) fn record_reset(&self, outcome: String) {
        if let Ok(mut snapshot) = self.spawn.lock() {
            snapshot.last_reset = Some(outcome);
        }
    }

    pub(crate) fn status(&self, pid: u32, session: &str, now: DateTime<Utc>) -> DaemonLoopStatus {
        let last_pass_at =
            DateTime::from_timestamp_millis(self.last_pass_ms.load(Ordering::Relaxed))
                .unwrap_or(now);
        let (pending_spawns, in_flight, last_reset) = match self.spawn.lock() {
            Ok(snapshot) => (
                snapshot.pending_spawns,
                snapshot.in_flight.clone(),
                snapshot.last_reset.clone(),
            ),
            Err(_) => (0, None, None),
        };
        let clock_now = self.clock.now();
        let wedge = self
            .wedge
            .lock()
            .ok()
            .and_then(|wedge| wedge.clone())
            .map(|wedge| LaneWedge {
                lane: wedge.lane.name().to_string(),
                step: wedge.step.to_string(),
                duration_ms: clock_now.saturating_duration_since(wedge.since).as_millis() as u64,
                detected_at: wedge.detected_at,
                wchan: None,
            });
        let job_oldest_ms = self
            .jobs
            .oldest(clock_now)
            .into_iter()
            .map(|(kind, job)| (kind.to_string(), job.age.as_millis() as u64))
            .collect();
        DaemonLoopStatus {
            pid,
            session: session.to_string(),
            written_at: now,
            last_pass_at,
            phase: self.current_step().name.to_string(),
            passes: self.passes.load(Ordering::Relaxed),
            pending_spawns,
            in_flight_spawn: in_flight.as_ref().map(|(name, _)| name.clone()),
            in_flight_started_at: in_flight.map(|(_, started)| started),
            loop_thread_wait: None,
            helpers_killed: Vec::new(),
            last_reset,
            max_pass_ms: None,
            p99_pass_ms: None,
            window_passes: 0,
            passes_over_100ms: 0,
            phase_latency: Default::default(),
            slow_steps: self.slow_events(),
            wedge,
            job_oldest_ms,
            lane_violations: cas_store::lane_guard::total_violations(),
            lane_violation_sites: cas_store::lane_guard::violation_sites()
                .iter()
                .map(cas_store::lane_guard::LaneViolation::describe)
                .collect(),
        }
    }
}

#[cfg(target_os = "linux")]
fn current_tid() -> i64 {
    // SAFETY: gettid has no arguments and cannot fail.
    unsafe { libc::syscall(libc::SYS_gettid) }
}

#[cfg(not(target_os = "linux"))]
fn current_tid() -> i64 {
    0
}

/// The kernel wait channel of one of this process's threads, e.g.
/// `pipe_read` or `futex_wait_queue` (Linux only).
fn thread_wait_channel(tid: i64) -> Option<String> {
    if tid <= 0 {
        return None;
    }
    let wchan = std::fs::read_to_string(format!("/proc/self/task/{tid}/wchan")).ok()?;
    let wchan = wchan.trim();
    (!wchan.is_empty() && wchan != "0").then(|| wchan.to_string())
}

/// `ps` elapsed time, `[[dd-]hh:]mm:ss`, in seconds.
fn parse_etime(value: &str) -> Option<u64> {
    let (days, clock) = match value.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, value),
    };
    let parts = clock
        .split(':')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    let seconds = match parts.as_slice() {
        [minutes, seconds] => minutes * 60 + seconds,
        [hours, minutes, seconds] => hours * 3600 + minutes * 60 + seconds,
        _ => return None,
    };
    Some(days * 86_400 + seconds)
}

/// One `ps` row: (pid, ppid, age seconds, command basename).
fn parse_ps_row(line: &str) -> Option<(i32, u32, u64, String)> {
    let mut fields = line.split_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let ppid = fields.next()?.parse().ok()?;
    let age = parse_etime(fields.next()?)?;
    let command = fields.collect::<Vec<_>>().join(" ");
    let basename = command.rsplit('/').next()?.to_string();
    Some((pid, ppid, age, basename))
}

/// Children of `parent` that are helper commands older than `min_age_secs`.
fn hung_helpers(ps_output: &str, parent: u32, min_age_secs: u64) -> Vec<(i32, u64, String)> {
    ps_output
        .lines()
        .filter_map(parse_ps_row)
        .filter(|(_, ppid, age, command)| {
            *ppid == parent && *age >= min_age_secs && HELPER_COMMANDS.contains(&command.as_str())
        })
        .map(|(pid, _, age, command)| (pid, age, command))
        .collect()
}

#[cfg(unix)]
fn kill_hung_helpers(parent: u32, min_age_secs: u64) -> Vec<String> {
    let Ok(output) = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,etime=,comm="])
        .output()
    else {
        return Vec::new();
    };
    let listing = String::from_utf8_lossy(&output.stdout);
    let mut killed = Vec::new();
    for (pid, age, command) in hung_helpers(&listing, parent, min_age_secs) {
        // SAFETY: plain kill(2) on a pid we just observed as our own child.
        if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
            tracing::warn!(
                pid,
                command = %command,
                age_secs = age,
                "cas-73b5: killed a hung helper to unblock the wedged daemon loop"
            );
            killed.push(format!("{command} (pid {pid}, {age}s)"));
        }
    }
    killed
}

#[cfg(not(unix))]
fn kill_hung_helpers(_parent: u32, _min_age_secs: u64) -> Vec<String> {
    Vec::new()
}

/// Start the watchdog thread. It checks the lanes' heartbeats every
/// [`LANE_HEARTBEAT_MS`] and writes the status file every
/// [`WATCHDOG_INTERVAL_SECS`], and at once when a lane wedges (cas-6931). It
/// stops when `shutdown` is set, and removes its status file on the way out
/// so a closed session does not read as a silent daemon.
pub(crate) fn spawn_watchdog(
    progress: Arc<LoopProgress>,
    cas_dir: PathBuf,
    session: String,
    shutdown: Arc<AtomicBool>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("cas-loop-watchdog".to_string())
        .spawn(move || {
            let pid = std::process::id();
            let mut helpers_killed: Vec<String> = Vec::new();
            let ticks_per_write = (WATCHDOG_INTERVAL_SECS * 1000 / LANE_HEARTBEAT_MS).max(1);
            let mut ticks_since_write = ticks_per_write;
            let mut window = PassWindow::default();
            let mut phase_latency = std::collections::BTreeMap::new();
            while !shutdown.load(Ordering::Relaxed) {
                let wedge = progress.check_wedge();
                let wedged_now = wedge.as_ref().is_some_and(|wedge| wedge.new);
                if ticks_since_write >= ticks_per_write || wedged_now {
                    let now = Utc::now();
                    let mut status = progress.status(pid, &session, now);
                    let loop_wedged = wedge.as_ref().is_some_and(|wedge| wedge.lane != Lane::Jobs);
                    if loop_wedged {
                        let wait = thread_wait_channel(progress.loop_tid.load(Ordering::Relaxed));
                        if let Some(wedge) = status.wedge.as_mut() {
                            wedge.wchan = wait.clone();
                        }
                        status.loop_thread_wait = wait;
                    }
                    let stalled = now.signed_duration_since(status.last_pass_at).num_seconds()
                        > LOOP_WEDGED_SECS;
                    if stalled && pending_reset(&cas_dir, &session).is_some() {
                        helpers_killed.extend(kill_hung_helpers(pid, HELPER_MIN_AGE_SECS));
                        let excess = helpers_killed.len().saturating_sub(KILLED_HELPERS_KEPT);
                        helpers_killed.drain(..excess);
                    }
                    status.helpers_killed = helpers_killed.clone();
                    // The pass windows span the regular interval; a write on a
                    // new wedge repeats the last one.
                    if ticks_since_write >= ticks_per_write {
                        window = progress.take_pass_window();
                        phase_latency = progress.take_phase_window();
                        ticks_since_write = 0;
                    }
                    status.max_pass_ms = window.max_ms;
                    status.p99_pass_ms = window.p99_ms;
                    status.window_passes = window.passes;
                    status.passes_over_100ms = window.over_100ms;
                    status.phase_latency = phase_latency.clone();
                    if let Err(error) = write_status(&cas_dir, &status) {
                        tracing::debug!(%error, "cas-73b5: could not write the daemon loop status");
                    }
                }
                std::thread::sleep(Duration::from_millis(LANE_HEARTBEAT_MS));
                ticks_since_write += 1;
            }
            let _ = std::fs::remove_file(status_path(&cas_dir, &session));
        })
        .map_err(|error| {
            tracing::warn!(%error, "cas-73b5: could not start the daemon loop watchdog");
        })
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct LogBuffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn capture_logs(run: impl FnOnce()) -> String {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let writer_logs = Arc::clone(&logs);
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || LogBuffer(Arc::clone(&writer_logs)))
            .finish();
        tracing::subscriber::with_default(subscriber, run);
        String::from_utf8(logs.lock().unwrap().clone()).unwrap()
    }

    /// Factory loop v2 §6.1–6.3 (cas-6931): a step over its budget is named,
    /// with its lane, in the slow ring, the `cas::loop_watchdog` log and the
    /// TUI status line, which shows it for 10 s. Fake clock.
    #[test]
    fn flv2_watchdog_names_slow_step() {
        let clock = FakeClock::new();
        let progress =
            LoopProgress::with_clock(LoopClock::Fake(Arc::clone(&clock)), JobHealth::new());
        let log = capture_logs(|| {
            let _lane = progress.attach(Lane::Control);
            progress.begin_pass();
            progress.enter(LoopPhase::ClientInput);
            clock.advance(Duration::from_millis(3));
            progress.enter(LoopPhase::PromptQueue);
            // A prompt-queue sub-step, marked from inside the step the way
            // `process_prompt_queue` marks its own.
            mark(Step::prompt_queue("prompt queue: stub step"));
            clock.advance(Duration::from_millis(45));
            {
                // A nested scope (the director apply's `timed()` steps)
                // returns to the enclosing step when it ends.
                let _scope = scoped(Step::control("change detection"));
                clock.advance(Duration::from_millis(4));
            }
            assert_eq!(progress.current_step().name, "prompt queue: stub step");
            progress.enter(LoopPhase::Render);
            progress.complete_pass();
            progress.enter(LoopPhase::Idle);
            // The idle sleep is never a slow step.
            clock.advance(Duration::from_millis(400));
            progress.begin_pass();
        });

        let ring = progress.slow_events();
        assert_eq!(ring.len(), 1, "only the over-budget step: {ring:?}");
        assert_eq!(ring[0].lane, "control");
        assert_eq!(ring[0].step, "prompt queue: stub step");
        assert_eq!(ring[0].elapsed_ms, 45);
        assert_eq!(ring[0].budget_ms, PROMPT_QUEUE_STEP_BUDGET_MS);

        let warn = log
            .lines()
            .find(|line| line.contains("daemon step over budget"))
            .unwrap_or_else(|| panic!("no over-budget log line: {log}"));
        assert!(warn.contains("WARN"), "{warn}");
        assert!(warn.contains("cas::loop_watchdog"), "{warn}");
        assert!(warn.contains("lane=control"), "{warn}");
        assert!(warn.contains("step=prompt queue: stub step"), "{warn}");
        assert!(warn.contains("elapsed_ms=45"), "{warn}");
        assert!(warn.contains("budget_ms=20"), "{warn}");

        assert_eq!(
            progress.status_line().as_deref(),
            Some("slow: control/prompt queue: stub step 45 ms")
        );
        let status = progress.status(1, "s", Utc::now());
        assert_eq!(status.slow_steps, ring);
        clock.advance(Duration::from_secs(SLOW_STATUS_SECS));
        assert_eq!(progress.status_line(), None, "the badge clears after 10 s");

        // An io step has the 5 ms budget, and the ring keeps the last 32.
        for _ in 0..(SLOW_RING_CAP + 8) {
            progress.enter(LoopPhase::PtyOutput);
            clock.advance(Duration::from_millis(IO_STEP_BUDGET_MS + 1));
            progress.enter(LoopPhase::Idle);
        }
        let ring = progress.slow_events();
        assert_eq!(ring.len(), SLOW_RING_CAP);
        // The badge changed to "none" under a second ago; it waits.
        assert_eq!(progress.status_line(), None);
        clock.advance(BADGE_MIN_INTERVAL);
        assert!(
            ring.iter()
                .all(|event| event.lane == "io" && event.step == "pty output")
        );
        assert_eq!(
            progress.status_line().as_deref(),
            Some("slow: io/pty output 6 ms")
        );
    }

    /// Factory loop v2 §6.4 (cas-6931): a lane blocked 1.5 s is reported as
    /// wedged with its step, within the 250 ms heartbeat of the 1 s wedge,
    /// in `loop.json` and the status line; a job pending past its budget is
    /// a named jobs-lane wedge.
    #[test]
    fn flv2_wedge_reports_step_and_duration() {
        let temp = tempfile::tempdir().unwrap();
        let progress = LoopProgress::new();
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle = spawn_watchdog(
            Arc::clone(&progress),
            temp.path().to_path_buf(),
            "flv2-wedge".to_string(),
            Arc::clone(&shutdown),
        )
        .expect("watchdog starts");

        let lane_progress = Arc::clone(&progress);
        let (blocked_tx, blocked) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let lane = std::thread::spawn(move || {
            let _lane = lane_progress.attach(Lane::Control);
            lane_progress.begin_pass();
            lane_progress.enter(LoopPhase::PromptQueue);
            mark(Step::prompt_queue("prompt queue: urgent inject"));
            blocked_tx.send(()).unwrap();
            // Blocked until the test has seen the wedge and 1.5 s have passed.
            let _ = released.recv_timeout(Duration::from_secs(20));
            lane_progress.enter(LoopPhase::Render);
            lane_progress.complete_pass();
            lane_progress.enter(LoopPhase::Idle);
        });
        blocked.recv().unwrap();
        let started = Instant::now();

        let mut wedge = None;
        while wedge.is_none() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
            wedge = crate::factory_daemon_health::read_status(temp.path(), "flv2-wedge")
                .and_then(|status| status.wedge);
        }
        let detected_after = started.elapsed();
        let wedge = wedge.expect("loop.json records the wedge");
        assert_eq!(wedge.lane, "control");
        assert_eq!(wedge.step, "prompt queue: urgent inject");
        assert!(wedge.duration_ms >= LANE_WEDGE_MS, "{wedge:?}");
        assert!(
            detected_after < Duration::from_secs(3),
            "a 1 s wedge is written on the heartbeat, not at the next 5 s write: \
             {detected_after:?}"
        );
        // The loop draws its own badge, so it can only draw once its wedge
        // is over: the badge never claims a loop wedge (C4b draws it live).
        assert_eq!(
            progress.status_line(),
            None,
            "no stale or unreachable wedged: badge"
        );

        let remaining = Duration::from_millis(1_500).saturating_sub(started.elapsed());
        std::thread::sleep(remaining);
        release.send(()).unwrap();
        lane.join().unwrap();
        assert_eq!(progress.check_wedge(), None, "the wedge ends with the pass");
        let blocked_step = progress
            .slow_events()
            .into_iter()
            .find(|event| event.step == "prompt queue: urgent inject")
            .expect("the blocked step is in the slow ring");
        assert_eq!(blocked_step.lane, "control");
        assert!(blocked_step.elapsed_ms >= 1_500, "{blocked_step:?}");
        let badge = progress
            .status_line()
            .expect("the stalled step shows as slow");
        assert!(
            badge.starts_with("slow: control/prompt queue: urgent inject 1"),
            "{badge}"
        );

        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap();

        // A job pending past its budget and the 1 s wedge names the jobs lane.
        let clock = FakeClock::new();
        let jobs = JobHealth::new();
        let progress =
            LoopProgress::with_clock(LoopClock::Fake(Arc::clone(&clock)), Arc::clone(&jobs));
        let ticket = jobs.begin_at("store write", Duration::from_millis(100), clock.now());
        clock.advance(Duration::from_millis(900));
        assert_eq!(progress.check_wedge(), None, "under the 1 s wedge");
        clock.advance(Duration::from_millis(600));
        let check = progress.check_wedge().expect("job wedge");
        assert_eq!(check.lane, Lane::Jobs);
        assert_eq!(check.step, "store write");
        assert_eq!(check.duration, Duration::from_millis(1_500));
        assert!(check.new);
        assert_eq!(
            progress.status_line().as_deref(),
            Some("wedged: jobs/store write for 1 s")
        );
        let status = progress.status(1, "s", Utc::now());
        assert_eq!(status.job_oldest_ms.get("store write"), Some(&1_500));
        assert_eq!(
            status.wedge.as_ref().map(|wedge| wedge.lane.as_str()),
            Some("jobs")
        );
        assert!(
            !progress.check_wedge().unwrap().new,
            "logged once per wedge"
        );
        drop(ticket);
        clock.advance(BADGE_MIN_INTERVAL);
        assert_eq!(
            progress.status_line(),
            None,
            "a finished job is not shown as wedged, even before the next heartbeat"
        );
        assert_eq!(progress.check_wedge(), None);
        assert!(progress.status(1, "s", Utc::now()).job_oldest_ms.is_empty());
    }

    /// cas-6931 review: the badge holds the worst slow step of its 10 s
    /// window and changes at most once a second, so a stream of small slow
    /// steps (each change is a redraw, itself a possible slow step) cannot
    /// make it churn. Fake clock.
    #[test]
    fn flv2_badge_holds_worst_step_and_changes_at_most_once_a_second() {
        let clock = FakeClock::new();
        let progress =
            LoopProgress::with_clock(LoopClock::Fake(Arc::clone(&clock)), JobHealth::new());
        let slow_step = |name: &'static str, ms: u64| {
            progress.enter_step(Step::prompt_queue(name));
            clock.advance(Duration::from_millis(ms));
            progress.enter(LoopPhase::Idle);
        };
        fn observe(shown: &mut Vec<Option<String>>, line: Option<String>) {
            if shown.last() != Some(&line) {
                shown.push(line);
            }
        }
        let mut shown = Vec::new();

        slow_step("prompt queue: a", 30);
        observe(&mut shown, progress.status_line());
        // Thirty smaller slow steps of other names within the next second.
        for i in 0..30 {
            slow_step(
                if i % 2 == 0 {
                    "prompt queue: b"
                } else {
                    "prompt queue: c"
                },
                21 + (i % 8),
            );
            observe(&mut shown, progress.status_line());
        }
        assert_eq!(
            shown,
            vec![Some("slow: control/prompt queue: a 30 ms".to_string())]
        );

        // A worse step replaces it, but not before a second has passed
        // since the last change.
        let since_change = Duration::from_millis(30 + 30 * 25);
        slow_step("prompt queue: worst", 80);
        observe(&mut shown, progress.status_line());
        assert_eq!(shown.len(), 1, "{shown:?}");
        clock.advance(BADGE_MIN_INTERVAL.saturating_sub(since_change));
        observe(&mut shown, progress.status_line());
        assert_eq!(
            shown.last().cloned().flatten().as_deref(),
            Some("slow: control/prompt queue: worst 80 ms")
        );

        // Smaller steps over the next seconds never displace the worst.
        for _ in 0..40 {
            slow_step("prompt queue: b", 40);
            clock.advance(Duration::from_millis(60));
            observe(&mut shown, progress.status_line());
        }
        assert_eq!(shown.len(), 2, "{shown:?}");

        // It expires 10 s after the worst step; nothing newer is worse
        // than the newest small step, which then takes over.
        clock.advance(Duration::from_secs(SLOW_STATUS_SECS));
        slow_step("prompt queue: b", 25);
        observe(&mut shown, progress.status_line());
        assert_eq!(
            shown.last().cloned().flatten().as_deref(),
            Some("slow: control/prompt queue: b 25 ms")
        );
        clock.advance(Duration::from_secs(SLOW_STATUS_SECS));
        observe(&mut shown, progress.status_line());
        assert_eq!(shown.last(), Some(&None));
        assert_eq!(shown.len(), 4, "{shown:?}");
    }

    #[test]
    fn pass_window_reports_max_p99_and_slow_passes_then_resets() {
        let progress = LoopProgress::new();
        assert_eq!(progress.take_pass_window(), PassWindow::default());
        progress.begin_pass();
        progress.complete_pass();
        // A pass without begin_pass (the first, before the loop marks it) is
        // counted in `passes` but not timed.
        progress.complete_pass();
        let window = progress.take_pass_window();
        assert_eq!(window.passes, 1);
        assert!(window.max_ms.unwrap() < SLOW_PASS_MS);
        assert_eq!(window.over_100ms, 0);
        assert_eq!(progress.take_pass_window(), PassWindow::default());

        let durations: Vec<u64> = (1..=200).collect();
        let summary = PassWindow::from_durations(durations, 200);
        assert_eq!(summary.max_ms, Some(200));
        assert_eq!(summary.p99_ms, Some(198));
        assert_eq!(summary.over_100ms, 101);
    }

    /// cas-ee9ab: the 5 s status file can only name the phase the loop is in
    /// now. The per-phase window attributes each slow pass to the phase that
    /// spent the time, and never times the idle sleep.
    #[test]
    fn phase_window_attributes_slow_time_to_its_phase_cas_ee9ab() {
        let progress = LoopProgress::new();
        progress.begin_pass();
        progress.enter(LoopPhase::ClientInput);
        std::thread::sleep(Duration::from_millis(5));
        progress.enter(LoopPhase::RefreshDirector);
        std::thread::sleep(Duration::from_millis(120));
        progress.enter(LoopPhase::Render);
        progress.complete_pass();
        progress.enter(LoopPhase::Idle);
        std::thread::sleep(Duration::from_millis(150));
        progress.begin_pass();
        progress.enter(LoopPhase::ClientInput);
        progress.enter(LoopPhase::Render);
        progress.complete_pass();
        progress.enter(LoopPhase::Idle);

        let window = progress.take_phase_window();
        let director = &window["refresh: director data"];
        assert_eq!(director.entries, 1);
        assert!(director.max_ms >= 120, "{director:?}");
        assert_eq!(director.over_100ms, 1);
        let input = &window["client input"];
        assert_eq!(input.entries, 2);
        assert!(input.max_ms < SLOW_PASS_MS, "{input:?}");
        assert_eq!(input.over_100ms, 0);
        assert_eq!(window["render"].entries, 2);
        assert!(
            !window.contains_key("idle"),
            "idle sleep was timed: {window:?}"
        );
        assert!(
            progress.take_phase_window().is_empty(),
            "window was not reset"
        );

        let mut status = progress.status(1, "s", Utc::now());
        status.phase_latency = window;
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(
            json["phase_latency"]["refresh: director data"]["over_100ms"],
            1
        );
        // Older status files without the field still parse.
        let mut legacy = json.clone();
        legacy.as_object_mut().unwrap().remove("phase_latency");
        let parsed: crate::factory_daemon_health::DaemonLoopStatus =
            serde_json::from_value(legacy).unwrap();
        assert!(parsed.phase_latency.is_empty());
    }

    #[test]
    fn etime_parses_every_ps_shape() {
        assert_eq!(parse_etime("05"), None);
        assert_eq!(parse_etime("01:05"), Some(65));
        assert_eq!(parse_etime("02:01:05"), Some(7265));
        assert_eq!(parse_etime("3-02:01:05"), Some(3 * 86_400 + 7265));
        assert_eq!(parse_etime("x:05"), None);
    }

    #[test]
    fn only_old_helper_children_of_the_daemon_are_candidates() {
        let listing = "\
            100   1 10:00 /usr/bin/cas\n\
            200 100 05:00 git\n\
            201 100 00:10 git\n\
            202 100 05:00 /usr/bin/gh\n\
            203 100 05:00 claude\n\
            204 999 05:00 git\n\
            205 100 1-00:00:00 ssh\n";
        let candidates = hung_helpers(listing, 100, 30);
        assert_eq!(
            candidates,
            vec![
                (200, 300, "git".to_string()),
                (202, 300, "gh".to_string()),
                (205, 86_400, "ssh".to_string()),
            ],
            "a young helper, a harness pane and another process's child are never killed"
        );
    }

    /// A real hung helper: `git cat-file --batch` blocks on its stdin, the way
    /// a git waiting on a lock or a prompt blocks a daemon pass.
    #[cfg(unix)]
    #[test]
    fn a_hung_git_child_is_killed_and_named() {
        use std::os::unix::process::ExitStatusExt;
        let mut child = Command::new("git")
            .args(["cat-file", "--batch"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("git is installed");
        std::thread::sleep(Duration::from_millis(1_200));
        let killed = kill_hung_helpers(std::process::id(), 1);
        assert!(
            killed
                .iter()
                .any(|entry| entry.starts_with(&format!("git (pid {}", child.id()))),
            "{killed:?}"
        );
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }

    #[test]
    fn progress_reports_phase_passes_and_spawn_state() {
        let progress = LoopProgress::new();
        let now = Utc::now();
        progress.enter(LoopPhase::Refresh);
        progress.set_spawn_snapshot(2, Some(("vivid-finch-91".to_string(), now)));
        progress.record_reset("dropped 2".to_string());
        let status = progress.status(7, "cas-src-test", now);
        assert_eq!(status.phase, "refresh");
        assert_eq!(status.passes, 0);
        assert_eq!(status.pending_spawns, 2);
        assert_eq!(status.in_flight_spawn.as_deref(), Some("vivid-finch-91"));
        assert_eq!(status.last_reset.as_deref(), Some("dropped 2"));
        progress.complete_pass();
        assert_eq!(progress.status(7, "cas-src-test", Utc::now()).passes, 1);
    }

    #[test]
    fn the_watchdog_publishes_a_wedged_loop_and_removes_its_file_on_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        let progress = LoopProgress::new();
        progress.enter(LoopPhase::Refresh);
        progress.last_pass_ms.store(
            (Utc::now() - chrono::Duration::seconds(600)).timestamp_millis(),
            Ordering::Relaxed,
        );
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle = spawn_watchdog(
            Arc::clone(&progress),
            temp.path().to_path_buf(),
            "cas-src-test".to_string(),
            Arc::clone(&shutdown),
        )
        .expect("watchdog starts");
        let path = status_path(temp.path(), "cas-src-test");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let status = crate::factory_daemon_health::read_status(temp.path(), "cas-src-test")
            .expect("status written while the loop is wedged");
        assert_eq!(status.phase, "refresh");
        assert!(
            Utc::now()
                .signed_duration_since(status.last_pass_at)
                .num_seconds()
                >= 600
        );
        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        assert!(!path.exists());
    }
}
