//! Logging infrastructure for Cassy
//!
//! Provides structured logging using tracing-subscriber with multiple layers:
//! - Console layer: only when --verbose, writes to stderr
//! - File layer: always on, writes to .cas/logs/
//! - EnvFilter: respects RUST_LOG env var

use arc_swap::ArcSwap;
use fs2::FileExt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

/// Initialize the logging system
///
/// Call this early in main() before any other code runs.
///
/// # Arguments
/// * `cas_root` - Path to .cas directory (if available)
/// * `verbose` - Whether --verbose flag was passed
/// * `config` - Logging configuration from config file
pub fn init(cas_root: Option<&Path>, verbose: bool, config: &LoggingConfig) -> io::Result<()> {
    // Build the env filter
    // Priority: RUST_LOG env var > config level > default (info)
    let default_level = config.level.as_str();
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    // Console layer - only when verbose, writes to stderr
    let console_layer = if verbose {
        Some(
            fmt::layer()
                .with_writer(io::stderr)
                .with_ansi(true)
                .with_target(true)
                .with_level(true)
                .with_filter(env_filter.clone()),
        )
    } else {
        None
    };

    // File layer - always on if logging is enabled and cas_root exists
    let file_layer = if config.enabled {
        if let Some(root) = cas_root {
            let log_dir = root.join(&config.log_dir);
            fs::create_dir_all(&log_dir)?;

            let log_writer = DailyLogWriter::new(&log_dir)?;

            Some(
                fmt::layer()
                    .with_writer(log_writer)
                    .with_ansi(false)
                    .with_target(true)
                    .with_level(true)
                    .with_filter(EnvFilter::new(default_level)),
            )
        } else {
            None
        }
    } else {
        None
    };

    // Build the subscriber with layers
    let subscriber = tracing_subscriber::registry()
        .with(console_layer)
        .with(file_layer);

    // Set as global default (ignore error if already set)
    tracing::subscriber::set_global_default(subscriber).ok();

    Ok(())
}

#[derive(Clone)]
struct DailyLogWriter {
    shared: Arc<DailyLogShared>,
}

struct DailyLogShared {
    log_dir: PathBuf,
    active: ArcSwap<ActiveLogFile>,
    next_date_check: AtomicU64,
    rotation: Mutex<()>,
    local_date: Arc<dyn Fn() -> chrono::NaiveDate + Send + Sync>,
    coarse_time: Arc<dyn Fn() -> u64 + Send + Sync>,
    open_file: Arc<dyn Fn(&Path, chrono::NaiveDate) -> io::Result<File> + Send + Sync>,
}

struct ActiveLogFile {
    date: chrono::NaiveDate,
    file: File,
}

impl Drop for ActiveLogFile {
    fn drop(&mut self) {
        // cas-cef2: a rotated handle may have been inherited across fork;
        // close-only release would keep the old log undeletable.
        if let Err(error) = FileExt::unlock(&self.file) {
            // Do not emit through tracing while dropping tracing's own file
            // writer. That can recurse into this writer during rotation or
            // shutdown; stderr is the safe visible fallback.
            eprintln!("ERROR: Failed to release Cassy log file lock: {error}");
        }
    }
}

const DATE_CHECK_INTERVAL_SECS: u64 = 1;

impl DailyLogWriter {
    fn new(log_dir: &Path) -> io::Result<Self> {
        Self::new_with_clocks_and_opener(
            log_dir,
            || chrono::Local::now().date_naive(),
            coarse_unix_time,
            open_log_file,
        )
    }

    fn new_with_clocks_and_opener<D, C, O>(
        log_dir: &Path,
        local_date: D,
        coarse_time: C,
        open_file: O,
    ) -> io::Result<Self>
    where
        D: Fn() -> chrono::NaiveDate + Send + Sync + 'static,
        C: Fn() -> u64 + Send + Sync + 'static,
        O: Fn(&Path, chrono::NaiveDate) -> io::Result<File> + Send + Sync + 'static,
    {
        let local_date = Arc::new(local_date);
        let coarse_time = Arc::new(coarse_time);
        let open_file = Arc::new(open_file);
        let date = local_date();
        let file = open_file(log_dir, date)?;
        let next_date_check = coarse_time().saturating_add(DATE_CHECK_INTERVAL_SECS);
        Ok(Self {
            shared: Arc::new(DailyLogShared {
                log_dir: log_dir.to_path_buf(),
                active: ArcSwap::from_pointee(ActiveLogFile { date, file }),
                next_date_check: AtomicU64::new(next_date_check),
                rotation: Mutex::new(()),
                local_date,
                coarse_time,
                open_file,
            }),
        })
    }
}

