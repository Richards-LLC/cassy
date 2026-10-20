//! Supervisor entry point for journaled Slack-thread authority.

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
