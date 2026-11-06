//! Supervisor entry point for Slack-thread authority and GitHub source handoff.

use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{CallToolResult, ErrorCode};
use serde_json::{Value, json};

use super::{CasService, CoordinationRequest, McpError};
use crate::coordination::external_work::{
    ExternalWorkCommand, ExternalWorkService, HttpCloudCoordinationPort, OperationContext,
    RemoteError, SqliteRequestJournal, VioletThreadResolver,
};

#[cfg(feature = "mcp-proxy")]
struct ProxyVioletThreadResolver {
    proxy: Option<Arc<cmcp_core::ProxyEngine>>,
    caller: cmcp_core::ProxyCaller,
}

#[cfg(feature = "mcp-proxy")]
impl VioletThreadResolver for ProxyVioletThreadResolver {
    fn resolve(&self, external_ref: &str, timeout: Duration) -> Result<Value, RemoteError> {
        let Some(proxy) = self.proxy.as_ref().map(Arc::clone) else {
            return Err(RemoteError::unavailable(
                "no MCP proxy is configured, so Violet cannot be reached; run `cas integrate violet`",
            ));
        };
        let caller = self.caller.clone();
        let args = serde_json::Map::from_iter([("external_ref".to_string(), json!(external_ref))]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                RemoteError::unavailable(format!("could not start Violet client: {error}"))
            })?;
        let result = runtime.block_on(async move {
            tokio::time::timeout(
                timeout,
                proxy.call_tool(
                    &caller,
                    cmcp_core::config::VIOLET_SERVER,
                    "violet_read",
                    Some(args),
                ),
            )
            .await
        });
        crate::coordination::external_work::violet_proxy_outcome(result)
    }
}

#[cfg(not(feature = "mcp-proxy"))]
struct ProxyVioletThreadResolver;

#[cfg(not(feature = "mcp-proxy"))]
impl VioletThreadResolver for ProxyVioletThreadResolver {
    fn resolve(&self, _external_ref: &str, _timeout: Duration) -> Result<Value, RemoteError> {
        Err(RemoteError::unavailable(
            "this build has no MCP proxy; run `cas integrate violet` with mcp-proxy enabled",
        ))
    }
}

impl CasService {
    /// Task creation and staffing reuse the authenticated Slack claim service.
    /// Workers consume the supervisor's receipt; they never acquire authority.
    pub(super) async fn refresh_task_source(&self, source: Option<&str>) -> Result<(), McpError> {
        let Some(source) = source.filter(|source| crate::mcp::tools::core::task::source_claim::is_slack_source(source)) else { return Ok(()); };
        let Ok(id) = self.inner.get_registered_agent_id_read_only() else { return Ok(()); };
        let agents = self.inner.open_agent_store()?;
        if !agents.get(&id).is_ok_and(|agent| agent.role == crate::types::AgentRole::Supervisor) { return Ok(()); }
        let request: CoordinationRequest = serde_json::from_value(json!({"action":"claim", "external_ref":source}))
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let result = self.external_work(request).await?;
        let text = result.content.iter().filter_map(|content| content.as_text().map(|text| text.text.as_str())).collect::<Vec<_>>().join("\n");
        let response: Value = serde_json::from_str(&text)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let receipt = &response["receipt"];
        if receipt["visible_owner"]["registration_id"].as_str() == Some(id.as_str())
            && receipt["next_action"] == "proceed"
            && matches!(response["code"].as_str(), Some("acquired" | "completed"))
            && receipt["resource"]["state"] == "claimed"
            && receipt["expires_at"].as_str().and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok()).is_some_and(|at| at > chrono::Utc::now())
        { return Ok(()); }
        let owner = receipt["visible_owner"]["label"].as_str()
            .or(receipt["visible_owner"]["registration_id"].as_str()).unwrap_or("not verified");
        Err(Self::error(ErrorCode::INVALID_REQUEST, format!(
            "Task source {source}: owner {owner}; {}. No task or staffing change was applied. Use the existing claim/status/handoff before continuing.",
            response["guidance"].as_str().or(response["code"].as_str()).unwrap_or("Slack authority unavailable")
        )))
    }

    pub(super) async fn refresh_task_sources(&self, ids: &[String]) -> Result<(), McpError> {
        let tasks = self.inner.open_task_store()?;
        for id in ids {
            let task = tasks.get(id).map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
            if task.execution_note.as_deref() == Some("no-code") { continue; }
            if crate::mcp::tools::core::task::source_claim::source_split(&self.inner.cas_root, id).is_some() { continue; }
            self.refresh_task_source(task.external_ref.as_deref()).await?;
        }
        Ok(())
    }

    pub(super) async fn external_work(
        &self,
        request: CoordinationRequest,
    ) -> Result<CallToolResult, McpError> {
        let agent_id = self.inner.get_registered_agent_id_read_only()?;
        let agents = self.inner.open_agent_store()?;
        let agent = agents.get(&agent_id).map_err(|_| {
            Self::error(
                ErrorCode::INVALID_REQUEST,
                "External thread claims require an authenticated registered supervisor.",
            )
        })?;
        if agent.role != crate::types::AgentRole::Supervisor {
            return Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                "External thread authority is available only to the authenticated supervisor.",
            ));
        }

        if request.action == "release_claim"
            && let Some(key) = request.resource_id.as_deref()
                .and_then(crate::cloud::peer_claims::github_source_key)
        {
            let claims = crate::cloud::peer_claims::PeerClaims::for_project(&self.inner.cas_root)
                .ok_or_else(|| Self::error(ErrorCode::INVALID_REQUEST,
                    "GitHub source handoff requires Cassy Cloud login for this project."))?;
            let root = self.inner.cas_root.clone();
            let registration = agent.id.clone();
            let budget = crate::cloud::peer_claims::PeerBudget::default();
            let released = budget.run(move || {
                claims.with_budget(budget).release_source_for_handoff(&root, &registration, &key)
            }).await.map_err(|error| Self::error(ErrorCode::INVALID_REQUEST, error))?
                .map_err(|error| Self::error(ErrorCode::INVALID_REQUEST, error))?;
            return Ok(Self::success(json!({
                "code": if released { "released" } else { "not_held" },
                "source": request.resource_id,
                "registration_id": agent.id,
            }).to_string()));
        }

        let command = command_from_request(&request)
            .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))?;
        let project_id =
            crate::cloud::resolve_canonical_id(&self.inner.cas_root).ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "This project has no canonical Cloud project identity.",
                )
            })?;
        let cloud_config =
            crate::cloud::CloudConfig::load_from_cas_dir_inheriting_user_credentials(
                &self.inner.cas_root,
            )
            .map_err(|error| Self::error(ErrorCode::INVALID_REQUEST, error.to_string()))?;
        let token = cloud_config
            .token
            .clone()
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "External thread authority requires Cassy Cloud login.",
                )
            })?;

        #[cfg(feature = "mcp-proxy")]
        let caller = self.proxy_caller()?;
        #[cfg(feature = "mcp-proxy")]
        let violet: Arc<dyn VioletThreadResolver> = match &self.proxy {
            Some(proxy) => Arc::new(ProxyVioletThreadResolver {
                proxy: Some(Arc::clone(proxy)),
                caller,
            }),
            None => Arc::new(ProxyVioletThreadResolver {
                proxy: None,
                caller,
            }),
        };
        #[cfg(not(feature = "mcp-proxy"))]
        let violet: Arc<dyn VioletThreadResolver> = Arc::new(ProxyVioletThreadResolver);

        let journal = SqliteRequestJournal::open(&self.inner.cas_root)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let context = OperationContext {
            project_id,
            // Cloud's registration identifier is the authenticated agent row ID.
            registration_id: agent.id,
        };
        let service = ExternalWorkService::new(
            journal,
            Arc::new(HttpCloudCoordinationPort::new(cloud_config.endpoint, token)),
            violet,
            context,
        );
        let service_for_call = service.clone();
        let result = tokio::task::spawn_blocking(move || service_for_call.execute(command))
            .await
            .map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("external-work call stopped: {error}"),
                )
            })?
            .map_err(|error| Self::error(ErrorCode::INVALID_REQUEST, error.to_string()))?;

        if matches!(
            result.get("code").and_then(Value::as_str),
            Some("pending" | "outcome_unknown" | "authority_unavailable")
        ) {
            let retry_service = service.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let _ = tokio::task::spawn_blocking(move || {
                    retry_service.reconcile_due(chrono::Utc::now().timestamp_millis())
                })
                .await;
            });
        }
        let text = serde_json::to_string(&result)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        Ok(Self::success(text))
    }
}

