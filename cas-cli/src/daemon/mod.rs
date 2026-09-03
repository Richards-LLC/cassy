//! Background maintenance operations
//!
//! Provides maintenance tasks that run during idle time:
//! - Process pending observations
//! - Consolidate related memories
//! - Prune stale entries
//! - Apply memory decay
//! - Generate embeddings
//! - Index code files (via file watcher)
//!
//! Note: The standalone daemon has been removed. Maintenance now runs via:
//! - Embedded daemon in the MCP server (automatic, idle-based)
//! - `cas daemon run` for one-off maintenance runs

pub mod queue;
pub mod watcher;

mod decay;
// cas-499c: `cas index code` (cli::index_cmd) and the doctor lag check need the repository
// derivation and the on-demand indexer, not just the daemon's re-exports.
pub mod indexing;
pub mod relevance;
mod maintenance;
mod observation;
pub(crate) mod source_text;
#[cfg(test)]
mod tests;
mod types;

pub use indexing::{
    index_code_files, reconcile_code_tree, run_code_index_cycle, run_embedding_cycle,
    run_indexing_cycle,
};
pub(crate) use maintenance::heartbeat_stale_agent_should_be_reaped;
pub use maintenance::{run_maintenance, run_once};
pub use queue::{
    MaintenanceTask, TaskQueue, TaskType, global_queue, queue_embedding_task,
    queue_observation_task, queue_scheduled_maintenance,
};
pub use types::{
    CodeIndexResult, DaemonConfig, DaemonRunResult, DaemonStatus, EmbeddingResult,
    MemoryDecayStatus,
};
pub use watcher::{CodeWatcher, WatchEvent, WatcherConfig};
