//! Search types shared with the live cas-cli search implementation.
//!
//! Tantivy indexing, querying, scoring, and metrics are owned by
//! `cas-cli::hybrid_search`. This module only keeps the temporal search
//! primitives and durable task-artifact identifiers that are shared across
//! crate boundaries.

pub mod temporal;

pub use temporal::{
    EntityHistory, EntitySnapshot, HistoryEventType, RelationshipEvent, TemporalEntryResult,
    TemporalParseError, TemporalQuery, TemporalRelation, TemporalRetriever, TimePeriod,
    filter_entities_by_time, filter_entries_by_time, filter_relationships_by_time,
    parse_date_flexible,
};

/// Searchable text artifact owned by a task's durable artifact directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDocument {
    pub task_id: String,
    pub path: String,
    pub content: String,
}

/// Stable index identifier for one artifact path owned by one task.
pub fn artifact_document_id(task_id: &str, path: &str) -> String {
    format!("artifact::{task_id}::{path}")
}

/// Decode an artifact index identifier for user-facing search rendering.
pub fn parse_artifact_document_id(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix("artifact::")?.split_once("::")
}
