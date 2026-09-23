//! Entity extraction from memory entries
//!
//! Extracts entities (people, projects, concepts, files, tools) and their
//! relationships from memory content using AI-powered analysis.

use crate::error::CoreError;
use cas_types::{Entity, EntityMention, EntityType, Entry, RelationType, Relationship};

/// Result of entity extraction from an entry
#[derive(Debug, Clone, Default)]
pub struct EntityExtractionResult {
    /// Extracted entities
    pub entities: Vec<ExtractedEntity>,

    /// Extracted relationships between entities
    pub relationships: Vec<ExtractedRelationship>,

    /// Whether extraction was deferred for later processing
    pub deferred: bool,
}

/// An extracted entity before storage
#[derive(Debug, Clone)]
pub struct ExtractedEntity {
    /// Entity name
    pub name: String,

    /// Entity type
    pub entity_type: EntityType,

    /// Alternative names/aliases found
    pub aliases: Vec<String>,

    /// Description from context
    pub description: Option<String>,

    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,

    /// Position in the source text (character offset)
    pub position: Option<usize>,

    /// The matched text in the source
    pub matched_text: Option<String>,
}

impl ExtractedEntity {
    /// Convert to an Entity for storage
    pub fn to_entity(&self, id: String) -> Entity {
        let mut entity = Entity::new(id, self.name.clone(), self.entity_type);
        entity.aliases = self.aliases.clone();
        entity.description = self.description.clone();
        entity.confidence = self.confidence;
        entity
    }

    /// Create an EntityMention for linking to an entry
    pub fn to_mention(&self, entity_id: String, entry_id: String) -> EntityMention {
        let mut mention = EntityMention::new(entity_id, entry_id);
        mention.position = self.position;
        mention.matched_text = self.matched_text.clone();
        mention.confidence = self.confidence;
        mention
    }
}

/// An extracted relationship before storage
#[derive(Debug, Clone)]
pub struct ExtractedRelationship {
    /// Source entity name
    pub source_name: String,

    /// Target entity name
    pub target_name: String,

    /// Relationship type
    pub relation_type: RelationType,

    /// Description/context
    pub description: Option<String>,

    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,
}

impl ExtractedRelationship {
    /// Convert to a Relationship for storage (requires entity IDs to be resolved)
    pub fn to_relationship(
        &self,
        id: String,
        source_id: String,
        target_id: String,
        entry_id: &str,
    ) -> Relationship {
        let mut rel = Relationship::new(id, source_id, target_id, self.relation_type);
        rel.description = self.description.clone();
        rel.weight = self.confidence;
        rel.source_entries.push(entry_id.to_string());
        rel
    }
}

/// Configuration for entity extraction
#[derive(Debug, Clone)]
pub struct EntityExtractorConfig {
    /// Model to use for extraction
    pub model: String,

    /// Minimum confidence threshold to include entities
    pub min_confidence: f32,

    /// Maximum entities to extract per entry
    pub max_entities: usize,

    /// Maximum relationships to extract per entry
    pub max_relationships: usize,
}

impl Default for EntityExtractorConfig {
    fn default() -> Self {
        Self {
            model: "claude-opus-5-5".to_string(),
            min_confidence: 0.6,
            max_entities: 10,
            max_relationships: 20,
        }
    }
}

/// Entity extractor using Claude
pub struct EntityExtractor {
    config: EntityExtractorConfig,
}

