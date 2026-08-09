use crate::cloud::CloudConfig;
use crate::mcp::tools::core::imports::*;
use crate::mcp::tools::types::{
    BlockReason, DimensionBreakdown, MemoryRememberResponse, RecommendedAction,
};

use cas_core::memory::{
    CandidateFacets, NewMemoryFacets, OverlapDecision, OverlapMatch, OverlapRecommendation,
    check_overlap, extract_facets_from_body,
};
use cas_store::Store;
use cas_types::Entry;

impl From<&OverlapMatch> for DimensionBreakdown {
    fn from(m: &OverlapMatch) -> Self {
        DimensionBreakdown {
            problem_statement: m.scores.problem_statement,
            root_cause: m.scores.root_cause,
            solution_approach: m.scores.solution_approach,
            referenced_files: m.scores.referenced_files,
            tags: m.scores.tags,
            penalty: m.scores.penalty,
            net: m.scores.net(),
        }
    }
}

impl From<OverlapRecommendation> for RecommendedAction {
    fn from(r: OverlapRecommendation) -> Self {
        match r {
            OverlapRecommendation::UpdateExisting => RecommendedAction::UpdateExisting,
            OverlapRecommendation::SurfaceForUserDecision => RecommendedAction::SurfaceForUserDecision,
        }
    }
}

/// Build a [`CallToolResult`] that carries a [`MemoryRememberResponse`] as
/// `structured_content` plus a human-readable text block. When
/// `is_error=true`, `CallToolResult::is_error` is set so agents parsing the
/// response know the tool call did not create a memory.
pub(crate) fn build_remember_result(
    response: &MemoryRememberResponse,
    text: String,
    is_error: bool,
) -> CallToolResult {
    let value = serde_json::to_value(response).unwrap_or(serde_json::Value::Null);
    CallToolResult {
        content: vec![Content::text(text)],
        structured_content: Some(value),
        is_error: Some(is_error),
        meta: None,
    }
}

/// Outcome of [`CasCore::run_overlap_check`] translated into the three
/// actions cas_remember needs to take. `None` means "no candidates /
/// nothing to do", equivalent to `LowOverlap`.
enum OverlapOutcome {
    Block {
        best: OverlapMatch,
        all_high_scoring: Vec<OverlapMatch>,
        recommendation: OverlapRecommendation,
    },
    CrossRef {
        links: Vec<OverlapMatch>,
        refresh: bool,
    },
}

/// Minimal frontmatter view used by the overlap check — mirrors the 4
/// fields that influence scoring (module, track, root_cause) plus the
/// legacy `name`/`description` if the caller embedded them.
#[derive(Default, serde::Deserialize)]
struct OverlapFrontmatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    module: Option<String>,
    #[serde(default)]
    track: Option<String>,
    #[serde(default)]
    root_cause: Option<String>,
}

fn parse_overlap_frontmatter(body: &str) -> OverlapFrontmatter {
    let trimmed = body.trim_start();
    if !trimmed.starts_with("---") {
        return OverlapFrontmatter::default();
    }
    let parts: Vec<&str> = trimmed.splitn(3, "---").collect();
    if parts.len() < 3 {
        return OverlapFrontmatter::default();
    }
    serde_yaml::from_str::<OverlapFrontmatter>(parts[1].trim()).unwrap_or_default()
}

/// Build [`NewMemoryFacets`] for the memory about to be stored. The title
/// override wins over frontmatter `name`; tags come from the request
/// (request tags are authoritative). Cross-reference tags from a previous
/// remember loop are filtered out so they don't inflate overlap scores.
fn build_new_facets(
    body: &str,
    req_title: Option<&str>,
    req_tags: &[String],
) -> (NewMemoryFacets, OverlapFrontmatter) {
    let fm = parse_overlap_frontmatter(body);
    let (body_tokens, file_refs) = extract_facets_from_body(body);
    let title = req_title
        .map(|s| s.to_string())
        .or_else(|| fm.name.clone())
        .unwrap_or_default()
        .to_lowercase();
    let description = fm.description.clone().unwrap_or_default().to_lowercase();
    let tags: std::collections::HashSet<String> = req_tags
        .iter()
        .filter(|t| !t.starts_with("related:"))
        .map(|t| t.to_lowercase())
        .collect();
    (
        NewMemoryFacets {
            title,
            description,
            module: fm.module.clone(),
            track: fm.track.clone(),
            root_cause: fm.root_cause.clone(),
            tags,
            file_refs,
            body_tokens,
        },
        fm,
    )
}

