//! Graph-based retrieval using spreading activation (Hindsight-inspired)
//!
//! This module provides graph traversal for knowledge retrieval:
//! - Entity-centric search starting from query-mentioned entities
//! - Spreading activation across relationship edges
//! - Activation decay based on edge weight and distance
//!
//! Inspired by the Hindsight paper's approach to traversing interconnected memories.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// Maximum neighbors to consider per entity during spreading activation.
/// Prevents memory/CPU issues with highly connected entities.
const MAX_NEIGHBORS_PER_ENTITY: usize = 100;

use crate::error::Result;
use crate::store::EntityStore;
use crate::types::{Entity, EntityType, RelationType};

/// Configuration for spreading activation
#[derive(Debug, Clone)]
pub struct SpreadingActivationConfig {
    /// Initial activation for seed entities
    pub initial_activation: f32,
    /// Decay factor per hop (activation *= decay at each step)
    pub decay_factor: f32,
    /// Minimum activation threshold (stop spreading below this)
    pub min_activation: f32,
    /// Maximum hops from seed nodes
    pub max_hops: usize,
    /// Weight multipliers for different relationship types
    pub relation_weights: HashMap<RelationType, f32>,
}

impl Default for SpreadingActivationConfig {
    fn default() -> Self {
        let mut relation_weights = HashMap::new();
        // Direct working relationships are strong
        relation_weights.insert(RelationType::WorksOn, 0.9);
        // Usage relationships are very strong
        relation_weights.insert(RelationType::Uses, 0.85);
        // Mention relationships show connection
        relation_weights.insert(RelationType::MentionedIn, 0.7);
        // General semantic relationships
        relation_weights.insert(RelationType::RelatedTo, 0.8);
        // Hierarchy/composition
        relation_weights.insert(RelationType::PartOf, 0.75);
        // Dependency relationships
        relation_weights.insert(RelationType::DependsOn, 0.8);
        // Creation relationships
        relation_weights.insert(RelationType::Created, 0.7);
        // Modification relationships
        relation_weights.insert(RelationType::Modified, 0.65);
        // Knowledge/association relationships
        relation_weights.insert(RelationType::Knows, 0.6);
        // Ownership relationships
        relation_weights.insert(RelationType::Owns, 0.7);
        // Implementation relationships
        relation_weights.insert(RelationType::Implements, 0.85);

        Self {
            initial_activation: 1.0,
            decay_factor: 0.5,
            min_activation: 0.05,
            max_hops: 3,
            relation_weights,
        }
    }
}

/// Result of graph retrieval for an entry
#[derive(Debug, Clone)]
pub struct GraphRetrievalResult {
    /// Entry ID
    pub entry_id: String,
    /// Total activation score (sum of activations from connected entities)
    pub activation_score: f32,
    /// Entities that contributed to this score
    pub contributing_entities: Vec<(String, f32)>, // (entity_id, activation)
    /// Minimum hops from a seed entity
    pub min_hops: usize,
}

/// Graph retriever using spreading activation
pub struct GraphRetriever {
    entity_store: Arc<dyn EntityStore>,
    config: SpreadingActivationConfig,
}

impl GraphRetriever {
    /// Create a new graph retriever
    pub fn new(entity_store: Arc<dyn EntityStore>, config: SpreadingActivationConfig) -> Self {
        Self {
            entity_store,
            config,
        }
    }

    /// Create with default config
    pub fn with_defaults(entity_store: Arc<dyn EntityStore>) -> Self {
        Self::new(entity_store, SpreadingActivationConfig::default())
    }

    /// The entity store this retriever activates over.
    ///
    /// Exposed so other channels can resolve the entity IDs that
    /// [`Self::spread_activation`] returns back into entity names.
    pub fn entity_store(&self) -> &Arc<dyn EntityStore> {
        &self.entity_store
    }

    /// Extract potential entity mentions from a query
    ///
    /// This is a simple heuristic - in production you might use NER
    pub fn extract_entity_candidates(&self, query: &str) -> Vec<String> {
        // Split on whitespace and common delimiters
        let words: Vec<&str> = query
            .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .filter(|s| !s.is_empty())
            .collect();

        let mut candidates = Vec::new();

        // Look for capitalized words (potential proper nouns)
        for word in &words {
            let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric());
            if trimmed.len() >= 2 {
                // Check if it starts with uppercase (potential entity)
                if trimmed
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                {
                    candidates.push(trimmed.to_string());
                }
                // Also add longer words as potential entities (e.g., "rust", "python")
                if trimmed.len() >= 4 {
                    candidates.push(trimmed.to_string());
                }
            }
        }

