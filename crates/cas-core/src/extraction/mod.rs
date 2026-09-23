//! Observation and entity extraction pipeline
//!
//! Extracts learnings, preferences, rule suggestions, and knowledge graph entities
//! from raw observations using AI-powered analysis.
//!
//! # Overview
//!
//! This module defines extraction pipelines for:
//! - Converting raw observations into structured entries (learnings, preferences, rules)
//! - Extracting entities and relationships for the knowledge graph
//!
//! # Implementations
//!
//! - `DeferredExtractor` - Marks observations for later processing (default)
//! - `AIExtractor` - Builds and parses AI-powered extraction prompts
//! - `EntityExtractor` - Extracts entities and relationships for knowledge graph
//! - `PatternEntityExtractor` - Simple pattern-based entity extraction (no AI)
//!
//! # Usage
//!
//! ```rust,ignore
//! use cas_core::extraction::{Extractor, AIExtractor, AIExtractorConfig};
//!
//! // Create AI extractor
//! let config = AIExtractorConfig::default();
//! let extractor = AIExtractor::new(config);
//!
//! // Extract from observation (async)
//! let result = extractor.extract_async(&observation).await?;
//! ```

// Entity extraction module for knowledge graph feature
pub mod entities;

// Entity summary generation (Hindsight observation network)
pub mod summary;

// Task knowledge extraction - extracts learnings from task close_reason and notes
pub mod tasks;

// Re-export commonly used types
pub use entities::{
    EntityExtractionResult, EntityExtractor, EntityExtractorConfig, ExtractedEntity,
    ExtractedRelationship, PatternEntityExtractor,
};
pub use summary::{ExtractedFact, SummaryConfig, SummaryGenerator, update_entity_summaries};
pub use tasks::{
    ExtractedDecision, ExtractedLearning, SuggestedRule, TaskExtractionResult,
    build_task_extraction_prompt, extract_from_task, learnings_to_entries,
};

use crate::error::CoreError;
use cas_types::Entry;

/// Result of extraction from an observation
#[derive(Debug, Clone, Default)]
pub struct ExtractionResult {
    /// Extracted learnings (facts and patterns)
    pub learnings: Vec<ExtractedItem>,

    /// Extracted preferences (user preferences and style)
    pub preferences: Vec<ExtractedItem>,

    /// Suggested rules (patterns to promote)
    pub rules: Vec<ExtractedItem>,

    /// Whether extraction was deferred for later processing
    pub deferred: bool,
}

/// A single extracted item
#[derive(Debug, Clone)]
pub struct ExtractedItem {
    /// The extracted content
    pub content: String,

    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,

    /// Quality score (1-10) - only items with score >= 6 should be kept
    pub quality_score: Option<u8>,

    /// Tags to apply
    pub tags: Vec<String>,

    /// Source observation ID
    pub source_id: String,

    /// Whether this should become a rule
    pub should_be_rule: bool,

    /// Glob pattern for rule paths (if should_be_rule is true)
    pub rule_paths: Option<String>,
}

/// Trait for extraction implementations
pub trait Extractor: Send + Sync {
    /// Extract structured information from an observation (sync, may defer)
    fn extract(&self, observation: &Entry) -> Result<ExtractionResult, CoreError>;

    /// Process multiple observations in batch (default: process individually)
    fn extract_batch(&self, observations: &[Entry]) -> Result<Vec<ExtractionResult>, CoreError> {
        observations.iter().map(|o| self.extract(o)).collect()
    }

    /// Extract from multiple observations with shared context (batched for quality)
    /// This is the preferred method - provides context across observations
    fn extract_batched(&self, observations: &[Entry]) -> Result<ExtractionResult, CoreError> {
        // Default: just combine individual extractions
        let mut combined = ExtractionResult::default();
        for obs in observations {
            let result = self.extract(obs)?;
            combined.learnings.extend(result.learnings);
            combined.preferences.extend(result.preferences);
            combined.rules.extend(result.rules);
        }
        Ok(combined)
    }
}

/// Placeholder extractor that marks entries for later processing
///
/// This is the default extractor when no AI extraction is available.
/// It simply marks observations as pending and returns empty results.
pub struct DeferredExtractor;

