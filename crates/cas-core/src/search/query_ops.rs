use chrono::Utc;
use tantivy::ReloadPolicy;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::Value;

use crate::error::CoreError;
use crate::search::{
    DocType, SearchIndex, SearchOptions, SearchResult, calibrate_scores, extract_id_patterns,
};
use cas_types::Entry;

impl SearchIndex {
    /// Quote field-like tokens whose field name is not registered in this
    /// index schema so Tantivy treats them as free text rather than rejecting
    /// the whole query. Registered fields stay untouched and therefore retain
    /// Tantivy's normal syntax validation.
    fn literalize_unknown_field_tokens(&self, query: &str) -> String {
        query
            .split_whitespace()
            .map(|token| {
                let Some((field, _)) = token.split_once(':') else {
                    return token.to_string();
                };

                if self.schema.get_field(field).is_ok() {
                    return token.to_string();
                }

                let escaped = token.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{escaped}\"")
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Search for entries matching a query (legacy - for backward compatibility)
    pub fn search(
        &self,
        opts: &SearchOptions,
        entries: &[Entry],
    ) -> Result<Vec<SearchResult>, CoreError> {
        if opts.query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let reader = self
            .index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e: tantivy::TantivyError| CoreError::Other(e.to_string()))?;

        let searcher = reader.searcher();

        // Parse query - search content, tags, and title
        let query_parser = QueryParser::for_index(
            &self.index,
            vec![self.content_field, self.tags_field, self.title_field],
        );

        let query = query_parser
            .parse_query(&self.literalize_unknown_field_tokens(&opts.query))
            .map_err(|e| CoreError::Parse(e.to_string()))?;

        // Get more results than needed for post-filtering
        let limit = opts.limit * 3;
        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(limit))
            .map_err(|e| CoreError::Other(e.to_string()))?;

        let mut results = Vec::new();

        for (bm25_score, doc_addr) in top_docs {
            let doc: tantivy::TantivyDocument = searcher
                .doc(doc_addr)
                .map_err(|e| CoreError::Other(e.to_string()))?;

            let id = doc
                .get_first(self.id_field)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            // Find the entry for boosting
            let entry = entries.iter().find(|e| e.id == id);

            // Skip if entry not found or doesn't match filters
            let Some(entry) = entry else {
                continue;
            };

            // Skip archived unless requested
            if entry.archived && !opts.include_archived {
                continue;
            }

            // Filter by tags
            if !opts.tags.is_empty() {
                let has_tag = opts.tags.iter().any(|t| entry.tags.contains(t));
                if !has_tag {
                    continue;
                }
            }

            // Filter by types
            if !opts.types.is_empty() {
                let type_str = entry.entry_type.to_string();
                if !opts.types.contains(&type_str) {
                    continue;
                }
            }

            // Apply boosts
            let boosted = self.apply_boosts(bm25_score as f64, entry, opts);

            results.push(SearchResult {
                id,
                doc_type: DocType::Entry,
                score: boosted,
                bm25_score: bm25_score as f64,
                boosted_score: boosted,
            });
        }

        // Re-sort by boosted score
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Calibrate scores to meaningful 0-1 range
        if !results.is_empty() {
            let mut scores: Vec<(String, f64)> =
                results.iter().map(|r| (r.id.clone(), r.score)).collect();
            calibrate_scores(&mut scores);

            // Apply calibrated scores back
            let score_map: std::collections::HashMap<&str, f64> =
                scores.iter().map(|(id, s)| (id.as_str(), *s)).collect();
            for result in results.iter_mut() {
                if let Some(&cal_score) = score_map.get(result.id.as_str()) {
                    result.score = cal_score;
                    result.boosted_score = cal_score;
                }
            }
        }

        // Limit results
        results.truncate(opts.limit);

        Ok(results)
    }