impl<'a> MakeWriter<'a> for DailyLogWriter {
    type Writer = DailyLogGuard;

    fn make_writer(&'a self) -> Self::Writer {
        DailyLogGuard {
            shared: Arc::clone(&self.shared),
        }
    }
}

struct DailyLogGuard {
    shared: Arc<DailyLogShared>,
}

impl DailyLogGuard {
    fn maybe_rotate(&self) {
        let now = (self.shared.coarse_time)();
        let next_check = self.shared.next_date_check.load(Ordering::Relaxed);
        if now < next_check
            || self
                .shared
                .next_date_check
                .compare_exchange(
                    next_check,
                    now.saturating_add(DATE_CHECK_INTERVAL_SECS),
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_err()
        {
            return;
        }

        // Only the thread which advances the coarse deadline pays for local
        // timezone conversion. The mutex is likewise off the steady-state path.
        let date = (self.shared.local_date)();
        if self.shared.active.load().date == date {
            return;
        }

        let _rotation = self
            .shared
            .rotation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.shared.active.load().date == date {
            return;
        }

        match (self.shared.open_file)(&self.shared.log_dir, date) {
            Ok(file) => self
                .shared
                .active
                .store(Arc::new(ActiveLogFile { date, file })),
            Err(error) => {
                // Keep the old append handle live so a transient rotation
                // failure cannot discard the log record. The atomic deadline
                // allows another attempt after the bounded check interval.
                eprintln!("Warning: Failed to rotate Cassy log to {date}: {error}");
            }
        }
    }
}

impl Write for DailyLogGuard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.maybe_rotate();
        let active = self.shared.active.load();
        (&active.file).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.maybe_rotate();
        let active = self.shared.active.load();
        (&active.file).flush()
    }
}

fn open_log_file(log_dir: &Path, date: chrono::NaiveDate) -> io::Result<File> {
    let log_path = log_path_for_date(log_dir, date);
    let file = File::options().create(true).append(true).open(log_path)?;
    FileExt::lock_shared(&file)?;
    Ok(file)
}

fn log_path_for_date(log_dir: &Path, date: chrono::NaiveDate) -> PathBuf {
    log_dir.join(format!("cas-{}.log", date.format("%Y-%m-%d")))
}

fn coarse_unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Clean up old log files based on retention policy
pub fn cleanup_old_logs(log_dir: &Path, retention_days: u32) -> io::Result<usize> {
    cleanup_old_logs_with_hooks(log_dir, retention_days, |_, _| {})
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CleanupStage {
    BeforeMetadata,
    BeforeRemove,
}

fn cleanup_old_logs_with_hooks(
    log_dir: &Path,
    retention_days: u32,
    mut hook: impl FnMut(&Path, CleanupStage),
) -> io::Result<usize> {
    let mut removed = 0;
    let now = SystemTime::now();
    let cutoff = now
        .checked_sub(Duration::from_secs(u64::from(retention_days) * 86_400))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let active_path = log_path_for_date(log_dir, chrono::Local::now().date_naive());

    for entry in fs::read_dir(log_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path == active_path {
            continue;
        }

        // Only consider cas-YYYY-MM-DD.log files
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let Some(date_str) = name
                .strip_prefix("cas-")
                .and_then(|name| name.strip_suffix(".log"))
            else {
                continue;
            };
            if chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d").is_err() {
                continue;
            }
            hook(&path, CleanupStage::BeforeMetadata);
            let modified = match entry.metadata().and_then(|metadata| metadata.modified()) {
                Ok(modified) => modified,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if modified < cutoff {
                let candidate = match File::options().read(true).write(true).open(&path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                match FileExt::try_lock_exclusive(&candidate) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                    Err(error) => return Err(error),
                }
                hook(&path, CleanupStage::BeforeRemove);
                let remove_result = fs::remove_file(&path);
                // cas-cef2: attempt LOCK_UN on every remove outcome before
                // returning or continuing through the cleanup scan.
                let unlock_result = FileExt::unlock(&candidate);
                match remove_result {
                    Ok(()) => removed += 1,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                }
                unlock_result?;
            }
        }
    }

    Ok(removed)
}