fn command_from_request(request: &CoordinationRequest) -> Result<ExternalWorkCommand, String> {
    let required = |field: Option<&str>, name: &str| {
        field
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("{} requires {name}", request.action))
    };
    let request_id = request.request_id.clone();
    match request.action.as_str() {
        "claim" => Ok(ExternalWorkCommand::Claim {
            external_ref: required(request.external_ref.as_deref(), "external_ref")?,
            request_id,
        }),
        "claim_status" => Ok(ExternalWorkCommand::ClaimStatus {
            request_id: required(request.request_id.as_deref(), "request_id")?,
        }),
        "release_claim" => Ok(ExternalWorkCommand::ReleaseClaim {
            resource_id: required(request.resource_id.as_deref(), "resource_id")?,
            expected_generation: request
                .expected_generation
                .ok_or_else(|| "release_claim requires expected_generation".to_string())?,
            request_id,
        }),
        "transfer_claim" => Ok(ExternalWorkCommand::TransferClaim {
            resource_id: required(request.resource_id.as_deref(), "resource_id")?,
            expected_generation: request
                .expected_generation
                .ok_or_else(|| "transfer_claim requires expected_generation".to_string())?,
            to_slot_id: required(request.to_slot_id.as_deref(), "to_slot_id")?,
            offer_ttl_secs: request.offer_ttl_secs,
            request_id,
        }),
        "accept_claim" => Ok(ExternalWorkCommand::AcceptClaim {
            handoff_id: required(request.handoff_id.as_deref(), "handoff_id")?,
            expected_generation: request
                .expected_generation
                .ok_or_else(|| "accept_claim requires expected_generation".to_string())?,
            request_id,
        }),
        "renew_claim" => Ok(ExternalWorkCommand::RenewClaim {
            resource_id: required(request.resource_id.as_deref(), "resource_id")?,
            expected_generation: request
                .expected_generation
                .ok_or_else(|| "renew_claim requires expected_generation".to_string())?,
            request_id,
        }),
        // cas-f869 (GH #1272): the result carries the exact violet_post fields.
        "reply_intent" => Ok(ExternalWorkCommand::ReplyIntent {
            resource_id: required(request.resource_id.as_deref(), "resource_id")?,
            expected_generation: request
                .expected_generation
                .ok_or_else(|| "reply_intent requires expected_generation".to_string())?,
            occurrence_id: request
                .occurrence_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            request_id,
        }),
        _ => Err(format!(
            "unsupported external-work action {}",
            request.action
        )),
    }
}
