//! Shared stakeholder admission belongs to Cloud and the trusted Violet relay.
//! The existing request journal holds only local receipt evidence for warnings.

use super::external_work::{
    CloudCoordinationPort, HttpCloudCoordinationPort, JournalError, SqliteRequestJournal,
};
use crate::config::{Config, StakeholderContactPolicy};
use cmcp_core::{PreparedProxyCall, ProxyCallEvent, ProxyCallInterceptor, ProxyCaller};
use rmcp::model::{CallToolResult, Content};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub const SCHEMA: &str = "cassy.stakeholder-contacts/v1";
const BASE: &str = "/api/coordination/stakeholder-contacts";
const BUDGET: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct StakeholderContacts {
    root: PathBuf,
    cloud: Option<Arc<dyn CloudCoordinationPort>>,
    project_id: Option<String>,
}

impl StakeholderContacts {
    pub fn from_project(root: &Path) -> Self {
        let cloud = crate::cloud::CloudConfig::load_from_cas_dir_inheriting_user_credentials(root)
            .ok()
            .and_then(|config| {
                config.token.filter(|token| !token.is_empty()).map(|token| {
                    Arc::new(HttpCloudCoordinationPort::new(config.endpoint, token))
                        as Arc<dyn CloudCoordinationPort>
                })
            });
        Self {
            root: root.into(),
            cloud,
            project_id: crate::cloud::resolve_canonical_id(root),
        }
    }

    fn policies(&self) -> Result<Vec<StakeholderContactPolicy>, String> {
        let policies = Config::load(&self.root)
            .map_err(|e| e.to_string())?
            .slack
            .unwrap_or_default()
            .stakeholder_contacts;
        for policy in &policies {
            policy.validate()?;
        }
        Ok(policies)
    }

    fn supports_shared_contacts(&self) -> bool {
        let (Some(cloud), Some(project)) = (&self.cloud, &self.project_id) else {
            return false;
        };
        cloud
            .get(
                &format!("{BASE}/capabilities"),
                &[("project_id", project)],
                BUDGET,
            )
            .is_ok_and(|value| {
                value["schema"] == SCHEMA
                    && value["atomic_reservations"] == true
                    && value["violet_settlement"] == true
            })
    }

