//! Observation and entity extraction pipeline
//!
//! This module re-exports extraction types from `cas-core` and extends them
//! with async AI-powered extraction methods routed through the light lane.
//!
//! # Architecture
//!
//! - Core types and sync methods: `cas_core::extraction`
//! - Async AI extraction: This module (extensions)
//!
//! # Usage
//!
//! ```rust,ignore
//! use crate::extraction::{AIExtractor, AIExtractorConfig};
//!
//! // Create AI extractor
//! let config = AIExtractorConfig::default();
//! let extractor = AIExtractor::new(config);
//!
//! // Build prompt (from cas-core)
//! let prompt = extractor.build_prompt(&observation);
//!
//! // Run async extraction (from this module)
//! let result = extractor.extract_async(&observation).await?;
//! ```

// Re-export everything from cas_core::extraction
pub use cas_core::extraction::*;

// Re-export submodules from cas_core for backward compatibility
pub mod entities {
    pub use cas_core::extraction::entities::*;

    // Extension trait for async entity extraction
    use crate::error::CasError;
    use cas_types::Entry;

    /// Extension trait for async entity extraction
    #[allow(async_fn_in_trait)]
    pub trait EntityExtractorAsync {
        /// Async extraction using claude_rs SDK
        async fn extract_async(
            &self,
            entry: &Entry,
        ) -> Result<cas_core::extraction::EntityExtractionResult, CasError>;
    }

    impl EntityExtractorAsync for cas_core::extraction::EntityExtractor {
        async fn extract_async(
            &self,
            entry: &Entry,
        ) -> Result<cas_core::extraction::EntityExtractionResult, CasError> {
            use crate::tracing::claude_wrapper::traced_prompt;
            use claude_rs::QueryOptions;

            let prompt_text = self.build_prompt(entry);

            let mut options = QueryOptions::new().model(self.model()).max_turns(1);
            if self.model() == "claude-opus-5-5" {
                options = options.extra_arg("--effort").extra_arg("low");
            }

            let result = traced_prompt(&prompt_text, options, "entity_extraction")
                .await
                .map_err(|e| CasError::Other(format!("Entity extraction failed: {e}")))?;

            self.parse_response(result.text(), &entry.id)
                .map_err(|e| CasError::Other(e.to_string()))
        }
    }
}

pub mod summary {
    pub use cas_core::extraction::summary::*;
}

pub mod tasks {
    pub use cas_core::extraction::tasks::*;
}

// Extension trait for async AI extraction
use crate::error::MemError;
use cas_types::Entry;

async fn extraction_response(
    prompt: String,
    model: &str,
    max_thinking_tokens: u32,
    label: &'static str,
) -> Result<String, MemError> {
    if model.starts_with("gpt-") {
        return tokio::task::spawn_blocking(move || {
            let cwd = std::env::current_dir().map_err(|e| MemError::Other(e.to_string()))?;
            crate::light_lane::run(&prompt, &cwd, std::time::Duration::from_secs(180))
                .map_err(|e| MemError::Other(format!("light lane extraction failed: {e}")))
        })
        .await
        .map_err(|e| MemError::Other(format!("light lane extraction task failed: {e}")))?;
    }
    use crate::tracing::claude_wrapper::traced_prompt;
    use claude_rs::QueryOptions;
    let mut options = QueryOptions::new().model(model).max_turns(1);
    if model == "claude-opus-5-5" {
        options = options.extra_arg("--effort").extra_arg("low");
    }
    if !model.contains("haiku") {
        options = options.max_thinking_tokens(max_thinking_tokens);
    }
    let result = traced_prompt(&prompt, options, label)
        .await
        .map_err(|e| MemError::Other(format!("Claude extraction failed: {e}")))?;
    Ok(result.text().to_string())
}

/// Extension trait for async AI extraction
#[allow(async_fn_in_trait)]
pub trait AIExtractorAsync {
    /// Async extraction of one observation
    async fn extract_async(&self, observation: &Entry) -> Result<ExtractionResult, MemError>;

    /// Async batched extraction (multiple observations with shared context)
    async fn extract_batch_async(
        &self,
        observations: &[Entry],
    ) -> Result<ExtractionResult, MemError>;
}

