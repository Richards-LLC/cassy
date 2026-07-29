//! Logging infrastructure for CAS
//!
//! Provides structured logging using tracing-subscriber with multiple layers:
//! - Console layer: only when --verbose, writes to stderr
//! - File layer: always on, writes to .cas/logs/
//! - EnvFilter: respects RUST_LOG env var

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
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

            // Re-evaluate the local date for every write so a daemon that
            // survives midnight moves to the new day's file without restart.
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
    state: Arc<Mutex<DailyLogState>>,
    local_date: Arc<dyn Fn() -> chrono::NaiveDate + Send + Sync>,
}

struct DailyLogState {
    log_dir: PathBuf,
    date: chrono::NaiveDate,
    file: File,
}

impl DailyLogWriter {
    fn new(log_dir: &Path) -> io::Result<Self> {
        Self::new_with_clock(log_dir, || chrono::Local::now().date_naive())
    }

    fn new_with_clock<F>(log_dir: &Path, local_date: F) -> io::Result<Self>
    where
        F: Fn() -> chrono::NaiveDate + Send + Sync + 'static,
    {
        let local_date = Arc::new(local_date);
        let date = local_date();
        let file = open_log_file(log_dir, date)?;
        Ok(Self {
            state: Arc::new(Mutex::new(DailyLogState {
                log_dir: log_dir.to_path_buf(),
                date,
                file,
            })),
            local_date,
        })
    }
}

impl<'a> MakeWriter<'a> for DailyLogWriter {
    type Writer = DailyLogGuard;

    fn make_writer(&'a self) -> Self::Writer {
        DailyLogGuard {
            state: Arc::clone(&self.state),
            local_date: Arc::clone(&self.local_date),
        }
    }
}

struct DailyLogGuard {
    state: Arc<Mutex<DailyLogState>>,
    local_date: Arc<dyn Fn() -> chrono::NaiveDate + Send + Sync>,
}

impl DailyLogGuard {
    fn with_current_file<T>(
        &self,
        operation: impl FnOnce(&mut File) -> io::Result<T>,
    ) -> io::Result<T> {
        let date = (self.local_date)();
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("daily log writer lock poisoned"))?;
        if state.date != date {
            let file = open_log_file(&state.log_dir, date)?;
            state.date = date;
            state.file = file;
        }
        operation(&mut state.file)
    }
}

impl Write for DailyLogGuard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.with_current_file(|file| file.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.with_current_file(File::flush)
    }
}

fn open_log_file(log_dir: &Path, date: chrono::NaiveDate) -> io::Result<File> {
    let log_path = log_path_for_date(log_dir, date);
    File::options().create(true).append(true).open(log_path)
}

fn log_path_for_date(log_dir: &Path, date: chrono::NaiveDate) -> PathBuf {
    log_dir.join(format!("cas-{}.log", date.format("%Y-%m-%d")))
}

/// Clean up old log files based on retention policy
pub fn cleanup_old_logs(log_dir: &Path, retention_days: u32) -> io::Result<usize> {
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
            let modified = entry.metadata()?.modified()?;
            if modified < cutoff {
                fs::remove_file(&path)?;
                removed += 1;
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
        let writer = DailyLogWriter::new_with_clock(dir.path(), move || date).unwrap();
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
        let writer_clock = Arc::clone(&clock);
        let writer = DailyLogWriter::new_with_clock(dir.path(), move || {
            *writer_clock.lock().unwrap()
        })
        .unwrap();
        let mut guard = writer.make_writer();

        guard.write_all(b"before midnight\n").unwrap();
        *clock.lock().unwrap() = second_date;
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
    fn cleanup_old_logs_uses_mtime_and_preserves_active_file() {
        let dir = tempdir().unwrap();
        let now = SystemTime::now();

        let stale_path = dir.path().join("cas-2026-01-01.log");
        fs::write(&stale_path, "stale log").unwrap();
        filetime::set_file_mtime(
            &stale_path,
            filetime::FileTime::from_system_time(
                now - Duration::from_secs(10 * 86_400),
            ),
        )
        .unwrap();

        // A daemon can still have an old-dated path open during rollout. A
        // recent mtime proves it is hot even though its filename is ancient.
        let hot_old_date = chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
        let hot_old_path = log_path_for_date(dir.path(), hot_old_date);
        let hot_writer =
            DailyLogWriter::new_with_clock(dir.path(), move || hot_old_date).unwrap();
        let mut hot_guard = hot_writer.make_writer();
        hot_guard.write_all(b"actively written log\n").unwrap();
        hot_guard.flush().unwrap();

        // The current date is the active destination and must be retained even
        // with an artificially stale mtime (including retention_days = 0).
        let active_path = log_path_for_date(dir.path(), chrono::Local::now().date_naive());
        fs::write(&active_path, "active log").unwrap();
        filetime::set_file_mtime(
            &active_path,
            filetime::FileTime::from_system_time(
                now - Duration::from_secs(30 * 86_400),
            ),
        )
        .unwrap();

        let removed = cleanup_old_logs(dir.path(), 7).unwrap();
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
}