impl EntityExtractor {
    /// Create a new entity extractor
    pub fn new(config: EntityExtractorConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn default_extractor() -> Self {
        Self::new(EntityExtractorConfig::default())
    }

    /// Get the model name for this extractor
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Get the minimum confidence threshold
    pub fn min_confidence(&self) -> f32 {
        self.config.min_confidence
    }

    /// Build the extraction prompt
    pub fn build_prompt(&self, entry: &Entry) -> String {
        let mut prompt = String::new();

        prompt.push_str("Extract entities and relationships from this memory entry.\n\n");
        prompt.push_str("## Entry\n\n");
        prompt.push_str(&format!("**ID:** {}\n", entry.id));
        prompt.push_str(&format!("**Type:** {}\n", entry.entry_type));

        if !entry.tags.is_empty() {
            prompt.push_str(&format!("**Tags:** {}\n", entry.tags.join(", ")));
        }

        prompt.push_str(&format!("\n**Content:**\n{}\n\n", entry.content));

        prompt.push_str("## Entity Types\n\n");
        prompt.push_str("- **person**: People (teammates, users, contributors)\n");
        prompt.push_str("- **project**: Projects, repositories, codebases\n");
        prompt.push_str("- **concept**: Abstract concepts (authentication, caching, patterns)\n");
        prompt.push_str("- **file**: Files, directories, paths\n");
        prompt.push_str("- **tool**: Tools, libraries, frameworks, technologies\n");
        prompt.push_str("- **organization**: Companies, teams, groups\n\n");

        prompt.push_str("## Relationship Types\n\n");
        prompt.push_str("- **works_on**: Person works on project\n");
        prompt.push_str("- **uses**: Entity uses tool/technology\n");
        prompt.push_str("- **part_of**: Hierarchy (file part of project)\n");
        prompt.push_str("- **depends_on**: Dependency relationship\n");
        prompt.push_str("- **related_to**: General semantic relationship\n");
        prompt.push_str("- **created**: Entity created something\n");
        prompt.push_str("- **modified**: Entity modified something\n");
        prompt.push_str("- **implements**: Entity implements pattern/interface\n\n");

        prompt.push_str("## Response Format\n\n");
        prompt.push_str("Respond with JSON only, no markdown:\n");
        prompt.push_str(
            r#"{
  "entities": [
    {
      "name": "Entity Name",
      "type": "person|project|concept|file|tool|organization",
      "aliases": ["alt name"],
      "description": "Brief description",
      "confidence": 0.9
    }
  ],
  "relationships": [
    {
      "source": "Source Entity Name",
      "target": "Target Entity Name",
      "type": "works_on|uses|part_of|depends_on|related_to|created|modified|implements",
      "description": "Why they're related",
      "confidence": 0.8
    }
  ]
}
"#,
        );

        prompt.push_str("\nRules:\n");
        prompt.push_str("- Only extract entities explicitly mentioned\n");
        prompt.push_str("- Use canonical names (not pronouns)\n");
        prompt.push_str("- Confidence 0.0-1.0 based on certainty\n");
        prompt.push_str(&format!(
            "- Max {} entities, {} relationships\n",
            self.config.max_entities, self.config.max_relationships
        ));
        prompt.push_str("- Omit low-confidence extractions\n");

        prompt
    }

    /// Parse the extraction response
    pub fn parse_response(
        &self,
        response: &str,
        _entry_id: &str,
    ) -> Result<EntityExtractionResult, CoreError> {
        // Find JSON in response
        let json_str = response
            .find('{')
            .and_then(|start| response.rfind('}').map(|end| &response[start..=end]))
            .unwrap_or(response);

        let value: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| CoreError::Parse(format!("Failed to parse entity extraction: {e}")))?;

        let mut result = EntityExtractionResult::default();

        // Parse entities
        if let Some(entities) = value.get("entities").and_then(|v| v.as_array()) {
            for item in entities.iter().take(self.config.max_entities) {
                if let Some(entity) = self.parse_entity(item) {
                    if entity.confidence >= self.config.min_confidence {
                        result.entities.push(entity);
                    }
                }
            }
        }

        // Parse relationships
        if let Some(relationships) = value.get("relationships").and_then(|v| v.as_array()) {
            for item in relationships.iter().take(self.config.max_relationships) {
                if let Some(rel) = self.parse_relationship(item) {
                    if rel.confidence >= self.config.min_confidence {
                        result.relationships.push(rel);
                    }
                }
            }
        }

        Ok(result)
    }

    fn parse_entity(&self, value: &serde_json::Value) -> Option<ExtractedEntity> {
        let name = value.get("name")?.as_str()?.to_string();
        let entity_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(EntityType::Concept);

        let aliases = value
            .get("aliases")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let description = value
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from);

        let confidence = value
            .get("confidence")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32)
            .unwrap_or(0.5);

        Some(ExtractedEntity {
            name,
            entity_type,
            aliases,
            description,
            confidence,
            position: None,
            matched_text: None,
        })
    }

    fn parse_relationship(&self, value: &serde_json::Value) -> Option<ExtractedRelationship> {
        let source_name = value.get("source")?.as_str()?.to_string();
        let target_name = value.get("target")?.as_str()?.to_string();

        let relation_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(RelationType::RelatedTo);

        let description = value
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from);

        let confidence = value
            .get("confidence")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32)
            .unwrap_or(0.5);

        Some(ExtractedRelationship {
            source_name,
            target_name,
            relation_type,
            description,
            confidence,
        })
    }

    /// Sync extraction (always defers to async)
    pub fn extract(&self, _entry: &Entry) -> Result<EntityExtractionResult, CoreError> {
        Ok(EntityExtractionResult {
            entities: vec![],
            relationships: vec![],
            deferred: true,
        })
    }
}

