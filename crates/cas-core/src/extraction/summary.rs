//! Entity summary generation (Hindsight observation network)
//!
//! This module generates summaries for entities from accumulated facts
//! in memory entries. Like Hindsight's observation network, it consolidates
//! distributed knowledge about an entity into a coherent summary.
//!
//! # How It Works
//!
//! 1. Collect all entries that mention the entity
//! 2. Extract relevant sentences/facts about the entity
//! 3. Deduplicate and rank facts by importance
//! 4. Generate a coherent summary from top facts
//!
//! # Integration Status
//! Integrated with daemon for automatic entity summary updates.

//! # Usage
//!
//! ```rust,ignore
//! let generator = SummaryGenerator::new();
//! let summary = generator.generate_entity_summary(&entity, &entries)?;
//! ```

use chrono::Utc;

use crate::error::Result;
use cas_store::{EntityStore, Store};
use cas_types::{Entity, Entry};

/// Configuration for summary generation
#[derive(Debug, Clone)]
pub struct SummaryConfig {
    /// Maximum number of facts to include in summary
    pub max_facts: usize,
    /// Minimum confidence for a fact to be included
    pub min_confidence: f32,
    /// Maximum summary length in characters
    pub max_length: usize,
    /// Include temporal context (when facts were learned)
    pub include_temporal: bool,
    /// Include source citations
    pub include_sources: bool,
}

impl Default for SummaryConfig {
    fn default() -> Self {
        Self {
            max_facts: 10,
            min_confidence: 0.5,
            max_length: 500,
            include_temporal: false,
            include_sources: false,
        }
    }
}

/// A fact extracted from an entry about an entity
#[derive(Debug, Clone)]
pub struct ExtractedFact {
    /// The fact content
    pub content: String,
    /// Source entry ID
    pub source_id: String,
    /// Confidence in this fact (0.0-1.0)
    pub confidence: f32,
    /// When this fact was recorded
    pub timestamp: chrono::DateTime<Utc>,
    /// Relevance score to the entity
    pub relevance: f32,
}

/// Summary generator for entities
pub struct SummaryGenerator {
    config: SummaryConfig,
}

impl SummaryGenerator {
    /// Create a new summary generator with default config
    pub fn new() -> Self {
        Self {
            config: SummaryConfig::default(),
        }
    }

    /// Create with custom config
    pub fn with_config(config: SummaryConfig) -> Self {
        Self { config }
    }

