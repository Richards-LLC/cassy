//! Score normalization and combination for hybrid search
//!
//! This module implements a sophisticated scoring system that:
//! - Uses percentile normalization (robust to outliers)
//! - Combines all channels in a single step (no information loss)
//! - Boosts entries found by multiple channels
//! - Adapts weights based on query characteristics
//! - Calibrates final scores to meaningful 0-1 range

use std::collections::HashMap;

// ============================================================================
// Query Analysis
// ============================================================================

/// Features extracted from a query for adaptive weight selection
#[derive(Debug, Clone, Default)]
pub struct QueryFeatures {
    /// Query contains quoted phrases (exact match intent)
    pub has_quotes: bool,
    /// Query contains special characters like error codes, paths
    pub has_special_chars: bool,
    /// Number of words in the query
    pub word_count: usize,
    /// Query contains temporal expressions (yesterday, last week, etc.)
    pub has_temporal: bool,
    /// Query is phrased as a question
    pub is_question: bool,
    /// Query looks like code or technical identifier
    pub is_code_like: bool,
}

impl QueryFeatures {
    /// Extract features from a query string
    pub fn extract(query: &str) -> Self {
        let query_lower = query.to_lowercase();

        let has_quotes = query.contains('"') || query.contains('\'');

        // Special chars that suggest exact matching needed
        let has_special_chars = query.chars().any(|c| {
            matches!(
                c,
                ':' | '/' | '\\' | '[' | ']' | '{' | '}' | '<' | '>' | '#' | '@'
            )
        });

        let word_count = query.split_whitespace().count();

        let temporal_phrases = [
            "yesterday",
            "today",
            "last week",
            "last month",
            "recently",
            "this week",
            "this month",
            "this year",
            "last year",
            "ago",
            "since",
            "between",
        ];
        let has_temporal = temporal_phrases.iter().any(|p| query_lower.contains(p));

        let is_question = query.trim().ends_with('?')
            || query_lower.starts_with("what ")
            || query_lower.starts_with("how ")
            || query_lower.starts_with("why ")
            || query_lower.starts_with("where ")
            || query_lower.starts_with("when ");

        // Code-like patterns: error codes, function names, file extensions
        let is_code_like = query.contains("::")
            || query.contains("()")
            || query.contains(".rs")
            || query.contains(".ts")
            || query.contains(".py")
            || query.chars().filter(|c| c.is_uppercase()).count() > 2
            || query.starts_with("E0")  // Rust error codes
            || query.contains("error:");

        Self {
            has_quotes,
            has_special_chars,
            word_count,
            has_temporal,
            is_question,
            is_code_like,
        }
    }

    /// Determine the query type for weight selection
    pub fn query_type(&self) -> QueryType {
        if self.has_quotes || self.is_code_like {
            QueryType::Exact
        } else if self.has_special_chars {
            QueryType::Technical
        } else if self.has_temporal {
            QueryType::Temporal
        } else if self.word_count <= 2 {
            QueryType::Keyword
        } else if self.is_question {
            QueryType::Conceptual
        } else {
            QueryType::Balanced
        }
    }
}

/// Query type classification for weight selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryType {
    /// Exact match needed (quotes, code)
    Exact,
    /// Technical query with special chars
    Technical,
    /// Time-based query
    Temporal,
    /// Short keyword lookup
    Keyword,
    /// Conceptual/semantic query
    Conceptual,
    /// Default balanced approach
    Balanced,
}

// ============================================================================
// Search Weights
// ============================================================================

/// Which scored channels can actually return rows in this process.
///
/// Weight tables like [`SearchWeights::for_query_type`] describe an *ideal*
/// allocation across every channel the design imagines. That is only honest
/// while every channel is live. Local embeddings were removed — `semantic`
/// search returns an empty vec unconditionally (see
/// `HybridSearch::semantic_search`) — so a Conceptual query was handing 60% of
/// its weight mass to a channel that contributes nothing, silently scaling
/// every result down and distorting the relative order of the channels that
/// did fire.
///
/// Capabilities make "this channel is dead" a first-class input instead of an
/// unwritten assumption: [`SearchWeights::for_capabilities`] zeroes the dead
/// channels and renormalizes, so the surviving weights always sum to 1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelCapabilities {
    /// Lexical channel. Structurally always live: `HybridSearch` cannot be
    /// constructed without a `SearchIndex`.
    pub bm25: bool,
    /// Embedding channel. Cloud-only since local embeddings were removed.
    pub semantic: bool,
    /// Time-aware channel.
    pub temporal: bool,
}

