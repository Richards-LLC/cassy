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
//! use cas::search::{SearchIndex, SearchOptions, HybridSearch, HybridSearchOptions};
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
//!
//! // Hybrid search (BM25 + semantic)
//! let hybrid = HybridSearch::open(&cas_root)?;
//! let opts = HybridSearchOptions {
//!     base: opts,
//!     enable_semantic: true,
//!     bm25_weight: 0.3,
//!     semantic_weight: 0.7,
//!     ..Default::default()
//! };
//! let results = hybrid.search(&opts, &entries)?;
//! ```
//!
//! # Score Normalization
//!
//! The [`scorer`] module provides utilities for normalizing and combining
//! scores from different search methods, including Reciprocal Rank Fusion (RRF).

// Dead code check enabled - all items used

// Background indexing
pub mod background;
pub use background::{BackgroundIndexer, IndexingConfig, IndexingResult};

// Query and results caching
pub mod cache;

// CAS-specific code search (wires up generic CodeSearch with concrete types)
pub mod code;

// Entity/temporal/graph search modules (CAS-specific)
pub mod entity_search;
pub mod graph;
pub use cas_core::search::temporal;

// Re-export generic types from cas-search
pub use cas_search::{
    CodeSearch, CodeSearchOptions, CodeSearchResult, CodeSearchStats, GrepMatch, GrepOptions,
    GrepSearch,
};

pub use cache::{CacheStats, SearchCache, SearchCacheStats};
pub use code::{CasCodeSearch, code_search_available, open_code_search, open_code_search_fast};
pub use entity_search::{EntityQuery, EntitySearch, EntitySearchResult};

pub mod hybrid;
pub mod metrics;
pub mod scorer;
pub mod semantic;

pub use hybrid::{HybridSearch, HybridSearchOptions};
pub use semantic::{SemanticChannel, open_semantic_channel};
pub use metrics::{LatencyTimer, MetricsStore, SearchEvent, SearchMethod, generate_event_id};

pub mod filter_grammar;
pub mod frontmatter;
mod id_utils;
mod search_index_impl;
mod search_index_query;

use std::sync::Mutex;

use chrono::Duration;
use tantivy::{Index, IndexReader};
use tantivy::query::QueryParser;
use tantivy::schema::*;

pub use id_utils::extract_id_patterns;

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
    /// A distilled project-knowledge page (`.cas/knowledge/<type>/<slug>.md`)
    KnowledgePage,
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
            DocType::KnowledgePage => "knowledge_page",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "entry" | "entries" | "memory" | "memories" => Some(DocType::Entry),
            "task" | "tasks" => Some(DocType::Task),
            "rule" | "rules" => Some(DocType::Rule),
            "skill" | "skills" => Some(DocType::Skill),
            "spec" | "specs" | "specification" | "specifications" => Some(DocType::Spec),
            "code_symbol" | "codesymbol" | "symbol" | "symbols" => Some(DocType::CodeSymbol),
            "code_file" | "codefile" | "file" | "files" | "code" => Some(DocType::CodeFile),
            "knowledge_page" | "knowledgepage" | "knowledge" | "page" | "pages" => {
                Some(DocType::KnowledgePage)
            }
            _ => None,
        }
    }
}

/// Default memory budget for BM25 index writer (50MB)
pub const DEFAULT_WRITER_MEMORY: usize = 50_000_000;

/// Search index backed by Tantivy
pub struct SearchIndex {
    index: Index,
    schema: Schema,
    id_field: Field,
    content_field: Field,
    tags_field: Field,
    type_field: Field,
    title_field: Field,
    doc_type_field: Field,
    // Code-specific fields
    language_field: Field,
    kind_field: Field,
    file_path_field: Field,
    // Memory frontmatter fields (cas-7b1e)
    module_field: Field,
    track_field: Field,
    problem_type_field: Field,
    severity_field: Field,
    root_cause_field: Field,
    mem_date_field: Field,
    // Configuration
    writer_memory: usize,
    // Cached IndexReader (auto-reloads on commit via ReloadPolicy)
    cached_reader: Mutex<Option<IndexReader>>,
    // Cached QueryParser (index + fields don't change)
    cached_query_parser: Mutex<Option<QueryParser>>,
}

/// Options for search queries
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// The search query
    pub query: String,
    /// Maximum number of results
    pub limit: usize,
    /// Boost results by feedback score
    pub boost_feedback: bool,
    /// Boost results by recency
    pub boost_recency: bool,
    /// Boost results by importance/priority score
    pub boost_importance: bool,
    /// Half-life for recency decay
    pub recency_half_life: Duration,
    /// Filter by tags (OR logic)
    pub tags: Vec<String>,
    /// Filter by entry types
    pub types: Vec<String>,
    /// Filter by document types (entry, task, rule, skill, code_symbol)
    pub doc_types: Vec<DocType>,
    /// Include archived entries
    pub include_archived: bool,
    // Code-specific filters (only apply when doc_types includes CodeSymbol)
    /// Filter by programming language (rust, typescript, python, go, elixir)
    pub language: Option<String>,
    /// Filter by symbol kind (function, struct, trait, etc.)
    pub kind: Option<String>,
    /// Filter by file path pattern (glob-style, e.g., "src/**/*.rs")
    pub file_path: Option<String>,
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
            doc_types: Vec::new(), // Empty = all types
            include_archived: false,
            language: None,
            kind: None,
            file_path: None,
        }
    }
}

