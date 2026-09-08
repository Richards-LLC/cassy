//! File watcher for automatic code indexing
//!
//! Watches for file changes in project directories and triggers
//! incremental re-indexing of modified files.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::error::CasError;

/// Events emitted by the file watcher
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// File was created or modified
    Modified(PathBuf),
    /// File was deleted
    Deleted(PathBuf),
    /// Error occurred
    Error(String),
}

/// Configuration for the file watcher
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// Directories to watch
    pub watch_paths: Vec<PathBuf>,
    /// File extensions to watch (e.g., ["rs", "ts", "py", "go"])
    pub extensions: Vec<String>,
    /// Debounce duration (to batch rapid changes)
    pub debounce_ms: u64,
    /// Patterns to ignore (in addition to .gitignore)
    pub ignore_patterns: Vec<String>,
}

struct WatcherRuntime {
    _watcher: RecommendedWatcher,
    stop_tx: Sender<()>,
    debounce_thread: Option<JoinHandle<()>>,
}

impl Drop for WatcherRuntime {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(thread) = self.debounce_thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_debounce_loop(
    raw_rx: Receiver<notify::Result<Event>>,
    stop_rx: Receiver<()>,
    debounce_duration: Duration,
    pending_files: Arc<Mutex<HashSet<PathBuf>>>,
    event_tx: Sender<WatchEvent>,
    extensions: Vec<String>,
    ignore_patterns: Vec<String>,
) {
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
    let tick_floor = Duration::from_millis(1);
    let debounce_duration = debounce_duration.max(tick_floor);

    loop {
        if stop_rx.try_recv().is_ok() {
            break;
        }

        let wait = pending
            .values()
            .map(|updated| {
                updated
                    .checked_add(debounce_duration)
                    .unwrap_or_else(Instant::now)
                    .saturating_duration_since(Instant::now())
                    .max(tick_floor)
            })
            .min()
            .unwrap_or(debounce_duration);

        match raw_rx.recv_timeout(wait) {
            Ok(Ok(event)) => {
                let updated = Instant::now();
                for path in event.paths {
                    pending.insert(path, updated);
                }
            }
            Ok(Err(error)) => {
                let _ = event_tx.send(WatchEvent::Error(format!("Watch error: {error:?}")));
            }
            Err(RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                let ready = pending
                    .iter()
                    .filter_map(|(path, updated)| {
                        (now.duration_since(*updated) >= debounce_duration).then(|| path.clone())
                    })
                    .collect::<Vec<_>>();
                for path in ready {
                    pending.remove(&path);
                    emit_debounced_path(
                        path,
                        &pending_files,
                        &event_tx,
                        &extensions,
                        &ignore_patterns,
                    );
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn emit_debounced_path(
    path: PathBuf,
    pending_files: &Arc<Mutex<HashSet<PathBuf>>>,
    event_tx: &Sender<WatchEvent>,
    extensions: &[String],
    ignore_patterns: &[String],
) {
    // FSEvents can coalesce a recursive change to its containing directory.
    // Expand that directory so indexing remains file based.
    if path.is_dir() {
        let files = crate::daemon::indexing::collect_source_files(
            std::slice::from_ref(&path),
            extensions,
            ignore_patterns,
        );
        if let Ok(mut pending) = pending_files.lock() {
            for file in files {
                pending.insert(file.clone());
                let _ = event_tx.send(WatchEvent::Modified(file));
            }
        }
        return;
    }

    if !CodeWatcher::should_watch_path(&path, extensions, ignore_patterns) {
        return;
    }

    if let Ok(mut pending) = pending_files.lock() {
        if path.exists() {
            pending.insert(path.clone());
            let _ = event_tx.send(WatchEvent::Modified(path));
        } else {
            let _ = event_tx.send(WatchEvent::Deleted(path));
        }
    }
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            watch_paths: vec![],
            extensions: vec![
                "rs".to_string(),
                "ts".to_string(),
                "tsx".to_string(),
                "py".to_string(),
                "go".to_string(),
            ],
            debounce_ms: 500,
            ignore_patterns: vec![
                "target/".to_string(),
                "node_modules/".to_string(),
                ".git/".to_string(),
                "__pycache__/".to_string(),
                "*.pyc".to_string(),
            ],
        }
    }
}

/// File watcher that monitors directories for code changes
pub struct CodeWatcher {
    config: WatcherConfig,
    /// Pending files to be indexed (thread-safe)
    pending_files: Arc<Mutex<HashSet<PathBuf>>>,
    /// Channel to receive watch events
    event_rx: Option<Receiver<WatchEvent>>,
    /// Sender for watch events (kept alive to prevent channel close)
    _event_tx: Option<Sender<WatchEvent>>,
    /// The raw watcher and filtered debounce loop, kept alive together.
    _watcher: Option<WatcherRuntime>,
    /// The first drain is a full reconciliation, not merely a batch of
    /// watcher events. Atomic because the scheduler reads it through `&self`.
    initial_reconcile: AtomicBool,
}

impl CodeWatcher {
    /// Create a new file watcher with the given configuration
    pub fn new(config: WatcherConfig) -> Self {
        Self {
            config,
            pending_files: Arc::new(Mutex::new(HashSet::new())),
            event_rx: None,
            _event_tx: None,
            _watcher: None,
            initial_reconcile: AtomicBool::new(false),
        }
    }

    /// Start watching the configured directories
    pub fn start(&mut self) -> Result<(), CasError> {
        let (tx, rx) = channel();
        self.event_rx = Some(rx);
        self._event_tx = Some(tx.clone());

        let pending = self.pending_files.clone();
        let extensions = self.config.extensions.clone();
        let ignore_patterns = self.config.ignore_patterns.clone();
        let debounce_duration = Duration::from_millis(self.config.debounce_ms);

        let (raw_tx, raw_rx) = channel::<notify::Result<Event>>();
        let raw_extensions = extensions.clone();
        let raw_ignore_patterns = ignore_patterns.clone();
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<Event>| match result {
                Ok(mut event) => {
                    // notify's native backends include open/close events. They
                    // are filesystem reads, not code changes, and feeding them
                    // into a debounce queue is enough to keep a serve process
                    // hot while another tool scans the tree.
                    if matches!(event.kind, EventKind::Access(_)) {
                        return;
                    }
                    event.paths.retain(|path| {
                        CodeWatcher::should_watch_event_path(
                            path,
                            &raw_extensions,
                            &raw_ignore_patterns,
                        )
                    });
                    if !event.paths.is_empty() {
                        let _ = raw_tx.send(Ok(event));
                    }
                }
                Err(error) => {
                    let _ = raw_tx.send(Err(error));
                }
            },
            notify::Config::default().with_follow_symlinks(false),
        )
        .map_err(|e| {
            CasError::Io(std::io::Error::other(format!(
                "Failed to create watcher: {e}"
            )))
        })?;

        let (stop_tx, stop_rx) = channel();
        let debounce_thread = std::thread::Builder::new()
            .name("notify-rs debouncer loop".to_string())
            .spawn(move || {
                run_debounce_loop(
                    raw_rx,
                    stop_rx,
                    debounce_duration,
                    pending,
                    tx,
                    extensions,
                    ignore_patterns,
                );
            })
            .map_err(|e| {
                CasError::Io(std::io::Error::other(format!(
                    "Failed to start watcher debounce thread: {e}"
                )))
            })?;

        // Start watching each configured path
        for path in &self.config.watch_paths {
            if path.exists() {
                watcher.watch(path, RecursiveMode::Recursive).map_err(|e| {
                    CasError::Io(std::io::Error::other(format!(
                        "Failed to watch {}: {}",
                        path.display(),
                        e
                    )))
                })?;
            }
        }

        // Store the watcher and debounce thread together so shutdown signals
        // the filtered loop before joining it.
        self._watcher = Some(WatcherRuntime {
            _watcher: watcher,
            stop_tx,
            debounce_thread: Some(debounce_thread),
        });

        Ok(())
    }

    /// Register the watcher before taking the initial tree snapshot.
    ///
    /// A second snapshot closes the backend-registration window (notably on
    /// macOS FSEvents): files created while the first walk runs become part of
    /// the reconciliation even if the backend coalesces that first event.
    /// Keeping the ordering inside this API prevents startup callers from
    /// accidentally reopening the pre-registration race.
    pub fn start_with_initial_scan(
        &mut self,
        mut scan: impl FnMut() -> Vec<PathBuf>,
    ) -> Result<(), CasError> {
        self.start()?;
        let mut files = scan();
        files.extend(scan());
        self.seed_initial(files);
        Ok(())
    }

    /// Roots whose complete source set the first reconciliation represents.
    pub fn watch_paths(&self) -> &[PathBuf] {
        &self.config.watch_paths
    }

    /// Check if a path should be watched based on extension and ignore patterns
    fn should_watch_path(path: &Path, extensions: &[String], ignore_patterns: &[String]) -> bool {
        if Self::is_ignored_path(path, ignore_patterns) {
            return false;
        }

        // Check extension
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        if !extensions.contains(&ext) {
            return false;
        }

        true
    }

    /// Check whether an event path is relevant before it reaches the debounce queue.
    fn should_watch_event_path(
        path: &Path,
        extensions: &[String],
        ignore_patterns: &[String],
    ) -> bool {
        if Self::is_ignored_path(path, ignore_patterns) {
            return false;
        }
        path.is_dir() || Self::should_watch_path(path, extensions, &[])
    }

    fn is_ignored_path(path: &Path, ignore_patterns: &[String]) -> bool {
        let path_str = path.to_string_lossy();
        for pattern in ignore_patterns {
            let pattern = pattern.trim();
            if let Some(prefix) = pattern.strip_suffix("/**") {
                let prefix = prefix.trim_end_matches('/');
                let prefix_components: Vec<_> = Path::new(prefix).components().collect();
                let path_components: Vec<_> = path.components().collect();
                if !prefix_components.is_empty()
                    && path_components
                        .windows(prefix_components.len())
                        .any(|components| components == prefix_components.as_slice())
                {
                    return true;
                }
            } else if pattern.ends_with('/') {
                // Directory pattern
                if path_str.contains(pattern) {
                    return true;
                }
            } else if let Some(suffix) = pattern.strip_prefix('*') {
                // Suffix pattern (e.g., *.pyc)
                if path_str.ends_with(suffix) {
                    return true;
                }
            } else if path_str.contains(pattern) {
                return true;
            }
        }
        false
    }

    /// Get and clear pending files for indexing
    pub fn take_pending(&self) -> Vec<PathBuf> {
        if let Ok(mut pending) = self.pending_files.lock() {
            let files: Vec<PathBuf> = pending.drain().collect();
            files
        } else {
            vec![]
        }
    }

    /// Seed the first daemon pass with every currently eligible file.
    pub fn seed_initial(&self, files: impl IntoIterator<Item = PathBuf>) {
        if let Ok(mut pending) = self.pending_files.lock() {
            pending.extend(files);
            self.initial_reconcile.store(true, Ordering::Release);
        }
    }

    pub fn take_initial_reconcile(&self) -> bool {
        self.initial_reconcile.swap(false, Ordering::AcqRel)
    }

    /// Check if there are pending files
    pub fn has_pending(&self) -> bool {
        if let Ok(pending) = self.pending_files.lock() {
            !pending.is_empty()
        } else {
            false
        }
    }

    /// Try to receive the next event (non-blocking)
    pub fn try_recv(&self) -> Option<WatchEvent> {
        self.event_rx.as_ref().and_then(|rx| rx.try_recv().ok())
    }

    /// Get the number of pending files
    pub fn pending_count(&self) -> usize {
        self.pending_files.lock().map(|p| p.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use crate::daemon::watcher::*;

    #[test]
    fn test_should_watch_rust_file() {
        let extensions = vec!["rs".to_string()];
        let ignore = vec!["target/".to_string()];

        assert!(CodeWatcher::should_watch_path(
            Path::new("src/main.rs"),
            &extensions,
            &ignore,
        ));
    }

    #[test]
    fn test_should_ignore_target() {
        let extensions = vec!["rs".to_string()];
        let ignore = vec!["target/".to_string()];

        assert!(!CodeWatcher::should_watch_path(
            Path::new("target/debug/main.rs"),
            &extensions,
            &ignore,
        ));
    }

    #[test]
    fn test_should_ignore_double_star_directory_pattern() {
        let extensions = vec!["rs".to_string()];
        let ignore = vec!["target/**".to_string()];

        assert!(!CodeWatcher::should_watch_path(
            Path::new("target/debug/main.rs"),
            &extensions,
            &ignore,
        ));
    }

    #[test]
    fn test_should_ignore_wrong_extension() {
        let extensions = vec!["rs".to_string()];
        let ignore = vec![];

        assert!(!CodeWatcher::should_watch_path(
            Path::new("src/main.txt"),
            &extensions,
            &ignore,
        ));
    }

    #[test]
    fn test_try_recv_returns_none_without_start() {
        // Before start(), there's no receiver, so try_recv should return None
        let watcher = CodeWatcher::new(WatcherConfig::default());
        assert!(watcher.try_recv().is_none());
    }

    #[test]
    fn test_watch_event_debug() {
        // Ensure WatchEvent implements Debug
        let event = WatchEvent::Modified(PathBuf::from("test.rs"));
        let debug_str = format!("{event:?}");
        assert!(debug_str.contains("Modified"));
    }

    #[test]
    fn test_watch_event_clone() {
        // Ensure WatchEvent implements Clone
        let event = WatchEvent::Error("test".to_string());
        let cloned = event.clone();
        match cloned {
            WatchEvent::Error(msg) => assert_eq!(msg, "test"),
            _ => panic!("Expected Error variant"),
        }
    }

    #[test]
    fn test_watcher_config_default() {
        let config = WatcherConfig::default();
        assert!(config.watch_paths.is_empty());
        assert!(config.extensions.contains(&"rs".to_string()));
        assert!(config.extensions.contains(&"ts".to_string()));
        assert!(config.extensions.contains(&"py".to_string()));
        assert_eq!(config.debounce_ms, 500);
        assert!(config.ignore_patterns.contains(&"target/".to_string()));
        assert!(
            config
                .ignore_patterns
                .contains(&"node_modules/".to_string())
        );
    }

    #[test]
    fn test_pending_files_operations() {
        let watcher = CodeWatcher::new(WatcherConfig::default());

        // Initially no pending files
        assert!(!watcher.has_pending());
        assert_eq!(watcher.pending_count(), 0);
        assert!(watcher.take_pending().is_empty());
    }

    #[test]
    fn initial_seed_is_drained_once_as_a_reconciliation() {
        let watcher = CodeWatcher::new(WatcherConfig::default());
        watcher.seed_initial([PathBuf::from("src/a.rs"), PathBuf::from("src/b.rs")]);
        assert_eq!(watcher.pending_count(), 2);
        assert!(watcher.take_initial_reconcile());
        assert!(!watcher.take_initial_reconcile());
        assert_eq!(watcher.take_pending().len(), 2);
    }

    #[test]
    fn watcher_initial_reconciliation_captures_create_and_delete_changes() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let deleted = root.join("deleted.rs");
        let created = root.join("created.rs");
        std::fs::write(&deleted, "pub fn deleted() {}\n").unwrap();

        let mut watcher = CodeWatcher::new(WatcherConfig {
            watch_paths: vec![root],
            extensions: vec!["rs".to_string()],
            debounce_ms: 20,
            ignore_patterns: Vec::new(),
        });
        let mut first_scan = true;
        watcher
            .start_with_initial_scan(|| {
                if first_scan {
                    first_scan = false;
                    let snapshot = vec![deleted.clone()];
                    std::fs::write(&created, "pub fn created() {}\n").unwrap();
                    std::fs::remove_file(&deleted).unwrap();
                    snapshot
                } else {
                    vec![created.clone()]
                }
            })
            .unwrap();

        assert!(watcher.take_initial_reconcile());
        let pending = watcher.take_pending();
        assert!(pending.contains(&created));
        // The first snapshot lets reconciliation retire this now-missing file.
        assert!(pending.contains(&deleted));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ignored_file_activity_does_not_spin_debouncer() {
        use std::io::Write;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;
        use std::time::Duration;

        fn debouncer_ticks() -> u64 {
            let task_dir = std::path::Path::new("/proc/self/task");
            let Some(entry) = std::fs::read_dir(task_dir)
                .unwrap()
                .flatten()
                .find(|entry| {
                    // Linux exposes only the first 15 bytes of a thread name
                    // through comm, so the crate's full name is truncated.
                    std::fs::read_to_string(entry.path().join("comm"))
                        .map(|name| name.trim() == "notify-rs debou")
                        .unwrap_or(false)
                })
            else {
                return 0;
            };

            let stat = std::fs::read_to_string(entry.path().join("stat")).unwrap();
            let fields = stat.rsplit_once(") ").unwrap().1.split_whitespace();
            let fields = fields.collect::<Vec<_>>();
            fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()
        }

        let temp = tempfile::TempDir::new().unwrap();
        let project_dir = temp.path().join("project");
        let ignored_dir = temp.path().join("ignored-target");
        std::fs::create_dir(&project_dir).unwrap();
        std::fs::create_dir(&ignored_dir).unwrap();
        let ignored_file = ignored_dir.join("ignored.rs");
        std::fs::write(&ignored_file, "initial\n").unwrap();
        std::os::unix::fs::symlink(&ignored_dir, project_dir.join("target")).unwrap();

        let mut watcher = CodeWatcher::new(WatcherConfig {
            watch_paths: vec![project_dir],
            extensions: vec!["rs".to_string()],
            debounce_ms: 500,
            ignore_patterns: vec!["target/".to_string()],
        });
        watcher.start().unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let writers = (0..8)
            .map(|_| {
                let writer_stop = Arc::clone(&stop);
                let ignored_file = ignored_file.clone();
                thread::spawn(move || {
                    while !writer_stop.load(Ordering::Relaxed) {
                        let mut file = std::fs::OpenOptions::new()
                            .write(true)
                            .truncate(true)
                            .open(&ignored_file)
                            .unwrap();
                        file.write_all(b"ignored\n").unwrap();
                        drop(file);
                        let _ = std::fs::File::open(&ignored_file);
                    }
                })
            })
            .collect::<Vec<_>>();

        let before = debouncer_ticks();
        thread::sleep(Duration::from_secs(5));
        let after = debouncer_ticks();

        stop.store(true, Ordering::Relaxed);
        for writer in writers {
            writer.join().unwrap();
        }
        drop(watcher);

        let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;
        let max_ticks = (ticks_per_second * 5 / 50).max(1) + 1;
        assert!(
            after.saturating_sub(before) <= max_ticks,
            "ignored file activity used {} debouncer ticks in 5s (limit {})",
            after.saturating_sub(before),
            max_ticks
        );
    }
}