impl Default for ChannelCapabilities {
    /// Every channel live — the shape the weight tables were written for.
    fn default() -> Self {
        Self {
            bm25: true,
            semantic: true,
            temporal: true,
        }
    }
}

impl ChannelCapabilities {
    /// All channels available.
    pub const ALL: Self = Self {
        bm25: true,
        semantic: true,
        temporal: true,
    };

    /// What a purely local Cassy can actually do today: no embeddings.
    pub const LOCAL: Self = Self {
        bm25: true,
        semantic: false,
        temporal: true,
    };

    /// Number of live channels.
    pub fn live_count(&self) -> usize {
        self.bm25 as usize + self.semantic as usize + self.temporal as usize
    }
}

/// Weights for combining search channels
#[derive(Debug, Clone, Copy)]
pub struct SearchWeights {
    pub bm25: f32,
    pub semantic: f32,
    pub temporal: f32,
}

impl Default for SearchWeights {
    fn default() -> Self {
        Self {
            bm25: 0.35,
            semantic: 0.45,
            temporal: 0.20,
        }
    }
}

impl SearchWeights {
    /// Create weights optimized for a specific query type
    pub fn for_query_type(query_type: QueryType) -> Self {
        match query_type {
            QueryType::Exact => Self {
                bm25: 0.75,
                semantic: 0.15,
                temporal: 0.10,
            },
            QueryType::Technical => Self {
                bm25: 0.60,
                semantic: 0.25,
                temporal: 0.15,
            },
            QueryType::Temporal => Self {
                bm25: 0.25,
                semantic: 0.30,
                temporal: 0.45,
            },
            QueryType::Keyword => Self {
                bm25: 0.45,
                semantic: 0.40,
                temporal: 0.15,
            },
            QueryType::Conceptual => Self {
                bm25: 0.20,
                semantic: 0.60,
                temporal: 0.20,
            },
            QueryType::Balanced => Self::default(),
        }
    }

    /// Get weights based on query analysis
    pub fn from_query(query: &str) -> Self {
        let features = QueryFeatures::extract(query);
        Self::for_query_type(features.query_type())
    }

    /// Redistribute this allocation over the channels that can actually fire.
    ///
    /// Dead channels are zeroed and their mass is spread across the live ones
    /// **in proportion to the live channels' original weights**, so the
    /// designed emphasis is preserved: a Conceptual query (bm25 0.20, semantic
    /// 0.60, temporal 0.20) with no embeddings becomes bm25 0.50 / temporal
    /// 0.50 rather than bm25 0.20 / temporal 0.20 summing to 0.4.
    ///
    /// The result always sums to 1.0. If a table allocates *nothing* to any
    /// live channel (or no channel is live at all) there is no proportion to
    /// preserve, so this falls back to bm25-only — the one channel that is
    /// structurally always present.
    pub fn for_capabilities(&self, caps: ChannelCapabilities) -> Self {
        let masked = Self {
            bm25: if caps.bm25 { self.bm25 } else { 0.0 },
            semantic: if caps.semantic { self.semantic } else { 0.0 },
            temporal: if caps.temporal { self.temporal } else { 0.0 },
        };

        let sum = masked.bm25 + masked.semantic + masked.temporal;
        if sum < 1e-6 {
            return Self {
                bm25: 1.0,
                semantic: 0.0,
                temporal: 0.0,
            };
        }

        Self {
            bm25: masked.bm25 / sum,
            semantic: masked.semantic / sum,
            temporal: masked.temporal / sum,
        }
    }

    /// Weights for a query type, honest about which channels are live.
    pub fn for_query_type_with_capabilities(
        query_type: QueryType,
        caps: ChannelCapabilities,
    ) -> Self {
        Self::for_query_type(query_type).for_capabilities(caps)
    }

    /// Weights from query analysis, honest about which channels are live.
    pub fn from_query_with_capabilities(query: &str, caps: ChannelCapabilities) -> Self {
        Self::from_query(query).for_capabilities(caps)
    }

    /// Sum of all channel weights. `1.0` for any normalized allocation.
    pub fn total(&self) -> f32 {
        self.bm25 + self.semantic + self.temporal
    }