/// Logging configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoggingConfig {
    /// Whether file-based logging is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Log directory (relative to .cas/)
    #[serde(default = "default_log_dir")]
    pub log_dir: String,

    /// Log level: trace, debug, info, warn, error
    #[serde(default = "default_level")]
    pub level: String,

    /// Days to retain log files
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

fn default_true() -> bool {
    true
}

fn default_log_dir() -> String {
    "logs".to_string()
}

fn default_level() -> String {
    "info".to_string()
}

fn default_retention_days() -> u32 {
    7
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            log_dir: default_log_dir(),
            level: default_level(),
            retention_days: default_retention_days(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::logging::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    #[test]
    fn test_logging_config_defaults() {
        let config = LoggingConfig::default();
        assert!(config.enabled);
        assert_eq!(config.log_dir, "logs");
        assert_eq!(config.level, "info");
        assert_eq!(config.retention_days, 7);
    }

    #[test]
    fn daily_log_writer_creates_current_date_file() {
        let dir = tempdir().unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 7, 29).unwrap();
        let writer = DailyLogWriter::new_with_clocks_and_opener(
            dir.path(),
            move || date,
            || 0,
            open_log_file,
        )
        .unwrap();
        let mut guard = writer.make_writer();
        guard.write_all(b"current\n").unwrap();
        guard.flush().unwrap();

        assert_eq!(
            fs::read_to_string(log_path_for_date(dir.path(), date)).unwrap(),
            "current\n"
        );
    }

    #[test]
    fn daily_log_writer_reopens_when_local_date_changes() {
        let dir = tempdir().unwrap();
        let first_date = chrono::NaiveDate::from_ymd_opt(2026, 7, 28).unwrap();
        let second_date = chrono::NaiveDate::from_ymd_opt(2026, 7, 29).unwrap();
        let clock = Arc::new(Mutex::new(first_date));
        let coarse_time = Arc::new(AtomicU64::new(0));
        let writer_clock = Arc::clone(&clock);
        let writer_time = Arc::clone(&coarse_time);
        let writer = DailyLogWriter::new_with_clocks_and_opener(
            dir.path(),
            move || *writer_clock.lock().unwrap(),
            move || writer_time.load(Ordering::Relaxed),
            open_log_file,
        )
        .unwrap();
        let mut guard = writer.make_writer();

        guard.write_all(b"before midnight\n").unwrap();
        *clock.lock().unwrap() = second_date;
        coarse_time.store(2, Ordering::Relaxed);
        guard.write_all(b"after midnight\n").unwrap();
        guard.flush().unwrap();

        assert_eq!(
            fs::read_to_string(log_path_for_date(dir.path(), first_date)).unwrap(),
            "before midnight\n"
        );
        assert_eq!(
            fs::read_to_string(log_path_for_date(dir.path(), second_date)).unwrap(),
            "after midnight\n"
        );
    }

    #[test]
    fn daily_log_writer_steady_state_avoids_local_date_lookup() {
        let dir = tempdir().unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 7, 29).unwrap();
        let date_calls = Arc::new(AtomicUsize::new(0));
        let writer_calls = Arc::clone(&date_calls);
        let writer = DailyLogWriter::new_with_clocks_and_opener(
            dir.path(),
            move || {
                writer_calls.fetch_add(1, Ordering::Relaxed);
                date
            },
            || 0,
            open_log_file,
        )
        .unwrap();
        let mut guard = writer.make_writer();

        for _ in 0..100 {
            guard.write_all(b"steady state\n").unwrap();
        }

        assert_eq!(
            date_calls.load(Ordering::Relaxed),
            1,
            "only construction should convert the local date before the coarse deadline"
        );
    }

    #[test]
    fn daily_log_writer_falls_back_and_retries_after_rotation_failure() {
        let dir = tempdir().unwrap();
        let first_date = chrono::NaiveDate::from_ymd_opt(2026, 7, 28).unwrap();
        let second_date = chrono::NaiveDate::from_ymd_opt(2026, 7, 29).unwrap();
        let date = Arc::new(Mutex::new(first_date));
        let coarse_time = Arc::new(AtomicU64::new(0));
        let fail_rotation = Arc::new(AtomicBool::new(true));
        let writer_date = Arc::clone(&date);
        let writer_time = Arc::clone(&coarse_time);
        let writer_failure = Arc::clone(&fail_rotation);
        let writer = DailyLogWriter::new_with_clocks_and_opener(
            dir.path(),
            move || *writer_date.lock().unwrap(),
            move || writer_time.load(Ordering::Relaxed),
            move |log_dir, requested_date| {
                if requested_date == second_date && writer_failure.load(Ordering::Relaxed) {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected rotation failure",
                    ))
                } else {
                    open_log_file(log_dir, requested_date)
                }
            },
        )
        .unwrap();
        let mut guard = writer.make_writer();

        guard.write_all(b"before midnight\n").unwrap();
        *date.lock().unwrap() = second_date;
        coarse_time.store(2, Ordering::Relaxed);
        guard.write_all(b"rotation failed but retained\n").unwrap();
        assert_eq!(
            fs::read_to_string(log_path_for_date(dir.path(), first_date)).unwrap(),
            "before midnight\nrotation failed but retained\n"
        );

        fail_rotation.store(false, Ordering::Relaxed);
        coarse_time.store(4, Ordering::Relaxed);
        guard.write_all(b"rotation retried\n").unwrap();
        guard.flush().unwrap();
        assert_eq!(
            fs::read_to_string(log_path_for_date(dir.path(), second_date)).unwrap(),
            "rotation retried\n"
        );
    }

    #[test]
    fn cleanup_old_logs_uses_mtime_and_preserves_active_file() {
        let dir = tempdir().unwrap();
        let now = SystemTime::now();

        let stale_path = dir.path().join("cas-2026-01-01.log");
        fs::write(&stale_path, "stale log").unwrap();
        filetime::set_file_mtime(
            &stale_path,
            filetime::FileTime::from_system_time(now - Duration::from_secs(10 * 86_400)),
        )
        .unwrap();

        // A daemon can still have an old-dated path open during rollout. A
        // recent mtime proves it is hot even though its filename is ancient.
        let hot_old_date = chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
        let hot_old_path = log_path_for_date(dir.path(), hot_old_date);
        let hot_writer = DailyLogWriter::new_with_clocks_and_opener(
            dir.path(),
            move || hot_old_date,
            || 0,
            open_log_file,
        )
        .unwrap();
        let mut hot_guard = hot_writer.make_writer();
        hot_guard.write_all(b"actively written log\n").unwrap();
        hot_guard.flush().unwrap();

        // The current date is the active destination and must be retained even
        // with an artificially stale mtime (including retention_days = 0).
        let active_path = log_path_for_date(dir.path(), chrono::Local::now().date_naive());
        fs::write(&active_path, "active log").unwrap();
        filetime::set_file_mtime(
            &active_path,
            filetime::FileTime::from_system_time(now - Duration::from_secs(30 * 86_400)),
        )
        .unwrap();

        let removed = cleanup_old_logs(dir.path(), 0).unwrap();
        assert_eq!(removed, 1);
        assert!(!stale_path.exists());
        assert!(hot_old_path.exists());
        assert!(active_path.exists());
        hot_guard.write_all(b"still active\n").unwrap();
        hot_guard.flush().unwrap();
        assert_eq!(
            fs::read_to_string(hot_old_path).unwrap(),
            "actively written log\nstill active\n"
        );
    }

    #[test]
    fn cleanup_old_logs_skips_files_unlinked_before_metadata() {
        let dir = tempdir().unwrap();
        let raced_path = dir.path().join("cas-2020-01-01.log");
        let removable_path = dir.path().join("cas-2020-01-02.log");
        fs::write(&raced_path, "raced").unwrap();
        fs::write(&removable_path, "removable").unwrap();

        let removed = cleanup_old_logs_with_hooks(dir.path(), 0, |path, stage| {
            if path == raced_path && stage == CleanupStage::BeforeMetadata {
                fs::remove_file(path).unwrap();
            }
        })
        .unwrap();

        assert_eq!(removed, 1);
        assert!(!raced_path.exists());
        assert!(!removable_path.exists());
    }

    #[test]
    fn cleanup_old_logs_skips_files_unlinked_before_remove() {
        let dir = tempdir().unwrap();
        let raced_path = dir.path().join("cas-2020-01-01.log");
        let removable_path = dir.path().join("cas-2020-01-02.log");
        fs::write(&raced_path, "raced").unwrap();
        fs::write(&removable_path, "removable").unwrap();

        let removed = cleanup_old_logs_with_hooks(dir.path(), 0, |path, stage| {
            if path == raced_path && stage == CleanupStage::BeforeRemove {
                fs::remove_file(path).unwrap();
            }
        })
        .unwrap();

        assert_eq!(removed, 1);
        assert!(!raced_path.exists());
        assert!(!removable_path.exists());
    }
}
