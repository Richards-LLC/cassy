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
    ) -> Result<CallToolResult, McpError> {
        use cas_store::SqliteRetrievalStore;

        let store = SqliteRetrievalStore::open(&self.inner.cas_root)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let aggregates = store
            .aggregate()
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let precision = store
            .rolling_injected_precision(30)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        Ok(Self::success(
            serde_json::json!({
                "version": 1,
                "groups": aggregates,
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
        if let Some(task_id) = req.task_id.as_deref() {
            return self
                .inner
                .cas_context_for_task(task_id, req.limit.unwrap_or(5), req.max_tokens)
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
