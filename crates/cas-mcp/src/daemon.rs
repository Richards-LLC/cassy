//! Embedded daemon types for MCP server
//!
//! This module contains types for the embedded daemon that runs maintenance
//! tasks in the background while the MCP server is active.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Status of the embedded daemon
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmbeddedDaemonStatus {
    /// Whether the daemon is running
    pub running: bool,
    /// Last maintenance run timestamp
    pub last_maintenance: Option<DateTime<Utc>>,
    /// Last cloud sync timestamp
    pub last_cloud_sync: Option<DateTime<Utc>>,
    /// Next scheduled maintenance
    pub next_maintenance: Option<DateTime<Utc>>,
    /// Total observations processed this session
    pub observations_processed: usize,
    /// Total memory decay applied this session
    pub decay_applied: usize,
    /// Pending items in cloud sync queue
    pub cloud_sync_pending: usize,
    /// Whether cloud sync is available (user logged in)
    pub cloud_sync_available: bool,
    /// Items pushed to cloud this session
    pub cloud_items_pushed: usize,
    /// Items pulled from cloud this session
    pub cloud_items_pulled: usize,
    /// Seconds since last MCP request
    pub idle_seconds: u64,
    /// Whether currently idle (eligible for maintenance)
    pub is_idle: bool,
    /// Last error if any
    pub last_error: Option<String>,
}

/// Activity tracker for idle detection
#[derive(Debug)]
pub struct ActivityTracker {
    /// Timestamp of last MCP request (as unix timestamp millis)
    last_request: AtomicU64,
    /// Minimum idle time before running maintenance (seconds)
    min_idle_secs: u64,
}

impl ActivityTracker {
    /// Create a new activity tracker
    pub fn new(min_idle_secs: u64) -> Self {
        Self {
            last_request: AtomicU64::new(now_millis()),
            min_idle_secs,
        }
    }

    /// Record that a request was received
    pub fn touch(&self) {
        self.last_request.store(now_millis(), Ordering::SeqCst);
    }

    /// Get seconds since last request
    pub fn idle_seconds(&self) -> u64 {
        let last = self.last_request.load(Ordering::SeqCst);
        let now = now_millis();
        (now.saturating_sub(last)) / 1000
    }

    /// Check if we've been idle long enough for maintenance
    pub fn is_idle(&self) -> bool {
        self.idle_seconds() >= self.min_idle_secs
    }
}