/// Simple pattern-based entity extractor (no AI required)
///
/// Uses regex patterns to identify common entity types without AI.
pub struct PatternEntityExtractor {
    /// Minimum entity name length
    pub min_name_length: usize,
}

impl Default for PatternEntityExtractor {
    fn default() -> Self {
        Self { min_name_length: 2 }
    }
}

impl PatternEntityExtractor {
    /// Extract entities using simple patterns
    pub fn extract(&self, entry: &Entry) -> EntityExtractionResult {
        let mut result = EntityExtractionResult::default();
        let content = &entry.content;

        // Extract file paths
        for entity in self.extract_files(content) {
            result.entities.push(entity);
        }

        // Extract tool/technology names (common patterns)
        for entity in self.extract_tools(content) {
            result.entities.push(entity);
        }

        result
    }

    fn extract_files(&self, content: &str) -> Vec<ExtractedEntity> {
        let mut entities = Vec::new();

        // Match file paths like src/main.rs, ./config.toml, etc.
        // Simple pattern: word/word.ext or ./word.ext
        let file_pattern =
            regex::Regex::new(r"(?:^|\s)([.a-zA-Z0-9_/-]+\.[a-zA-Z0-9]{1,10})(?:\s|$|[,;:)])")
                .unwrap();

        for cap in file_pattern.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                let path = m.as_str().to_string();
                // Filter out common non-file patterns
                if path.len() >= self.min_name_length
                    && !path.starts_with('.')
                    && !path.ends_with('.')
                    && path.contains('/')
                    || path.contains('.')
                {
                    entities.push(ExtractedEntity {
                        name: path.clone(),
                        entity_type: EntityType::File,
                        aliases: vec![],
                        description: None,
                        confidence: 0.8,
                        position: Some(m.start()),
                        matched_text: Some(path),
                    });
                }
            }
        }

        entities
    }

    fn extract_tools(&self, content: &str) -> Vec<ExtractedEntity> {
        let mut entities = Vec::new();

        // Common tools/technologies
        let tools = [
            ("Rust", "tool"),
            ("Python", "tool"),
            ("JavaScript", "tool"),
            ("TypeScript", "tool"),
            ("Go", "tool"),
            ("Docker", "tool"),
            ("Kubernetes", "tool"),
            ("PostgreSQL", "tool"),
            ("SQLite", "tool"),
            ("Redis", "tool"),
            ("Git", "tool"),
            ("GitHub", "organization"),
            ("npm", "tool"),
            ("cargo", "tool"),
            ("React", "tool"),
            ("Vue", "tool"),
            ("Node", "tool"),
            ("AWS", "organization"),
            ("Azure", "organization"),
            ("GCP", "organization"),
        ];

        let content_lower = content.to_lowercase();
        for (name, entity_type) in tools {
            if content_lower.contains(&name.to_lowercase()) {
                entities.push(ExtractedEntity {
                    name: name.to_string(),
                    entity_type: entity_type.parse().unwrap_or(EntityType::Tool),
                    aliases: vec![],
                    description: None,
                    confidence: 0.7,
                    position: content_lower.find(&name.to_lowercase()),
                    matched_text: Some(name.to_string()),
                });
            }
        }

        entities
    }
}

#[cfg(test)]
mod tests {
    use crate::extraction::entities::*;
    use cas_types::EntryType;

    #[test]
    fn test_entity_extractor_build_prompt() {
        let extractor = EntityExtractor::default_extractor();
        let entry = Entry {
            id: "test-001".to_string(),
            entry_type: EntryType::Learning,
            content: "The Store trait in CAS handles all database operations.".to_string(),
            tags: vec!["rust".to_string()],
            ..Default::default()
        };

        let prompt = extractor.build_prompt(&entry);
        assert!(prompt.contains("test-001"));
        assert!(prompt.contains("Store trait"));
        assert!(prompt.contains("person"));
        assert!(prompt.contains("project"));
    }

