//! Full-text search using Tantivy
//!
//! This module provides comprehensive search capabilities for CAS:
//!
//! - **BM25 Search** - Traditional text search using Tantivy
//! - **Hybrid Search** - Combines BM25 with semantic embeddings
//! - **Reranking** - Optional ML-powered result reranking
//!
//! # Search Features
//!
//! - Feedback boosting (higher score for helpful entries)
//! - Recency boosting (exponential decay based on age)
//! - Importance boosting (based on priority/stability scores)
//! - Tag and type filtering
//! - Multi-type search (entries, tasks, rules, skills)
//!
//! # Usage
//!
//! ```rust,ignore
//! use cas_core::search::{SearchIndex, SearchOptions};
//!
//! // BM25-only search
//! let index = SearchIndex::open(&index_dir)?;
//! let opts = SearchOptions {
//!     query: "rust testing".to_string(),
//!     limit: 10,
//!     boost_feedback: true,
//!     ..Default::default()
//! };
//! let results = index.search(&opts, &entries)?;
//! ```
//!
//! # Score Normalization
//!
//! The [`scorer`] module provides utilities for normalizing and combining
//! scores from different search methods, including Reciprocal Rank Fusion (RRF).

mod index_ops;
mod query_ops;
#[cfg(test)]
mod tests;

pub mod metrics;
pub mod scorer;
pub mod temporal;

pub use metrics::{
    LatencyTimer, MethodComparison, MethodMetrics, MetricsStore, ResultFeedback, SearchEvent,
    SearchMethod, generate_event_id,
};
pub use scorer::{
    QueryFeatures, QueryType, SearchWeights, calibrate_scores, combine_multi_channel,
    combine_scores, normalize_scores, percentile_normalize, reciprocal_rank_fusion,
    rrf_with_magnitude,
};
pub use temporal::{
    EntityHistory, EntitySnapshot, HistoryEventType, RelationshipEvent, TemporalEntryResult,
    TemporalParseError, TemporalQuery, TemporalRelation, TemporalRetriever, TimePeriod,
    filter_entities_by_time, filter_entries_by_time, filter_relationships_by_time,
    parse_date_flexible,
};

use chrono::Duration;
use regex::Regex;
use std::path::Path;
use tantivy::schema::{Field, STORED, STRING, Schema, TEXT};
use tantivy::{Index, IndexWriter};

use crate::error::CoreError;

/// Extract CAS ID patterns from a query string
/// Returns (extracted_ids, remaining_query)
/// Matches patterns like: cas-XXXX, cas-sk0a, rule-041, etc.
pub fn extract_id_patterns(query: &str) -> (Vec<String>, String) {
    // Legacy local IDs remain case-insensitive 2..=8 alphanumerics. Cloud
    // reserved task IDs are a distinct wire contract: exactly sixteen
    // *lowercase* hexadecimal characters, with word boundaries preventing a
    // prefix of a longer invalid token from being accepted.
    let re = Regex::new(
        r"\b(cas-[0-9a-f]{16}|(?i:cas-[a-z0-9]{2,8}|rule-[a-z0-9]{2,6}|skill-[a-z0-9]{2,6}))\b",
    )
    .unwrap_or_else(|_| Regex::new("$").expect("fallback regex"));

    let mut ids = Vec::new();
    let mut remaining = query.to_string();

    for cap in re.captures_iter(query) {
        if let Some(m) = cap.get(1) {
            ids.push(m.as_str().to_lowercase());
        }
    }

    remaining = re.replace_all(&remaining, "").to_string();
    remaining = remaining.split_whitespace().collect::<Vec<_>>().join(" ");

    (ids, remaining)
}

/// Document type for unified search
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocType {
    Entry,
    Task,
    Rule,
    Skill,
    Spec,
    CodeSymbol,
    CodeFile,
    Artifact,
}