/// Build [`CandidateFacets`] for an existing entry. Structured fields come
/// from frontmatter if the entry's body carries it; tags come from
/// `entry.tags` (already split). `related_count` counts existing
/// `related:*` tags for the cap check.
/// Build a short BM25 query string from the new memory's facets. Prefers
/// reference symbols (most discriminating), then title tokens. Keeps the
/// query small so search stays fast — overlap budget is <500ms.
fn build_overlap_query(new: &NewMemoryFacets) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for r in new.file_refs.iter().take(5) {
        parts.push(r);
    }
    for t in new.title.split_whitespace().take(5) {
        if t.len() >= 3 {
            parts.push(t);
        }
    }
    parts.join(" ")
}

fn build_candidate_facets(entry: &Entry) -> CandidateFacets {
    let fm = parse_overlap_frontmatter(&entry.content);
    let (body_tokens, file_refs) = extract_facets_from_body(&entry.content);
    let tags: std::collections::HashSet<String> = entry
        .tags
        .iter()
        .filter(|t| !t.starts_with("related:"))
        .map(|t| t.to_lowercase())
        .collect();
    let related_count = entry
        .tags
        .iter()
        .filter(|t| t.starts_with("related:"))
        .count();
    let title = entry
        .title
        .clone()
        .or(fm.name)
        .unwrap_or_default()
        .to_lowercase();
    CandidateFacets {
        slug: entry.id.clone(),
        title,
        description: fm.description.unwrap_or_default().to_lowercase(),
        module: fm.module,
        track: fm.track,
        root_cause: fm.root_cause,
        tags,
        file_refs,
        body_tokens,
        related_count,
    }
}

impl CasCore {
    /// Run the pre-insert overlap check for a new memory. Returns
    /// `Some(OverlapOutcome)` when the decision requires action, or `None`
    /// when no candidates were found / the decision was LowOverlap.
    ///
    /// Errors are reported through `Result` but callers treat them as
    /// best-effort — overlap detection must never fail a legitimate write.
    fn run_overlap_check(
        &self,
        store: &dyn Store,
        content: &str,
        title: Option<&str>,
        tags: &[String],
        _new_id: &str,
    ) -> Result<Option<OverlapOutcome>, String> {
        let (new_facets, _fm) = build_new_facets(content, title, tags);

        // Fetch candidates via BM25 over the project's current memory set.
        // The same-module preference from R13 is applied at scoring time
        // via `build_candidate_facets` / `score_candidate` (mismatched
        // modules incur a -1 penalty that pushes them down). The MCP
        // server's cached search index only implements `search_for_dedup`
        // today; when a cas-cli-backed SearchIndex with
        // `search_candidates_by_module` is plumbed through, the hard
        // module filter can live here instead.
        use cas_core::SearchIndexTrait;
        let search = self
            .open_search_index()
            .map_err(|e| format!("open_search_index: {e:?}"))?;
        let query = build_overlap_query(&new_facets);
        let all = store.list().map_err(|e| format!("store.list: {e}"))?;
        let hits = search
            .search_for_dedup(&query, 5, &all)
            .map_err(|e| format!("search_for_dedup: {e}"))?;
        let candidates: Vec<Entry> = hits
            .into_iter()
            .filter_map(|h| store.get(&h.id).ok())
            .collect();

        if candidates.is_empty() {
            return Ok(None);
        }

        let cand_facets: Vec<CandidateFacets> =
            candidates.iter().map(build_candidate_facets).collect();

        // Interactive mode flag: MCP callers are agents (non-interactive)
        // unless a future request opts in. Headless default per R7.
        let interactive = false;

        match check_overlap(&new_facets, &cand_facets, interactive) {
            OverlapDecision::LowOverlap => Ok(None),
            OverlapDecision::ModerateOverlap {
                links,
                refresh_recommended,
            } => Ok(Some(OverlapOutcome::CrossRef {
                links,
                refresh: refresh_recommended,
            })),
            OverlapDecision::HighOverlap {
                best,
                all_high_scoring,
                recommendation,
            } => Ok(Some(OverlapOutcome::Block {
                best,
                all_high_scoring,
                recommendation,
            })),
        }
    }

