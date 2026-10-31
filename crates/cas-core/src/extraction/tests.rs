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
    // Check that prompt contains quality criteria
    assert!(prompt.contains("Quality Standards"));
    assert!(prompt.contains("quality_score"));
    assert!(prompt.contains("Project-specific"));
    // Check observation is included
    assert!(prompt.contains("[Write]"));
}

#[test]
fn test_parse_response_with_quality() {
    let extractor = AIExtractor::new(AIExtractorConfig::default());
    // Items with quality_score >= 6 should be kept
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
    assert_eq!(result.learnings[0].confidence, 0.9);
    assert_eq!(result.rules.len(), 1);
    assert!(!result.deferred);
}

#[test]
fn test_parse_response_with_markdown() {
    let extractor = AIExtractor::new(AIExtractorConfig::default());
    let response = r#"Here's the extraction:

```json
{
    "learnings": [{"content": "VouchWall uses mise for task running", "quality_score": 7, "confidence": 0.7}]
}
```
"#;

    let result = extractor.parse_response(response, "test-id").unwrap();
    assert_eq!(result.learnings.len(), 1);
}

#[test]
fn test_quality_filtering() {
    let extractor = AIExtractor::new(AIExtractorConfig::default());
    // Items with quality_score < 6 should be filtered out
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
    // Only the high-quality learning should be kept
    assert_eq!(result.learnings.len(), 1);
    assert_eq!(result.learnings[0].quality_score, Some(8));
    assert!(result.learnings[0].content.contains("VouchWall"));
    // Low quality preference should be filtered
    assert_eq!(result.preferences.len(), 0);
}

#[test]
fn test_short_content_filtered() {
    let extractor = AIExtractor::new(AIExtractorConfig::default());
    // Content < 10 chars should be filtered
    let response = r#"{
            "learnings": [
                {"content": "Short", "quality_score": 9, "confidence": 0.9},
                {"content": "This is a longer learning about the project structure", "quality_score": 7, "confidence": 0.8}
            ]
        }"#;

    let result = extractor.parse_response(response, "test-id").unwrap();
    assert_eq!(result.learnings.len(), 1);
    assert!(result.learnings[0].content.contains("longer learning"));
}

#[test]
fn test_should_be_rule_parsing() {
    let extractor = AIExtractor::new(AIExtractorConfig::default());
    let response = r#"{
            "learnings": [
                {
                    "content": "Always wrap browser APIs with ClientOnly for SSR",
                    "quality_score": 9,
                    "confidence": 0.95,
                    "tags": ["react", "ssr"],
                    "should_be_rule": true,
                    "paths": "**/*.tsx"
                }
            ]
        }"#;

    let result = extractor.parse_response(response, "test-id").unwrap();
    assert_eq!(result.learnings.len(), 1);
    assert!(result.learnings[0].should_be_rule);
    assert_eq!(result.learnings[0].rule_paths, Some("**/*.tsx".to_string()));
}

#[test]
fn test_parse_response_empty_arrays() {
    let extractor = AIExtractor::new(AIExtractorConfig::default());
    let response = r#"{
            "learnings": [],
            "preferences": [],
            "rules": [],
            "skipped_count": 10,
            "skip_reason": "No project-specific insights"
        }"#;

    let result = extractor.parse_response(response, "test-id").unwrap();
    assert!(result.learnings.is_empty());
    assert!(result.preferences.is_empty());
    assert!(result.rules.is_empty());
}

#[test]
fn test_parse_response_missing_optional_fields() {
    let extractor = AIExtractor::new(AIExtractorConfig::default());
    // Response without confidence, quality_score, tags - should use defaults
    let response = r#"{
            "learnings": [
                {"content": "Project uses custom middleware pattern for auth"}
            ]
        }"#;

    let result = extractor.parse_response(response, "test-id").unwrap();
    assert_eq!(result.learnings.len(), 1);
    assert_eq!(result.learnings[0].confidence, 0.5); // default
    assert!(result.learnings[0].quality_score.is_none());
    assert!(result.learnings[0].tags.is_empty());
    assert!(!result.learnings[0].should_be_rule);
}