impl Extractor for DeferredExtractor {
    fn extract(&self, _observation: &Entry) -> Result<ExtractionResult, CoreError> {
        // Mark for later - actual extraction will happen via AIExtractor
        Ok(ExtractionResult {
            learnings: vec![],
            preferences: vec![],
            rules: vec![],
            deferred: true,
        })
    }
}

/// Configuration for AI-powered extraction
#[derive(Debug, Clone)]
pub struct AIExtractorConfig {
    /// Model to use for extraction
    pub model: String,

    /// Maximum thinking tokens for complex analysis
    pub max_thinking_tokens: u32,

    /// Whether to extract learnings
    pub extract_learnings: bool,

    /// Whether to extract preferences
    pub extract_preferences: bool,

    /// Whether to suggest rules
    pub suggest_rules: bool,
}

impl Default for AIExtractorConfig {
    fn default() -> Self {
        Self {
            model: "gpt-6-luna".to_string(),
            max_thinking_tokens: 2000,
            extract_learnings: true,
            extract_preferences: true,
            suggest_rules: true,
        }
    }
}

/// AI-powered extractor prompt and response parser
///
/// This struct provides configuration and prompt building logic. The
/// application layer routes model calls through the selected light lane.
///
/// # Example
///
/// ```rust,ignore
/// use cas_core::extraction::{AIExtractor, AIExtractorConfig};
///
/// let config = AIExtractorConfig::default();
/// let extractor = AIExtractor::new(config);
///
/// // Build prompt for AI call
/// let prompt = extractor.build_prompt(&observation);
/// // Application layer makes AI call and parses response
/// let result = extractor.parse_response(&response_text, &observation.id)?;
/// ```
pub struct AIExtractor {
    /// Configuration for extraction
    config: AIExtractorConfig,
}

impl AIExtractor {
    /// Create a new AI extractor with the given configuration
    pub fn new(config: AIExtractorConfig) -> Self {
        Self { config }
    }

    /// Get the model name for this extractor
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Get the max thinking tokens setting
    pub fn max_thinking_tokens(&self) -> u32 {
        self.config.max_thinking_tokens
    }

    /// Build the extraction prompt for Claude (single observation)
    pub fn build_prompt(&self, observation: &Entry) -> String {
        self.build_batch_prompt(std::slice::from_ref(observation))
    }

