//! Context scorer using BM25 search
//!
//! This module provides a ContextScorer implementation that uses BM25
//! text search combined with temporal scoring for context selection.
//! Semantic/embedding-based scoring has been removed.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use cas_core::hooks::{BasicContextScorer, ContextQuery, ContextScorer};
use cas_types::Entry;

use crate::error::Result;
use crate::hybrid_search::{HybridSearch, HybridSearchOptions, SearchOptions};

/// Context scorer using BM25 + temporal search
///
/// Uses BM25 text matching and temporal signals to score entries for
/// context selection. Falls back to basic scoring if search fails.
pub struct HybridContextScorer {
    hybrid_search: HybridSearch,
}

impl HybridContextScorer {
    /// Create a new hybrid context scorer
    pub fn new(hybrid_search: HybridSearch) -> Self {
        Self { hybrid_search }
    }

    /// Try to open hybrid context scorer from a Cassy directory
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let hybrid_search = HybridSearch::open(cas_dir)?;
        Ok(Self { hybrid_search })
    }

    /// Try to open with graph retriever for better context awareness
    pub fn open_with_graph(cas_dir: &Path) -> Result<Self> {
        let hybrid_search = HybridSearch::open_with_graph(cas_dir)?;
        Ok(Self { hybrid_search })
    }

    /// Score entries using hybrid search
    fn score_with_hybrid(&self, entries: &[Entry], query: &str) -> Result<Vec<(String, f32)>> {
        if query.trim().is_empty() || entries.is_empty() {
            return Ok(Vec::new());
        }

        let opts = HybridSearchOptions {
            base: SearchOptions {
                query: query.to_string(),
                limit: entries.len() * 2, // Get more results for better coverage
                ..Default::default()
            },
            enable_semantic: false, // Disabled - using BM25 only
            enable_temporal: true,
            enable_graph: self.hybrid_search.has_graph_retriever(),
            enable_code: false,   // Not relevant for memory context
            enable_rerank: false, // Skip for performance
            use_adaptive_weights: true,
            calibrate_scores: true,
            ..Default::default()
        };

        let results = self.hybrid_search.search(&opts, entries)?;

        Ok(results
            .into_iter()
            .map(|r| (r.id, r.score as f32))
            .collect())
    }
}

/// Reward an entry that matches several distinctive terms from the active
/// context query. Hybrid search already blends lexical relevance with recency
/// and importance, but a recently-created generic entry can otherwise crowd a
/// task-specific memory out of the compact ambient-memory window.
///
/// `pub(crate)` for the retrieval-eval harness, whose scorer-swap replica calls
/// this exact function rather than copying it, so an experiment cannot silently
/// diverge from production on the bonus term.
pub(crate) fn contextual_overlap_bonus(entry: &Entry, query: &str) -> f32 {
    const COMMON_CONTEXT_TERMS: &[&str] = &[
        "about", "after", "against", "and", "are", "for", "from", "into", "must", "not", "the",
        "that", "this", "with",
    ];
    let query_terms: HashSet<String> = query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| term.chars().count() >= 3)
        .map(str::to_lowercase)
        .filter(|term| !COMMON_CONTEXT_TERMS.contains(&term.as_str()))
        .collect();
    if query_terms.is_empty() {
        return 0.0;
    }

    let mut entry_text = entry.content.clone();
    if let Some(title) = &entry.title {
        entry_text.push(' ');
        entry_text.push_str(title);
    }
    let entry_terms: HashSet<String> = entry_text
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| term.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect();
    let matching_terms = query_terms.intersection(&entry_terms).count();

    // One common word such as "report" is not enough to make a memory
    // task-specific. Multiple overlapping terms are durable evidence that it
    // belongs in the focused ambient context.
    if matching_terms < 2 {
        return 0.0;
    }

    // Score the evidence itself rather than diluting it by the full query
    // length. Task context commonly repeats title terms in the description and
    // appends the cwd, neither of which should weaken a genuine lexical match.
    // Cap the boost so preferences retain their stronger type weighting.
    (0.2 * matching_terms as f32).min(1.0)
}