impl Default for ActivityTracker {
    fn default() -> Self {
        Self::new(60) // 1 minute default
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Configuration for the embedded daemon
#[derive(Debug, Clone)]
pub struct EmbeddedDaemonConfig {
    /// Path to CAS root
    pub cas_root: std::path::PathBuf,
    /// How often to run full maintenance (seconds)
    pub maintenance_interval_secs: u64,
    /// How often to run cloud sync (seconds)
    pub cloud_sync_interval_secs: u64,
    /// Minimum idle time before maintenance (seconds)
    pub min_idle_secs: u64,
    /// Minimum idle time before cloud sync (seconds) — lower than maintenance
    /// because cloud sync is lightweight (gzipped HTTP POST in spawn_blocking)
    pub cloud_sync_idle_secs: u64,
    /// Enable memory decay
    pub apply_decay: bool,
    /// Enable observation processing
    pub process_observations: bool,
    /// Enable cloud sync
    pub cloud_sync_enabled: bool,
    /// Batch size for operations
    pub batch_size: usize,
    /// Days to retain immutable event/recording archives (0 = keep forever)
    pub archive_retention_days: u64,
    // === Code indexing configuration ===
    /// Enable background code indexing
    pub index_code: bool,
    /// Paths to watch for code changes (relative to project root)
    pub code_watch_paths: Vec<std::path::PathBuf>,
    /// File extensions to index
    pub code_extensions: Vec<String>,
    /// Glob patterns to exclude from indexing
    pub code_exclude_patterns: Vec<String>,
    /// Code indexing interval (seconds)
    pub code_index_interval_secs: u64,
    /// Debounce time for file watcher (milliseconds)
    pub code_debounce_ms: u64,
    // === Structural git-history index (EPIC cas-6212 / cas-7a21) ===
    /// Enable the background git-history indexing pass
    pub index_history: bool,
    /// git-history indexing interval (seconds)
    pub history_index_interval_secs: u64,
    // === GitHub + CHANGELOG docs (EPIC cas-6212 / cas-9a38) ===
    /// Enable the background GitHub/CHANGELOG doc indexing pass
    pub index_history_docs: bool,
    /// Doc indexing interval (seconds). Deliberately much longer than the git
    /// interval: this half depends on a third party's rate limits, and issue
    /// bodies change far more slowly than commits arrive (spec §8).
    pub history_docs_interval_secs: u64,
    // === Automagic embedding drain (EPIC cas-6212 / cas-db6e) ===
    /// Enable the background drain that embeds pending knowledge pages and
    /// code-history units without anyone running `cas cloud sync`.
    pub embed_drain: bool,
    /// Embedding drain interval (seconds).
    pub embed_drain_interval_secs: u64,
}

impl Default for EmbeddedDaemonConfig {
    fn default() -> Self {
        Self {
            cas_root: std::path::PathBuf::new(),
            maintenance_interval_secs: 30 * 60, // 30 minutes
            cloud_sync_interval_secs: 60,        // 1 minute
            min_idle_secs: 60,                  // 1 minute idle before maintenance
            cloud_sync_idle_secs: 10,           // 10s idle before cloud sync (lightweight)
            apply_decay: true,
            process_observations: true,
            cloud_sync_enabled: true, // Auto-sync enabled by default
            batch_size: 20,
            archive_retention_days: 0,
            // Code indexing defaults
            index_code: false, // Disabled by default (opt-in)
            code_watch_paths: vec![],
            code_extensions: vec![],
            code_exclude_patterns: vec![],
            code_index_interval_secs: 60, // 1 minute
            code_debounce_ms: 500,        // 500ms
            // git-history indexing: on by default. Unlike code indexing it
            // needs no watcher, no parser and no config — a delta pass over a
            // day's commits is bounded work (spec §4.3).
            index_history: true,
            history_index_interval_secs: 300, // 5 minutes (spec §4.3)
            // Docs: on by default, but 15 minutes rather than 5. Spec §8's
            // arithmetic — ~96 filtered queries/day against a 5,000 point/hour
            // budget — is what makes "on by default" defensible here. A repo
            // with no `gh` and no CHANGELOG records two boundaries and costs
            // nothing further.
            index_history_docs: true,
            history_docs_interval_secs: 900, // 15 minutes (spec §8)
            // On by default and gated by nothing but the capability itself: a
            // logged-out install makes no request and creates no vector store,
            // and a logged-in one should never need a human to type `cas cloud
            // sync` before its own corpus is searchable. 300 s matches the git
            // history arm — new commits become embeddable within a tick of
            // being indexed, and a full backfill (~2,100 units, spec §7.1)
            // clears in about five ticks rather than one burst.
            embed_drain: true,
            embed_drain_interval_secs: 300,
        }
    }
}

/// Result from running maintenance
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MaintenanceResult {
    /// Duration in seconds
    pub duration_secs: f64,
    /// Observations processed
    pub observations_processed: usize,
    /// Decay applied
    pub decay_applied: bool,
}

#[cfg(test)]
mod tests {
    use crate::daemon::*;

    #[test]
    fn test_activity_tracker() {
        let tracker = ActivityTracker::new(1);
        tracker.touch();
        assert!(!tracker.is_idle());
        assert!(tracker.idle_seconds() < 1);
    }

    #[test]
    fn test_daemon_config_defaults() {
        let config = EmbeddedDaemonConfig::default();
        assert_eq!(config.maintenance_interval_secs, 30 * 60);
        assert!(config.apply_decay);
    }

    #[test]
    fn test_daemon_status_default() {
        let status = EmbeddedDaemonStatus::default();
        assert!(!status.running);
        assert!(status.last_maintenance.is_none());
    }
}
