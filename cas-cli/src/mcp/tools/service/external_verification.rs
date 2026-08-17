use rmcp::model::{CallToolResult, ErrorCode};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{CasService, McpError, VerificationRequest};
use cas_store::{
    DelegationBudget, DelegationReceipt, DelegationReceiptState, DelegationReserveOutcome,
    DelegationReserveRequest, DelegationVerdict, EXTERNAL_PRODUCTION_VERIFICATION_GATE,
    ExternalVerificationOutcome, ExternalVerificationRequest, RequiredCheck,
    SqliteDelegationReceiptStore, assess_response, assessment_for_outcome, authorize_request,
    record_assessment,
};

impl CasService {
    pub(super) async fn verification_external(
        &self,
        req: VerificationRequest,
    ) -> Result<CallToolResult, McpError> {
        let proxy = self.proxy.as_ref().ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_REQUEST,
                "External verification requires a configured MCP proxy.",
            )
        })?;
        let task_id = required(req.task_id, "task_id")?;
        let message = required(req.message, "message")?;
        let local_proof_reference = required(req.local_proof_reference, "local_proof_reference")?;
        let required_checks: Vec<RequiredCheck> =
            serde_json::from_str(required(req.required_checks, "required_checks")?.as_str())
                .map_err(|_| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "required_checks must be a JSON array of {name, expected} objects",
                    )
                })?;

        let caller = self.proxy_caller()?;
        let gate_request = ExternalVerificationRequest {
            caller_role: caller.role,
            local_proof_reference: local_proof_reference.clone(),
            required_checks: required_checks.clone(),
        };
        authorize_request(&gate_request).map_err(|error| {
            Self::error(
                ErrorCode::INVALID_REQUEST,
                format!("External verification denied: {error}"),
            )
        })?;
        let factory_session_id = caller.factory_session.clone().ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_REQUEST,
                "External verification requires a registered factory supervisor session.",
            )
        })?;

        let task_store = self.inner.open_task_store()?;
        let task = task_store.get(&task_id).map_err(|error| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("External verification task not found: {error}"),
            )
        })?;
        let epic_id = if task.task_type == crate::types::TaskType::Epic {
            task.id.clone()
        } else {
            task_store
                .get_parent_epic(&task_id)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to resolve delegation epic: {error}"),
                    )
                })?
                .map(|epic| epic.id)
                .ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_REQUEST,
                        "External verification requires a task attached to an epic.",
                    )
                })?
        };

        let config = load_proxy_config(&self.inner.cas_root)?;
        let gateway = config
            .delegation
            .external_production_verification
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "External production verification is not configured.",
                )
            })?;
        for tool in [&gateway.start_tool, &gateway.wait_tool] {
            if !config
                .allowlist
                .iter()
                .any(|route| route.server == gateway.server && route.tool == *tool)
            {
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    format!(
                        "External verification route {}.{} is not explicitly allowlisted.",
                        gateway.server, tool
                    ),
                ));
            }
        }

        let canonical = json!({
            "gate": EXTERNAL_PRODUCTION_VERIFICATION_GATE,
            "task_id": task_id,
            "message": message,
            "local_proof_reference": local_proof_reference,
            "required_checks": required_checks,
        });
        let request_digest = format!("{:x}", Sha256::digest(canonical.to_string().as_bytes()));
        let store = SqliteDelegationReceiptStore::open(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open delegation receipt store: {error}"),
            )
        })?;
        let budget = DelegationBudget {
            max_per_run: gateway.max_per_run,
            max_active_per_factory_session: gateway.max_active_per_factory_session,
            max_active_per_epic: gateway.max_active_per_epic,
        };
        let reservation = DelegationReserveRequest {
            factory_session_id,
            epic_id,
            task_id: task_id.clone(),
            gate_kind: EXTERNAL_PRODUCTION_VERIFICATION_GATE.to_string(),
            request_digest,
            reserved_amount: gateway.reserved_amount,
        };
        let outcome = store
            .reserve_or_resume(&reservation, &budget)
            .map_err(|error| {
                Self::error(
                    ErrorCode::INVALID_REQUEST,
                    format!("External verification reservation denied: {error}"),
                )
            })?;
        let receipt = match &outcome {
            DelegationReserveOutcome::Created(receipt)
            | DelegationReserveOutcome::Existing(receipt)
            | DelegationReserveOutcome::Resume(receipt) => receipt.clone(),
        };
        if receipt.state == DelegationReceiptState::Completed {
            return Ok(render_receipt(&receipt));
        }

        let (tool, arguments) = match outcome {
            DelegationReserveOutcome::Resume(receipt) => {
                let run_id = store.resume_run(&receipt.id).map_err(|error| {
                    Self::error(
                        ErrorCode::INVALID_REQUEST,
                        format!("External verification cannot resume: {error}"),
                    )
                })?;
                let mut args = Map::new();
                args.insert("run_id".to_string(), Value::String(run_id));
                args.insert(
                    "timeout_seconds".to_string(),
                    Value::Number(gateway.timeout_seconds.into()),
                );
                (gateway.wait_tool.clone(), args)
            }
            DelegationReserveOutcome::Created(receipt)
            | DelegationReserveOutcome::Existing(receipt) => {
                let mut args = Map::new();
                args.insert(
                    "message".to_string(),
                    Value::String(provider_message(
                        &message,
                        &local_proof_reference,
                        &required_checks,
                    )),
                );
                args.insert("speed".to_string(), Value::String("smarter".to_string()));
                args.insert(
                    "timeout_seconds".to_string(),
                    Value::Number(gateway.timeout_seconds.into()),
                );
                args.insert(
                    "idempotency_key".to_string(),
                    Value::String(receipt.idempotency_key),
                );
                args.insert("response_format".to_string(), response_format());
                (gateway.start_tool.clone(), args)
            }
        };

        let raw = match proxy
            .call_external_production_verification_tool(
                &caller,
                &gateway.server,
                &tool,
                Some(arguments),
            )
            .await
        {
            Ok(raw) => raw,
            Err(error) => {
                let assessment =
                    assessment_for_outcome(ExternalVerificationOutcome::TransportFailure);
                let terminal = record_assessment(
                    &store,
                    &receipt.id,
                    &assessment,
                    receipt.reserved_amount,
                    &format!("delegation://receipt/{}", receipt.id),
                )
                .map_err(|store_error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to record transport failure: {store_error}"),
                    )
                })?;
                tracing::warn!(receipt_id = %receipt.id, error = %error, "external verification transport failed closed");
                return Ok(render_receipt(&terminal));
            }
        };

        let payload = extract_tool_payload(&raw);
        let run_id = find_string(&payload, &["run_id"]).or_else(|| {
            find_pointer_string(&payload, &["/run/id", "/result/run_id", "/result/run/id"])
        });
        if let Some(run_id) = run_id.as_deref() {
            store.record_run_id(&receipt.id, run_id).map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to record delegated run id: {error}"),
                )
            })?;
        }
        if payload
            .get("wait_timed_out")
            .or_else(|| payload.pointer("/result/wait_timed_out"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let run_id = run_id.ok_or_else(|| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    "External verifier timed out without a resumable run_id; refusing to start another run.",
                )
            })?;
            let timed_out = store
                .record_timeout(&receipt.id, &run_id)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to persist timed-out delegation: {error}"),
                    )
                })?;
            return Ok(render_receipt(&timed_out));
        }

        let outcome = provider_outcome(&payload);
        let assessment = match outcome {
            ExternalVerificationOutcome::Response => {
                let response = payload
                    .get("json")
                    .or_else(|| payload.pointer("/result/json"))
                    .cloned()
                    .unwrap_or(payload.clone());
                assess_response(&gate_request, response)
            }
            other => assessment_for_outcome(other),
        };
        let evidence_reference = run_id
            .as_deref()
            .map(|run_id| format!("viktor://run/{run_id}"))
            .unwrap_or_else(|| format!("delegation://receipt/{}", receipt.id));
        let terminal = record_assessment(
            &store,
            &receipt.id,
            &assessment,
            receipt.reserved_amount,
            &evidence_reference,
        )
        .map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to persist external verification verdict: {error}"),
            )
        })?;
        Ok(render_receipt(&terminal))
    }
}

