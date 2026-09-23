//! AI-powered memory consolidation
//!
//! Uses Claude to intelligently merge similar memories, identify duplicates,
//! and suggest consolidated content.

// #![allow(dead_code)] // Check unused // API for AI consolidation feature

use crate::error::CasError;
use crate::types::Entry;
use serde::{Deserialize, Serialize};

/// Configuration for consolidation
#[derive(Debug, Clone)]
pub struct ConsolidationConfig {
    /// Similarity threshold for considering memories as related (0.0-1.0)
    pub similarity_threshold: f64,
    /// Maximum memories to process in a batch
    pub batch_size: usize,
    /// Model to use for consolidation (sonnet recommended)
    pub model: String,
    /// Whether to auto-apply suggestions or just report
    pub auto_apply: bool,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.7,
            batch_size: 10,
            model: "sonnet".to_string(),
            auto_apply: false,
        }
    }
}

/// A suggestion from the AI for how to consolidate memories
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationSuggestion {
    /// IDs of entries to merge
    pub source_ids: Vec<String>,
    /// Suggested merged content
    pub merged_content: String,
    /// Suggested title for merged entry
    pub merged_title: Option<String>,
    /// Suggested tags for merged entry
    pub merged_tags: Vec<String>,
    /// Reasoning for the merge
    pub reasoning: String,
    /// Confidence score (0.0-1.0)
    pub confidence: f64,
    /// Action type: merge, dedupe, or update
    pub action: ConsolidationAction,
}

/// Action type for consolidation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsolidationAction {
    /// Merge multiple entries into one
    Merge,
    /// Remove duplicates, keep one
    Dedupe,
    /// Update an existing entry with new information
    Update,
    /// No action needed
    Skip,
}

/// Result of a consolidation run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationResult {
    /// Suggestions generated
    pub suggestions: Vec<ConsolidationSuggestion>,
    /// Number of entries analyzed
    pub entries_analyzed: usize,
    /// Number of potential duplicates found
    pub duplicates_found: usize,
    /// Number of merge opportunities found
    pub merge_opportunities: usize,
}

/// Build the prompt for Claude to analyze memories
pub fn build_consolidation_prompt(entries: &[Entry]) -> String {
    let mut prompt = String::new();

    prompt.push_str("Analyze these memories from an AI coding assistant's memory system and suggest consolidations.\n\n");
    prompt.push_str("## Task\n\n");
    prompt.push_str("Review the following memories and identify:\n");
    prompt.push_str("1. **Duplicates**: Memories that say essentially the same thing\n");
    prompt.push_str(
        "2. **Related**: Memories that could be merged into a more comprehensive entry\n",
    );
    prompt.push_str("3. **Outdated**: Memories that conflict with newer information\n\n");
    prompt.push_str("For each group you identify, suggest how to consolidate them.\n\n");

    prompt.push_str("## Memories\n\n");

    for entry in entries {
        prompt.push_str(&format!("### ID: {}\n", entry.id));
        prompt.push_str(&format!("**Type**: {}\n", entry.entry_type));
        if let Some(title) = &entry.title {
            prompt.push_str(&format!("**Title**: {title}\n"));
        }
        if !entry.tags.is_empty() {
            prompt.push_str(&format!("**Tags**: {}\n", entry.tags.join(", ")));
        }
        prompt.push_str(&format!(
            "**Helpful/Harmful**: {}/{}\n",
            entry.helpful_count, entry.harmful_count
        ));
        prompt.push_str(&format!(
            "**Created**: {}\n",
            entry.created.format("%Y-%m-%d")
        ));
        prompt.push_str(&format!("\n{}\n\n", entry.content));
        prompt.push_str("---\n\n");
    }

    prompt.push_str("## Response Format\n\n");
    prompt.push_str("Respond with JSON only, no markdown code blocks:\n");
    prompt.push_str(
        r#"
{
  "suggestions": [
    {
      "source_ids": ["id1", "id2"],
      "action": "merge",
      "merged_content": "The consolidated content here...",
      "merged_title": "Optional title",
      "merged_tags": ["tag1", "tag2"],
      "reasoning": "Why these should be merged",
      "confidence": 0.85
    }
  ],
  "analysis": {
    "duplicates_found": 2,
    "merge_opportunities": 1,
    "summary": "Brief summary of findings"
  }
}

Action types:
- "merge": Combine multiple memories into one
- "dedupe": Remove duplicates, keeping the best version
- "update": Update one memory with info from others
- "skip": No action needed for this group
"#,
    );

    prompt
}

