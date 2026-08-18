//! Entity-aware search for knowledge graph queries
//!
//! Enables queries like:
//! - "memories about project X"
//! - "what did person Y work on"
//! - "entries mentioning tool Z"
//!
//! Combines entity lookup with traditional text search.

use std::sync::Arc;

/// Maximum number of related entities to return in search results.
/// Prevents memory issues when entities have many relationships.
const MAX_RELATED_ENTITIES: usize = 50;

use crate::error::CasError;
use crate::store::EntityStore;
use crate::types::{Entity, EntityType};

/// Parsed entity query
#[derive(Debug, Clone)]
pub struct EntityQuery {
    /// Entity name or pattern to search for
    pub entity_name: String,

    /// Optional entity type filter
    pub entity_type: Option<EntityType>,

    /// Query intent (about, by, uses, etc.)
    pub intent: QueryIntent,

    /// Remaining text query to combine with entity search
    pub text_query: Option<String>,
}

/// Intent of the entity query
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryIntent {
    /// Find entries about/mentioning an entity
    About,
    /// Find entries created by a person
    By,
    /// Find entities that use something
    Uses,
    /// Find entities related to something
    RelatedTo,
    /// Find entities working on something
    WorksOn,
    /// General entity mention
    Mentions,
}

impl EntityQuery {
    /// Parse a query string for entity references
    ///
    /// Supported formats:
    /// - `about:entity_name` - entries about the entity
    /// - `by:person_name` - entries by person
    /// - `uses:tool_name` - entities using tool
    /// - `@entity_name` - shorthand for about
    /// - `entity:type:name` - explicit type filter
    ///
    /// Returns None if no entity query pattern is found.
    pub fn parse(query: &str) -> Option<Self> {
        let query = query.trim();

        // Check for prefix patterns
        if let Some(rest) = query.strip_prefix("about:") {
            return Some(Self::with_intent(rest.trim(), QueryIntent::About));
        }

        if let Some(rest) = query.strip_prefix("by:") {
            return Some(Self {
                entity_name: rest.trim().to_string(),
                entity_type: Some(EntityType::Person),
                intent: QueryIntent::By,
                text_query: None,
            });
        }

        if let Some(rest) = query.strip_prefix("uses:") {
            return Some(Self {
                entity_name: rest.trim().to_string(),
                entity_type: Some(EntityType::Tool),
                intent: QueryIntent::Uses,
                text_query: None,
            });
        }

        if let Some(rest) = query.strip_prefix("related:") {
            return Some(Self::with_intent(rest.trim(), QueryIntent::RelatedTo));
        }

        if let Some(rest) = query.strip_prefix("workson:") {
            return Some(Self {
                entity_name: rest.trim().to_string(),
                entity_type: Some(EntityType::Project),
                intent: QueryIntent::WorksOn,
                text_query: None,
            });
        }

        // @ shorthand
        if let Some(rest) = query.strip_prefix('@') {
            return Some(Self::with_intent(rest.trim(), QueryIntent::Mentions));
        }

        // entity:type:name format
        if query.starts_with("entity:") {
            let parts: Vec<&str> = query.splitn(3, ':').collect();
            if parts.len() >= 3 {
                let entity_type = parts[1].parse().ok();
                return Some(Self {
                    entity_name: parts[2].to_string(),
                    entity_type,
                    intent: QueryIntent::About,
                    text_query: None,
                });
            }
        }

        // Check for natural language patterns
        let lower = query.to_lowercase();

        // "memories about X" or "entries about X"
        if let Some(pos) = lower.find(" about ") {
            let entity_part = &query[pos + 7..];
            return Some(Self::with_intent(entity_part.trim(), QueryIntent::About));
        }

        // "what did X work on" or "X's work"
        if lower.contains("work on") || lower.contains("works on") {
            // Extract entity name before "work"
            if let Some(pos) = lower.find(" work") {
                let entity_part = &query[..pos];
                // Remove "what did " prefix if present
                let entity_part = entity_part
                    .strip_prefix("what did ")
                    .or_else(|| entity_part.strip_prefix("what does "))
                    .unwrap_or(entity_part)
                    .trim();

                if !entity_part.is_empty() {
                    return Some(Self {
                        entity_name: entity_part.to_string(),
                        entity_type: Some(EntityType::Person),
                        intent: QueryIntent::WorksOn,
                        text_query: None,
                    });
                }
            }
        }

        // "entries mentioning X"
        if let Some(pos) = lower.find("mentioning ") {
            let entity_part = &query[pos + 11..];
            return Some(Self::with_intent(entity_part.trim(), QueryIntent::Mentions));
        }

        None
    }

