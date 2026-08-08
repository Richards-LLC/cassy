//! File watcher for automatic code indexing
//!
//! Watches for file changes in project directories and triggers
//! incremental re-indexing of modified files.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebouncedEvent, Debouncer, new_debouncer};

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
    /// The debouncer (kept alive to maintain the watcher)
    _debouncer: Option<Debouncer<RecommendedWatcher>>,
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
            _debouncer: None,
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

        // Create debounced watcher
        let debounce_duration = Duration::from_millis(self.config.debounce_ms);

        let mut debouncer = new_debouncer(
            debounce_duration,
            move |res: Result<Vec<DebouncedEvent>, _>| {
                match res {
                    Ok(events) => {
                        for event in events {
                            let path = event.path;

                            // Check if this file should be watched
                            if !Self::should_watch_path(&path, &extensions, &ignore_patterns) {
                                continue;
                            }

                            // Add to pending set
                            if let Ok(mut pending) = pending.lock() {
                                if path.exists() {
                                    pending.insert(path.clone());
                                    let _ = tx.send(WatchEvent::Modified(path));
                                } else {
                                    let _ = tx.send(WatchEvent::Deleted(path));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(WatchEvent::Error(format!("Watch error: {e:?}")));
                    }
                }
            },
        )
        .map_err(|e| {
            CasError::Io(std::io::Error::other(format!(
                "Failed to create debouncer: {e}"
            )))
        })?;

        // Start watching each configured path
        for path in &self.config.watch_paths {
            if path.exists() {
                debouncer
                    .watcher()
                    .watch(path, RecursiveMode::Recursive)
                    .map_err(|e| {
                        CasError::Io(std::io::Error::other(format!(
                            "Failed to watch {}: {}",
                            path.display(),
                            e
                        )))
                    })?;
            }
        }

        // Store the debouncer to keep it alive
        self._debouncer = Some(debouncer);

        Ok(())
    }

    /// Check if a path should be watched based on extension and ignore patterns
    fn should_watch_path(path: &Path, extensions: &[String], ignore_patterns: &[String]) -> bool {
        // Check extension
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        if !extensions.contains(&ext) {
            return false;
        }

        // Check ignore patterns
        let path_str = path.to_string_lossy();
        for pattern in ignore_patterns {
            if pattern.ends_with('/') {
                // Directory pattern
                if path_str.contains(pattern) {
                    return false;
                }
            } else if let Some(suffix) = pattern.strip_prefix('*') {
                // Suffix pattern (e.g., *.pyc)
                if path_str.ends_with(suffix) {
                    return false;
                }
            } else if path_str.contains(pattern) {
                return false;
            }
        }

        true
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
}