    /// Normalize weights to sum to 1.0
    pub fn normalized(&self) -> Self {
        let sum = self.bm25 + self.semantic + self.temporal;
        if sum < 1e-6 {
            return Self::default();
        }
        Self {
            bm25: self.bm25 / sum,
            semantic: self.semantic / sum,
            temporal: self.temporal / sum,
        }
    }

    /// Create custom weights (will be normalized)
    pub fn custom(bm25: f32, semantic: f32, temporal: f32) -> Self {
        Self {
            bm25,
            semantic,
            temporal,
        }
        .normalized()
    }
}

// ============================================================================
// Score Normalization
// ============================================================================

/// Normalize scores using percentile-based scaling
///
/// More robust than min-max normalization because:
/// - Single outliers don't dominate the scale
/// - Tight clusters maintain relative distances
/// - Scores above the percentile get soft-clipped, not hard-cut
pub fn percentile_normalize(scores: &[(String, f64)], percentile: f64) -> Vec<(String, f64)> {
    if scores.is_empty() {
        return Vec::new();
    }

    // Get sorted score values
    let mut sorted_scores: Vec<f64> = scores.iter().map(|(_, s)| *s).collect();
    sorted_scores.sort_by(|a, b| a.total_cmp(b));

    // Calculate percentile value
    let idx = ((percentile / 100.0) * (sorted_scores.len() - 1) as f64).round() as usize;
    let idx = idx.min(sorted_scores.len() - 1);
    let p_val = sorted_scores[idx];

    if p_val < 1e-10 {
        // All scores near zero - return uniform high scores
        return scores.iter().map(|(id, _)| (id.clone(), 1.0)).collect();
    }

    // Normalize: divide by percentile value, soft-clip at 1.2
    scores
        .iter()
        .map(|(id, score)| {
            let normalized = (score / p_val).min(1.2);
            (id.clone(), normalized)
        })
        .collect()
}

/// Legacy min-max normalization (kept for compatibility)
pub fn normalize_scores(scores: &[(String, f64)]) -> Vec<(String, f64)> {
    if scores.is_empty() {
        return Vec::new();
    }

    let min_score = scores.iter().map(|(_, s)| *s).fold(f64::INFINITY, f64::min);
    let max_score = scores
        .iter()
        .map(|(_, s)| *s)
        .fold(f64::NEG_INFINITY, f64::max);

    let range = max_score - min_score;

    if range < 1e-10 {
        return scores.iter().map(|(id, _)| (id.clone(), 1.0)).collect();
    }

    scores
        .iter()
        .map(|(id, score)| (id.clone(), (score - min_score) / range))
        .collect()
}

// ============================================================================
// Score Combination
// ============================================================================

/// Combine multiple search channels in a single step
///
/// Key improvements over sequential combination:
/// - No repeated normalization (preserves information)
/// - Multi-channel boost rewards entries found by multiple methods
/// - Uses percentile normalization for robustness
pub fn combine_multi_channel(
    bm25_scores: &[(String, f64)],
    semantic_scores: &[(String, f64)],
    temporal_scores: &[(String, f64)],
    weights: SearchWeights,
) -> Vec<(String, f64)> {
    let weights = weights.normalized();

    // Normalize BM25 with percentile (raw scores can be 0-10+)
    let bm25_norm = percentile_normalize(bm25_scores, 90.0);

    // Semantic scores are already 0-1 from cosine similarity
    // But we still normalize to handle distribution skew
    let semantic_norm = if semantic_scores.is_empty() {
        Vec::new()
    } else {
        percentile_normalize(semantic_scores, 90.0)
    };

    // Temporal scores are already 0-1 from decay function
    let temporal_norm = temporal_scores.to_vec();

    // Build lookup maps
    let bm25_map: HashMap<&str, f64> = bm25_norm.iter().map(|(id, s)| (id.as_str(), *s)).collect();
    let semantic_map: HashMap<&str, f64> = semantic_norm
        .iter()
        .map(|(id, s)| (id.as_str(), *s))
        .collect();
    let temporal_map: HashMap<&str, f64> = temporal_norm
        .iter()
        .map(|(id, s)| (id.as_str(), *s))
        .collect();

    // Get all unique IDs
    let mut all_ids: Vec<&str> = bm25_map
        .keys()
        .chain(semantic_map.keys())
        .chain(temporal_map.keys())
        .copied()
        .collect();
    all_ids.sort();
    all_ids.dedup();

    // Combine scores in single pass
    let mut combined: Vec<(String, f64)> = all_ids
        .into_iter()
        .map(|id| {
            let bm25 = bm25_map.get(id).copied().unwrap_or(0.0);
            let semantic = semantic_map.get(id).copied().unwrap_or(0.0);
            let temporal = temporal_map.get(id).copied().unwrap_or(0.0);

            // Count channels that found this entry (with meaningful score)
            let threshold = 0.05;
            let channel_count = (bm25 > threshold) as i32
                + (semantic > threshold) as i32
                + (temporal > threshold) as i32;

            // Multi-channel boost: entries found by multiple methods are more reliable
            // 1 channel: 1.0x, 2 channels: 1.15x, 3 channels: 1.30x
            let multi_channel_boost = 1.0 + 0.15 * (channel_count - 1).max(0) as f64;

            // Weighted combination
            let base_score = (weights.bm25 as f64) * bm25
                + (weights.semantic as f64) * semantic
                + (weights.temporal as f64) * temporal;

            let final_score = base_score * multi_channel_boost;

            (id.to_string(), final_score)
        })
        .collect();

    // Sort by score descending
    combined.sort_by(|a, b| b.1.total_cmp(&a.1));

    combined
}

