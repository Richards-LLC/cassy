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
        let args = crate::coordination::external_work::violet_identity_read_args(external_ref);
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
        self.refresh_task_source_for(source, None).await
    }

    pub(super) async fn refresh_task_source_for(&self, source: Option<&str>, task_id: Option<&str>) -> Result<(), McpError> {
        let Some(source) = source.filter(|source| crate::mcp::tools::core::task::source_claim::is_slack_source(source)) else { return Ok(()); };
        if task_id.is_some_and(|id| self.inner.source_split(id, Some(source)).is_some()) { return Ok(()); }
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
            "Task source {source}: owner {owner}; {}. No task or staffing change was applied. Ask the owner to offer this task via factory action=epic_split id=<epic-or-standalone-task> target=<your-supervisor> theirs=<task>, then accept it before staffing. Thread reply authority stays with the owner.",
            response["guidance"].as_str().or(response["code"].as_str()).unwrap_or("Slack authority unavailable")
        )))
    }

    pub(super) async fn refresh_task_sources(&self, ids: &[String]) -> Result<(), McpError> {
        let tasks = self.inner.open_task_store()?;
        for id in ids {
            let task = tasks.get(id).map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
            if task.execution_note.as_deref() == Some("no-code") { continue; }
            self.refresh_task_source_for(task.external_ref.as_deref(), Some(id)).await?;
        }
        Ok(())
    }

    pub(super) async fn external_work(
        &self,
        request: CoordinationRequest,
    ) -> Result<CallToolResult, McpError> {
        let source = (request.action == "claim").then(|| request.external_ref.clone()).flatten();
        let actor = source.as_ref().and_then(|_| self.inner.get_registered_agent_id_read_only().ok())
            .and_then(|id| self.inner.open_agent_store().ok()?.get(&id).ok());
        let outcome = self.external_work_inner(request).await;
        if let (Some(source), Some(actor)) = (source.as_deref(), actor.as_ref()) {
            let response = match &outcome {
                Ok(result) => result.content.iter().filter_map(|content| content.as_text())
                    .find_map(|text| serde_json::from_str::<Value>(&text.text).ok())
                    .unwrap_or_else(|| json!({"code":"invalid_response"})),
                Err(error) => json!({"code": error.data.as_ref().and_then(|data| data["error_code"].as_str())
                    .unwrap_or("claim_failed"), "mcp_error_code": error.code}),
            };
            surface_claim_refusal(&self.inner.cas_root, actor, source, &response).map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: format!("claim_blocker_unavailable: {error}").into(),
                data: Some(json!({"error_code":"claim_blocker_unavailable","claim_code":response["code"]})),
            })?;
        }
        outcome
    }

    async fn external_work_inner(
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
            registration_id: agent.id.clone(),
        };
        let service = ExternalWorkService::new(
            journal,
            Arc::new(HttpCloudCoordinationPort::new(cloud_config.endpoint, token)),
            violet,
            context,
        );
        let service_for_call = service.clone();
        let execution = tokio::task::spawn_blocking(move || service_for_call.execute(command)).await
            .unwrap_or_else(|error| Err(crate::coordination::external_work::ExternalWorkError {
                code: "claim_execution_failed".into(), message: format!("external-work call stopped: {error}"),
            }));
        let result = execution.map_err(|error| McpError {
            code: if error.code == "claim_execution_failed" { ErrorCode::INTERNAL_ERROR } else { ErrorCode::INVALID_REQUEST },
            message: error.to_string().into(),
            data: Some(json!({"error_code":error.code})),
        })?;

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