    /// Unified search across all document types (entries, tasks, rules, skills)
    /// Supports direct ID lookups for patterns like "cas-XXXX", "rule-XXX", etc.
    pub fn search_unified(&self, opts: &SearchOptions) -> Result<Vec<SearchResult>, CoreError> {
        if opts.query.trim().is_empty() {
            return Ok(Vec::new());
        }

        // Extract ID patterns from query (e.g., "cas-8cb5 cas-4a23")
        let (id_patterns, remaining_query) = extract_id_patterns(&opts.query);

        let mut results = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        // 1. Direct ID lookups for extracted patterns (score = 1.0 for exact matches)
        if !id_patterns.is_empty() {
            let reader = self
                .index
                .reader_builder()
                .reload_policy(ReloadPolicy::OnCommitWithDelay)
                .try_into()
                .map_err(|e: tantivy::TantivyError| CoreError::Other(e.to_string()))?;

            let searcher = reader.searcher();

            for id_pattern in &id_patterns {
                // Search for exact ID match using term query
                let id_term = tantivy::Term::from_field_text(self.id_field, id_pattern);
                let term_query = tantivy::query::TermQuery::new(
                    id_term,
                    tantivy::schema::IndexRecordOption::Basic,
                );

                if let Ok(top_docs) = searcher.search(&term_query, &TopDocs::with_limit(1)) {
                    for (_score, doc_addr) in top_docs {
                        if let Ok(doc) = searcher.doc::<tantivy::TantivyDocument>(doc_addr) {
                            let id = doc
                                .get_first(self.id_field)
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();

                            let doc_type_str = doc
                                .get_first(self.doc_type_field)
                                .and_then(|v| v.as_str())
                                .unwrap_or("entry");

                            let doc_type = DocType::parse(doc_type_str).unwrap_or(DocType::Entry);

                            // Filter by doc_types if specified
                            if !opts.doc_types.is_empty() && !opts.doc_types.contains(&doc_type) {
                                continue;
                            }

                            if seen_ids.insert(id.clone()) {
                                results.push(SearchResult {
                                    id,
                                    doc_type,
                                    score: 1.0, // Exact ID match gets perfect score
                                    bm25_score: 1.0,
                                    boosted_score: 1.0,
                                });
                            }
                        }
                    }
                }
            }
        }

        // 2. Text search for remaining query terms (if any)
        if !remaining_query.is_empty() {
            let reader = self
                .index
                .reader_builder()
                .reload_policy(ReloadPolicy::OnCommitWithDelay)
                .try_into()
                .map_err(|e: tantivy::TantivyError| CoreError::Other(e.to_string()))?;

            let searcher = reader.searcher();

            // Parse query - search content, tags, and title
            let query_parser = QueryParser::for_index(
                &self.index,
                vec![self.content_field, self.tags_field, self.title_field],
            );

            let query = query_parser
                .parse_query(&self.literalize_unknown_field_tokens(&remaining_query))
                .map_err(|e| CoreError::Parse(e.to_string()))?;

            // Get more results than needed for post-filtering
            let limit = opts.limit * 3;
            let top_docs = searcher
                .search(&query, &TopDocs::with_limit(limit))
                .map_err(|e| CoreError::Other(e.to_string()))?;

            for (bm25_score, doc_addr) in top_docs {
                let doc: tantivy::TantivyDocument = searcher
                    .doc(doc_addr)
                    .map_err(|e| CoreError::Other(e.to_string()))?;

                let id = doc
                    .get_first(self.id_field)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                // Skip if already found via ID lookup
                if seen_ids.contains(&id) {
                    continue;
                }

                let doc_type_str = doc
                    .get_first(self.doc_type_field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("entry");

                let doc_type = DocType::parse(doc_type_str).unwrap_or(DocType::Entry);

                // Filter by doc_types if specified
                if !opts.doc_types.is_empty() && !opts.doc_types.contains(&doc_type) {
                    continue;
                }

                seen_ids.insert(id.clone());
                results.push(SearchResult {
                    id,
                    doc_type,
                    score: bm25_score as f64,
                    bm25_score: bm25_score as f64,
                    boosted_score: bm25_score as f64,
                });
            }
        }

        // Re-sort by score (ID matches first with score 1.0, then text matches)
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Calibrate scores to meaningful 0-1 range (skip if only ID matches)
        let has_text_results = results.iter().any(|r| r.score < 1.0);
        if has_text_results && !results.is_empty() {
            let mut scores: Vec<(String, f64)> =
                results.iter().map(|r| (r.id.clone(), r.score)).collect();
            calibrate_scores(&mut scores);

            let score_map: std::collections::HashMap<&str, f64> =
                scores.iter().map(|(id, s)| (id.as_str(), *s)).collect();
            for result in results.iter_mut() {
                if let Some(&cal_score) = score_map.get(result.id.as_str()) {
                    result.score = cal_score;
                    result.boosted_score = cal_score;
                }
            }
        }

        // Limit results
        results.truncate(opts.limit);

        Ok(results)
    }

    /// Apply feedback, recency, and importance boosts to a score
    fn apply_boosts(&self, score: f64, entry: &Entry, opts: &SearchOptions) -> f64 {
        let mut boosted = score;

        // Feedback boost: score * (1 + 0.1*helpful) * max(0.1, 1 - 0.1*harmful)
        if opts.boost_feedback {
            let helpful_mult = 1.0 + 0.1 * entry.helpful_count as f64;
            let harmful_mult = (1.0 - 0.1 * entry.harmful_count as f64).max(0.1);
            boosted *= helpful_mult * harmful_mult;
        }

        // Recency boost: exponential decay
        if opts.boost_recency {
            let last_time = entry.last_accessed.unwrap_or(entry.created);
            let days_ago = (Utc::now() - last_time).num_days() as f64;
            let half_life = opts.recency_half_life.num_days() as f64;

            if half_life > 0.0 {
                let decay = 0.5_f64.powf(days_ago / half_life);
                // Scale between 0.5 and 1.0
                boosted *= 0.5 + 0.5 * decay;
            }
        }

        // Importance boost: importance score is 0.0-1.0, we scale it to 0.5-1.5
        // So importance=0.5 (default) gives 1.0 multiplier (no change)
        // importance=1.0 gives 1.5x boost, importance=0.0 gives 0.5x penalty
        if opts.boost_importance {
            let importance_mult = 0.5 + entry.importance as f64;
            boosted *= importance_mult;
        }

        boosted
    }

    /// Search for a single entry (first result)
    pub fn search_first(
        &self,
        query: &str,
        entries: &[Entry],
    ) -> Result<Option<SearchResult>, CoreError> {
        let opts = SearchOptions {
            query: query.to_string(),
            limit: 1,
            ..Default::default()
        };

        let results = self.search(&opts, entries)?;
        Ok(results.into_iter().next())
    }
}

// Implement SearchIndexTrait for SearchIndex to support dedup module
impl crate::dedup::SearchIndexTrait for SearchIndex {
    fn search_for_dedup(
        &self,
        query: &str,
        limit: usize,
        entries: &[Entry],
    ) -> Result<Vec<crate::dedup::SearchHit>, CoreError> {
        let opts = SearchOptions {
            query: query.to_string(),
            limit,
            ..Default::default()
        };

        let results = self.search(&opts, entries)?;
        Ok(results
            .into_iter()
            .map(|r| crate::dedup::SearchHit {
                id: r.id,
                bm25_score: r.score,
            })
            .collect())
    }
}
