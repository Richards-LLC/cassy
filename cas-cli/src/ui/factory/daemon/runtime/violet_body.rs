//! Fetch a claimed Slack message through Violet, without a local body cache.
use serde_json::{Value, json};
use super::violet_reader::{BodySource, NamedRefusal};

/// The read port returns Violet's original tool result, kept in memory only.
pub(crate) struct VioletBodySource<F>(pub F);

impl<F: Fn(Value) -> Result<Value, NamedRefusal>> BodySource for VioletBodySource<F> {
    fn fetch_body(&self, target: &Value) -> Result<String, NamedRefusal> {
        let field = |value: &Value| value.as_str().filter(|text| !text.trim().is_empty()).map(str::to_string);
        let (Some(workspace), Some(channel), Some(root), Some(message)) = (
            field(&target["external_ref"]["workspace_id"]), field(&target["external_ref"]["channel_id"]),
            field(&target["external_ref"]["root_ts"]), field(&target["message_ts"]),
        ) else {
            return Err(NamedRefusal::new("invalid_target", "Slack target lacks workspace, channel, root or message"));
        };
        if target["external_ref"]["provider"] != "slack" {
            return Err(NamedRefusal::new("invalid_target", "Target provider is not Slack"));
        }
        // external_ref excludes message_id/thread_id/cursors. include_threads
        // is required: false is an identity-only read with no message text.
        let result = (self.0)(json!({
            "external_ref":{"workspace_id":workspace,"channel_id":channel,"message_ts":message},
            "include_threads":true,"include_files":false,"include_channels":false
        }))?;
        if result["isError"] == true {
            return Err(NamedRefusal::new("violet_refused", "Violet refused the target read"));
        }
        let receipt = if let Some(value) = result.get("structuredContent").filter(|v| v.is_object()) {
            value.clone()
        } else {
            result["content"].as_array().into_iter().flatten()
                .filter_map(|item| item["text"].as_str())
                .find_map(|text| serde_json::from_str::<Value>(text).ok())
                .ok_or_else(|| NamedRefusal::new("invalid_response", "Violet returned no readable receipt"))?
        };
        if receipt["ok"] != true {
            return Err(NamedRefusal::new("violet_refused", "Violet refused the target read"));
        }
        if receipt["schema_version"] != 1 {
            return Err(NamedRefusal::new("invalid_response", "Violet returned an unsupported receipt"));
        }
        let identity = &receipt["identity"];
        if identity["workspace_id"] != workspace || identity["channel_id"] != channel || identity["root_thread_ts"] != root {
            return Err(NamedRefusal::new("identity_mismatch", "Violet identity does not match the claimed Slack target"));
        }
        // thread_roots is truncated context, never the selected message body.
        let selected = receipt["messages"].as_array().into_iter().flatten()
            .find(|item| item["message_id"] == message)
            .ok_or_else(|| NamedRefusal::new("message_missing", "Violet did not return the claimed message"))?;
        if selected["thread_id"].as_str().is_some_and(|thread| thread != root) {
            return Err(NamedRefusal::new("identity_mismatch", "Violet message belongs to another thread"));
        }
        selected["text"].as_str().map(str::to_string)
            .ok_or_else(|| NamedRefusal::new("message_missing", "Violet returned no text for the claimed message"))
    }
}

/// Called inside the daemon's blocking poll. Only the explicit supervisor
/// identity may read Violet; no environment-inferred caller or body persistence.
#[cfg(feature = "mcp-proxy")]
pub(crate) fn read(cas_dir: &std::path::Path, registration: &str, session: &str, args: Value) -> Result<Value, NamedRefusal> {
    use crate::coordination::external_work::CALL_REMOTE_BUDGET;
    use super::violet_activity::ProxyChannelReader;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()
        .map_err(|_| NamedRefusal::new("violet_unavailable", "Could not start the Violet reader"))?;
    runtime.block_on(async {
        let caller = cmcp_core::ProxyCaller {
            agent_id:registration.into(), role:cas_types::AgentRole::Supervisor,
            session_id:registration.into(), factory_session:Some(session.into()), active_task_ids:Vec::new(),
        };
        let reader = tokio::time::timeout(CALL_REMOTE_BUDGET, ProxyChannelReader::connect(cas_dir, caller)).await
            .map_err(|_| NamedRefusal::new("violet_timeout", "Violet connection timed out"))?
            .map_err(|_| NamedRefusal::new("violet_unavailable", "Violet is unavailable; check its integration and credentials"))?;
        let response = tokio::time::timeout(CALL_REMOTE_BUDGET, reader.read_target(args)).await;
        let _ = tokio::time::timeout(CALL_REMOTE_BUDGET, reader.shutdown()).await;
        response.map_err(|_| NamedRefusal::new("violet_timeout", "Violet target read timed out"))?
            .map_err(|_| NamedRefusal::new("violet_unavailable", "Violet target read failed"))
    })
}

#[cfg(not(feature = "mcp-proxy"))]
pub(crate) fn read(_: &std::path::Path, _: &str, _: &str, _: Value) -> Result<Value, NamedRefusal> {
    Err(NamedRefusal::new("violet_unavailable", "This build does not include the Violet proxy"))
}
