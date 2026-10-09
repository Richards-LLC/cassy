//! The hub boundary of `cas violet`: one trait for a `tools/call` and one for
//! the direct upload, the decoding of a Violet receipt, and the named error.
//!
//! The production [`ProxyHub`] dispatches through [`cmcp_core::ProxyEngine`]
//! with the merged `[servers.violet]` registration, so the hub URL, bearer,
//! Vercel bypass and allowlist policy are exactly the ones the MCP proxy uses.
//! Tests drive the command against a scripted [`Hub`] instead.

use serde_json::{Value, json};

use super::files::LocalFile;

/// One Violet call and one direct upload. `call` returns the decoded receipt,
/// whether it says `ok: true` or `ok: false`; only a transport or decoding
/// failure is an `Err`.
pub trait Hub {
    fn call(&mut self, tool: &str, args: Value) -> Result<Value, VioletError>;
    /// Send the file's bytes, read from disk, to a `file_external` upload URL.
    fn upload(&mut self, upload_url: &str, file: &LocalFile) -> Result<(), VioletError>;
}

/// A named failure. Hub codes pass through unchanged; the command's own codes
/// follow the hub's shell client (`missing_credential`, `invalid_token`,
/// `hub_unreachable`, `hub_bad_response`, `local_request_failed`).
#[derive(Debug, Clone, PartialEq)]
pub struct VioletError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    /// The hub's own error receipt, printed verbatim under `--json`. A thread
    /// failure keeps `posted[]` and `failed_index` here.
    pub envelope: Option<Value>,
    /// Slack uploads begun before the failure; they may need inspection.
    pub file_ids: Vec<String>,
}

impl VioletError {
    pub fn local(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            retryable: false,
            envelope: None,
            file_ids: Vec::new(),
        }
    }

    /// The error a hub receipt with `ok: false` carries, keeping the receipt.
    pub fn from_receipt(receipt: Value) -> Self {
        let code = receipt
            .pointer("/error/code")
            .and_then(Value::as_str)
            .filter(|code| !code.trim().is_empty())
            .unwrap_or("violet_post_failed")
            .to_string();
        let message = receipt
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(excerpt)
            .unwrap_or_else(|| "Violet refused the request without a message".to_string());
        let retryable = receipt
            .pointer("/error/retryable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Self {
            code,
            message,
            retryable,
            envelope: Some(receipt),
            file_ids: Vec::new(),
        }
    }

    /// The single JSON document `--json` prints for this failure.
    pub fn to_json(&self) -> Value {
        let mut value = self.envelope.clone().unwrap_or_else(|| {
            json!({
                "ok": false,
                "error": {
                    "code": self.code,
                    "message": self.message,
                    "retryable": self.retryable,
                }
            })
        });
        if !self.file_ids.is_empty() {
            value["uploads_begun"] = json!(self.file_ids);
        }
        value
    }
}

impl std::fmt::Display for VioletError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if !self.file_ids.is_empty() {
            write!(
                f,
                " (Slack upload {} was begun; check the channel before posting again)",
                self.file_ids.join(", ")
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for VioletError {}

/// Decode a Violet receipt from the MCP tool result that carries it
/// (`structuredContent`, or JSON text in `content[]`) or from a bare receipt.
/// The receipt is returned whether `ok` is true or false; anything without a
/// boolean `ok` is `hub_bad_response`.
pub fn decode_tool_result(result: &Value) -> Result<Value, VioletError> {
    if has_ok(result) {
        return Ok(result.clone());
    }
    if let Some(structured) = result.get("structuredContent").filter(|v| has_ok(v)) {
        return Ok(structured.clone());
    }
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| {
            content
                .iter()
                .find_map(|item| item.get("text").and_then(Value::as_str))
        })
        .ok_or_else(|| {
            VioletError::local(
                "hub_bad_response",
                "the hub returned a tool result without a receipt; a write may be unconfirmed, \
                 inspect the channel before retrying",
            )
        })?;
    let receipt: Value = serde_json::from_str(text).map_err(|_| {
        VioletError::local(
            "hub_bad_response",
            "the hub's tool text is not JSON; a write may be unconfirmed, inspect the channel \
             before retrying",
        )
    })?;
    if !has_ok(&receipt) {
        return Err(VioletError::local(
            "hub_bad_response",
            "the hub's receipt has no ok flag; a write may be unconfirmed, inspect the channel \
             before retrying",
        ));
    }
    Ok(receipt)
}

fn has_ok(value: &Value) -> bool {
    value.get("ok").is_some_and(Value::is_boolean)
}

/// Call the hub and turn an `ok: false` receipt into its error.
pub fn call_ok(hub: &mut dyn Hub, tool: &str, args: Value) -> Result<Value, VioletError> {
    let receipt = hub.call(tool, args)?;
    if receipt.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(receipt)
    } else {
        Err(VioletError::from_receipt(receipt))
    }
}

