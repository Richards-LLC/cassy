//! Background cloud sync for idle project stores (cas-f1fb).
//!
//! Cloud sync otherwise runs only inside `cas serve`, which exists only while
//! an agent session is open in that project. A project nobody opens on a
//! machine stops pulling and pushing entirely. The machine hub is the one
//! long-lived host process (launchd `dev.cas.commander-hub` on macOS, the
//! user systemd `cas-hub.service` on Linux), so it sweeps the registered
//! stores itself:
//!
//! * **Discovery**: the host known-repos registry, through
//!   [`super::projects::list_projects`]. A store qualifies when it has
//!   `cloud.json`, is logged in, and has not set `cloud.auto_sync = false`.
//! * **Live stores are skipped**: a store with a live `cas serve` (its
//!   `daemon.sock` accepts a connection) is already syncing itself.
//! * **One store at a time, under its own identity**: each store runs as a
//!   child `cas cloud sync --background`, with its cwd and `CAS_ROOT` bound to
//!   that store and an allow-listed environment. A fresh process per store
//!   means no process-wide identity cache can carry one store's identity into
//!   another. That carry-over is the failure behind the fleet-wide
//!   contamination of epic cas-5985.
//! * **Never two syncs for one store**: `--background` takes the store's
//!   [`crate::cloud::sync_lease`] lease, which every serve daemon also takes,
//!   or exits [`SYNC_LEASE_BUSY_EXIT`] without syncing.
//! * **Bounded**: at most [`IdleSyncConfig::max_stores_per_sweep`] stores per
//!   sweep (least recently attempted first), a killed child after
//!   [`IdleSyncConfig::store_timeout`], and exponential per-store backoff
//!   after a failure.
//!
//! Each store's last result is kept in `~/.cas/hub/idle-sync.json`, which
//! `cas doctor` reports.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cloud::sync_lease::SYNC_LEASE_BUSY_EXIT;

/// File name of the sweep state inside the hub directory (`~/.cas/hub`).
pub const STATE_FILE: &str = "idle-sync.json";

/// Log of the latest background sync, inside each store's `.cas/logs`.
pub const STORE_LOG_FILE: &str = "hub-idle-sync.log";

/// Delay before the first sweep, so hub startup is never slowed by a sync.
const STARTUP_DELAY: Duration = Duration::from_secs(60);

/// How often a running child is polled for exit, timeout and cancellation.
const CHILD_POLL: Duration = Duration::from_millis(200);

/// Longest failure detail kept in the state file.
const DETAIL_LIMIT: usize = 300;

/// Environment passed through to a store's sync child. Everything else is
/// dropped, including the hub's own `CAS_*` session and agent identity, so a
/// hub launched from inside a factory session cannot lend that identity to a
/// store. `CAS_ROOT` is then set to the store being synced.
const ENV_ALLOWLIST: &[&str] = &[
    "HOME",
    "PATH",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
    // Host-level cloud settings, the same for every store on this machine.
    "CAS_CLOUD_ENDPOINT",
    "CAS_USER_CLOUD_JSON",
    // Test guard: a child must panic rather than touch a protected HOME.
    "CAS_TEST_PROTECTED_HOME",
];

/// Bounds of the idle-store sweep.
#[derive(Debug, Clone)]
pub struct IdleSyncConfig {
    /// Time between sweeps. A qualifying store syncs within one interval.
    pub interval: Duration,
    /// Longest one store's sync may run before its child is killed.
    pub store_timeout: Duration,
    /// Most stores synced in one sweep; the rest wait for the next.
    pub max_stores_per_sweep: usize,
    /// Ceiling of the per-store failure backoff.
    pub backoff_max: Duration,
}

impl Default for IdleSyncConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(300),
            store_timeout: Duration::from_secs(180),
            max_stores_per_sweep: 8,
            backoff_max: Duration::from_secs(6 * 60 * 60),
        }
    }
}

