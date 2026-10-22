//! GH #1208: durable external reminder probes (`branch_contained_in`,
//! `tag_exists`) run off the daemon loop.
//!
//! The probe used to run inside `process_reminders`, which made one serial
//! `git fetch` per pending reminder on the loop thread once a minute and froze
//! input echo and rendering in every pane for 2.5–9 s. The loop now only
//! collects a finished scan and starts the next one on a short-lived thread.
//! The scan fetches each origin ref once, backs off a ref whose read keeps
//! failing, and first expires reminders whose owning session is long gone.
//! The pending reminder row stays the durable edge state, so a lost scan or a
//! daemon restart only delays a wake.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::ci_watch::{
    CiWatchError, EXTERNAL_BRANCH_CONTAINED_EVENT, EXTERNAL_CI_COMPLETED_EVENT,
    EXTERNAL_TAG_EXISTS_EVENT, EXTERNAL_WAKE_INTERVAL, ExternalGitOutput, ExternalProbeKey,
    ExternalWakeCondition, ExternalWakeObservation, ExternalWakeProbe,
};

/// Longest wait between reads of an origin ref that keeps failing.
pub(crate) const EXTERNAL_WAKE_MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);

/// How long both the owner and the target of a durable external reminder may
/// go without a heartbeat (and without a live process) before the reminder is
/// expired. A shorter gap is a restart or handoff, which the cross-session
/// takeover in `fire_reminder` is designed to survive.
pub(crate) const DEAD_SESSION_GRACE_SECS: i64 = 24 * 60 * 60;

/// A pending external reminder whose condition was observed true.
#[derive(Debug, Clone)]
pub(crate) struct ExternalWake {
    pub(crate) reminder: cas_store::Reminder,
    pub(crate) condition: ExternalWakeCondition,
    pub(crate) observation: ExternalWakeObservation,
}

#[derive(Debug, Clone, Copy)]
struct BackoffEntry {
    failures: u32,
    retry_at: Instant,
}

/// Per-ref exponential backoff for origin reads that keep failing, such as a
/// deleted branch or an unreachable origin. Every reminder naming the ref
/// waits with it. A successful read clears the entry.
#[derive(Debug, Default)]
pub(crate) struct ExternalWakeBackoff {
    entries: HashMap<ExternalProbeKey, BackoffEntry>,
}

impl ExternalWakeBackoff {
    /// The wait after `failures` consecutive failed reads: two scan intervals
    /// after the first, doubling up to [`EXTERNAL_WAKE_MAX_BACKOFF`].
    pub(crate) fn delay(failures: u32) -> Duration {
        let factor = 1u32 << failures.clamp(1, 10);
        EXTERNAL_WAKE_INTERVAL
            .saturating_mul(factor)
            .min(EXTERNAL_WAKE_MAX_BACKOFF)
    }

    pub(crate) fn is_waiting(&self, key: &ExternalProbeKey, now: Instant) -> bool {
        self.entries.get(key).is_some_and(|entry| now < entry.retry_at)
    }

    pub(crate) fn failures(&self, key: &ExternalProbeKey) -> u32 {
        self.entries.get(key).map_or(0, |entry| entry.failures)
    }

    pub(crate) fn record(&mut self, key: &ExternalProbeKey, failed: bool, now: Instant) {
        if !failed {
            self.entries.remove(key);
            return;
        }
        let failures = self.failures(key).saturating_add(1);
        let delay = Self::delay(failures);
        tracing::debug!(%key, failures, ?delay, "external reminder probe backing off");
        self.entries.insert(
            key.clone(),
            BackoffEntry {
                failures,
                retry_at: now + delay,
            },
        );
    }
}

/// Probe each candidate once against one view of origin, skipping refs still
/// in backoff, then record this scan's read outcomes in `backoff`.
pub(crate) fn probe_external_wakes<R>(
    probe: &mut ExternalWakeProbe<R>,
    candidates: Vec<(cas_store::Reminder, ExternalWakeCondition)>,
    backoff: &mut ExternalWakeBackoff,
    now: Instant,
) -> Vec<ExternalWake>
where
    R: FnMut(&[String]) -> Result<ExternalGitOutput, CiWatchError>,
{
    let mut ready = Vec::new();
    for (reminder, condition) in candidates {
        if backoff.is_waiting(&condition.probe_key(), now) {
            continue;
        }
        match probe.observe(&condition) {
            Ok(Some(observation)) => ready.push(ExternalWake {
                reminder,
                condition,
                observation,
            }),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                reminder_id = reminder.id,
                event_type = condition.event_type(),
                %error,
                "external reminder probe unavailable; retaining pending row"
            ),
        }
    }
    for (key, failed) in probe.read_outcomes() {
        backoff.record(&key, failed, now);
    }
    ready
}

