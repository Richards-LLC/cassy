use crate::mcp::tools::service::imports::*;

impl CasService {
    pub(in crate::mcp::tools::service) async fn search_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::SearchRequest;
        let provenance_version = req.provenance_version;
        let session_id = req.session_id;
        let inner_req = SearchRequest {
            query: req
                .query
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "query required"))?,
            limit: req.limit.unwrap_or(10),
            doc_type: req.doc_type,
            scope: req.scope.unwrap_or_else(|| "all".to_string()),
            tags: req.tags,
        };
        self.inner
            .cas_search_with_provenance(Parameters(inner_req), provenance_version, session_id)
            .await
    }

    pub(in crate::mcp::tools::service) async fn retrieval_feedback_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        use cas_store::{RetrievalOutcome, RetrievalStore, SqliteRetrievalStore};
        use std::str::FromStr;

        let query_id = req
            .query_id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "query_id required"))?;
        let result_id = req
            .result_id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "result_id required"))?;
        let outcome = RetrievalOutcome::from_str(
            req.outcome
                .as_deref()
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "outcome required"))?,
        )
        .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
        let actor_id = req
            .actor_id
            .or_else(|| std::env::var("CAS_AGENT_ID").ok())
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "actor_id required"))?;
        let session_id = req
            .session_id
            .or_else(|| std::env::var("CAS_SESSION_ID").ok())
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "session_id required"))?;
        let store = SqliteRetrievalStore::open(&self.inner.cas_root)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let event_id = format!("out-{}", uuid::Uuid::new_v4().simple());
        let event = store
            .record_outcome(
                &event_id,
                &query_id,
                &result_id,
                outcome,
                &actor_id,
                &session_id,
                req.correction_ref.as_deref(),
            )
            .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
        let response = serde_json::json!({
            "version": 1,
            "event_id": event.id,
            "query_id": event.query_id,
            "result_id": event.result_id,
            "outcome": event.outcome,
            "attribution": event.attribution,
            "created_at": event.created_at,
        });
        Ok(Self::success(response.to_string()))
    }

    pub(in crate::mcp::tools::service) async fn retrieval_metrics_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        use cas_store::SqliteRetrievalStore;

        if let Some(parameter) = Self::retrieval_metrics_unsupported_parameter(&req) {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("retrieval_metrics does not support parameter '{parameter}'"),
            ));
        }
        if req
            .session_id
            .as_deref()
            .is_some_and(|session_id| session_id.trim().is_empty())
        {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "session_id cannot be empty",
            ));
        }

        let store = SqliteRetrievalStore::open(&self.inner.cas_root)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let session_id = req.session_id.as_deref();
        let aggregates = match session_id {
            Some(session_id) => store.aggregate_for_session(session_id),
            None => store.aggregate(),
        }
        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let precision = match session_id {
            Some(session_id) => store.rolling_injected_precision_for_session(30, session_id),
            None => store.rolling_injected_precision(30),
        }
        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let funnel = match session_id {
            Some(session_id) => store.evidence_funnel_for_session(session_id),
            None => store.evidence_funnel(),
        }
        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let session_scope = Self::retrieval_metrics_session_scope(
            &self.inner.cas_root,
            session_id,
            funnel.retrieved > 0,
        );
        let judge_measurement = Self::retrieval_metrics_judge_measurement(
            &self.inner.cas_root,
            precision.judged,
            session_id.is_some(),
        );
        Ok(Self::success(
            serde_json::json!({
                "version": 1,
                "groups": aggregates,
                "session_scope": session_scope,
                "retrieval_funnel": funnel,
                "retrieval_funnel_definitions": {
                    "retrieved": "distinct query/result rows returned by retrieval",
                    "injected": "retrieved rows placed in SessionStart or ambient context",
                    "opened": "body pulled after injection; opening does not prove use",
                    "used": "explicit caller feedback marked used; opening does not imply this",
                    "judged_helpful": "helpful label from the relevance judge; use does not imply this",
                },
                "judge_measurement": judge_measurement,
                // Keep the scalar easy to consume while publishing the
                // numerator/denominator beside it. `null` is intentional
                // until a judge has produced at least one label.
                "rolling_injected_precision": precision.precision,
                "injected_precision": precision.precision,
                "injected_precision_numerator": precision.helpful,
                "injected_precision_denominator": precision.judged,
                "injected_precision_window_days": precision.window_days,
                "injected_precision_stats": precision,
            })
            .to_string(),
        ))
    }

    fn retrieval_metrics_session_scope(
        cas_root: &std::path::Path,
        session_id: Option<&str>,
        has_retrieval_data: bool,
    ) -> serde_json::Value {
        use cas_store::{AgentStore, SqliteAgentStore};

        let Some(requested) = session_id else {
            return serde_json::json!({
                "filter": "all",
                "identity_kind": "all_sessions",
                "status": "available",
                "strict": false,
            });
        };

        let agents = match SqliteAgentStore::open(cas_root).and_then(|store| store.list(None)) {
            Ok(agents) => agents,
            Err(error) => {
                return serde_json::json!({
                    "filter": "strict",
                    "requested_session_id": requested,
                    "identity_kind": if has_retrieval_data {
                        "stored_retrieval_session"
                    } else {
                        "unresolved"
                    },
                    "status": if has_retrieval_data { "available" } else { "unavailable" },
                    "strict": true,
                    "reason": if has_retrieval_data {
                        "stored_retrieval_evidence"
                    } else {
                        "agent_registry_unavailable"
                    },
                    "detail": error.to_string(),
                });
            }
        };

        if agents
            .iter()
            .any(|agent| agent.id == requested || agent.cc_session_id.as_deref() == Some(requested))
        {
            return serde_json::json!({
                "filter": "strict",
                "requested_session_id": requested,
                "identity_kind": "agent_session",
                "status": if has_retrieval_data { "available" } else { "valid_empty" },
                "strict": true,
                "reason": if has_retrieval_data {
                    "stored_retrieval_evidence"
                } else {
                    "registered_agent_has_no_retrieval_results"
                },
            });
        }

        // Retrieval rows are durable beyond the live agent registry. Their
        // presence is sufficient to recognize a historical canonical token.
        if has_retrieval_data {
            return serde_json::json!({
                "filter": "strict",
                "requested_session_id": requested,
                "identity_kind": "stored_retrieval_session",
                "status": "available",
                "strict": true,
                "reason": "stored_retrieval_evidence",
            });
        }

        let matching_names: Vec<_> = agents
            .iter()
            .filter(|agent| agent.name == requested)
            .collect();
        if !matching_names.is_empty() {
            let canonical_session_id =
                (matching_names.len() == 1).then(|| matching_names[0].id.clone());
            return serde_json::json!({
                "filter": "strict",
                "requested_session_id": requested,
                "identity_kind": "agent_name",
                "status": "invalid_identity_kind",
                "strict": true,
                "reason": "agent_name_is_not_a_session_id",
                "canonical_session_id": canonical_session_id,
                "matching_agent_sessions": matching_names.len(),
            });
        }

        let matching_factory_sessions = agents
            .iter()
            .filter(|agent| agent.factory_session.as_deref() == Some(requested))
            .count();
        if matching_factory_sessions > 0 {
            return serde_json::json!({
                "filter": "strict",
                "requested_session_id": requested,
                "identity_kind": "factory_session",
                "status": "invalid_identity_kind",
                "strict": true,
                "reason": "factory_session_is_not_an_agent_session_id",
                "matching_agent_sessions": matching_factory_sessions,
                "next_action": "query one agent id/CAS_SESSION_ID; this filter never widens to every factory member",
            });
        }

        serde_json::json!({
            "filter": "strict",
            "requested_session_id": requested,
            "identity_kind": "unknown",
            "status": "unknown",
            "strict": true,
            "reason": "no_registered_agent_or_stored_retrieval_evidence",
        })
    }

    fn retrieval_metrics_judge_measurement(
        cas_root: &std::path::Path,
        judged: u64,
        session_filtered: bool,
    ) -> serde_json::Value {
        if judged > 0 {
            return serde_json::json!({
                "status": "available",
                "reason": null,
                "scope": if session_filtered { "session" } else { "all_sessions" },
                "labels_in_window": judged,
                "window_days": 30,
            });
        }

        match crate::config::Config::load(cas_root) {
            Ok(config) => {
                let sampler_enabled = config.daemon().relevance_sampling_enabled;
                let judge_configured =
                    crate::daemon::relevance::SCHEDULED_RELEVANCE_JUDGE_CONFIGURED;
                serde_json::json!({
                    "status": "unavailable",
                    "reason": if !sampler_enabled {
                        "sampler_disabled"
                    } else if !judge_configured {
                        "judge_unconfigured"
                    } else {
                        "no_judge_labels_in_window"
                    },
                    "scope": if session_filtered { "session" } else { "all_sessions" },
                    "labels_in_window": 0,
                    "window_days": 30,
                    "sampler_enabled": sampler_enabled,
                    "scheduled_judge_configured": judge_configured,
                })
            }
            Err(error) => serde_json::json!({
                "status": "unavailable",
                "reason": "sampler_configuration_unavailable",
                "scope": if session_filtered { "session" } else { "all_sessions" },
                "labels_in_window": 0,
                "window_days": 30,
                "detail": error.to_string(),
            }),
        }
    }

    /// Return the first explicitly supplied field that retrieval_metrics does
    /// not support. Serialization keeps this fail-closed when a new field is
    /// added to the unified request: it must be explicitly allow-listed here.
    fn retrieval_metrics_unsupported_parameter(req: &SearchContextRequest) -> Option<String> {
        let serde_json::Value::Object(fields) = serde_json::to_value(req).ok()? else {
            return Some("<request serialization failed>".to_string());
        };
        fields
            .into_iter()
            .filter(|(name, value)| {
                !value.is_null() && !matches!(name.as_str(), "action" | "session_id")
            })
            .map(|(name, _)| name)
            .min()
    }

    pub(in crate::mcp::tools::service) async fn skill_impact_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        use cas_store::SqliteSurfacedArtifactStore;

        let store = SqliteSurfacedArtifactStore::open(&self.inner.cas_root)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let artifacts = store
            .aggregate(req.limit.unwrap_or(100).clamp(1, 1_000))
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        Ok(Self::success(
            serde_json::json!({
                "version": 1,
                "artifacts": artifacts,
            })
            .to_string(),
        ))
    }

    pub(in crate::mcp::tools::service) async fn context_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        let limit = req.limit.unwrap_or_else(|| {
            crate::config::Config::load(&self.inner.cas_root)
                .unwrap_or_default()
                .context_limit()
        });
        if let Some(task_id) = req.task_id.as_deref() {
            return self
                .inner
                .cas_context_for_task(task_id, limit, req.max_tokens)
                .await;
        }

        use crate::mcp::tools::LimitRequest;
        let inner_req = LimitRequest {
            limit: req.limit,
            scope: req.scope.unwrap_or_else(|| "all".to_string()),
            sort: None,
            sort_order: None,
            team_id: None,
        };
        self.inner.cas_context(Parameters(inner_req)).await
    }

    pub(in crate::mcp::tools::service) async fn context_for_subagent_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::SubAgentContextRequest;
        let inner_req = SubAgentContextRequest {
            task_id: req
                .task_id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "task_id required"))?,
            max_tokens: req.max_tokens.unwrap_or(2000),
            include_memories: req.include_memories.unwrap_or(true),
        };
        self.inner
            .cas_context_for_subagent(Parameters(inner_req))
            .await
    }

    pub(in crate::mcp::tools::service) async fn observe_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::ObserveRequest;
        let inner_req = ObserveRequest {
            content: req
                .content
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "content required"))?,
            observation_type: req
                .observation_type
                .unwrap_or_else(|| "general".to_string()),
            source_tool: req.source_tool,
            tags: req.tags,
            scope: req.scope.unwrap_or_else(|| "project".to_string()),
        };
        self.inner.cas_observe(Parameters(inner_req)).await
    }

    pub(in crate::mcp::tools::service) async fn entity_list_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::EntityListRequest;
        let inner_req = EntityListRequest {
            entity_type: req.entity_type.clone(),
            query: req.query.clone(),
            tags: req.tags.clone(),
            scope: req.scope.clone(),
            sort: req.sort,
            sort_order: req.sort_order,
            limit: req.limit,
        };
        self.inner.cas_entity_list(Parameters(inner_req)).await
    }

    pub(in crate::mcp::tools::service) async fn entity_show_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required"))?,
        };
        self.inner.cas_entity_show(Parameters(inner_req)).await
    }

    pub(in crate::mcp::tools::service) async fn entity_extract_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::EntityExtractRequest;
        let inner_req = EntityExtractRequest {
            query: req.query,
            scope: req.scope,
            tags: req.tags,
            entity_type: req.entity_type,
            limit: req.limit,
        };
        self.inner.cas_entity_extract(Parameters(inner_req)).await
    }
}
