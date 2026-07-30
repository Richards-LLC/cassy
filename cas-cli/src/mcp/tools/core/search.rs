use crate::mcp::tools::core::imports::*;
use chrono::Utc;
use serde::Serialize;

#[derive(Debug, Serialize)]
struct SearchEnvelopeV1 {
    version: u8,
    schema: &'static str,
    ranking_policy: &'static str,
    query_id: String,
    query_family: String,
    hits: Vec<SearchHitV1>,
}

#[derive(Debug, Serialize)]
struct SearchHitV1 {
    rank: usize,
    id: String,
    document_type: String,
    preview: String,
    provenance: SearchProvenanceV1,
}

#[derive(Debug, Serialize)]
struct SearchProvenanceV1 {
    source: SearchSourceV1,
    scope: String,
    scores: SearchScoresV1,
    rationale: Vec<&'static str>,
    freshness: SearchFreshnessV1,
    conflict: bool,
    signals: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SearchSourceV1 {
    index: &'static str,
    origin: Option<String>,
}

#[derive(Debug, Serialize)]
struct SearchScoresV1 {
    final_score: f64,
    bm25_score: f64,
    boosted_score: f64,
}

#[derive(Debug, Default, Serialize)]
struct SearchFreshnessV1 {
    created_at: Option<String>,
    updated_at: Option<String>,
    valid_until: Option<String>,
    review_after: Option<String>,
    stale: bool,
}

#[derive(Debug, Default)]
struct HitMetadata {
    origin: Option<String>,
    scope: String,
    freshness: SearchFreshnessV1,
    conflict: bool,
    signals: Vec<String>,
}

fn query_family(query: &str, document_filter: Option<&str>) -> String {
    let trimmed = query.trim();
    let parsed = crate::hybrid_search::filter_grammar::parse_filter_query(trimmed);
    if document_filter
        .map(|value| value.starts_with("code") || value == "symbol" || value == "file")
        .unwrap_or(false)
    {
        "code".to_string()
    } else if !crate::hybrid_search::extract_id_patterns(&parsed.residual)
        .0
        .is_empty()
    {
        "id_lookup".to_string()
    } else if trimmed.split_whitespace().any(|token| token.contains(':')) {
        "filtered".to_string()
    } else if trimmed.ends_with('?')
        || trimmed
            .split_whitespace()
            .next()
            .map(|word| {
                matches!(
                    word.to_ascii_lowercase().as_str(),
                    "what" | "why" | "when" | "where" | "who" | "how"
                )
            })
            .unwrap_or(false)
    {
        "question".to_string()
    } else {
        "keyword".to_string()
    }
}

impl CasCore {
    // ========================================================================
    // Search Tools (1) - Use doc_type param for filtering
    // ========================================================================

    /// Unified search
    pub async fn cas_search(
        &self,
        Parameters(req): Parameters<SearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.cas_search_with_provenance(Parameters(req), None, None)
            .await
    }

    /// Consolidated-search extension point for opt-in versioned provenance.
    ///
    /// Kept separate from `SearchRequest` so the legacy public request type
    /// and direct `cas_search` clients remain source- and shape-compatible.
    pub(crate) async fn cas_search_with_provenance(
        &self,
        Parameters(req): Parameters<SearchRequest>,
        provenance_version: Option<usize>,
        session_id: Option<String>,
    ) -> Result<CallToolResult, McpError> {
        use crate::types::ScopeFilter;

        if let Some(version) = provenance_version
            && version != 1
        {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("unsupported provenance_version {version}; expected 1"),
            ));
        }
        let include_provenance = provenance_version.is_some();
        let query_family = query_family(&req.query, req.doc_type.as_deref());

        let search = self.open_search_index()?;

        let doc_types = req
            .doc_type
            .as_ref()
            .and_then(|t| match t.to_lowercase().as_str() {
                "entry" | "entries" | "memory" | "memories" => Some(vec![DocType::Entry]),
                "task" | "tasks" => Some(vec![DocType::Task]),
                "rule" | "rules" => Some(vec![DocType::Rule]),
                "skill" | "skills" => Some(vec![DocType::Skill]),
                "code" | "code_symbol" | "symbol" | "symbols" => Some(vec![DocType::CodeSymbol]),
                "code_file" | "file" | "files" => Some(vec![DocType::CodeFile]),
                _ => None,
            })
            .unwrap_or_default();

        // Parse scope filter
        let scope_filter: ScopeFilter = match req.scope.to_lowercase().as_str() {
            "global" => ScopeFilter::Global,
            "project" => ScopeFilter::Project,
            _ => ScopeFilter::All,
        };