    /// Build the extraction prompt for batched observations
    /// Batching provides better context for quality assessment
    pub fn build_batch_prompt(&self, observations: &[Entry]) -> String {
        let mut prompt = String::new();

        prompt.push_str(r#"# Memory Extraction Task

You are extracting valuable learnings from AI coding session observations.

## Quality Standards - CRITICAL

ONLY extract learnings that meet ALL criteria:
1. **Project-specific**: Contains file paths, module names, config keys, or patterns unique to this codebase
2. **Actionable**: Could help a future agent working on this project
3. **Non-obvious**: Would NOT be known from general programming knowledge
4. **Contextual**: Explains WHY something works, not just WHAT was done

## What to ALWAYS SKIP
- Generic programming knowledge ("2>&1 redirects stderr", "mix compile compiles")
- Obvious tool usage ("cargo build builds the project", "npm install installs deps")
- Single-word observations or trivial edits
- File writes without context about WHY
- Standard library/framework behavior that's in documentation

## Quality Scoring (1-10)
+3: Contains project-specific file paths or module names
+2: Contains numbered steps, patterns, or structured insights
+2: Explains WHY (rationale), not just WHAT
+1: Over 50 characters with meaningful technical content
+1: Mentions specific config values, API endpoints, or commands
-3: Generic programming knowledge anyone would know
-2: Under 30 characters or trivially obvious
-2: Just describes a file edit without insight
-1: Could apply to any project (not specific)

Only include items with quality_score >= 6.

## Observations from Session
"#);

        for obs in observations.iter().take(15) {
            prompt.push_str("\n---\n");
            prompt.push_str(&format!(
                "**[{}]** ",
                obs.source_tool.as_deref().unwrap_or("?")
            ));
            if !obs.tags.is_empty() {
                prompt.push_str(&format!("(tags: {}) ", obs.tags.join(", ")));
            }
            prompt.push_str(&format!("\n{}\n", obs.content));
        }

        prompt.push_str(r#"

## Response Format

Extract 0-5 high-quality learnings. Return empty arrays if nothing valuable.

```json
{
  "learnings": [
    {
      "content": "VouchWall uses mise for task automation - run 'mise run db' to start TimescaleDB container with proper config",
      "quality_score": 8,
      "confidence": 0.9,
      "tags": ["tooling", "database"],
      "should_be_rule": false
    }
  ],
  "preferences": [
    {
      "content": "Project uses Tailwind with custom theme colors defined in tailwind.config.js",
      "quality_score": 7,
      "confidence": 0.85,
      "tags": ["styling"]
    }
  ],
  "rules": [
    {
      "content": "Always wrap browser-only components with ClientOnly for SSR compatibility",
      "quality_score": 9,
      "confidence": 0.95,
      "tags": ["react", "ssr"],
      "paths": "**/*.tsx"
    }
  ],
  "skipped_count": 12,
  "skip_reason": "Generic tool usage and file edits without project-specific insights"
}
```

CRITICAL: If observations are routine with no project-specific insights, return:
```json
{"learnings": [], "preferences": [], "rules": [], "skipped_count": N, "skip_reason": "No project-specific insights"}
```

Respond with JSON only, no markdown code fences.
"#);

        prompt
    }

    /// Parse Claude's JSON response into ExtractionResult
    pub fn parse_response(
        &self,
        response: &str,
        source_id: &str,
    ) -> Result<ExtractionResult, CoreError> {
        // Try to find JSON in the response
        let json_str = response
            .find('{')
            .and_then(|start| response.rfind('}').map(|end| &response[start..=end]))
            .unwrap_or(response);

        let value: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| CoreError::Parse(format!("Failed to parse extraction response: {e}")))?;

        let mut result = ExtractionResult::default();

        // Parse learnings
        if let Some(learnings) = value.get("learnings").and_then(|v| v.as_array()) {
            for item in learnings {
                if let Some(extracted) = self.parse_item(item, source_id) {
                    result.learnings.push(extracted);
                }
            }
        }

        // Parse preferences
        if let Some(preferences) = value.get("preferences").and_then(|v| v.as_array()) {
            for item in preferences {
                if let Some(extracted) = self.parse_item(item, source_id) {
                    result.preferences.push(extracted);
                }
            }
        }

        // Parse rules
        if let Some(rules) = value.get("rules").and_then(|v| v.as_array()) {
            for item in rules {
                if let Some(extracted) = self.parse_item(item, source_id) {
                    result.rules.push(extracted);
                }
            }
        }

        Ok(result)
    }

    /// Parse a single extracted item from JSON
    fn parse_item(&self, value: &serde_json::Value, source_id: &str) -> Option<ExtractedItem> {
        let content = value.get("content")?.as_str()?.to_string();

        // Skip empty or trivially short content
        if content.len() < 10 {
            return None;
        }

        let confidence = value
            .get("confidence")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32)
            .unwrap_or(0.5);

        let quality_score = value
            .get("quality_score")
            .and_then(|v| v.as_u64())
            .map(|v| v.min(10) as u8);

        // Filter by quality score if present - only keep items >= 6
        if let Some(score) = quality_score {
            if score < 6 {
                return None;
            }
        }

        let tags = value
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let should_be_rule = value
            .get("should_be_rule")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let rule_paths = value
            .get("paths")
            .and_then(|v| v.as_str())
            .map(String::from);

        Some(ExtractedItem {
            content,
            confidence,
            quality_score,
            tags,
            source_id: source_id.to_string(),
            should_be_rule,
            rule_paths,
        })
    }
}

impl Extractor for AIExtractor {
    fn extract(&self, _observation: &Entry) -> Result<ExtractionResult, CoreError> {
        // Sync extraction defers to async
        // Use extract_async() in application layer for actual AI extraction
        Ok(ExtractionResult {
            learnings: vec![],
            preferences: vec![],
            rules: vec![],
            deferred: true,
        })
    }
}

/// Get the default extractor
pub fn default_extractor() -> Box<dyn Extractor> {
    Box::new(DeferredExtractor)
}

/// Get an AI extractor with default config
pub fn ai_extractor() -> AIExtractor {
    AIExtractor::new(AIExtractorConfig::default())
}

#[cfg(test)]
mod tests;