fn required(value: Option<String>, field: &str) -> Result<String, McpError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CasService::error(
                ErrorCode::INVALID_PARAMS,
                format!("{field} required for external_verify"),
            )
        })
}

fn load_proxy_config(root: &std::path::Path) -> Result<cmcp_core::config::Config, McpError> {
    let path = root.join("proxy.toml");
    cmcp_core::config::Config::load_merged(path.exists().then_some(path.as_path())).map_err(
        |error| {
            CasService::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to load external verification config: {error}"),
            )
        },
    )
}

fn provider_message(message: &str, local_proof: &str, checks: &[RequiredCheck]) -> String {
    let checks = checks
        .iter()
        .map(|check| format!("- {}: {}", check.name, check.expected))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Read-only external production verification. Do not mutate, submit, approve, or authenticate with write capability.\n\nRequest: {message}\nLocal proof: {local_proof}\nRequired checks:\n{checks}"
    )
}

fn response_format() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["verdict", "checks", "limitations"],
        "properties": {
            "verdict": {"enum": ["pass", "fail", "inconclusive"]},
            "checks": {"type": "array", "minItems": 1, "items": {
                "type": "object", "additionalProperties": false,
                "required": ["name", "expected", "observed", "evidence"],
                "properties": {
                    "name": {"type": "string"}, "expected": {"type": "string"},
                    "observed": {"type": "string"}, "evidence": {"type": "string"}
                }
            }},
            "limitations": {"type": "array", "items": {"type": "string"}}
        }
    })
}