impl AIExtractorAsync for AIExtractor {
    async fn extract_async(&self, observation: &Entry) -> Result<ExtractionResult, MemError> {
        let start_time = std::time::Instant::now();
        let prompt_text = self.build_prompt(observation);
        let response_text = extraction_response(
            prompt_text,
            self.model(),
            self.max_thinking_tokens(),
            "extraction",
        )
        .await?;
        let extraction_result = self
            .parse_response(&response_text, &observation.id)
            .map_err(|e| MemError::Other(e.to_string()))?;

        // Trace the extraction
        if let Some(tracer) = crate::tracing::DevTracer::get() {
            let trace = crate::tracing::ExtractionTrace {
                observation_id: observation.id.clone(),
                content_length: observation.content.len(),
                method: format!("ai:{}", self.model()),
                memories_extracted: extraction_result.learnings.len()
                    + extraction_result.preferences.len(),
                quality_score: None,
                tags_extracted: extraction_result
                    .learnings
                    .iter()
                    .flat_map(|l| l.tags.clone())
                    .collect(),
            };
            let _ = tracer.record_extraction(
                &trace,
                start_time.elapsed().as_millis() as u64,
                true,
                None,
            );
        }

        Ok(extraction_result)
    }

    async fn extract_batch_async(
        &self,
        observations: &[Entry],
    ) -> Result<ExtractionResult, MemError> {
        if observations.is_empty() {
            return Ok(ExtractionResult::default());
        }

        let start_time = std::time::Instant::now();
        let prompt_text = self.build_batch_prompt(observations);

        let response_text = extraction_response(
            prompt_text,
            self.model(),
            self.max_thinking_tokens(),
            "extraction_batch",
        )
        .await?;

        // Use the first observation ID as source, but include all
        let source_ids: Vec<_> = observations.iter().map(|o| o.id.as_str()).collect();
        let combined_source = source_ids.join(",");
        let extraction_result = self
            .parse_response(&response_text, &combined_source)
            .map_err(|e| MemError::Other(e.to_string()))?;

        // Trace the extraction with quality info
        if let Some(tracer) = crate::tracing::DevTracer::get() {
            let avg_quality: Option<f32> = {
                let scores: Vec<u8> = extraction_result
                    .learnings
                    .iter()
                    .chain(extraction_result.preferences.iter())
                    .filter_map(|l| l.quality_score)
                    .collect();
                if scores.is_empty() {
                    None
                } else {
                    Some(scores.iter().map(|&s| s as f32).sum::<f32>() / scores.len() as f32)
                }
            };

            let trace = crate::tracing::ExtractionTrace {
                observation_id: format!("batch[{}]", observations.len()),
                content_length: observations.iter().map(|o| o.content.len()).sum(),
                method: format!("ai-batch:{}", self.model()),
                memories_extracted: extraction_result.learnings.len()
                    + extraction_result.preferences.len(),
                quality_score: avg_quality,
                tags_extracted: extraction_result
                    .learnings
                    .iter()
                    .flat_map(|l| l.tags.clone())
                    .collect(),
            };
            let _ = tracer.record_extraction(
                &trace,
                start_time.elapsed().as_millis() as u64,
                true,
                None,
            );
        }

        Ok(extraction_result)
    }
}

#[cfg(test)]
mod tests {
    use crate::extraction::*;
    use cas_types::EntryType;

    #[test]
    fn test_deferred_extractor() {
        let extractor = DeferredExtractor;
        let observation = Entry {
            id: "test".to_string(),
            entry_type: EntryType::Observation,
            content: "Test observation".to_string(),
            ..Default::default()
        };

        let result = extractor.extract(&observation).unwrap();
        assert!(result.deferred);
        assert!(result.learnings.is_empty());
    }

    #[test]
    fn test_ai_extractor_config_default() {
        let config = AIExtractorConfig::default();
        assert!(config.extract_learnings);
        assert!(config.extract_preferences);
        assert!(config.suggest_rules);
        assert_eq!(config.max_thinking_tokens, 2000);
    }

    /// Manual provider receipt for the daemon's default extraction route.
    #[tokio::test]
    #[ignore = "requires a live Codex login and incurs model usage"]
    async fn live_light_lane_extraction() {
        let extractor = AIExtractor::new(AIExtractorConfig::default());
        let observation = Entry {
            id: "light-lane-live-observation".to_string(),
            entry_type: EntryType::Observation,
            content: "In cas-cli/src/store/syncing_task.rs, concurrent task writers failed with SQLITE_BUSY because a new SQLite connection omitted busy_timeout=5000. Setting the timeout on every newly opened connection allowed the competing writer to finish before the next write; retrying only the transaction did not fix it.".to_string(),
            ..Default::default()
        };
        let result = extractor.extract_async(&observation).await.unwrap();
        assert!(
            !result.learnings.is_empty(),
            "live extraction returned no learning"
        );
    }