    /// Create a new CAS service with daemon support
    pub fn with_daemon(
        cas_root: std::path::PathBuf,
        activity: Option<std::sync::Arc<ActivityTracker>>,
        daemon: Option<std::sync::Arc<EmbeddedDaemon>>,
    ) -> Self {
        Self {
            cas_root,
            activity,
            daemon,
            agent_id: std::sync::OnceLock::new(),
            agent_identity_source: std::sync::OnceLock::new(),
            peer: std::sync::Arc::new(std::sync::RwLock::new(None)),
            cached_store: std::sync::OnceLock::new(),
            cached_rule_store: std::sync::OnceLock::new(),
            cached_task_store: std::sync::OnceLock::new(),
            cached_skill_store: std::sync::OnceLock::new(),
            cached_entity_store: std::sync::OnceLock::new(),
            cached_agent_store: std::sync::OnceLock::new(),
            cached_verification_store: std::sync::OnceLock::new(),
            cached_worktree_store: std::sync::OnceLock::new(),
            cached_knowledge_store: std::sync::OnceLock::new(),
            cached_search_index: std::sync::OnceLock::new(),
            cached_config: std::sync::OnceLock::new(),
        }
    }

    /// Get the project store path
    pub fn project_path(&self) -> &std::path::Path {
        &self.cas_root
    }

    /// Pre-set the agent ID (for testing where no daemon is running)
    ///
    /// In production, the agent_id is discovered lazily via daemon socket query.
    /// In tests, there's no daemon, so we pre-set the agent_id directly.
    pub fn set_agent_id_for_testing(&self, agent_id: String) {
        self.bind_agent_identity(
            agent_id,
            crate::mcp::server::AgentIdentitySource::ServerInternal,
        )
        .expect("test identity must bind exactly once");
    }

    // ========================================================================
    // Workflow Guidance (injected on task start/claim)
    // ========================================================================

    /// Generate workflow guidance to show when starting or claiming a task
    pub(super) fn workflow_guidance() -> String {
        "\n\n📋 Workflow Guidance:\n\
         • Search: `mcp__cas__search` for exploratory queries, Grep for exact patterns\n\
         • Progress: `mcp__cas__task action: notes` to track discoveries\n\
         • Learnings: `mcp__cas__memory action: remember` for reusable knowledge"
            .to_string()
    }

    // ========================================================================
    // Memory Tools (12)
    // ========================================================================