impl IdleSyncConfig {
    /// Wait after the `failures`-th consecutive failure: twice the interval,
    /// doubling per failure, capped at [`Self::backoff_max`].
    pub fn backoff(&self, failures: u32) -> Duration {
        let factor = 1u32.checked_shl(failures.min(20)).unwrap_or(u32::MAX);
        self.interval
            .checked_mul(factor)
            .unwrap_or(self.backoff_max)
            .min(self.backoff_max)
    }
}

/// The result of one store's turn in a sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepOutcome {
    /// `cas cloud sync` completed.
    Synced,
    /// `cas cloud sync` exited with an error.
    Failed,
    /// The sync ran past the per-store timeout and was killed.
    TimedOut,
    /// Another syncer held the store's lease; nothing ran.
    Busy,
    /// A live `cas serve` is syncing the store itself; nothing ran.
    LiveServe,
}

impl SweepOutcome {
    /// Whether this outcome counts toward the store's failure backoff.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Failed | Self::TimedOut)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Synced => "synced",
            Self::Failed => "failed",
            Self::TimedOut => "timed out",
            Self::Busy => "skipped (another sync held the lease)",
            Self::LiveServe => "skipped (a live cas serve syncs it)",
        }
    }
}

/// One store's latest sweep result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreSweepRecord {
    pub repo: PathBuf,
    pub last_attempt_at: DateTime<Utc>,
    pub outcome: SweepOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_attempt_after: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<DateTime<Utc>>,
}

/// The persisted sweep state: every swept store's latest record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdleSyncState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sweep_at: Option<DateTime<Utc>>,
    /// Keyed by the store's repository path.
    #[serde(default)]
    pub stores: BTreeMap<String, StoreSweepRecord>,
}

impl IdleSyncState {
    /// Read the state. A missing file is an empty state.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parse idle-sync state {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => {
                Err(error).with_context(|| format!("read idle-sync state {}", path.display()))
            }
        }
    }

    /// Write the state atomically: a temp file, then a rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("write {}", temp.display()))?;
        std::fs::rename(&temp, path).with_context(|| format!("replace {}", path.display()))
    }

    pub fn record(&self, repo: &Path) -> Option<&StoreSweepRecord> {
        self.stores.get(&store_key(repo))
    }
}

/// The sweep state file under a hub directory.
pub fn state_path(hub_root: &Path) -> PathBuf {
    hub_root.join(STATE_FILE)
}

/// The sweep state file for this user's hub (`~/.cas/hub/idle-sync.json`).
pub fn default_state_path() -> Result<PathBuf> {
    Ok(state_path(
        super::HubRuntimePaths::default_for_user()?.root(),
    ))
}

fn store_key(repo: &Path) -> String {
    repo.to_string_lossy().into_owned()
}

/// Why a registered store does or does not take part in a sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    Eligible,
    NotCloudLinked,
    LoggedOut,
    AutoSyncOff,
    LiveServe,
}

/// Decide whether `cas_root` should be synced by the sweep now.
pub fn eligibility(cas_root: &Path) -> Eligibility {
    if !cas_root.join("cloud.json").is_file() {
        return Eligibility::NotCloudLinked;
    }
    // Read-only: the sync child itself inherits and caches the user login.
    let project_login = crate::cloud::CloudConfig::load_from_cas_dir(cas_root)
        .is_ok_and(|config| config.is_logged_in());
    if !project_login
        && !crate::cloud::CloudConfig::load_user().is_ok_and(|user| user.is_logged_in())
    {
        return Eligibility::LoggedOut;
    }
    let auto_sync = crate::config::Config::load(cas_root)
        .ok()
        .and_then(|config| config.cloud)
        .is_none_or(|cloud| cloud.auto_sync);
    if !auto_sync {
        return Eligibility::AutoSyncOff;
    }
    if serve_is_live(cas_root) {
        return Eligibility::LiveServe;
    }
    Eligibility::Eligible
}

/// Whether a `cas serve` is live in this store. Every serve's daemon joins
/// the `daemon.sock` election, so a live owner accepts a connection, and a
/// survivor rebinds within the election interval after an owner dies.
#[cfg(unix)]
pub fn serve_is_live(cas_root: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(crate::mcp::socket::socket_path(cas_root)).is_ok()
}

