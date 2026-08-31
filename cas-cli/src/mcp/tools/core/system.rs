use crate::mcp::tools::core::imports::*;
use cas_types::{Entry, MemoryTier};

#[derive(Debug, Default)]
struct EntryCorpusStats {
    /// Counts indexed by tier, then archived flag (`archived = 0`, `archived = 1`).
    by_tier: [(usize, usize); 4],
    total: usize,
    archived: usize,
    /// Entries in working/in-context tiers with `archived = 0`.
    live: usize,
}

impl EntryCorpusStats {
    fn record(&mut self, entry: &Entry) {
        let tier_index = match entry.memory_tier {
            MemoryTier::InContext => 0,
            MemoryTier::Working => 1,
            MemoryTier::Cold => 2,
            MemoryTier::Archive => 3,
        };
        let counts = &mut self.by_tier[tier_index];
        if entry.archived {
            counts.1 += 1;
            self.archived += 1;
        } else {
            counts.0 += 1;
            if entry.memory_tier.is_active() {
                self.live += 1;
            }
        }
        self.total += 1;
    }

    fn from_store_lists(entries: &[Entry], archived_entries: &[Entry]) -> Self {
        let mut stats = Self::default();
        for entry in entries.iter().chain(archived_entries) {
            stats.record(entry);
        }
        stats
    }

    fn tier_line(&self, tier: MemoryTier, tier_index: usize) -> String {
        let (unarchived, archived) = self.by_tier[tier_index];
        format!("  {tier}: {unarchived} archived=0, {archived} archived=1")
    }
}

#[derive(Debug, Default)]
struct StoreBackedIndexCounts {
    entries: usize,
    tasks: usize,
    rules: usize,
    skills: usize,
    latest_store_update: Option<chrono::DateTime<chrono::Utc>>,
}

/// Filesystem timestamps can be coarser than the store clock on some
/// platforms, so tolerate a small amount of clock/filesystem skew while still
/// catching edits that remain unindexed for an operator-visible interval.
const INDEX_FRESHNESS_TOLERANCE_SECS: i64 = 2;

fn index_last_modified(index_dir: &std::path::Path) -> Option<chrono::DateTime<chrono::Utc>> {
    std::fs::metadata(index_dir.join("meta.json"))
        .ok()?
        .modified()
        .ok()
        .map(chrono::DateTime::<chrono::Utc>::from)
}

impl StoreBackedIndexCounts {
    fn total(&self) -> usize {
        self.entries + self.tasks + self.rules + self.skills
    }

    fn observe(&mut self, timestamp: chrono::DateTime<chrono::Utc>) {
        self.latest_store_update = Some(
            self.latest_store_update
                .map_or(timestamp, |current| current.max(timestamp)),
        );
    }
}

fn format_index_counts(
    indexed: &StoreBackedIndexCounts,
    expected: &StoreBackedIndexCounts,
) -> String {
    format!(
        "{} of {} store-backed documents (entries {}/{}, tasks {}/{}, rules {}/{}, skills {}/{})",
        indexed.total(),
        expected.total(),
        indexed.entries,
        expected.entries,
        indexed.tasks,
        expected.tasks,
        indexed.rules,
        expected.rules,
        indexed.skills,
        expected.skills,
    )
}

impl CasCore {
    // ========================================================================
    // System Tools (5)
    // ========================================================================

