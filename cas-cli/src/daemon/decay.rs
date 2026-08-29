use chrono::Utc;
use std::path::Path;
use std::sync::Arc;

use crate::consolidation::ConsolidationConfig;
use crate::daemon::DaemonConfig;
use crate::error::CasError;
use crate::store::Store;

/// Apply memory decay to entries based on time and access patterns.
pub(crate) fn apply_memory_decay(store: &Arc<dyn Store>) -> Result<usize, CasError> {
    use crate::types::{EntryType, MemoryTier};

    // Expiry is an absolute validity boundary, not a soft signal like age or
    // stability. List every active entry so even a previously pinned memory is
    // removed from the in-context tier once its valid_until has passed.
    let entries = store.list()?;
    let now = Utc::now();
    let mut count = 0;

    for entry in entries {
        let mut updated = entry.clone();
        let mut needs_update = false;

        if updated.is_expired() && updated.memory_tier != MemoryTier::Archive {
            updated.memory_tier = MemoryTier::Archive;
            needs_update = true;
        }

        // Archived entries need no further decay work. Expired entries above
        // intentionally reach this branch after their one-time demotion.
        if updated.memory_tier == MemoryTier::Archive {
            if needs_update {
                store.update(&updated)?;
                count += 1;
            }
            continue;
        }

        // In-context residency protects a currently relevant memory from
        // ordinary feedback and age-based decay. Expiry is handled above as
        // an absolute validity boundary, so an expired in-context entry has
        // already been demoted before reaching this skip.
        if updated.memory_tier == MemoryTier::InContext {
            continue;
        }

        if updated.entry_type == EntryType::Observation
            && updated.memory_tier == MemoryTier::Working
            && updated.feedback_score() <= 0
        {
            updated.memory_tier = MemoryTier::Cold;
            needs_update = true;
        }

        if updated.importance < 0.3
            && updated.memory_tier == MemoryTier::Working
            && updated.feedback_score() <= 0
        {
            updated.memory_tier = MemoryTier::Cold;
            needs_update = true;
        }

        if updated.feedback_score() < 0 && updated.memory_tier != MemoryTier::Archive {
            updated.memory_tier = MemoryTier::Archive;
            needs_update = true;
        }

        let days_old = (now - entry.created).num_days() as f32;
        if days_old >= 3.0 {
            let days_since_access = entry
                .last_accessed
                .map(|time| (now - time).num_days() as f32)
                .unwrap_or(days_old);

            if days_since_access > 7.0 {
                updated.apply_decay(days_since_access / 30.0);
                needs_update = true;
            }
        }

        if updated.stability < 0.3 && updated.memory_tier == MemoryTier::Working {
            updated.memory_tier = MemoryTier::Cold;
            needs_update = true;
        }

        if updated.stability < 0.15 && updated.memory_tier == MemoryTier::Cold {
            updated.memory_tier = MemoryTier::Archive;
            needs_update = true;
        }

        if needs_update {
            store.update(&updated)?;
            count += 1;
        }
    }

    Ok(count)
}

/// Run AI-powered consolidation.
pub(crate) fn run_consolidation(
    store: &Arc<dyn Store>,
    config: &DaemonConfig,
) -> Result<usize, CasError> {
    use crate::consolidation::{ConsolidationAction, ai::consolidate_all};
    use crate::types::{Entry, EntryType, Scope};
    use tokio::runtime::Runtime;

    let entries = store.list()?;
    let consolidation_config = ConsolidationConfig {
        model: config.model.clone(),
        batch_size: config.batch_size,
        ..Default::default()
    };

    let runtime =
        Runtime::new().map_err(|error| CasError::Other(format!("Runtime error: {error}")))?;
    let result = runtime.block_on(consolidate_all(&entries, &consolidation_config))?;

    let mut applied = 0;

    for suggestion in result.suggestions {
        if suggestion.confidence >= 0.85 && suggestion.action != ConsolidationAction::Skip {
            for id in &suggestion.source_ids {
                let _ = store.archive(id);
            }

            let id = store.generate_id()?;
            let entry = Entry {
                id,
                scope: Scope::default(),
                entry_type: EntryType::Learning,
                observation_type: None,
                source_ids: suggestion.source_ids.clone(),
                tags: suggestion.merged_tags,
                created: Utc::now(),
                content: suggestion.merged_content,
                raw_content: None,
                compressed: false,
                memory_tier: Default::default(),
                title: suggestion.merged_title,
                helpful_count: 0,
                harmful_count: 0,
                last_accessed: None,
                archived: false,
                session_id: None,
                source_tool: None,
                pending_extraction: false,
                pending_embedding: true,
                stability: 0.5,
                access_count: 0,
                importance: 0.5,
                valid_from: None,
                valid_until: None,
                review_after: None,
                last_reviewed: None,
                domain: None,
                belief_type: Default::default(),
                confidence: 1.0,
                branch: None,
                team_id: None,
                share: None,
            };

            store.add(&entry)?;
            applied += 1;
        }
    }

    Ok(applied)
}

/// Auto-prune stale entries.
pub(crate) fn auto_prune(store: &Arc<dyn Store>) -> Result<usize, CasError> {
    let entries = store.list_prunable(0.1)?;
    let mut pruned = 0;

    for entry in entries {
        if entry.should_prune(0.1) {
            store.archive(&entry.id)?;
            pruned += 1;
        }
    }

    Ok(pruned)
}

/// Update entity summaries for entities that need refreshing.
pub(crate) fn run_entity_summary_update(
    store: &Arc<dyn Store>,
    cas_root: &Path,
) -> Result<usize, CasError> {
    use crate::extraction::summary::{SummaryGenerator, update_entity_summaries};
    use crate::store::open_entity_store;

    let entity_store = open_entity_store(cas_root)?;
    let generator = SummaryGenerator::new();

    let updates =
        generator.generate_for_stale_entities(entity_store.as_ref(), store.as_ref(), 7)?;

    if updates.is_empty() {
        return Ok(0);
    }

    Ok(update_entity_summaries(
        entity_store.as_ref(),
        store.as_ref(),
        &updates,
    )?)
}