/// A claim never acquires anything here. This only attaches an existing,
/// daemon-authenticated mention to its owner's failed manual claim.
fn surface_claim_refusal(
    root: &std::path::Path, agent: &cas_types::Agent, external_ref: &str, result: &Value,
) -> Result<(), crate::error::CasError> {
    use crate::error::CasError;
    use rusqlite::params;
    let code = result["code"].as_str().unwrap_or("invalid_response");
    if matches!(code, "acquired" | "completed") || agent.role != cas_types::AgentRole::Supervisor {
        return Ok(());
    }
    let Ok(url) = url::Url::parse(external_ref) else { return Ok(()) };
    let parts: Vec<_> = url.path_segments().into_iter().flatten().collect();
    if url.scheme() != "https" || !url.host_str().is_some_and(|host| host.ends_with(".slack.com"))
        || parts.len() != 3 || parts[0] != "archives"
    { return Ok(()); }
    let channel = parts[1];
    if !channel.bytes().all(|b| b.is_ascii_alphanumeric()) { return Ok(()); }
    let root_ts = url.query_pairs().find(|(key, _)| key == "thread_ts").map(|(_, value)| value.into_owned())
        .or_else(|| parts[2].strip_prefix('p').filter(|ts| ts.len() > 6 && ts.bytes().all(|b| b.is_ascii_digit()))
            .map(|ts| format!("{}.{}", &ts[..ts.len()-6], &ts[ts.len()-6..])));
    let Some(root_ts) = root_ts.filter(|ts| ts.bytes().all(|b| b.is_ascii_digit() || b == b'.')) else { return Ok(()) };
    let journal = SqliteRequestJournal::open(root).map_err(|error| CasError::Other(error.to_string()))?;
    let project = crate::cloud::resolve_canonical_id(root).unwrap_or_default();
    let cached = journal.task_source_receipt(external_ref, &project).map_err(|error| CasError::Other(error.to_string()))?;
    let cached_owner = cached.as_ref().filter(|receipt| receipt["resource"]["state"] == "claimed")
        .and_then(|receipt| receipt["visible_owner"]["registration_id"].as_str());
    let owner_id = result.pointer("/receipt/visible_owner/registration_id").and_then(Value::as_str)
        .or(cached_owner).unwrap_or(&agent.id);
    let owner = if owner_id == agent.id { agent.clone() } else {
        crate::store::open_agent_store(root)?.get(owner_id)?
    };
    if owner.role != cas_types::AgentRole::Supervisor { return Ok(()); }
    let Some(session) = owner.factory_session.as_deref() else { return Ok(()) };
    let queue = crate::store::open_prompt_queue_store(root)?;
    // Include already surfaced/acked mentions: a supervisor normally claims
    // after reading its wake. Origin is stamped by CAS, never caller prose.
    let mentions: Vec<(i64, String)> = {
        let connection = cas_store::shared_db::shared_connection(&root.join("cas.db"))?;
        let connection = connection.lock().map_err(|_| CasError::Other("prompt queue lock poisoned".into()))?;
        let mut query = connection.prepare("SELECT id, prompt FROM prompt_queue
            WHERE source=?1 AND factory_session=?2 AND origin_kind='daemon'
              AND instr(prompt, ?3)>0 ORDER BY id DESC LIMIT 64")?;
        query.query_map(params![format!("violet-activity:{channel}"), session, format!(" thread_ts=\"{root_ts}\"")],
            |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?
    };
    let mention = mentions.into_iter().find(|(_, prompt)| {
        let tag = prompt.split('>').next().unwrap_or_default();
        tag.starts_with("<cas-violet-activity ") && tag.contains(" kind=\"mention\"")
            && tag.contains(" addressed=\"mention\"") && tag.contains(&format!(" channel=\"{channel}\""))
            && tag.contains(&format!(" thread_ts=\"{root_ts}\""))
            && (prompt.split_whitespace().any(|part| part.strip_prefix("delivery_id=")
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())) || cached.is_some())
    });
    let Some((mention_id, _)) = mention else { return Ok(()) };
    drop(queue);
    let context = json!({"kind":"blocker", "code":code, "external_ref":external_ref,
        "owner_registration_id":owner.id, "mention_prompt_id":mention_id,
        "request_id":result.pointer("/receipt/request_id"), "thread":{"channel_id":channel,"root_ts":root_ts}});
    let detail = format!("Managed thread claim failed ({code}). Thread: {external_ref}.\n{}", context)
        .replace('<', "&lt;");
    // Exact owner only: this path must never use the generic live-peer fallback.
    crate::supervisor_notices::enqueue_with_context(root, &crate::supervisor_notices::Recipient::agent(&owner),
        "blocker", &detail, &format!("violet-claim:{mention_id}:{code}"), Some(context))
}

#[cfg(test)]
mod mention_claim_tests {
    use super::*;
    use cas_store::{PromptQueueStore, QueueOrigin};
    #[test]
    fn failed_mention_claim_records_one_owner_blocker_cas_0288() {
        let temp = tempfile::tempdir().unwrap();
        let mut env = crate::test_support::TestEnvGuard::temp_home(); env.set("CAS_ROOT", temp.path());
        let mut owner = cas_types::Agent::new_with_role("owner-id".into(), "owner".into(), cas_types::AgentRole::Supervisor);
        owner.factory_session = Some("owner-session".into());
        let agents = crate::store::open_agent_store(temp.path()).unwrap(); agents.init().unwrap(); agents.register(&owner).unwrap();
        let mut other = cas_types::Agent::new_with_role("other-id".into(), "other".into(), cas_types::AgentRole::Supervisor);
        other.factory_session = Some("other-session".into()); agents.register(&other).unwrap();
        let queue = crate::store::open_prompt_queue_store(temp.path()).unwrap(); queue.init().unwrap();
        let mention = queue.enqueue_idempotent("violet-activity:C08COORD01", "supervisor",
            "<cas-violet-activity v=\"1\" channel=\"C08COORD01\" kind=\"mention\" addressed=\"mention\" thread_ts=\"1760616000.123456\">\nClaimed delivery_id=0000de2a-1111-4111-8111-00000000de2a\n</cas-violet-activity>",
            Some("owner-session"), None, None, "mention-fixture", Some(&QueueOrigin::Daemon)).unwrap();
        let mention_id = match mention {
            cas_store::EnqueueIdempotentResult::Created(id) | cas_store::EnqueueIdempotentResult::AlreadyExists(id) => id,
        };
        queue.mark_processed(mention_id).unwrap();
        let result = json!({"code":"size_cap_exceeded","receipt":{"request_id":"request-fixture"}});
        let link = "https://example.slack.com/archives/C08COORD01/p1760616000123456";
        for _ in 0..2 { surface_claim_refusal(temp.path(), &owner, link, &result).unwrap(); }
        let routed = json!({"code":"size_cap_exceeded", "receipt":{"visible_owner":{"registration_id":owner.id}}});
        surface_claim_refusal(temp.path(), &other, link, &routed).unwrap();
        surface_claim_refusal(temp.path(), &owner, link, &json!({"code":"acquired"})).unwrap();
        let outbox = crate::store::open_supervisor_queue_store(temp.path()).unwrap();
        assert!(outbox.peek(&other.id, 10).unwrap().is_empty());
        let notices = outbox.peek(&owner.id, 10).unwrap();
        assert_eq!(notices.len(), 1);
        let payload: Value = serde_json::from_str(&notices[0].payload).unwrap();
        assert_eq!(payload["kind"], "blocker");
        assert_eq!(payload["context"]["code"], "size_cap_exceeded");
        assert_eq!(payload["context"]["external_ref"], link);
    }
}