fn excerpt(text: &str) -> String {
    let text = text.trim().replace('\n', " ");
    if text.chars().count() <= 300 {
        return text;
    }
    format!("{}…", text.chars().take(300).collect::<String>())
}

/// Map a proxy connection failure code to the command's named error. Values
/// are never part of the code; a missing variable is named, never printed.
pub fn connect_error(code: &str) -> VioletError {
    if let Some(name) = code.strip_prefix("missing_credential_env:") {
        return VioletError::local(
            "missing_credential",
            format!("credential environment variable {name} is unset; run `cas integrate violet`"),
        );
    }
    match code {
        "authentication_required" => VioletError::local(
            "invalid_token",
            "the hub rejected this machine's bearer (HTTP 401): VIOLET_SLACK_TOKEN is unknown or \
             revoked, or VIOLET_VERCEL_BYPASS is wrong. Run `cas integrate violet`, then `cas doctor`",
        ),
        "unexpected_content_type" => VioletError::local(
            "hub_bad_response",
            "the hub answered with something other than MCP; check the [servers.violet] URL with \
             `cas doctor`",
        ),
        other => VioletError::local(
            "hub_unreachable",
            format!(
                "could not reach the Violet hub ({other}); check connectivity and the \
                 [servers.violet] URL with `cas doctor`"
            ),
        ),
    }
}

// ---------------------------------------------------------------------------
// Production transport
// ---------------------------------------------------------------------------

/// The live hub, reached through the proxy engine with the same registration
/// `cas serve` loads. Construct and drop it outside any Tokio runtime.
#[cfg(feature = "mcp-proxy")]
pub struct ProxyHub {
    runtime: tokio::runtime::Runtime,
    engine: Option<cmcp_core::ProxyEngine>,
    caller: cmcp_core::ProxyCaller,
}

#[cfg(feature = "mcp-proxy")]
impl ProxyHub {
    /// Connect to `[servers.violet]` only. Inside a Cassy project the
    /// project's `.cas/proxy.toml` and worker policy apply, exactly as for
    /// the MCP proxy; elsewhere the machine registration is used.
    pub fn connect(cas_root: Option<&std::path::Path>) -> Result<Self, VioletError> {
        use cmcp_core::config::{ServerConfig, VIOLET_SERVER};

        let not_configured = |detail: String| {
            VioletError::local(
                "not_configured",
                format!("{detail}; run `cas integrate violet`, then `cas doctor`"),
            )
        };
        let mut config = match cas_root {
            Some(root) => crate::mcp::load_proxy_config_for_process(root),
            None => cmcp_core::config::Config::load_merged(None),
        }
        .map_err(|error| {
            not_configured(format!(
                "the proxy configuration could not be read ({error:#})"
            ))
        })?;
        let mut server = config.servers.remove(VIOLET_SERVER).ok_or_else(|| {
            not_configured(format!(
                "Violet is not registered here (no [servers.{VIOLET_SERVER}] in the proxy configuration, \
                 or this worker's policy withholds it)"
            ))
        })?;

        // Resolve credential references the way the proxy does (VIOLET_*
        // first, the legacy names second), from the environment and then the
        // machine credentials file. A worker never receives a variable its
        // factory policy reserves for the supervisor.
        let policy = cas_root.and_then(|root| crate::mcp::worker_proxy_policy(root).ok().flatten());
        let mut machine =
            crate::cli::integrate::violet::machine_credential_values().unwrap_or_default();
        if let Some(policy) = &policy {
            machine.retain(|name, _| !policy.denies_env(name));
        }
        let resolve = |value: &mut String| -> Result<(), VioletError> {
            if let Some(name) = value.strip_prefix("env:").filter(|name| !name.is_empty()) {
                let resolved = cmcp_core::config::violet_credential_value(name, |candidate| {
                    std::env::var(candidate)
                        .ok()
                        .filter(|value| !value.trim().is_empty())
                        .or_else(|| machine.get(candidate).cloned())
                })
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| connect_error(&format!("missing_credential_env:{name}")))?;
                *value = resolved;
            }
            Ok(())
        };
        match &mut server {
            ServerConfig::Http { auth, headers, .. } | ServerConfig::Sse { auth, headers, .. } => {
                if let Some(auth) = auth.as_mut() {
                    resolve(auth)?;
                }
                for value in headers.values_mut() {
                    resolve(value)?;
                }
            }
            ServerConfig::Stdio { .. } => {}
        }
        config.servers = std::collections::HashMap::from([(VIOLET_SERVER.to_string(), server)]);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                VioletError::local(
                    "local_request_failed",
                    format!("could not start a runtime: {error}"),
                )
            })?;
        let engine = runtime
            .block_on(cmcp_core::ProxyEngine::from_configs(config.servers.clone()))
            .map_err(|error| {
                VioletError::local(
                    "hub_unreachable",
                    format!("could not start the proxy: {error}"),
                )
            })?;
        let health = runtime.block_on(engine.health_snapshot());
        let state = health
            .servers
            .iter()
            .find(|server| server.name == VIOLET_SERVER)
            .map(|server| {
                (
                    server.state == cmcp_core::UpstreamState::Healthy,
                    server.last_error_code.clone(),
                )
            });
        match state {
            Some((true, _)) => {}
            Some((false, code)) => {
                runtime.block_on(engine.shutdown());
                return Err(connect_error(
                    code.as_deref().unwrap_or("connection_failed"),
                ));
            }
            None => {
                runtime.block_on(engine.shutdown());
                return Err(connect_error("connection_failed"));
            }
        }
        runtime.block_on(crate::mcp::install_proxy_policy(&engine, &config));
        Ok(Self {
            runtime,
            engine: Some(engine),
            caller: cli_caller(),
        })
    }
}