/// Pending `branch_contained_in` and `tag_exists` reminders this factory
/// session may fire, with their parsed conditions.
pub(crate) fn external_wake_candidates(
    cas_dir: &Path,
    session_name: &str,
) -> Vec<(cas_store::Reminder, ExternalWakeCondition)> {
    let store = match crate::store::open_reminder_store(cas_dir) {
        Ok(store) => store,
        Err(error) => {
            tracing::debug!(%error, "external reminder scan could not open the reminder store");
            return Vec::new();
        }
    };
    let mut candidates = Vec::new();
    for event_type in [EXTERNAL_BRANCH_CONTAINED_EVENT, EXTERNAL_TAG_EXISTS_EVENT] {
        let reminders = match store.get_event_reminders(event_type) {
            Ok(reminders) => reminders,
            Err(error) => {
                tracing::warn!(event_type, %error, "failed to load external reminder candidates");
                continue;
            }
        };
        for reminder in reminders {
            if !super::queue_and_events::reminder_matches_factory_session(
                reminder.session_id.as_deref(),
                reminder.cross_session,
                session_name,
            ) {
                continue;
            }
            let Some(filter) = reminder.trigger_filter.as_ref() else {
                tracing::warn!(
                    reminder_id = reminder.id,
                    event_type,
                    "external reminder has no condition filter"
                );
                continue;
            };
            match super::ci_watch::parse_external_wake_condition(event_type, filter) {
                Ok(condition) => candidates.push((reminder, condition)),
                Err(error) => tracing::warn!(
                    reminder_id = reminder.id,
                    event_type,
                    %error,
                    "external reminder has invalid condition filter"
                ),
            }
        }
    }
    candidates
}

/// Whether an agent's session is gone for a reminder created at
/// `reminder_created`: no heartbeat for [`DEAD_SESSION_GRACE_SECS`] and no
/// live process, or no agent row and a reminder older than that grace. A
/// store error is not evidence of death.
///
/// A missing row alone is not death: SessionEnd and factory teardown delete
/// agent rows, so a restarted or resumed factory has no row for its owner
/// until it registers again. Its durable reminders must survive that gap.
fn session_gone(
    agents: &dyn cas_store::AgentStore,
    agent_id: &str,
    reminder_created: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    process_alive: &dyn Fn(&cas_types::Agent) -> bool,
) -> bool {
    match agents.get(agent_id) {
        Err(cas_store::StoreError::NotFound(_)) => {
            (now - reminder_created).num_seconds() > DEAD_SESSION_GRACE_SECS
        }
        Err(_) => false,
        Ok(agent) => {
            (now - agent.last_heartbeat).num_seconds() > DEAD_SESSION_GRACE_SECS
                && !process_alive(&agent)
        }
    }
}

/// Expire pending external reminders (`branch_contained_in`, `tag_exists`,
/// `ci_completed`) whose owner and target sessions are both gone (see
/// [`session_gone`]). These rows
/// are durable by design, so without this a dead session's wake is re-probed
/// forever, including one already satisfied whose target can never receive
/// it. Returns the number expired.
pub(crate) fn expire_dead_session_reminders(
    cas_dir: &Path,
    now: chrono::DateTime<chrono::Utc>,
    process_alive: &dyn Fn(&cas_types::Agent) -> bool,
) -> usize {
    let (Ok(reminders), Ok(agents)) = (
        crate::store::open_reminder_store(cas_dir),
        crate::store::open_agent_store(cas_dir),
    ) else {
        return 0;
    };
    let mut expired = 0;
    for event_type in [
        EXTERNAL_BRANCH_CONTAINED_EVENT,
        EXTERNAL_TAG_EXISTS_EVENT,
        EXTERNAL_CI_COMPLETED_EVENT,
    ] {
        let Ok(pending) = reminders.get_event_reminders(event_type) else {
            continue;
        };
        for reminder in pending {
            let gone = |agent_id: &str| {
                session_gone(agents.as_ref(), agent_id, reminder.created_at, now, process_alive)
            };
            let owner_gone = gone(&reminder.owner_id);
            let target_gone = owner_gone
                && (reminder.target_id == reminder.owner_id || gone(&reminder.target_id));
            if !target_gone {
                continue;
            }
            match reminders.expire(reminder.id) {
                Ok(true) => {
                    expired += 1;
                    tracing::info!(
                        reminder_id = reminder.id,
                        event_type,
                        owner = %reminder.owner_id,
                        target = %reminder.target_id,
                        "expired external reminder whose owning session is gone"
                    );
                }
                Ok(false) => {}
                Err(error) => tracing::warn!(
                    reminder_id = reminder.id,
                    %error,
                    "failed to expire dead-session external reminder"
                ),
            }
        }
    }
    expired
}