#[test]
fn test_parse_response_with_extra_json_text() {
    let extractor = AIExtractor::new(AIExtractorConfig::default());
    // Response with text before and after JSON
    let response = r#"Here's my analysis of the observations:

{
    "learnings": [{"content": "Uses Phoenix LiveView for real-time updates", "quality_score": 8}],
    "preferences": [],
    "rules": []
}

That's all the valuable insights I found."#;

    let result = extractor.parse_response(response, "test-id").unwrap();
    assert_eq!(result.learnings.len(), 1);
}

#[test]
fn test_parse_response_only_preferences() {
    let extractor = AIExtractor::new(AIExtractorConfig::default());
    let response = r#"{
            "learnings": [],
            "preferences": [
                {"content": "User prefers Tailwind CSS with custom theme colors", "quality_score": 7, "confidence": 0.8, "tags": ["styling"]}
            ],
            "rules": []
        }"#;

    let result = extractor.parse_response(response, "test-id").unwrap();
    assert!(result.learnings.is_empty());
    assert_eq!(result.preferences.len(), 1);
    assert!(result.preferences[0].content.contains("Tailwind"));
}

#[test]
fn test_parse_response_only_rules() {
    let extractor = AIExtractor::new(AIExtractorConfig::default());
    let response = r#"{
            "learnings": [],
            "preferences": [],
            "rules": [
                {"content": "Always validate API inputs with Ecto changesets", "quality_score": 9, "confidence": 0.95, "tags": ["elixir", "validation"], "paths": "lib/**/*.ex"}
            ]
        }"#;

    let result = extractor.parse_response(response, "test-id").unwrap();
    assert!(result.learnings.is_empty());
    assert!(result.preferences.is_empty());
    assert_eq!(result.rules.len(), 1);
    assert!(result.rules[0].content.contains("Ecto"));
}

#[test]
fn test_ai_extractor_sync_defers() {
    let extractor = AIExtractor::new(AIExtractorConfig::default());
    let observation = Entry {
        id: "test".to_string(),
        entry_type: EntryType::Observation,
        content: "Test observation".to_string(),
        ..Default::default()
    };

    // Sync extraction should defer (async needed for AI)
    let result = extractor.extract(&observation).unwrap();
    assert!(result.deferred);
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
    // Should have default config
    let observation = Entry {
        id: "test".to_string(),
        entry_type: EntryType::Observation,
        content: "Test observation for extraction".to_string(),
        ..Default::default()
    };

    // Should work without panicking
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

#[test]
fn test_extractor_trait_batched() {
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

    let result = extractor.extract_batched(&observations).unwrap();
    // Combined result should be empty since DeferredExtractor returns empty
    assert!(result.learnings.is_empty());
}

#[test]
fn test_ai_extractor_config_custom() {
    let config = AIExtractorConfig {
        model: "claude-sonnet-4-20250514".to_string(),
        max_thinking_tokens: 5000,
        extract_learnings: true,
        extract_preferences: false,
        suggest_rules: false,
    };

    assert_eq!(config.model, "claude-sonnet-4-20250514");
    assert_eq!(config.max_thinking_tokens, 5000);
    assert!(config.extract_learnings);
    assert!(!config.extract_preferences);
    assert!(!config.suggest_rules);
}

#[test]
fn test_build_batch_prompt_multiple_observations() {
    let extractor = AIExtractor::new(AIExtractorConfig::default());
    let observations = vec![
        Entry {
            id: "obs1".to_string(),
            entry_type: EntryType::Observation,
            content: "Write: created new API handler".to_string(),
            source_tool: Some("Write".to_string()),
            tags: vec!["api".to_string()],
            ..Default::default()
        },
        Entry {
            id: "obs2".to_string(),
            entry_type: EntryType::Observation,
            content: "Bash: cargo test passed".to_string(),
            source_tool: Some("Bash".to_string()),
            ..Default::default()
        },
    ];

    let prompt = extractor.build_batch_prompt(&observations);
    // Both observations should be in the prompt
    assert!(prompt.contains("[Write]"));
    assert!(prompt.contains("[Bash]"));
    assert!(prompt.contains("(tags: api)"));
    assert!(prompt.contains("API handler"));
    assert!(prompt.contains("cargo test"));
}