/// The identity this one-shot process presents to the proxy policy: the
/// factory role it was spawned with, if any, otherwise a plain session.
#[cfg(feature = "mcp-proxy")]
fn cli_caller() -> cmcp_core::ProxyCaller {
    let role = std::env::var("CAS_AGENT_ROLE")
        .ok()
        .and_then(|role| role.parse::<crate::types::AgentRole>().ok())
        .unwrap_or_default();
    let agent_id = std::env::var("CAS_AGENT_NAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "cas-violet-cli".to_string());
    cmcp_core::ProxyCaller {
        agent_id,
        role,
        session_id: format!("cas-violet-{}", std::process::id()),
        factory_session: None,
        active_task_ids: Vec::new(),
    }
}

#[cfg(feature = "mcp-proxy")]
impl Hub for ProxyHub {
    fn call(&mut self, tool: &str, args: Value) -> Result<Value, VioletError> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| VioletError::local("hub_unreachable", "the proxy engine has stopped"))?;
        let arguments = args.as_object().cloned();
        let result = self.runtime.block_on(engine.call_tool(
            &self.caller,
            cmcp_core::config::VIOLET_SERVER,
            tool,
            arguments,
        ));
        match result {
            Ok(result) => decode_tool_result(&result),
            Err(error) => {
                let detail = cmcp_core::describe_upstream_call_error(&error);
                if detail.contains("proxy policy denied") {
                    return Err(VioletError::local(
                        "denied_by_policy",
                        format!("{detail}. Run `cas integrate violet` and re-check `cas doctor`"),
                    ));
                }
                let lower = detail.to_ascii_lowercase();
                if lower.contains("401") || lower.contains("unauthorized") {
                    return Err(connect_error("authentication_required"));
                }
                Err(VioletError::local(
                    "hub_unreachable",
                    format!(
                        "the call to the hub failed ({detail}); a write may be unconfirmed, \
                         inspect the channel before retrying"
                    ),
                ))
            }
        }
    }

    fn upload(&mut self, upload_url: &str, file: &LocalFile) -> Result<(), VioletError> {
        upload_from_disk(upload_url, file)
    }
}

#[cfg(feature = "mcp-proxy")]
impl Drop for ProxyHub {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            self.runtime.block_on(engine.shutdown());
        }
    }
}

/// Stream the file from disk to Slack's upload URL. The URL is a short-lived
/// capability: it is never printed, and it receives no hub credentials.
#[cfg_attr(not(feature = "mcp-proxy"), allow(dead_code))]
pub fn upload_from_disk(upload_url: &str, file: &LocalFile) -> Result<(), VioletError> {
    use std::io::Read;
    use std::time::Duration;

    if !crate::artifacts::slack::safe_url(upload_url) {
        return Err(VioletError::local(
            "hub_bad_response",
            "the hub returned an upload location that is not https",
        ));
    }
    let reader = std::fs::File::open(&file.path).map_err(|_| {
        VioletError::local(
            "local_request_failed",
            format!("could not reopen {} for upload", file.path.display()),
        )
    })?;
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(300))
        .timeout_write(Duration::from_secs(300))
        .build();
    let response = agent
        .post(upload_url)
        .set("Content-Type", "application/octet-stream")
        .set("Content-Length", &file.size_bytes.to_string())
        .send(reader.take(file.size_bytes));
    match response {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, _)) => Err(VioletError::local(
            "violet_upload_failed",
            format!(
                "the Slack upload URL answered {status} for {}; nothing was shared",
                file.filename
            ),
        )),
        Err(ureq::Error::Transport(_)) => Err(VioletError::local(
            "violet_upload_failed",
            format!(
                "network error sending {} to the Slack upload URL; nothing was shared",
                file.filename
            ),
        )),
    }
}
