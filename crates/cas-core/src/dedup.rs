//! Deduplication using BM25 text similarity
//!
//! Uses BM25 to detect similar entries based on text matching.
//!
//! # Architecture
//!
//! This module provides BM25-based deduplication via the `Deduplicator` struct.
//! It requires a search index that implements the `SearchIndexTrait` trait.
//!
//! # Example
//!
//! ```rust,ignore
//! use cas_core::dedup::{Deduplicator, SimilarityResult};
//!
//! let dedup = Deduplicator::new(&store, &search);
//! let similar = dedup.find_similar("test content", 0.5)?;
//! ```

use chrono::Utc;

use cas_store::Store;
use cas_types::Entry;

use crate::error::CoreError;

/// Result of similarity check
#[derive(Debug, Clone)]
pub struct SimilarityResult {
    /// The similar entry
    pub entry: Entry,
    /// Similarity score (0.0-1.0)
    pub similarity: f64,
}

/// Trait for search indexes that support BM25-style search
///
/// This trait abstracts over the concrete SearchIndex implementation
/// to allow the dedup module to work with any compatible search backend.
pub trait SearchIndexTrait: Send + Sync {
    /// Search for entries matching a query
    ///
    /// Returns a list of (entry_id, bm25_score) pairs.
    fn search_for_dedup(
        &self,
        query: &str,
        limit: usize,
        entries: &[Entry],
    ) -> Result<Vec<SearchHit>, CoreError>;

    /// Search for top-N BM25 candidates constrained to a single memory
    /// `module`. Used by overlap detection (cas-7b1e / Unit 3) as the primary
    /// candidate retrieval path when the incoming memory has a structured
    /// module field — same-module candidates are the only valid pool.
    ///
    /// Implementations that do not index structured frontmatter should
    /// return `Ok(vec![])` (the default) so overlap detection degrades to
    /// its legacy dedup path.
    fn search_candidates_by_module(
        &self,
        _query: &str,
        _module: &str,
        _limit: usize,
    ) -> Result<Vec<SearchHit>, CoreError> {
        Ok(Vec::new())
    }
}

/// A search hit from BM25 search
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Entry ID
    pub id: String,
    /// BM25 score
    pub bm25_score: f64,
}

/// Deduplicator for finding and handling similar entries using BM25 search
pub struct Deduplicator<'a, S: SearchIndexTrait> {
    store: &'a dyn Store,
    search: &'a S,
}

impl<'a, S: SearchIndexTrait> Deduplicator<'a, S> {
    /// Create a new deduplicator
    pub fn new(store: &'a dyn Store, search: &'a S) -> Self {
        Self { store, search }
    }

    /// Find entries similar to the given content
    ///
    /// Uses normalized BM25 scores - the highest scoring entry gets 1.0,
    /// and others are scaled proportionally.
    pub fn find_similar(
        &self,
        content: &str,
        threshold: f64,
    ) -> Result<Vec<SimilarityResult>, CoreError> {
        let entries = self.store.list()?;

        if entries.is_empty() {
            return Ok(Vec::new());
        }

        let results = self.search.search_for_dedup(content, 10, &entries)?;

        if results.is_empty() {
            return Ok(Vec::new());
        }

        // Normalize scores - max score becomes 1.0
        let max_score = results.iter().map(|r| r.bm25_score).fold(0.0_f64, f64::max);

        if max_score == 0.0 {
            return Ok(Vec::new());
        }

        let mut similar = Vec::new();

        for result in results {
            let normalized = result.bm25_score / max_score;

            if normalized >= threshold {
                if let Ok(entry) = self.store.get(&result.id) {
                    similar.push(SimilarityResult {
                        entry,
                        similarity: normalized,
                    });
                }
            }
        }

        Ok(similar)
    }

    /// Merge new content into an existing entry
    pub fn merge_content(&self, entry_id: &str, new_content: &str) -> Result<(), CoreError> {
        let mut entry = self.store.get(entry_id)?;

        // Append new content with separator
        entry.content = format!("{}\n\n---\n\n{}", entry.content, new_content);
        entry.last_accessed = Some(Utc::now());

        self.store.update(&entry)?;
        Ok(())
    }

