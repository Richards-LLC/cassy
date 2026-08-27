use std::collections::HashMap;

use chrono::Utc;
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::{IndexReader, ReloadPolicy};

use cas_core::dedup::{SearchHit, SearchIndexTrait};
use cas_core::error::CoreError;

use crate::error::MemError;
use crate::hybrid_search::filter_grammar::parse_filter_query;
use crate::hybrid_search::id_utils::path_matches_pattern;
use crate::hybrid_search::{
    DocType, SearchIndex, SearchOptions, SearchResult, extract_id_patterns, scorer,
};
use crate::types::Entry;

/// Turn whitespace-delimited overlap terms into literal query-parser phrases.
///
/// Overlap queries are generated from memory symbols and title words, not
/// authored search syntax. Quoting each term prevents punctuation such as a
/// trailing `:` or `/` from being interpreted as Tantivy operators.
fn literal_dedup_query(query: &str) -> String {
    query
        .split_whitespace()
        .filter(|term| term.chars().any(char::is_alphanumeric))
        .map(|term| {
            let escaped = term.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl SearchIndex {
    /// Get or create a cached IndexReader.
    /// The reader uses `ReloadPolicy::OnCommitWithDelay` so it automatically
    /// picks up new segments after writes without manual invalidation.
    fn reader(&self) -> Result<IndexReader, MemError> {
        let mut guard = self.cached_reader.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(ref reader) = *guard {
            return Ok(reader.clone());
        }
        let reader: IndexReader = self
            .index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        *guard = Some(reader.clone());
        Ok(reader)
    }

    /// Get or create a cached QueryParser for the standard search fields.
    fn query_parser(&self) -> QueryParser {
        let mut guard = self
            .cached_query_parser
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(ref parser) = *guard {
            return parser.clone();
        }
        let parser = QueryParser::for_index(
            &self.index,
            vec![self.content_field, self.tags_field, self.title_field],
        );
        *guard = Some(parser.clone());
        parser
    }

    /// Quote field-like tokens whose field name is not registered in this
    /// index schema so Tantivy treats them as free text instead of rejecting
    /// the whole query. Registered fields are left untouched to retain
    /// Tantivy's strict syntax validation.
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
}

impl SearchIndex {
    /// Count committed documents for one logical search document type.
    ///
    /// The unified index can also contain code, history, artifact, and
    /// knowledge documents, so doctor must compare store-backed types
    /// individually instead of comparing the total Tantivy document count.
    pub fn count_documents(&self, doc_type: DocType) -> Result<u64, MemError> {
        let reader = self.reader()?;
        reader.reload()?;
        let term = tantivy::Term::from_field_text(self.doc_type_field, doc_type.as_str());
        let query = TermQuery::new(term, IndexRecordOption::Basic);
        Ok(reader.searcher().search(&query, &Count)? as u64)
    }

    /// Resolve a filter key (from [`parse_filter_query`]) to its Tantivy field.
    /// Returns `None` for unrecognized keys — caller should treat them as
    /// raw keyword text.
    fn field_for_filter_key(&self, key: &str) -> Option<tantivy::schema::Field> {
        match key {
            "module" => Some(self.module_field),
            "track" => Some(self.track_field),
            "problem_type" => Some(self.problem_type_field),
            "severity" => Some(self.severity_field),
            "root_cause" => Some(self.root_cause_field),
            "date" => Some(self.mem_date_field),
            _ => None,
        }
    }

    /// Build a BooleanQuery that AND-combines a text query (optional) with
    /// a list of term filters. Returns `None` if there are no filters and
    /// no text — caller should short-circuit.
    fn build_filtered_query(
        &self,
        text_query: Option<Box<dyn Query>>,
        filters: &[(String, String)],
    ) -> Option<Box<dyn Query>> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        if let Some(q) = text_query {
            clauses.push((Occur::Must, q));
        }
        for (k, v) in filters {
            if let Some(field) = self.field_for_filter_key(k) {
                let term = tantivy::Term::from_field_text(field, v);
                let tq: Box<dyn Query> =
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic));
                clauses.push((Occur::Must, tq));
            }
        }
        if clauses.is_empty() {
            None
        } else if clauses.len() == 1 {
            Some(clauses.into_iter().next().unwrap().1)
        } else {
            Some(Box::new(BooleanQuery::new(clauses)))
        }
    }
}