fn extract_tool_payload(raw: &Value) -> Value {
    if let Some(structured) = raw
        .get("structuredContent")
        .filter(|structured| !structured.is_null())
    {
        return structured.clone();
    }
    raw.pointer("/content/0/text")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| json!({"error": "malformed"}))
}

fn find_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn find_pointer_string(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .map(str::to_string)
}

fn provider_outcome(payload: &Value) -> ExternalVerificationOutcome {
    let code = find_string(payload, &["error", "status", "verdict"])
        .or_else(|| {
            find_pointer_string(
                payload,
                &[
                    "/detail/error",
                    "/result/error",
                    "/result/status",
                    "/run/status",
                ],
            )
        })
        .unwrap_or_default()
        .to_ascii_lowercase();
    match code.as_str() {
        "requires_action" => ExternalVerificationOutcome::RequiresAction,
        "thread_busy" => ExternalVerificationOutcome::ThreadBusy,
        "insufficient_scope" => ExternalVerificationOutcome::InsufficientScope,
        "rate_limited" | "rate_limit_exceeded" => ExternalVerificationOutcome::RateLimited,
        "cancelled" => ExternalVerificationOutcome::Cancelled,
        "transport_failure" => ExternalVerificationOutcome::TransportFailure,
        _ => ExternalVerificationOutcome::Response,
    }
}