    #[test]
    fn test_parse_entity_response() {
        let extractor = EntityExtractor::default_extractor();
        let response = r#"{
            "entities": [
                {"name": "CAS", "type": "project", "description": "Coding Agent System", "confidence": 0.9},
                {"name": "Rust", "type": "tool", "confidence": 0.8}
            ],
            "relationships": [
                {"source": "CAS", "target": "Rust", "type": "uses", "confidence": 0.85}
            ]
        }"#;

        let result = extractor.parse_response(response, "test-001").unwrap();
        assert_eq!(result.entities.len(), 2);
        assert_eq!(result.entities[0].name, "CAS");
        assert_eq!(result.entities[0].entity_type, EntityType::Project);
        assert_eq!(result.relationships.len(), 1);
        assert_eq!(result.relationships[0].relation_type, RelationType::Uses);
    }

    #[test]
    fn test_pattern_extractor_files() {
        let extractor = PatternEntityExtractor::default();
        let entry = Entry {
            id: "test".to_string(),
            content: "Modified src/main.rs and tests/cli_test.rs".to_string(),
            ..Default::default()
        };

        let result = extractor.extract(&entry);
        let file_entities: Vec<_> = result
            .entities
            .iter()
            .filter(|e| e.entity_type == EntityType::File)
            .collect();

        assert!(file_entities.len() >= 2);
    }

    #[test]
    fn test_pattern_extractor_tools() {
        let extractor = PatternEntityExtractor::default();
        let entry = Entry {
            id: "test".to_string(),
            content: "Using Rust and SQLite for the database layer".to_string(),
            ..Default::default()
        };

        let result = extractor.extract(&entry);
        let tool_names: Vec<_> = result.entities.iter().map(|e| e.name.as_str()).collect();

        assert!(tool_names.contains(&"Rust"));
        assert!(tool_names.contains(&"SQLite"));
    }

    #[test]
    fn test_extracted_entity_conversion() {
        let extracted = ExtractedEntity {
            name: "CAS".to_string(),
            entity_type: EntityType::Project,
            aliases: vec!["Coding Agent System".to_string()],
            description: Some("Memory system".to_string()),
            confidence: 0.9,
            position: Some(10),
            matched_text: Some("CAS".to_string()),
        };

        let entity = extracted.to_entity("ent-001".to_string());
        assert_eq!(entity.name, "CAS");
        assert_eq!(entity.entity_type, EntityType::Project);
        assert_eq!(entity.aliases, vec!["Coding Agent System"]);

        let mention = extracted.to_mention("ent-001".to_string(), "entry-001".to_string());
        assert_eq!(mention.entity_id, "ent-001");
        assert_eq!(mention.position, Some(10));
    }

    #[test]
    fn test_extracted_relationship_conversion() {
        let extracted = ExtractedRelationship {
            source_name: "CAS".to_string(),
            target_name: "Rust".to_string(),
            relation_type: RelationType::Uses,
            description: Some("CAS is built with Rust".to_string()),
            confidence: 0.9,
        };

        let rel = extracted.to_relationship(
            "rel-001".to_string(),
            "ent-001".to_string(),
            "ent-002".to_string(),
            "entry-001",
        );

        assert_eq!(rel.source_id, "ent-001");
        assert_eq!(rel.target_id, "ent-002");
        assert_eq!(rel.relation_type, RelationType::Uses);
        assert!(rel.source_entries.contains(&"entry-001".to_string()));
    }

    #[test]
    fn test_entity_extractor_sync_defers() {
        let extractor = EntityExtractor::default_extractor();
        let entry = Entry {
            id: "test".to_string(),
            content: "Test content".to_string(),
            ..Default::default()
        };

        let result = extractor.extract(&entry).unwrap();
        assert!(result.deferred);
    }

    #[test]
    fn test_confidence_filtering() {
        let extractor = EntityExtractor::new(EntityExtractorConfig {
            min_confidence: 0.7,
            ..Default::default()
        });

        let response = r#"{
            "entities": [
                {"name": "HighConf", "type": "project", "confidence": 0.9},
                {"name": "LowConf", "type": "project", "confidence": 0.5}
            ],
            "relationships": []
        }"#;

        let result = extractor.parse_response(response, "test").unwrap();
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].name, "HighConf");
    }
}