impl SearchIndex {
    pub fn search(
        &self,
        opts: &SearchOptions,
        entries: &[Entry],
    ) -> Result<Vec<SearchResult>, MemError> {
        if opts.query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let reader = self.reader()?;
        let searcher = reader.searcher();

        let query_parser = self.query_parser();
        let query = query_parser
            .parse_query(&self.literalize_unknown_field_tokens(&opts.query))
            .map_err(|e| MemError::Parse(e.to_string()))?;

        // Get more results than needed for post-filtering
        let limit = opts.limit * 3;
        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit))?;

        // Build a HashMap for O(1) entry lookup instead of O(n) per result
        let entry_map: HashMap<&str, &Entry> =
            entries.iter().map(|e| (e.id.as_str(), e)).collect();

        let mut results = Vec::new();

        for (bm25_score, doc_addr) in top_docs {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;

            let id = doc
                .get_first(self.id_field)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            // Find the entry for boosting
            let entry = entry_map.get(id.as_str()).copied();

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
            scorer::calibrate_scores(&mut scores);

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
    pub fn search_unified(&self, opts: &SearchOptions) -> Result<Vec<SearchResult>, MemError> {
        if opts.query.trim().is_empty() {
            return Ok(Vec::new());
        }

        // Parse filter grammar FIRST (cas-7b1e) — `module:cas-mcp` tokens
        // must not reach the ID pattern extractor, which would otherwise
        // mistake `cas-mcp` for a Cassy ID.
        let pre_parsed = parse_filter_query(opts.query.trim());
        let filters = pre_parsed.filters.clone();

        // Extract ID patterns from the residual (e.g., "cas-8cb5 cas-4a23")
        let (id_patterns, remaining_query) = extract_id_patterns(&pre_parsed.residual);

        let mut results = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        // 1. Direct ID lookups for extracted patterns (score = 1.0 for exact matches)
        if !id_patterns.is_empty() {
            let reader = self.reader()?;
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

        // 2. Text search for remaining query terms (if any), with optional
        //    structured filters parsed from `key:value` tokens (cas-7b1e).
        //    Quote unknown field-like tokens so Tantivy searches their text
        //    literally instead of treating them as invalid field references.
        let parsed = super::filter_grammar::ParsedQuery {
            residual: self.literalize_unknown_field_tokens(&remaining_query),
            filters: filters.clone(),
        };
        if !parsed.residual.is_empty() || !parsed.filters.is_empty() {
            let reader = self.reader()?;
            let searcher = reader.searcher();

            // Build a text query from the residual (if any) and AND it with
            // any structured filters.
            let text_query: Option<Box<dyn Query>> = if parsed.residual.is_empty() {
                None
            } else {
                let query_parser = self.query_parser();
                Some(
                    query_parser
                        .parse_query(&parsed.residual)
                        .map_err(|e| MemError::Parse(e.to_string()))?,
                )
            };

            let Some(query) = self.build_filtered_query(text_query, &parsed.filters) else {
                return Ok(results);
            };

            // Get more results than needed for post-filtering
            let limit = opts.limit * 3;
            let top_docs = searcher.search(&*query, &TopDocs::with_limit(limit))?;

            for (bm25_score, doc_addr) in top_docs {
                let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;

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

                // Apply code-specific filters for CodeSymbol results
                if doc_type == DocType::CodeSymbol {
                    // Filter by language
                    if let Some(ref lang_filter) = opts.language {
                        let lang = doc
                            .get_first(self.language_field)
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        if !lang.eq_ignore_ascii_case(lang_filter) {
                            continue;
                        }
                    }

                    // Filter by kind
                    if let Some(ref kind_filter) = opts.kind {
                        let kind = doc
                            .get_first(self.kind_field)
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        if !kind.eq_ignore_ascii_case(kind_filter) {
                            continue;
                        }
                    }

                    // Filter by file path pattern (simple glob)
                    if let Some(ref path_filter) = opts.file_path {
                        let file_path = doc
                            .get_first(self.file_path_field)
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        if !path_matches_pattern(file_path, path_filter) {
                            continue;
                        }
                    }
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
            scorer::calibrate_scores(&mut scores);

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
    ) -> Result<Option<SearchResult>, MemError> {
        let opts = SearchOptions {
            query: query.to_string(),
            limit: 1,
            ..Default::default()
        };

        let results = self.search(&opts, entries)?;
        Ok(results.into_iter().next())
    }
}

impl SearchIndex {
    /// Retrieve top-N BM25 candidates scoped to a single memory `module`.
    ///
    /// This is the load-bearing API for overlap detection (cas-7b1e):
    /// incoming structured memories must only be compared against other
    /// memories with the same `module`. Legacy memories (no frontmatter)
    /// have no module field indexed and are therefore excluded.
    ///
    /// Returns hits ranked by BM25 score of the text `query` intersected
    /// with a hard `module == module` term filter. When `query` is empty,
    /// returns the top documents in that module unranked (score `0.0`).
    pub fn search_module_candidates(
        &self,
        query: &str,
        module: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>, MemError> {
        let reader = self.reader()?;
        let searcher = reader.searcher();

        let module_term = tantivy::Term::from_field_text(self.module_field, module);
        let module_q: Box<dyn Query> =
            Box::new(TermQuery::new(module_term, IndexRecordOption::Basic));

        let text_q: Option<Box<dyn Query>> = if query.trim().is_empty() {
            None
        } else {
            let parser = self.query_parser();
            Some(
                parser
                    .parse_query(&self.literalize_unknown_field_tokens(query))
                    .map_err(|e| MemError::Parse(e.to_string()))?,
            )
        };

        let query: Box<dyn Query> = match text_q {
            Some(tq) => Box::new(BooleanQuery::new(vec![
                (Occur::Must, module_q),
                (Occur::Must, tq),
            ])),
            None => module_q,
        };

        let top_docs = searcher.search(&*query, &TopDocs::with_limit(limit))?;
        let mut hits = Vec::with_capacity(top_docs.len());
        for (score, addr) in top_docs {
            let doc: tantivy::TantivyDocument = searcher.doc(addr)?;
            let id = doc
                .get_first(self.id_field)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if id.is_empty() {
                continue;
            }
            hits.push(SearchHit {
                id,
                bm25_score: score as f64,
            });
        }
        Ok(hits)
    }
}

impl SearchIndexTrait for SearchIndex {
    fn search_for_dedup(
        &self,
        query: &str,
        limit: usize,
        entries: &[Entry],
    ) -> Result<Vec<SearchHit>, CoreError> {
        let mut opts = SearchOptions {
            query: query.to_string(),
            limit,
            ..Default::default()
        };
        let results = match self.search(&opts, entries) {
            Ok(results) => results,
            Err(MemError::Parse(_)) => {
                opts.query = literal_dedup_query(query);
                if opts.query.is_empty() {
                    return Ok(Vec::new());
                }
                self.search(&opts, entries)
                    .map_err(|e| CoreError::Other(e.to_string()))?
            }
            Err(error) => return Err(CoreError::Other(error.to_string())),
        };

        Ok(results
            .into_iter()
            .map(|result| SearchHit {
                id: result.id,
                bm25_score: result.bm25_score,
            })
            .collect())
    }

    fn search_candidates_by_module(
        &self,
        query: &str,
        module: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>, CoreError> {
        self.search_module_candidates(query, module, limit)
            .map_err(|e| CoreError::Other(e.to_string()))
    }
}

#[cfg(test)]
mod dedup_query_tests {
    use super::*;

    fn entry(id: &str, content: &str) -> Entry {
        Entry {
            id: id.to_string(),
            content: content.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn search_for_dedup_accepts_logged_punctuation_heavy_overlap_queries() {
        let index = SearchIndex::in_memory().unwrap();
        let entries = vec![
            entry(
                "penguinz",
                "Linux sysmem fallback supports Windows app-level version 6.1.24",
            ),
            entry(
                "gabber",
                "Gabber worktrees require reset restore discipline before committing work",
            ),
        ];
        for candidate in &entries {
            index.index_entry(candidate).unwrap();
        }

        let cases = [
            (
                "6.1.24 8.9 app-level 1-3. cas-e-series windows sysmem fallback linux:",
                "penguinz",
            ),
            (
                "committing. work. reset/restore skills/docs .cas/worktrees/ git hazards gabber worktrees:",
                "gabber",
            ),
        ];
        for (query, expected_id) in cases {
            let hits = index
                .search_for_dedup(query, 5, &entries)
                .unwrap_or_else(|error| panic!("logged overlap query must parse: {error}"));
            assert!(
                hits.iter().any(|hit| hit.id == expected_id),
                "expected {expected_id} candidate for {query:?}, got {hits:?}"
            );
        }
    }

    #[test]
    fn search_for_dedup_preserves_clean_query_results() {
        let index = SearchIndex::in_memory().unwrap();
        let entries = vec![
            entry("matching", "windows sysmem fallback"),
            entry("other", "unrelated memory"),
        ];
        for candidate in &entries {
            index.index_entry(candidate).unwrap();
        }

        let query = "windows sysmem";
        let direct = index
            .search(
                &SearchOptions {
                    query: query.to_string(),
                    limit: 5,
                    ..Default::default()
                },
                &entries,
            )
            .unwrap();
        let dedup = index.search_for_dedup(query, 5, &entries).unwrap();

        assert_eq!(
            dedup.iter().map(|hit| &hit.id).collect::<Vec<_>>(),
            direct.iter().map(|hit| &hit.id).collect::<Vec<_>>()
        );
        assert_eq!(dedup[0].bm25_score, direct[0].bm25_score);
    }
}