    /// Check if content is a duplicate (similarity above threshold)
    pub fn is_duplicate(&self, content: &str, threshold: f64) -> Result<bool, CoreError> {
        let similar = self.find_similar(content, threshold)?;
        Ok(!similar.is_empty())
    }

    /// Find the most similar entry
    pub fn find_most_similar(
        &self,
        content: &str,
        threshold: f64,
    ) -> Result<Option<SimilarityResult>, CoreError> {
        let similar = self.find_similar(content, threshold)?;
        Ok(similar.into_iter().next())
    }
}

/// Deduplication action to take
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupAction {
    /// Add as new entry
    Add,
    /// Skip (duplicate exists)
    Skip,
    /// Merge with existing entry
    Merge,
}

/// Result of deduplication check
#[derive(Debug, Clone)]
pub struct DedupResult {
    /// Recommended action
    pub action: DedupAction,
    /// Similar entry if found
    pub similar: Option<SimilarityResult>,
}

impl DedupResult {
    /// Check if content should be added
    pub fn should_add(&self) -> bool {
        self.action == DedupAction::Add
    }

    /// Check if content should be skipped
    pub fn should_skip(&self) -> bool {
        self.action == DedupAction::Skip
    }

    /// Check if content should be merged
    pub fn should_merge(&self) -> bool {
        self.action == DedupAction::Merge
    }
}

/// Check what to do with new content based on similarity
// gate-audit[force]: selector Chooses deduplication policy for a memory write; not factory task/agent admission.
// gate-audit[skip_if_similar]: selector Chooses memory-write deduplication behavior; not a factory policy waiver.
pub fn check_dedup<S: SearchIndexTrait>(
    store: &dyn Store,
    search: &S,
    content: &str,
    threshold: f64,
    force: bool,
    skip_if_similar: bool,
    merge_if_similar: bool,
) -> Result<DedupResult, CoreError> {
    if force {
        return Ok(DedupResult {
            action: DedupAction::Add,
            similar: None,
        });
    }

    let dedup = Deduplicator::new(store, search);
    let similar = dedup.find_most_similar(content, threshold)?;

    if let Some(similar) = similar {
        let action = if skip_if_similar {
            DedupAction::Skip
        } else if merge_if_similar {
            DedupAction::Merge
        } else {
            // Interactive mode would ask here
            // For now, return the similar entry and let caller decide
            DedupAction::Skip
        };

        Ok(DedupResult {
            action,
            similar: Some(similar),
        })
    } else {
        Ok(DedupResult {
            action: DedupAction::Add,
            similar: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::dedup::*;

    #[test]
    fn test_dedup_action_equality() {
        assert_eq!(DedupAction::Add, DedupAction::Add);
        assert_ne!(DedupAction::Add, DedupAction::Skip);
        assert_ne!(DedupAction::Skip, DedupAction::Merge);
    }

    #[test]
    fn test_dedup_result_methods() {
        let result = DedupResult {
            action: DedupAction::Add,
            similar: None,
        };
        assert!(result.should_add());
        assert!(!result.should_skip());
        assert!(!result.should_merge());

        let result = DedupResult {
            action: DedupAction::Skip,
            similar: None,
        };
        assert!(!result.should_add());
        assert!(result.should_skip());
        assert!(!result.should_merge());

        let result = DedupResult {
            action: DedupAction::Merge,
            similar: None,
        };
        assert!(!result.should_add());
        assert!(!result.should_skip());
        assert!(result.should_merge());
    }

    #[test]
    fn test_search_hit_debug() {
        let hit = SearchHit {
            id: "test-id".to_string(),
            bm25_score: 0.95,
        };
        let debug_str = format!("{hit:?}");
        assert!(debug_str.contains("test-id"));
        assert!(debug_str.contains("0.95"));
    }

    #[test]
    fn test_similarity_result_clone() {
        let entry = Entry::new("test-id".to_string(), "test content".to_string());
        let result = SimilarityResult {
            entry,
            similarity: 0.85,
        };
        let cloned = result.clone();
        assert_eq!(cloned.entry.id, "test-id");
        assert_eq!(cloned.similarity, 0.85);
    }
}
