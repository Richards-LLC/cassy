//! Shared stakeholder admission belongs to Cloud and the trusted Violet relay.
//! The existing request journal holds only local receipt evidence for warnings.

use std::{path::{Path, PathBuf}, sync::Arc, time::Duration};
use cmcp_core::{PreparedProxyCall, ProxyCallEvent, ProxyCallInterceptor, ProxyCaller};
use rmcp::model::{CallToolResult, Content};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use super::external_work::{CloudCoordinationPort, HttpCloudCoordinationPort, SqliteRequestJournal};
use crate::config::{Config, StakeholderContactPolicy};

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
            .ok().and_then(|config| {
                config.token.filter(|token| !token.is_empty()).map(|token| {
                    Arc::new(HttpCloudCoordinationPort::new(config.endpoint, token)) as Arc<dyn CloudCoordinationPort>
                })
            });
        Self { root: root.into(), cloud, project_id: crate::cloud::resolve_canonical_id(root) }
    }

    fn policies(&self) -> Result<Vec<StakeholderContactPolicy>, String> {
        let policies = Config::load(&self.root).map_err(|e| e.to_string())?
            .slack.unwrap_or_default().stakeholder_contacts;
        for policy in &policies { policy.validate()?; }
        Ok(policies)
    }

    fn supports_shared_contacts(&self) -> bool {
        let (Some(cloud), Some(project)) = (&self.cloud, &self.project_id) else { return false; };
        cloud.get(&format!("{BASE}/capabilities"), &[("project_id", project)], BUDGET)
            .is_ok_and(|value| value["schema"] == SCHEMA
                && value["atomic_reservations"] == true && value["violet_settlement"] == true)
    }

    fn prepare_post(&self, caller: &ProxyCaller, arguments: Option<serde_json::Map<String, Value>>) -> anyhow::Result<PreparedProxyCall> {
        // Replaced by shared capability/admission below after the red test.
        let _ = caller;
        Ok(PreparedProxyCall::forward(arguments))
    }

    /// No successful local observation can grant cross-machine authority.
    fn record_local_receipt(&self, event: ProxyCallEvent<'_>) {
        if event.server != cmcp_core::config::VIOLET_SERVER || event.tool != "violet_post" { return; }
        let Some(args) = event.arguments.as_ref() else { return; };
        let Ok(policies) = self.policies() else { return; };
        let policies = mentioned_policies(args, policies);
        if policies.is_empty() { return; }
        let Some(receipt) = receipt_payload(event.result) else { return; };
        // A thread root counts even if a later reply failed. An upload begin,
        // refusal, or a transport success without a Slack receipt never counts.
        let posted = receipt.get("message").or_else(|| receipt.pointer("/posted/0"));
        let Some(posted) = posted else { return; };
        let (Some(message_id), Some(permalink)) = (
            posted["message_id"].as_str(), posted["permalink"].as_str()
        ) else { return; };
        let Some((seconds, fraction)) = message_id.split_once('.') else { return; };
        let Ok(seconds) = seconds.parse::<i64>() else { return; };
        let fraction = format!("{fraction:0<9}");
        let Ok(nanos) = fraction[..9.min(fraction.len())].parse::<u32>() else { return; };
        let Some(at) = chrono::DateTime::from_timestamp(seconds, nanos) else { return; };
        let Ok(journal) = SqliteRequestJournal::open(&self.root) else { return; };
        let id = format!("stakeholder-contact:{}", uuid::Uuid::new_v4());
        let input = json!({"policies": policies, "poster": event.caller.agent_id});
        if journal.prepare(&id, "stakeholder_contact", &input).is_ok() {
            let evidence = json!({"code":"completed", "policies":policies,
                "last_contact_at":at.to_rfc3339(), "last_poster":event.caller.agent_id,
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
        if policies.is_empty() { return json!({"state":"unconfigured", "contacts":[]}); }
        if self.supports_shared_contacts() {
            let cloud = self.cloud.as_ref().expect("negotiated Cloud");
            let project = self.project_id.as_deref().expect("negotiated project");
            return match cloud.get(BASE, &[("project_id", project)], BUDGET) {
                Ok(value) if value["schema"] == SCHEMA && value["code"] == "completed"
                    && value["contacts"].is_array() => json!({"state":"shared", "observed_at":value["observed_at"], "contacts":value["contacts"]}),
                _ => json!({"state":"unavailable", "reason":"Shared stakeholder status unavailable; last contact is unknown"}),
            };
        }
        let evidence = SqliteRequestJournal::open(&self.root)
            .and_then(|journal| journal.stakeholder_contact_evidence());
        match evidence {
            Ok(evidence) => json!({"state":"local_warning", "reason":"Cloud/Violet shared stakeholder capability unavailable; evidence covers this machine only", "contacts":local_contacts(&policies, &evidence)}),
            Err(error) => json!({"state":"unavailable", "reason":error.to_string()}),
        }
    }
}

impl ProxyCallInterceptor for StakeholderContacts {
    fn prepare<'a>(&'a self, caller: &'a ProxyCaller, server: &'a str, tool: &'a str,
        arguments: Option<serde_json::Map<String, Value>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output=anyhow::Result<PreparedProxyCall>> + Send + 'a>> {
        Box::pin(async move {
            if server != cmcp_core::config::VIOLET_SERVER || tool != "violet_post" {
                return Ok(PreparedProxyCall::forward(arguments));
            }
            let service = self.clone();
            let caller = caller.clone();
            tokio::task::spawn_blocking(move || service.prepare_post(&caller, arguments)).await?
        })
    }
    fn call_succeeded(&self, event: ProxyCallEvent<'_>) { self.record_local_receipt(event); }
}

pub async fn install(engine: &cmcp_core::ProxyEngine, root: &Path) {
    engine.set_call_interceptor(Arc::new(StakeholderContacts::from_project(root))).await;
}

fn root_text(args: &serde_json::Map<String, Value>) -> Option<&str> {
    if args.get("reply_to").is_some_and(|value| !value.is_null()) { return None; }
    match args.get("kind").and_then(Value::as_str).unwrap_or("message") {
        "message" | "thread" => args.get("text")?.as_str(),
        "file" => args.get("initial_comment")?.as_str(),
        "file_external" if args.get("action").and_then(Value::as_str) == Some("complete") => args.get("initial_comment")?.as_str(),
        _ => None,
    }
}

fn mentioned_policies(args: &serde_json::Map<String, Value>, policies: Vec<StakeholderContactPolicy>) -> Vec<StakeholderContactPolicy> {
    let Some(text) = root_text(args) else { return Vec::new(); };
    policies.into_iter().filter(|policy| text.contains(&format!("<@{}>", policy.user_id))
        || text.contains(&format!("<@{}|", policy.user_id))).collect()
}

fn receipt_payload(result: &Value) -> Option<Value> {
    result.get("structuredContent").cloned().or_else(|| result["content"].as_array()?.iter().find_map(|item| {
        serde_json::from_str(item["text"].as_str()?).ok()
    }))
}

fn local_contacts(policies: &[StakeholderContactPolicy], evidence: &[Value]) -> Vec<Value> {
    policies.iter().map(|policy| {
        let record = evidence.iter().filter(|record| record["policies"].as_array().is_some_and(|items| items.iter().any(|item| {
            item["workspace_id"] == policy.workspace_id && item["user_id"] == policy.user_id
        }))).max_by_key(|record| record["last_contact_at"].as_str().unwrap_or(""));
        let next = record.and_then(|record| chrono::DateTime::parse_from_rfc3339(record["last_contact_at"].as_str()?).ok())
            .and_then(|at| at.checked_add_signed(chrono::Duration::try_seconds(policy.minimum_interval_secs as i64)?))
            .map(|at| at.to_rfc3339());
        json!({"workspace_id":policy.workspace_id, "user_id":policy.user_id,
            "last_contact_at":record.map(|r| &r["last_contact_at"]),
            "last_poster":record.map(|r| &r["last_poster"]),
            "permalink":record.map(|r| &r["permalink"]), "next_allowed_at":next,
            "owner":null, "evidence":"local_only"})
    }).collect()
}

fn refusal(code: &str, message: &str, detail: Value) -> CallToolResult {
    CallToolResult::error(vec![Content::text(json!({"ok":false,
        "error":{"code":code,"message":message,"retryable":false,"detail":detail}}).to_string())])
}

pub fn render_status(status: &Value) -> Vec<String> {
    let state = status["state"].as_str().unwrap_or("unavailable");
    if matches!(state, "unconfigured" | "not_requested") { return Vec::new(); }
    let mut lines = vec![format!("Stakeholder contacts [{state}]")];
    if let Some(reason) = status["reason"].as_str() { lines.push(format!("  {reason}")); }
    for contact in status["contacts"].as_array().into_iter().flatten() {
        let text = |key: &str| contact[key].as_str().unwrap_or("unknown");
        lines.push(format!("  {} / {} | last {} by {}", text("workspace_id"), text("user_id"), text("last_contact_at"), text("last_poster")));
        lines.push(format!("    Next allowed {} | {}", text("next_allowed_at"), text("permalink")));
        lines.push(format!("    Owner {}", contact["owner"]["registration_id"].as_str().unwrap_or("unknown/unclaimed")));
    }
    lines
}

#[cfg(test)]
#[path = "stakeholder_contacts_tests.rs"]
mod tests;