    /// Store a new memory
    pub async fn cas_remember(
        &self,
        Parameters(req): Parameters<RememberRequest>,
    ) -> Result<CallToolResult, McpError> {
        // `mode` contract (cas-e382): Phase 1 accepts `None` and
        // `"interactive"` (treated identically). `"autofix"` is reserved
        // for Phase 2 and returns an explicit error rather than silently
        // degrading — callers should fail loudly if they ask for a feature
        // that does not exist yet.
        match req.mode.as_deref() {
            None | Some("interactive") => {}
            Some("autofix") => {
                return Err(McpError {
                    code: ErrorCode::INVALID_REQUEST,
                    message: Cow::from(
                        "mode=autofix is reserved for Phase 2 and is not supported in Phase 1. \
                         Use mode=interactive (default) and act on a Blocked response yourself.",
                    ),
                    data: None,
                });
            }
            Some(other) => {
                return Err(McpError {
                    code: ErrorCode::INVALID_REQUEST,
                    message: Cow::from(format!(
                        "unknown mode '{other}'. Valid modes: 'interactive' (default). 'autofix' is reserved for Phase 2."
                    )),
                    data: None,
                });
            }
        }

        let store = self.open_store()?;

        let entry_type: EntryType = req.entry_type.parse().unwrap_or(EntryType::Learning);
        let id = store.generate_id().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate ID: {e}")),
            data: None,
        })?;

        let mut tags: Vec<String> = req
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

        // Parse temporal validity timestamps
        let valid_from = req.valid_from.and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .ok()
        });
        let valid_until = req.valid_until.and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .ok()
        });

        // ====================================================================
        // Pre-insert overlap detection (cas-4721)
        //
        // Runs by default on every remember call. `bypass_overlap=true` skips
        // it for bulk imports and tests. The check has three outcomes:
        //   - HighOverlap  → block the insert, return an error pointing at
        //                    the existing memory (caller updates in place).
        //   - ModerateOverlap → proceed, but bidirectionally add `related:*`
        //                    tags between the new memory and its matches.
        //   - LowOverlap   → proceed normally.
        // ====================================================================
        let bypass = req.bypass_overlap.unwrap_or(false);
        let mut refresh_recommended = false;
        let mut linked_slugs: Vec<String> = Vec::new();
        if !bypass {
            match self.run_overlap_check(
                store.as_ref(),
                &req.content,
                req.title.as_deref(),
                &tags,
                &id,
            ) {
                Ok(Some(OverlapOutcome::Block { best, all_high_scoring, recommendation })) => {
                    // Build the structured Blocked response (cas-e382).
                    // The tool call returns Ok(CallToolResult) with
                    // is_error=true so agents can parse `structured_content`
                    // and decide how to proceed. Legacy text-only clients
                    // still get a readable message in the content block.
                    let other_high_scoring: Vec<String> = all_high_scoring
                        .iter()
                        .skip(1)
                        .map(|m| m.slug.clone())
                        .collect();
                    let recommendation_action = RecommendedAction::from(recommendation);
                    let response = MemoryRememberResponse::Blocked {
                        reason: BlockReason::HighOverlap,
                        existing_slug: best.slug.clone(),
                        dimension_scores: DimensionBreakdown::from(&best),
                        recommended_action: recommendation_action,
                        other_high_scoring: other_high_scoring.clone(),
                    };
                    let mut msg = format!(
                        "Overlap detected: this memory duplicates an existing entry.\n\
                         Existing slug: {}\n\
                         Score: {}/5 (problem={}, root_cause={}, solution={}, files={}, tags={}, penalty={})\n\
                         Recommendation: {}\n",
                        best.slug,
                        best.scores.net(),
                        best.scores.problem_statement,
                        best.scores.root_cause,
                        best.scores.solution_approach,
                        best.scores.referenced_files,
                        best.scores.tags,
                        best.scores.penalty,
                        match recommendation_action {
                            RecommendedAction::UpdateExisting =>
                                "update the existing memory in place",
                            RecommendedAction::SurfaceForUserDecision =>
                                "confirm with user before creating",
                        },
                    );
                    if !other_high_scoring.is_empty() {
                        msg.push_str(&format!(
                            "Other high-scoring candidates ({}): {}\n",
                            other_high_scoring.len(),
                            other_high_scoring.join(", ")
                        ));
                        msg.push_str("Consider running a refresh on this module.\n");
                    }
                    msg.push_str(
                        "To override (bulk imports / tests only), set bypass_overlap=true.",
                    );
                    return Ok(build_remember_result(&response, msg, true));
                }
                Ok(Some(OverlapOutcome::CrossRef { links, refresh })) => {
                    refresh_recommended = refresh;
                    for link in &links {
                        if !link.cap_reached {
                            tags.push(format!("related:{}", link.slug));
                            linked_slugs.push(link.slug.clone());
                            // Bidirectional: append related:<new-id> to the
                            // existing entry's tags.
                            if let Ok(mut other) = store.get(&link.slug) {
                                let marker = format!("related:{id}");
                                if !other.tags.contains(&marker) {
                                    other.tags.push(marker);
                                    let _ = store.update(&other);
                                }
                            }
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    // Overlap detection is best-effort — never fail the write
                    // because the search index hiccuped. Log and proceed.
                    tracing::warn!("overlap detection failed: {e}; proceeding with insert");
                }
            }
        }

        // ====================================================================
        // Team auto-promote (cas-6d96)
        //
        // Resolution order:
        //   1. Explicit `team_id` in request → use as-is (caller wins).
        //   2. `personal=true` → stay personal (None), skip auto-promote.
        //   3. Neither set → consult project CloudConfig.active_team_id().
        //
        // Failure to load CloudConfig is non-fatal — behaves like "no team".
        // This keeps `cas remember` working offline / in unconfigured projects.
        // ====================================================================
        let effective_team_id: Option<String> = if req.team_id.is_some() {
            req.team_id.clone()
        } else if req.personal == Some(true) {
            None
        } else {
            CloudConfig::load_from_cas_dir(&self.cas_root)
                .ok()
                .and_then(|cfg| cfg.active_team_id())
        };

        let entry = Entry {
            id: id.clone(),
            scope: Scope::default(),
            entry_type,
            observation_type: None,
            tags,
            created: chrono::Utc::now(),
            content: req.content,
            raw_content: None,
            compressed: false,
            memory_tier: MemoryTier::Working,
            title: req.title,
            helpful_count: 0,
            harmful_count: 0,
            last_accessed: None,
            archived: false,
            // Preserve author session provenance so ambient recall can avoid
            // immediately echoing a memory back into the same conversation.
            session_id: self.get_agent_id().ok(),
            source_tool: Some("mcp".to_string()),
            pending_extraction: false,
            pending_embedding: true,
            stability: 0.5,
            access_count: 0,
            importance: req.importance,
            valid_from,
            valid_until,
            review_after: None,
            last_reviewed: None,
            domain: None,
            belief_type: Default::default(),
            confidence: 1.0,
            branch,
            team_id: effective_team_id,
            share: None,
        };

        store.add(&entry).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to store entry: {e}")),
            data: None,
        })?;

        if let Ok(search) = self.open_search_index() {
            let _ = search.index_entry(&entry);
        }

        let mut msg = format!("Created entry: {id}");
        if !linked_slugs.is_empty() {
            msg.push_str(&format!(
                "\nCross-referenced with: {}",
                linked_slugs.join(", ")
            ));
        }
        if refresh_recommended {
            msg.push_str(
                "\nNote: one or more candidates are at the cross-reference cap — \
                 consider running a memory refresh on this module.",
            );
        }
        let response = MemoryRememberResponse::Created {
            slug: id.clone(),
            related_memories: linked_slugs,
            refresh_recommended,
        };
        Ok(build_remember_result(&response, msg, false))
    }

    /// Get an entry by ID
    ///
    /// Also tracks access for session-aware context boosting:
    /// - Updates `last_accessed` timestamp
    /// - Increments `access_count`
    /// - Reinforces memory stability
    pub async fn cas_get(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        let mut entry = store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Entry not found: {e}")),
            data: None,
        })?;

        // Track access for session-aware context boosting
        // reinforce() updates last_accessed, access_count, and stability
        entry.reinforce();

        // Persist access tracking (best-effort, don't fail the get)
        let _ = store.update(&entry);

        let output = format!(
            "ID: {}\nType: {:?}\nTags: {}\nCreated: {}\nImportance: {:.2}\nStability: {:.2}\nFeedback: +{} -{}\n\n{}",
            entry.id,
            entry.entry_type,
            if entry.tags.is_empty() {
                "none".to_string()
            } else {
                entry.tags.join(", ")
            },
            entry.created.format("%Y-%m-%d %H:%M"),
            entry.importance,
            entry.stability,
            entry.helpful_count,
            entry.harmful_count,
            entry.content
        );

        Ok(Self::success(output))
    }

    /// Mark entry as helpful
    pub async fn cas_helpful(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        let mut entry = store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Entry not found: {e}")),
            data: None,
        })?;

        entry.helpful_count += 1;
        entry.reinforce();
        entry.last_reviewed = Some(chrono::Utc::now());

        store.update(&entry).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!(
            "Marked {} as helpful (score: {:.2})",
            req.id,
            entry.feedback_score()
        )))
    }

    /// Mark entry as harmful
    pub async fn cas_harmful(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        let mut entry = store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Entry not found: {e}")),
            data: None,
        })?;

        entry.harmful_count += 1;

        store.update(&entry).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!(
            "Marked {} as harmful (score: {:.2})",
            req.id,
            entry.feedback_score()
        )))
    }

    /// Mark entry as reviewed (sets last_reviewed timestamp)
    pub async fn cas_mark_reviewed(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        let mut entry = store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Entry not found: {e}")),
            data: None,
        })?;

        entry.last_reviewed = Some(chrono::Utc::now());

        store.update(&entry).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!("Marked {} as reviewed", req.id)))
    }

    /// List recent entries
    pub async fn cas_recent(
        &self,
        Parameters(req): Parameters<RecentRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        // Fetch more to account for branch filtering
        let all_entries = store.recent(req.n * 3).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to get recent: {e}")),
            data: None,
        })?;

        // Filter by branch context (worktree scoping)
        let current_branch = self.current_worktree_branch();
        let entries: Vec<_> = all_entries
            .into_iter()
            .filter(|e| {
                match (&current_branch, &e.branch) {
                    // In a worktree: show entries from this branch or unscoped entries
                    (Some(cb), Some(eb)) => cb == eb,
                    (Some(_), None) => true, // Unscoped entries visible in all worktrees
                    // Not in a worktree: show all entries
                    (None, _) => true,
                }
            })
            .take(req.n)
            .collect();

        if entries.is_empty() {
            return Ok(Self::success("No entries found"));
        }

        let mut output = format!("Recent entries ({}):\n\n", entries.len());
        for entry in entries {
            let recent_at = store.recent_timestamp(&entry).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to get recent timestamp: {e}")),
                data: None,
            })?;
            let branch_indicator = entry
                .branch
                .as_ref()
                .map(|b| format!(" [{b}]"))
                .unwrap_or_default();
            output.push_str(&format!(
                "- [{}] {}{} {}\n",
                entry.id,
                recent_at.format("%Y-%m-%d %H:%M"),
                branch_indicator,
                entry.preview(50)
            ));
        }

        Ok(Self::success(output))
    }

    /// Delete an entry
    pub async fn cas_delete(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        // Verify entry exists
        store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Entry not found: {e}")),
            data: None,
        })?;

        store.delete(&req.id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to delete: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!("Deleted entry: {}", req.id)))
    }

    /// List all entries
    pub async fn cas_list(
        &self,
        Parameters(req): Parameters<LimitRequest>,
    ) -> Result<CallToolResult, McpError> {
        use cas_types::EntrySortOptions;

        let store = self.open_store()?;

        let all_entries = store.list().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list: {e}")),
            data: None,
        })?;

        // Filter by branch context (worktree scoping)
        let current_branch = self.current_worktree_branch();
        let mut entries: Vec<_> = all_entries
            .into_iter()
            .filter(|e| {
                match (&current_branch, &e.branch) {
                    // In a worktree: show entries from this branch or unscoped entries
                    (Some(cb), Some(eb)) => cb == eb,
                    (Some(_), None) => true, // Unscoped entries visible in all worktrees
                    // Not in a worktree: show all entries
                    (None, _) => true,
                }
            })
            .collect();

        // Filter by team_id if specified
        if let Some(ref team_id) = req.team_id {
            entries.retain(|e| e.team_id.as_ref() == Some(team_id));
        }

        // Apply sorting
        let sort_opts =
            EntrySortOptions::from_params(req.sort.as_deref(), req.sort_order.as_deref());

        use cas_types::{EntrySortField, SortOrder};
        match sort_opts.field {
            EntrySortField::Created => {
                entries.sort_by(|a, b| match sort_opts.order {
                    SortOrder::Asc => a.created.cmp(&b.created),
                    SortOrder::Desc => b.created.cmp(&a.created),
                });
            }
            EntrySortField::Updated => {
                // Use last_accessed as "updated" time, falling back to created
                entries.sort_by(|a, b| {
                    let a_time = a.last_accessed.as_ref().unwrap_or(&a.created);
                    let b_time = b.last_accessed.as_ref().unwrap_or(&b.created);
                    match sort_opts.order {
                        SortOrder::Asc => a_time.cmp(b_time),
                        SortOrder::Desc => b_time.cmp(a_time),
                    }
                });
            }
            EntrySortField::Importance => {
                entries.sort_by(|a, b| {
                    let cmp = a.importance.total_cmp(&b.importance);
                    match sort_opts.order {
                        SortOrder::Asc => cmp,
                        SortOrder::Desc => cmp.reverse(),
                    }
                });
            }
            EntrySortField::Title => {
                entries.sort_by(|a, b| {
                    let a_title = a.title.as_deref().unwrap_or("");
                    let b_title = b.title.as_deref().unwrap_or("");
                    match sort_opts.order {
                        SortOrder::Asc => a_title.cmp(b_title),
                        SortOrder::Desc => b_title.cmp(a_title),
                    }
                });
            }
        }

        if entries.is_empty() {
            return Ok(Self::success("No entries found"));
        }

        let limit = req.limit.unwrap_or(20);
        let mut output = format!(
            "Entries ({} total, showing {}):\n\n",
            entries.len(),
            entries.len().min(limit)
        );
        for entry in entries.iter().take(limit) {
            let branch_indicator = entry
                .branch
                .as_ref()
                .map(|b| format!(" [{b}]"))
                .unwrap_or_default();
            output.push_str(&format!(
                "- [{}] {:?} {}{} - {}\n",
                entry.id,
                entry.entry_type,
                entry.created.format("%Y-%m-%d"),
                branch_indicator,
                entry.preview(40)
            ));
        }

        if entries.len() > limit {
            output.push_str(&format!("\n... and {} more", entries.len() - limit));
        }

        Ok(Self::success(output))
    }

    /// Archive an entry
    pub async fn cas_archive(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        let mut entry = store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Entry not found: {e}")),
            data: None,
        })?;

        entry.archived = true;
        store.update(&entry).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to archive: {e}")),
            data: None,
        })?;

        // Remove from search index
        if let Ok(search) = self.open_search_index() {
            let _ = search.delete(&req.id);
        }

        Ok(Self::success(format!("Archived entry: {}", req.id)))
    }

    /// Unarchive an entry
    pub async fn cas_unarchive(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        // Try to get from archived entries
        let archived = store.list_archived().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list archived: {e}")),
            data: None,
        })?;

        let mut entry = archived
            .into_iter()
            .find(|e| e.id == req.id)
            .ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Archived entry not found: {}", req.id)),
                data: None,
            })?;

        entry.archived = false;
        store.update(&entry).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to unarchive: {e}")),
            data: None,
        })?;

        // Re-add to search index
        if let Ok(search) = self.open_search_index() {
            let _ = search.index_entry(&entry);
        }

        // Note: Vector embedding will be regenerated by the daemon

        Ok(Self::success(format!("Restored entry: {}", req.id)))
    }

    /// Update an entry
    pub async fn cas_update(
        &self,
        Parameters(req): Parameters<EntryUpdateRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_store()?;

        let mut entry = store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Entry not found: {e}")),
            data: None,
        })?;

        let mut changes = Vec::new();

        if let Some(content) = req.content {
            entry.content = content;
            changes.push("content");
        }

        if let Some(tags) = req.tags {
            entry.tags = tags
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            changes.push("tags");
        }

        if let Some(importance) = req.importance {
            entry.importance = importance.clamp(0.0, 1.0);
            changes.push("importance");
        }

        if changes.is_empty() {
            return Ok(Self::success("No changes specified"));
        }

        store.update(&entry).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!(
            "Updated {}: {}",
            req.id,
            changes.join(", ")
        )))
    }
}
