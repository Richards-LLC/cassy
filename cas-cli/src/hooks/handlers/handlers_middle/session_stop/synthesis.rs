use crate::hooks::handlers::*;

fn source_ids_for_session(store: &dyn Store, session_id: &str) -> Result<Vec<String>, MemError> {
    Ok(store
        .list_by_session(session_id)?
        .into_iter()
        .filter(|entry| entry.entry_type == EntryType::Observation)
        .map(|entry| entry.id)
        .collect())
}

pub fn synthesize_with_ai_sync(
    cas_root: &std::path::Path,
    buffered: &[crate::tracing::BufferedObservation],
    session_id: &str,
    source_ids: &[String],
) -> Result<usize, MemError> {
    use std::time::Duration;
    use tokio::runtime::Runtime;

    let rt =
        Runtime::new().map_err(|e| MemError::Other(format!("Failed to create runtime: {e}")))?;

    // 5 second timeout to prevent blocking the hook
    rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            synthesize_with_ai_async(cas_root, buffered, session_id, source_ids),
        )
        .await
        .map_err(|_| MemError::Other("AI buffer synthesis timed out after 5s".to_string()))?
    })
}

/// AI-powered buffer synthesis
///
/// Uses Claude to analyze buffered observations and extract meaningful learnings.
/// More nuanced than rule-based synthesis but requires API call.
async fn synthesize_with_ai_async(
    cas_root: &std::path::Path,
    buffered: &[crate::tracing::BufferedObservation],
    session_id: &str,
    source_ids: &[String],
) -> Result<usize, MemError> {
    use crate::tracing::claude_wrapper::traced_prompt;
    use crate::types::EntryType;
    use claude_rs::QueryOptions;

    // Build observation text for the prompt
    let obs_text: String = buffered
        .iter()
        .take(30) // Limit to prevent token overflow
        .map(|o| {
            let error_marker = if o.is_error { " [ERROR]" } else { "" };
            let file_info = o.file_path.as_deref().unwrap_or("");
            format!(
                "- [{}]{} {} {}",
                o.tool_name,
                error_marker,
                file_info,
                o.content.chars().take(100).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let prompt_text = format!(
        r#"Analyze these tool observations from a coding session and extract meaningful learnings.

## Observations
{obs_text}

## Task
Extract 1-3 actionable learnings from these observations. Focus on:
- Patterns that worked or failed
- Architectural decisions made
- Error patterns and their resolutions
- Project-specific conventions discovered

Respond with JSON only:
{{"learnings": [{{"content": "...", "importance": 0.5, "tags": ["tag1"]}}]}}"#
    );

    let result = traced_prompt(
        &prompt_text,
        QueryOptions::new().model("claude-haiku-4-5").max_turns(1),
        "buffer_synthesis",
    )
    .await
    .map_err(|e| MemError::Other(format!("AI synthesis failed: {e}")))?;

    let response_text = result.text();

    // Parse JSON response
    let json_str = response_text
        .find('{')
        .and_then(|start| {
            response_text
                .rfind('}')
                .map(|end| &response_text[start..=end])
        })
        .unwrap_or(response_text);

    #[derive(serde::Deserialize)]
    struct AiLearning {
        content: String,
        importance: Option<f32>,
        tags: Option<Vec<String>>,
    }

    #[derive(serde::Deserialize)]
    struct AiResponse {
        learnings: Vec<AiLearning>,
    }

    let ai_response: AiResponse = serde_json::from_str(json_str)
        .map_err(|e| MemError::Parse(format!("Failed to parse AI response: {e}")))?;

    // Store the learnings
    let store = open_store(cas_root)?;
    let mut count = 0;

    for learning in ai_response.learnings.into_iter().take(3) {
        let id = store.generate_id()?;
        let mut tags = learning.tags.unwrap_or_default();
        tags.push("ai-synthesized".to_string());

        let entry = Entry {
            id,
            entry_type: EntryType::Learning,
            content: learning.content,
            tags,
            session_id: Some(session_id.to_string()),
            source_ids: source_ids.to_vec(),
            importance: learning.importance.unwrap_or(0.5),
            ..Default::default()
        };

        if store.add(&entry).is_ok() {
            count += 1;
        }
    }

    Ok(count)
}

/// Synthesize buffered observations into learnings
///
/// Instead of storing every observation, we analyze the buffer and extract
/// meaningful learnings at session end. This dramatically reduces noise.
///
/// When `ai-extraction` feature is enabled and config allows, uses Claude
/// for smarter synthesis. Otherwise falls back to rule-based synthesis.
pub fn synthesize_buffered_observations(
    cas_root: &std::path::Path,
    buffered: &[crate::tracing::BufferedObservation],
    session_id: &str,
) -> Result<usize, MemError> {
    let store = open_store(cas_root)?;
    let source_ids = source_ids_for_session(store.as_ref(), session_id)?;
    synthesize_buffered_observations_with_sources(cas_root, buffered, session_id, &source_ids)
}

/// Synthesize buffered observations while retaining the raw observation IDs
/// captured by the caller before session cleanup archives older observations.
pub fn synthesize_buffered_observations_with_sources(
    cas_root: &std::path::Path,
    buffered: &[crate::tracing::BufferedObservation],
    session_id: &str,
    source_ids: &[String],
) -> Result<usize, MemError> {
    use crate::types::EntryType;

    // Try AI synthesis first if enabled and we have enough observations
    {
        let config = Config::load(cas_root).unwrap_or_default();
        let use_ai = config
            .hooks
            .as_ref()
            .map(|h| h.generate_summaries)
            .unwrap_or(false);

        if use_ai && buffered.len() >= 3 {
            match synthesize_with_ai_sync(cas_root, buffered, session_id, source_ids) {
                Ok(count) if count > 0 => return Ok(count),
                Ok(_) => {} // Fall through to rule-based
                Err(e) => eprintln!("cas: AI synthesis failed, using rule-based: {e}"),
            }
        }
    }

    let store = open_store(cas_root)?;

    // Group observations by type
    let errors: Vec<_> = buffered.iter().filter(|o| o.is_error).collect();
    let writes: Vec<_> = buffered.iter().filter(|o| o.tool_name == "Write").collect();
    let significant_edits: Vec<_> = buffered.iter().filter(|o| o.tool_name == "Edit").collect();
    let builds: Vec<_> = buffered
        .iter()
        .filter(|o| o.tool_name == "Bash" && !o.is_error)
        .collect();

    let mut learnings_created = 0;

    // Create learning from errors (failures are valuable learning opportunities)
    if !errors.is_empty() {
        let error_summary = errors
            .iter()
            .take(5)
            .map(|e| format!("- {}", e.content.chars().take(100).collect::<String>()))
            .collect::<Vec<_>>()
            .join("\n");

        let content = format!(
            "Session encountered {} errors/failures:\n{}",
            errors.len(),
            error_summary
        );

        let id = store.generate_id()?;
        let entry = Entry {
            id,
            entry_type: EntryType::Learning,
            content,
            tags: vec!["session-errors".to_string(), "synthesized".to_string()],
            session_id: Some(session_id.to_string()),
            source_ids: source_ids.to_vec(),
            importance: 0.7, // Errors are valuable learnings
            ..Default::default()
        };

        if store.add(&entry).is_ok() {
            learnings_created += 1;
        }
    }

    // Create learning from new file creation (architectural decisions)
    if !writes.is_empty() {
        let files_created: Vec<_> = writes
            .iter()
            .filter_map(|w| w.file_path.as_ref())
            .take(10)
            .collect();

        if !files_created.is_empty() {
            let content = format!(
                "Created {} new files: {}",
                writes.len(),
                files_created
                    .iter()
                    .map(|f| f.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );

            let id = store.generate_id()?;
            let entry = Entry {
                id,
                entry_type: EntryType::Learning,
                content,
                tags: vec!["files-created".to_string(), "synthesized".to_string()],
                session_id: Some(session_id.to_string()),
                source_ids: source_ids.to_vec(),
                importance: 0.5,
                ..Default::default()
            };

            if store.add(&entry).is_ok() {
                learnings_created += 1;
            }
        }
    }

    // Create learning from significant edits
    if significant_edits.len() >= 3 {
        let files_modified: Vec<_> = significant_edits
            .iter()
            .filter_map(|e| e.file_path.as_ref())
            .take(10)
            .collect();

        let content = format!(
            "Made {} significant edits to: {}",
            significant_edits.len(),
            files_modified
                .iter()
                .map(|f| f.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );

        let id = store.generate_id()?;
        let entry = Entry {
            id,
            entry_type: EntryType::Learning,
            content,
            tags: vec!["significant-edits".to_string(), "synthesized".to_string()],
            session_id: Some(session_id.to_string()),
            source_ids: source_ids.to_vec(),
            importance: 0.4,
            ..Default::default()
        };

        if store.add(&entry).is_ok() {
            learnings_created += 1;
        }
    }

    // Create learning from successful builds/tests (patterns that work)
    let successful_tests: Vec<_> = builds
        .iter()
        .filter(|b| {
            let content_lower = b.content.to_lowercase();
            content_lower.contains("test") || content_lower.contains("build")
        })
        .collect();

    if !successful_tests.is_empty() {
        let content = format!(
            "Successful build/test commands: {}",
            successful_tests
                .iter()
                .take(5)
                .map(|t| t.content.chars().take(80).collect::<String>())
                .collect::<Vec<_>>()
                .join("; ")
        );

        let id = store.generate_id()?;
        let entry = Entry {
            id,
            entry_type: EntryType::Learning,
            content,
            tags: vec!["build-success".to_string(), "synthesized".to_string()],
            session_id: Some(session_id.to_string()),
            source_ids: source_ids.to_vec(),
            importance: 0.3,
            ..Default::default()
        };

        if store.add(&entry).is_ok() {
            learnings_created += 1;
        }
    }

    Ok(learnings_created)
}