/// cas-c634: pending reminders whose issuer and recipient are both gone
/// (see [`session_gone`]). No live agent sees them otherwise.
pub(crate) fn orphaned_reminder_count(
    cas_dir: &Path,
    now: chrono::DateTime<chrono::Utc>,
    process_alive: &dyn Fn(&cas_types::Agent) -> bool,
) -> usize {
    // Red: the pre-cas-c634 behaviour, which never counted them.
    let _ = (cas_dir, now, process_alive);
    0
}

/// One complete scan; runs on the scanner thread, never the daemon loop.
fn run_external_wake_scan(
    cas_dir: &Path,
    project: &Path,
    session_name: &str,
    backoff: &mut ExternalWakeBackoff,
) -> Vec<ExternalWake> {
    expire_dead_session_reminders(
        cas_dir,
        chrono::Utc::now(),
        &crate::mcp::tools::service::agent_liveness::agent_process_is_alive,
    );
    let candidates = external_wake_candidates(cas_dir, session_name);
    if candidates.is_empty() {
        return Vec::new();
    }
    let mut probe = ExternalWakeProbe::new(|args: &[String]| {
        super::ci_watch::run_external_git_command(project, args)
    });
    probe_external_wakes(&mut probe, candidates, backoff, Instant::now())
}

struct ExternalWakeScan {
    ready: Vec<ExternalWake>,
    backoff: ExternalWakeBackoff,
}

/// The daemon loop's handle on the background external-wake scan. At most one
/// scan runs at a time and one starts per [`EXTERNAL_WAKE_INTERVAL`]. The
/// backoff state travels with the scan and comes back with its results.
pub(crate) struct ExternalWakeScanner {
    last_started: Option<Instant>,
    in_flight: Option<mpsc::Receiver<ExternalWakeScan>>,
    backoff: Option<ExternalWakeBackoff>,
}

impl Default for ExternalWakeScanner {
    fn default() -> Self {
        Self {
            last_started: None,
            in_flight: None,
            backoff: Some(ExternalWakeBackoff::default()),
        }
    }
}

impl ExternalWakeScanner {
    /// Called on the daemon loop. Never waits: it takes a finished scan's
    /// wakes, if any, and starts the next scan when one is due.
    pub(crate) fn poll(
        &mut self,
        cas_dir: &Path,
        project: &Path,
        session_name: &str,
    ) -> Vec<ExternalWake> {
        let ready = self.take_finished(None);
        if self.in_flight.is_none()
            && self
                .last_started
                .is_none_or(|started| started.elapsed() >= EXTERNAL_WAKE_INTERVAL)
        {
            self.start(
                cas_dir.to_path_buf(),
                project.to_path_buf(),
                session_name.to_string(),
            );
        }
        ready
    }