/// Legacy two-channel combination (kept for compatibility)
pub fn combine_scores(
    bm25_results: &[(String, f64)],
    semantic_results: &[(String, f64)],
    bm25_weight: f32,
    semantic_weight: f32,
) -> Vec<(String, f64)> {
    // Use new multi-channel with empty temporal
    combine_multi_channel(
        bm25_results,
        semantic_results,
        &[],
        SearchWeights::custom(bm25_weight, semantic_weight, 0.0),
    )
}

// ============================================================================
// Reciprocal Rank Fusion
// ============================================================================

/// Enhanced RRF that considers score magnitude
///
/// Standard RRF only uses rank position. This version also considers
/// how confident each ranker was (via normalized score magnitude).
pub fn rrf_with_magnitude(rankings: &[Vec<(String, f64)>], k: f64) -> Vec<(String, f64)> {
    if rankings.is_empty() {
        return Vec::new();
    }

    let mut rrf_scores: HashMap<String, f64> = HashMap::new();
    let mut max_norm_scores: HashMap<String, f64> = HashMap::new();
    let mut channel_counts: HashMap<String, i32> = HashMap::new();

    for ranking in rankings {
        if ranking.is_empty() {
            continue;
        }

        let top_score = ranking[0].1.max(0.001);

        for (rank, (id, score)) in ranking.iter().enumerate() {
            // Standard RRF contribution
            let rrf = 1.0 / (k + rank as f64 + 1.0);
            *rrf_scores.entry(id.clone()).or_insert(0.0) += rrf;

            // Track normalized score for magnitude boost
            let norm_score = score / top_score;
            let entry = max_norm_scores.entry(id.clone()).or_insert(0.0);
            if norm_score > *entry {
                *entry = norm_score;
            }

            // Track channel count
            *channel_counts.entry(id.clone()).or_insert(0) += 1;
        }
    }

    // Apply magnitude and multi-channel boost
    let mut results: Vec<(String, f64)> = rrf_scores
        .into_iter()
        .map(|(id, rrf)| {
            let magnitude = max_norm_scores.get(&id).copied().unwrap_or(0.0);
            let channels = channel_counts.get(&id).copied().unwrap_or(1);

            // Boost high-confidence matches
            let magnitude_boost = 1.0 + 0.25 * magnitude;
            // Boost multi-channel matches
            let channel_boost = 1.0 + 0.10 * (channels - 1).max(0) as f64;

            let boosted = rrf * magnitude_boost * channel_boost;
            (id, boosted)
        })
        .collect();

    results.sort_by(|a, b| b.1.total_cmp(&a.1));
    results
}

/// Standard RRF (kept for compatibility)
pub fn reciprocal_rank_fusion(rankings: &[Vec<(String, f64)>], k: f64) -> Vec<(String, f64)> {
    let mut rrf_scores: HashMap<String, f64> = HashMap::new();

    for ranking in rankings {
        for (rank, (id, _)) in ranking.iter().enumerate() {
            let rrf_contribution = 1.0 / (k + rank as f64 + 1.0);
            *rrf_scores.entry(id.clone()).or_insert(0.0) += rrf_contribution;
        }
    }

    let mut results: Vec<(String, f64)> = rrf_scores.into_iter().collect();
    results.sort_by(|a, b| b.1.total_cmp(&a.1));

    results
}

