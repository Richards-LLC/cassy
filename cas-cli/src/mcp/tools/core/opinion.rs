use crate::mcp::tools::core::imports::*;

impl CasCore {
    // ========================================================================
    // Opinion Tools (Hindsight-inspired epistemic memory)
    // ========================================================================

    /// Reinforce an opinion with supporting evidence
    pub async fn cas_opinion_reinforce(
        &self,
        Parameters(req): Parameters<OpinionReinforceRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        let mut entry = store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Entry not found: {e}")),
            data: None,
        })?;

        // Verify it's an opinion or hypothesis
        if entry.belief_type == BeliefType::Fact {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Cannot reinforce a fact. Only opinions and hypotheses can be reinforced.",
            ));
        }

        let old_confidence = entry.confidence;
        let old_type = entry.belief_type;

        // Reinforce the confidence
        entry.reinforce_confidence(0.5);
        entry.helpful_count += 1;

        store.update(&entry).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        let mut msg = format!(
            "Reinforced {}: confidence {:.0}% → {:.0}%",
            req.id,
            old_confidence * 100.0,
            entry.confidence * 100.0
        );

        // Note if belief type was promoted
        if entry.belief_type != old_type {
            msg.push_str(&format!(" (promoted to {:?})", entry.belief_type));
        }

        msg.push_str(&format!("\nEvidence: {}", truncate_str(&req.evidence, 100)));

        Ok(Self::success(msg))
    }

    /// Weaken an opinion with contradicting evidence
    pub async fn cas_opinion_weaken(
        &self,
        Parameters(req): Parameters<OpinionWeakenRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        let mut entry = store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Entry not found: {e}")),
            data: None,
        })?;

        if entry.belief_type == BeliefType::Fact {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Cannot weaken a fact. Use cas_harmful to mark facts as incorrect.",
            ));
        }

        let old_confidence = entry.confidence;
        let old_type = entry.belief_type;

        entry.weaken_confidence(0.5);
        entry.harmful_count += 1;

        store.update(&entry).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        let mut msg = format!(
            "Weakened {}: confidence {:.0}% → {:.0}%",
            req.id,
            old_confidence * 100.0,
            entry.confidence * 100.0
        );

        if entry.belief_type != old_type {
            msg.push_str(&format!(" (demoted to {:?})", entry.belief_type));
        }

        msg.push_str(&format!("\nEvidence: {}", truncate_str(&req.evidence, 100)));

        Ok(Self::success(msg))
    }

    /// Strongly contradict an opinion
    pub async fn cas_opinion_contradict(
        &self,
        Parameters(req): Parameters<OpinionContradictRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        let mut entry = store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Entry not found: {e}")),
            data: None,
        })?;

        if entry.belief_type == BeliefType::Fact {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Cannot contradict a fact. Use cas_harmful to mark facts as incorrect.",
            ));
        }

        let old_confidence = entry.confidence;

        entry.contradict_confidence(0.5);
        entry.harmful_count += 2; // Strong contradiction counts more

        // Archive if confidence is very low
        let archived = entry.confidence < 0.1;
        if archived {
            entry.archived = true;
        }

        store.update(&entry).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        let mut msg = format!(
            "Contradicted {}: confidence {:.0}% → {:.0}%",
            req.id,
            old_confidence * 100.0,
            entry.confidence * 100.0
        );

        if archived {
            msg.push_str(" (archived due to very low confidence)");
        }

        msg.push_str(&format!("\nEvidence: {}", truncate_str(&req.evidence, 100)));

        Ok(Self::success(msg))
    }

    // ========================================================================
    // Entity Tools (Knowledge Graph)
    // ========================================================================

    /// List entities in the knowledge graph
    pub async fn cas_entity_list(
        &self,
        Parameters(req): Parameters<EntityListRequest>,
    ) -> Result<CallToolResult, McpError> {
        use crate::types::ScopeFilter;
        use std::collections::HashSet;

        let entity_store = self.open_entity_store()?;

        let entity_type = req
            .entity_type
            .as_ref()
            .and_then(|t| t.parse::<crate::types::EntityType>().ok());

        let has_tags_filter = req.tags.as_ref().is_some_and(|t| !t.is_empty());
        let has_scope_filter = req.scope.as_ref().is_some_and(|s| s != "all");
        let query = req
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty());

        let mut entities = if let Some(query) = query {
            if !has_tags_filter && !has_scope_filter {
                // Fast path: push query filtering into SQL when no entry-based filters are needed.
                entity_store
                    .search_entities(query, entity_type)
                    .map_err(|e| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!("Failed to search entities: {e}")),
                        data: None,
                    })?
            } else {
                entity_store
                    .list_entities(entity_type)
                    .map_err(|e| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!("Failed to list entities: {e}")),
                        data: None,
                    })?
            }
        } else {
            entity_store
                .list_entities(entity_type)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to list entities: {e}")),
                    data: None,
                })?
        };

        // Apply tags/scope filter (entities mentioned in entries matching the filter)
        if has_tags_filter || has_scope_filter {
            let store = self.open_store()?;
            let entries = store.list().map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to list entries: {e}")),
                data: None,
            })?;

            // Parse scope filter
            let scope_filter: ScopeFilter = req
                .scope
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or(ScopeFilter::All);

            // Parse tags filter
            let tag_filter: Vec<String> = req
                .tags
                .as_ref()
                .map(|t| t.split(',').map(|s| s.trim().to_lowercase()).collect())
                .unwrap_or_default();

            // Apply a bounded entry scan before mention lookups.
            let entity_limit = req.limit.unwrap_or(50);
            let entry_scan_limit = entity_limit.saturating_mul(20).max(entity_limit);
            let mut filtered_entry_ids: Vec<String> = Vec::new();
            for entry in entries {
                // Scope filter
                let scope_match = match scope_filter {
                    ScopeFilter::Global => entry.scope == crate::types::Scope::Global,
                    ScopeFilter::Project => entry.scope == crate::types::Scope::Project,
                    ScopeFilter::All => true,
                };
                if !scope_match {
                    continue;
                }

                // Tags filter (entry must have all specified tags)
                if !tag_filter.is_empty() {
                    let entry_tags: Vec<String> =
                        entry.tags.iter().map(|t| t.to_lowercase()).collect();
                    if !tag_filter.iter().all(|t| entry_tags.contains(t)) {
                        continue;
                    }
                }

                filtered_entry_ids.push(entry.id);
                if filtered_entry_ids.len() >= entry_scan_limit {
                    break;
                }
            }

            // Get entity IDs mentioned in the filtered entries
            let mut allowed_entity_ids: HashSet<String> = HashSet::new();
            for entry_id in &filtered_entry_ids {
                if let Ok(mentions) = entity_store.get_entry_mentions(entry_id) {
                    for mention in mentions {
                        allowed_entity_ids.insert(mention.entity_id);
                    }
                }
            }

            // Filter entities to only include those mentioned in matching entries
            entities.retain(|e| allowed_entity_ids.contains(&e.id));
        }

        // Apply query filter (case-insensitive substring match on name or description)
        if has_tags_filter || has_scope_filter {
            if let Some(query) = query {
                let query_lower = query.to_lowercase();
                entities.retain(|e| {
                    e.name.to_lowercase().contains(&query_lower)
                        || e.aliases
                            .iter()
                            .any(|a| a.to_lowercase().contains(&query_lower))
                        || e.description
                            .as_ref()
                            .is_some_and(|d| d.to_lowercase().contains(&query_lower))
                });
            }
        } else if let Some(query) = query {
            // Keep description matches when using SQL search_entities fast path.
            let query_lower = query.to_lowercase();
            entities.retain(|e| {
                e.name.to_lowercase().contains(&query_lower)
                    || e.aliases
                        .iter()
                        .any(|a| a.to_lowercase().contains(&query_lower))
                    || e.description
                        .as_ref()
                        .is_some_and(|d| d.to_lowercase().contains(&query_lower))
            });
        }

        // Apply sorting
        let sort_field = req.sort.as_deref().unwrap_or("updated");
        let sort_desc = req.sort_order.as_deref().unwrap_or("desc") == "desc";

        match sort_field {
            "name" => {
                entities.sort_by(|a, b| {
                    let cmp = a.name.to_lowercase().cmp(&b.name.to_lowercase());
                    if sort_desc { cmp.reverse() } else { cmp }
                });
            }
            "created" => {
                entities.sort_by(|a, b| {
                    let cmp = a.created.cmp(&b.created);
                    if sort_desc { cmp.reverse() } else { cmp }
                });
            }
            "mentions" => {
                entities.sort_by(|a, b| {
                    let cmp = a.mention_count.cmp(&b.mention_count);
                    if sort_desc { cmp.reverse() } else { cmp }
                });
            }
            _ => {
                // Default: sort by updated
                entities.sort_by(|a, b| {
                    let cmp = a.updated.cmp(&b.updated);
                    if sort_desc { cmp.reverse() } else { cmp }
                });
            }
        }

        let limit = req.limit.unwrap_or(50);
        let entities: Vec<_> = entities.into_iter().take(limit).collect();

        if entities.is_empty() {
            return Ok(Self::success("No entities found".to_string()));
        }

        let mut output = format!("Entities ({}):\n\n", entities.len());
        for entity in &entities {
            output.push_str(&format!(
                "- [{:?}] {} ({})\n",
                entity.entity_type, entity.name, entity.id
            ));
        }

        Ok(Self::success(output))
    }

    /// Show entity details
    pub async fn cas_entity_show(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let entity_store = self.open_entity_store()?;

        // Try by ID first, then by name
        let entity = match entity_store.get_entity(&req.id) {
            Ok(e) => e,
            Err(_) => entity_store
                .get_entity_by_name(&req.id, None)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to search entity: {e}")),
                    data: None,
                })?
                .ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("Entity not found: {}", req.id)),
                    data: None,
                })?,
        };

        let mentions = entity_store
            .get_entity_mentions(&entity.id)
            .unwrap_or_default();
        let relationships = entity_store
            .get_entity_relationships(&entity.id)
            .unwrap_or_default();

        let mut output = format!(
            "Entity: {}\n==============\n\nID: {}\nType: {:?}\n",
            entity.name, entity.id, entity.entity_type
        );

        if !entity.aliases.is_empty() {
            output.push_str(&format!("Aliases: {}\n", entity.aliases.join(", ")));
        }
        if let Some(desc) = &entity.description {
            output.push_str(&format!("Description: {desc}\n"));
        }
        output.push_str(&format!(
            "Created: {}\n",
            entity.created.format("%Y-%m-%d %H:%M")
        ));
        output.push_str(&format!("Mentions: {} entries\n", mentions.len()));
        output.push_str(&format!("Relationships: {}\n", relationships.len()));

        Ok(Self::success(output))
    }

    /// Extract entities from entries (backfill)
    pub async fn cas_entity_extract(
        &self,
        Parameters(req): Parameters<EntityExtractRequest>,
    ) -> Result<CallToolResult, McpError> {
        use crate::extraction::entities::PatternEntityExtractor;
        use crate::types::ScopeFilter;

        let store = self.open_store()?;
        let entity_store = self.open_entity_store()?;
        let extractor = PatternEntityExtractor::default();

        let entries = store.list().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list entries: {e}")),
            data: None,
        })?;

        // Parse scope filter
        let scope_filter: ScopeFilter = req
            .scope
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(ScopeFilter::All);

        // Parse tags filter
        let tag_filter: Vec<String> = req
            .tags
            .as_ref()
            .map(|t| t.split(',').map(|s| s.trim().to_lowercase()).collect())
            .unwrap_or_default();

        // Parse entity type filter
        let entity_type_filter = req
            .entity_type
            .as_ref()
            .and_then(|t| t.parse::<crate::types::EntityType>().ok());

        // Parse query filter
        let query_filter = req.query.as_ref().map(|q| q.to_lowercase());

        let limit = req.limit.unwrap_or(100);
        let mut entries_to_process = Vec::new();
        for entry in entries {
            // Scope filter
            let scope_match = match scope_filter {
                ScopeFilter::Global => entry.scope == crate::types::Scope::Global,
                ScopeFilter::Project => entry.scope == crate::types::Scope::Project,
                ScopeFilter::All => true,
            };
            if !scope_match {
                continue;
            }

            // Tags filter (entry must have all specified tags)
            if !tag_filter.is_empty() {
                let entry_tags: Vec<String> = entry.tags.iter().map(|t| t.to_lowercase()).collect();
                if !tag_filter.iter().all(|t| entry_tags.contains(t)) {
                    continue;
                }
            }

            // Query filter (content or title contains query)
            if let Some(ref query) = query_filter {
                let content_match = entry.content.to_lowercase().contains(query);
                let title_match = entry
                    .title
                    .as_ref()
                    .is_some_and(|t| t.to_lowercase().contains(query));
                if !content_match && !title_match {
                    continue;
                }
            }

            entries_to_process.push(entry);
            if entries_to_process.len() >= limit {
                break;
            }
        }

        let mut total_entities = 0;
        let mut total_mentions = 0;
        let mut processed_entries = 0;

        for entry in &entries_to_process {
            // Skip if entry already has mentions
            if let Ok(existing) = entity_store.get_entry_mentions(&entry.id) {
                if !existing.is_empty() {
                    continue;
                }
            }

            let result = extractor.extract(entry);
            if result.entities.is_empty() {
                continue;
            }

            processed_entries += 1;

            for extracted in &result.entities {
                // Skip entities that don't match the type filter
                if let Some(ref type_filter) = entity_type_filter {
                    if extracted.entity_type != *type_filter {
                        continue;
                    }
                }

                let entity_id = match entity_store
                    .get_entity_by_name(&extracted.name, Some(extracted.entity_type))
                {
                    Ok(Some(existing)) => existing.id,
                    Ok(None) => {
                        let id = entity_store.generate_entity_id().map_err(|e| McpError {
                            code: ErrorCode::INTERNAL_ERROR,
                            message: Cow::from(format!("Failed to generate ID: {e}")),
                            data: None,
                        })?;
                        let entity = extracted.to_entity(id.clone());
                        entity_store.add_entity(&entity).map_err(|e| McpError {
                            code: ErrorCode::INTERNAL_ERROR,
                            message: Cow::from(format!("Failed to add entity: {e}")),
                            data: None,
                        })?;
                        total_entities += 1;
                        id
                    }
                    Err(_) => continue,
                };

                let mention = extracted.to_mention(entity_id, entry.id.clone());
                let _ = entity_store.add_mention(&mention);
                total_mentions += 1;
            }
        }

        Ok(Self::success(format!(
            "Entity extraction complete:\n- Entries processed: {processed_entries}\n- New entities: {total_entities}\n- Mentions created: {total_mentions}"
        )))
    }

    // ========================================================================
    // System Info Tools
    // ========================================================================

    /// Get ML/embedding system information (deprecated - local ML removed)
    pub async fn cas_system_info(&self) -> Result<CallToolResult, McpError> {
        let mut output = String::from("Search System Info\n==================\n\n");

        output.push_str("⚠️  Local ML embeddings have been deprecated.\n");
        output.push_str("Semantic search is now a cloud-only feature.\n\n");

        output.push_str("Local Search:\n");
        output.push_str("  Engine: BM25 full-text search (Tantivy)\n");
        output.push_str("  Type: Keyword/lexical matching\n");

        // Search index info
        output.push_str("\nSearch Index:\n");
        if let Ok(search) = self.open_search_index() {
            output.push_str(&format!("  Fields: {}\n", search.field_names().join(", ")));
            output.push_str(&format!("  Field Count: {}\n", search.field_count()));
            output.push_str("  Status: Initialized\n");
        } else {
            output.push_str("  Status: Not initialized\n");
        }

        output.push_str("\nSemantic Search:\n");
        output.push_str("  Status: Cloud-only (requires API)\n");

        // Check for API keys
        let has_openai = std::env::var("OPENAI_API_KEY").is_ok();
        let has_voyage = std::env::var("VOYAGE_API_KEY").is_ok();

        if has_openai || has_voyage {
            output.push_str("  Cloud Providers:\n");
            if has_openai {
                output.push_str("    - OpenAI API: Configured\n");
            }
            if has_voyage {
                output.push_str("    - Voyage AI API: Configured\n");
            }
        } else {
            output.push_str("  Cloud Providers: None configured\n");
            output.push_str("    Set OPENAI_API_KEY or VOYAGE_API_KEY for semantic search\n");
        }

        Ok(Self::success(output))
    }
}