    /// Get session context
    pub async fn cas_context(
        &self,
        Parameters(req): Parameters<LimitRequest>,
    ) -> Result<CallToolResult, McpError> {
        let limit = req.limit.unwrap_or_else(|| {
            crate::config::Config::load(&self.cas_root)
                .unwrap_or_default()
                .context_limit()
        });

        // Create a minimal HookInput for context building
        let hook_input = HookInput {
            session_id: "mcp".to_string(),
            transcript_path: None,
            cwd: self
                .cas_root
                .parent()
                .unwrap_or(&self.cas_root)
                .to_string_lossy()
                .to_string(),
            workspace_root: None,
            permission_mode: None,
            hook_event_name: "McpContext".to_string(),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            tool_use_id: None,
            tool_input_truncated: None,
            user_prompt: None,
            machine_prompt_provenance: None,
            source: None,
            reason: None,
            subagent_type: None,
            agent_id: None,
            agent_type: None,
            subagent_prompt: None,
            agent_role: std::env::var("CAS_AGENT_ROLE").ok(),
            message: None,
            message_is_final: None,
            index: None,
            stop_hook_active: None,
        };

        let context = build_context(&hook_input, limit, &self.cas_root).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to build context: {e}")),
            data: None,
        })?;

        if context.is_empty() {
            return Ok(Self::success("No context available"));
        }

        Ok(Self::success(context))
    }

    /// Get ambient context focused on a specific task.
    ///
    /// This keeps the normal session-context shape (including Helpful
    /// Memories), while adding the focused task title and description to the
    /// context query used by the memory scorer.
    pub async fn cas_context_for_task(
        &self,
        task_id: &str,
        limit: usize,
        max_tokens: Option<usize>,
    ) -> Result<CallToolResult, McpError> {
        let task = self.open_task_store()?.get(task_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;
        let task_content = format!("{} {}", task.title, task.description);

        let hook_input = HookInput {
            session_id: "mcp".to_string(),
            transcript_path: None,
            cwd: self
                .cas_root
                .parent()
                .unwrap_or(&self.cas_root)
                .to_string_lossy()
                .to_string(),
            workspace_root: None,
            permission_mode: None,
            hook_event_name: "McpContext".to_string(),
            tool_name: None,
            tool_input: None,
            tool_response: None,
            tool_use_id: None,
            tool_input_truncated: None,
            user_prompt: Some(task_content),
            machine_prompt_provenance: None,
            source: None,
            reason: None,
            subagent_type: None,
            agent_id: None,
            agent_type: None,
            subagent_prompt: None,
            agent_role: std::env::var("CAS_AGENT_ROLE").ok(),
            message: None,
            message_is_final: None,
            index: None,
            stop_hook_active: None,
        };

        let context =
            build_context_with_token_budget(&hook_input, limit, &self.cas_root, max_tokens)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to build task-focused context: {e}")),
                    data: None,
                })?;

        if context.is_empty() {
            return Ok(Self::success("No context available"));
        }

        Ok(Self::success(context))
    }

    /// Get memory statistics
    pub async fn cas_stats(&self) -> Result<CallToolResult, McpError> {
        use cas_store::SqliteRetrievalStore;

        let store = self.open_store()?;
        let rule_store = self.open_rule_store()?;
        let task_store = self.open_task_store()?;
        let skill_store = self.open_skill_store()?;

        let entries = store.list().unwrap_or_default();
        let archived = store.list_archived().unwrap_or_default();
        let rules = rule_store.list().unwrap_or_default();
        let tasks = task_store.list(None).unwrap_or_default();
        let skills = skill_store.list(None).unwrap_or_default();

        let corpus = EntryCorpusStats::from_store_lists(&entries, &archived);
        let proven_rules = rules
            .iter()
            .filter(|r| r.status == RuleStatus::Proven)
            .count();
        let open_tasks = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Open)
            .count();
        let in_progress = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::InProgress)
            .count();
        let enabled_skills = skills
            .iter()
            .filter(|s| s.status == SkillStatus::Enabled)
            .count();

        let retrieval_funnel = match SqliteRetrievalStore::open(&self.cas_root) {
            Err(error) => format!("Retrieval telemetry unavailable: {error}"),
            Ok(retrieval_store) => {
                match (
                    retrieval_store.aggregate(),
                    retrieval_store.rolling_injected_precision(30),
                ) {
                    (Err(error), _) | (_, Err(error)) => {
                        format!("Retrieval telemetry unavailable: {error}")
                    }
                    (Ok(retrieval_groups), Ok(rolling_precision)) => {
                        let precision_display = rolling_precision
                            .precision
                            .map(|value| {
                                format!(
                                    "{:.1}% ({}/{} judge labels)",
                                    value * 100.0,
                                    rolling_precision.helpful,
                                    rolling_precision.judged
                                )
                            })
                            .unwrap_or_else(|| "unavailable (0 judge labels)".to_string());
                        let mut funnel = format!(
                            "Retrieval funnel (denominator: results; rolling injected precision, 30d: {precision_display})"
                        );
                        if retrieval_groups.is_empty() {
                            funnel.push_str("\n  No retrieval results recorded");
                        } else {
                            for group in &retrieval_groups {
                                funnel.push_str(&format!(
                                    "\n  {}/{} / {}: results={} -> resolved={} -> used/body-pulled={} -> helpful={}",
                                    group.document_type,
                                    group.query_family,
                                    group.ranking_policy,
                                    group.results,
                                    group.resolved_results,
                                    group.used_results,
                                    group.helpful_results,
                                ));
                            }
                        }
                        funnel
                    }
                }
            }
        };

        let decay_cycle = self
            .daemon_status()
            .await
            .map(|status| {
                format!(
                    "Memory decay (last cycle): protected={} promoted_on_access={}",
                    status.curated_entries_protected, status.promoted_on_access
                )
            })
            .unwrap_or_else(|| "Memory decay (last cycle): unavailable".to_string());

        let output = format!(
            "Cassy Statistics\n\
             ==============\n\n\
             Entries: {} ({} live, {} archived)\n\
             Tier breakdown (archived flag):\n\
             {}\n\
             {}\n\
             {}\n\
             {}\n\
             Rules: {} ({} proven)\n\
             Tasks: {} ({} open, {} in progress)\n\
             Skills: {} ({} enabled)\n\
             {}\n\
             {}",
            corpus.total,
            corpus.live,
            corpus.archived,
            corpus.tier_line(MemoryTier::InContext, 0),
            corpus.tier_line(MemoryTier::Working, 1),
            corpus.tier_line(MemoryTier::Cold, 2),
            corpus.tier_line(MemoryTier::Archive, 3),
            rules.len(),
            proven_rules,
            tasks.len(),
            open_tasks,
            in_progress,
            skills.len(),
            enabled_skills,
            decay_cycle,
            retrieval_funnel
        );

        Ok(Self::success(output))
    }

    /// Record an observation
    pub async fn cas_observe(
        &self,
        Parameters(req): Parameters<ObserveRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        let id = store.generate_id().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate ID: {e}")),
            data: None,
        })?;

        let observation_type: ObservationType = match req.observation_type.to_lowercase().as_str() {
            "decision" => ObservationType::Decision,
            "bugfix" => ObservationType::Bugfix,
            "feature" => ObservationType::Feature,
            "refactor" => ObservationType::Refactor,
            "discovery" => ObservationType::Discovery,
            _ => ObservationType::General,
        };

        let tags: Vec<String> = req
            .tags
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // Auto-detect branch for worktree scoping
        let branch = self.current_worktree_branch();

        let entry = Entry {
            source_ids: Vec::new(),
            id: id.clone(),
            scope: Scope::default(),
            entry_type: EntryType::Observation,
            observation_type: Some(observation_type),
            tags,
            created: chrono::Utc::now(),
            content: req.content,
            raw_content: None,
            compressed: false,
            memory_tier: MemoryTier::Working,
            title: None,
            helpful_count: 0,
            harmful_count: 0,
            last_accessed: None,
            archived: false,
            session_id: None,
            source_tool: req.source_tool,
            pending_extraction: true,
            pending_embedding: true,
            stability: 0.5,
            access_count: 0,
            importance: 0.5,
            valid_from: None,
            valid_until: None,
            review_after: None,
            last_reviewed: None,
            domain: None,
            // Observations start as hypotheses with medium confidence
            belief_type: BeliefType::Hypothesis,
            confidence: 0.5,
            branch,
            team_id: None,
            share: None,
        };

        store.add(&entry).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to store observation: {e}")),
            data: None,
        })?;

        if let Ok(search) = self.open_search_index() {
            let _ = search.index_entry(&entry);
        }

        Ok(Self::success(format!(
            "Recorded observation: {id} ({observation_type:?})"
        )))
    }

    // ========================================================================
    // Additional Memory Tools
    // ========================================================================

    /// Set memory tier
    pub async fn cas_set_tier(
        &self,
        Parameters(req): Parameters<MemoryTierRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        let mut entry = store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Entry not found: {e}")),
            data: None,
        })?;

        let tier = match req.tier.to_lowercase().as_str() {
            "cold" => MemoryTier::Cold,
            "archive" => MemoryTier::Archive,
            _ => MemoryTier::Working,
        };

        entry.memory_tier = tier;
        store.update(&entry).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!("Set {} to {:?} tier", req.id, tier)))
    }

    // ========================================================================
    // Additional System Tools
    // ========================================================================

    /// Run diagnostics
    pub async fn cas_doctor(&self) -> Result<CallToolResult, McpError> {
        let mut checks = Vec::new();
        let mut issues = Vec::new();
        let mut expected_index = StoreBackedIndexCounts::default();

        // Check store
        match self.open_store() {
            Ok(store) => match store.list() {
                Ok(entries) => {
                    expected_index.entries = entries.len();
                    if let Ok(recent) = store.recent(1)
                        && let Some(entry) = recent.first()
                        && let Ok(updated) = store.recent_timestamp(entry)
                    {
                        expected_index.observe(updated);
                    }
                    checks.push(format!("Store: OK ({} entries)", entries.len()));
                }
                Err(e) => issues.push(format!("Store list failed: {e}")),
            },
            Err(e) => issues.push(format!("Store open failed: {e}")),
        }

        if let Some(status) = self.daemon_status().await {
            checks.push(format!(
                "Memory decay: last cycle protected={}, promoted_on_access={}",
                status.curated_entries_protected, status.promoted_on_access
            ));
        } else {
            checks.push("Memory decay: last cycle unavailable (daemon not running)".to_string());
        }

        // Check task store
        match self.open_task_store() {
            Ok(store) => match store.list(None) {
                Ok(tasks) => {
                    expected_index.tasks = tasks.len();
                    for task in &tasks {
                        expected_index.observe(task.updated_at);
                    }
                    checks.push(format!("Task Store: OK ({} tasks)", tasks.len()));
                }
                Err(e) => issues.push(format!("Task store list failed: {e}")),
            },
            Err(e) => issues.push(format!("Task store open failed: {e}")),
        }

        // Check rule store
        match self.open_rule_store() {
            Ok(store) => match store.list() {
                Ok(rules) => {
                    expected_index.rules = rules.len();
                    for rule in &rules {
                        expected_index.observe(rule.created);
                    }
                    checks.push(format!("Rule Store: OK ({} rules)", rules.len()));
                }
                Err(e) => issues.push(format!("Rule store list failed: {e}")),
            },
            Err(e) => issues.push(format!("Rule store open failed: {e}")),
        }

        // Check skill store
        match self.open_skill_store() {
            Ok(store) => match store.list(None) {
                Ok(skills) => {
                    expected_index.skills = skills.len();
                    for skill in &skills {
                        expected_index.observe(skill.updated_at);
                    }
                    checks.push(format!("Skill Store: OK ({} skills)", skills.len()));
                }
                Err(e) => issues.push(format!("Skill store list failed: {e}")),
            },
            Err(e) => issues.push(format!("Skill store open failed: {e}")),
        }

        // Check search index
        match self.open_search_index() {
            Ok(search) => {
                let mut indexed = StoreBackedIndexCounts::default();
                let count_result = (|| -> Result<(), String> {
                    indexed.entries = search
                        .count_documents(DocType::Entry)
                        .map_err(|e| e.to_string())? as usize;
                    indexed.tasks = search
                        .count_documents(DocType::Task)
                        .map_err(|e| e.to_string())? as usize;
                    indexed.rules = search
                        .count_documents(DocType::Rule)
                        .map_err(|e| e.to_string())? as usize;
                    indexed.skills = search
                        .count_documents(DocType::Skill)
                        .map_err(|e| e.to_string())? as usize;
                    Ok(())
                })();

                match count_result {
                    Ok(())
                        if indexed.total() != expected_index.total()
                            || index_last_modified(&crate::hybrid_search::tantivy_index_dir(
                                &self.cas_root,
                            ))
                            .zip(expected_index.latest_store_update.as_ref())
                            .is_some_and(
                                |(indexed_at, store_update)| {
                                    store_update.signed_duration_since(indexed_at).num_seconds()
                                        > INDEX_FRESHNESS_TOLERANCE_SECS
                                },
                            ) =>
                    {
                        issues.push(format!(
                            "Search Index: STALE — run reindex bm25=true ({})",
                            format_index_counts(&indexed, &expected_index)
                        ));
                    }
                    Ok(()) => checks.push(format!(
                        "Search Index: OK ({})",
                        format_index_counts(&indexed, &expected_index)
                    )),
                    Err(error) => issues.push(format!(
                        "Search Index: unable to verify freshness — run reindex bm25=true ({error})"
                    )),
                }
            }
            Err(e) => issues.push(format!("Search index failed: {e}")),
        }

        let mut output = "Cassy Diagnostics\n===============\n\n".to_string();
        output.push_str("## Checks\n");
        for check in &checks {
            output.push_str(&format!("- {check}\n"));
        }

        if !issues.is_empty() {
            output.push_str("\n## Issues\n");
            for issue in &issues {
                output.push_str(&format!("- {issue}\n"));
            }
        }

        output.push_str(&format!(
            "\nStatus: {} checks passed, {} issues",
            checks.len(),
            issues.len()
        ));

        Ok(Self::success(output))
    }

    /// Build focused context for sub-agent delegation
    ///
    /// Implements sub-agent architecture pattern from context engineering.
    /// Provides clean, focused context for specialized agents.
    pub async fn cas_context_for_subagent(
        &self,
        Parameters(req): Parameters<SubAgentContextRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        // Get the task
        let task = task_store.get(&req.task_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        let mut context_parts = Vec::new();
        let mut tokens_used = 0usize;
        let max_tokens = req.max_tokens;

        // Estimate tokens helper
        let estimate = |s: &str| s.len().div_ceil(4);

        // 1. Task header with full details
        let header = format!(
            "# Task: {}\n\n**ID:** {}\n**Type:** {:?}\n**Priority:** {}\n**Status:** {:?}\n",
            task.title,
            task.id,
            task.task_type,
            task.priority.label(),
            task.status
        );
        context_parts.push(header.clone());
        tokens_used += estimate(&header);

        // 2. Task description (if exists)
        if !task.description.is_empty() {
            let desc = format!("## Description\n\n{}\n", task.description);
            if tokens_used + estimate(&desc) < max_tokens {
                context_parts.push(desc.clone());
                tokens_used += estimate(&desc);
            }
        }

        // 3. Task notes (valuable working context)
        if !task.notes.is_empty() {
            let notes = format!("## Working Notes\n\n{}\n", task.notes);
            if tokens_used + estimate(&notes) < max_tokens {
                context_parts.push(notes.clone());
                tokens_used += estimate(&notes);
            }
        }

        // 4. Dependencies (blocking tasks)
        if let Ok(deps) = task_store.get_dependencies(&task.id) {
            if !deps.is_empty() {
                let mut dep_section = "## Dependencies\n\n".to_string();
                for dep in deps.iter().take(5) {
                    if let Ok(blocking_task) = task_store.get(&dep.to_id) {
                        dep_section.push_str(&format!(
                            "- **{}** {} ({:?})\n",
                            blocking_task.id, blocking_task.title, dep.dep_type
                        ));
                    }
                }
                if tokens_used + estimate(&dep_section) < max_tokens {
                    context_parts.push(dep_section.clone());
                    tokens_used += estimate(&dep_section);
                }
            }
        }

        // 5. Related memories via semantic search (if enabled and budget allows)
        if req.include_memories && tokens_used < max_tokens - 200 {
            if let Ok(search) = self.open_search_index() {
                let opts = SearchOptions {
                    query: format!("{} {}", task.title, task.description),
                    limit: 5,
                    doc_types: vec![DocType::Entry],
                    ..Default::default()
                };

                if let Ok(results) = search.search_unified(&opts) {
                    if !results.is_empty() {
                        let store = self.open_store()?;
                        let mut memory_section = "## Related Memories\n\n".to_string();
                        for result in results.iter().take(3) {
                            if let Ok(entry) = store.get(&result.id) {
                                let preview = truncate_str(&entry.content, 100);
                                memory_section.push_str(&format!(
                                    "- **{}** {}\n",
                                    entry.id,
                                    preview.replace('\n', " ")
                                ));
                            }
                        }
                        if tokens_used + estimate(&memory_section) < max_tokens {
                            context_parts.push(memory_section.clone());
                            tokens_used += estimate(&memory_section);
                        }
                    }
                }
            }
        }

        // 6. Footer with agent instructions
        let footer = format!(
            "\n---\n*Context for sub-agent (~{} tokens). Task ID: {} for updates.*",
            tokens_used, task.id
        );
        context_parts.push(footer);

        Ok(Self::success(context_parts.join("\n")))
    }

    /// Reindex search
    pub async fn cas_reindex(
        &self,
        Parameters(req): Parameters<ReindexRequest>,
    ) -> Result<CallToolResult, McpError> {
        let mut results = Vec::new();

        if req.bm25 || !req.embeddings {
            // Rebuild BM25 index
            let store = self.open_store()?;
            let entries = store.list().unwrap_or_default();

            let task_store = self.open_task_store()?;
            let tasks = task_store.list(None).unwrap_or_default();

            let rule_store = self.open_rule_store()?;
            let rules = rule_store.list().unwrap_or_default();

            let skill_store = self.open_skill_store()?;
            let skills = skill_store.list(None).unwrap_or_default();

            let index_dir = crate::hybrid_search::tantivy_index_dir(&self.cas_root);
            // Clear existing index first
            let _ = std::fs::remove_dir_all(&index_dir);
            match SearchIndex::open(&index_dir) {
                Ok(search) => {
                    let mut indexed = 0;
                    for entry in &entries {
                        if search.index_entry(entry).is_ok() {
                            indexed += 1;
                        }
                    }
                    for task in &tasks {
                        if search.index_task(task).is_ok() {
                            indexed += 1;
                        }
                    }
                    for rule in &rules {
                        if search.index_rule(rule).is_ok() {
                            indexed += 1;
                        }
                    }
                    for skill in &skills {
                        if search.index_skill(skill).is_ok() {
                            indexed += 1;
                        }
                    }
                    let config = crate::config::Config::load(&self.cas_root).ok();
                    let configured_artifacts_root = config
                        .as_ref()
                        .and_then(|config| config.factory().artifacts_root);
                    let artifacts_root = crate::config::resolved_factory_artifacts_root(
                        configured_artifacts_root.as_deref(),
                    );
                    let artifacts = crate::hybrid_search::artifacts::discover_all_task_artifacts(
                        &artifacts_root,
                    );
                    match search.index_artifacts(&artifacts) {
                        Ok(count) => indexed += count,
                        Err(error) => {
                            tracing::warn!(error = %error, "artifact backfill indexing failed")
                        }
                    }
                    results.push(format!("BM25: Indexed {indexed} documents"));
                }
                Err(e) => results.push(format!("BM25: Failed - {e}")),
            }
        }

        if req.embeddings {
            results.push("Embeddings: Skipped (semantic search is now cloud-only)".to_string());
        }

        Ok(Self::success(format!(
            "Reindex complete:\n{}",
            results.join("\n")
        )))
    }
}