impl ContextScorer for HybridContextScorer {
    fn score_entries(&self, entries: &[Entry], context: &ContextQuery) -> Vec<(Entry, f32)> {
        let query = context.to_query_string();

        // If we don't have meaningful context, fall back to basic scoring
        if !context.has_content() || query.trim().is_empty() {
            return BasicContextScorer.score_entries(entries, context);
        }

        // Try hybrid search scoring.
        let mut scored = match self.score_with_hybrid(entries, &query) {
            Ok(hybrid_scores) if !hybrid_scores.is_empty() => {
                // Build lookup map for hybrid scores
                let hybrid_map: HashMap<&str, f32> = hybrid_scores
                    .iter()
                    .map(|(id, score)| (id.as_str(), *score))
                    .collect();

                // Combine hybrid scores with basic scores for entries not in hybrid results
                let scored: Vec<(Entry, f32)> = entries
                    .iter()
                    .map(|e| {
                        let hybrid_score = hybrid_map.get(e.id.as_str()).copied();
                        let basic_score = BasicContextScorer::calculate_score(e);

                        // Use hybrid score if available, otherwise fall back to basic
                        // Scale basic score to be comparable (basic scores are typically 0.5-3.0)
                        let final_score = match hybrid_score {
                            Some(hs) => {
                                // Blend hybrid and basic: 70% hybrid, 30% basic
                                // This preserves some of the feedback/importance signals
                                let basic_normalized = (basic_score / 3.0).min(1.0);
                                hs * 0.7 + basic_normalized * 0.3
                            }
                            None => {
                                // Entry not in hybrid results - use basic with penalty
                                let basic_normalized = (basic_score / 3.0).min(1.0);
                                basic_normalized * 0.3 // Lower score for non-matching entries
                            }
                        };

                        (e.clone(), final_score)
                    })
                    .collect();

                scored
            }
            Ok(_) | Err(_) => {
                // Hybrid search returned empty or failed - use basic scoring
                BasicContextScorer.score_entries(entries, context)
            }
        };

        // Apply this after either hybrid or basic scoring, so task-focused
        // context remains reliable while an index is warming or unavailable.
        for (entry, score) in &mut scored {
            *score += contextual_overlap_bonus(entry, &query);
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }

    fn name(&self) -> &'static str {
        "hybrid"
    }
}

#[cfg(test)]
mod tests {
    use crate::hooks::scorer::*;
    use cas_types::EntryType;

    #[test]
    fn test_context_query_empty() {
        let context = ContextQuery::default();
        assert!(!context.has_content());
        assert!(
            context.to_query_string().is_empty() || context.to_query_string().trim().is_empty()
        );
    }

    #[test]
    fn test_context_query_with_task() {
        let context = ContextQuery {
            task_titles: vec!["Implement feature X".to_string()],
            cwd: "/project".to_string(),
            user_prompt: None,
            recent_files: vec![],
            git_branch: None,
        };
        assert!(context.has_content());
        assert!(context.to_query_string().contains("Implement feature X"));
    }

    #[test]
    fn test_fallback_to_basic() {
        // Without a real hybrid search, we can only test the fallback behavior
        let entries = vec![Entry {
            id: "1".to_string(),
            entry_type: EntryType::Learning,
            created: chrono::Utc::now(),
            ..Default::default()
        }];

        // Empty context should use basic scoring
        let context = ContextQuery::default();
        let scored = BasicContextScorer.score_entries(&entries, &context);
        assert_eq!(scored.len(), 1);
        assert!(scored[0].1 > 0.0);
    }

    #[test]
    fn contextual_overlap_bonus_prioritizes_multi_term_task_matches() {
        let task_memory = Entry {
            content: "Gate restoration must preserve the dual identity report contract.".into(),
            importance: 0.1,
            ..Default::default()
        };
        let generic_memory = Entry {
            content: "Generic historical process note with no report-specific guidance.".into(),
            importance: 0.95,
            ..Default::default()
        };
        let query = "Gate 2D restoration report with dual-identity checks Produce HTML report \
                     deliverables covering Gate 2D restoration and dual-identity .tmp-project";

        let task_score = BasicContextScorer::calculate_score(&task_memory)
            + contextual_overlap_bonus(&task_memory, query);
        let generic_score = BasicContextScorer::calculate_score(&generic_memory)
            + contextual_overlap_bonus(&generic_memory, query);

        assert!(
            task_score > generic_score,
            "a multi-term task match ({task_score}) must outrank a high-importance generic memory ({generic_score}) when hybrid search falls back"
        );
        assert_eq!(contextual_overlap_bonus(&generic_memory, query), 0.0);
    }