/// Parse Claude's response into consolidation suggestions
pub fn parse_consolidation_response(response: &str) -> Result<ConsolidationResult, CasError> {
    // Try to extract JSON from the response (handle markdown code blocks)
    let json_str = if response.contains("```json") {
        response
            .split("```json")
            .nth(1)
            .and_then(|s| s.split("```").next())
            .unwrap_or(response)
    } else if response.contains("```") {
        response
            .split("```")
            .nth(1)
            .and_then(|s| s.split("```").next())
            .unwrap_or(response)
    } else {
        response
    };

    #[derive(Deserialize)]
    struct Response {
        suggestions: Vec<ConsolidationSuggestion>,
        analysis: Option<Analysis>,
    }

    #[derive(Deserialize)]
    struct Analysis {
        duplicates_found: Option<usize>,
        merge_opportunities: Option<usize>,
    }

    let parsed: Response = serde_json::from_str(json_str.trim()).map_err(|e| {
        CasError::Other(format!(
            "Failed to parse consolidation response: {}. Response: {}",
            e,
            &json_str[..json_str.len().min(500)]
        ))
    })?;

    let analysis = parsed.analysis.unwrap_or(Analysis {
        duplicates_found: None,
        merge_opportunities: None,
    });

    Ok(ConsolidationResult {
        suggestions: parsed.suggestions,
        entries_analyzed: 0, // Filled in by caller
        duplicates_found: analysis.duplicates_found.unwrap_or(0),
        merge_opportunities: analysis.merge_opportunities.unwrap_or(0),
    })
}

/// Find groups of potentially related entries based on tags and type
///
/// Note: Semantic clustering via embeddings has been removed. This now uses
/// tag-based grouping only. Semantic clustering may return as a cloud feature.
pub fn find_related_groups(entries: &[Entry], _similarity_threshold: f64) -> Vec<Vec<&Entry>> {
    find_related_groups_by_tags(entries)
}

/// Find related entries by tags and type
fn find_related_groups_by_tags(entries: &[Entry]) -> Vec<Vec<&Entry>> {
    use std::collections::HashMap;

    let mut groups: HashMap<String, Vec<&Entry>> = HashMap::new();

    for entry in entries {
        // Create a key from type and primary tag
        let key = if !entry.tags.is_empty() {
            format!("{}:{}", entry.entry_type, entry.tags[0])
        } else {
            entry.entry_type.to_string()
        };

        groups.entry(key).or_default().push(entry);
    }

    // Only return groups with more than one entry
    groups.into_values().filter(|g| g.len() > 1).collect()
}

pub mod ai {
    //! AI-powered consolidation using Claude

    use crate::consolidation::*;
    use crate::tracing::claude_wrapper::traced_prompt;
    use claude_rs::QueryOptions;

    /// Run AI-powered consolidation on a batch of entries
    pub async fn consolidate_batch(
        entries: &[Entry],
        config: &ConsolidationConfig,
    ) -> Result<ConsolidationResult, CasError> {
        if entries.is_empty() {
            return Ok(ConsolidationResult {
                suggestions: vec![],
                entries_analyzed: 0,
                duplicates_found: 0,
                merge_opportunities: 0,
            });
        }

        let prompt_text = build_consolidation_prompt(entries);

        let mut options = QueryOptions::default().model(&config.model);
        if config.model == "claude-opus-5-5" {
            options = options.extra_arg("--effort").extra_arg("low");
        }

        let result = traced_prompt(&prompt_text, options, "consolidation")
            .await
            .map_err(|e| CasError::Other(format!("Claude consolidation failed: {e}")))?;

        let response_text = result.text();
        let mut result = parse_consolidation_response(response_text)?;
        result.entries_analyzed = entries.len();

        Ok(result)
    }

    /// Run consolidation on all entries, processing in batches
    pub async fn consolidate_all(
        entries: &[Entry],
        config: &ConsolidationConfig,
    ) -> Result<ConsolidationResult, CasError> {
        let mut all_suggestions = Vec::new();
        let mut total_duplicates = 0;
        let mut total_merge_opportunities = 0;

        // Group related entries first
        let groups = find_related_groups(entries, config.similarity_threshold);

        for group in groups {
            // Process each group with AI, chunking large groups into batch_size pieces
            let group_entries: Vec<Entry> = group.into_iter().cloned().collect();
            for chunk in group_entries.chunks(config.batch_size) {
                let batch_result = consolidate_batch(chunk, config).await?;

                all_suggestions.extend(batch_result.suggestions);
                total_duplicates += batch_result.duplicates_found;
                total_merge_opportunities += batch_result.merge_opportunities;
            }
        }

        Ok(ConsolidationResult {
            suggestions: all_suggestions,
            entries_analyzed: entries.len(),
            duplicates_found: total_duplicates,
            merge_opportunities: total_merge_opportunities,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::consolidation::*;

    #[test]
    fn test_parse_consolidation_response() {
        let response = r#"{
            "suggestions": [
                {
                    "source_ids": ["2024-01-01-001", "2024-01-02-001"],
                    "action": "merge",
                    "merged_content": "Combined content",
                    "merged_title": "Title",
                    "merged_tags": ["rust"],
                    "reasoning": "Similar topics",
                    "confidence": 0.9
                }
            ],
            "analysis": {
                "duplicates_found": 1,
                "merge_opportunities": 1,
                "summary": "Found related entries"
            }
        }"#;

        let result = parse_consolidation_response(response).unwrap();
        assert_eq!(result.suggestions.len(), 1);
        assert_eq!(result.suggestions[0].source_ids.len(), 2);
        assert_eq!(result.suggestions[0].action, ConsolidationAction::Merge);
        assert_eq!(result.duplicates_found, 1);
    }

    #[test]
    fn test_parse_response_with_code_block() {
        let response =
            "Here's the analysis:\n```json\n{\"suggestions\": [], \"analysis\": {}}\n```";
        let result = parse_consolidation_response(response).unwrap();
        assert!(result.suggestions.is_empty());
    }
}