// ============================================================================
// Score Calibration
// ============================================================================

/// Calibrate scores to a meaningful 0-1 range
///
/// After calibration:
/// - 0.80+ = Highly relevant (strong match)
/// - 0.50-0.79 = Relevant (good match)
/// - 0.30-0.49 = Possibly relevant (weak match)
/// - <0.30 = Marginal (included for completeness)
pub fn calibrate_scores(scores: &mut [(String, f64)]) {
    if scores.is_empty() {
        return;
    }

    // Find the max score
    let max_score = scores.iter().map(|(_, s)| *s).fold(0.0f64, f64::max);

    if max_score < 1e-10 {
        return;
    }

    // Scale so best result is ~0.85 (leaving headroom for "perfect" matches)
    let scale = 0.85 / max_score;

    for (_, score) in scores.iter_mut() {
        *score = (*score * scale).min(1.0);
    }
}

/// Calibrate scores with a floor for entries that matched at least one channel
pub fn calibrate_scores_with_floor(scores: &mut [(String, f64)], floor: f64) {
    calibrate_scores(scores);

    // Apply floor to ensure minimum visibility for any match
    for (_, score) in scores.iter_mut() {
        if *score > 0.0 && *score < floor {
            *score = floor;
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use crate::hybrid_search::scorer::*;

    #[test]
    fn test_query_features() {
        // Exact match query
        let features = QueryFeatures::extract("\"exact phrase\"");
        assert!(features.has_quotes);
        assert_eq!(features.query_type(), QueryType::Exact);

        // Code-like query
        let features = QueryFeatures::extract("error: E0382");
        assert!(features.is_code_like);
        assert_eq!(features.query_type(), QueryType::Exact);

        // Temporal query
        let features = QueryFeatures::extract("what did I learn last week");
        assert!(features.has_temporal);
        assert_eq!(features.query_type(), QueryType::Temporal);

        // Conceptual query
        let features = QueryFeatures::extract("how does memory management work?");
        assert!(features.is_question);
        assert_eq!(features.query_type(), QueryType::Conceptual);
    }

    #[test]
    fn test_search_weights_sum_to_one() {
        for query_type in [
            QueryType::Exact,
            QueryType::Technical,
            QueryType::Temporal,
            QueryType::Keyword,
            QueryType::Conceptual,
            QueryType::Balanced,
        ] {
            let weights = SearchWeights::for_query_type(query_type).normalized();
            let sum = weights.bm25 + weights.semantic + weights.temporal;
            assert!(
                (sum - 1.0).abs() < 1e-6,
                "Weights for {query_type:?} sum to {sum}"
            );
        }
    }

    #[test]
    fn test_percentile_normalize() {
        let scores = vec![
            ("a".to_string(), 100.0),
            ("b".to_string(), 50.0),
            ("c".to_string(), 25.0),
            ("d".to_string(), 10.0),
            ("e".to_string(), 1.0),
        ];

        let normalized = percentile_normalize(&scores, 90.0);

        // Top score should be near or above 1.0
        let a_score = normalized.iter().find(|(id, _)| id == "a").unwrap().1;
        assert!(a_score >= 1.0);

        // Scores should maintain relative order
        let b_score = normalized.iter().find(|(id, _)| id == "b").unwrap().1;
        let c_score = normalized.iter().find(|(id, _)| id == "c").unwrap().1;
        assert!(a_score > b_score);
        assert!(b_score > c_score);
    }

    #[test]
    fn test_multi_channel_boost() {
        // Entry A: found by BM25 only
        // Entry B: found by BM25 and semantic
        // Entry C: found by all three
        let bm25 = vec![
            ("a".to_string(), 1.0),
            ("b".to_string(), 0.8),
            ("c".to_string(), 0.6),
        ];
        let semantic = vec![("b".to_string(), 0.9), ("c".to_string(), 0.7)];
        let temporal = vec![("c".to_string(), 0.8)];

        let combined = combine_multi_channel(&bm25, &semantic, &temporal, SearchWeights::default());

        // C should get significant multi-channel boost
        let c_score = combined.iter().find(|(id, _)| id == "c").unwrap().1;
        let a_score = combined.iter().find(|(id, _)| id == "a").unwrap().1;

        // Despite lower raw scores, C should be competitive due to multi-channel boost
        // (exact ordering depends on weights, but C shouldn't be far behind)
        assert!(c_score > 0.0);
        assert!(a_score > 0.0);
    }

    #[test]
    fn test_rrf_with_magnitude() {
        let ranking1 = vec![
            ("a".to_string(), 10.0), // Clear #1
            ("b".to_string(), 2.0),
        ];
        let ranking2 = vec![
            ("b".to_string(), 0.9), // Clear #1 in this ranking
            ("a".to_string(), 0.1),
        ];

        let combined = rrf_with_magnitude(&[ranking1, ranking2], 60.0);

        // Both should be present
        assert_eq!(combined.len(), 2);

        // Both appeared in both rankings
        let a_score = combined.iter().find(|(id, _)| id == "a").unwrap().1;
        let b_score = combined.iter().find(|(id, _)| id == "b").unwrap().1;
        assert!(a_score > 0.0);
        assert!(b_score > 0.0);
    }

    #[test]
    fn test_calibrate_scores() {
        let mut scores = vec![
            ("a".to_string(), 2.0),
            ("b".to_string(), 1.0),
            ("c".to_string(), 0.5),
        ];

        calibrate_scores(&mut scores);

        // Max should be ~0.85
        let max = scores.iter().map(|(_, s)| *s).fold(0.0f64, f64::max);
        assert!((max - 0.85).abs() < 0.01);

        // All should be <= 1.0
        for (_, score) in &scores {
            assert!(*score <= 1.0);
        }
    }

    #[test]
    fn test_normalize_scores() {
        let test_cases = vec![
            (
                vec![
                    ("a".to_string(), 10.0),
                    ("b".to_string(), 5.0),
                    ("c".to_string(), 0.0),
                ],
                1.0,
                0.0,
            ),
            (
                vec![("a".to_string(), 100.0), ("b".to_string(), 50.0)],
                1.0,
                0.0,
            ),
        ];

        for (input, expected_first, expected_last) in test_cases {
            let result = normalize_scores(&input);
            assert_eq!(result.len(), input.len());
            assert!((result[0].1 - expected_first).abs() < 1e-6);
            assert!((result[result.len() - 1].1 - expected_last).abs() < 1e-6);
        }
    }

    #[test]
    fn test_normalize_scores_empty() {
        let result = normalize_scores(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_normalize_scores_uniform() {
        let input = vec![
            ("a".to_string(), 5.0),
            ("b".to_string(), 5.0),
            ("c".to_string(), 5.0),
        ];
        let result = normalize_scores(&input);
        for (_, score) in result {
            assert_eq!(score, 1.0);
        }
    }

    #[test]
    fn test_combine_scores() {
        let bm25 = vec![("a".to_string(), 10.0), ("b".to_string(), 5.0)];
        let semantic = vec![("b".to_string(), 0.9), ("c".to_string(), 0.8)];

        let combined = combine_scores(&bm25, &semantic, 0.5, 0.5);

        assert_eq!(combined.len(), 3);

        let a_score = combined
            .iter()
            .find(|(id, _)| id == "a")
            .map(|(_, s)| *s)
            .unwrap();
        let b_score = combined
            .iter()
            .find(|(id, _)| id == "b")
            .map(|(_, s)| *s)
            .unwrap();
        let c_score = combined
            .iter()
            .find(|(id, _)| id == "c")
            .map(|(_, s)| *s)
            .unwrap();

        assert!(a_score > 0.0);
        assert!(b_score > 0.0);
        assert!(c_score >= 0.0);
    }

    #[test]
    fn test_reciprocal_rank_fusion() {
        let ranking1 = vec![
            ("a".to_string(), 10.0),
            ("b".to_string(), 5.0),
            ("c".to_string(), 1.0),
        ];
        let ranking2 = vec![
            ("b".to_string(), 0.9),
            ("a".to_string(), 0.5),
            ("d".to_string(), 0.1),
        ];

        let combined = reciprocal_rank_fusion(&[ranking1, ranking2], 60.0);

        assert_eq!(combined.len(), 4);

        let a_score = combined
            .iter()
            .find(|(id, _)| id == "a")
            .map(|(_, s)| *s)
            .unwrap();
        let b_score = combined
            .iter()
            .find(|(id, _)| id == "b")
            .map(|(_, s)| *s)
            .unwrap();
        let c_score = combined
            .iter()
            .find(|(id, _)| id == "c")
            .map(|(_, s)| *s)
            .unwrap();
        let d_score = combined
            .iter()
            .find(|(id, _)| id == "d")
            .map(|(_, s)| *s)
            .unwrap();

        assert!(a_score > c_score);
        assert!(b_score > d_score);
    }

    #[test]
    fn test_rrf_empty() {
        let result = reciprocal_rank_fusion(&[], 60.0);
        assert!(result.is_empty());
    }
}

#[cfg(test)]
mod capability_tests {
    use crate::hybrid_search::scorer::*;

    const ALL_QUERY_TYPES: [QueryType; 6] = [
        QueryType::Exact,
        QueryType::Technical,
        QueryType::Temporal,
        QueryType::Keyword,
        QueryType::Conceptual,
        QueryType::Balanced,
    ];

    #[test]
    fn conceptual_weights_sum_to_one_across_live_channels_only() {
        // The defect this fixes: Conceptual allocated 0.60 to the semantic
        // channel, which returns an empty vec locally. 60% of every conceptual
        // query's weight mass evaporated.
        let ideal = SearchWeights::for_query_type(QueryType::Conceptual);
        assert!(
            ideal.semantic > 0.5,
            "precondition: the ideal table leans on semantic"
        );

        let honest = ideal.for_capabilities(ChannelCapabilities::LOCAL);

        assert_eq!(
            honest.semantic, 0.0,
            "a dead channel must carry no weight at all"
        );
        assert!(
            (honest.total() - 1.0).abs() < 1e-6,
            "live weights must sum to 1.0, got {} ({honest:?})",
            honest.total()
        );
    }

    #[test]
    fn every_query_type_sums_to_one_with_no_embeddings() {
        for qt in ALL_QUERY_TYPES {
            let w = SearchWeights::for_query_type_with_capabilities(qt, ChannelCapabilities::LOCAL);
            assert!(
                (w.total() - 1.0).abs() < 1e-6,
                "{qt:?} weights sum to {} instead of 1.0: {w:?}",
                w.total()
            );
            assert_eq!(w.semantic, 0.0, "{qt:?} still funds the dead channel");
        }
    }

    #[test]
    fn dead_channel_mass_is_redistributed_proportionally_not_uniformly() {
        // Temporal leans hard on the temporal channel (0.25/0.30/0.45).
        // Dropping semantic must preserve that lean, not flatten it to 50/50.
        let honest = SearchWeights::for_query_type(QueryType::Temporal)
            .for_capabilities(ChannelCapabilities::LOCAL);

        assert!(
            honest.temporal > honest.bm25,
            "a temporal query must still favour the temporal channel: {honest:?}"
        );
        // 0.25 : 0.45 preserved → 0.357… : 0.642…
        assert!((honest.bm25 - 0.25 / 0.70).abs() < 1e-5, "{honest:?}");
        assert!((honest.temporal - 0.45 / 0.70).abs() < 1e-5, "{honest:?}");
    }

    #[test]
    fn full_capabilities_leave_the_designed_weights_untouched() {
        for qt in ALL_QUERY_TYPES {
            let ideal = SearchWeights::for_query_type(qt);
            let same = ideal.for_capabilities(ChannelCapabilities::ALL);
            // The tables already sum to 1.0, so renormalizing is a no-op.
            assert!((same.bm25 - ideal.bm25).abs() < 1e-6, "{qt:?}");
            assert!((same.semantic - ideal.semantic).abs() < 1e-6, "{qt:?}");
            assert!((same.temporal - ideal.temporal).abs() < 1e-6, "{qt:?}");
        }
    }

    #[test]
    fn no_live_channel_falls_back_to_bm25_rather_than_zeroing_every_score() {
        let dead = ChannelCapabilities {
            bm25: false,
            semantic: false,
            temporal: false,
        };
        let w = SearchWeights::for_query_type(QueryType::Balanced).for_capabilities(dead);

        assert_eq!(w.bm25, 1.0, "must not silently zero out all scoring");
        assert!((w.total() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn live_count_reports_usable_channels() {
        assert_eq!(ChannelCapabilities::ALL.live_count(), 3);
        assert_eq!(ChannelCapabilities::LOCAL.live_count(), 2);
    }
}