/// A search result with scoring information
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Document ID
    pub id: String,
    /// Document type (entry, task, rule, skill)
    pub doc_type: DocType,
    /// Final score after all boosts
    pub score: f64,
    /// Raw BM25 score
    pub bm25_score: f64,
    /// Score after boosts applied
    pub boosted_score: f64,
}

/// Expected number of fields in the current schema version.
/// Bump this when adding new fields to trigger automatic index rebuild.
pub(crate) const EXPECTED_FIELD_COUNT: usize = 15;

#[cfg(test)]
mod tests {
    use crate::hybrid_search::*;
    use crate::types::{Entry, Task};

    fn create_test_entry(id: &str, content: &str) -> Entry {
        Entry {
            id: id.to_string(),
            content: content.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_index_and_search() {
        let index = SearchIndex::in_memory().unwrap();

        let entries = vec![
            create_test_entry("001", "Rust is a systems programming language"),
            create_test_entry("002", "Python is good for data science"),
            create_test_entry("003", "JavaScript runs in browsers"),
        ];

        for entry in &entries {
            index.index_entry(entry).unwrap();
        }

        let opts = SearchOptions {
            query: "programming".to_string(),
            limit: 10,
            ..Default::default()
        };

        let results = index.search(&opts, &entries).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].id, "001");
    }

    #[test]
    fn test_feedback_boost() {
        let index = SearchIndex::in_memory().unwrap();

        // Entry with low BM25 score but high feedback
        let mut entry1 = create_test_entry("001", "Rust programming");
        entry1.helpful_count = 10;

        // Entry with higher BM25 score (more matching terms) but no feedback
        let entry2 = create_test_entry("002", "Rust programming language tutorial guide");

        let entries = vec![entry1, entry2];

        for entry in &entries {
            index.index_entry(entry).unwrap();
        }

        // Without boost: entry2 should rank higher due to more content
        let opts = SearchOptions {
            query: "programming".to_string(),
            limit: 10,
            boost_feedback: false,
            ..Default::default()
        };
        let results_without = index.search(&opts, &entries).unwrap();

        // With boost: entry1's feedback should help it compete or rank higher
        let opts = SearchOptions {
            query: "programming".to_string(),
            limit: 10,
            boost_feedback: true,
            ..Default::default()
        };
        let results_with = index.search(&opts, &entries).unwrap();

        // Find positions
        let pos_without = results_without.iter().position(|r| r.id == "001").unwrap();
        let pos_with = results_with.iter().position(|r| r.id == "001").unwrap();

        // With feedback boost, entry1 should rank better (lower position = better)
        // or at least maintain position
        assert!(
            pos_with <= pos_without,
            "Feedback boost should improve ranking: pos_with={pos_with}, pos_without={pos_without}"
        );
    }

    #[test]
    fn test_extract_id_patterns() {
        // Single ID
        let (ids, remaining) = extract_id_patterns("cas-8cb5");
        assert_eq!(ids, vec!["cas-8cb5"]);
        assert_eq!(remaining, "");

        // Multiple IDs
        let (ids, remaining) = extract_id_patterns("cas-8cb5 cas-4a23 cas-c6a3");
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"cas-8cb5".to_string()));
        assert!(ids.contains(&"cas-4a23".to_string()));
        assert!(ids.contains(&"cas-c6a3".to_string()));
        assert_eq!(remaining, "");

        // Mixed query with IDs and text
        let (ids, remaining) = extract_id_patterns("find cas-1234 and cas-5678 about rust");
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"cas-1234".to_string()));
        assert!(ids.contains(&"cas-5678".to_string()));
        assert_eq!(remaining, "find and about rust");

        // Rule IDs
        let (ids, _remaining) = extract_id_patterns("rule-041 rule-003");
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"rule-041".to_string()));
        assert!(ids.contains(&"rule-003".to_string()));

        // No IDs
        let (ids, remaining) = extract_id_patterns("search for rust programming");
        assert!(ids.is_empty());
        assert_eq!(remaining, "search for rust programming");

        // Case insensitive
        let (ids, _) = extract_id_patterns("CAS-ABCD cas-1234");
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"cas-abcd".to_string()));
        assert!(ids.contains(&"cas-1234".to_string()));
    }

    #[test]
    fn test_unified_search_with_id_patterns() {
        let index = SearchIndex::in_memory().unwrap();

        // Create test tasks
        let task1 = Task {
            id: "cas-1234".to_string(),
            title: "Test task one".to_string(),
            description: "First task description".to_string(),
            ..Default::default()
        };
        let task2 = Task {
            id: "cas-5678".to_string(),
            title: "Test task two".to_string(),
            description: "Second task description".to_string(),
            ..Default::default()
        };

        index.index_task(&task1).unwrap();
        index.index_task(&task2).unwrap();

        // Search by single ID
        let opts = SearchOptions {
            query: "cas-1234".to_string(),
            limit: 10,
            ..Default::default()
        };
        let results = index.search_unified(&opts).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "cas-1234");
        assert_eq!(results[0].score, 1.0); // Exact ID match

        // Search by multiple IDs
        let opts = SearchOptions {
            query: "cas-1234 cas-5678".to_string(),
            limit: 10,
            ..Default::default()
        };
        let results = index.search_unified(&opts).unwrap();
        assert_eq!(results.len(), 2);

        // Search by ID with doc_type filter
        let opts = SearchOptions {
            query: "cas-1234".to_string(),
            limit: 10,
            doc_types: vec![DocType::Task],
            ..Default::default()
        };
        let results = index.search_unified(&opts).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_type, DocType::Task);
    }
}