    #[test]
    fn test_extraction_result_default() {
        let result = ExtractionResult::default();
        assert!(result.learnings.is_empty());
        assert!(result.preferences.is_empty());
        assert!(result.rules.is_empty());
        assert!(!result.deferred);
    }

    #[test]
    fn test_build_prompt() {
        let extractor = AIExtractor::new(AIExtractorConfig::default());
        let observation = Entry {
            id: "2024-01-15-001".to_string(),
            entry_type: EntryType::Observation,
            content: "Write: src/main.rs - added new handler for API endpoints".to_string(),
            source_tool: Some("Write".to_string()),
            ..Default::default()
        };

        let prompt = extractor.build_prompt(&observation);
        assert!(prompt.contains("Quality Standards"));
        assert!(prompt.contains("quality_score"));
        assert!(prompt.contains("Project-specific"));
        assert!(prompt.contains("[Write]"));
    }

    #[test]
    fn test_parse_response_with_quality() {
        let extractor = AIExtractor::new(AIExtractorConfig::default());
        let response = r#"{
            "learnings": [
                {"content": "Use table-driven tests for API handlers in src/handlers/", "quality_score": 8, "confidence": 0.9, "tags": ["testing"]}
            ],
            "preferences": [],
            "rules": [
                {"content": "Always run cargo fmt before commit", "quality_score": 7, "confidence": 0.8, "tags": ["rust"]}
            ]
        }"#;

        let result = extractor.parse_response(response, "test-id").unwrap();
        assert_eq!(result.learnings.len(), 1);
        assert_eq!(
            result.learnings[0].content,
            "Use table-driven tests for API handlers in src/handlers/"
        );
        assert_eq!(result.learnings[0].quality_score, Some(8));
        assert_eq!(result.rules.len(), 1);
    }

    #[test]
    fn test_quality_filtering() {
        let extractor = AIExtractor::new(AIExtractorConfig::default());
        let response = r#"{
            "learnings": [
                {"content": "mix compile compiles the project", "quality_score": 3, "confidence": 0.9, "tags": ["elixir"]},
                {"content": "VouchWall API uses Phoenix with custom auth middleware", "quality_score": 8, "confidence": 0.85, "tags": ["phoenix", "auth"]}
            ],
            "preferences": [
                {"content": "Uses tabs", "quality_score": 2, "confidence": 0.5}
            ],
            "rules": []
        }"#;

        let result = extractor.parse_response(response, "test-id").unwrap();
        assert_eq!(result.learnings.len(), 1);
        assert_eq!(result.learnings[0].quality_score, Some(8));
        assert!(result.learnings[0].content.contains("VouchWall"));
        assert_eq!(result.preferences.len(), 0);
    }

    #[test]
    fn test_default_extractor() {
        let extractor = default_extractor();
        let observation = Entry {
            id: "test".to_string(),
            entry_type: EntryType::Observation,
            content: "Test".to_string(),
            ..Default::default()
        };

        let result = extractor.extract(&observation).unwrap();
        assert!(result.deferred);
    }

    #[test]
    fn test_ai_extractor_constructor() {
        let extractor = ai_extractor();
        let observation = Entry {
            id: "test".to_string(),
            entry_type: EntryType::Observation,
            content: "Test observation for extraction".to_string(),
            ..Default::default()
        };

        let result = extractor.extract(&observation).unwrap();
        assert!(result.deferred);
    }

    #[test]
    fn test_extractor_trait_batch() {
        let extractor = DeferredExtractor;
        let observations = vec![
            Entry {
                id: "obs1".to_string(),
                entry_type: EntryType::Observation,
                content: "First observation".to_string(),
                ..Default::default()
            },
            Entry {
                id: "obs2".to_string(),
                entry_type: EntryType::Observation,
                content: "Second observation".to_string(),
                ..Default::default()
            },
        ];

        let results = extractor.extract_batch(&observations).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].deferred);
        assert!(results[1].deferred);
    }
}