#[cfg(not(unix))]
pub fn serve_is_live(_cas_root: &Path) -> bool {
    false
}

/// The result of running one store's sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunResult {
    Synced,
    Failed(String),
    TimedOut,
    Busy,
    /// The hub is stopping; the child was killed and nothing is recorded.
    Cancelled,
}

/// Runs one store's cloud sync. The real runner spawns `cas cloud sync`;
/// tests substitute their own.
pub trait StoreSyncRunner: Send + Sync {
    fn run(
        &self,
        repo: &Path,
        cas_root: &Path,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> RunResult;
}

/// Runs `cas cloud sync --background` as a child bound to one store.
#[derive(Debug, Clone)]
pub struct ProcessSyncRunner {
    binary: PathBuf,
    extra_env: Vec<(OsString, OsString)>,
}

impl ProcessSyncRunner {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            extra_env: Vec::new(),
        }
    }

    /// The runner for the running `cas` binary. After an upgrade replaced it
    /// on disk, Linux reports the old path with a ` (deleted)` suffix; the
    /// path itself then names the new install, which is the right binary.
    pub fn current() -> Result<Self> {
        let exe = std::env::current_exe().context("resolve the running cas binary")?;
        let raw = exe.to_string_lossy();
        let binary = raw
            .strip_suffix(" (deleted)")
            .map(PathBuf::from)
            .unwrap_or_else(|| exe.clone());
        anyhow::ensure!(
            binary.is_file(),
            "cas binary {} is missing",
            binary.display()
        );
        Ok(Self::new(binary))
    }

    /// Add an environment variable on top of the allowlist (tests).
    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    fn command(&self, repo: &Path, cas_root: &Path) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .args(["cloud", "sync", "--background"])
            .current_dir(repo)
            .env_clear();
        for key in ENV_ALLOWLIST {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        for (key, value) in &self.extra_env {
            command.env(key, value);
        }
        command.env("CAS_ROOT", cas_root).stdin(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Own process group, so a timeout kills any helper it started.
            command.process_group(0);
        }
        command
    }
}

fn kill_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    // SAFETY: the child leads its own process group (process_group(0)), so
    // its pid is that group's id and the signal reaches only its tree.
    unsafe {
        libc::killpg(child.id() as i32, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn log_tail(log: &mut std::fs::File) -> Option<String> {
    let length = log.seek(SeekFrom::End(0)).ok()?;
    log.seek(SeekFrom::Start(length.saturating_sub(4096)))
        .ok()?;
    let mut text = String::new();
    log.read_to_string(&mut text).ok()?;
    let line = text.lines().rev().find(|line| !line.trim().is_empty())?;
    Some(truncate(line.trim()))
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= DETAIL_LIMIT {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(DETAIL_LIMIT).collect();
    cut.push('…');
    cut
}

impl StoreSyncRunner for ProcessSyncRunner {
    fn run(
        &self,
        repo: &Path,
        cas_root: &Path,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> RunResult {
        let log_dir = cas_root.join("logs");
        if let Err(error) = std::fs::create_dir_all(&log_dir) {
            return RunResult::Failed(format!("create {}: {error}", log_dir.display()));
        }
        let log_path = log_dir.join(STORE_LOG_FILE);
        let log = match OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&log_path)
        {
            Ok(log) => log,
            Err(error) => {
                return RunResult::Failed(format!("open {}: {error}", log_path.display()));
            }
        };
        let (stdout, stderr) = match (log.try_clone(), log.try_clone()) {
            (Ok(stdout), Ok(stderr)) => (stdout, stderr),
            _ => return RunResult::Failed(format!("share {}", log_path.display())),
        };
        let mut child = match self
            .command(repo, cas_root)
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                return RunResult::Failed(format!("spawn {}: {error}", self.binary.display()));
            }
        };
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => {
                    kill_child(&mut child);
                    return RunResult::Failed(format!("wait for cloud sync: {error}"));
                }
            }
            if cancel.load(Ordering::SeqCst) {
                kill_child(&mut child);
                return RunResult::Cancelled;
            }
            if started.elapsed() >= timeout {
                kill_child(&mut child);
                return RunResult::TimedOut;
            }
            std::thread::sleep(CHILD_POLL);
        };
        let mut log = log;
        match status.code() {
            Some(0) => RunResult::Synced,
            Some(code) if code == SYNC_LEASE_BUSY_EXIT => RunResult::Busy,
            code => {
                let exit =
                    code.map_or_else(|| "killed by a signal".to_owned(), |c| format!("exit {c}"));
                let detail = log_tail(&mut log)
                    .map(|line| format!("{exit}: {line}"))
                    .unwrap_or(exit);
                RunResult::Failed(detail)
            }
        }
    }
}