impl DocType {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocType::Entry => "entry",
            DocType::Task => "task",
            DocType::Rule => "rule",
            DocType::Skill => "skill",
            DocType::Spec => "spec",
            DocType::CodeSymbol => "code_symbol",
            DocType::CodeFile => "code_file",
            DocType::Artifact => "artifact",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "entry" | "entries" | "memory" | "memories" => Some(DocType::Entry),
            "task" | "tasks" => Some(DocType::Task),
            "rule" | "rules" => Some(DocType::Rule),
            "skill" | "skills" => Some(DocType::Skill),
            "spec" | "specs" | "specification" | "specifications" => Some(DocType::Spec),
            "code" | "code_symbol" | "codesymbol" | "symbol" | "symbols" => {
                Some(DocType::CodeSymbol)
            }
            "code_file" | "codefile" | "file" | "files" => Some(DocType::CodeFile),
            "artifact" | "artifacts" => Some(DocType::Artifact),
            _ => None,
        }
    }
}

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

/// Search index backed by Tantivy
#[derive(Clone)]
pub struct SearchIndex {
    pub(crate) index: Index,
    pub(crate) schema: Schema,
    pub(crate) id_field: Field,
    pub(crate) content_field: Field,
    pub(crate) tags_field: Field,
    pub(crate) type_field: Field,
    pub(crate) title_field: Field,
    pub(crate) doc_type_field: Field,
}

/// Options for search queries
#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub query: String,
    pub limit: usize,
    pub boost_feedback: bool,
    pub boost_recency: bool,
    pub boost_importance: bool,
    pub recency_half_life: Duration,
    pub tags: Vec<String>,
    pub types: Vec<String>,
    pub doc_types: Vec<DocType>,
    pub include_archived: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: 10,
            boost_feedback: false,
            boost_recency: false,
            boost_importance: false,
            recency_half_life: Duration::days(30),
            tags: Vec::new(),
            types: Vec::new(),
            doc_types: Vec::new(),
            include_archived: false,
        }
    }
}

/// A search result with scoring information
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: String,
    pub doc_type: DocType,
    pub score: f64,
    pub bm25_score: f64,
    pub boosted_score: f64,
}

impl SearchIndex {
    /// Open or create a search index
    pub fn open(index_dir: &Path) -> Result<Self, CoreError> {
        let mut schema_builder = Schema::builder();

        let id_field = schema_builder.add_text_field("id", STRING | STORED);
        let content_field = schema_builder.add_text_field("content", TEXT);
        let tags_field = schema_builder.add_text_field("tags", TEXT);
        let type_field = schema_builder.add_text_field("type", STRING);
        let title_field = schema_builder.add_text_field("title", TEXT);
        let doc_type_field = schema_builder.add_text_field("doc_type", STRING | STORED);

        let schema = schema_builder.build();

        let index = if index_dir.exists() && index_dir.join("meta.json").exists() {
            Index::open_in_dir(index_dir).map_err(|e| CoreError::Other(e.to_string()))?
        } else {
            std::fs::create_dir_all(index_dir)?;
            Index::create_in_dir(index_dir, schema.clone())
                .map_err(|e| CoreError::Other(e.to_string()))?
        };

        Ok(Self {
            index,
            schema,
            id_field,
            content_field,
            tags_field,
            type_field,
            title_field,
            doc_type_field,
        })
    }

    /// Create an in-memory search index (for testing)
    pub fn in_memory() -> Result<Self, CoreError> {
        let mut schema_builder = Schema::builder();

        let id_field = schema_builder.add_text_field("id", STRING | STORED);
        let content_field = schema_builder.add_text_field("content", TEXT);
        let tags_field = schema_builder.add_text_field("tags", TEXT);
        let type_field = schema_builder.add_text_field("type", STRING);
        let title_field = schema_builder.add_text_field("title", TEXT);
        let doc_type_field = schema_builder.add_text_field("doc_type", STRING | STORED);

        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema.clone());

        Ok(Self {
            index,
            schema,
            id_field,
            content_field,
            tags_field,
            type_field,
            title_field,
            doc_type_field,
        })
    }

    pub(crate) fn writer(&self) -> Result<IndexWriter, CoreError> {
        self.index
            .writer(50_000_000)
            .map_err(|e| CoreError::Other(e.to_string()))
    }

    pub fn field_names(&self) -> Vec<&str> {
        self.schema
            .fields()
            .map(|(_, entry)| entry.name())
            .collect()
    }

    pub fn field_count(&self) -> usize {
        self.schema.fields().count()
    }
}