        // Parse tags filter (comma-separated, case-insensitive matching)
        let tags_filter: Vec<String> = req
            .tags
            .as_ref()
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // Helper to check if item tags match the filter (all filter tags must be present)
        let matches_tags = |item_tags: &[String]| -> bool {
            if tags_filter.is_empty() {
                return true;
            }
            let item_tags_lower: Vec<String> = item_tags.iter().map(|t| t.to_lowercase()).collect();
            tags_filter
                .iter()
                .all(|filter_tag| item_tags_lower.iter().any(|t| t.contains(filter_tag)))
        };

        let opts = SearchOptions {
            query: req.query.clone(),
            limit: req.limit * 2, // Fetch more to account for scope filtering
            doc_types,
            ..Default::default()
        };

        let results = search.search_unified(&opts).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Search failed: {e}")),
            data: None,
        })?;

        if results.is_empty() && !include_provenance {
            return Ok(Self::success("No results found"));
        }

        let store = self.open_store().ok();
        let task_store = self.open_task_store().ok();
        let rule_store = self.open_rule_store().ok();
        let skill_store = self.open_skill_store().ok();
        let code_store = crate::store::open_code_store(&self.cas_root).ok();

        let mut output = format!("Search results for \"{}\":\n\n", req.query);
        let mut count = 0;
        let mut provenance_hits = Vec::new();

        // Track seen qualified_names for code symbol deduplication
        let mut seen_qualified_names = std::collections::HashSet::new();

        for result in results.iter() {
            if count >= req.limit {
                break;
            }

            // Get preview and check scope/tags filters
            let (preview, matches_filters, metadata) = match result.doc_type {
                DocType::Entry => {
                    if let Some(ref s) = store {
                        if let Ok(e) = s.get(&result.id) {
                            let scope_ok = scope_filter == ScopeFilter::All
                                || (scope_filter == ScopeFilter::Global
                                    && e.scope == Scope::Global)
                                || (scope_filter == ScopeFilter::Project
                                    && e.scope == Scope::Project);
                            let tags_ok = matches_tags(&e.tags);
                            let now = Utc::now();
                            let stale = e.valid_until.map(|value| value < now).unwrap_or(false)
                                || e.review_after.map(|value| value <= now).unwrap_or(false);
                            let conflict = e.harmful_count > e.helpful_count && e.harmful_count > 0;
                            let mut signals = Vec::new();
                            if e.valid_until.map(|value| value < now).unwrap_or(false) {
                                signals.push("expired".to_string());
                            }
                            if e.review_after.map(|value| value <= now).unwrap_or(false) {
                                signals.push("review_due".to_string());
                            }
                            if conflict {
                                signals.push("negative_feedback_conflict".to_string());
                            }
                            (
                                format!("[Entry] {}", e.preview(60)),
                                scope_ok && tags_ok,
                                HitMetadata {
                                    origin: e.source_tool.clone(),
                                    scope: e.scope.to_string(),
                                    freshness: SearchFreshnessV1 {
                                        created_at: Some(e.created.to_rfc3339()),
                                        updated_at: None,
                                        valid_until: e.valid_until.map(|value| value.to_rfc3339()),
                                        review_after: e
                                            .review_after
                                            .map(|value| value.to_rfc3339()),
                                        stale,
                                    },
                                    conflict,
                                    signals,
                                },
                            )
                        } else {
                            (
                                format!("[Entry] {}", result.id),
                                tags_filter.is_empty(),
                                HitMetadata {
                                    scope: "unknown".to_string(),
                                    ..Default::default()
                                },
                            )
                        }
                    } else {
                        (
                            format!("[Entry] {}", result.id),
                            tags_filter.is_empty(),
                            HitMetadata {
                                scope: "unknown".to_string(),
                                ..Default::default()
                            },
                        )
                    }
                }
                DocType::Task => {
                    // Tasks are always project-scoped, have labels not tags
                    let scope_ok = scope_filter != ScopeFilter::Global;
                    // Skip tasks if tags filter specified (tasks use labels, not tags)
                    let tags_ok = tags_filter.is_empty();
                    if let Some(ref s) = task_store {
                        if let Ok(t) = s.get(&result.id) {
                            let type_label = if t.task_type == TaskType::Epic {
                                "Epic"
                            } else {
                                "Task"
                            };
                            let conflict = t.status == TaskStatus::Blocked;
                            (
                                format!(
                                    "[{}] P{} {:?} {}",
                                    type_label, t.priority.0, t.status, t.title
                                ),
                                scope_ok && tags_ok,
                                HitMetadata {
                                    origin: None,
                                    scope: t.scope.to_string(),
                                    freshness: SearchFreshnessV1 {
                                        created_at: Some(t.created_at.to_rfc3339()),
                                        updated_at: Some(t.updated_at.to_rfc3339()),
                                        ..Default::default()
                                    },
                                    conflict,
                                    signals: if conflict {
                                        vec!["blocked".to_string()]
                                    } else {
                                        Vec::new()
                                    },
                                },
                            )
                        } else {
                            (
                                format!("[Task] {}", result.id),
                                scope_ok && tags_ok,
                                HitMetadata {
                                    scope: "project".to_string(),
                                    ..Default::default()
                                },
                            )
                        }
                    } else {
                        (
                            format!("[Task] {}", result.id),
                            scope_ok && tags_ok,
                            HitMetadata {
                                scope: "project".to_string(),
                                ..Default::default()
                            },
                        )
                    }
                }
                DocType::Rule => {
                    if let Some(ref s) = rule_store {
                        if let Ok(r) = s.get(&result.id) {
                            let scope_ok = scope_filter == ScopeFilter::All
                                || (scope_filter == ScopeFilter::Global
                                    && r.scope == Scope::Global)
                                || (scope_filter == ScopeFilter::Project
                                    && r.scope == Scope::Project);
                            let tags_ok = matches_tags(&r.tags);
                            let now = Utc::now();
                            let status_stale = matches!(
                                r.status,
                                cas_types::RuleStatus::Stale | cas_types::RuleStatus::Retired
                            );
                            let review_due =
                                r.review_after.map(|value| value <= now).unwrap_or(false);
                            let conflict = r.harmful_count > r.helpful_count && r.harmful_count > 0;
                            let mut signals = Vec::new();
                            if status_stale {
                                signals.push(format!("status_{}", r.status));
                            }
                            if review_due {
                                signals.push("review_due".to_string());
                            }
                            if conflict {
                                signals.push("negative_feedback_conflict".to_string());
                            }
                            (
                                format!("[Rule] {:?} {}", r.status, r.preview(50)),
                                scope_ok && tags_ok,
                                HitMetadata {
                                    origin: if r.source_ids.is_empty() {
                                        None
                                    } else {
                                        Some("derived_memory".to_string())
                                    },
                                    scope: r.scope.to_string(),
                                    freshness: SearchFreshnessV1 {
                                        created_at: Some(r.created.to_rfc3339()),
                                        review_after: r
                                            .review_after
                                            .map(|value| value.to_rfc3339()),
                                        stale: status_stale || review_due,
                                        ..Default::default()
                                    },
                                    conflict,
                                    signals,
                                },
                            )
                        } else {
                            (
                                format!("[Rule] {}", result.id),
                                tags_filter.is_empty(),
                                HitMetadata {
                                    scope: "unknown".to_string(),
                                    ..Default::default()
                                },
                            )
                        }
                    } else {
                        (
                            format!("[Rule] {}", result.id),
                            tags_filter.is_empty(),
                            HitMetadata {
                                scope: "unknown".to_string(),
                                ..Default::default()
                            },
                        )
                    }
                }
                DocType::Skill => {
                    if let Some(ref s) = skill_store {
                        if let Ok(skill) = s.get(&result.id) {
                            let scope_ok = scope_filter == ScopeFilter::All
                                || (scope_filter == ScopeFilter::Global
                                    && skill.scope == Scope::Global)
                                || (scope_filter == ScopeFilter::Project
                                    && skill.scope == Scope::Project);
                            let tags_ok = matches_tags(&skill.tags);
                            let disabled = skill.status == cas_types::SkillStatus::Disabled;
                            (
                                format!("[Skill] {:?} {}", skill.status, skill.name),
                                scope_ok && tags_ok,
                                HitMetadata {
                                    origin: None,
                                    scope: skill.scope.to_string(),
                                    freshness: SearchFreshnessV1 {
                                        created_at: Some(skill.created_at.to_rfc3339()),
                                        updated_at: Some(skill.updated_at.to_rfc3339()),
                                        stale: disabled,
                                        ..Default::default()
                                    },
                                    conflict: false,
                                    signals: if disabled {
                                        vec!["disabled".to_string()]
                                    } else {
                                        Vec::new()
                                    },
                                },
                            )
                        } else {
                            (
                                format!("[Skill] {}", result.id),
                                tags_filter.is_empty(),
                                HitMetadata {
                                    scope: "unknown".to_string(),
                                    ..Default::default()
                                },
                            )
                        }
                    } else {
                        (
                            format!("[Skill] {}", result.id),
                            tags_filter.is_empty(),
                            HitMetadata {
                                scope: "unknown".to_string(),
                                ..Default::default()
                            },
                        )
                    }
                }
                DocType::CodeSymbol => {
                    // Code symbols are project-scoped, no tags
                    let scope_ok = scope_filter != ScopeFilter::Global;
                    let tags_ok = tags_filter.is_empty();
                    if let Some(ref code_store) = code_store {
                        if let Ok(sym) = code_store.get_symbol(&result.id) {
                            // Deduplicate by qualified_name (same symbol indexed from different paths)
                            if !seen_qualified_names.insert(sym.qualified_name.clone()) {
                                continue; // Skip duplicate
                            }
                            (
                                format!(
                                    "[Code] {:?} {} in {}",
                                    sym.kind, sym.qualified_name, sym.file_path
                                ),
                                scope_ok && tags_ok,
                                HitMetadata {
                                    origin: Some("code_index".to_string()),
                                    scope: "project".to_string(),
                                    ..Default::default()
                                },
                            )
                        } else {
                            (
                                format!("[Code] {}", result.id),
                                scope_ok && tags_ok,
                                HitMetadata {
                                    origin: Some("code_index".to_string()),
                                    scope: "project".to_string(),
                                    ..Default::default()
                                },
                            )
                        }
                    } else {
                        (
                            format!("[Code] {}", result.id),
                            scope_ok && tags_ok,
                            HitMetadata {
                                origin: Some("code_index".to_string()),
                                scope: "project".to_string(),
                                ..Default::default()
                            },
                        )
                    }
                }
                DocType::CodeFile => {
                    // Code files are project-scoped, no tags
                    let scope_ok = scope_filter != ScopeFilter::Global;
                    let tags_ok = tags_filter.is_empty();
                    (
                        format!("[File] {}", result.id),
                        scope_ok && tags_ok,
                        HitMetadata {
                            origin: Some("code_index".to_string()),
                            scope: "project".to_string(),
                            ..Default::default()
                        },
                    )
                }
                DocType::Spec => {
                    // Specs are project-scoped
                    let scope_ok = scope_filter != ScopeFilter::Global;
                    let tags_ok = tags_filter.is_empty();
                    (
                        format!("[Spec] {}", result.id),
                        scope_ok && tags_ok,
                        HitMetadata {
                            scope: "project".to_string(),
                            ..Default::default()
                        },
                    )
                }
            };

            if matches_filters {
                count += 1;
                provenance_hits.push(SearchHitV1 {
                    rank: count - 1,
                    id: result.id.clone(),
                    document_type: result.doc_type.as_str().to_string(),
                    preview: preview.clone(),
                    provenance: SearchProvenanceV1 {
                        source: SearchSourceV1 {
                            index: "tantivy_unified_v1",
                            origin: metadata.origin,
                        },
                        scope: metadata.scope,
                        scores: SearchScoresV1 {
                            final_score: result.score,
                            bm25_score: result.bm25_score,
                            boosted_score: result.boosted_score,
                        },
                        rationale: vec![
                            "current_default_ranking",
                            "bm25_component",
                            "provenance_observational_only",
                        ],
                        freshness: metadata.freshness,
                        conflict: metadata.conflict,
                        signals: metadata.signals,
                    },
                });
                output.push_str(&format!(
                    "{}. {} (score: {:.2})\n   ID: {}\n\n",
                    count, preview, result.score, result.id
                ));
            }
        }

        if count == 0 && !include_provenance {
            return Ok(Self::success("No results found"));
        }

        if include_provenance {
            use cas_store::{
                DEFAULT_RETRIEVAL_POLICY, RetrievalHitIdentity, RetrievalStore,
                SqliteRetrievalStore,
            };

            let query_id = format!("qry-{}", uuid::Uuid::new_v4().simple());
            let feedback_session_id = session_id.or_else(|| std::env::var("CAS_SESSION_ID").ok());
            let identities = provenance_hits
                .iter()
                .map(|hit| RetrievalHitIdentity {
                    result_id: hit.id.clone(),
                    document_type: hit.document_type.clone(),
                    rank: hit.rank,
                })
                .collect::<Vec<_>>();
            let retrieval_store = SqliteRetrievalStore::open(&self.cas_root).map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("failed to open retrieval feedback store: {error}"),
                )
            })?;
            retrieval_store
                .record_query(
                    &query_id,
                    &req.query,
                    &query_family,
                    DEFAULT_RETRIEVAL_POLICY,
                    feedback_session_id.as_deref(),
                    &identities,
                )
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("failed to persist retrieval identity: {error}"),
                    )
                })?;
            let envelope = SearchEnvelopeV1 {
                version: 1,
                schema: "cas.retrieval.provenance.v1",
                ranking_policy: DEFAULT_RETRIEVAL_POLICY,
                query_id,
                query_family,
                hits: provenance_hits,
            };
            return Ok(Self::success(serde_json::to_string(&envelope).map_err(
                |error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()),
            )?));
        }

        Ok(Self::success(format!("{output}Found {count} results")))
    }
}