/// What one sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Stores whose sync ran (or was refused by the lease), in order.
    pub ran: Vec<(PathBuf, SweepOutcome)>,
    /// Stores skipped because a live serve syncs them.
    pub live: Vec<PathBuf>,
    /// Stores still in failure backoff.
    pub backing_off: Vec<PathBuf>,
    /// Qualifying stores left for a later sweep by the per-sweep cap.
    pub deferred: Vec<PathBuf>,
    /// The hub stopped mid-sweep.
    pub cancelled: bool,
}

/// One sweep over `repos` (registered project roots). Runs at most
/// `config.max_stores_per_sweep` syncs, one at a time, least recently
/// attempted first, and saves the state after every store.
pub fn run_sweep(
    config: &IdleSyncConfig,
    repos: &[PathBuf],
    runner: &dyn StoreSyncRunner,
    state_path: &Path,
    now: DateTime<Utc>,
    cancel: &AtomicBool,
) -> Result<SweepReport> {
    let mut state = IdleSyncState::load(state_path).unwrap_or_else(|error| {
        tracing::warn!(%error, "idle sync: unreadable state; starting fresh");
        IdleSyncState::default()
    });
    let registered: BTreeSet<String> = repos.iter().map(|repo| store_key(repo)).collect();
    state.stores.retain(|key, _| registered.contains(key));

    let mut order: Vec<&PathBuf> = repos.iter().collect();
    order.sort_by_key(|repo| state.record(repo).map(|record| record.last_attempt_at));
    order.dedup();

    let mut report = SweepReport::default();
    for repo in order {
        if cancel.load(Ordering::SeqCst) {
            report.cancelled = true;
            break;
        }
        let key = store_key(repo);
        let cas_root = repo.join(".cas");
        let previous = state.stores.get(&key).cloned();
        if previous
            .as_ref()
            .and_then(|record| record.next_attempt_after)
            .is_some_and(|next| next > now)
        {
            report.backing_off.push(repo.clone());
            continue;
        }
        match eligibility(&cas_root) {
            Eligibility::NotCloudLinked | Eligibility::LoggedOut | Eligibility::AutoSyncOff => {
                state.stores.remove(&key);
                continue;
            }
            Eligibility::LiveServe => {
                state.stores.insert(
                    key,
                    StoreSweepRecord {
                        repo: repo.clone(),
                        last_attempt_at: now,
                        outcome: SweepOutcome::LiveServe,
                        detail: None,
                        duration_ms: 0,
                        consecutive_failures: 0,
                        next_attempt_after: None,
                        last_success_at: previous.and_then(|record| record.last_success_at),
                    },
                );
                report.live.push(repo.clone());
                save(&state, state_path);
                continue;
            }
            Eligibility::Eligible => {}
        }
        if report.ran.len() >= config.max_stores_per_sweep {
            report.deferred.push(repo.clone());
            continue;
        }
        let started = Instant::now();
        let result = runner.run(repo, &cas_root, config.store_timeout, cancel);
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let (outcome, detail) = match result {
            RunResult::Synced => (SweepOutcome::Synced, None),
            RunResult::Failed(detail) => (SweepOutcome::Failed, Some(truncate(&detail))),
            RunResult::TimedOut => (
                SweepOutcome::TimedOut,
                Some(format!(
                    "killed after {}s; see {}",
                    config.store_timeout.as_secs(),
                    cas_root.join("logs").join(STORE_LOG_FILE).display()
                )),
            ),
            RunResult::Busy => (SweepOutcome::Busy, None),
            RunResult::Cancelled => {
                report.cancelled = true;
                break;
            }
        };
        let previous_failures = previous
            .as_ref()
            .map_or(0, |record| record.consecutive_failures);
        let consecutive_failures = if outcome.is_failure() {
            previous_failures.saturating_add(1)
        } else if outcome == SweepOutcome::Synced {
            0
        } else {
            previous_failures
        };
        let next_attempt_after = outcome
            .is_failure()
            .then(|| chrono::Duration::from_std(config.backoff(consecutive_failures)).ok())
            .flatten()
            .map(|delay| now + delay);
        let last_success_at = if outcome == SweepOutcome::Synced {
            Some(now)
        } else {
            previous.and_then(|record| record.last_success_at)
        };
        state.stores.insert(
            key,
            StoreSweepRecord {
                repo: repo.clone(),
                last_attempt_at: now,
                outcome,
                detail,
                duration_ms,
                consecutive_failures,
                next_attempt_after,
                last_success_at,
            },
        );
        report.ran.push((repo.clone(), outcome));
        save(&state, state_path);
    }
    state.last_sweep_at = Some(now);
    state.save(state_path)?;
    Ok(report)
}