    /// Generate a summary for an entity from related entries
    pub fn generate_summary(&self, entity: &Entity, entries: &[Entry]) -> Result<String> {
        // Extract facts about the entity from entries
        let facts = self.extract_facts(entity, entries);

        if facts.is_empty() {
            return Ok(format!(
                "{} is a {} with {} recorded mention(s).",
                entity.name, entity.entity_type, entity.mention_count
            ));
        }

        // Rank facts by relevance and confidence
        let mut ranked_facts = facts;
        ranked_facts.sort_by(|a, b| {
            let score_a = a.relevance * a.confidence;
            let score_b = b.relevance * b.confidence;
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top facts
        let top_facts: Vec<_> = ranked_facts
            .into_iter()
            .take(self.config.max_facts)
            .collect();

        // Generate summary text
        self.compose_summary(entity, &top_facts)
    }

    /// Extract facts about an entity from entries
    fn extract_facts(&self, entity: &Entity, entries: &[Entry]) -> Vec<ExtractedFact> {
        let mut facts = Vec::new();
        let entity_name_lower = entity.name.to_lowercase();
        let aliases_lower: Vec<String> = entity.aliases.iter().map(|a| a.to_lowercase()).collect();

        for entry in entries {
            // Skip archived entries
            if entry.archived {
                continue;
            }

            let content_lower = entry.content.to_lowercase();

            // Check if entry mentions the entity
            let mentions_entity = content_lower.contains(&entity_name_lower)
                || aliases_lower.iter().any(|a| content_lower.contains(a));

            if !mentions_entity {
                continue;
            }

            // Extract sentences that mention the entity
            let sentences = self.extract_relevant_sentences(&entry.content, entity);

            for sentence in sentences {
                let relevance = self.calculate_relevance(&sentence, entity);

                if relevance >= self.config.min_confidence {
                    facts.push(ExtractedFact {
                        content: sentence,
                        source_id: entry.id.clone(),
                        confidence: entry.confidence,
                        timestamp: entry.created,
                        relevance,
                    });
                }
            }
        }

        // Deduplicate similar facts
        self.deduplicate_facts(facts)
    }

    /// Extract sentences from content that mention the entity
    fn extract_relevant_sentences(&self, content: &str, entity: &Entity) -> Vec<String> {
        let mut sentences = Vec::new();

        // Split into sentences (simple heuristic)
        let sentence_endings = ['.', '!', '?', '\n'];
        let mut current = String::new();

        for ch in content.chars() {
            current.push(ch);
            if sentence_endings.contains(&ch) {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() && self.sentence_mentions_entity(&trimmed, entity) {
                    sentences.push(trimmed);
                }
                current.clear();
            }
        }

        // Handle last sentence without ending punctuation
        let trimmed = current.trim().to_string();
        if !trimmed.is_empty() && self.sentence_mentions_entity(&trimmed, entity) {
            sentences.push(trimmed);
        }

        sentences
    }

    /// Check if a sentence mentions the entity
    fn sentence_mentions_entity(&self, sentence: &str, entity: &Entity) -> bool {
        let lower = sentence.to_lowercase();
        lower.contains(&entity.name.to_lowercase())
            || entity
                .aliases
                .iter()
                .any(|a| lower.contains(&a.to_lowercase()))
    }

    /// Calculate relevance score for a fact
    fn calculate_relevance(&self, sentence: &str, entity: &Entity) -> f32 {
        let lower = sentence.to_lowercase();
        let mut score: f32 = 0.0;

        // Check for entity name (higher score for exact match)
        if lower.contains(&entity.name.to_lowercase()) {
            score += 0.5;
        }

        // Check for entity type keywords
        let type_keywords: Vec<&str> = match entity.entity_type {
            cas_types::EntityType::Person => {
                vec!["person", "developer", "author", "created", "wrote"]
            }
            cas_types::EntityType::Project => {
                vec!["project", "repository", "codebase", "application"]
            }
            cas_types::EntityType::Tool => {
                vec!["tool", "library", "framework", "uses", "implemented"]
            }
            cas_types::EntityType::Concept => vec!["concept", "pattern", "approach", "method"],
            cas_types::EntityType::File => vec!["file", "module", "source", "code", "directory"],
            cas_types::EntityType::Organization => vec!["company", "team", "organization", "group"],
        };

        for keyword in type_keywords {
            if lower.contains(keyword) {
                score += 0.1;
            }
        }

        // Bonus for informative sentence length (not too short, not too long)
        let word_count = sentence.split_whitespace().count();
        if (5..=30).contains(&word_count) {
            score += 0.2;
        }

        // Bonus for action verbs (indicates factual content)
        let action_verbs = [
            "is", "was", "has", "does", "can", "will", "uses", "provides", "creates",
        ];
        for verb in action_verbs {
            if lower.contains(verb) {
                score += 0.1;
                break;
            }
        }

        score.min(1.0)
    }

    /// Deduplicate similar facts using pre-computed word sets.
    ///
    /// Tokenizes each fact once upfront instead of re-tokenizing on every
    /// pairwise comparison, reducing per-comparison cost from O(W) to O(min(W1,W2)).
    fn deduplicate_facts(&self, facts: Vec<ExtractedFact>) -> Vec<ExtractedFact> {
        use std::collections::HashSet;

        // Pre-compute word sets for all facts (tokenize once)
        let word_sets: Vec<HashSet<String>> = facts
            .iter()
            .map(|f| {
                f.content
                    .to_lowercase()
                    .split_whitespace()
                    .filter(|w| w.len() > 3)
                    .map(|w| w.to_string())
                    .collect()
            })
            .collect();

        let mut unique_indices: Vec<usize> = Vec::new();

        for (i, words) in word_sets.iter().enumerate() {
            if words.is_empty() {
                unique_indices.push(i);
                continue;
            }

            let is_duplicate = unique_indices.iter().any(|&j| {
                let existing = &word_sets[j];
                if existing.is_empty() {
                    return false;
                }
                let intersection = words.intersection(existing).count();
                let union = words.union(existing).count();
                // Jaccard similarity > 0.6 means likely duplicate
                (intersection as f32 / union as f32) > 0.6
            });

            if !is_duplicate {
                unique_indices.push(i);
            }
        }

        // Collect unique facts by index
        let mut unique_set: HashSet<usize> = unique_indices.iter().copied().collect();
        let mut result = Vec::with_capacity(unique_indices.len());
        for (i, fact) in facts.into_iter().enumerate() {
            if unique_set.remove(&i) {
                result.push(fact);
            }
        }
        result
    }

    /// Check if two facts are similar (Jaccard similarity > 0.6 on words > 3 chars)
    #[cfg(test)]
    fn facts_similar(&self, a: &str, b: &str) -> bool {
        use std::collections::HashSet;
        let words_a: HashSet<String> = a
            .to_lowercase()
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .map(|w| w.to_string())
            .collect();
        let words_b: HashSet<String> = b
            .to_lowercase()
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .map(|w| w.to_string())
            .collect();
        if words_a.is_empty() || words_b.is_empty() {
            return false;
        }
        let intersection = words_a.intersection(&words_b).count();
        let union = words_a.union(&words_b).count();
        (intersection as f32 / union as f32) > 0.6
    }

    /// Compose a summary from extracted facts
    fn compose_summary(&self, entity: &Entity, facts: &[ExtractedFact]) -> Result<String> {
        let mut summary = String::new();

        // Opening statement
        summary.push_str(&format!(
            "{} is a {} ",
            entity.name,
            entity.entity_type.to_string().to_lowercase()
        ));

        if let Some(ref desc) = entity.description {
            summary.push_str(&format!("described as: {desc}. "));
        } else {
            summary.push_str("with the following characteristics: ");
        }

        // Add top facts
        let mut char_count = summary.len();
        let mut fact_count = 0;

        for fact in facts {
            if char_count + fact.content.len() > self.config.max_length {
                break;
            }

            // Clean up the fact
            let clean_fact = fact.content.trim();

            if self.config.include_temporal {
                let date = fact.timestamp.format("%Y-%m-%d").to_string();
                summary.push_str(&format!("[{date}] "));
            }

            summary.push_str(clean_fact);

            if !clean_fact.ends_with('.') {
                summary.push('.');
            }
            summary.push(' ');

            char_count = summary.len();
            fact_count += 1;
        }

        // Add mention count if we have room
        if fact_count > 0 && char_count + 50 < self.config.max_length {
            summary.push_str(&format!(
                "(Based on {} recorded mention{}.)",
                entity.mention_count,
                if entity.mention_count != 1 { "s" } else { "" }
            ));
        }

        Ok(summary.trim().to_string())
    }

    /// Generate summaries for entities that need updating
    pub fn generate_for_stale_entities(
        &self,
        entity_store: &dyn EntityStore,
        store: &dyn Store,
        max_age_days: i64,
    ) -> Result<Vec<(String, String)>> {
        let mut updates = Vec::new();
        let now = Utc::now();
        let max_age = chrono::Duration::days(max_age_days);

        // Get all entities
        let entities = entity_store.list_entities(None)?;

        for entity in entities {
            // Check if summary needs updating
            let needs_update = match entity.summary_updated {
                None => true,
                Some(updated) => (now - updated) > max_age,
            };

            if needs_update && entity.mention_count > 0 {
                // Get entries that mention this entity
                let entry_ids = entity_store.get_entity_entries(&entity.id, 50)?;
                let mut entries = Vec::new();

                for id in entry_ids {
                    if let Ok(entry) = store.get(&id) {
                        entries.push(entry);
                    }
                }

                if !entries.is_empty() {
                    let summary = self.generate_summary(&entity, &entries)?;
                    updates.push((entity.id, summary));
                }
            }
        }

        Ok(updates)
    }
}

impl Default for SummaryGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// Update entity summaries in the entity store
pub fn update_entity_summaries(
    entity_store: &dyn EntityStore,
    _store: &dyn Store,
    updates: &[(String, String)],
) -> Result<usize> {
    let mut count = 0;

    for (entity_id, summary) in updates {
        if let Ok(mut entity) = entity_store.get_entity(entity_id) {
            entity.summary = Some(summary.clone());
            entity.summary_updated = Some(Utc::now());

            if entity_store.update_entity(&entity).is_ok() {
                count += 1;
            }
        }
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use crate::extraction::summary::*;
    use cas_types::{Entity, EntityType, Entry, EntryType};

    fn create_test_entity() -> Entity {
        Entity::new("ent-001".to_string(), "Rust".to_string(), EntityType::Tool)
    }

    fn create_test_entries() -> Vec<Entry> {
        vec![
            Entry {
                id: "e001".to_string(),
                content: "Rust is a systems programming language focused on safety and performance.".to_string(),
                entry_type: EntryType::Learning,
                confidence: 0.9,
                ..Default::default()
            },
            Entry {
                id: "e002".to_string(),
                content: "Rust uses ownership and borrowing to ensure memory safety without garbage collection.".to_string(),
                entry_type: EntryType::Learning,
                confidence: 0.85,
                ..Default::default()
            },
            Entry {
                id: "e003".to_string(),
                content: "The Rust compiler provides helpful error messages.".to_string(),
                entry_type: EntryType::Learning,
                confidence: 0.8,
                ..Default::default()
            },
        ]
    }

    #[test]
    fn test_extract_facts() {
        let generator = SummaryGenerator::new();
        let entity = create_test_entity();
        let entries = create_test_entries();

        let facts = generator.extract_facts(&entity, &entries);

        assert!(!facts.is_empty());
        assert!(
            facts
                .iter()
                .all(|f| f.content.to_lowercase().contains("rust"))
        );
    }

    #[test]
    fn test_generate_summary() {
        let generator = SummaryGenerator::new();
        let entity = create_test_entity();
        let entries = create_test_entries();

        let summary = generator.generate_summary(&entity, &entries).unwrap();

        assert!(summary.contains("Rust"));
        assert!(summary.contains("tool"));
    }

    #[test]
    fn test_empty_entries() {
        let generator = SummaryGenerator::new();
        let entity = create_test_entity();
        let entries: Vec<Entry> = vec![];

        let summary = generator.generate_summary(&entity, &entries).unwrap();

        assert!(summary.contains("Rust"));
        assert!(summary.contains("tool"));
    }

    #[test]
    fn test_facts_similar() {
        let generator = SummaryGenerator::new();

        // Very similar sentences (high word overlap)
        assert!(generator.facts_similar(
            "Rust is a systems programming language for safety",
            "Rust is a systems programming language focused on safety"
        ));

        // Different sentences
        assert!(!generator.facts_similar(
            "Rust is a programming language",
            "Python is used for data science"
        ));
    }

    #[test]
    fn test_calculate_relevance() {
        let generator = SummaryGenerator::new();
        let entity = create_test_entity();

        let high_relevance = "Rust is a tool used for systems programming";
        let low_relevance = "Something unrelated";

        let high_score = generator.calculate_relevance(high_relevance, &entity);
        let low_score = generator.calculate_relevance(low_relevance, &entity);

        assert!(high_score > low_score);
    }

    #[test]
    fn test_summary_config_default() {
        let config = SummaryConfig::default();
        assert_eq!(config.max_facts, 10);
        assert_eq!(config.min_confidence, 0.5);
        assert_eq!(config.max_length, 500);
        assert!(!config.include_temporal);
        assert!(!config.include_sources);
    }

    #[test]
    fn test_summary_with_custom_config() {
        let config = SummaryConfig {
            max_facts: 5,
            min_confidence: 0.7,
            max_length: 300,
            include_temporal: true,
            include_sources: false,
        };
        let generator = SummaryGenerator::with_config(config);
        let entity = create_test_entity();
        let entries = create_test_entries();

        let summary = generator.generate_summary(&entity, &entries).unwrap();
        assert!(summary.len() <= 300 + 100); // Allow some margin for formatting
    }
}