    fn with_intent(name: &str, intent: QueryIntent) -> Self {
        // Check if there's a type prefix like "project:Cassy"
        if let Some((type_str, entity_name)) = name.split_once(':') {
            let entity_type = type_str.parse().ok();
            Self {
                entity_name: entity_name.to_string(),
                entity_type,
                intent,
                text_query: None,
            }
        } else {
            Self {
                entity_name: name.to_string(),
                entity_type: None,
                intent,
                text_query: None,
            }
        }
    }

    /// Check if this query has an entity component
    pub fn has_entity(&self) -> bool {
        !self.entity_name.is_empty()
    }
}

/// Result of an entity-aware search
#[derive(Debug, Clone)]
pub struct EntitySearchResult {
    /// Matched entity (if found)
    pub entity: Option<Entity>,

    /// Entry IDs that mention this entity
    pub entry_ids: Vec<String>,

    /// Related entities (via relationships)
    pub related_entities: Vec<Entity>,

    /// Confidence of the entity match
    pub confidence: f32,
}

/// Entity-aware search engine
pub struct EntitySearch {
    entity_store: Arc<dyn EntityStore>,
}

impl EntitySearch {
    /// Create a new entity search engine
    pub fn new(entity_store: Arc<dyn EntityStore>) -> Self {
        Self { entity_store }
    }

    /// Search for entries related to an entity
    pub fn search(
        &self,
        query: &EntityQuery,
        limit: usize,
    ) -> Result<EntitySearchResult, CasError> {
        // Find the entity by name
        let entity = self
            .entity_store
            .get_entity_by_name(&query.entity_name, query.entity_type)?;

        if let Some(ref ent) = entity {
            // Get entries that mention this entity
            let entry_ids = self.entity_store.get_entity_entries(&ent.id, limit)?;

            // Get related entities via relationships (limited to prevent memory issues)
            let connected = self.entity_store.get_connected_entities(&ent.id)?;
            let related_entities: Vec<Entity> = connected
                .into_iter()
                .take(MAX_RELATED_ENTITIES)
                .map(|(e, _)| e)
                .collect();

            Ok(EntitySearchResult {
                entity: Some(ent.clone()),
                entry_ids,
                related_entities,
                confidence: ent.confidence,
            })
        } else {
            // Entity not found - try fuzzy search
            let entities = self
                .entity_store
                .search_entities(&query.entity_name, query.entity_type)?;

            if let Some(ent) = entities.first() {
                let entry_ids = self.entity_store.get_entity_entries(&ent.id, limit)?;
                let connected = self.entity_store.get_connected_entities(&ent.id)?;
                let related_entities: Vec<Entity> = connected
                    .into_iter()
                    .take(MAX_RELATED_ENTITIES)
                    .map(|(e, _)| e)
                    .collect();

                Ok(EntitySearchResult {
                    entity: Some(ent.clone()),
                    entry_ids,
                    related_entities,
                    confidence: ent.confidence * 0.8, // Lower confidence for fuzzy match
                })
            } else {
                Ok(EntitySearchResult {
                    entity: None,
                    entry_ids: vec![],
                    related_entities: vec![],
                    confidence: 0.0,
                })
            }
        }
    }

    /// Get entries for a specific entity ID
    pub fn get_entity_entries(
        &self,
        entity_id: &str,
        limit: usize,
    ) -> Result<Vec<String>, CasError> {
        Ok(self.entity_store.get_entity_entries(entity_id, limit)?)
    }

    /// Find entities matching a pattern
    pub fn find_entities(
        &self,
        query: &str,
        entity_type: Option<EntityType>,
    ) -> Result<Vec<Entity>, CasError> {
        Ok(self.entity_store.search_entities(query, entity_type)?)
    }
}

