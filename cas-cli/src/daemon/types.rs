use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const MEMORY_DECAY_STATUS_FILE: &str = "memory-decay-status.json";

/// Counters from the most recently completed memory-decay cycle.
///
/// Embedded MCP status is process-local, but `cas doctor` runs in a separate
/// process. Keep the two counters in a small atomically-replaced sidecar so
/// doctor can report the same cycle rather than guessing from entry state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDecayStatus {
    pub recorded_at: DateTime<Utc>,
    pub curated_entries_protected: usize,
    pub promoted_on_access: usize,
}

impl MemoryDecayStatus {
    pub(crate) fn path(cas_root: &std::path::Path) -> PathBuf {
        cas_root.join(MEMORY_DECAY_STATUS_FILE)
    }

    pub(crate) fn read(cas_root: &std::path::Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(Self::path(cas_root))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    pub(crate) fn write(
        cas_root: &std::path::Path,
        curated_entries_protected: usize,
        promoted_on_access: usize,
    ) -> std::io::Result<()> {
        let status = Self {
            recorded_at: Utc::now(),
            curated_entries_protected,
            promoted_on_access,
        };
        let path = Self::path(cas_root);
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(&status)
            .map_err(|error| std::io::Error::other(format!("serialize status: {error}")))?;
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(temporary, path)
    }
}

/// Configuration for the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// How often to run maintenance tasks (minutes)
    pub interval_minutes: u64,
    /// Minimum idle time before running (minutes)
    pub min_idle_minutes: u64,
    /// Maximum entries to process per run
    pub batch_size: usize,
    /// Enable observation processing
    pub process_observations: bool,
    /// Enable memory consolidation
    pub consolidate_memories: bool,
    /// Enable automatic pruning
    pub auto_prune: bool,
    /// Enable memory decay
    pub apply_decay: bool,
    /// Importance floor for curated-memory stability protection.
    pub curated_importance_floor: f32,
    /// Promote cold/archive memories to working when accessed.
    pub promote_on_access: bool,
    /// Model for AI tasks
    pub model: String,
    /// Path to Cassy root
    pub cas_root: PathBuf,
    /// Enable entity summary generation
    pub update_entity_summaries: bool,
    /// Enable background code indexing
    pub index_code: bool,
    /// Paths to watch for code changes (relative to project root)
    pub code_watch_paths: Vec<PathBuf>,
    /// Code indexing interval (seconds)
    pub code_index_interval_secs: u64,
    /// Age (in hours) after which stale/shutdown agents are deleted (0 = never delete)
    pub agent_purge_age_hours: u64,
    /// Maximum compressed bytes retained by immutable event/recording archives.
    pub archive_max_bytes: u64,
    /// Days to retain immutable event/recording archives (0 = keep forever)
    /// (legacy compatibility; size cap is the active retention policy).
    pub archive_retention_days: u64,
    /// Enable incremental BM25 indexing
    pub index_bm25: bool,
    /// Batch size for BM25 indexing
    pub index_batch_size: usize,
    /// Maximum entries to index per run
    pub index_max_per_run: usize,
    /// BM25 indexing interval (seconds)
    pub index_interval_secs: u64,
    /// Enable injected-relevance sampling.
    pub relevance_sampling_enabled: bool,
    /// Minimum interval between relevance sampling passes (seconds).
    pub relevance_sampling_interval_secs: u64,
    /// Maximum result rows offered to the relevance judge per pass.
    pub relevance_sampling_sample_size: usize,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            interval_minutes: 30,
            min_idle_minutes: 5,
            batch_size: 20,
            process_observations: true,
            consolidate_memories: true,
            auto_prune: true,
            apply_decay: true,
            curated_importance_floor: 0.9,
            promote_on_access: true,
            model: "haiku".to_string(),
            cas_root: PathBuf::new(),
            update_entity_summaries: true,
            index_code: true,
            code_watch_paths: vec![],
            code_index_interval_secs: 30,
            agent_purge_age_hours: 24,
            archive_max_bytes: cas_store::DEFAULT_TRACE_ARCHIVE_MAX_BYTES,
            archive_retention_days: 0,
            index_bm25: true,
            index_batch_size: 32,
            index_max_per_run: 200,
            index_interval_secs: 30,
            relevance_sampling_enabled: true,
            relevance_sampling_interval_secs: 7 * 24 * 60 * 60,
            relevance_sampling_sample_size: 20,
        }
    }
}