    fn take_finished(&mut self, wait: Option<Duration>) -> Vec<ExternalWake> {
        let Some(receiver) = self.in_flight.as_ref() else {
            return Vec::new();
        };
        let received = match wait {
            None => receiver.try_recv().map_err(|error| match error {
                mpsc::TryRecvError::Empty => None,
                mpsc::TryRecvError::Disconnected => Some(()),
            }),
            Some(timeout) => receiver.recv_timeout(timeout).map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => None,
                mpsc::RecvTimeoutError::Disconnected => Some(()),
            }),
        };
        match received {
            Ok(scan) => {
                self.in_flight = None;
                self.backoff = Some(scan.backoff);
                scan.ready
            }
            Err(None) => Vec::new(),
            Err(Some(())) => {
                tracing::warn!("external reminder scan ended without a result; retrying next interval");
                self.in_flight = None;
                self.backoff = Some(ExternalWakeBackoff::default());
                Vec::new()
            }
        }
    }

    fn start(&mut self, cas_dir: PathBuf, project: PathBuf, session_name: String) {
        self.last_started = Some(Instant::now());
        let mut backoff = self.backoff.take().unwrap_or_default();
        let (sender, receiver) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("cas-external-wake".into())
            .spawn(move || {
                let ready = run_external_wake_scan(&cas_dir, &project, &session_name, &mut backoff);
                let _ = sender.send(ExternalWakeScan { ready, backoff });
            });
        match spawned {
            Ok(_) => self.in_flight = Some(receiver),
            Err(error) => {
                tracing::warn!(%error, "could not start the external reminder scan");
                self.backoff = Some(ExternalWakeBackoff::default());
            }
        }
    }

    /// Test hook: wait up to `timeout` for the in-flight scan.
    #[cfg(test)]
    pub(crate) fn wait_for_scan(&mut self, timeout: Duration) -> Option<Vec<ExternalWake>> {
        self.in_flight.as_ref()?;
        let ready = self.take_finished(Some(timeout));
        self.in_flight.is_none().then_some(ready)
    }

    #[cfg(test)]
    pub(crate) fn backoff(&self) -> Option<&ExternalWakeBackoff> {
        self.backoff.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn reminder(id: i64, event: &str) -> cas_store::Reminder {
        cas_store::Reminder {
            id,
            owner_id: "sup".into(),
            target_id: "sup".into(),
            message: format!("wake {id}"),
            trigger_type: cas_store::ReminderTriggerType::Event,
            trigger_at: None,
            trigger_event: Some(event.into()),
            trigger_filter: None,
            status: cas_store::ReminderStatus::Pending,
            ttl_secs: 0,
            created_at: chrono::Utc::now(),
            fired_at: None,
            cancelled_at: None,
            fired_event: None,
            session_id: None,
            origin_session_id: None,
            cross_session: true,
            task_id: None,
        }
    }

    fn branch(id: i64, target: &str) -> (cas_store::Reminder, ExternalWakeCondition) {
        (
            reminder(id, EXTERNAL_BRANCH_CONTAINED_EVENT),
            ExternalWakeCondition::BranchContained {
                commit: format!("commit-{id}"),
                target_branch: target.into(),
            },
        )
    }

    fn tag(id: i64, tag: &str) -> (cas_store::Reminder, ExternalWakeCondition) {
        (
            reminder(id, EXTERNAL_TAG_EXISTS_EVENT),
            ExternalWakeCondition::TagExists { tag: tag.into() },
        )
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A fake origin: records every git command and fails the fetch of any
    /// branch in `failing`.
    #[derive(Clone, Default)]
    struct FakeGit {
        calls: Rc<RefCell<Vec<Vec<String>>>>,
        failing: Rc<RefCell<Vec<String>>>,
        timing_out: Rc<RefCell<Vec<String>>>,
    }

    impl FakeGit {
        fn runner(&self) -> impl FnMut(&[String]) -> Result<ExternalGitOutput, CiWatchError> {
            let fake = self.clone();
            move |args: &[String]| {
                fake.calls.borrow_mut().push(args.to_vec());
                let ok = |stdout: String| Ok(ExternalGitOutput { success: true, stdout });
                match args[0].as_str() {
                    "fetch" => {
                        let refspec = &args[3];
                        let named = |list: &Vec<String>| {
                            list.iter().any(|b| refspec.starts_with(&format!("+refs/heads/{b}:")))
                        };
                        if named(&fake.timing_out.borrow()) {
                            return Err(CiWatchError::Unavailable("timed out".into()));
                        }
                        if named(&fake.failing.borrow()) {
                            return Ok(ExternalGitOutput { success: false, stdout: String::new() });
                        }
                        ok(String::new())
                    }
                    "ls-remote" => ok(format!("{SHA}\t{}\n", args[4])),
                    "rev-parse" => ok(format!("{SHA}\n")),
                    _ => ok(String::new()),
                }
            }
        }

        fn count(&self, command: &str) -> usize {
            self.calls.borrow().iter().filter(|c| c[0] == command).count()
        }

        fn fetches_of(&self, branch: &str) -> usize {
            let prefix = format!("+refs/heads/{branch}:");
            self.calls
                .borrow()
                .iter()
                .filter(|c| c[0] == "fetch" && c[3].starts_with(&prefix))
                .count()
        }
    }

    /// GH #1208: seven reminders over two branches and one tag cost two
    /// fetches and one `ls-remote`, not seven network round trips.
    #[test]
    fn a_scan_reads_each_origin_ref_once() {
        let git = FakeGit::default();
        let mut probe = ExternalWakeProbe::new(git.runner());
        let candidates = vec![
            branch(1, "main"),
            branch(2, "main"),
            branch(3, "epic/v41"),
            branch(4, "main"),
            branch(5, "epic/v41"),
            tag(6, "v9.99.0"),
            tag(7, "v9.99.0"),
        ];
        let mut backoff = ExternalWakeBackoff::default();
        let ready = probe_external_wakes(&mut probe, candidates, &mut backoff, Instant::now());

        assert_eq!(git.count("fetch"), 2, "{:?}", git.calls.borrow());
        assert_eq!(git.fetches_of("main"), 1);
        assert_eq!(git.fetches_of("epic/v41"), 1);
        assert_eq!(git.count("ls-remote"), 1);
        // Containment is still decided per reminder, locally.
        assert_eq!(git.count("merge-base"), 5);
        assert_eq!(
            ready.iter().map(|wake| wake.reminder.id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(ready[0].observation.compared_ref, "refs/remotes/origin/main");
        assert_eq!(ready[0].observation.compared_sha, SHA);
    }

    /// GH #1208: a fetch that keeps failing is retried after a doubling wait,
    /// not every minute forever; healthy refs are unaffected and a success
    /// clears the backoff.
    #[test]
    fn a_failing_fetch_backs_off_and_recovers() {
        let git = FakeGit::default();
        git.failing.borrow_mut().push("gone".into());
        git.timing_out.borrow_mut().push("slow".into());
        let mut backoff = ExternalWakeBackoff::default();
        let gone = ExternalProbeKey::Branch("gone".into());
        let slow = ExternalProbeKey::Branch("slow".into());
        let main = ExternalProbeKey::Branch("main".into());
        let t0 = Instant::now();
        let scan = |backoff: &mut ExternalWakeBackoff, at: Duration| {
            let mut probe = ExternalWakeProbe::new(git.runner());
            let candidates = vec![branch(1, "gone"), branch(2, "main"), branch(3, "slow")];
            probe_external_wakes(&mut probe, candidates, backoff, t0 + at)
        };

        let ready = scan(&mut backoff, Duration::ZERO);
        assert_eq!(ready.iter().map(|w| w.reminder.id).collect::<Vec<_>>(), vec![2]);
        assert_eq!(backoff.failures(&gone), 1);
        assert_eq!(backoff.failures(&slow), 1, "a timed-out fetch is a failure too");
        assert_eq!(backoff.failures(&main), 0);

        // The next scan skips both failing refs; main is still fetched.
        scan(&mut backoff, Duration::from_secs(60));
        assert_eq!(git.fetches_of("gone"), 1);
        assert_eq!(git.fetches_of("slow"), 1);
        assert_eq!(git.fetches_of("main"), 2);

        // Retried after two intervals; a second failure doubles the wait.
        scan(&mut backoff, Duration::from_secs(120));
        assert_eq!(git.fetches_of("gone"), 2);
        assert_eq!(backoff.failures(&gone), 2);
        scan(&mut backoff, Duration::from_secs(120 + 239));
        assert_eq!(git.fetches_of("gone"), 2);

        // Origin recovers: the next due read succeeds and clears the entry.
        git.failing.borrow_mut().clear();
        let ready = scan(&mut backoff, Duration::from_secs(120 + 240));
        assert_eq!(git.fetches_of("gone"), 3);
        assert_eq!(backoff.failures(&gone), 0);
        assert!(ready.iter().any(|w| w.reminder.id == 1));

        assert_eq!(ExternalWakeBackoff::delay(1), Duration::from_secs(120));
        assert_eq!(ExternalWakeBackoff::delay(4), Duration::from_secs(960));
        assert_eq!(ExternalWakeBackoff::delay(5), EXTERNAL_WAKE_MAX_BACKOFF);
        assert_eq!(ExternalWakeBackoff::delay(u32::MAX), EXTERNAL_WAKE_MAX_BACKOFF);
    }

    /// GH #1208: the issue's stale rows were cross-session, TTL 0 and owned by
    /// sessions dead for days, so nothing ever ended them. A row is expired
    /// once its owner and target are both gone; a live, recent or
    /// process-backed session keeps it, as does any non-external reminder. A
    /// missing agent row counts as gone only for a reminder older than the
    /// grace, so a factory restart does not expire its owner's wakes.
    #[test]
    fn dead_session_external_reminders_expire() {
        let dir = tempfile::tempdir().unwrap();
        let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let now = chrono::Utc::now();
        for (id, hours_ago) in [("dead", 25), ("zombie", 25), ("recent", 2), ("live", 0)] {
            let mut agent = cas_types::Agent::new(id.into(), id.into());
            agent.last_heartbeat = now - chrono::Duration::hours(hours_ago);
            agents.register(&agent).unwrap();
        }
        let reminders = crate::store::open_reminder_store(&cas_dir).unwrap();
        let create = |owner: &str, target: &str, event: &str| {
            reminders
                .create_with_scope(
                    owner,
                    Some(target),
                    "wake",
                    cas_store::ReminderTriggerType::Event,
                    None,
                    Some(event),
                    Some(&serde_json::json!({"commit": "abc", "target_branch": "main"})),
                    0,
                    Some("old-factory"),
                    None,
                    true,
                    None,
                )
                .unwrap()
        };
        let dead_self = create("dead", "dead", EXTERNAL_BRANCH_CONTAINED_EVENT);
        // No agent row: SessionEnd and teardown delete rows, so a restarted
        // factory's owner is missing until it registers again. A recent
        // reminder survives that gap; one older than the grace is expired.
        let restarted = create("pruned", "pruned", EXTERNAL_TAG_EXISTS_EVENT);
        let missing_old = create("pruned", "pruned", EXTERNAL_TAG_EXISTS_EVENT);
        // The #3042 shape: a ci_completed wake whose supervisor session died.
        let dead_ci = create("dead", "pruned", EXTERNAL_CI_COMPLETED_EVENT);
        let live = create("live", "live", EXTERNAL_BRANCH_CONTAINED_EVENT);
        let recent = create("recent", "recent", EXTERNAL_BRANCH_CONTAINED_EVENT);
        let live_target = create("dead", "live", EXTERNAL_BRANCH_CONTAINED_EVENT);
        let process_backed = create("zombie", "zombie", EXTERNAL_BRANCH_CONTAINED_EVENT);
        let director_event = create("pruned", "pruned", "task_completed");
        // cas-c634: a retired worker's reminder to itself that also expires
        // on its own TTL is left to that TTL, but counted as orphaned.
        let director_with_ttl = reminders
            .create_with_scope(
                "pruned", Some("pruned"), "wake", cas_store::ReminderTriggerType::Event, None,
                Some("task_blocked"), None, 86_400 * 30, Some("old-factory"), None, true, None,
            )
            .unwrap();
        let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
        for id in [missing_old, dead_ci, director_event, director_with_ttl] {
            conn.execute(
                "UPDATE reminders SET created_at = ?1 WHERE id = ?2",
                rusqlite::params![(now - chrono::Duration::hours(25)).to_rfc3339(), id],
            )
            .unwrap();
        }

        let process_alive = |agent: &cas_types::Agent| agent.id == "zombie";
        // cas-c634: the never-expiring internal-event reminder of a retired
        // worker (director_event) is expired with the external ones.
        assert_eq!(expire_dead_session_reminders(&cas_dir, now, &process_alive), 4);
        assert_eq!(expire_dead_session_reminders(&cas_dir, now, &process_alive), 0);
        assert_eq!(
            orphaned_reminder_count(&cas_dir, now, &process_alive),
            1,
            "the TTL'd internal reminder of a retired worker is counted"
        );

        let mut pending: Vec<i64> = reminders
            .list_all_pending()
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        pending.sort_unstable();
        assert_eq!(
            pending,
            vec![restarted, live, recent, live_target, process_backed, director_with_ttl]
        );
        for expired in [dead_self, missing_old, dead_ci, director_event] {
            assert!(!pending.contains(&expired));
        }
    }
}