fn save(state: &IdleSyncState, path: &Path) {
    if let Err(error) = state.save(path) {
        tracing::warn!(%error, "idle sync: could not save sweep state");
    }
}

/// Registered project roots, from the host known-repos registry.
pub fn registered_repos() -> Vec<PathBuf> {
    match super::projects::list_projects(&[]) {
        Ok(projects) => projects.into_iter().map(|project| project.path).collect(),
        Err(error) => {
            tracing::debug!(%error, "idle sync: known-repos registry unavailable");
            Vec::new()
        }
    }
}

/// The hub's sweep loop. [`IdleSyncLoop::stop`] kills an in-flight child, so
/// stopping the hub is never held up by a store's sync.
pub struct IdleSyncLoop {
    handle: tokio::task::JoinHandle<()>,
    cancel: Arc<AtomicBool>,
}

impl IdleSyncLoop {
    pub fn stop(self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.handle.abort();
    }
}

/// Start the sweep inside the hub's runtime.
pub fn spawn_idle_sync_loop(hub_root: PathBuf) -> IdleSyncLoop {
    let cancel = Arc::new(AtomicBool::new(false));
    let loop_cancel = Arc::clone(&cancel);
    let handle = tokio::spawn(async move {
        let runner: Arc<dyn StoreSyncRunner> = match ProcessSyncRunner::current() {
            Ok(runner) => Arc::new(runner),
            Err(error) => {
                tracing::warn!(%error, "idle sync disabled: no cas binary to run");
                return;
            }
        };
        let config = IdleSyncConfig::default();
        let path = state_path(&hub_root);
        let mut interval =
            tokio::time::interval_at(tokio::time::Instant::now() + STARTUP_DELAY, config.interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let (config, runner, path, cancel) = (
                config.clone(),
                Arc::clone(&runner),
                path.clone(),
                Arc::clone(&loop_cancel),
            );
            let swept = tokio::task::spawn_blocking(move || {
                let repos = registered_repos();
                run_sweep(&config, &repos, runner.as_ref(), &path, Utc::now(), &cancel)
            })
            .await;
            match swept {
                Ok(Ok(report)) if !report.ran.is_empty() => {
                    tracing::info!(
                        ran = report.ran.len(),
                        live = report.live.len(),
                        deferred = report.deferred.len(),
                        "idle sync sweep"
                    );
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => tracing::warn!(%error, "idle sync sweep failed"),
                Err(error) => tracing::warn!(%error, "idle sync sweep panicked"),
            }
        }
    });
    IdleSyncLoop { handle, cancel }
}

#[cfg(test)]
#[path = "idle_sync_tests.rs"]
mod tests;