fn render_receipt(receipt: &DelegationReceipt) -> CallToolResult {
    let verdict = receipt.terminal_verdict.map(|verdict| verdict.to_string());
    let passing = receipt.terminal_verdict == Some(DelegationVerdict::Pass);
    CasService::success(
        serde_json::to_string_pretty(&json!({
            "receipt_id": receipt.id,
            "task_id": receipt.task_id,
            "state": receipt.state.to_string(),
            "run_id": receipt.run_id,
            "verdict": verdict,
            "passing": passing,
            "evidence_reference": receipt.evidence_reference,
        }))
        .unwrap_or_else(|_| "{\"passing\":false,\"verdict\":\"malformed\"}".to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::server::CasCore;
    use crate::store::{init_cas_dir, open_task_store};
    use crate::types::{AgentRole, Task, TaskType};
    use cmcp_core::config::{
        Config as ProxyConfig, ExternalProductionVerificationConfig, ExternalToolConfig,
        ServerConfig,
    };
    use std::collections::HashMap;

    #[tokio::test]
    async fn supervisor_flow_reserves_live_store_and_fails_closed_on_transport() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = init_cas_dir(temp.path()).unwrap();
        let core = CasCore::with_daemon(cas_root.clone(), None, None);
        core.register_agent(
            "supervisor-session".to_string(),
            "supervisor".to_string(),
            None,
        )
        .unwrap();
        let agent_store = core.open_agent_store().unwrap();
        let mut agent = agent_store.get("supervisor-session").unwrap();
        agent.role = AgentRole::Supervisor;
        agent.factory_session = Some("factory-live".to_string());
        agent_store.update(&agent).unwrap();

        let task_store = open_task_store(&cas_root).unwrap();
        let mut epic = Task::new("cas-gate-epic".to_string(), "epic".to_string());
        epic.task_type = TaskType::Epic;
        task_store.add(&epic).unwrap();
        let task = Task::new("cas-gate-task".to_string(), "task".to_string());
        task_store
            .create_atomic(&task, &[], Some(&epic.id), Some(&agent.id))
            .unwrap();

        let mut config = ProxyConfig::default();
        config.allowlist = vec![
            ExternalToolConfig {
                server: "viktor".to_string(),
                tool: "ask_viktor".to_string(),
            },
            ExternalToolConfig {
                server: "viktor".to_string(),
                tool: "wait_for_run".to_string(),
            },
        ];
        config.delegation.external_production_verification =
            Some(ExternalProductionVerificationConfig {
                server: "viktor".to_string(),
                start_tool: "ask_viktor".to_string(),
                wait_tool: "wait_for_run".to_string(),
                reserved_amount: 1,
                max_per_run: 1,
                max_active_per_factory_session: 4,
                max_active_per_epic: 2,
                timeout_seconds: 30,
            });
        config.save_to(&cas_root.join("proxy.toml")).unwrap();
        let engine = cmcp_core::ProxyEngine::from_configs(Default::default())
            .await
            .unwrap();
        crate::mcp::server::install_proxy_policy(&engine, &config).await;
        let service = CasService::new(core, Some(std::sync::Arc::new(engine)));
        let req: VerificationRequest = serde_json::from_value(json!({
            "action": "external_verify",
            "task_id": task.id,
            "message": "Confirm the public health endpoint returns ready",
            "local_proof_reference": "local://scoped-test",
            "required_checks": "[{\"name\":\"health\",\"expected\":\"ready\"}]"
        }))
        .unwrap();

        let result = service.verification_external(req).await.unwrap();
        let encoded = serde_json::to_value(result).unwrap();
        let body: Value = serde_json::from_str(
            encoded["content"][0]["text"]
                .as_str()
                .expect("receipt result text"),
        )
        .unwrap();
        assert_eq!(body["verdict"], "transport_failure");
        assert_eq!(body["passing"], false);
        let receipt_id = body["receipt_id"].as_str().unwrap();
        let receipt = SqliteDelegationReceiptStore::open(&cas_root)
            .unwrap()
            .get(receipt_id)
            .unwrap();
        assert_eq!(receipt.state, DelegationReceiptState::Completed);
        assert_eq!(
            receipt.terminal_verdict,
            Some(DelegationVerdict::TransportFailure)
        );
        assert_eq!(receipt.factory_session_id, "factory-live");
        assert_eq!(receipt.epic_id, epic.id);
    }

    #[tokio::test]
    async fn supervisor_flow_records_real_upstream_run_and_passing_gate_verdict() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = init_cas_dir(temp.path()).unwrap();
        let core = CasCore::with_daemon(cas_root.clone(), None, None);
        core.register_agent(
            "supervisor-session".to_string(),
            "supervisor".to_string(),
            None,
        )
        .unwrap();
        let agent_store = core.open_agent_store().unwrap();
        let mut agent = agent_store.get("supervisor-session").unwrap();
        agent.role = AgentRole::Supervisor;
        agent.factory_session = Some("factory-live".to_string());
        agent_store.update(&agent).unwrap();

        let task_store = open_task_store(&cas_root).unwrap();
        let mut epic = Task::new("cas-gate-epic".to_string(), "epic".to_string());
        epic.task_type = TaskType::Epic;
        task_store.add(&epic).unwrap();
        let task = Task::new("cas-gate-task".to_string(), "task".to_string());
        task_store
            .create_atomic(&task, &[], Some(&epic.id), Some(&agent.id))
            .unwrap();

        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mock_mcp_viktor_server.py");
        let upstream = ServerConfig::Stdio {
            command: "python3".to_string(),
            args: vec![fixture.to_string_lossy().into_owned()],
            env: HashMap::new(),
        };
        let mut config = ProxyConfig::default();
        config.add_server("viktor".to_string(), upstream.clone());
        config.allowlist = vec![
            ExternalToolConfig {
                server: "viktor".to_string(),
                tool: "ask_viktor".to_string(),
            },
            ExternalToolConfig {
                server: "viktor".to_string(),
                tool: "wait_for_run".to_string(),
            },
        ];
        config.delegation.external_production_verification =
            Some(ExternalProductionVerificationConfig {
                server: "viktor".to_string(),
                start_tool: "ask_viktor".to_string(),
                wait_tool: "wait_for_run".to_string(),
                reserved_amount: 1,
                max_per_run: 1,
                max_active_per_factory_session: 4,
                max_active_per_epic: 2,
                timeout_seconds: 30,
            });
        config.save_to(&cas_root.join("proxy.toml")).unwrap();
        let engine =
            cmcp_core::ProxyEngine::from_configs(HashMap::from([("viktor".to_string(), upstream)]))
                .await
                .unwrap();
        crate::mcp::server::install_proxy_policy(&engine, &config).await;
        let service = CasService::new(core, Some(std::sync::Arc::new(engine)));
        let req: VerificationRequest = serde_json::from_value(json!({
            "action": "external_verify",
            "task_id": task.id,
            "message": "Confirm the public health endpoint returns ready",
            "local_proof_reference": "local://scoped-test",
            "required_checks": "[{\"name\":\"health\",\"expected\":\"ready\"}]"
        }))
        .unwrap();

        let result = service.verification_external(req).await.unwrap();
        let encoded = serde_json::to_value(result).unwrap();
        let body: Value = serde_json::from_str(
            encoded["content"][0]["text"]
                .as_str()
                .expect("receipt result text"),
        )
        .unwrap();
        assert_eq!(body["verdict"], "pass");
        assert_eq!(body["passing"], true);
        assert_eq!(body["run_id"], "run-fixture-1");
        let receipt = SqliteDelegationReceiptStore::open(&cas_root)
            .unwrap()
            .get(body["receipt_id"].as_str().unwrap())
            .unwrap();
        assert_eq!(receipt.state, DelegationReceiptState::Completed);
        assert_eq!(receipt.terminal_verdict, Some(DelegationVerdict::Pass));
        assert_eq!(
            receipt.evidence_reference.as_deref(),
            Some("viktor://run/run-fixture-1")
        );
    }
}