    /// cas-3b80 acceptance criterion 1, on the production wiring.
    ///
    /// `hooks/context.rs` builds `ContextStores` and hands it to
    /// `build_context_with_stores` with `HybridContextScorer` as the
    /// `entry_scorer`; this drives exactly that call. Before this task the
    /// assertion below was impossible to satisfy for a fresh session: with no
    /// in-progress task and no prompt of its own, `ContextQuery::has_content()`
    /// was false, the scorer early-returned onto `BasicContextScorer`, and
    /// every session in a project received byte-identical Helpful Memories.
    #[test]
    fn two_different_sessions_get_different_helpful_memories() {
        use cas_core::hooks::context::{ContextStores, SurfacedItemCallback};
        use cas_store::{SqliteStore, Store};
        use cas_types::Entry;
        use std::sync::{Arc, Mutex};

        let temp = tempfile::tempdir().expect("tempdir");
        let cas_dir = temp.path();
        let store = SqliteStore::open(cas_dir).expect("open store");
        store.init().expect("init store");

        let corpus = [
            (
                "mem-retrieval",
                "Retrieval ranking regressions are caught by the labeled retrieval eval harness precision baseline",
            ),
            (
                "mem-release",
                "Release tag pushes must announce the digest before the release workflow promotes it",
            ),
            (
                "mem-hooks",
                "SessionStart hooks open every store once and must never block the harness critical path",
            ),
            (
                "mem-sqlite",
                "SQLite writers serialize under one lock, so a busy timeout beats a retry loop",
            ),
            (
                "mem-slack",
                "Slack release notes need a user thread and a dev thread with Was and Now framing",
            ),
            (
                "mem-worktree",
                "Worktree cleanup deletes the branch only when the lane is finished",
            ),
        ];
        for (id, content) in corpus {
            let entry = Entry {
                id: id.to_string(),
                content: content.to_string(),
                entry_type: cas_types::EntryType::Learning,
                created: chrono::Utc::now(),
                ..Default::default()
            };
            store.add(&entry).expect("add entry");
        }

        let scorer = HybridContextScorer::open_with_graph(cas_dir).expect("open scorer");
        let config = crate::config::Config::default();

        let rank = |carried_prompt: &str| -> Vec<String> {
            let surfaced: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&surfaced);
            let callback: SurfacedItemCallback =
                Box::new(move |id: &str, item_type: &str, _preview| {
                    if item_type.eq_ignore_ascii_case("memory")
                        && let Ok(mut ids) = sink.lock()
                    {
                        ids.push(id.to_string());
                    }
                });

            let mut stores = ContextStores::empty();
            stores.project_store = Some(&store as &dyn Store);
            stores.entry_scorer = Some(&scorer as &dyn ContextScorer);
            stores.carried_prompt = Some(carried_prompt.to_string());

            let input = cas_core::hooks::types::HookInput {
                session_id: "scorer-test".to_string(),
                cwd: cas_dir.display().to_string(),
                hook_event_name: "SessionStart".to_string(),
                ..Default::default()
            };
            cas_core::hooks::context::build_context_with_stores(
                &input,
                &stores,
                &config,
                5,
                Some(&callback),
                "mcp__cas__",
            )
            .expect("build context");
            let ids = surfaced.lock().expect("lock").clone();
            ids
        };

        let retrieval_session =
            rank("Fix the retrieval eval harness so the ranking baseline reflects precision");
        let release_session =
            rank("Announce the release digest in Slack before the tag workflow promotes it");

        assert!(
            !retrieval_session.is_empty() && !release_session.is_empty(),
            "the Helpful Memories section must not be silent"
        );
        assert_ne!(
            retrieval_session, release_session,
            "two sessions about different work must not receive the same memory ranking"
        );
    }
}