    fn prepare_post(
        &self,
        caller: &ProxyCaller,
        arguments: Option<serde_json::Map<String, Value>>,
    ) -> anyhow::Result<PreparedProxyCall> {
        let mut prepared = PreparedProxyCall::forward(arguments);
        let Some(args) = prepared.arguments.as_mut() else {
            return Ok(prepared);
        };
        let Some(text) = root_text(args).filter(|text| mentioned_users(text).next().is_some())
        else {
            return Ok(prepared);
        };
        let policies = match self.policies() {
            Ok(policies) => mentioned_policies(text, policies),
            Err(_) => {
                // A local configuration failure is not shared admission.
                // Do not copy config parse diagnostics (potentially secrets)
                // into an upstream result or turn unrelated posts into refusals.
                prepared.warning = Some(
                    "Stakeholder contact: configuration unavailable; post permitted.\nShared admission and last contact are unknown. Check slack.stakeholder_contacts."
                        .into(),
                );
                return Ok(prepared);
            }
        };
        if policies.is_empty() {
            return Ok(prepared);
        }
        // Clients cannot smuggle a grant through this adapter or select a
        // weaker local policy. Cloud and Violet validate the effective policy.
        args.remove("contact_authorization");
        if !self.supports_shared_contacts() {
            let evidence = SqliteRequestJournal::read_stakeholder_contact_evidence(&self.root);
            let contacts = evidence
                .as_ref()
                .map(|evidence| local_contacts(&policies, evidence));
            let mut warning = "Stakeholder contact: shared exclusion is unavailable; this post is permitted with a local-only warning. Last contact on other machines is unknown.".to_string();
            match contacts {
                Ok(contacts) => {
                    for line in render_status(&json!({"state":"local_warning","contacts":contacts}))
                    {
                        warning.push('\n');
                        warning.push_str(&line);
                    }
                }
                Err(error) => warning.push_str(&format!(" Last local contact unknown: {error}")),
            }
            prepared.warning = Some(warning);
            return Ok(prepared);
        }
        let id = if let Some(id) = args.get("request_id") {
            match id.as_str().and_then(|id| uuid::Uuid::parse_str(id).ok()) {
                Some(id) => id.to_string(),
                None => {
                    prepared.response = Some(refusal(
                        "invalid_request",
                        "Contact request_id must be a UUID; reuse it for an identical retry",
                        Value::Null,
                    ));
                    return Ok(prepared);
                }
            }
        } else if let Some(key) = args.get("idempotency_key").and_then(Value::as_str) {
            // UUIDv8: deterministic, namespaced SHA-256 of a thread's stable key.
            let digest = Sha256::digest(serde_json::to_vec(&json!([
                SCHEMA,
                self.project_id,
                caller.agent_id,
                key
            ]))?);
            let mut bytes: [u8; 16] = digest[..16].try_into().expect("16 digest bytes");
            bytes[6] = (bytes[6] & 0x0f) | 0x80;
            bytes[8] = (bytes[8] & 0x3f) | 0x80;
            uuid::Uuid::from_bytes(bytes).to_string()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        args.insert("request_id".into(), json!(id));
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(args)?));
        let payload = json!({"schema":SCHEMA,"project_id":self.project_id,
            "registration_id":caller.agent_id,"request_id":id,
            "channel":args.get("channel"),"payload_sha256":digest,"policies":policies});
        let journal = SqliteRequestJournal::open(&self.root)?;
        let local_id = format!("stakeholder-reservation:{id}");
        if let Err(error) = journal.prepare(&local_id, "stakeholder_reservation", &payload) {
            prepared.response = Some(refusal(
                if matches!(error, JournalError::IdempotencyConflict { .. }) {
                    "idempotency_conflict"
                } else {
                    "authority_unavailable"
                },
                &error.to_string(),
                json!({"request_id":id}),
            ));
            return Ok(prepared);
        }
        let result = self.cloud.as_ref().expect("negotiated Cloud").post(
            &format!("{BASE}/reserve"),
            &payload,
            BUDGET,
        );
        let response = match result {
            Ok(value) if value["schema"] == SCHEMA => value,
            _ => {
                prepared.response = Some(refusal(
                    "outcome_unknown",
                    "Shared contact admission is unconfirmed. Do not post; reconcile this request_id before retrying.",
                    json!({"request_id":id}),
                ));
                return Ok(prepared);
            }
        };
        journal.record_result(&local_id, &response, None)?;
        if response["code"] == "acquired"
            && response["authorization"]["operation_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
            && response["authorization"]["token"]
                .as_str()
                .is_some_and(|token| !token.is_empty())
        {
            args.insert(
                "contact_authorization".into(),
                response["authorization"].clone(),
            );
            prepared.failure_context = Some(format!(
                "Stakeholder post request_id {id} is unconfirmed; reconcile this same ID before retrying"
            ));
            if let Some(warnings) = response["warnings"]
                .as_array()
                .filter(|warnings| !warnings.is_empty())
            {
                prepared.warning = Some(
                    warnings
                        .iter()
                        .map(|value| {
                            value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| value.to_string())
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
        } else if response["code"] == "completed" && response["post_receipt"]["ok"] == true {
            // A reconciled replay returns its original receipt, never posts twice.
            prepared.response = Some(CallToolResult::success(vec![Content::text(
                response["post_receipt"].to_string(),
            )]));
        } else {
            let code = response["code"]
                .as_str()
                .filter(|code| *code != "acquired")
                .unwrap_or("invalid_response");
            let mut message = response["message"]
                .as_str()
                .unwrap_or("Shared contact admission refused; no post was sent")
                .to_string();
            for line in render_status(&json!({"state":"shared", "contacts":[response["detail"]]}))
                .into_iter()
                .skip(1)
            {
                message.push('\n');
                message.push_str(&line);
            }
            let mut detail = response["detail"].clone();
            if !detail.is_object() {
                detail = json!({});
            }
            detail["request_id"] = json!(id);
            prepared.response = Some(refusal(code, &message, detail));
        }
        Ok(prepared)
    }

    /// No successful local observation can grant cross-machine authority.
    fn record_local_receipt(&self, caller: &str, text: &str, result: &Value) {
        let Ok(policies) = self.policies() else {
            return;
        };
        let policies = mentioned_policies(text, policies);
        if policies.is_empty() {
            return;
        }
        let Some(receipt) = receipt_payload(result) else {
            return;
        };
        // A thread root counts even if a later reply failed. An upload begin,
        // refusal, or a transport success without a Slack receipt never counts.
        let posted = receipt
            .get("message")
            .or_else(|| receipt.pointer("/posted/0"));
        let Some(posted) = posted else {
            return;
        };
        let (Some(message_id), Some(permalink)) =
            (posted["message_id"].as_str(), posted["permalink"].as_str())
        else {
            return;
        };
        let Some((seconds, fraction)) = message_id.split_once('.') else {
            return;
        };
        let Ok(seconds) = seconds.parse::<i64>() else {
            return;
        };
        if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return;
        }
        let fraction = format!("{fraction:0<9}");
        let Ok(nanos) = fraction[..9.min(fraction.len())].parse::<u32>() else {
            return;
        };
        let Some(at) = chrono::DateTime::from_timestamp(seconds, nanos) else {
            return;
        };
        let Ok(journal) = SqliteRequestJournal::open(&self.root) else {
            return;
        };
        let id = format!("stakeholder-contact:{}", uuid::Uuid::new_v4());
        let input = json!({"policies": policies, "poster": caller});
        if journal.prepare(&id, "stakeholder_contact", &input).is_ok() {
            let evidence = json!({"code":"completed", "policies":policies,
                "last_contact_at":at.to_rfc3339(), "last_poster":caller,
                "permalink":permalink, "message_id":message_id});
            if let Err(error) = journal.record_result(&id, &evidence, None) {
                tracing::warn!(%error, "could not retain local stakeholder contact evidence");
            }
        }
    }

    pub fn status(&self) -> Value {
        let policies = match self.policies() {
            Ok(policies) => policies,
            Err(error) => return json!({"state":"unavailable", "reason":error}),
        };
        if policies.is_empty() {
            return json!({"state":"unconfigured", "contacts":[]});
        }
        if self.supports_shared_contacts() {
            let cloud = self.cloud.as_ref().expect("negotiated Cloud");
            let project = self.project_id.as_deref().expect("negotiated project");
            return match cloud.get(BASE, &[("project_id", project)], BUDGET) {
                Ok(value)
                    if value["schema"] == SCHEMA
                        && value["code"] == "completed"
                        && value["contacts"].is_array() =>
                {
                    json!({"state":"shared", "observed_at":value["observed_at"], "contacts":value["contacts"]})
                }
                _ => {
                    json!({"state":"unavailable", "reason":"Shared stakeholder status unavailable; last contact is unknown"})
                }
            };
        }
        let evidence = SqliteRequestJournal::read_stakeholder_contact_evidence(&self.root);
        match evidence {
            Ok(evidence) => {
                json!({"state":"local_warning", "reason":"Cloud/Violet shared stakeholder capability unavailable; evidence covers this machine only", "contacts":local_contacts(&policies, &evidence)})
            }
            Err(error) => json!({"state":"unavailable", "reason":error.to_string()}),
        }
    }
}

impl ProxyCallInterceptor for StakeholderContacts {
    fn prepare<'a>(
        &'a self,
        caller: &'a ProxyCaller,
        server: &'a str,
        tool: &'a str,
        arguments: Option<serde_json::Map<String, Value>>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<PreparedProxyCall>> + Send + 'a>,
    > {
        Box::pin(async move {
            if server != cmcp_core::config::VIOLET_SERVER || tool != "violet_post" {
                return Ok(PreparedProxyCall::forward(arguments));
            }
            let service = self.clone();
            let caller = caller.clone();
            tokio::task::spawn_blocking(move || service.prepare_post(&caller, arguments)).await?
        })
    }
    fn observes_result(&self, server: &str, tool: &str) -> bool {
        server == cmcp_core::config::VIOLET_SERVER && tool == "violet_post"
    }

    fn call_succeeded<'a>(
        &'a self,
        event: ProxyCallEvent<'a>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let Some(text) = event
                .arguments
                .as_ref()
                .and_then(root_text)
                .filter(|text| mentioned_users(text).next().is_some())
            else {
                return;
            };
            let service = self.clone();
            let caller = event.caller.agent_id.clone();
            let text = text.to_owned();
            let result = event.result.clone();
            // Await readback consistency, but keep config and SQLite I/O off
            // the executor. A failed local stamp cannot undo an upstream post.
            if let Err(error) = tokio::task::spawn_blocking(move || {
                service.record_local_receipt(&caller, &text, &result);
            })
            .await
            {
                tracing::warn!(%error, "could not retain local stakeholder contact evidence");
            }
        })
    }
}