/// Combine entity search with text search results
pub fn combine_results(
    entity_result: &EntitySearchResult,
    text_result_ids: &[String],
    entity_weight: f32,
) -> Vec<(String, f32)> {
    use std::collections::HashMap;

    let mut scores: HashMap<String, f32> = HashMap::new();

    // Add entity-based entries with entity weight
    for id in &entity_result.entry_ids {
        *scores.entry(id.clone()).or_insert(0.0) += entity_weight * entity_result.confidence;
    }

    // Add text search results with remaining weight
    let text_weight = 1.0 - entity_weight;
    for (rank, id) in text_result_ids.iter().enumerate() {
        // RRF-style scoring: 1/(k + rank)
        let rrf_score = 1.0 / (60.0 + rank as f32);
        *scores.entry(id.clone()).or_insert(0.0) += text_weight * rrf_score;
    }

    // Sort by combined score
    let mut results: Vec<_> = scores.into_iter().collect();
    results.sort_by(|a, b| b.1.total_cmp(&a.1));

    results
}

#[cfg(test)]
mod tests {
    use crate::hybrid_search::entity_search::*;

    #[test]
    fn test_parse_about_query() {
        let query = EntityQuery::parse("about:Cassy").unwrap();
        assert_eq!(query.entity_name, "Cassy");
        assert_eq!(query.intent, QueryIntent::About);
        assert!(query.entity_type.is_none());
    }

    #[test]
    fn test_parse_by_query() {
        let query = EntityQuery::parse("by:Alice").unwrap();
        assert_eq!(query.entity_name, "Alice");
        assert_eq!(query.entity_type, Some(EntityType::Person));
        assert_eq!(query.intent, QueryIntent::By);
    }

    #[test]
    fn test_parse_uses_query() {
        let query = EntityQuery::parse("uses:Rust").unwrap();
        assert_eq!(query.entity_name, "Rust");
        assert_eq!(query.entity_type, Some(EntityType::Tool));
        assert_eq!(query.intent, QueryIntent::Uses);
    }

    #[test]
    fn test_parse_at_shorthand() {
        let query = EntityQuery::parse("@SQLite").unwrap();
        assert_eq!(query.entity_name, "SQLite");
        assert_eq!(query.intent, QueryIntent::Mentions);
    }

    #[test]
    fn test_parse_typed_entity() {
        let query = EntityQuery::parse("about:project:Cassy").unwrap();
        assert_eq!(query.entity_name, "Cassy");
        assert_eq!(query.entity_type, Some(EntityType::Project));
    }

    #[test]
    fn test_parse_entity_type_name() {
        let query = EntityQuery::parse("entity:tool:Rust").unwrap();
        assert_eq!(query.entity_name, "Rust");
        assert_eq!(query.entity_type, Some(EntityType::Tool));
    }

    #[test]
    fn test_parse_natural_about() {
        let query = EntityQuery::parse("memories about Cassy").unwrap();
        assert_eq!(query.entity_name, "Cassy");
        assert_eq!(query.intent, QueryIntent::About);
    }

    #[test]
    fn test_parse_natural_works_on() {
        let query = EntityQuery::parse("what did Alice work on").unwrap();
        assert_eq!(query.entity_name, "Alice");
        assert_eq!(query.intent, QueryIntent::WorksOn);
    }

    #[test]
    fn test_parse_mentioning() {
        let query = EntityQuery::parse("entries mentioning Rust").unwrap();
        assert_eq!(query.entity_name, "Rust");
        assert_eq!(query.intent, QueryIntent::Mentions);
    }

    #[test]
    fn test_no_entity_query() {
        let result = EntityQuery::parse("regular search query");
        assert!(result.is_none());
    }

    #[test]
    fn test_combine_results() {
        let entity_result = EntitySearchResult {
            entity: None,
            entry_ids: vec!["e1".to_string(), "e2".to_string()],
            related_entities: vec![],
            confidence: 0.9,
        };

        let text_ids = vec!["e2".to_string(), "e3".to_string(), "e4".to_string()];

        let combined = combine_results(&entity_result, &text_ids, 0.5);

        // e2 should be first (appears in both)
        assert_eq!(combined[0].0, "e2");
        assert!(combined[0].1 > combined[1].1);
    }
}
