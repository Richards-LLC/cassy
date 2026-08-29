use std::sync::Arc;

use crate::daemon::DaemonConfig;
use crate::error::CasError;
use crate::store::Store;

fn parse_learning_source_ids(source_id: &str) -> Vec<String> {
    source_id
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) fn process_observations(
    store: &Arc<dyn Store>,
    config: &DaemonConfig,
) -> Result<usize, CasError> {
    use crate::extraction::{AIExtractor, AIExtractorAsync, AIExtractorConfig};
    use crate::types::{Entry, EntryType};

    let pending = store.list_pending(config.batch_size)?;

    if pending.is_empty() {
        return Ok(0);
    }

    let extractor_config = AIExtractorConfig {
        model: config.model.clone(),
        extract_preferences: false,
        suggest_rules: false,
        ..Default::default()
    };
    let extractor = AIExtractor::new(extractor_config);

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| CasError::Other(format!("Failed to create runtime: {error}")))?;

    // Skip entries that are too short and mark them as extracted
    let mut batch: Vec<Entry> = Vec::with_capacity(pending.len());
    for entry in &pending {
        if entry.content.len() < 20 {
            store.mark_extracted(&entry.id)?;
        } else {
            batch.push(entry.clone());
        }
    }

    if batch.is_empty() {
        return Ok(0);
    }

    let mut extracted_count = 0;

    match runtime.block_on(extractor.extract_batch_async(&batch)) {
        Ok(result) => {
            for learning in result.learnings {
                if learning.confidence >= 0.6 {
                    let id = store.generate_id()?;
                    let new_entry = Entry {
                        id,
                        entry_type: EntryType::Learning,
                        content: learning.content,
                        tags: learning.tags,
                        source_ids: parse_learning_source_ids(&learning.source_id),
                        importance: learning.confidence,
                        ..Default::default()
                    };
                    let _ = store.add(&new_entry);
                    extracted_count += 1;
                }
            }
            // Mark all batched entries as extracted
            for entry in &batch {
                store.mark_extracted(&entry.id)?;
            }
        }
        Err(error) => {
            eprintln!(
                "cas: Batch extraction failed for {} observations: {}",
                batch.len(),
                error
            );
            // Mark all as extracted to avoid retrying forever
            for entry in &batch {
                store.mark_extracted(&entry.id)?;
            }
        }
    }

    Ok(extracted_count)
}

#[cfg(test)]
mod tests {
    use super::parse_learning_source_ids;

    #[test]
    fn learning_source_ids_preserve_every_observation_id() {
        assert_eq!(
            parse_learning_source_ids("obs-1, obs-2,,  obs-3 "),
            ["obs-1", "obs-2", "obs-3"]
        );
        assert!(parse_learning_source_ids("  , ").is_empty());
    }
}