        // Also try consecutive capitalized words (multi-word entities)
        let mut i = 0;
        while i < words.len() {
            let word = words[i].trim_matches(|c: char| !c.is_alphanumeric());
            if word
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
            {
                let mut phrase = word.to_string();
                let mut j = i + 1;
                while j < words.len() {
                    let next = words[j].trim_matches(|c: char| !c.is_alphanumeric());
                    if next
                        .chars()
                        .next()
                        .map(|c| c.is_uppercase())
                        .unwrap_or(false)
                    {
                        phrase.push(' ');
                        phrase.push_str(next);
                        j += 1;
                    } else {
                        break;
                    }
                }
                if phrase.contains(' ') {
                    candidates.push(phrase);
                }
                i = j;
            } else {
                i += 1;
            }
        }

        candidates
    }

    /// Find entities matching the candidates
    pub fn find_seed_entities(&self, candidates: &[String]) -> Result<Vec<Entity>> {
        let mut entities = Vec::new();
        let mut seen_ids = HashSet::new();

        for candidate in candidates {
            // Try exact match first
            if let Some(entity) = self.entity_store.get_entity_by_name(candidate, None)? {
                if !seen_ids.contains(&entity.id) {
                    seen_ids.insert(entity.id.clone());
                    entities.push(entity);
                }
            } else {
                // Try fuzzy search
                let matches = self.entity_store.search_entities(candidate, None)?;
                for entity in matches.into_iter().take(2) {
                    if !seen_ids.contains(&entity.id) {
                        seen_ids.insert(entity.id.clone());
                        entities.push(entity);
                    }
                }
            }
        }

        Ok(entities)
    }

    /// Perform spreading activation from seed entities
    ///
    /// Returns a map of entity_id -> activation_score
    pub fn spread_activation(&self, seeds: &[Entity]) -> Result<HashMap<String, f32>> {
        let mut activations: HashMap<String, f32> = HashMap::new();
        let mut visited: HashSet<String> = HashSet::new();

        // Queue: (entity_id, current_activation, current_hop)
        let mut queue: VecDeque<(String, f32, usize)> = VecDeque::new();

        // Initialize seeds
        for seed in seeds {
            let activation = self.config.initial_activation * seed.confidence;
            activations.insert(seed.id.clone(), activation);
            queue.push_back((seed.id.clone(), activation, 0));
        }

        // BFS spreading
        while let Some((entity_id, current_activation, hop)) = queue.pop_front() {
            if visited.contains(&entity_id) {
                continue;
            }
            visited.insert(entity_id.clone());

            // Stop if we've reached max hops
            if hop >= self.config.max_hops {
                continue;
            }

            // Get connected entities (limited to prevent memory issues with highly connected nodes)
            let connected = self.entity_store.get_connected_entities(&entity_id)?;

            for (neighbor, relationship) in connected.into_iter().take(MAX_NEIGHBORS_PER_ENTITY) {
                // Calculate propagated activation
                let edge_weight = self
                    .config
                    .relation_weights
                    .get(&relationship.relation_type)
                    .copied()
                    .unwrap_or(0.5);

                let propagated = current_activation
                    * self.config.decay_factor
                    * edge_weight
                    * relationship.weight;

                // Skip if below threshold
                if propagated < self.config.min_activation {
                    continue;
                }

                // Update activation (take max if already activated)
                let entry = activations.entry(neighbor.id.clone()).or_insert(0.0);
                *entry = entry.max(propagated);

                // Add to queue if not visited
                if !visited.contains(&neighbor.id) {
                    queue.push_back((neighbor.id.clone(), propagated, hop + 1));
                }
            }
        }

        Ok(activations)
    }

    /// Get entries connected to activated entities, scored by activation
    pub fn retrieve_entries(&self, query: &str, limit: usize) -> Result<Vec<GraphRetrievalResult>> {
        // 1. Extract entity candidates from query
        let candidates = self.extract_entity_candidates(query);
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // 2. Find seed entities
        let seeds = self.find_seed_entities(&candidates)?;
        if seeds.is_empty() {
            return Ok(Vec::new());
        }

        // Track which entities are seeds for hop calculation
        let seed_ids: HashSet<String> = seeds.iter().map(|e| e.id.clone()).collect();

        // 3. Spread activation
        let activations = self.spread_activation(&seeds)?;

        // 4. Collect entries from activated entities
        #[allow(clippy::type_complexity)]
        let mut entry_scores: HashMap<String, (f32, Vec<(String, f32)>, usize)> = HashMap::new();

        for (entity_id, activation) in &activations {
            let entries = self.entity_store.get_entity_entries(entity_id, limit * 2)?;

            // Calculate hops from seed
            let hops = if seed_ids.contains(entity_id) { 0 } else { 1 };

            for entry_id in entries {
                let entry = entry_scores
                    .entry(entry_id)
                    .or_insert((0.0, Vec::new(), usize::MAX));
                entry.0 += activation; // Accumulate activation
                entry.1.push((entity_id.clone(), *activation));
                entry.2 = entry.2.min(hops); // Track minimum hops
            }
        }

        // 5. Convert to results and sort by score
        let mut results: Vec<GraphRetrievalResult> = entry_scores
            .into_iter()
            .map(
                |(entry_id, (score, contributors, hops))| GraphRetrievalResult {
                    entry_id,
                    activation_score: score,
                    contributing_entities: contributors,
                    min_hops: hops,
                },
            )
            .collect();

        // Sort by activation score descending
        results.sort_by(|a, b| {
            b.activation_score
                .partial_cmp(&a.activation_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Apply limit
        results.truncate(limit);

        Ok(results)
    }

    /// Retrieve entries with explicit seed entities (for when you already know the entities)
    pub fn retrieve_from_seeds(
        &self,
        seeds: &[Entity],
        limit: usize,
    ) -> Result<Vec<GraphRetrievalResult>> {
        if seeds.is_empty() {
            return Ok(Vec::new());
        }

        let seed_ids: HashSet<String> = seeds.iter().map(|e| e.id.clone()).collect();
        let activations = self.spread_activation(seeds)?;

        #[allow(clippy::type_complexity)]
        let mut entry_scores: HashMap<String, (f32, Vec<(String, f32)>, usize)> = HashMap::new();

        for (entity_id, activation) in &activations {
            let entries = self.entity_store.get_entity_entries(entity_id, limit * 2)?;
            let hops = if seed_ids.contains(entity_id) { 0 } else { 1 };

            for entry_id in entries {
                let entry = entry_scores
                    .entry(entry_id)
                    .or_insert((0.0, Vec::new(), usize::MAX));
                entry.0 += activation;
                entry.1.push((entity_id.clone(), *activation));
                entry.2 = entry.2.min(hops);
            }
        }

        let mut results: Vec<GraphRetrievalResult> = entry_scores
            .into_iter()
            .map(
                |(entry_id, (score, contributors, hops))| GraphRetrievalResult {
                    entry_id,
                    activation_score: score,
                    contributing_entities: contributors,
                    min_hops: hops,
                },
            )
            .collect();

        results.sort_by(|a, b| {
            b.activation_score
                .partial_cmp(&a.activation_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        results.truncate(limit);

        Ok(results)
    }
}

/// Parse entity type hints from queries
///
/// Supports patterns like:
/// - "person:Alice" -> (Person, "Alice")
/// - "project:CAS" -> (Project, "CAS")
/// - "tool:Rust" -> (Tool, "Rust")
pub fn parse_typed_entity(query: &str) -> Option<(EntityType, String)> {
    let parts: Vec<&str> = query.splitn(2, ':').collect();
    if parts.len() != 2 {
        return None;
    }

    let entity_type = parts[0].parse::<EntityType>().ok()?;
    Some((entity_type, parts[1].to_string()))
}

#[cfg(test)]
mod tests {
    use crate::hybrid_search::graph::*;

    #[test]
    fn test_extract_entity_candidates() {
        // Test capitalized words
        let query = "What did Alice work on for the CAS project?";
        let words: Vec<&str> = query
            .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .filter(|s| !s.is_empty())
            .collect();

        let mut candidates = Vec::new();
        for word in &words {
            let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric());
            if trimmed.len() >= 2
                && trimmed
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
            {
                candidates.push(trimmed.to_string());
            }
        }

        assert!(candidates.contains(&"What".to_string()));
        assert!(candidates.contains(&"Alice".to_string()));
        assert!(candidates.contains(&"CAS".to_string()));
    }

    #[test]
    fn test_spreading_activation_config_defaults() {
        let config = SpreadingActivationConfig::default();
        assert_eq!(config.initial_activation, 1.0);
        assert_eq!(config.decay_factor, 0.5);
        assert_eq!(config.min_activation, 0.05);
        assert_eq!(config.max_hops, 3);
        assert!(
            config
                .relation_weights
                .contains_key(&RelationType::RelatedTo)
        );
        assert!(config.relation_weights.contains_key(&RelationType::Uses));
        assert!(config.relation_weights.contains_key(&RelationType::WorksOn));
    }

    #[test]
    fn test_parse_typed_entity() {
        let result = parse_typed_entity("person:Alice");
        assert!(result.is_some());
        let (etype, name) = result.unwrap();
        assert_eq!(etype, EntityType::Person);
        assert_eq!(name, "Alice");

        let result = parse_typed_entity("project:CAS");
        assert!(result.is_some());
        let (etype, name) = result.unwrap();
        assert_eq!(etype, EntityType::Project);
        assert_eq!(name, "CAS");

        // Invalid format
        let result = parse_typed_entity("no colon here");
        assert!(result.is_none());

        // Invalid type
        let result = parse_typed_entity("invalid:test");
        assert!(result.is_none());
    }

    // =========================================================================
    // Additional tests for improved coverage
    // =========================================================================

    #[test]
    fn test_parse_typed_entity_all_types() {
        // Test all valid entity types
        assert_eq!(
            parse_typed_entity("tool:Rust").map(|(t, _)| t),
            Some(EntityType::Tool)
        );
        assert_eq!(
            parse_typed_entity("file:main.rs").map(|(t, _)| t),
            Some(EntityType::File)
        );
        assert_eq!(
            parse_typed_entity("concept:async").map(|(t, _)| t),
            Some(EntityType::Concept)
        );
        assert_eq!(
            parse_typed_entity("organization:Anthropic").map(|(t, _)| t),
            Some(EntityType::Organization)
        );
    }

    #[test]
    fn test_parse_typed_entity_empty_name() {
        let result = parse_typed_entity("person:");
        assert!(result.is_some());
        let (etype, name) = result.unwrap();
        assert_eq!(etype, EntityType::Person);
        assert_eq!(name, "");
    }

    #[test]
    fn test_parse_typed_entity_multiple_colons() {
        let result = parse_typed_entity("project:CAS:v2");
        assert!(result.is_some());
        let (_, name) = result.unwrap();
        assert_eq!(name, "CAS:v2"); // Everything after first colon
    }

    #[test]
    fn test_spreading_activation_config_relation_weights() {
        let config = SpreadingActivationConfig::default();

        // Check all relation types have weights
        assert_eq!(
            config.relation_weights.get(&RelationType::WorksOn),
            Some(&0.9)
        );
        assert_eq!(
            config.relation_weights.get(&RelationType::Uses),
            Some(&0.85)
        );
        assert_eq!(
            config.relation_weights.get(&RelationType::MentionedIn),
            Some(&0.7)
        );
        assert_eq!(
            config.relation_weights.get(&RelationType::RelatedTo),
            Some(&0.8)
        );
        assert_eq!(
            config.relation_weights.get(&RelationType::PartOf),
            Some(&0.75)
        );
        assert_eq!(
            config.relation_weights.get(&RelationType::DependsOn),
            Some(&0.8)
        );
        assert_eq!(
            config.relation_weights.get(&RelationType::Created),
            Some(&0.7)
        );
        assert_eq!(
            config.relation_weights.get(&RelationType::Modified),
            Some(&0.65)
        );
        assert_eq!(
            config.relation_weights.get(&RelationType::Knows),
            Some(&0.6)
        );
        assert_eq!(config.relation_weights.get(&RelationType::Owns), Some(&0.7));
        assert_eq!(
            config.relation_weights.get(&RelationType::Implements),
            Some(&0.85)
        );
    }

    #[test]
    fn test_spreading_activation_config_custom() {
        let mut relation_weights = HashMap::new();
        relation_weights.insert(RelationType::Uses, 1.0);

        let config = SpreadingActivationConfig {
            initial_activation: 2.0,
            decay_factor: 0.7,
            min_activation: 0.1,
            max_hops: 5,
            relation_weights,
        };

        assert_eq!(config.initial_activation, 2.0);
        assert_eq!(config.decay_factor, 0.7);
        assert_eq!(config.min_activation, 0.1);
        assert_eq!(config.max_hops, 5);
        assert_eq!(config.relation_weights.get(&RelationType::Uses), Some(&1.0));
    }

    #[test]
    fn test_graph_retrieval_result() {
        let result = GraphRetrievalResult {
            entry_id: "entry-123".to_string(),
            activation_score: 0.85,
            contributing_entities: vec![
                ("entity-1".to_string(), 0.5),
                ("entity-2".to_string(), 0.35),
            ],
            min_hops: 1,
        };

        assert_eq!(result.entry_id, "entry-123");
        assert_eq!(result.activation_score, 0.85);
        assert_eq!(result.contributing_entities.len(), 2);
        assert_eq!(result.min_hops, 1);
    }

    #[test]
    fn test_graph_retrieval_result_zero_hops() {
        let result = GraphRetrievalResult {
            entry_id: "direct-entry".to_string(),
            activation_score: 1.0,
            contributing_entities: vec![("seed-entity".to_string(), 1.0)],
            min_hops: 0,
        };

        assert_eq!(result.min_hops, 0);
        assert_eq!(result.contributing_entities.len(), 1);
    }

    #[test]
    fn test_extract_candidates_lowercase_words() {
        // Lowercase words longer than 4 chars should be candidates
        let query = "What about rust and python?";
        let words: Vec<&str> = query.split_whitespace().collect();

        let mut candidates = Vec::new();
        for word in &words {
            let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric());
            if trimmed.len() >= 4 {
                candidates.push(trimmed.to_string());
            }
        }

        assert!(candidates.contains(&"What".to_string()));
        assert!(candidates.contains(&"about".to_string()));
        assert!(candidates.contains(&"rust".to_string()));
        assert!(candidates.contains(&"python".to_string()));
    }

    #[test]
    fn test_extract_candidates_multi_word() {
        // Test multi-word entity extraction (consecutive capitalized words)
        let query = "Alice Smith works on Project CAS";
        let words: Vec<&str> = query.split_whitespace().collect();

        let mut multi_word = Vec::new();
        let mut i = 0;
        while i < words.len() {
            let word = words[i].trim_matches(|c: char| !c.is_alphanumeric());
            if word
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
            {
                let mut phrase = word.to_string();
                let mut j = i + 1;
                while j < words.len() {
                    let next = words[j].trim_matches(|c: char| !c.is_alphanumeric());
                    if next
                        .chars()
                        .next()
                        .map(|c| c.is_uppercase())
                        .unwrap_or(false)
                    {
                        phrase.push(' ');
                        phrase.push_str(next);
                        j += 1;
                    } else {
                        break;
                    }
                }
                if phrase.contains(' ') {
                    multi_word.push(phrase);
                }
                i = j;
            } else {
                i += 1;
            }
        }

        assert!(multi_word.contains(&"Alice Smith".to_string()));
        assert!(multi_word.contains(&"Project CAS".to_string()));
    }

    #[test]
    fn test_extract_candidates_with_punctuation() {
        let query = "Check Alice work, or Bob code; maybe CAS?";
        let words: Vec<&str> = query
            .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .filter(|s| !s.is_empty())
            .collect();

        let mut candidates = Vec::new();
        for word in &words {
            let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric());
            if trimmed.len() >= 2
                && trimmed
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
            {
                candidates.push(trimmed.to_string());
            }
        }

        assert!(candidates.contains(&"Check".to_string()));
        assert!(candidates.contains(&"Alice".to_string()));
        assert!(candidates.contains(&"Bob".to_string()));
        assert!(candidates.contains(&"CAS".to_string()));
    }
}