/// Status of the daemon.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonStatus {
    /// Whether daemon is running
    pub running: bool,
    /// Last run timestamp
    pub last_run: Option<DateTime<Utc>>,
    /// Next scheduled run
    pub next_run: Option<DateTime<Utc>>,
    /// Number of observations processed
    pub observations_processed: usize,
    /// Number of memories consolidated
    pub memories_consolidated: usize,
    /// Number of entries pruned
    pub entries_pruned: usize,
    /// Number of entries with decay applied
    pub decay_applied: usize,
    /// Number of curated entries protected from stability demotion.
    pub curated_entries_protected: usize,
    /// Number of cold/archive entries promoted to working on access.
    pub promoted_on_access: usize,
    /// Last error if any
    pub last_error: Option<String>,
}

/// Result of a single daemon run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonRunResult {
    /// Start time
    pub started_at: DateTime<Utc>,
    /// End time
    pub ended_at: DateTime<Utc>,
    /// Duration in seconds
    pub duration_secs: f64,
    /// Observations processed
    pub observations_processed: usize,
    /// Consolidation suggestions applied
    pub consolidations_applied: usize,
    /// Entries pruned
    pub entries_pruned: usize,
    /// Entries with decay applied
    pub decay_applied: usize,
    /// Number of curated entries protected from stability demotion.
    pub curated_entries_protected: usize,
    /// Number of cold/archive entries promoted to working on access.
    pub promoted_on_access: usize,
    /// Entries indexed in BM25
    pub entries_indexed: usize,
    /// Indexing errors
    pub indexing_errors: Vec<String>,
    /// Entity summaries updated
    pub entity_summaries_updated: usize,
    /// Events pruned (older than retention period)
    pub events_pruned: usize,
    /// Lease history entries pruned
    pub lease_history_pruned: usize,
    /// Recordings pruned (older than retention period)
    pub recordings_pruned: usize,
    /// Immutable trace archive files evicted to enforce the size cap.
    pub trace_archives_evicted: usize,
    /// Stale agents cleaned (marked dead and leases reclaimed)
    pub agents_cleaned: usize,
    /// Old stale/shutdown agents permanently deleted
    pub agents_purged: usize,
    /// Tasks with interruption notes added (leases released while in progress)
    pub tasks_interrupted: usize,
    /// Orphaned worktrees cleaned up
    pub worktrees_cleaned: usize,
    /// Errors encountered
    pub errors: Vec<String>,
}

/// Result of embedding generation (stub for compatibility).
#[derive(Debug, Clone, Default)]
pub struct EmbeddingResult {
    /// Number of embeddings generated (always 0 - daemon removed)
    pub generated: usize,
    /// Errors encountered
    pub errors: Vec<(String, String)>,
}

/// Result of a code indexing run.
#[derive(Debug, Clone, Default)]
pub struct CodeIndexResult {
    /// Number of files indexed
    pub files_indexed: usize,
    /// Number of files deleted from index
    pub files_deleted: usize,
    /// Number of symbols indexed
    pub symbols_indexed: usize,
    /// Errors encountered
    pub errors: Vec<String>,
    /// Files deliberately not indexed because their bytes are not decodable
    /// source text, with the reason. GH #698: these are NOT failures — a
    /// failure implies a retry could succeed, and re-reading the same bytes
    /// never will, which is what made the doctor warning permanent.
    pub skipped: Vec<(std::path::PathBuf, String)>,
    /// What the closing code-vector queue reconcile changed, when one ran.
    /// `None` means no reconcile was attempted (an inner incremental call);
    /// `Some(default)` means one ran and found nothing to fix (cas-8a03).
    pub vector_reconcile: Option<cas_store::CodeVectorReconcile>,
}