pub async fn install(engine: &cmcp_core::ProxyEngine, root: &Path) {
    engine
        .set_call_interceptor(Arc::new(StakeholderContacts::from_project(root)))
        .await;
}

fn root_text(args: &serde_json::Map<String, Value>) -> Option<&str> {
    if args
        .get("reply_to")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return None;
    }
    match args
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("message")
    {
        "message" | "thread" => args.get("text")?.as_str(),
        "file" => args.get("initial_comment")?.as_str(),
        "file_external" if args.get("step").and_then(Value::as_str) == Some("complete") => {
            args.get("initial_comment")?.as_str()
        }
        _ => None,
    }
}

fn mentioned_users(text: &str) -> impl Iterator<Item = &str> {
    text.split("<@").skip(1).filter_map(|mention| {
        let (mention, _) = mention.split_once('>')?;
        let id = mention.split('|').next()?;
        (id.len() > 1
            && matches!(id.as_bytes()[0], b'U' | b'W')
            && id.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then_some(id)
    })
}

fn mentioned_policies(
    text: &str,
    policies: Vec<StakeholderContactPolicy>,
) -> Vec<StakeholderContactPolicy> {
    policies
        .into_iter()
        .filter(|policy| mentioned_users(text).any(|user| user == policy.user_id))
        .collect()
}

fn receipt_payload(result: &Value) -> Option<Value> {
    result.get("structuredContent").cloned().or_else(|| {
        result["content"]
            .as_array()?
            .iter()
            .find_map(|item| serde_json::from_str(item["text"].as_str()?).ok())
    })
}

fn local_contacts(policies: &[StakeholderContactPolicy], evidence: &[Value]) -> Vec<Value> {
    policies
        .iter()
        .map(|policy| {
            let record = evidence
                .iter()
                .filter(|record| {
                    record["policies"].as_array().is_some_and(|items| {
                        items.iter().any(|item| {
                            item["workspace_id"] == policy.workspace_id
                                && item["user_id"] == policy.user_id
                        })
                    })
                })
                .max_by_key(|record| record["last_contact_at"].as_str().unwrap_or(""));
            let next = record
                .and_then(|record| {
                    chrono::DateTime::parse_from_rfc3339(record["last_contact_at"].as_str()?).ok()
                })
                .and_then(|at| {
                    at.checked_add_signed(chrono::Duration::try_seconds(
                        policy.minimum_interval_secs as i64,
                    )?)
                })
                .map(|at| at.to_rfc3339());
            json!({"workspace_id":policy.workspace_id, "user_id":policy.user_id,
            "last_contact_at":record.map(|r| &r["last_contact_at"]),
            "last_poster":record.map(|r| &r["last_poster"]),
            "permalink":record.map(|r| &r["permalink"]), "next_allowed_at":next,
            "owner":null, "evidence":"local_only"})
        })
        .collect()
}

fn refusal(code: &str, message: &str, detail: Value) -> CallToolResult {
    CallToolResult::error(vec![Content::text(
        json!({"ok":false,
        "error":{"code":code,"message":message,"retryable":false,"detail":detail}})
        .to_string(),
    )])
}

pub fn render_status(status: &Value) -> Vec<String> {
    let state = status["state"].as_str().unwrap_or("unavailable");
    if matches!(state, "unconfigured" | "not_requested") {
        return Vec::new();
    }
    let mut lines = vec![format!("Stakeholder contacts [{state}]")];
    if let Some(reason) = status["reason"].as_str() {
        lines.push(format!("  {reason}"));
    }
    for contact in status["contacts"].as_array().into_iter().flatten() {
        let text = |key: &str| contact[key].as_str().unwrap_or("unknown");
        lines.push(format!(
            "  {} / {} | last {} by {}",
            text("workspace_id"),
            text("user_id"),
            text("last_contact_at"),
            text("last_poster")
        ));
        lines.push(format!(
            "    Next allowed {} | {}",
            text("next_allowed_at"),
            text("permalink")
        ));
        lines.push(format!(
            "    Owner {}",
            contact["owner"]["registration_id"]
                .as_str()
                .unwrap_or("unknown/unclaimed")
        ));
    }
    lines
}

#[cfg(test)]
#[path = "stakeholder_contacts_tests.rs"]
mod tests;
