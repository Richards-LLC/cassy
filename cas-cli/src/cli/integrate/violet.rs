//! `cas integrate violet` — one command that makes the Violet Slack
//! hub reachable from every project on a machine (task **cas-8fad**).
//!
//! ## What this replaces
//!
//! Before this command, reaching the hub meant hand-copying a gitignored
//! `.cas/proxy.toml` into each project plus hand-editing two harness config
//! files, so a second machine or a new teammate silently had no Slack path.
//! This command writes the registration **once, at machine scope**
//! (`~/.config/code-mode-mcp/config.toml`), which
//! [`cmcp_core::config::Config::load_merged`] already merges beneath a project
//! `.cas/proxy.toml` and `cas serve` already loads.
//!
//! ## Credential rule
//!
//! Every generated proxy or harness artifact references a credential by
//! environment variable **name** (`auth = "env:VIOLET_SLACK_TOKEN_<LABEL>"`,
//! `x-vercel-protection-bypass = "env:VIOLET_VERCEL_BYPASS"`). Provisioning
//! writes the values only to the private machine credentials file; a value is
//! never read into a report, printed, or embedded in an error. The only fact
//! this module publishes about a variable is [`EnvState`] — set, empty, or
//! unset.
//!
//! ## Seams
//!
//! [`EnvLookup`] and [`HubProbe`] are traits so the command and the doctor
//! check are exercised against a fake environment and a fake `tools/list`
//! rather than a live hub. [`MachinePaths`] is passed in for the same reason:
//! a test points it at a `tempdir` and asserts on real written files.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use url::Url;

use cmcp_core::config::{
    Config as ProxyConfig, ExternalToolConfig, ServerConfig, VIOLET_BYPASS_HEADER,
    VIOLET_DEFAULT_BYPASS_ENV, VIOLET_SERVER, VIOLET_TOOLS, canonical_violet_credential_name,
    violet_compatibility, violet_credential_value, violet_hub_url,
};

use crate::cloud::{CloudConfig, DeviceConfig};

use super::fs as ifs;
use super::types::{IntegrationAction, IntegrationOutcome, IntegrationStatus, Platform};

/// Where an operator is told to put the two values. Named in every remedy so
/// the message is actionable without opening the onboarding doc.
pub const HUB_CLIENT_ROUTE: &str = "/api/clients";
pub const HUB_BYPASS_ROUTE: &str = "/api/bypass";
pub const HUB_CLIENT_ISSUE: &str = "violet_ps#5";
pub const VERCEL_PROJECT: &str = "violet_ps";
/// GH #1164: never `cas login`. Signing in does not mint a hub client token;
/// `cas integrate violet` does (and says so itself if minting needs a Cloud
/// session).
pub const CREDENTIALS_HINT: &str = "re-run `cas integrate violet`: it mints this machine's hub client token and \
     stores both values in the machine credentials file sourced by your login shell — see \
     docs/VIOLET_ONBOARDING.md";

// ---------------------------------------------------------------------------
// CLI surface
// ---------------------------------------------------------------------------

#[derive(Args, Debug, Clone, Default)]
pub struct VioletArgs {
    /// Environment variable holding this machine's hub bearer token.
    /// Defaults to `VIOLET_SLACK_TOKEN_<LABEL>`.
    #[arg(long, value_name = "NAME")]
    pub token_env: Option<String>,
    /// Environment variable holding the Vercel edge-protection bypass secret.
    #[arg(long, value_name = "NAME", default_value = VIOLET_DEFAULT_BYPASS_ENV)]
    pub bypass_env: String,
    /// Per-machine client label override (e.g. `LAPTOP`).
    #[arg(long, value_name = "LABEL")]
    pub label: Option<String>,
    /// Hub MCP endpoint. Only needed against a staging hub.
    #[arg(long, value_name = "URL", default_value = violet_hub_url())]
    pub url: String,
    /// Leave the Claude Code and Codex MCP registrations alone.
    #[arg(long)]
    pub no_harness: bool,
    /// Skip the authenticated `tools/list` receipt (offline setup).
    #[arg(long)]
    pub skip_verify: bool,
    /// Report what would change without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

impl VioletArgs {
    /// Bearer variable name: `--token-env` wins, then `--label`, then the
    /// hostname-derived label. A label is upper-cased and non-alphanumerics
    /// are folded to `_` so `my laptop` and `my-laptop` name the same variable.
    pub fn resolved_token_env(&self) -> String {
        if let Some(explicit) = self.token_env.as_deref().map(str::trim)
            && !explicit.is_empty()
        {
            return explicit.to_string();
        }
        let label = resolve_label(self.label.as_deref(), DeviceConfig::hostname().as_deref());
        self.resolved_token_env_for_label(&label)
    }

    fn resolved_token_env_for_label(&self, label: &str) -> String {
        if let Some(explicit) = self.token_env.as_deref().map(str::trim)
            && !explicit.is_empty()
        {
            return explicit.to_string();
        }
        format!("VIOLET_SLACK_TOKEN_{}", sanitize_label(label))
    }
}

fn sanitize_label(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn resolve_label(override_label: Option<&str>, hostname: Option<&str>) -> String {
    sanitize_label(
        override_label
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .or_else(|| hostname.map(str::trim).filter(|label| !label.is_empty()))
            .unwrap_or("unknown-host"),
    )
}

// ---------------------------------------------------------------------------
// Seams
// ---------------------------------------------------------------------------

/// Read-only view of the process environment. Implementations return the
/// *value* only so emptiness can be distinguished from absence; callers must
/// reduce it to an [`EnvState`] before it reaches a report or the terminal.
pub trait EnvLookup {
    fn get(&self, name: &str) -> Option<String>;
}

pub struct ProcessEnv;

impl EnvLookup for ProcessEnv {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// Whether a named credential variable is usable, without revealing its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvState {
    Set,
    /// Exported but empty — a distinct and common failure (a truncated
    /// credentials file line) that must not be reported as "unset".
    Empty,
    Unset,
}

impl EnvState {
    pub fn of(env: &dyn EnvLookup, name: &str) -> Self {
        match violet_credential_value(name, |candidate| env.get(candidate)) {
            Some(value) if !value.trim().is_empty() => Self::Set,
            Some(_) => Self::Empty,
            None => Self::Unset,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Empty => "set but empty",
            Self::Unset => "unset",
        }
    }

    pub fn is_usable(self) -> bool {
        matches!(self, Self::Set)
    }
}

/// Outcome of an authenticated `tools/list` against the hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum ProbeOutcome {
    /// The hub answered with this exact tool list. `schema_problems` names
    /// every way the served `violet_post` input schema would be dropped or
    /// misread by a harness (see [`violet_post_schema_problems`]); empty when
    /// the schema is usable or the hub did not offer `violet_post`.
    Tools {
        tools: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        schema_problems: Vec<String>,
    },
    /// HTTP 401. Reported with header state only, never a value.
    Unauthorized,
    /// Any other transport failure, carrying the proxy's error code.
    Unreachable { code: String },
    /// Not attempted (`--skip-verify`, or a credential is missing).
    Skipped { reason: String },
}

/// An authenticated `tools/list` against the hub.
pub trait HubProbe {
    fn list_tools(&self, server: &ServerConfig) -> ProbeOutcome;
}

/// The hub tool whose input schema is checked by [`violet_post_schema_problems`].
pub const VIOLET_POST_TOOL: &str = "violet_post";

/// `kind` values every `violet_post` schema must offer. Without `edit` and
/// `delete`, agents send `kind=message` with a `message_id`, the hub rejects
/// it, and they conclude a post cannot be changed (GH #1051).
pub const VIOLET_POST_REQUIRED_KINDS: [&str; 5] = ["message", "file", "reaction", "edit", "delete"];

/// Name every way a served `violet_post` input schema would be dropped or
/// misread by a harness; empty means usable.
///
/// Claude Code and Codex register the hub as a direct HTTP MCP server, so
/// this schema reaches agents with no Cassy layer in between (cas-96c0).
/// Claude Code drops a tool whose schema has a top-level `anyOf`, `oneOf` or
/// `allOf`, which is how `violet_post` vanished from Claude sessions until
/// violet_ps#26. A missing `kind` enum, or one without `edit`/`delete`, is
/// how agents came to believe edits were impossible.
pub fn violet_post_schema_problems(schema: &serde_json::Value) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(root) = schema.as_object() else {
        return vec!["input schema is not a JSON object".to_string()];
    };
    if root.get("type").and_then(serde_json::Value::as_str) != Some("object") {
        problems.push("input schema root is not type=object".to_string());
    }
    for combinator in ["anyOf", "oneOf", "allOf"] {
        if root.contains_key(combinator) {
            problems.push(format!(
                "input schema has a top-level {combinator}, so Claude Code drops the tool"
            ));
        }
    }
    let kinds: Option<Vec<&str>> = root
        .get("properties")
        .and_then(|properties| properties.get("kind"))
        .and_then(|kind| kind.get("enum"))
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect()
        });
    match kinds {
        None => problems.push("properties.kind is not an enum".to_string()),
        Some(kinds) => {
            let missing: Vec<&str> = VIOLET_POST_REQUIRED_KINDS
                .iter()
                .copied()
                .filter(|required| !kinds.contains(required))
                .collect();
            if !missing.is_empty() {
                problems.push(format!("kind enum lacks {}", missing.join(", ")));
            }
        }
    }
    problems
}

/// Live probe: passes the effective server unchanged to the proxy, which
/// resolves its authentication and header references in-process.
pub struct ProxyHubProbe;

impl HubProbe for ProxyHubProbe {
    fn list_tools(&self, server: &ServerConfig) -> ProbeOutcome {
        use std::collections::HashMap;
        let server = server.clone();
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                return ProbeOutcome::Unreachable {
                    code: format!("runtime_unavailable: {error}"),
                };
            }
        };
        runtime.block_on(async move {
            let engine = match cmcp_core::ProxyEngine::from_configs(HashMap::from([(
                VIOLET_SERVER.to_string(),
                server,
            )]))
            .await
            {
                Ok(engine) => engine,
                Err(error) => {
                    return ProbeOutcome::Unreachable {
                        code: format!("{error}"),
                    };
                }
            };
            let health = engine.health_snapshot().await;
            let record = health
                .servers
                .iter()
                .find(|server| server.name == VIOLET_SERVER);
            let outcome = match record {
                Some(server) if server.state == cmcp_core::UpstreamState::Healthy => {
                    let catalog = engine.catalog_entries_by_server().await;
                    let entries = catalog.get(VIOLET_SERVER).map(Vec::as_slice).unwrap_or(&[]);
                    let tools = entries.iter().map(|e| e.name.clone()).collect();
                    let schema_problems = entries
                        .iter()
                        .find(|e| e.name == VIOLET_POST_TOOL)
                        .map(|e| violet_post_schema_problems(&e.input_schema))
                        .unwrap_or_default();
                    ProbeOutcome::Tools {
                        tools,
                        schema_problems,
                    }
                }
                Some(server) => match server.last_error_code.as_deref() {
                    Some("authentication_required") => ProbeOutcome::Unauthorized,
                    Some(code) => ProbeOutcome::Unreachable {
                        code: code.to_string(),
                    },
                    None => ProbeOutcome::Unreachable {
                        code: "connection_failed".to_string(),
                    },
                },
                None => ProbeOutcome::Unreachable {
                    code: "not_configured".to_string(),
                },
            };
            engine.shutdown().await;
            outcome
        })
    }
}

/// Secrets returned by the hub or a bypass fallback. This type never crosses
/// the report/terminal boundary; it exists only long enough to populate the
/// machine credentials file and the current process environment.
#[derive(Clone)]
struct CredentialValues {
    token: String,
    bypass: String,
}

impl std::fmt::Debug for CredentialValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialValues")
            .field("token", &"<redacted>")
            .field("bypass", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HubClientError {
    RouteUnavailable,
    Unauthorized,
    Forbidden,
    LabelTaken,
    HttpStatus(u16),
    Transport(String),
    InvalidResponse,
}

impl std::fmt::Display for HubClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RouteUnavailable => write!(f, "route unavailable"),
            Self::Unauthorized => write!(f, "unauthorized"),
            Self::Forbidden => write!(f, "forbidden"),
            Self::LabelTaken => write!(f, "label taken"),
            Self::HttpStatus(status) => write!(f, "HTTP {status}"),
            Self::Transport(error) => write!(f, "transport error: {error}"),
            Self::InvalidResponse => write!(f, "invalid response"),
        }
    }
}

trait HubClient {
    fn create_client(
        &self,
        hub_url: &str,
        cloud_token: &str,
        label: &str,
    ) -> std::result::Result<(String, Option<String>), HubClientError>;
    fn fetch_bypass(
        &self,
        hub_url: &str,
        cloud_token: &str,
    ) -> std::result::Result<String, HubClientError>;
}

fn hub_route_url(hub_url: &str, route: &str) -> std::result::Result<String, HubClientError> {
    let mut url = Url::parse(hub_url).map_err(|_| HubClientError::InvalidResponse)?;
    url.set_path(route);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn classify_hub_status(status: u16) -> HubClientError {
    match status {
        401 => HubClientError::Unauthorized,
        403 => HubClientError::Forbidden,
        404 | 405 => HubClientError::RouteUnavailable,
        _ => HubClientError::HttpStatus(status),
    }
}

struct ProcessHubClient;

impl HubClient for ProcessHubClient {
    fn create_client(
        &self,
        hub_url: &str,
        cloud_token: &str,
        label: &str,
    ) -> std::result::Result<(String, Option<String>), HubClientError> {
        let url = hub_route_url(hub_url, HUB_CLIENT_ROUTE)?;
        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {cloud_token}"))
            .set("Content-Type", "application/json")
            .send_json(serde_json::json!({
                "label": label,
                "connector": "slack",
            }));
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(status, response)) => {
                if status == 409 {
                    let body = response.into_string().unwrap_or_default();
                    let is_taken = serde_json::from_str::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("error")
                                .and_then(serde_json::Value::as_str)
                                .map(|error| error == "label_taken")
                        })
                        .unwrap_or(false);
                    return Err(if is_taken {
                        HubClientError::LabelTaken
                    } else {
                        HubClientError::HttpStatus(status)
                    });
                }
                return Err(classify_hub_status(status));
            }
            Err(ureq::Error::Transport(error)) => {
                return Err(HubClientError::Transport(error.to_string()));
            }
        };
        let body = response
            .into_json::<serde_json::Value>()
            .map_err(|_| HubClientError::InvalidResponse)?;
        let token = body
            .get("token")
            .and_then(serde_json::Value::as_str)
            .filter(|token| !token.trim().is_empty())
            .ok_or(HubClientError::InvalidResponse)?
            .to_string();
        let bypass = body
            .get("bypass")
            .and_then(serde_json::Value::as_str)
            .filter(|bypass| !bypass.trim().is_empty())
            .map(str::to_string);
        Ok((token, bypass))
    }

    fn fetch_bypass(
        &self,
        hub_url: &str,
        cloud_token: &str,
    ) -> std::result::Result<String, HubClientError> {
        let url = hub_route_url(hub_url, HUB_BYPASS_ROUTE)?;
        let response = ureq::get(&url)
            .set("Authorization", &format!("Bearer {cloud_token}"))
            .call();
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(status, _)) => return Err(classify_hub_status(status)),
            Err(ureq::Error::Transport(error)) => {
                return Err(HubClientError::Transport(error.to_string()));
            }
        };
        let body = response
            .into_json::<serde_json::Value>()
            .map_err(|_| HubClientError::InvalidResponse)?;
        body.get("bypass")
            .and_then(serde_json::Value::as_str)
            .filter(|bypass| !bypass.trim().is_empty())
            .map(str::to_string)
            .ok_or(HubClientError::InvalidResponse)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BypassReadError {
    HttpStatus(u16),
    Transport(String),
    InvalidResponse,
}

trait BypassReader {
    fn read(
        &self,
        vercel_token: &str,
        project: &str,
    ) -> std::result::Result<String, BypassReadError>;
}

struct ProcessBypassReader;

impl BypassReader for ProcessBypassReader {
    fn read(
        &self,
        vercel_token: &str,
        project: &str,
    ) -> std::result::Result<String, BypassReadError> {
        let url = format!("https://api.vercel.com/v1/projects/{project}/protection-bypass");
        let response = ureq::get(&url)
            .set("Authorization", &format!("Bearer {vercel_token}"))
            .call();
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(status, _)) => return Err(BypassReadError::HttpStatus(status)),
            Err(ureq::Error::Transport(error)) => {
                return Err(BypassReadError::Transport(error.to_string()));
            }
        };
        let body = response
            .into_json::<serde_json::Value>()
            .map_err(|_| BypassReadError::InvalidResponse)?;
        ["bypass", "secret", "protectionBypass"]
            .iter()
            .find_map(|key| {
                body.get(*key)
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
            })
            .ok_or(BypassReadError::InvalidResponse)
    }
}

trait SecretPrompt {
    fn read(&self) -> Result<String>;
}

struct ProcessSecretPrompt;

impl SecretPrompt for ProcessSecretPrompt {
    fn read(&self) -> Result<String> {
        inquire::Password::new("Vercel protection bypass secret")
            .without_confirmation()
            .prompt()
            .context("could not read the Vercel bypass secret")
    }
}

trait DeviceIdentity {
    fn hostname(&self) -> Option<String>;
    fn device_id(&self) -> Result<Option<String>>;
}

struct ProcessDeviceIdentity;

impl DeviceIdentity for ProcessDeviceIdentity {
    fn hostname(&self) -> Option<String> {
        DeviceConfig::hostname()
    }

    fn device_id(&self) -> Result<Option<String>> {
        DeviceConfig::load()
            .map_err(|error| anyhow::anyhow!(error.to_string()))
            .map(|config| config.map(|config| config.device_id))
    }
}

fn provisioning_error(args: &str) -> anyhow::Error {
    anyhow::anyhow!(args.to_string())
}

fn hub_auth_error(error: &HubClientError) -> anyhow::Error {
    match error {
        HubClientError::Unauthorized | HubClientError::Forbidden => anyhow::anyhow!(
            "hub route POST {HUB_CLIENT_ROUTE} rejected the Cassy Cloud login ({}); run `cas login` and retry",
            error
        ),
        HubClientError::RouteUnavailable => {
            anyhow::anyhow!("hub route POST {HUB_CLIENT_ROUTE} not available ({HUB_CLIENT_ISSUE})")
        }
        _ => anyhow::anyhow!("hub route POST {HUB_CLIENT_ROUTE} failed: {error}"),
    }
}

fn fallback_bypass(
    env: &dyn EnvLookup,
    vercel: &dyn BypassReader,
    prompt: &dyn SecretPrompt,
) -> Result<String> {
    if let Some(token) = env
        .get("VERCEL_TOKEN")
        .filter(|token| !token.trim().is_empty())
    {
        if let Ok(bypass) = vercel.read(&token, VERCEL_PROJECT)
            && !bypass.trim().is_empty()
        {
            return Ok(bypass);
        }
    }
    let bypass = prompt.read()?;
    anyhow::ensure!(
        !bypass.trim().is_empty(),
        "the Vercel bypass secret cannot be empty"
    );
    Ok(bypass)
}

fn mint_client(
    args: &VioletArgs,
    label: &str,
    cloud_token: &str,
    hub: &dyn HubClient,
    device: &dyn DeviceIdentity,
) -> Result<(String, Option<String>, String)> {
    match hub.create_client(&args.url, cloud_token, label) {
        Ok((token, bypass)) => Ok((token, bypass, label.to_string())),
        Err(HubClientError::LabelTaken) => {
            let device_id = device
                .device_id()?
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| {
                    provisioning_error(&format!(
                        "hub route POST {HUB_CLIENT_ROUTE} reported label_taken; no device id is available in ~/.config/cas/device.json"
                    ))
                })?;
            let suffix: String = device_id.chars().take(6).collect();
            let retry_label = format!("{label}_{suffix}");
            let (token, bypass) = hub
                .create_client(&args.url, cloud_token, &retry_label)
                .map_err(|error| match error {
                    HubClientError::LabelTaken => provisioning_error(&format!(
                        "hub route POST {HUB_CLIENT_ROUTE} rejected both labels as taken"
                    )),
                    other => hub_auth_error(&other),
                })?;
            Ok((token, bypass, retry_label))
        }
        Err(error) => Err(hub_auth_error(&error)),
    }
}

fn provision_credentials(
    args: &VioletArgs,
    env: &dyn EnvLookup,
    hub: &dyn HubClient,
    vercel: &dyn BypassReader,
    prompt: &dyn SecretPrompt,
    device: &dyn DeviceIdentity,
) -> Result<(String, CredentialValues)> {
    let cloud_token = CloudConfig::load_effective()
        .token
        .filter(|token| !token.trim().is_empty());
    provision_credentials_with_cloud_token(
        args,
        env,
        cloud_token.as_deref(),
        hub,
        vercel,
        prompt,
        device,
    )
}

fn provision_credentials_with_cloud_token(
    args: &VioletArgs,
    env: &dyn EnvLookup,
    cloud_token: Option<&str>,
    hub: &dyn HubClient,
    vercel: &dyn BypassReader,
    prompt: &dyn SecretPrompt,
    device: &dyn DeviceIdentity,
) -> Result<(String, CredentialValues)> {
    let label = resolve_label(args.label.as_deref(), device.hostname().as_deref());
    let token_env = args.resolved_token_env_for_label(&label);
    let existing_token = violet_credential_value(&token_env, |name| env.get(name))
        .filter(|value| !value.trim().is_empty());
    let existing_bypass = violet_credential_value(args.bypass_env.trim(), |name| env.get(name))
        .filter(|value| !value.trim().is_empty());
    if existing_token.is_some() && existing_bypass.is_some() {
        return Ok((
            label,
            CredentialValues {
                token: existing_token.unwrap_or_default(),
                bypass: existing_bypass.unwrap_or_default(),
            },
        ));
    }

    let (token, hub_bypass, cloud_token, actual_label) = if let Some(token) = existing_token {
        (token, None, cloud_token.map(str::to_string), label.clone())
    } else {
        let cloud_token = cloud_token.ok_or_else(|| {
            provisioning_error(&format!(
                "Violet onboarding requires an existing Cassy Cloud login for hub route POST {HUB_CLIENT_ROUTE}; run `cas login` and retry"
            ))
        })?;
        let (token, bypass, actual_label) = mint_client(args, &label, cloud_token, hub, device)?;
        (token, bypass, Some(cloud_token.to_string()), actual_label)
    };
    let bypass = if let Some(bypass) = existing_bypass {
        bypass
    } else if let Some(bypass) = hub_bypass {
        bypass
    } else {
        match cloud_token {
            Some(cloud_token) => match hub.fetch_bypass(&args.url, &cloud_token) {
                Ok(bypass) => bypass,
                Err(HubClientError::RouteUnavailable) => fallback_bypass(env, vercel, prompt)?,
                Err(error @ (HubClientError::Unauthorized | HubClientError::Forbidden)) => {
                    return Err(anyhow::anyhow!(
                        "hub route GET {HUB_BYPASS_ROUTE} rejected the Cassy Cloud login ({}); run `cas login` and retry",
                        error
                    ));
                }
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "hub route GET {HUB_BYPASS_ROUTE} failed: {error}"
                    ));
                }
            },
            None => fallback_bypass(env, vercel, prompt)?,
        }
    };
    Ok((actual_label, CredentialValues { token, bypass }))
}

// ---------------------------------------------------------------------------
// Machine paths
// ---------------------------------------------------------------------------

/// The three machine-scoped files this command owns.
#[derive(Debug, Clone)]
pub struct MachinePaths {
    /// User-level proxy registration, merged beneath any project `.cas/proxy.toml`.
    pub user_proxy: PathBuf,
    /// `<CLAUDE_CONFIG_DIR|$HOME>/.claude.json`.
    pub claude_json: Option<PathBuf>,
    /// Other Claude account profiles on this machine (`$HOME/.claude.json`,
    /// `$HOME/.claude*/.claude.json`), excluding `claude_json`. GH #1164: a
    /// profile this command never looked at kept a stale `violet` entry.
    pub claude_profiles: Vec<PathBuf>,
    /// `<CODEX_HOME|$HOME/.codex>/config.toml`.
    pub codex_config: Option<PathBuf>,
    /// The only file allowed to contain the two plaintext onboarding values.
    pub credentials_file: PathBuf,
    /// The login-shell profile that must source the credentials file.
    pub login_profile: Option<PathBuf>,
}

/// Every Claude profile `.claude.json` under `home` other than `selected`:
/// `home/.claude.json` and `home/.claude*/.claude.json`, sorted. Only regular
/// files are returned.
pub fn discover_claude_profiles(home: &Path, selected: Option<&Path>) -> Vec<PathBuf> {
    let mut found = vec![home.join(".claude.json")];
    if let Ok(entries) = std::fs::read_dir(home) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with(".claude")
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                found.push(entry.path().join(".claude.json"));
            }
        }
    }
    found.retain(|path| ifs::is_regular_file(path) && Some(path.as_path()) != selected);
    found.sort();
    found.dedup();
    found
}

impl MachinePaths {
    /// Resolve from the environment, honouring the per-account overrides the
    /// factory already sets (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`) so a spawned
    /// worker registers into the account it is actually running as.
    pub fn from_env(env: &dyn EnvLookup) -> Result<Self> {
        let user_proxy = cmcp_core::config::Scope::User
            .config_path()
            .context("could not determine the user MCP configuration path")?;
        let home = env.get("HOME").map(PathBuf::from);
        let home_for_credentials = home
            .clone()
            .context("could not determine HOME for Violet credentials")?;
        let claude_dir = env
            .get("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| home.clone());
        let codex_dir = env
            .get("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".codex")));
        let credentials_file = env
            .get("CAS_CREDENTIALS_FILE")
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                env.get("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .filter(|path| !path.as_os_str().is_empty())
                    .map(|path| path.join("cas").join("credentials.env"))
            })
            .unwrap_or_else(|| {
                home_for_credentials
                    .join(".config")
                    .join("cas")
                    .join("credentials.env")
            });
        let login_profile = Some(profile_write_path(&login_profile_path(
            &home_for_credentials,
            env.get("SHELL").as_deref(),
        ))?);
        let claude_json = claude_dir.map(|d| d.join(".claude.json"));
        let claude_profiles = env
            .get("HOME")
            .map(PathBuf::from)
            .map(|home| discover_claude_profiles(&home, claude_json.as_deref()))
            .unwrap_or_default();
        Ok(Self {
            user_proxy,
            claude_json,
            claude_profiles,
            codex_config: codex_dir.map(|d| d.join("config.toml")),
            credentials_file,
            login_profile,
        })
    }
}

fn login_profile_path(home: &Path, shell: Option<&str>) -> PathBuf {
    let shell_name = shell
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if shell_name == "zsh" {
        return home.join(".zprofile");
    }
    if shell_name == "bash" && home.join(".bash_profile").exists() {
        return home.join(".bash_profile");
    }
    home.join(".profile")
}

fn valid_env_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().enumerate().all(|(index, byte)| {
            byte == b'_'
                || (byte.is_ascii_alphanumeric() && (index > 0 || byte.is_ascii_alphabetic()))
        })
}

fn assignment_name(line: &str) -> Option<&str> {
    let mut value = line.trim_start();
    if let Some(rest) = value.strip_prefix("export") {
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        value = rest.trim_start();
    }
    let (name, _) = value.split_once('=')?;
    let name = name.trim();
    valid_env_name(name).then_some(name)
}

fn assignment_value(line: &str) -> Option<(String, String)> {
    let name = assignment_name(line)?.to_string();
    let raw_value = line.split_once('=')?.1.trim();
    let value = if raw_value.starts_with('\'') && raw_value.ends_with('\'') {
        raw_value[1..raw_value.len() - 1].replace("'\\''", "'")
    } else if raw_value.starts_with('"') && raw_value.ends_with('"') {
        raw_value[1..raw_value.len() - 1].replace("\\\"", "\"")
    } else {
        raw_value.to_string()
    };
    Some((name, value))
}

fn shell_quote(value: &str) -> String {
    value.replace('\'', "'\\''")
}

fn write_private_file(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("credentials path has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("creating credentials directory {}", parent.display()))?;
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        anyhow::bail!(
            "{} is a symlink; refusing to write credentials",
            path.display()
        );
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("credentials file name is not UTF-8")?;
    let temp_path = parent.join(format!(
        ".{file_name}.cas-credentials.{}.tmp",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp_path)
        .with_context(|| format!("creating {}", path.display()))?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp_path, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp_path);
        return Err(error).with_context(|| format!("writing {}", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Upsert exactly the two owned exports, preserving unrelated credentials.
/// Plaintext values are accepted only in this writer and never appear in a
/// report or error string.
fn write_credentials(
    path: &Path,
    token_name: &str,
    token: &str,
    bypass_name: &str,
    bypass: &str,
) -> Result<bool> {
    anyhow::ensure!(
        valid_env_name(token_name),
        "invalid token environment variable name"
    );
    anyhow::ensure!(
        valid_env_name(bypass_name),
        "invalid bypass environment variable name"
    );
    anyhow::ensure!(
        !token.contains(['\r', '\n']) && !bypass.contains(['\r', '\n']),
        "credential values cannot contain newlines"
    );
    let existing = if ifs::is_regular_file(path) {
        ifs::read_capped(path)?
    } else if path.exists() {
        anyhow::bail!("{} is not a regular file", path.display());
    } else {
        String::new()
    };
    let mut lines: Vec<&str> = existing
        .lines()
        .filter(|line| {
            !matches!(
                assignment_name(line),
                Some(name) if name == token_name || name == bypass_name
            )
        })
        .collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.push("");
    let rendered = format!(
        "{}export {token_name}='{}'\nexport {bypass_name}='{}'\n",
        lines.join("\n"),
        shell_quote(token),
        shell_quote(bypass),
    );
    let changed = rendered != existing;
    if changed {
        write_private_file(path, &rendered)?;
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(changed)
}

/// Rename installed-machine keys in the credentials file to their canonical
/// `VIOLET_*` names, keeping each line's value text and position. A legacy
/// line whose canonical name already holds a value is dropped; an empty
/// canonical line yields to the legacy value it would otherwise have fallen
/// back to. Returns the canonical names that changed. Idempotent; values never
/// leave this function.
fn rename_legacy_credentials(path: &Path, dry_run: bool) -> Result<Vec<String>> {
    if !ifs::is_regular_file(path) {
        return Ok(Vec::new());
    }
    let existing = ifs::read_capped(path)?;
    let renames: std::collections::BTreeMap<&str, String> = existing
        .lines()
        .filter_map(assignment_name)
        .filter_map(|name| {
            let canonical = canonical_violet_credential_name(name);
            (canonical != name).then_some((name, canonical))
        })
        .collect();
    if renames.is_empty() {
        return Ok(Vec::new());
    }
    let canonical_with_value: std::collections::BTreeSet<String> = existing
        .lines()
        .filter_map(assignment_value)
        .filter(|(_, value)| !value.trim().is_empty())
        .map(|(name, _)| name)
        .collect();
    let mut renamed = std::collections::BTreeSet::new();
    let mut lines = Vec::new();
    for line in existing.lines() {
        match assignment_name(line) {
            Some(name) if renames.contains_key(name) => {
                let canonical = &renames[name];
                renamed.insert(canonical.clone());
                if canonical_with_value.contains(canonical) {
                    continue;
                }
                let at = line.find(name).expect("assignment name occurs in its line");
                lines.push(format!(
                    "{}{canonical}{}",
                    &line[..at],
                    &line[at + name.len()..]
                ));
            }
            Some(name)
                if renames.values().any(|canonical| canonical == name)
                    && !canonical_with_value.contains(name) =>
            {
                // An empty canonical assignment would shadow the renamed value.
            }
            _ => lines.push(line.to_string()),
        }
    }
    let mut rendered = lines.join("\n");
    if existing.ends_with('\n') {
        rendered.push('\n');
    }
    if !dry_run && rendered != existing {
        write_private_file(path, &rendered)?;
    }
    Ok(renamed.into_iter().collect())
}

fn profile_source_line(credentials: &Path) -> String {
    let path = shell_quote(&credentials.to_string_lossy());
    format!("[ -f '{path}' ] && . '{path}'")
}

/// Resolve a symlinked login profile before using atomic replacement. Renaming
/// a temporary file onto the link itself would replace the link and leave the
/// real profile untouched; resolving first preserves the operator's link while
/// making the actual target that changed visible in the report.
fn profile_write_path(profile: &Path) -> Result<PathBuf> {
    let metadata = match fs::symlink_metadata(profile) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(profile.to_path_buf());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", profile.display()));
        }
    };
    if metadata.file_type().is_symlink() {
        let target = fs::canonicalize(profile)
            .with_context(|| format!("resolving symlinked login profile {}", profile.display()))?;
        anyhow::ensure!(
            ifs::is_regular_file(&target),
            "symlinked login profile {} resolves to a non-regular file {}",
            profile.display(),
            target.display()
        );
        return Ok(target);
    }
    if metadata.file_type().is_file() {
        return Ok(profile.to_path_buf());
    }
    anyhow::bail!("{} is not a regular file", profile.display());
}

fn ensure_profile_line(profile: &Path, credentials: &Path) -> Result<bool> {
    let line = profile_source_line(credentials);
    let profile = profile_write_path(profile)?;
    let existing = if ifs::is_regular_file(&profile) {
        ifs::read_capped(&profile)?
    } else {
        String::new()
    };
    if existing.lines().any(|candidate| candidate.trim() == line) {
        return Ok(false);
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    ifs::atomic_write_create_dirs(&profile, &format!("{existing}{separator}{line}\n"))?;
    Ok(true)
}

fn sourced_profile_path(line: &str) -> Option<PathBuf> {
    let value = line
        .split_once("&& . ")
        .map(|(_, value)| value.trim())
        .or_else(|| line.trim().strip_prefix(". ").map(str::trim))
        .or_else(|| line.trim().strip_prefix("source ").map(str::trim))?;
    let value = value.split_whitespace().next()?;
    let value = if value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("'\\''", "'")
    } else if value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].replace("\\\"", "\"")
    } else {
        value.to_string()
    };
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn load_shell_assignments(
    path: &Path,
    values: &mut std::collections::BTreeMap<String, String>,
    visited: &mut std::collections::BTreeSet<PathBuf>,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    let identity = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(identity) {
        return;
    }
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    for line in contents.lines() {
        if let Some((name, value)) = assignment_value(line) {
            values.insert(name, value);
        }
        if let Some(source) = sourced_profile_path(line) {
            load_shell_assignments(&source, values, visited, depth + 1);
        }
    }
}

/// Load private machine credentials for a proxy child whose parent harness
/// does not forward arbitrary environment variables (notably Codex MCP
/// subprocesses). Existing process values win, and shell code is never
/// executed while reading the credentials/profile files.
#[cfg(feature = "mcp-proxy")]
pub fn load_machine_credentials_into_process_env() -> Result<usize> {
    load_machine_credentials_into_process_env_except(&[])
}

/// Worker resource denials must also apply to machine-file bootstrap, which
/// would otherwise restore variables removed at the PTY boundary.
#[cfg(feature = "mcp-proxy")]
pub fn load_machine_credentials_into_process_env_except(excluded: &[String]) -> Result<usize> {
    load_machine_credentials_with_installer(excluded, |name, value| {
        // SAFETY: this is process initialization, before the async proxy
        // runtime starts and before any threads are spawned.
        unsafe { std::env::set_var(name, value) };
    })
}

/// Share credential parsing and exclusion with tests without calling the
/// startup-only process setter. The test installer records restoration in its
/// TestEnvGuard, including values absent before bootstrap.
#[cfg(feature = "mcp-proxy")]
fn load_machine_credentials_with_installer(
    excluded: &[String],
    mut install: impl FnMut(&str, &str),
) -> Result<usize> {
    let paths = MachinePaths::from_env(&ProcessEnv)?;
    let mut values = std::collections::BTreeMap::new();
    let mut visited = std::collections::BTreeSet::new();
    load_shell_assignments(&paths.credentials_file, &mut values, &mut visited, 0);
    if let Some(profile) = paths.login_profile.as_deref() {
        load_shell_assignments(profile, &mut values, &mut visited, 0);
    }
    let mut loaded = 0;
    for (name, value) in values {
        if excluded.contains(&name) || value.trim().is_empty() || std::env::var_os(&name).is_some()
        {
            continue;
        }
        install(&name, &value);
        loaded += 1;
    }
    Ok(loaded)
}

/// cas-e753: the machine credentials a long-running process may need after
/// startup, returned as values rather than installed into its environment.
/// Names already set in the environment are omitted, as for startup loading.
#[cfg(feature = "mcp-proxy")]
pub(crate) fn machine_credential_values() -> Result<std::collections::BTreeMap<String, String>> {
    let mut values = std::collections::BTreeMap::new();
    load_machine_credentials_with_installer(&[], |name, value| {
        values.insert(name.to_string(), value.to_string());
    })?;
    Ok(values)
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WriteState {
    /// The file did not describe this registration; it now does.
    Written,
    /// Already byte-identical in effect; nothing was rewritten.
    AlreadyCurrent,
    /// Would be written, but `--dry-run` was requested.
    Planned,
    /// Deliberately not touched (`--no-harness`, or the path is unknown).
    Skipped,
}

impl WriteState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::AlreadyCurrent => "already current",
            Self::Planned => "planned (dry run)",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HarnessEntry {
    pub harness: String,
    pub path: Option<PathBuf>,
    pub state: WriteState,
    /// Present when a harness registration could not be attempted.
    pub note: Option<String>,
}

/// What happened to the project `.cas/proxy.toml` that shadows the machine
/// registration, if this command found one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectProxyEntry {
    pub path: PathBuf,
    pub state: WriteState,
    /// What changed, or why nothing did. Always present: a file that can
    /// silently override machine policy is never reported by state alone.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VioletReport {
    pub url: String,
    pub token_env: String,
    pub bypass_env: String,
    pub token_env_state: EnvState,
    pub bypass_env_state: EnvState,
    pub registration_path: PathBuf,
    pub registration: WriteState,
    pub credentials_path: PathBuf,
    pub credentials: WriteState,
    /// Canonical names whose installed-machine keys this run renamed in the
    /// credentials file. Names only; values are never reported.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub renamed_credentials: Vec<String>,
    pub login_profile_path: Option<PathBuf>,
    pub login_profile: WriteState,
    /// Routes that will actually be admitted from here. A project
    /// `.cas/proxy.toml` *replaces* the machine allowlist rather than widening
    /// it, so when one is present this is its policy, not the machine's.
    pub allowlist: Vec<String>,
    /// The project file whose policy shadows the machine registration.
    pub project_proxy: Option<ProjectProxyEntry>,
    pub harnesses: Vec<HarnessEntry>,
    pub probe: ProbeOutcome,
    /// References required by the effective project/machine server.
    pub probe_env_states: Vec<(String, EnvState)>,
    /// How the hub's live tool list disagrees with the allowlist, if at all.
    pub drift: ToolDrift,
    /// Exact next command or edit, when the operator must do something.
    pub remedy: Option<String>,
}

impl VioletReport {
    pub fn credentials_ready(&self) -> bool {
        self.probe_env_states
            .iter()
            .all(|(_, state)| state.is_usable())
    }

    /// Green means: effective credential references usable, registration on disk, and
    /// the hub answered with exactly the allowlisted tools and a usable
    /// `violet_post` schema. A skipped probe is deliberately *not* green — an
    /// unverified setup has never been proven.
    pub fn is_green(&self) -> bool {
        self.credentials_ready()
            && matches!(
                self.registration,
                WriteState::Written | WriteState::AlreadyCurrent
            )
            && self.drift.is_empty()
            && matches!(&self.probe, ProbeOutcome::Tools { schema_problems, .. } if schema_problems.is_empty())
    }
}

// ---------------------------------------------------------------------------
// Core
// ---------------------------------------------------------------------------

/// Two very different disagreements between the hub and the allowlist.
///
/// They are kept apart because their consequences differ. A tool the hub
/// offers that no route admits means **every call to it is denied by policy**
/// — the release post fails. An allowlisted name the hub no longer offers is
/// **inert**: dispatch of the live tools still works, the entry is merely
/// stale. Collapsing the two would either cry wolf over harmless clutter or
/// bury a genuine outage inside a cosmetic one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ToolDrift {
    /// Live hub tools with no allowlist route. Blocks dispatch.
    pub unallowlisted: Vec<String>,
    /// Allowlisted routes the hub no longer offers. Inert but stale.
    pub retired: Vec<String>,
}

impl ToolDrift {
    pub fn is_empty(&self) -> bool {
        self.unallowlisted.is_empty() && self.retired.is_empty()
    }

    /// True when something the operator needs is unreachable right now.
    pub fn blocks_dispatch(&self) -> bool {
        !self.unallowlisted.is_empty()
    }

    /// `source` is the file that actually holds the allowlist being compared.
    /// Naming it is not decoration: with a project `.cas/proxy.toml` in play
    /// the entries are in one of two files, and an operator who is not told
    /// which one edits the wrong one (cas-a0ab).
    pub fn describe(
        &self,
        live: &[String],
        allowlisted: &[String],
        source: Option<&Path>,
    ) -> String {
        let mut parts = Vec::new();
        if !self.unallowlisted.is_empty() {
            parts.push(format!(
                "hub offers un-allowlisted {} — calls to it are denied by policy",
                self.unallowlisted.join(", ")
            ));
        }
        if !self.retired.is_empty() {
            parts.push(format!(
                "allowlist still names retired {}",
                self.retired.join(", ")
            ));
        }
        let allowlist_label = match source {
            Some(path) => format!("allowlist in {}", path.display()),
            None => "allowlist".to_string(),
        };
        format!(
            "hub tool contract drifted: {} (hub: [{}]; {allowlist_label}: [{}])",
            parts.join("; "),
            sorted_unique(live).join(", "),
            sorted_unique(allowlisted).join(", ")
        )
    }
}

fn sorted_unique(values: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = values.iter().map(String::as_str).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Compare the hub's live tool list against the routes the registration
/// admits. Order-insensitive, so re-ordering a file is never reported as drift.
pub fn tool_drift(allowlisted: &[String], live: &[String]) -> ToolDrift {
    let expected = sorted_unique(allowlisted);
    // Only the canonical hub contract determines dispatch health. Deprecated
    // upstream aliases can disappear independently without affecting callers.
    let contract = violet_compatibility();
    let actual: Vec<_> = sorted_unique(live)
        .into_iter()
        .filter(|tool| {
            !contract
                .retired_tools
                .iter()
                .any(|retired| retired == *tool)
        })
        .collect();
    ToolDrift {
        unallowlisted: actual
            .iter()
            .filter(|tool| !expected.contains(tool))
            .map(|tool| (*tool).to_string())
            .collect(),
        retired: expected
            .iter()
            .filter(|tool| !actual.contains(tool))
            .map(|tool| (*tool).to_string())
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// The project file that shadows the machine registration
// ---------------------------------------------------------------------------
//
// A project `.cas/proxy.toml` is merged *above* the machine registration and,
// for the allowlist, it does not widen — it replaces (see
// `Config::load_merged_with_sources_from`). So a project file left behind by an
// older setup keeps the retired `slack_*` routes authoritative no matter how
// many times the machine file is rewritten, which is exactly how `cas doctor`
// came to print a remediation that could not clear its own warning (cas-a0ab).
//
// The file is edited with `toml_edit` rather than round-tripped through
// `ProxyConfig`, because it is operator-owned: its comments, key order, and
// every unrelated server and route survive the rewrite.

/// The plan for one project `.cas/proxy.toml`.
#[derive(Debug, Clone)]
struct ProjectProxyPlan {
    /// The rewritten document, when something has to change.
    rewritten: Option<String>,
    /// Violet routes the file admits once the plan is applied. Because a
    /// project allowlist replaces the machine one, this *is* the effective
    /// dispatch policy for this project.
    effective_tools: Vec<String>,
    server_override: Option<ServerConfig>,
    /// True when the file governs policy here but names no hub route, so no
    /// rewrite of the machine file can make the hub reachable.
    shadows_without_routes: bool,
    note: String,
}

/// The file whose allowlist is authoritative for dispatch here. A project
/// `.cas/proxy.toml` replaces the machine allowlist rather than widening it
/// (`Config::load_merged_with_sources_from`), so whenever one exists it — and
/// only it — decides which hub routes are admitted.
fn allowlist_source<'a>(project_proxy: Option<&'a Path>, user_proxy: &'a Path) -> &'a Path {
    project_proxy.unwrap_or(user_proxy)
}

fn canonical_entries() -> Vec<String> {
    VIOLET_TOOLS
        .iter()
        .map(|tool| format!("{VIOLET_SERVER}.{tool}"))
        .collect()
}

/// The `(server, tool)` an allowlist item names, for both spellings a project
/// file may use: the canonical `"server.tool"` string (plus the historical
/// separator aliases [`ExternalToolConfig::parse_allowlist_entry`] accepts) and
/// the structured `{ server = "…", tool = "…" }` inline table.
fn entry_route(value: &toml_edit::Value) -> Option<ExternalToolConfig> {
    if let Some(text) = value.as_str() {
        return ExternalToolConfig::parse_allowlist_entry(text).ok();
    }
    let table = value.as_inline_table()?;
    let server = table.get("server")?.as_str()?;
    let tool = table.get("tool")?.as_str()?;
    ExternalToolConfig::parse_allowlist_entry(&format!("{server}.{tool}")).ok()
}

/// Where a server definition actually points, for a note an operator can act
/// on without opening the file.
/// Two registrations of the same hub: both network transports at the same
/// URL (ignoring a trailing slash). Credentials are not compared.
fn same_hub(left: &ServerConfig, right: &ServerConfig) -> bool {
    let url = |server: &ServerConfig| match server {
        ServerConfig::Http { url, .. } | ServerConfig::Sse { url, .. } => {
            Some(url.trim().trim_end_matches('/').to_string())
        }
        ServerConfig::Stdio { .. } => None,
    };
    url(left).is_some_and(|left| Some(left) == url(right))
}

/// The `env:` names a server's auth and headers reference, in order.
fn server_env_references(server: &ServerConfig) -> Vec<String> {
    probe_env_states(server, &NoEnv)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// An environment with nothing set, for name-only questions.
struct NoEnv;

impl EnvLookup for NoEnv {
    fn get(&self, _name: &str) -> Option<String> {
        None
    }
}

fn server_endpoint(server: &ServerConfig) -> &str {
    match server {
        ServerConfig::Http { url, .. } | ServerConfig::Sse { url, .. } => url,
        ServerConfig::Stdio { command, .. } => command,
    }
}

/// Validate the references the proxy resolves, including custom headers.
fn probe_env_states(server: &ServerConfig, env: &dyn EnvLookup) -> Vec<(String, EnvState)> {
    let (auth, headers) = match server {
        ServerConfig::Http { auth, headers, .. } | ServerConfig::Sse { auth, headers, .. } => {
            (auth, headers)
        }
        ServerConfig::Stdio { .. } => return Vec::new(),
    };
    let mut values = Vec::new();
    values.extend(auth.as_deref());
    let mut headers = headers.iter().collect::<Vec<_>>();
    headers.sort_by_key(|(name, _)| *name);
    values.extend(headers.into_iter().map(|(_, value)| value.as_str()));
    let mut states = Vec::new();
    for name in values
        .into_iter()
        .filter_map(|value| value.strip_prefix("env:"))
    {
        if !states.iter().any(|(existing, _)| existing == name) {
            states.push((name.to_string(), EnvState::of(env, name)));
        }
    }
    states
}

fn missing_probe_credentials(states: &[(String, EnvState)]) -> Vec<String> {
    states
        .iter()
        .filter(|(_, state)| !state.is_usable())
        .map(|(name, state)| format!("{name} ({})", state.as_str()))
        .collect()
}

/// What to do with the project file's own `[servers.violet]` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerAction {
    /// The file does not define the hub server at all.
    Absent,
    /// Byte-for-byte the machine registration: pure duplication, safe to drop.
    Drop,
    /// GH #1164: the same hub URL as the machine registration, but with its
    /// own credential references. A committed project file cannot name a
    /// per-machine hub client token, so the machine registration (which
    /// carries this machine's token) supplies it instead.
    DropSameHub,
    /// A deliberate override (a staging hub, a different bearer variable).
    /// Dropping it would silently move this project to another endpoint, so
    /// it stays and is reported instead.
    Keep,
}

fn plan_project_proxy(
    path: &Path,
    machine_server: Option<&ServerConfig>,
) -> Result<ProjectProxyPlan> {
    let raw = ifs::read_capped(path)?;
    let mut document: toml_edit::DocumentMut = raw
        .parse()
        .with_context(|| format!("parsing {}", path.display()))?;

    let migrated_legacy = super::violet_retirement::retire_proxy_document(&mut document);

    let declares_server = document
        .get("servers")
        .and_then(|servers| servers.as_table_like())
        .is_some_and(|servers| servers.contains_key(VIOLET_SERVER));

    let allowlist_item = document.get("allowlist");
    if let Some(item) = allowlist_item
        && item.as_array().is_none()
    {
        anyhow::bail!(
            "{}: `allowlist` is not an array of routes; repair it by hand before re-running",
            path.display()
        );
    }
    let existing_tools: Vec<String> = allowlist_item
        .and_then(|item| item.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(entry_route)
                .filter(|route| route.server == VIOLET_SERVER)
                .map(|route| route.tool)
                .collect()
        })
        .unwrap_or_default();

    // A project file that says nothing about this hub is not ours to edit —
    // but it still replaces the machine allowlist, so its silence is the
    // operative policy and the caller must say so rather than claim success.
    if !declares_server && existing_tools.is_empty() {
        return Ok(ProjectProxyPlan {
            rewritten: None,
            effective_tools: Vec::new(),
            server_override: None,
            shadows_without_routes: true,
            note: format!(
                "names no {VIOLET_SERVER} route and is authoritative for dispatch policy \
                 here; left untouched"
            ),
        });
    }

    // `--url` exists so a project can point at a staging hub, and the proxy
    // merges project server tables *over* machine ones. So a block is only
    // "duplicate" if it is actually identical to the machine registration;
    // dropping a differing one would silently move this project to another
    // endpoint — the same silent policy change refused for the allowlist.
    let project_server = if declares_server {
        toml::from_str::<ProxyConfig>(&document.to_string())
            .with_context(|| format!("reading {}", path.display()))?
            .servers
            .remove(VIOLET_SERVER)
    } else {
        None
    };
    let server_action = match (declares_server, &project_server) {
        (false, _) => ServerAction::Absent,
        (true, project) if project.as_ref() == machine_server => ServerAction::Drop,
        (true, Some(project))
            if machine_server.is_some_and(|machine| same_hub(project, machine)) =>
        {
            ServerAction::DropSameHub
        }
        (true, _) => ServerAction::Keep,
    };
    let kept_override_note = || {
        let endpoint = project_server
            .as_ref()
            .map(server_endpoint)
            .unwrap_or("unparsed endpoint");
        format!(
            "kept [servers.{VIOLET_SERVER}]: it overrides the machine registration \
             (url {endpoint})"
        )
    };

    let canonical = canonical_entries();
    let allowlist_is_canonical = existing_tools == VIOLET_TOOLS;
    let drops_server = matches!(
        server_action,
        ServerAction::Drop | ServerAction::DropSameHub
    );
    if allowlist_is_canonical && !drops_server {
        let mut note = "already names exactly the hub's current routes".to_string();
        if server_action == ServerAction::Keep {
            note.push_str("; ");
            note.push_str(&kept_override_note());
        }
        return Ok(ProjectProxyPlan {
            rewritten: migrated_legacy.then(|| document.to_string()),
            effective_tools: existing_tools,
            server_override: project_server,
            shadows_without_routes: false,
            note,
        });
    }

    let mut changes = Vec::new();
    match server_action {
        ServerAction::Drop | ServerAction::DropSameHub => {
            let mut emptied = false;
            if let Some(servers) = document
                .get_mut("servers")
                .and_then(|item| item.as_table_like_mut())
            {
                servers.remove(VIOLET_SERVER);
                emptied = servers.is_empty();
            }
            if emptied {
                document.remove("servers");
            }
            changes.push(if server_action == ServerAction::Drop {
                format!(
                    "dropped the [servers.{VIOLET_SERVER}] block (identical to the machine \
                     registration, which supplies it)"
                )
            } else {
                let named = project_server
                    .as_ref()
                    .map(|server| server_env_references(server).join(", "))
                    .filter(|names| !names.is_empty())
                    .unwrap_or_else(|| "its own credentials".to_string());
                format!(
                    "dropped [servers.{VIOLET_SERVER}]: same hub URL as the machine registration; \
                     it named {named}, a per-machine hub client credential, so this machine's \
                     registration supplies the server instead"
                )
            });
        }
        ServerAction::Keep => changes.push(kept_override_note()),
        ServerAction::Absent => {}
    }

    if !allowlist_is_canonical {
        if document.get("allowlist").is_none() {
            document["allowlist"] = toml_edit::value(toml_edit::Array::new());
        }
        let array = document["allowlist"]
            .as_array_mut()
            .expect("checked above: allowlist is an array");
        // Keep the file's own shape: a multi-line array stays multi-line, an
        // inline one stays inline.
        let sample_prefix = array
            .len()
            .checked_sub(1)
            .and_then(|last| array.get(last))
            .and_then(|value| value.decor().prefix())
            .and_then(|prefix| prefix.as_str())
            .unwrap_or_default()
            .to_string();
        let trailing_comma = array.trailing_comma();
        array.retain(|value| entry_route(value).is_none_or(|route| route.server != VIOLET_SERVER));
        for entry in &canonical {
            let prefix = if array.is_empty() && !sample_prefix.contains('\n') {
                String::new()
            } else if sample_prefix.is_empty() {
                " ".to_string()
            } else {
                sample_prefix.clone()
            };
            array.push_formatted(toml_edit::Value::from(entry.as_str()).decorated(&prefix, ""));
        }
        array.set_trailing_comma(trailing_comma);
        changes.push(if existing_tools.is_empty() {
            format!("added the hub routes {}", canonical.join(", "))
        } else {
            format!(
                "rewrote its {VIOLET_SERVER} routes to {}",
                canonical.join(", ")
            )
        });
    }

    Ok(ProjectProxyPlan {
        rewritten: Some(document.to_string()),
        effective_tools: VIOLET_TOOLS.iter().map(|t| (*t).to_string()).collect(),
        server_override: if server_action == ServerAction::Keep {
            project_server
        } else {
            None
        },
        shadows_without_routes: false,
        note: changes.join("; "),
    })
}

/// Run the registration/reconciliation portion of the integration. Pure with
/// respect to its seams: every filesystem write goes through `paths` and
/// `project_proxy`, every credential fact through `env`, and the only network
/// call through `probe`. Production [`execute`] provisions credentials first.
///
/// `project_proxy` is the project's `.cas/proxy.toml` when the caller resolved
/// one. It is repaired, not merely reported: rewriting only the machine file
/// while a project file keeps the retired routes authoritative is what made
/// this command's own "already configured" receipt a lie (cas-a0ab).
pub fn run(
    args: &VioletArgs,
    project_proxy: Option<&Path>,
    paths: &MachinePaths,
    env: &dyn EnvLookup,
    probe: &dyn HubProbe,
) -> Result<VioletReport> {
    run_with_credentials(args, project_proxy, paths, env, probe, None)
}

fn run_with_credentials(
    args: &VioletArgs,
    project_proxy: Option<&Path>,
    paths: &MachinePaths,
    env: &dyn EnvLookup,
    probe: &dyn HubProbe,
    credentials: Option<&CredentialValues>,
) -> Result<VioletReport> {
    let token_env = args.resolved_token_env();
    let bypass_env = args.bypass_env.trim().to_string();
    anyhow::ensure!(
        !token_env.is_empty() && !bypass_env.is_empty(),
        "--token-env and --bypass-env must name environment variables"
    );

    let token_env_state = EnvState::of(env, &token_env);
    let bypass_env_state = EnvState::of(env, &bypass_env);
    let login_profile_path = paths
        .login_profile
        .as_deref()
        .map(profile_write_path)
        .transpose()?;

    // Rename before writing so a provisioned value replaces the renamed line
    // instead of leaving a legacy duplicate behind.
    let renamed_credentials = rename_legacy_credentials(&paths.credentials_file, args.dry_run)
        .with_context(|| format!("renaming keys in {}", paths.credentials_file.display()))?;
    let (credentials_state, profile_state) = match credentials {
        Some(_values) if args.dry_run => (WriteState::Planned, WriteState::Planned),
        Some(values) => {
            let changed = write_credentials(
                &paths.credentials_file,
                &token_env,
                &values.token,
                &bypass_env,
                &values.bypass,
            )
            .with_context(|| format!("writing {}", paths.credentials_file.display()))?;
            let credentials_state = if changed {
                WriteState::Written
            } else {
                WriteState::AlreadyCurrent
            };
            let profile_state = match login_profile_path.as_deref() {
                Some(profile) => {
                    let changed = ensure_profile_line(profile, &paths.credentials_file)
                        .with_context(|| format!("writing {}", profile.display()))?;
                    if changed {
                        WriteState::Written
                    } else {
                        WriteState::AlreadyCurrent
                    }
                }
                None => WriteState::Skipped,
            };
            (credentials_state, profile_state)
        }
        None => (WriteState::Skipped, WriteState::Skipped),
    };
    let credentials_state = match credentials_state {
        WriteState::Skipped | WriteState::AlreadyCurrent if !renamed_credentials.is_empty() => {
            if args.dry_run {
                WriteState::Planned
            } else {
                WriteState::Written
            }
        }
        state => state,
    };

    // The registration is written even when a variable is missing: it names
    // variables, holds no secret, and having it on disk is what makes the
    // remedy a one-line credentials-file edit instead of a second setup pass.
    let mut config = ProxyConfig::load_from(&paths.user_proxy)
        .with_context(|| format!("reading {}", paths.user_proxy.display()))?;
    let changed = config.ensure_violet_registration(&args.url, &token_env, &bypass_env);
    let registration = if !changed {
        WriteState::AlreadyCurrent
    } else if args.dry_run {
        WriteState::Planned
    } else {
        config
            .save_to(&paths.user_proxy)
            .with_context(|| format!("writing {}", paths.user_proxy.display()))?;
        WriteState::Written
    };

    // The project file is reconciled *after* the machine registration, so a
    // file this command refuses to edit still leaves a correct machine file
    // behind, and the error names the one path an operator must repair.
    let machine_server = config.servers.get(VIOLET_SERVER).cloned();
    let project = match project_proxy.filter(|path| ifs::is_regular_file(path)) {
        Some(path) => {
            let plan = plan_project_proxy(path, machine_server.as_ref())?;
            let state = match &plan.rewritten {
                // Not ours to edit vs. ours and already right: an operator who
                // sees "skipped" must be able to tell which one happened.
                None if plan.shadows_without_routes => WriteState::Skipped,
                None => WriteState::AlreadyCurrent,
                Some(_) if args.dry_run => WriteState::Planned,
                Some(text) => {
                    ifs::atomic_write_create_dirs(path, text)
                        .with_context(|| format!("writing {}", path.display()))?;
                    WriteState::Written
                }
            };
            Some((
                ProjectProxyEntry {
                    path: path.to_path_buf(),
                    state,
                    note: plan.note.clone(),
                },
                plan,
            ))
        }
        None => None,
    };

    // What this project will actually dispatch: the project file when one
    // governs policy here, the machine registration otherwise.
    let allowlist = match &project {
        Some((_, plan)) => plan.effective_tools.clone(),
        None => config.violet_allowlisted_tools(),
    };

    let mut harnesses = if args.no_harness {
        vec![
            HarnessEntry {
                harness: "claude-code".to_string(),
                path: paths.claude_json.clone(),
                state: WriteState::Skipped,
                note: Some("--no-harness".to_string()),
            },
            HarnessEntry {
                harness: "codex".to_string(),
                path: paths.codex_config.clone(),
                state: WriteState::Skipped,
                note: Some("--no-harness".to_string()),
            },
        ]
    } else {
        let mut harnesses = vec![register_claude(
            paths.claude_json.as_deref(),
            &args.url,
            &token_env,
            &bypass_env,
            args.dry_run,
        )];
        // GH #1164: other Claude account profiles that already register the
        // hub are reconciled too; one this command never looked at kept a
        // stale entry Claude Code then used.
        harnesses.extend(paths.claude_profiles.iter().filter_map(|profile| {
            reconcile_claude_profile(profile, &args.url, &token_env, &bypass_env, args.dry_run)
        }));
        harnesses.push(register_codex(
            paths.codex_config.as_deref(),
            &args.url,
            &token_env,
            &bypass_env,
            args.dry_run,
        ));
        harnesses
    };

    let probe_server = project
        .as_ref()
        .and_then(|(_, plan)| plan.server_override.as_ref())
        .or(machine_server.as_ref())
        .context("Violet server registration is missing")?;
    let probe_env_states = probe_env_states(probe_server, env);
    let missing = missing_probe_credentials(&probe_env_states);
    let probe_outcome = if args.skip_verify {
        ProbeOutcome::Skipped {
            reason: "--skip-verify".to_string(),
        }
    } else if !missing.is_empty() {
        ProbeOutcome::Skipped {
            reason: format!("Set {}", missing.join(" and ")),
        }
    } else if args.dry_run {
        ProbeOutcome::Skipped {
            reason: "--dry-run".to_string(),
        }
    } else {
        probe.list_tools(probe_server)
    };

    // GH #1164: "already current" is a structural claim about one file. Each
    // claude-code entry carries the authenticated tools/list verdict for the
    // registration it now holds (this machine's url, bearer and bypass
    // references), so a rejected bearer is never reported as merely current.
    let override_url = project
        .as_ref()
        .and_then(|(_, plan)| plan.server_override.as_ref())
        .map(|server| server_endpoint(server).to_string());
    let verdict = claude_entry_verdict(
        &probe_outcome,
        override_url.as_deref(),
        &args.url,
        &token_env,
    );
    for harness in harnesses.iter_mut().filter(|harness| {
        harness.harness == "claude-code"
            && matches!(
                harness.state,
                WriteState::Written | WriteState::AlreadyCurrent | WriteState::Planned
            )
    }) {
        harness.note = Some(match harness.note.take() {
            Some(note) => format!("{note}; {verdict}"),
            None => verdict.clone(),
        });
    }

    // After a successful write the allowlist is exactly the constant, so any
    // drift here means the hub itself moved — worth reporting either way.
    let source = allowlist_source(
        project.as_ref().map(|(entry, _)| entry.path.as_path()),
        &paths.user_proxy,
    );
    let (drift, drift_message) = match &probe_outcome {
        ProbeOutcome::Tools { tools, .. } => {
            let drift = tool_drift(&allowlist, tools);
            let message =
                (!drift.is_empty()).then(|| drift.describe(tools, &allowlist, Some(source)));
            (drift, message)
        }
        _ => (ToolDrift::default(), None),
    };

    // Drift that survives this command needs a different sentence from drift
    // this command just fixed: re-running cannot widen a project policy that
    // deliberately names no hub route.
    let shadowed_without_routes = project
        .as_ref()
        .filter(|(_, plan)| plan.shadows_without_routes)
        .map(|(entry, _)| entry.path.clone());
    let drift_remedy = drift_message
        .as_deref()
        .map(|drift| match &shadowed_without_routes {
            Some(path) => format!(
                "{drift}. {} is authoritative for dispatch policy here and names no \
                 {VIOLET_SERVER} route: add {} to its allowlist",
                path.display(),
                canonical_entries()
                    .iter()
                    .map(|entry| format!("\"{entry}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => format!(
                "{drift}. Re-run `cas integrate violet` to rewrite the allowlist against \
                 the hub's current contract."
            ),
        });

    let override_path = project
        .as_ref()
        .filter(|(_, plan)| plan.server_override.is_some())
        .map(|(entry, _)| entry.path.clone());
    let bearer_env = server_bearer_env(probe_server).unwrap_or_else(|| token_env.clone());
    let remedy = build_remedy(
        &probe_env_states,
        &probe_outcome,
        drift_remedy,
        &RemedyContext {
            override_path: override_path.as_deref(),
            endpoint: server_endpoint(probe_server),
            bearer_env: &bearer_env,
            credentials_file: &paths.credentials_file,
        },
    );

    Ok(VioletReport {
        url: server_endpoint(probe_server).to_string(),
        token_env,
        bypass_env,
        token_env_state,
        bypass_env_state,
        registration_path: paths.user_proxy.clone(),
        registration,
        credentials_path: paths.credentials_file.clone(),
        credentials: credentials_state,
        renamed_credentials,
        login_profile_path,
        login_profile: profile_state,
        allowlist,
        project_proxy: project.map(|(entry, _)| entry),
        harnesses,
        probe: probe_outcome,
        probe_env_states,
        drift,
        remedy,
    })
}

/// What the remedy needs to name the right fix (GH #1164).
struct RemedyContext<'a> {
    /// The project file whose distinct-URL `[servers.violet]` override is the
    /// effective server, if any. Cassy never mints the tokens it names.
    override_path: Option<&'a Path>,
    endpoint: &'a str,
    /// The variable holding the effective server's bearer.
    bearer_env: &'a str,
    credentials_file: &'a Path,
}

/// The bearer variable an `auth = "env:NAME"` registration references.
fn server_bearer_env(server: &ServerConfig) -> Option<String> {
    match server {
        ServerConfig::Http { auth, .. } | ServerConfig::Sse { auth, .. } => auth
            .as_deref()
            .and_then(|auth| auth.strip_prefix("env:"))
            .map(str::to_string),
        ServerConfig::Stdio { .. } => None,
    }
}

fn build_remedy(
    env_states: &[(String, EnvState)],
    probe: &ProbeOutcome,
    drift: Option<String>,
    context: &RemedyContext<'_>,
) -> Option<String> {
    let missing = missing_probe_credentials(env_states);
    if !missing.is_empty() {
        return Some(match context.override_path {
            Some(path) => format!(
                "Set {}: the [servers.{VIOLET_SERVER}] override in {} names it for {}, and Cassy \
                 does not mint it. Export it in this shell, or delete that block to use this \
                 machine's registration",
                missing.join(" and "),
                path.display(),
                context.endpoint
            ),
            None => format!("Set {}; {CREDENTIALS_HINT}", missing.join(" and ")),
        });
    }
    if let Some(drift) = drift {
        return Some(drift);
    }
    match probe {
        ProbeOutcome::Tools {
            schema_problems, ..
        } if !schema_problems.is_empty() => Some(schema_problem_remedy(schema_problems)),
        ProbeOutcome::Unauthorized => Some(match context.override_path {
            Some(path) => format!(
                "The hub rejected the bearer in {} (HTTP 401; Authorization: Bearer <set>), named \
                 by the [servers.{VIOLET_SERVER}] override in {}. Replace that token, or delete \
                 the block to use this machine's registration.",
                context.bearer_env,
                path.display()
            ),
            None => rejected_bearer_remedy(context.bearer_env, context.credentials_file),
        }),
        ProbeOutcome::Unreachable { code } => Some(format!(
            "The hub did not answer ({code}). Check connectivity, then re-run \
             `cas integrate violet`."
        )),
        _ => None,
    }
}

/// GH #1164: a hub client token the hub rejects is revoked or invalid. Signing
/// in again does not replace it, and `cas integrate violet` keeps an existing
/// token, so it must be removed before a re-run mints a new client.
fn rejected_bearer_remedy(bearer_env: &str, credentials_file: &Path) -> String {
    format!(
        "The hub rejected the bearer in {bearer_env} (HTTP 401; Authorization: Bearer <set>): \
         this hub client token is revoked or invalid. Unset {bearer_env}, delete its line from \
         {}, then re-run `cas integrate violet` to mint a new hub client.",
        credentials_file.display()
    )
}

/// The hub, not Cassy, owns the `violet_post` schema: harnesses receive it
/// directly, so the only remedy is a hub fix.
fn schema_problem_remedy(problems: &[String]) -> String {
    format!(
        "The hub serves an unusable {VIOLET_POST_TOOL} schema: {}. Harnesses receive it \
         directly from the hub, so report it in Richards-LLC/violet_ps (see violet_ps#26).",
        problems.join("; ")
    )
}

// ---------------------------------------------------------------------------
// Harness registration
// ---------------------------------------------------------------------------

/// Claude Code: a user-scope HTTP server in the selected profile's
/// `.claude.json`. `${VAR}` is expanded by the client at launch, so this stays
/// a name reference. The whole document is round-tripped through
/// `serde_json::Value`, which preserves every unrelated key (project history,
/// onboarding state) rather than rewriting the file from a partial model.
fn register_claude(
    path: Option<&Path>,
    url: &str,
    token_env: &str,
    bypass_env: &str,
    dry_run: bool,
) -> HarnessEntry {
    let Some(path) = path else {
        return HarnessEntry {
            harness: "claude-code".to_string(),
            path: None,
            state: WriteState::Skipped,
            note: Some("neither CLAUDE_CONFIG_DIR nor HOME is set".to_string()),
        };
    };
    let stale = existing_claude_violet_entry(path).and_then(|entry| stale_claude_reason(&entry));
    match apply_claude(path, url, token_env, bypass_env, dry_run) {
        Ok(state) => HarnessEntry {
            harness: "claude-code".to_string(),
            path: Some(path.to_path_buf()),
            state,
            note: stale_claude_note(state, stale),
        },
        Err(error) => HarnessEntry {
            harness: "claude-code".to_string(),
            path: Some(path.to_path_buf()),
            state: WriteState::Skipped,
            note: Some(format!("{error:#}")),
        },
    }
}

/// GH #1164: reconcile another Claude profile only if it already registers the
/// hub. A profile that never did is not this command's to change.
fn reconcile_claude_profile(
    path: &Path,
    url: &str,
    token_env: &str,
    bypass_env: &str,
    dry_run: bool,
) -> Option<HarnessEntry> {
    let existing = existing_claude_violet_entry(path)?;
    let stale = stale_claude_reason(&existing);
    Some(
        match apply_claude(path, url, token_env, bypass_env, dry_run) {
            Ok(state) => HarnessEntry {
                harness: "claude-code".to_string(),
                path: Some(path.to_path_buf()),
                state,
                note: stale_claude_note(state, stale),
            },
            Err(error) => HarnessEntry {
                harness: "claude-code".to_string(),
                path: Some(path.to_path_buf()),
                state: WriteState::Skipped,
                note: Some(format!("{error:#}")),
            },
        },
    )
}

/// The `mcpServers.violet` entry a Claude profile holds, if it parses.
fn existing_claude_violet_entry(path: &Path) -> Option<serde_json::Value> {
    if !ifs::is_regular_file(path) {
        return None;
    }
    let raw = ifs::read_capped(path).ok()?;
    let document: serde_json::Value = serde_json::from_str(&raw).ok()?;
    document.get("mcpServers")?.get(VIOLET_SERVER).cloned()
}

/// Why an existing Claude entry cannot authenticate as written. Names the
/// shape only; a literal value is never echoed.
fn stale_claude_reason(entry: &serde_json::Value) -> Option<&'static str> {
    let headers = entry.get("headers")?.as_object()?;
    let authorization = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .and_then(|(_, value)| value.as_str());
    if authorization.is_some_and(|value| !value.contains("${")) {
        return Some("literal bearer");
    }
    if headers
        .values()
        .filter_map(|value| value.as_str())
        .any(|value| value.contains("${MECHA_"))
    {
        return Some("MECHA_* references that no longer expand");
    }
    None
}

fn stale_claude_note(state: WriteState, stale: Option<&'static str>) -> Option<String> {
    match (state, stale) {
        (WriteState::Written, Some(reason)) => Some(format!(
            "rewrote a stale entry ({reason}) with env references"
        )),
        (WriteState::Planned, Some(reason)) => Some(format!(
            "would rewrite a stale entry ({reason}) with env references"
        )),
        _ => None,
    }
}

/// The authenticated tools/list verdict for a claude-code entry holding this
/// machine's registration (GH #1164).
fn claude_entry_verdict(
    probe: &ProbeOutcome,
    override_url: Option<&str>,
    url: &str,
    token_env: &str,
) -> String {
    if let Some(override_url) = override_url {
        return format!(
            "not verified here: this project's override probes {override_url}, while the entry \
             targets {url}"
        );
    }
    match probe {
        ProbeOutcome::Tools { tools, .. } => {
            format!("authenticated tools/list: {} tool(s)", tools.len())
        }
        ProbeOutcome::Unauthorized => format!(
            "authenticated tools/list rejected Bearer ${{{token_env}}} (HTTP 401): Claude Code \
             cannot use this entry until the token is replaced"
        ),
        ProbeOutcome::Unreachable { code } => {
            format!("not verified: hub unreachable ({code})")
        }
        ProbeOutcome::Skipped { reason } => format!("not verified: {reason}"),
    }
}

fn apply_claude(
    path: &Path,
    url: &str,
    token_env: &str,
    bypass_env: &str,
    dry_run: bool,
) -> Result<WriteState> {
    let mut document: serde_json::Value = if ifs::is_regular_file(path) {
        let raw = ifs::read_capped(path)?;
        if raw.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?
        }
    } else if path.exists() {
        anyhow::bail!("{} is not a regular file", path.display());
    } else {
        serde_json::json!({})
    };

    if !document.is_object() {
        anyhow::bail!("{} is not a JSON object", path.display());
    }
    let desired = serde_json::json!({
        "type": "http",
        "url": url,
        "headers": {
            "Authorization": format!("Bearer ${{{token_env}}}"),
            VIOLET_BYPASS_HEADER: format!("${{{bypass_env}}}"),
        }
    });
    let servers = document
        .as_object_mut()
        .expect("checked above")
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        anyhow::bail!("{}: mcpServers is not an object", path.display());
    }
    let migrated = super::violet_retirement::retire_claude_entry(&mut document);
    let servers = document.get_mut("mcpServers").expect("checked above");
    if !migrated && servers.get(VIOLET_SERVER) == Some(&desired) {
        return Ok(WriteState::AlreadyCurrent);
    }
    if dry_run {
        return Ok(WriteState::Planned);
    }
    servers
        .as_object_mut()
        .expect("checked above")
        .insert(VIOLET_SERVER.to_string(), desired);
    let serialized = serde_json::to_string_pretty(&document)?;
    ifs::atomic_write_create_dirs(path, &format!("{serialized}\n"))?;
    Ok(WriteState::Written)
}

/// Codex: an `[mcp_servers.violet]` table naming the bearer variable.
/// Edited with `toml_edit` so an operator's 3000-line `config.toml` keeps its
/// comments, ordering, and every unrelated table.
fn register_codex(
    path: Option<&Path>,
    url: &str,
    token_env: &str,
    bypass_env: &str,
    dry_run: bool,
) -> HarnessEntry {
    let Some(path) = path else {
        return HarnessEntry {
            harness: "codex".to_string(),
            path: None,
            state: WriteState::Skipped,
            note: Some("neither CODEX_HOME nor HOME is set".to_string()),
        };
    };
    match apply_codex(path, url, token_env, bypass_env, dry_run) {
        Ok(state) => HarnessEntry {
            harness: "codex".to_string(),
            path: Some(path.to_path_buf()),
            state,
            note: None,
        },
        Err(error) => HarnessEntry {
            harness: "codex".to_string(),
            path: Some(path.to_path_buf()),
            state: WriteState::Skipped,
            note: Some(format!("{error:#}")),
        },
    }
}

fn apply_codex(
    path: &Path,
    url: &str,
    token_env: &str,
    bypass_env: &str,
    dry_run: bool,
) -> Result<WriteState> {
    let raw = if ifs::is_regular_file(path) {
        ifs::read_capped(path)?
    } else if path.exists() {
        anyhow::bail!("{} is not a regular file", path.display());
    } else {
        String::new()
    };
    let mut document: toml_edit::DocumentMut = raw
        .parse()
        .with_context(|| format!("parsing {}", path.display()))?;

    let migrated = super::violet_retirement::retire_codex_entry(&mut document);
    let existing = document
        .get("mcp_servers")
        .and_then(|servers| servers.get(VIOLET_SERVER));
    let current_matches = existing.is_some_and(|table| {
        table.get("url").and_then(|v| v.as_str()) == Some(url)
            && table.get("bearer_token_env_var").and_then(|v| v.as_str()) == Some(token_env)
            && table
                .get("env_http_headers")
                .and_then(|headers| headers.get(VIOLET_BYPASS_HEADER))
                .and_then(|v| v.as_str())
                == Some(bypass_env)
    });
    if current_matches && !migrated {
        return Ok(WriteState::AlreadyCurrent);
    }
    if dry_run {
        return Ok(WriteState::Planned);
    }

    let servers = document["mcp_servers"].or_insert(toml_edit::table());
    if let Some(table) = servers.as_table_mut() {
        table.set_implicit(true);
    }
    let mut headers = toml_edit::InlineTable::new();
    headers.insert(VIOLET_BYPASS_HEADER, bypass_env.into());
    let entry = servers[VIOLET_SERVER].or_insert(toml_edit::table());
    entry["url"] = toml_edit::value(url);
    entry["bearer_token_env_var"] = toml_edit::value(token_env);
    entry["env_http_headers"] = toml_edit::value(headers);

    ifs::atomic_write_create_dirs(path, &document.to_string())?;
    Ok(WriteState::Written)
}

// ---------------------------------------------------------------------------
// Doctor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorSeverity {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorRow {
    pub severity: DoctorSeverity,
    pub message: String,
}

/// Read-only status for `cas doctor`. Never writes, never prompts.
///
/// `project_proxy` is the project's `.cas/proxy.toml` when it exists. Because
/// a project file *replaces* rather than widens the machine allowlist, a
/// project that declares its own policy and omits the hub routes is reported
/// as the concrete failure it is, with the routes to add.
pub fn doctor_row(
    project_proxy: Option<&Path>,
    paths: &MachinePaths,
    env: &dyn EnvLookup,
    probe: &dyn HubProbe,
) -> DoctorRow {
    let merged =
        match ProxyConfig::load_merged_with_sources_from(Some(&paths.user_proxy), project_proxy) {
            Ok((config, _)) => config,
            Err(error) => {
                return DoctorRow {
                    severity: DoctorSeverity::Error,
                    message: format!(
                        "proxy configuration could not be read ({error:#}). Repair it, then run \
                     `cas integrate violet`"
                    ),
                };
            }
        };

    let Some(server) = merged.servers.get(VIOLET_SERVER) else {
        return DoctorRow {
            severity: DoctorSeverity::Warning,
            message: format!(
                "not registered on this machine ({} has no {VIOLET_SERVER} server). Run \
                 `cas integrate violet`",
                paths.user_proxy.display()
            ),
        };
    };
    let token_env = match server {
        ServerConfig::Http { auth, .. } | ServerConfig::Sse { auth, .. } => {
            auth.as_deref().and_then(|auth| auth.strip_prefix("env:"))
        }
        ServerConfig::Stdio { .. } => None,
    };
    let Some(token_env) = token_env else {
        return DoctorRow {
            severity: DoctorSeverity::Error,
            message: format!(
                "the {VIOLET_SERVER} registration does not reference its bearer by \
                 environment-variable name. Run `cas integrate violet` to rewrite it as an \
                 env: reference"
            ),
        };
    };
    let states = probe_env_states(server, env);
    let endpoint = server_endpoint(server);
    let token_state = EnvState::of(env, token_env);
    let allowlist = merged.violet_allowlisted_tools();

    if allowlist.is_empty() {
        let where_from = project_proxy
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| paths.user_proxy.display().to_string());
        return DoctorRow {
            severity: DoctorSeverity::Error,
            message: format!(
                "{token_env} is {}, but no {VIOLET_SERVER} route is allowlisted: {where_from} \
                 is authoritative for dispatch policy and names none. Run `cas integrate \
                 violet` for a machine without a project proxy file, or add {} to that \
                 file's allowlist",
                token_state.as_str(),
                VIOLET_TOOLS
                    .iter()
                    .map(|tool| format!("\"{VIOLET_SERVER}.{tool}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
    }

    let missing = missing_probe_credentials(&states);
    if !missing.is_empty() {
        return DoctorRow {
            severity: DoctorSeverity::Error,
            message: format!("hub {endpoint}: {}; {CREDENTIALS_HINT}", missing.join(", ")),
        };
    }
    let credentials = states
        .iter()
        .map(|(name, _)| format!("{name} set"))
        .collect::<Vec<_>>()
        .join(", ");

    match probe.list_tools(server) {
        ProbeOutcome::Tools {
            tools,
            schema_problems,
        } => {
            let drift = tool_drift(&allowlist, &tools);
            if !schema_problems.is_empty() {
                // Claude Code drops a tool it cannot read, so a broken schema
                // is an outage for every Claude session even though the
                // tool list itself looks right.
                DoctorRow {
                    severity: DoctorSeverity::Error,
                    message: format!(
                        "hub {endpoint}: {}",
                        schema_problem_remedy(&schema_problems)
                    ),
                }
            } else if drift.is_empty() {
                DoctorRow {
                    severity: DoctorSeverity::Ok,
                    message: format!(
                        "registered ({credentials}); hub {endpoint} reachable and bearer accepted, \
                         answered with {} tool(s): {}; `cas violet post|thread|read` ready",
                        tools.len(),
                        tools.join(", ")
                    ),
                }
            } else {
                // Which file the stale entries live in is the whole
                // remediation: a project `.cas/proxy.toml` replaces the
                // machine allowlist, so "run the command" was a false remedy
                // until the command learned to rewrite that file too
                // (cas-a0ab).
                let source = allowlist_source(project_proxy, &paths.user_proxy);
                DoctorRow {
                    // A hub tool nothing admits is a live outage; a stale entry
                    // for a tool the hub retired is only clutter, so it must
                    // not turn `cas doctor` red for a machine that can post
                    // perfectly well right now.
                    severity: if drift.blocks_dispatch() {
                        DoctorSeverity::Error
                    } else {
                        DoctorSeverity::Warning
                    },
                    message: format!(
                        "{} (hub: {endpoint}). Run `cas integrate violet` to rewrite that file",
                        drift.describe(&tools, &allowlist, Some(source)),
                    ),
                }
            }
        }
        ProbeOutcome::Unauthorized => DoctorRow {
            severity: DoctorSeverity::Error,
            message: format!(
                "hub {endpoint} rejected this machine (HTTP 401 invalid_token; Authorization: Bearer \
                 <set>): the bearer is bad, so `cas violet` cannot post. {}",
                rejected_bearer_remedy(token_env, &paths.credentials_file)
            ),
        },
        ProbeOutcome::Unreachable { code } => DoctorRow {
            severity: DoctorSeverity::Warning,
            message: format!(
                "registered, but {} (hub: {endpoint}), so `cas violet` cannot reach it; run `cas integrate violet` once connectivity is back",
                probe_failure_detail(&code)
            ),
        },
        ProbeOutcome::Skipped { reason } => DoctorRow {
            severity: DoctorSeverity::Warning,
            message: format!("registered, but not verified ({reason}; hub: {endpoint})"),
        },
    }
}

fn probe_failure_detail(code: &str) -> String {
    let Some(name) = code.strip_prefix("missing_credential_env:") else {
        return format!("the hub did not answer ({code})");
    };
    if name.is_empty()
        || name.len() > 256
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return "a credential environment variable is unset".to_string();
    }
    format!("credential environment variable {name} is unset")
}

// ---------------------------------------------------------------------------
// Command entry point
// ---------------------------------------------------------------------------

/// The project `.cas/proxy.toml` that governs dispatch where this command was
/// invoked, resolved by the same ancestor walk the proxy loader uses so the
/// command repairs the very file `cas doctor` reads.
fn project_proxy_path() -> Option<PathBuf> {
    let path = crate::store::detect::find_cas_root()
        .ok()?
        .join("proxy.toml");
    ifs::is_regular_file(&path).then_some(path)
}

/// A default integration refresh keeps this machine's credential references,
/// spelled with their canonical `VIOLET_*` names: the same run renames the
/// credentials-file keys they expand from. Explicit --label/--token-env select
/// a new registration intentionally.
fn existing_machine_env_names(paths: &MachinePaths, url: &str) -> Result<Option<(String, String)>> {
    Ok(
        registered_machine_env_names(paths, url)?.map(|(token, bypass)| {
            (
                canonical_violet_credential_name(&token),
                canonical_violet_credential_name(&bypass),
            )
        }),
    )
}

fn registered_machine_env_names(
    paths: &MachinePaths,
    url: &str,
) -> Result<Option<(String, String)>> {
    let mut config = ProxyConfig::load_from(&paths.user_proxy)?;
    config.retire_legacy_hub_registration();
    if config
        .servers
        .get(VIOLET_SERVER)
        .is_some_and(|server| server_endpoint(server) == url)
        && let Some((Some(token), Some(bypass))) = config.violet_env_names()
    {
        return Ok(Some((token, bypass)));
    }
    if let Some(path) = paths
        .claude_json
        .as_deref()
        .filter(|path| ifs::is_regular_file(path))
    {
        let mut doc: serde_json::Value = serde_json::from_str(&ifs::read_capped(path)?)?;
        super::violet_retirement::retire_claude_entry(&mut doc);
        let entry = &doc["mcpServers"][VIOLET_SERVER];
        if entry["url"].as_str() == Some(url) {
            let token = entry["headers"]["Authorization"]
                .as_str()
                .and_then(|text| text.strip_prefix("Bearer ${"))
                .and_then(|text| text.strip_suffix('}'));
            let bypass = entry["headers"][VIOLET_BYPASS_HEADER]
                .as_str()
                .and_then(|text| text.strip_prefix("${"))
                .and_then(|text| text.strip_suffix('}'));
            if let (Some(token), Some(bypass)) = (token, bypass) {
                return Ok(Some((token.into(), bypass.into())));
            }
        }
    }
    if let Some(path) = paths
        .codex_config
        .as_deref()
        .filter(|path| ifs::is_regular_file(path))
    {
        let mut doc: toml_edit::DocumentMut = ifs::read_capped(path)?.parse()?;
        super::violet_retirement::retire_codex_entry(&mut doc);
        if let Some(entry) = doc
            .get("mcp_servers")
            .and_then(|servers| servers.get(VIOLET_SERVER))
            && entry.get("url").and_then(|value| value.as_str()) == Some(url)
        {
            let token = entry
                .get("bearer_token_env_var")
                .and_then(|value| value.as_str());
            let bypass = entry
                .get("env_http_headers")
                .and_then(|headers| headers.get(VIOLET_BYPASS_HEADER))
                .and_then(|value| value.as_str());
            if let (Some(token), Some(bypass)) = (token, bypass) {
                return Ok(Some((token.into(), bypass.into())));
            }
        }
    }
    Ok(None)
}

pub fn execute(args: &VioletArgs, json: bool, full: bool) -> Result<IntegrationOutcome> {
    if !args.dry_run && !args.no_harness {
        super::violet_retirement::retire_installed_hub(project_proxy_path().as_deref())?;
    }
    let env = ProcessEnv;
    let paths = MachinePaths::from_env(&env)?;
    let project_proxy = project_proxy_path();
    let device = ProcessDeviceIdentity;
    let label = resolve_label(args.label.as_deref(), device.hostname().as_deref());
    let mut effective_args = args.clone();
    effective_args.label = Some(label);
    if args.token_env.is_none()
        && args.label.is_none()
        && let Some((token, bypass)) = existing_machine_env_names(&paths, &args.url)?
    {
        effective_args.token_env = Some(token);
        if args.bypass_env == VIOLET_DEFAULT_BYPASS_ENV {
            effective_args.bypass_env = bypass;
        }
    }
    let credentials = if args.dry_run {
        Some((
            effective_args
                .label
                .clone()
                .unwrap_or_else(|| "UNKNOWN_HOST".to_string()),
            CredentialValues {
                token: String::new(),
                bypass: String::new(),
            },
        ))
    } else {
        let hub = ProcessHubClient;
        let vercel = ProcessBypassReader;
        let prompt = ProcessSecretPrompt;
        let provisioned =
            provision_credentials(&effective_args, &env, &hub, &vercel, &prompt, &device)?;
        effective_args.label = Some(provisioned.0.clone());
        Some(provisioned)
    };
    let credential_values = credentials.as_ref().map(|(_, values)| values);
    if !args.dry_run {
        if let Some(values) = credential_values {
            // The probe and generated harnesses use env-name references, while
            // this process must verify the freshly provisioned values immediately.
            unsafe {
                std::env::set_var(&effective_args.resolved_token_env(), &values.token);
                std::env::set_var(&effective_args.bypass_env, &values.bypass);
            }
        }
    }
    let report = run_with_credentials(
        &effective_args,
        project_proxy.as_deref(),
        &paths,
        &env,
        &ProxyHubProbe,
        credential_values,
    )?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }

    // "Already configured" is a claim about dispatch, not about one file: a
    // project proxy this run had to repair means the machine was *not*
    // already configured, however untouched the machine file was.
    let changed_anything =
        |state: WriteState| matches!(state, WriteState::Written | WriteState::Planned);
    let wrote_anything = changed_anything(report.registration)
        || changed_anything(report.credentials)
        || changed_anything(report.login_profile)
        || report
            .project_proxy
            .as_ref()
            .is_some_and(|entry| changed_anything(entry.state))
        || report
            .harnesses
            .iter()
            .any(|harness| changed_anything(harness.state));
    let status = if !report.credentials_ready() || !report.drift.is_empty() {
        IntegrationStatus::Stale
    } else {
        match &report.probe {
            ProbeOutcome::Unauthorized | ProbeOutcome::Unreachable { .. } => {
                IntegrationStatus::TransportError
            }
            _ if wrote_anything => IntegrationStatus::Configured,
            _ => IntegrationStatus::AlreadyConfigured,
        }
    };

    let mut outcome = IntegrationOutcome::new(Platform::Violet, IntegrationAction::Init, status);
    outcome.summary.push(format!("hub: {}", report.url));
    outcome.summary.push(format!(
        "credentials: {}",
        report
            .probe_env_states
            .iter()
            .map(|(name, state)| format!("{name} {}", state.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    outcome.summary.push(format!(
        "credentials file: {} ({}){}",
        report.credentials.as_str(),
        report.credentials_path.display(),
        if report.renamed_credentials.is_empty() {
            String::new()
        } else {
            format!(
                "; keys renamed to {}",
                report.renamed_credentials.join(", ")
            )
        }
    ));
    if let Some(profile) = &report.login_profile_path {
        outcome.summary.push(format!(
            "login profile: {} ({})",
            report.login_profile.as_str(),
            profile.display()
        ));
    }
    outcome.summary.push(format!(
        "machine registration: {} ({})",
        report.registration.as_str(),
        report.registration_path.display()
    ));
    outcome
        .summary
        .push(format!("allowlist: {}", report.allowlist.join(", ")));
    if let Some(entry) = &report.project_proxy {
        outcome.summary.push(format!(
            "project proxy: {} ({}) — {}",
            entry.state.as_str(),
            entry.path.display(),
            entry.note
        ));
    }
    for harness in &report.harnesses {
        outcome.summary.push(format!(
            "{}: {}{}",
            harness.harness,
            harness.state.as_str(),
            harness
                .note
                .as_deref()
                .map(|note| format!(" ({note})"))
                .unwrap_or_default()
        ));
    }
    match &report.probe {
        ProbeOutcome::Tools {
            tools,
            schema_problems,
        } => {
            outcome.summary.push(format!(
                "authenticated tools/list: {} tool(s): {}",
                tools.len(),
                tools.join(", ")
            ));
            if !schema_problems.is_empty() {
                outcome.summary.push(format!(
                    "{VIOLET_POST_TOOL} schema: {}",
                    schema_problems.join("; ")
                ));
            }
        }
        ProbeOutcome::Unauthorized => outcome.summary.push(
            "authenticated tools/list: refused (HTTP 401; Authorization: Bearer <set>)".to_string(),
        ),
        ProbeOutcome::Unreachable { code } => outcome
            .summary
            .push(format!("authenticated tools/list: unreachable ({code})")),
        ProbeOutcome::Skipped { reason } => outcome
            .summary
            .push(format!("authenticated tools/list: skipped ({reason})")),
    }
    if let Some(remedy) = &report.remedy {
        outcome.summary.push(format!("next: {remedy}"));
    }
    if matches!(report.registration, WriteState::Written) {
        outcome.files.push(report.registration_path.clone());
    }
    if matches!(report.credentials, WriteState::Written) {
        outcome.files.push(report.credentials_path.clone());
    }
    if matches!(report.login_profile, WriteState::Written)
        && let Some(path) = &report.login_profile_path
    {
        outcome.files.push(path.clone());
    }
    if let Some(entry) = &report.project_proxy
        && matches!(entry.state, WriteState::Written)
    {
        outcome.files.push(entry.path.clone());
    }
    for harness in &report.harnesses {
        if matches!(harness.state, WriteState::Written)
            && let Some(path) = &harness.path
        {
            outcome.files.push(path.clone());
        }
    }

    // Refuse loudly on a rejected credential so a scripted setup fails here
    // rather than at the first release post.
    if matches!(report.probe, ProbeOutcome::Unauthorized) {
        if !json {
            super::render_summary(&outcome.summary, full)?;
        }
        anyhow::bail!(
            "Violet refused this machine's credential (HTTP 401). Nothing was verified; the \
             registration on disk still names {} and holds no secret.",
            report.token_env
        );
    }
    Ok(outcome)
}

/// Convenience used by `cas doctor`: resolve machine paths from the real
/// environment and produce the row.
pub fn doctor_row_from_env(project_proxy: Option<&Path>) -> Option<DoctorRow> {
    let env = ProcessEnv;
    let paths = MachinePaths::from_env(&env).ok()?;
    Some(doctor_row(project_proxy, &paths, &env, &ProxyHubProbe))
}

/// A stable, order-independent view of the credential-bearing strings a
/// generated artifact contains. Used by the leak tests.
#[cfg(test)]
fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    const FAKE_TOKEN: &str = "xoxb-fake-secret-value-do-not-leak";
    const FAKE_BYPASS: &str = "bypass-secret-do-not-leak";
    const TEST_LABEL: &str = "SOUNDWAVE";
    const TEST_TOKEN_ENV: &str = "VIOLET_SLACK_TOKEN_SOUNDWAVE";

    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn worker_machine_bootstrap_respects_denials_and_supervisor_retains_credentials_gh_1047() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let credentials = env.home().join("credentials.env");
        std::fs::write(
            &credentials,
            "export WORKER_DENIED_FIXTURE_TOKEN='denied-fixture'\n\
             export WORKER_ALLOWED_FIXTURE_TOKEN='allowed-fixture'\n\
             export WORKER_EMPTY_FIXTURE_TOKEN='   '\n\
             export WORKER_EXPORTED_EMPTY_FIXTURE_TOKEN='file-fixture'\n",
        )
        .unwrap();
        env.set("CAS_CREDENTIALS_FILE", &credentials);
        env.remove("WORKER_DENIED_FIXTURE_TOKEN");
        env.remove("WORKER_ALLOWED_FIXTURE_TOKEN");
        env.remove("WORKER_EMPTY_FIXTURE_TOKEN");
        env.set("WORKER_EXPORTED_EMPTY_FIXTURE_TOKEN", "");
        assert_eq!(
            load_machine_credentials_with_installer(
                &["WORKER_DENIED_FIXTURE_TOKEN".into()],
                |name, value| env.set(name, value),
            )
            .unwrap(),
            1,
        );
        assert!(std::env::var_os("WORKER_DENIED_FIXTURE_TOKEN").is_none());
        assert_eq!(
            std::env::var("WORKER_ALLOWED_FIXTURE_TOKEN").unwrap(),
            "allowed-fixture"
        );
        assert!(std::env::var_os("WORKER_EMPTY_FIXTURE_TOKEN").is_none());
        assert_eq!(
            std::env::var("WORKER_EXPORTED_EMPTY_FIXTURE_TOKEN").unwrap(),
            ""
        );
        assert_eq!(
            load_machine_credentials_with_installer(
                &["WORKER_DENIED_FIXTURE_TOKEN".into()],
                |name, value| env.set(name, value),
            )
            .unwrap(),
            0,
            "repeated worker bootstrap must not restore a denied credential",
        );
        env.set("WORKER_ALLOWED_FIXTURE_TOKEN", "existing-fixture");
        assert_eq!(
            load_machine_credentials_with_installer(&[], |name, value| env.set(name, value))
                .unwrap(),
            1,
        );
        assert_eq!(
            std::env::var("WORKER_DENIED_FIXTURE_TOKEN").unwrap(),
            "denied-fixture"
        );
        assert_eq!(
            std::env::var("WORKER_ALLOWED_FIXTURE_TOKEN").unwrap(),
            "existing-fixture"
        );
        assert!(std::env::var_os("WORKER_EMPTY_FIXTURE_TOKEN").is_none());
        assert_eq!(
            std::env::var("WORKER_EXPORTED_EMPTY_FIXTURE_TOKEN").unwrap(),
            ""
        );
    }

    struct FakeEnv(HashMap<String, String>);

    impl FakeEnv {
        fn with(pairs: &[(&str, &str)]) -> Self {
            Self(
                pairs
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            )
        }
    }

    impl EnvLookup for FakeEnv {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    struct FakeProbe(ProbeOutcome);

    impl HubProbe for FakeProbe {
        fn list_tools(&self, _server: &ServerConfig) -> ProbeOutcome {
            self.0.clone()
        }
    }

    #[derive(Default)]
    struct RecordingProbe(RefCell<Vec<ServerConfig>>);

    impl HubProbe for RecordingProbe {
        fn list_tools(&self, server: &ServerConfig) -> ProbeOutcome {
            self.0.borrow_mut().push(server.clone());
            live_tools()
        }
    }

    fn staging_probe_fixture(dir: &Path) -> (PathBuf, FakeEnv) {
        let project = write_project_proxy(
            dir,
            &format!(
                "allowlist = {:?}\n[servers.violet]\ntransport = \"http\"\nurl = \"https://staging.example.test/mcp/slack\"\nauth = \"env:STAGING_TOKEN\"\n[servers.violet.headers]\n{VIOLET_BYPASS_HEADER} = \"env:STAGING_BYPASS\"\nx-project-key = \"env:STAGING_KEY\"\n",
                canonical_entries()
            ),
        );
        let mut env = ready_env();
        for name in ["STAGING_TOKEN", "STAGING_BYPASS", "STAGING_KEY"] {
            env.0.insert(name.into(), "fixture-secret".into());
        }
        (project, env)
    }

    #[test]
    fn cas_8121_integrate_probes_project_override() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let (project, env) = staging_probe_fixture(dir.path());
        let probe = RecordingProbe::default();
        let report = run(&test_args(), Some(&project), &paths, &env, &probe).unwrap();
        let config = ProxyConfig::load_from(&project).unwrap();
        assert_eq!(
            probe.0.borrow().as_slice(),
            &[config.servers[VIOLET_SERVER].clone()]
        );
        assert!(report.is_green(), "{report:?}");
        assert_eq!(report.url, "https://staging.example.test/mcp/slack");
    }

    #[test]
    fn cas_8121_doctor_probes_project_override_and_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let (project, env) = staging_probe_fixture(dir.path());
        run(&test_args(), None, &paths, &env, &FakeProbe(live_tools())).unwrap();
        let probe = RecordingProbe::default();
        let row = doctor_row(Some(&project), &paths, &env, &probe);
        let config = ProxyConfig::load_from(&project).unwrap();
        assert_eq!(
            probe.0.borrow().as_slice(),
            &[config.servers[VIOLET_SERVER].clone()]
        );
        assert_eq!(row.severity, DoctorSeverity::Ok);
        assert!(
            row.message
                .contains("https://staging.example.test/mcp/slack")
        );
    }

    #[test]
    fn cas_8121_doctor_probes_machine_url_without_project_override() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = VioletArgs {
            url: "https://machine.example.test/mcp/slack".into(),
            ..test_args()
        };
        let env = ready_env();
        run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();
        let probe = RecordingProbe::default();
        let row = doctor_row(None, &paths, &env, &probe);
        assert_eq!(server_endpoint(&probe.0.borrow()[0]), args.url);
        assert!(row.message.contains(&args.url));
    }

    #[test]
    fn cas_8121_project_credentials_do_not_require_machine_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let (project, mut env) = staging_probe_fixture(dir.path());
        env.0.retain(|name, _| name.starts_with("STAGING_"));
        let probe = RecordingProbe::default();
        let report = run(&test_args(), Some(&project), &paths, &env, &probe).unwrap();
        assert!(report.is_green(), "{report:?}");
        assert!(report.remedy.is_none(), "{report:?}");
        assert_eq!(probe.0.borrow().len(), 1);
        assert_eq!(
            doctor_row(Some(&project), &paths, &env, &probe).severity,
            DoctorSeverity::Ok
        );
        assert_eq!(probe.0.borrow().len(), 2);
    }

    #[test]
    fn cas_8121_missing_custom_header_stops_probe_without_exposing_values() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let (project, mut env) = staging_probe_fixture(dir.path());
        env.0.remove("STAGING_KEY");
        let probe = RecordingProbe::default();
        let report = run(&test_args(), Some(&project), &paths, &env, &probe).unwrap();
        assert!(!report.credentials_ready());
        assert!(matches!(report.probe, ProbeOutcome::Skipped { .. }));
        assert!(report.remedy.unwrap().contains("STAGING_KEY"));
        let row = doctor_row(Some(&project), &paths, &env, &probe);
        assert_eq!(row.severity, DoctorSeverity::Error);
        assert!(row.message.contains("STAGING_KEY"));
        assert!(!row.message.contains("fixture-secret"));
        assert!(probe.0.borrow().is_empty());
    }

    #[test]
    fn cas_8121_sse_override_preserves_transport_and_headers() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let (project, env) = staging_probe_fixture(dir.path());
        let content = std::fs::read_to_string(&project)
            .unwrap()
            .replace("transport = \"http\"", "transport = \"sse\"");
        std::fs::write(&project, content).unwrap();
        let probe = RecordingProbe::default();
        run(&test_args(), Some(&project), &paths, &env, &probe).unwrap();
        assert_eq!(
            doctor_row(Some(&project), &paths, &env, &probe).severity,
            DoctorSeverity::Ok
        );
        let config = ProxyConfig::load_from(&project).unwrap();
        assert!(matches!(&probe.0.borrow()[0], ServerConfig::Sse { .. }));
        assert_eq!(
            probe.0.borrow().as_slice(),
            &[
                config.servers[VIOLET_SERVER].clone(),
                config.servers[VIOLET_SERVER].clone()
            ]
        );
    }

    struct FakeHub {
        creates: RefCell<Vec<std::result::Result<(String, Option<String>), HubClientError>>>,
        bypasses: RefCell<Vec<std::result::Result<String, HubClientError>>>,
        labels: RefCell<Vec<String>>,
        cloud_tokens: RefCell<Vec<String>>,
    }

    fn take_hub_response<T>(responses: &RefCell<Vec<T>>, method: &str) -> T {
        let mut responses = responses.borrow_mut();
        assert!(
            !responses.is_empty(),
            "FakeHub::{method} called unexpectedly: no queued response remains"
        );
        responses.remove(0)
    }

    impl HubClient for FakeHub {
        fn create_client(
            &self,
            _hub_url: &str,
            cloud_token: &str,
            label: &str,
        ) -> std::result::Result<(String, Option<String>), HubClientError> {
            self.labels.borrow_mut().push(label.to_string());
            self.cloud_tokens.borrow_mut().push(cloud_token.to_string());
            take_hub_response(&self.creates, "create_client")
        }

        fn fetch_bypass(
            &self,
            _hub_url: &str,
            cloud_token: &str,
        ) -> std::result::Result<String, HubClientError> {
            self.cloud_tokens.borrow_mut().push(cloud_token.to_string());
            take_hub_response(&self.bypasses, "fetch_bypass")
        }
    }

    struct FakeBypassReader {
        result: std::result::Result<String, BypassReadError>,
        calls: RefCell<usize>,
    }

    impl BypassReader for FakeBypassReader {
        fn read(
            &self,
            _vercel_token: &str,
            _project: &str,
        ) -> std::result::Result<String, BypassReadError> {
            *self.calls.borrow_mut() += 1;
            self.result.clone()
        }
    }

    struct FakePrompt {
        value: String,
        calls: RefCell<usize>,
    }

    impl SecretPrompt for FakePrompt {
        fn read(&self) -> Result<String> {
            *self.calls.borrow_mut() += 1;
            Ok(self.value.clone())
        }
    }

    struct FakeDevice {
        hostname: Option<String>,
        device_id: Option<String>,
    }

    impl DeviceIdentity for FakeDevice {
        fn hostname(&self) -> Option<String> {
            self.hostname.clone()
        }

        fn device_id(&self) -> Result<Option<String>> {
            Ok(self.device_id.clone())
        }
    }

    fn live_tools() -> ProbeOutcome {
        ProbeOutcome::Tools {
            tools: VIOLET_TOOLS.iter().map(|t| t.to_string()).collect(),
            schema_problems: Vec::new(),
        }
    }

    fn paths_in(dir: &Path) -> MachinePaths {
        MachinePaths {
            user_proxy: dir.join("config").join("code-mode-mcp").join("config.toml"),
            claude_json: Some(dir.join("home").join(".claude.json")),
            claude_profiles: Vec::new(),
            codex_config: Some(dir.join("home").join(".codex").join("config.toml")),
            credentials_file: dir
                .join("home")
                .join(".config")
                .join("cas")
                .join("credentials.env"),
            login_profile: Some(dir.join("home").join(".profile")),
        }
    }

    fn ready_env() -> FakeEnv {
        let mut values = HashMap::new();
        values.insert(TEST_TOKEN_ENV.to_string(), FAKE_TOKEN.to_string());
        values.insert(
            VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            FAKE_BYPASS.to_string(),
        );
        FakeEnv(values)
    }

    #[test]
    fn legacy_credentials_provision_without_cloud_and_new_values_win() {
        let args = test_args();
        let hub = FakeHub {
            creates: RefCell::new(Vec::new()),
            bypasses: RefCell::new(Vec::new()),
            labels: RefCell::new(Vec::new()),
            cloud_tokens: RefCell::new(Vec::new()),
        };
        let vercel = FakeBypassReader {
            result: Ok("unused".to_string()),
            calls: RefCell::new(0),
        };
        let prompt = FakePrompt {
            value: "unused".to_string(),
            calls: RefCell::new(0),
        };
        let device = FakeDevice {
            hostname: Some("soundwave".to_string()),
            device_id: None,
        };
        let mut env = FakeEnv::with(&[
            (
                cmcp_core::config::violet_credential_names(TEST_TOKEN_ENV)[1].as_str(),
                "legacy-token",
            ),
            (
                violet_compatibility().legacy_bypass_env.as_str(),
                "legacy-bypass",
            ),
        ]);
        for expected in [
            ("legacy-token", "legacy-bypass"),
            ("new-token", "new-bypass"),
        ] {
            let (_, values) = provision_credentials_with_cloud_token(
                &args, &env, None, &hub, &vercel, &prompt, &device,
            )
            .unwrap();
            assert_eq!(values.token, expected.0);
            assert_eq!(values.bypass, expected.1);
            assert_eq!(EnvState::of(&env, TEST_TOKEN_ENV), EnvState::Set);
            env.0
                .insert(TEST_TOKEN_ENV.to_string(), "new-token".to_string());
            env.0.insert(
                VIOLET_DEFAULT_BYPASS_ENV.to_string(),
                "new-bypass".to_string(),
            );
        }
        assert_eq!(*vercel.calls.borrow(), 0);
        assert_eq!(*prompt.calls.borrow(), 0);
    }

    #[test]
    fn production_alias_tools_do_not_report_contract_drift() {
        let allowlisted = VIOLET_TOOLS.map(str::to_string);
        let live: Vec<_> = VIOLET_TOOLS
            .iter()
            .map(|tool| (*tool).to_owned())
            .chain(violet_compatibility().retired_tools.iter().cloned())
            .collect();
        assert!(tool_drift(&allowlisted, &live).is_empty());
        let missing_read: Vec<_> = std::iter::once("violet_post".to_owned())
            .chain(violet_compatibility().retired_tools.iter().cloned())
            .collect();
        assert_eq!(
            tool_drift(&allowlisted, &missing_read).retired,
            ["violet_read"]
        );
    }

    #[test]
    fn legacy_project_registration_retires_alias_and_preserves_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let project = write_project_proxy(
            dir.path(),
            &format!(
                r#"
# Legacy project registration is retired.
allowlist = ["{retired}.{read}", "{retired}.{post}", "neon.run_sql"]
worker_read_routes = ["{retired}.{read}"]
[worker_access]
{retired} = "read-only"
[servers.{retired}]
transport = "http"
url = "{url}"
auth = "env:{token}"
"#,
                retired = violet_compatibility().retired_server,
                read = violet_compatibility().retired_tools[0],
                post = violet_compatibility().retired_tools[1],
                url = violet_hub_url(),
                token = cmcp_core::config::violet_credential_names(TEST_TOKEN_ENV)[1]
            ),
        );
        let args = VioletArgs {
            no_harness: true,
            ..test_args()
        };
        let report = run(
            &args,
            Some(&project),
            &paths,
            &ready_env(),
            &FakeProbe(live_tools()),
        )
        .unwrap();
        assert!(report.is_green(), "{report:?}");
        let config = ProxyConfig::load_from(&project).unwrap();
        assert!(
            !config
                .servers
                .contains_key(&violet_compatibility().retired_server)
        );
        // GH #1164: the retired block pointed at this machine's hub with a
        // per-machine token name, so it is not renamed into a project-level
        // [servers.violet]; the machine registration supplies the server.
        assert!(!config.servers.contains_key(VIOLET_SERVER), "{config:?}");
        let note = &report.project_proxy.as_ref().unwrap().note;
        assert!(note.contains("same hub URL"), "{note}");
        assert_eq!(config.violet_allowlisted_tools(), VIOLET_TOOLS);
        assert_eq!(
            config.worker_access.get(VIOLET_SERVER),
            Some(&cmcp_core::config::WorkerAccess::ReadOnly)
        );
        assert!(
            config
                .worker_read_routes
                .iter()
                .any(|route| route.server == VIOLET_SERVER && route.tool == "violet_read")
        );
        assert!(config.allowlist.iter().any(|route| route.server == "neon"));
        assert!(
            std::fs::read_to_string(&project)
                .unwrap()
                .contains("# Legacy project")
        );
        let again = run(
            &args,
            Some(&project),
            &paths,
            &ready_env(),
            &FakeProbe(live_tools()),
        )
        .unwrap();
        assert_eq!(
            again.project_proxy.unwrap().state,
            WriteState::AlreadyCurrent
        );
    }

    #[test]
    fn default_refresh_keeps_installed_machine_credentials_under_violet_names() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let c = violet_compatibility();
        let token = format!("{}_LAPTOP", c.legacy_token_prefix);
        let expected = Some((
            "VIOLET_SLACK_TOKEN_LAPTOP".to_string(),
            VIOLET_DEFAULT_BYPASS_ENV.to_string(),
        ));
        let mut config = ProxyConfig::default();
        config.add_server(
            c.retired_server.clone(),
            ServerConfig::Http {
                url: c.legacy_hub_url.clone(),
                auth: Some(format!("env:{token}")),
                headers: HashMap::from([(
                    VIOLET_BYPASS_HEADER.into(),
                    format!("env:{}", c.legacy_bypass_env),
                )]),
                oauth: false,
            },
        );
        config.save_to(&paths.user_proxy).unwrap();
        assert_eq!(
            existing_machine_env_names(&paths, violet_hub_url()).unwrap(),
            expected
        );
        std::fs::remove_file(&paths.user_proxy).unwrap();
        let path = paths.claude_json.as_deref().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::json!({"mcpServers":{(c.retired_server.as_str()):{"url":c.legacy_hub_url,"headers":{"Authorization":format!("Bearer ${{{token}}}"), VIOLET_BYPASS_HEADER:format!("${{{}}}",c.legacy_bypass_env)}}}}).to_string()).unwrap();
        assert_eq!(
            existing_machine_env_names(&paths, violet_hub_url()).unwrap(),
            expected
        );
        std::fs::remove_file(path).unwrap();
        let path = paths.codex_config.as_deref().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("[mcp_servers.{}]\nurl = {:?}\nbearer_token_env_var = {:?}\nenv_http_headers = {{ {} = {:?} }}\n", c.retired_server, c.legacy_hub_url, token, VIOLET_BYPASS_HEADER, c.legacy_bypass_env)).unwrap();
        assert_eq!(
            existing_machine_env_names(&paths, violet_hub_url()).unwrap(),
            expected
        );
        assert!(
            existing_machine_env_names(&paths, "https://custom.example/mcp")
                .unwrap()
                .is_none()
        );
    }

    fn legacy_credentials_file(paths: &MachinePaths, keep: &str) -> String {
        let c = violet_compatibility();
        let text = format!(
            "# machine secrets\nexport KEEP='unrelated'\n{keep}export {}_LAPTOP='{FAKE_TOKEN}'\nexport {}='{FAKE_BYPASS}'\n",
            c.legacy_token_prefix, c.legacy_bypass_env
        );
        std::fs::create_dir_all(paths.credentials_file.parent().unwrap()).unwrap();
        std::fs::write(&paths.credentials_file, &text).unwrap();
        text
    }

    #[test]
    fn integrate_renames_legacy_credential_keys_idempotently_without_reporting_values() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        legacy_credentials_file(&paths, "");
        let args = VioletArgs {
            no_harness: true,
            ..test_args()
        };
        let report = run(&args, None, &paths, &ready_env(), &FakeProbe(live_tools())).unwrap();
        assert_eq!(
            report.renamed_credentials,
            ["VIOLET_SLACK_TOKEN_LAPTOP", VIOLET_DEFAULT_BYPASS_ENV]
        );
        assert_eq!(report.credentials, WriteState::Written);
        let renamed = std::fs::read_to_string(&paths.credentials_file).unwrap();
        assert_eq!(
            renamed,
            format!(
                "# machine secrets\nexport KEEP='unrelated'\nexport VIOLET_SLACK_TOKEN_LAPTOP='{FAKE_TOKEN}'\nexport VIOLET_VERCEL_BYPASS='{FAKE_BYPASS}'\n"
            )
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&paths.credentials_file)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        for rendered in [
            serde_json::to_string(&report).unwrap(),
            format!("{report:?}"),
        ] {
            assert!(
                !contains_any(&rendered, &[FAKE_TOKEN, FAKE_BYPASS]),
                "{rendered}"
            );
        }

        let again = run(&args, None, &paths, &ready_env(), &FakeProbe(live_tools())).unwrap();
        assert!(again.renamed_credentials.is_empty());
        assert_eq!(again.credentials, WriteState::Skipped);
        assert_eq!(
            std::fs::read_to_string(&paths.credentials_file).unwrap(),
            renamed
        );
        assert!(
            !serde_json::to_string(&again)
                .unwrap()
                .contains("renamed_credentials")
        );
    }

    #[test]
    fn credential_rename_resolves_both_generations_and_dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        // A populated canonical key wins; the legacy duplicate is dropped.
        legacy_credentials_file(&paths, "export VIOLET_SLACK_TOKEN_LAPTOP='newer'\n");
        assert_eq!(
            rename_legacy_credentials(&paths.credentials_file, false).unwrap(),
            ["VIOLET_SLACK_TOKEN_LAPTOP", VIOLET_DEFAULT_BYPASS_ENV]
        );
        assert_eq!(
            std::fs::read_to_string(&paths.credentials_file).unwrap(),
            format!(
                "# machine secrets\nexport KEEP='unrelated'\nexport VIOLET_SLACK_TOKEN_LAPTOP='newer'\nexport VIOLET_VERCEL_BYPASS='{FAKE_BYPASS}'\n"
            )
        );
        assert!(
            rename_legacy_credentials(&paths.credentials_file, false)
                .unwrap()
                .is_empty()
        );

        // An empty canonical key would shadow the value it falls back to.
        legacy_credentials_file(&paths, "export VIOLET_SLACK_TOKEN_LAPTOP=''\n");
        rename_legacy_credentials(&paths.credentials_file, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&paths.credentials_file).unwrap(),
            format!(
                "# machine secrets\nexport KEEP='unrelated'\nexport VIOLET_SLACK_TOKEN_LAPTOP='{FAKE_TOKEN}'\nexport VIOLET_VERCEL_BYPASS='{FAKE_BYPASS}'\n"
            )
        );

        let original = legacy_credentials_file(&paths, "");
        assert_eq!(
            rename_legacy_credentials(&paths.credentials_file, true)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            std::fs::read_to_string(&paths.credentials_file).unwrap(),
            original
        );
        std::fs::remove_file(&paths.credentials_file).unwrap();
        assert!(
            rename_legacy_credentials(&paths.credentials_file, false)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn integrate_moves_an_installed_project_registration_to_violet_hub() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let c = violet_compatibility();
        let legacy = duplicate_server_block()
            .replace(violet_hub_url(), &c.legacy_hub_url)
            .replace(
                TEST_TOKEN_ENV,
                &cmcp_core::config::violet_credential_names(TEST_TOKEN_ENV)[1],
            )
            .replace(
                &format!("env:{VIOLET_DEFAULT_BYPASS_ENV}"),
                &format!("env:{}", c.legacy_bypass_env),
            );
        let project = write_project_proxy(
            dir.path(),
            &format!("allowlist = [\"violet.violet_read\", \"violet.violet_post\"]\n\n{legacy}"),
        );
        let args = VioletArgs {
            no_harness: true,
            ..test_args()
        };
        let report = run(
            &args,
            Some(&project),
            &paths,
            &ready_env(),
            &FakeProbe(live_tools()),
        )
        .unwrap();
        assert!(report.is_green(), "{report:?}");
        assert_eq!(report.url, "https://violet-hub.vercel.app/mcp/slack");
        let project_raw = std::fs::read_to_string(&project).unwrap();
        assert!(
            !project_raw.contains("[servers.violet]"),
            "identical to the machine registration once migrated: {project_raw}"
        );
        let machine = std::fs::read_to_string(&paths.user_proxy).unwrap();
        assert!(
            machine.contains("url = \"https://violet-hub.vercel.app/mcp/slack\""),
            "{machine}"
        );
        assert!(machine.contains(&format!("auth = \"env:{TEST_TOKEN_ENV}\"")));
        assert!(machine.contains(&format!("\"env:{VIOLET_DEFAULT_BYPASS_ENV}\"")));
        for text in [&project_raw, &machine] {
            for legacy in [
                c.legacy_hub_url.as_str(),
                c.legacy_token_prefix.as_str(),
                c.legacy_bypass_env.as_str(),
            ] {
                assert!(!text.contains(legacy), "{legacy} survived:\n{text}");
            }
        }
    }

    #[test]
    fn doctor_and_violet_skill_carry_no_retired_hub_vocabulary() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let c = violet_compatibility();
        let legacy_token = cmcp_core::config::violet_credential_names(TEST_TOKEN_ENV)[1].clone();
        // An installed machine: former hostname, legacy names in the file and
        // in the environment.
        let mut config = ProxyConfig::default();
        config.ensure_violet_registration(
            violet_hub_url(),
            TEST_TOKEN_ENV,
            VIOLET_DEFAULT_BYPASS_ENV,
        );
        config.add_server(
            VIOLET_SERVER.to_string(),
            ServerConfig::Http {
                url: c.legacy_hub_url.clone(),
                auth: Some(format!("env:{legacy_token}")),
                headers: HashMap::from([(
                    VIOLET_BYPASS_HEADER.into(),
                    format!("env:{}", c.legacy_bypass_env),
                )]),
                oauth: false,
            },
        );
        config.save_to(&paths.user_proxy).unwrap();
        let env = FakeEnv::with(&[
            (legacy_token.as_str(), FAKE_TOKEN),
            (c.legacy_bypass_env.as_str(), FAKE_BYPASS),
        ]);
        let row = doctor_row(None, &paths, &env, &FakeProbe(live_tools()));
        assert_eq!(row.severity, DoctorSeverity::Ok, "{row:?}");
        assert!(row.message.contains(violet_hub_url()), "{row:?}");
        assert!(
            row.message.contains(&format!("{TEST_TOKEN_ENV} set")),
            "{row:?}"
        );

        let mut texts = vec![("doctor row", row.message)];
        for (path, text) in [
            (
                "SKILL.md",
                include_str!("../../builtins/skills/violet/SKILL.md"),
            ),
            (
                "references/attachments.md",
                include_str!("../../builtins/skills/violet/references/attachments.md"),
            ),
            (
                "references/contract.md",
                include_str!("../../builtins/skills/violet/references/contract.md"),
            ),
            (
                "references/publication.md",
                include_str!("../../builtins/skills/violet/references/publication.md"),
            ),
            (
                "references/registration.md",
                include_str!("../../builtins/skills/violet/references/registration.md"),
            ),
        ] {
            texts.push((path, text.to_string()));
        }
        for (source, text) in texts {
            let lower = text.to_ascii_lowercase();
            for legacy in [
                c.legacy_hub_url.as_str(),
                c.retired_server.as_str(),
                c.legacy_token_prefix.as_str(),
                c.legacy_bypass_env.as_str(),
                c.retired_tools[0].as_str(),
                c.retired_tools[1].as_str(),
            ] {
                assert!(
                    !lower.contains(&legacy.to_ascii_lowercase()),
                    "{source} names {legacy}"
                );
            }
        }
    }

    fn test_args() -> VioletArgs {
        VioletArgs {
            label: Some(TEST_LABEL.to_string()),
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn hostname_is_the_default_label_and_label_override_is_folded() {
        assert_eq!(resolve_label(None, Some("soundwave")), "SOUNDWAVE");
        assert_eq!(
            resolve_label(Some("Daniel-laptop"), Some("soundwave")),
            "DANIEL_LAPTOP"
        );
    }

    #[test]
    fn credentials_upsert_preserves_unrelated_exports_and_profile_uses_login_shell() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = dir.path().join("config").join("credentials.env");
        std::fs::create_dir_all(credentials.parent().unwrap()).unwrap();
        std::fs::write(
            &credentials,
            "export KEEP='unrelated'\nexport VIOLET_VERCEL_BYPASS='old'\n",
        )
        .unwrap();

        write_credentials(
            &credentials,
            "VIOLET_SLACK_TOKEN_SOUNDWAVE",
            FAKE_TOKEN,
            VIOLET_DEFAULT_BYPASS_ENV,
            FAKE_BYPASS,
        )
        .unwrap();
        let written = std::fs::read_to_string(&credentials).unwrap();
        assert!(written.contains("export KEEP='unrelated'"));
        assert!(written.contains(&format!(
            "export VIOLET_SLACK_TOKEN_SOUNDWAVE='{FAKE_TOKEN}'"
        )));
        assert!(written.contains(&format!(
            "export {VIOLET_DEFAULT_BYPASS_ENV}='{FAKE_BYPASS}'"
        )));
        assert_eq!(
            std::fs::metadata(&credentials)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let profile = dir.path().join(".profile");
        ensure_profile_line(&profile, &credentials).unwrap();
        let profile_text = std::fs::read_to_string(&profile).unwrap();
        assert!(profile_text.contains(&profile_source_line(&credentials)));
        assert!(!profile_text.contains(".bashrc"));
    }

    #[test]
    fn provisioning_mints_with_cloud_login_and_hostname_label() {
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            ..test_args()
        };
        let hub = FakeHub {
            creates: RefCell::new(vec![Ok((
                FAKE_TOKEN.to_string(),
                Some(FAKE_BYPASS.to_string()),
            ))]),
            bypasses: RefCell::new(Vec::new()),
            labels: RefCell::new(Vec::new()),
            cloud_tokens: RefCell::new(Vec::new()),
        };
        let vercel = FakeBypassReader {
            result: Err(BypassReadError::InvalidResponse),
            calls: RefCell::new(0),
        };
        let prompt = FakePrompt {
            value: "prompted-secret".to_string(),
            calls: RefCell::new(0),
        };
        let device = FakeDevice {
            hostname: Some("soundwave".to_string()),
            device_id: Some("device-123456".to_string()),
        };

        let (label, values) = provision_credentials_with_cloud_token(
            &args,
            &FakeEnv::with(&[]),
            Some("cloud-bearer"),
            &hub,
            &vercel,
            &prompt,
            &device,
        )
        .unwrap();
        assert_eq!(label, "SOUNDWAVE");
        assert_eq!(values.token, FAKE_TOKEN);
        assert_eq!(values.bypass, FAKE_BYPASS);
        assert_eq!(hub.labels.borrow().as_slice(), &["SOUNDWAVE"]);
        assert_eq!(hub.cloud_tokens.borrow().as_slice(), &["cloud-bearer"]);
        assert_eq!(*vercel.calls.borrow(), 0);
        assert_eq!(*prompt.calls.borrow(), 0);
    }

    #[test]
    fn provisioning_retries_one_taken_label_with_device_suffix() {
        let args = VioletArgs {
            label: Some("Daniel-laptop".to_string()),
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            ..test_args()
        };
        let hub = FakeHub {
            creates: RefCell::new(vec![
                Err(HubClientError::LabelTaken),
                Ok((FAKE_TOKEN.to_string(), Some(FAKE_BYPASS.to_string()))),
            ]),
            bypasses: RefCell::new(Vec::new()),
            labels: RefCell::new(Vec::new()),
            cloud_tokens: RefCell::new(Vec::new()),
        };
        let vercel = FakeBypassReader {
            result: Err(BypassReadError::InvalidResponse),
            calls: RefCell::new(0),
        };
        let prompt = FakePrompt {
            value: "unused".to_string(),
            calls: RefCell::new(0),
        };
        let device = FakeDevice {
            hostname: Some("soundwave".to_string()),
            device_id: Some("abcdef-device".to_string()),
        };

        let (label, _) = provision_credentials_with_cloud_token(
            &args,
            &FakeEnv::with(&[]),
            Some("cloud-bearer"),
            &hub,
            &vercel,
            &prompt,
            &device,
        )
        .unwrap();
        assert_eq!(label, "DANIEL_LAPTOP_abcdef");
        assert_eq!(
            hub.labels.borrow().as_slice(),
            &["DANIEL_LAPTOP", "DANIEL_LAPTOP_abcdef"]
        );
    }

    #[test]
    fn missing_hub_mint_route_fails_closed_without_local_mint() {
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            ..test_args()
        };
        let hub = FakeHub {
            creates: RefCell::new(vec![Err(HubClientError::RouteUnavailable)]),
            bypasses: RefCell::new(Vec::new()),
            labels: RefCell::new(Vec::new()),
            cloud_tokens: RefCell::new(Vec::new()),
        };
        let vercel = FakeBypassReader {
            result: Ok(FAKE_BYPASS.to_string()),
            calls: RefCell::new(0),
        };
        let prompt = FakePrompt {
            value: "prompted-secret".to_string(),
            calls: RefCell::new(0),
        };
        let error = provision_credentials_with_cloud_token(
            &args,
            &FakeEnv::with(&[]),
            Some("cloud-bearer"),
            &hub,
            &vercel,
            &prompt,
            &FakeDevice {
                hostname: Some("soundwave".to_string()),
                device_id: Some("device-123456".to_string()),
            },
        )
        .unwrap_err();
        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("hub route POST /api/clients not available (violet_ps#5)"),
            "{rendered}"
        );
        assert!(!rendered.contains(FAKE_TOKEN));
        assert_eq!(*vercel.calls.borrow(), 0);
        assert_eq!(*prompt.calls.borrow(), 0);
    }

    #[test]
    fn missing_hub_bypass_uses_read_only_vercel_then_hidden_prompt_once() {
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            ..test_args()
        };
        let env = FakeEnv::with(&[
            (TEST_TOKEN_ENV, FAKE_TOKEN),
            ("VERCEL_TOKEN", "vercel-bearer"),
        ]);
        let hub = FakeHub {
            creates: RefCell::new(Vec::new()),
            bypasses: RefCell::new(vec![Err(HubClientError::RouteUnavailable)]),
            labels: RefCell::new(Vec::new()),
            cloud_tokens: RefCell::new(Vec::new()),
        };
        let vercel = FakeBypassReader {
            result: Err(BypassReadError::HttpStatus(404)),
            calls: RefCell::new(0),
        };
        let prompt = FakePrompt {
            value: "prompted-secret".to_string(),
            calls: RefCell::new(0),
        };
        let device = FakeDevice {
            hostname: Some("soundwave".to_string()),
            device_id: Some("device-123456".to_string()),
        };
        let (_, values) = provision_credentials_with_cloud_token(
            &args,
            &env,
            Some("cloud-bearer"),
            &hub,
            &vercel,
            &prompt,
            &device,
        )
        .unwrap();
        assert_eq!(values.bypass, "prompted-secret");
        assert_eq!(*vercel.calls.borrow(), 1);
        assert_eq!(*prompt.calls.borrow(), 1);
    }

    #[test]
    fn label_selects_the_per_machine_bearer_variable() {
        let mut args = test_args();
        assert_eq!(args.resolved_token_env(), TEST_TOKEN_ENV);

        args.label = Some("daniel-laptop".to_string());
        assert_eq!(
            args.resolved_token_env(),
            "VIOLET_SLACK_TOKEN_DANIEL_LAPTOP"
        );

        // An explicit --token-env always wins over a label.
        args.token_env = Some("VIOLET_SLACK_TOKEN_CI".to_string());
        assert_eq!(args.resolved_token_env(), "VIOLET_SLACK_TOKEN_CI");
    }

    #[test]
    fn non_interactive_run_writes_env_reference_only_artifacts_and_prints_the_tool_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = VioletArgs {
            label: Some("laptop".to_string()),
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            ..Default::default()
        };
        let env = FakeEnv::with(&[
            ("VIOLET_SLACK_TOKEN_LAPTOP", FAKE_TOKEN),
            (VIOLET_DEFAULT_BYPASS_ENV, FAKE_BYPASS),
        ]);

        let report = run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();
        assert!(report.is_green(), "{report:?}");
        assert_eq!(report.registration, WriteState::Written);
        assert_eq!(report.allowlist, VIOLET_TOOLS);
        assert_eq!(report.token_env, "VIOLET_SLACK_TOKEN_LAPTOP");
        assert_eq!(report.token_env_state, EnvState::Set);
        assert_eq!(report.remedy, None);
        assert_eq!(
            report.probe,
            ProbeOutcome::Tools {
                tools: vec!["violet_read".to_string(), "violet_post".to_string()],
                schema_problems: Vec::new(),
            }
        );
        assert!(
            report
                .harnesses
                .iter()
                .all(|h| h.state == WriteState::Written),
            "{:?}",
            report.harnesses
        );

        // Every artifact names variables and holds no value.
        let secrets = [FAKE_TOKEN, FAKE_BYPASS];
        for path in [
            paths.user_proxy.clone(),
            paths.claude_json.clone().unwrap(),
            paths.codex_config.clone().unwrap(),
        ] {
            let written = std::fs::read_to_string(&path).unwrap();
            assert!(
                !contains_any(&written, &secrets),
                "{} leaked a credential value",
                path.display()
            );
            assert!(written.contains("VIOLET_SLACK_TOKEN_LAPTOP"), "{written}");
        }
        // …and neither does the report that becomes terminal/JSON output.
        let rendered = serde_json::to_string(&report).unwrap();
        assert!(!contains_any(&rendered, &secrets));

        // Idempotent: a second run rewrites nothing.
        let second = run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();
        assert_eq!(second.registration, WriteState::AlreadyCurrent);
        assert!(
            second
                .harnesses
                .iter()
                .all(|h| h.state == WriteState::AlreadyCurrent),
            "{:?}",
            second.harnesses
        );
    }

    #[test]
    fn integrated_credentials_are_written_and_profile_sourcing_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = VioletArgs {
            label: Some("laptop".to_string()),
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..Default::default()
        };
        let env = FakeEnv::with(&[
            ("VIOLET_SLACK_TOKEN_LAPTOP", FAKE_TOKEN),
            (VIOLET_DEFAULT_BYPASS_ENV, FAKE_BYPASS),
        ]);
        let values = CredentialValues {
            token: FAKE_TOKEN.to_string(),
            bypass: FAKE_BYPASS.to_string(),
        };
        let report = run_with_credentials(
            &args,
            None,
            &paths,
            &env,
            &FakeProbe(live_tools()),
            Some(&values),
        )
        .unwrap();
        assert_eq!(report.credentials, WriteState::Written);
        assert_eq!(report.login_profile, WriteState::Written);
        let credentials = std::fs::read_to_string(&paths.credentials_file).unwrap();
        assert!(credentials.contains(FAKE_TOKEN));
        assert!(credentials.contains(FAKE_BYPASS));

        let second = run_with_credentials(
            &args,
            None,
            &paths,
            &env,
            &FakeProbe(live_tools()),
            Some(&values),
        )
        .unwrap();
        assert_eq!(second.credentials, WriteState::AlreadyCurrent);
        assert_eq!(second.login_profile, WriteState::AlreadyCurrent);
    }

    #[cfg(unix)]
    #[test]
    fn integrated_credentials_write_through_a_symlinked_login_profile() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let mut paths = paths_in(dir.path());
        let profile_target = dir.path().join("real-profile");
        let profile_link = dir.path().join(".bash_profile");
        std::fs::write(&profile_target, "# operator profile\n").unwrap();
        symlink(&profile_target, &profile_link).unwrap();
        paths.login_profile = Some(profile_link);
        let args = VioletArgs {
            label: Some("laptop".to_string()),
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..Default::default()
        };
        let values = CredentialValues {
            token: FAKE_TOKEN.to_string(),
            bypass: FAKE_BYPASS.to_string(),
        };

        let report = run_with_credentials(
            &args,
            None,
            &paths,
            &ready_env(),
            &FakeProbe(live_tools()),
            Some(&values),
        )
        .unwrap();

        assert_eq!(report.login_profile, WriteState::Written);
        assert_eq!(
            report.login_profile_path,
            Some(profile_target.canonicalize().unwrap())
        );
        let written = std::fs::read_to_string(profile_target).unwrap();
        assert!(written.contains(&profile_source_line(&paths.credentials_file)));
    }

    #[test]
    fn missing_variable_names_the_variable_and_the_file_without_probing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            ..test_args()
        };
        let mut values = HashMap::new();
        values.insert(TEST_TOKEN_ENV.to_string(), "   ".to_string());
        let env = FakeEnv(values);

        let report = run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();
        assert!(!report.is_green());
        assert_eq!(report.token_env_state, EnvState::Empty);
        assert_eq!(report.bypass_env_state, EnvState::Unset);
        // The probe is never attempted with a known-bad credential.
        assert!(matches!(report.probe, ProbeOutcome::Skipped { .. }));
        let remedy = report.remedy.unwrap();
        assert!(remedy.contains(TEST_TOKEN_ENV), "{remedy}");
        assert!(remedy.contains("set but empty"), "{remedy}");
        assert!(remedy.contains(VIOLET_DEFAULT_BYPASS_ENV), "{remedy}");
        assert!(remedy.contains("credentials file"), "{remedy}");
        // The registration is still written, so the fix is a one-line edit.
        assert_eq!(report.registration, WriteState::Written);
    }

    #[test]
    fn unauthorized_probe_reports_redacted_header_state_and_a_mint_remedy() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            ..test_args()
        };

        let report = run(
            &args,
            None,
            &paths,
            &ready_env(),
            &FakeProbe(ProbeOutcome::Unauthorized),
        )
        .unwrap();
        assert!(!report.is_green());
        let remedy = report.remedy.unwrap();
        assert!(remedy.contains("401"), "{remedy}");
        assert!(remedy.contains("Bearer <set>"), "{remedy}");
        assert!(!remedy.contains(FAKE_TOKEN));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            dry_run: true,
            ..test_args()
        };
        let report = run(&args, None, &paths, &ready_env(), &FakeProbe(live_tools())).unwrap();
        assert_eq!(report.registration, WriteState::Planned);
        assert!(!paths.user_proxy.exists());
        assert!(!paths.claude_json.unwrap().exists());
        assert!(!paths.codex_config.unwrap().exists());
    }

    #[test]
    fn codex_registration_preserves_unrelated_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let codex = paths.codex_config.clone().unwrap();
        std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
        std::fs::write(
            &codex,
            "# operator comment worth keeping\nmodel = \"gpt-5\"\n\n\
             [mcp_servers.other]\ncommand = \"other-mcp\"\n",
        )
        .unwrap();

        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            ..test_args()
        };
        run(&args, None, &paths, &ready_env(), &FakeProbe(live_tools())).unwrap();

        let written = std::fs::read_to_string(&codex).unwrap();
        assert!(
            written.contains("# operator comment worth keeping"),
            "{written}"
        );
        assert!(written.contains("[mcp_servers.other]"), "{written}");
        assert!(
            written.contains(&format!("bearer_token_env_var = \"{}\"", TEST_TOKEN_ENV)),
            "{written}"
        );
        let parsed: toml::Value = toml::from_str(&written).unwrap();
        assert_eq!(
            parsed["mcp_servers"]["violet"]["env_http_headers"][VIOLET_BYPASS_HEADER].as_str(),
            Some(VIOLET_DEFAULT_BYPASS_ENV)
        );
    }

    #[test]
    fn claude_registration_preserves_unrelated_keys() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let claude = paths.claude_json.clone().unwrap();
        std::fs::create_dir_all(claude.parent().unwrap()).unwrap();
        std::fs::write(
            &claude,
            r#"{"numStartups":42,"projects":{"/tmp/x":{"allowedTools":[]}},
                "mcpServers":{"playwright":{"type":"stdio","command":"npx"}}}"#,
        )
        .unwrap();

        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            ..test_args()
        };
        run(&args, None, &paths, &ready_env(), &FakeProbe(live_tools())).unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&claude).unwrap()).unwrap();
        assert_eq!(written["numStartups"], 42);
        assert!(written["projects"]["/tmp/x"].is_object());
        assert_eq!(written["mcpServers"]["playwright"]["command"], "npx");
        assert_eq!(
            written["mcpServers"]["violet"]["headers"]["Authorization"],
            format!("Bearer ${{{TEST_TOKEN_ENV}}}")
        );
    }

    #[test]
    fn drift_names_both_the_retired_and_the_new_tool() {
        // A full rename is both halves at once, and it blocks dispatch.
        let allowlist = vec![
            "slack_post_message".to_string(),
            "slack_read_channel".to_string(),
        ];
        let live = vec!["violet_post".to_string(), "violet_read".to_string()];
        let drift = tool_drift(&allowlist, &live);
        assert_eq!(drift.unallowlisted, vec!["violet_post", "violet_read"]);
        assert_eq!(
            drift.retired,
            vec!["slack_post_message", "slack_read_channel"]
        );
        assert!(drift.blocks_dispatch());
        let described = drift.describe(&live, &allowlist, None);
        assert!(described.contains("denied by policy"), "{described}");
        assert!(described.contains("slack_post_message"), "{described}");

        // A stale leftover next to the live routes is NOT an outage: every hub
        // tool is still admitted, so this must not block dispatch.
        let cluttered = vec![
            "violet_read".to_string(),
            "violet_post".to_string(),
            "slack_upload_file".to_string(),
        ];
        let stale = tool_drift(&cluttered, &live);
        assert!(stale.unallowlisted.is_empty());
        assert_eq!(stale.retired, vec!["slack_upload_file"]);
        assert!(!stale.blocks_dispatch());

        assert!(
            tool_drift(
                &["violet_read".to_string(), "violet_post".to_string()],
                &live,
            )
            .is_empty(),
            "order must not be treated as drift"
        );
    }

    #[test]
    fn doctor_is_green_only_when_registered_exported_and_verified() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();

        // Unregistered machine: a warning that names the one command.
        let row = doctor_row(None, &paths, &env, &FakeProbe(live_tools()));
        assert_eq!(row.severity, DoctorSeverity::Warning);
        assert!(row.message.contains("cas integrate violet"), "{row:?}");

        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();

        let row = doctor_row(None, &paths, &env, &FakeProbe(live_tools()));
        assert_eq!(row.severity, DoctorSeverity::Ok, "{row:?}");
        assert!(row.message.contains("violet_read"), "{row:?}");
        // GH #1157: the row states `cas violet` readiness, not just registration.
        assert!(
            row.message.contains("reachable and bearer accepted"),
            "{row:?}"
        );
        assert!(
            row.message.contains("`cas violet post|thread|read` ready"),
            "{row:?}"
        );
        assert!(!row.message.contains(FAKE_TOKEN));
    }

    #[test]
    fn doctor_is_red_with_the_exact_remedy_when_a_variable_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        run(&args, None, &paths, &ready_env(), &FakeProbe(live_tools())).unwrap();

        let row = doctor_row(
            None,
            &paths,
            &FakeEnv::with(&[(VIOLET_DEFAULT_BYPASS_ENV, FAKE_BYPASS)]),
            &FakeProbe(live_tools()),
        );
        assert_eq!(row.severity, DoctorSeverity::Error);
        assert!(row.message.contains(TEST_TOKEN_ENV), "{row:?}");
        assert!(row.message.contains("unset"), "{row:?}");
        assert!(row.message.contains("credentials file"), "{row:?}");
    }

    #[test]
    fn doctor_names_missing_proxy_credential_variable() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();

        let row = doctor_row(
            None,
            &paths,
            &env,
            &FakeProbe(ProbeOutcome::Unreachable {
                code: format!("missing_credential_env:{TEST_TOKEN_ENV}"),
            }),
        );
        assert_eq!(row.severity, DoctorSeverity::Warning);
        assert!(row.message.contains(TEST_TOKEN_ENV), "{row:?}");
        assert!(row.message.contains("unset"), "{row:?}");
        assert!(!row.message.contains("connection_failed"), "{row:?}");
    }

    #[test]
    fn doctor_is_red_when_the_hub_tool_contract_drifts() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();

        let row = doctor_row(
            None,
            &paths,
            &env,
            &FakeProbe(ProbeOutcome::Tools {
                tools: vec!["violet_read".to_string(), "violet_broadcast".to_string()],
                schema_problems: Vec::new(),
            }),
        );
        assert_eq!(row.severity, DoctorSeverity::Error);
        assert!(row.message.contains("violet_broadcast"), "{row:?}");
        assert!(row.message.contains("cas integrate violet"), "{row:?}");
    }

    /// The `violet_post` input schema both Claude Code and Codex received from
    /// the hub on 2026-10-09, after violet_ps#26 (cas-96c0).
    fn served_violet_post_schema() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/violet/violet_post_input_schema_20261009.json"
        ))
        .unwrap()
    }

    /// The shape the hub served before violet_ps#26: one top-level `anyOf`
    /// branch per kind and no root `properties`. Claude Code dropped the
    /// tool, and Codex agents read it as "kind is always message" (GH #1051).
    fn pre_fix_any_of_violet_post_schema() -> serde_json::Value {
        let branch = |kind: &str, required: &[&str]| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "channel": {"type": "string"},
                    "kind": {"const": kind},
                    "message_id": {"type": "string"},
                    "text": {"type": "string"},
                },
                "required": required,
            })
        };
        serde_json::json!({
            "type": "object",
            "anyOf": [
                branch("message", &["channel", "kind", "text"]),
                branch("file", &["channel", "kind"]),
                branch("reaction", &["channel", "kind", "message_id"]),
                branch("edit", &["channel", "kind", "message_id", "text"]),
                branch("delete", &["channel", "kind", "message_id"]),
            ],
        })
    }

    #[test]
    fn served_violet_post_schema_is_object_root_with_edit_and_delete_kinds() {
        let schema = served_violet_post_schema();
        assert_eq!(violet_post_schema_problems(&schema), Vec::<String>::new());
        // Pin the corrective guidance agents read when they try to edit with
        // kind=message (GH #1051).
        let message_id = schema["properties"]["message_id"]["description"]
            .as_str()
            .unwrap();
        assert!(message_id.contains("kind=edit"), "{message_id}");
        let kind = schema["properties"]["kind"]["description"]
            .as_str()
            .unwrap();
        for shape in ["edit: message_id, text", "delete: message_id"] {
            assert!(kind.contains(shape), "{kind}");
        }
    }

    #[test]
    fn pre_fix_any_of_violet_post_schema_is_reported() {
        let problems = violet_post_schema_problems(&pre_fix_any_of_violet_post_schema());
        assert!(
            problems.iter().any(|p| p.contains("top-level anyOf")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("kind is not an enum")),
            "{problems:?}"
        );
    }

    #[test]
    fn violet_post_schema_without_edit_and_delete_kinds_is_reported() {
        let mut schema = served_violet_post_schema();
        schema["properties"]["kind"]["enum"] = serde_json::json!(["message", "file", "reaction"]);
        assert_eq!(
            violet_post_schema_problems(&schema),
            vec!["kind enum lacks edit, delete".to_string()]
        );
    }

    #[test]
    fn doctor_and_receipt_are_red_when_the_hub_serves_an_unusable_violet_post_schema() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        let broken = ProbeOutcome::Tools {
            tools: VIOLET_TOOLS.iter().map(|t| t.to_string()).collect(),
            schema_problems: violet_post_schema_problems(&pre_fix_any_of_violet_post_schema()),
        };

        let report = run(&args, None, &paths, &env, &FakeProbe(broken.clone())).unwrap();
        assert!(!report.is_green(), "{report:?}");
        let remedy = report.remedy.as_deref().unwrap();
        assert!(remedy.contains("violet_ps"), "{remedy}");
        assert!(remedy.contains("top-level anyOf"), "{remedy}");

        let row = doctor_row(None, &paths, &env, &FakeProbe(broken));
        assert_eq!(row.severity, DoctorSeverity::Error, "{row:?}");
        assert!(row.message.contains("violet_post"), "{row:?}");
        assert!(row.message.contains("top-level anyOf"), "{row:?}");
    }

    /// A machine whose project file still lists the retired `slack_*` names
    /// alongside the live ones can post today: every hub tool is admitted. It
    /// must read amber, not red — otherwise `cas doctor` reports an outage
    /// where there is only clutter.
    #[test]
    fn doctor_is_amber_not_red_for_stale_entries_that_still_admit_every_hub_tool() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();

        let project = dir.path().join("proxy.toml");
        std::fs::write(
            &project,
            "allowlist = [\"violet.violet_read\", \"violet.violet_post\", \
             \"violet.slack_upload_file\"]\n",
        )
        .unwrap();

        let row = doctor_row(Some(&project), &paths, &env, &FakeProbe(live_tools()));
        assert_eq!(row.severity, DoctorSeverity::Warning, "{row:?}");
        assert!(row.message.contains("slack_upload_file"), "{row:?}");
        assert!(!row.message.contains("denied by policy"), "{row:?}");
        // The stale entries are in the *project* file, and only naming it
        // makes the remedy checkable (cas-a0ab).
        assert!(
            row.message.contains(&project.display().to_string()),
            "{row:?}"
        );
        assert!(
            !row.message
                .contains(&paths.user_proxy.display().to_string()),
            "the machine file holds no stale entry and must not be blamed: {row:?}"
        );
    }

    /// The 2026-09-04 report (cas-a0ab): the machine file was already
    /// canonical, an untracked project `.cas/proxy.toml` still named the
    /// retired `slack_*` quartet, and because a project allowlist *replaces*
    /// the machine one the command's "already-configured" receipt was a lie —
    /// `cas doctor` kept warning after every re-run.
    fn shadowing_project_proxy() -> String {
        let token_env = TEST_TOKEN_ENV;
        let hub_url = violet_hub_url();
        format!(
            "# project dispatch policy — keep the neon route\n\
         allowlist = [\n\
         \x20 \"neon.run_sql\",\n\
         \x20 \"violet.violet_read\",\n\
         \x20 \"violet.violet_post\",\n\
         \x20 \"violet.slack_list_channels\",\n\
         \x20 \"violet.slack_post_message\",\n\
         \x20 \"violet.slack_read_channel\",\n\
         \x20 \"violet.slack_upload_file\",\n\
         ]\n\
         \n\
         [servers.neon]\n\
         transport = \"stdio\"\n\
         command = \"neon-mcp\"\n\
         \n\
         [servers.violet]\n\
         transport = \"http\"\n\
         url = \"{hub_url}\"\n\
         auth = \"env:{token_env}\"\n\
         \n\
         [servers.violet.headers]\n\
         x-vercel-protection-bypass = \"env:VIOLET_VERCEL_BYPASS\"\n"
        )
    }

    /// A `[servers.violet]` block byte-equal in effect to the machine
    /// registration `ensure_violet_registration` writes under the default
    /// variable names — the only shape that is a true duplicate and so the
    /// only one safe to drop.
    fn duplicate_server_block() -> String {
        let token_env = TEST_TOKEN_ENV;
        let hub_url = violet_hub_url();
        format!(
            "[servers.violet]\ntransport = \"http\"\n\
             url = \"{hub_url}\"\n\
             auth = \"env:{token_env}\"\n\
             \n\
             [servers.violet.headers]\n\
             {VIOLET_BYPASS_HEADER} = \"env:{VIOLET_DEFAULT_BYPASS_ENV}\"\n"
        )
    }

    fn write_project_proxy(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("project").join(".cas").join("proxy.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn one_run_repairs_a_project_proxy_that_shadows_a_clean_machine_registration() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        // A machine file that is already canonical — the exact state that used
        // to make the command exit "already configured" and change nothing.
        run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();
        let project = write_project_proxy(dir.path(), &shadowing_project_proxy());

        let before = doctor_row(Some(&project), &paths, &env, &FakeProbe(live_tools()));
        assert_eq!(before.severity, DoctorSeverity::Warning, "{before:?}");

        let report = run(
            &args,
            Some(&project),
            &paths,
            &env,
            &FakeProbe(live_tools()),
        )
        .unwrap();
        let entry = report.project_proxy.clone().expect("project file reported");
        assert_eq!(entry.state, WriteState::Written, "{report:?}");
        assert_eq!(entry.path, project);
        assert_eq!(report.registration, WriteState::AlreadyCurrent);
        assert!(report.is_green(), "{report:?}");
        assert_eq!(report.allowlist, VIOLET_TOOLS);

        let rewritten = std::fs::read_to_string(&project).unwrap();
        // Exact bytes: an operator-owned file must come back looking like an
        // operator wrote it — same comment, same multi-line array shape, same
        // key order — or nobody will trust the command with it twice.
        assert_eq!(
            rewritten,
            "# project dispatch policy — keep the neon route\n\
             allowlist = [\n\
             \x20 \"neon.run_sql\",\n\
             \x20 \"violet.violet_read\",\n\
             \x20 \"violet.violet_post\",\n\
             ]\n\
             \n\
             [servers.neon]\n\
             transport = \"stdio\"\n\
             command = \"neon-mcp\"\n"
        );
        let parsed = ProxyConfig::load_from(&project).unwrap();
        assert_eq!(parsed.violet_allowlisted_tools(), VIOLET_TOOLS);
        // Everything unrelated survives, comments included.
        assert!(
            parsed
                .allowlist
                .iter()
                .any(|route| route.server == "neon" && route.tool == "run_sql"),
            "{rewritten}"
        );
        assert!(parsed.servers.contains_key("neon"), "{rewritten}");
        assert!(
            rewritten.contains("# project dispatch policy"),
            "{rewritten}"
        );
        // The duplicate registration is gone: the machine file supplies it.
        assert!(!parsed.servers.contains_key(VIOLET_SERVER), "{rewritten}");

        // Doctor now agrees, which is the whole acceptance criterion.
        let after = doctor_row(Some(&project), &paths, &env, &FakeProbe(live_tools()));
        assert_eq!(after.severity, DoctorSeverity::Ok, "{after:?}");

        // Idempotent: a second run is byte-identical and rewrites nothing.
        let second = run(
            &args,
            Some(&project),
            &paths,
            &env,
            &FakeProbe(live_tools()),
        )
        .unwrap();
        assert_eq!(
            second.project_proxy.unwrap().state,
            WriteState::AlreadyCurrent
        );
        assert_eq!(std::fs::read_to_string(&project).unwrap(), rewritten);
    }

    /// A project file whose only Violet trace is the duplicate server
    /// block: dropping the block alone would leave a file that admits nothing,
    /// so the routes it evidently wanted are named explicitly.
    #[test]
    fn a_duplicate_server_block_without_routes_is_replaced_by_the_routes() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        let project = write_project_proxy(dir.path(), &duplicate_server_block());

        let report = run(
            &args,
            Some(&project),
            &paths,
            &env,
            &FakeProbe(live_tools()),
        )
        .unwrap();
        assert_eq!(
            report.project_proxy.as_ref().unwrap().state,
            WriteState::Written,
            "{report:?}"
        );
        assert!(report.is_green(), "{report:?}");

        let parsed = ProxyConfig::load_from(&project).unwrap();
        assert_eq!(parsed.violet_allowlisted_tools(), VIOLET_TOOLS);
        assert!(!parsed.servers.contains_key(VIOLET_SERVER));
        let row = doctor_row(Some(&project), &paths, &env, &FakeProbe(live_tools()));
        assert_eq!(row.severity, DoctorSeverity::Ok, "{row:?}");
    }

    /// `--url` exists so a project can point at a staging hub, and the proxy
    /// merges project server tables *over* machine ones. A block that differs
    /// from the machine registration is therefore an override, not a
    /// duplicate: dropping it would silently move the project to another
    /// endpoint. The stale routes are still corrected around it.
    #[test]
    fn a_project_server_block_that_overrides_the_machine_registration_survives() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        const STAGING: &str = "https://staging.example.test/mcp/slack";
        let project = write_project_proxy(
            dir.path(),
            &format!(
                "allowlist = [\n\
                 \x20 \"violet.violet_read\",\n\
                 \x20 \"violet.slack_post_message\",\n\
                 ]\n\
                 \n\
                 [servers.violet]\n\
                 transport = \"http\"\n\
                 url = \"{STAGING}\"\n\
                 auth = \"env:VIOLET_SLACK_TOKEN_PROJECT_OVERRIDE\"\n"
            ),
        );

        let report = run(
            &args,
            Some(&project),
            &paths,
            &env,
            &FakeProbe(live_tools()),
        )
        .unwrap();
        let entry = report.project_proxy.clone().unwrap();
        assert_eq!(entry.state, WriteState::Written, "{report:?}");
        assert!(entry.note.contains("kept [servers.violet]"), "{entry:?}");
        assert!(entry.note.contains(STAGING), "{entry:?}");

        let rendered = std::fs::read_to_string(&project).unwrap();
        let parsed = ProxyConfig::load_from(&project).unwrap();
        // The override survives, pointing where the project put it…
        let server = parsed
            .servers
            .get(VIOLET_SERVER)
            .expect("the override must survive");
        assert_eq!(server_endpoint(server), STAGING, "{rendered}");
        // …while the retired route it carried is corrected.
        assert_eq!(parsed.violet_allowlisted_tools(), VIOLET_TOOLS);

        // Keeping a block is not a change: a second run must not rewrite.
        let second = run(
            &args,
            Some(&project),
            &paths,
            &env,
            &FakeProbe(live_tools()),
        )
        .unwrap();
        let second_entry = second.project_proxy.unwrap();
        assert_eq!(second_entry.state, WriteState::AlreadyCurrent, "{rendered}");
        assert!(
            second_entry
                .note
                .contains("overrides the machine registration"),
            "the override must stay visible on every run: {second_entry:?}"
        );
        assert_eq!(std::fs::read_to_string(&project).unwrap(), rendered);
    }

    /// Adding an `allowlist` key to a file that already opens a `[servers.…]`
    /// table is the one edit that can silently produce a *different* document:
    /// a root key emitted after a table header belongs to that table. This
    /// pins the rendering, because the damage would be invisible until some
    /// unrelated server stopped connecting.
    #[test]
    fn an_added_allowlist_stays_a_root_key_ahead_of_existing_server_tables() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        let project = write_project_proxy(
            dir.path(),
            &format!(
                "[servers.neon]\ntransport = \"stdio\"\ncommand = \"neon-mcp\"\n\n{}",
                duplicate_server_block()
            ),
        );

        run(
            &args,
            Some(&project),
            &paths,
            &env,
            &FakeProbe(live_tools()),
        )
        .unwrap();

        let rendered = std::fs::read_to_string(&project).unwrap();
        let parsed = ProxyConfig::load_from(&project).unwrap();
        assert_eq!(
            parsed.violet_allowlisted_tools(),
            VIOLET_TOOLS,
            "{rendered}"
        );
        assert!(parsed.servers.contains_key("neon"), "{rendered}");
        assert_eq!(parsed.servers.len(), 1, "{rendered}");
        assert!(
            rendered.find("allowlist").unwrap() < rendered.find("[servers.neon]").unwrap(),
            "the root key must precede the first table header:\n{rendered}"
        );
    }

    /// A project that declares its own policy and never mentions the hub is
    /// not this command's to rewrite: widening its allowlist would be a
    /// silent policy change. It is reported, with the exact edit, instead.
    #[test]
    fn a_project_proxy_that_names_no_hub_route_is_left_alone_and_named_in_the_remedy() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        let original = "allowlist = [\"neon.run_sql\"]\n";
        let project = write_project_proxy(dir.path(), original);

        let report = run(
            &args,
            Some(&project),
            &paths,
            &env,
            &FakeProbe(live_tools()),
        )
        .unwrap();
        let entry = report.project_proxy.clone().unwrap();
        assert_eq!(entry.state, WriteState::Skipped, "{report:?}");
        assert_eq!(std::fs::read_to_string(&project).unwrap(), original);
        assert!(!report.is_green(), "{report:?}");
        let remedy = report.remedy.clone().unwrap();
        assert!(remedy.contains(&project.display().to_string()), "{remedy}");
        assert!(remedy.contains("violet.violet_read"), "{remedy}");
        assert!(
            !remedy.contains("Re-run `cas integrate violet`"),
            "re-running cannot fix this, so it must not be offered: {remedy}"
        );
    }

    #[test]
    fn dry_run_does_not_touch_a_shadowing_project_proxy() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            dry_run: true,
            ..test_args()
        };
        let project = write_project_proxy(dir.path(), &shadowing_project_proxy());

        let report = run(
            &args,
            Some(&project),
            &paths,
            &ready_env(),
            &FakeProbe(live_tools()),
        )
        .unwrap();
        assert_eq!(report.project_proxy.unwrap().state, WriteState::Planned);
        assert_eq!(
            std::fs::read_to_string(&project).unwrap(),
            shadowing_project_proxy()
        );
    }

    /// An `allowlist` that is not an array cannot be edited safely, and
    /// guessing would destroy operator configuration. The machine file is
    /// still written, and the error names the one path to repair.
    #[test]
    fn a_malformed_project_allowlist_is_refused_by_name_after_the_machine_file_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        let project = write_project_proxy(
            dir.path(),
            "allowlist = \"violet.violet_read\"\n[servers.violet]\n\
             transport = \"http\"\nurl = \"https://x\"\n",
        );

        let error = run(
            &args,
            Some(&project),
            &paths,
            &ready_env(),
            &FakeProbe(live_tools()),
        )
        .unwrap_err();
        let rendered = format!("{error:#}");
        assert!(
            rendered.contains(&project.display().to_string()),
            "{rendered}"
        );
        assert!(paths.user_proxy.is_file(), "machine file must still land");
    }

    #[test]
    fn doctor_reports_a_project_proxy_file_that_shadows_the_machine_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();

        let project = dir.path().join("proxy.toml");
        std::fs::write(&project, "allowlist = [\"neon.run_sql\"]\n").unwrap();

        let row = doctor_row(Some(&project), &paths, &env, &FakeProbe(live_tools()));
        assert_eq!(row.severity, DoctorSeverity::Error);
        assert!(row.message.contains("authoritative"), "{row:?}");
        assert!(row.message.contains("violet.violet_read"), "{row:?}");
        assert!(
            row.message.contains(&project.display().to_string()),
            "{row:?}"
        );
    }

    #[test]
    fn unauthorized_hub_never_reaches_the_ok_row() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = VioletArgs {
            bypass_env: VIOLET_DEFAULT_BYPASS_ENV.to_string(),
            url: violet_hub_url().to_string(),
            no_harness: true,
            ..test_args()
        };
        run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();

        let row = doctor_row(None, &paths, &env, &FakeProbe(ProbeOutcome::Unauthorized));
        assert_eq!(row.severity, DoctorSeverity::Error);
        assert!(row.message.contains("401"), "{row:?}");
        assert!(row.message.contains("invalid_token"), "{row:?}");
        assert!(row.message.contains("the bearer is bad"), "{row:?}");
        assert!(row.message.contains("`cas violet` cannot post"), "{row:?}");
        assert!(!row.message.contains(FAKE_TOKEN));
    }

    // -----------------------------------------------------------------------
    // GH #1164: same-URL project override loop; claude-code entry verification
    // -----------------------------------------------------------------------

    fn same_url_override_project(dir: &Path) -> PathBuf {
        write_project_proxy(
            dir,
            &format!(
                "allowlist = [\n\
                 \x20 \"violet.violet_read\",\n\
                 \x20 \"violet.violet_post\",\n\
                 ]\n\
                 \n\
                 [servers.violet]\n\
                 transport = \"http\"\n\
                 url = \"{}\"\n\
                 auth = \"env:VIOLET_SLACK_TOKEN_CASSY_PROXY\"\n",
                violet_hub_url()
            ),
        )
    }

    fn no_harness_args() -> VioletArgs {
        VioletArgs {
            no_harness: true,
            ..test_args()
        }
    }

    /// GH #1164 part 1: a committed project block naming another machine's
    /// token for the *same* hub URL left integrate stale forever, advising
    /// `cas login`, which never mints that token. It is dropped, and the
    /// machine registration's own token verifies the hub.
    #[test]
    fn a_same_url_project_override_naming_another_token_is_dropped_gh_1164() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = no_harness_args();
        run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();
        let project = same_url_override_project(dir.path());

        let probe = RecordingProbe::default();
        let report = run(&args, Some(&project), &paths, &env, &probe).unwrap();

        let entry = report.project_proxy.clone().expect("project file reported");
        assert_eq!(entry.state, WriteState::Written, "{report:?}");
        assert!(entry.note.contains("dropped [servers.violet]"), "{entry:?}");
        assert!(entry.note.contains("same hub URL"), "{entry:?}");
        let parsed = ProxyConfig::load_from(&project).unwrap();
        assert!(!parsed.servers.contains_key(VIOLET_SERVER), "{parsed:?}");

        let probed = probe.0.borrow();
        assert_eq!(probed.len(), 1, "the machine registration must be verified");
        match &probed[0] {
            ServerConfig::Http { auth, .. } => {
                assert_eq!(
                    auth.as_deref(),
                    Some(format!("env:{TEST_TOKEN_ENV}").as_str())
                )
            }
            other => panic!("unexpected probe server {other:?}"),
        }
        assert!(report.is_green(), "{report:?}");
        assert_eq!(report.remedy, None, "{report:?}");
    }

    /// A distinct-URL override is a deliberate staging target and is kept. When
    /// its token is missing, the advice names the override, not `cas login`
    /// (which can never mint a token a project file invented).
    #[test]
    fn a_missing_override_token_names_the_override_not_cas_login_gh_1164() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = no_harness_args();
        const STAGING: &str = "https://staging.example.test/mcp/slack";
        let project = write_project_proxy(
            dir.path(),
            &format!(
                "allowlist = [\n  \"violet.violet_read\",\n  \"violet.violet_post\",\n]\n\n\
                 [servers.violet]\ntransport = \"http\"\nurl = \"{STAGING}\"\n\
                 auth = \"env:STAGING_ONLY_TOKEN\"\n"
            ),
        );
        let report = run(
            &args,
            Some(&project),
            &paths,
            &env,
            &FakeProbe(live_tools()),
        )
        .unwrap();
        let remedy = report
            .remedy
            .clone()
            .expect("a missing token needs a remedy");
        assert!(remedy.contains("STAGING_ONLY_TOKEN"), "{remedy}");
        assert!(remedy.contains(&project.display().to_string()), "{remedy}");
        assert!(!remedy.contains("cas login"), "{remedy}");
    }

    /// A rejected hub client bearer is not repaired by signing in again: the
    /// remedy names the variable, the credentials file and the re-mint path.
    #[test]
    fn a_rejected_bearer_remedy_names_the_variable_and_re_mint_not_cas_login_gh_1164() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let report = run(
            &no_harness_args(),
            None,
            &paths,
            &env,
            &FakeProbe(ProbeOutcome::Unauthorized),
        )
        .unwrap();
        let remedy = report.remedy.clone().expect("401 needs a remedy");
        assert!(remedy.contains(TEST_TOKEN_ENV), "{remedy}");
        assert!(
            remedy.contains(&paths.credentials_file.display().to_string()),
            "{remedy}"
        );
        assert!(remedy.contains("re-run `cas integrate violet`"), "{remedy}");
        assert!(!remedy.contains("cas login"), "{remedy}");
        assert!(
            !CREDENTIALS_HINT.contains("cas login"),
            "{CREDENTIALS_HINT}"
        );
    }

    /// GH #1164 part 2: "already current" is a structural claim. Each
    /// claude-code entry now carries the authenticated tools/list verdict, so
    /// a rejected bearer is never reported as merely current.
    #[test]
    fn claude_code_entries_carry_the_authenticated_tools_list_verdict_gh_1164() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = test_args();
        run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();

        let verified = run(&args, None, &paths, &env, &FakeProbe(live_tools())).unwrap();
        let claude = verified
            .harnesses
            .iter()
            .find(|h| h.harness == "claude-code")
            .unwrap();
        assert_eq!(claude.state, WriteState::AlreadyCurrent);
        let note = claude.note.clone().unwrap_or_default();
        assert!(
            note.contains("authenticated tools/list: 2 tool(s)"),
            "{claude:?}"
        );

        let rejected = run(
            &args,
            None,
            &paths,
            &env,
            &FakeProbe(ProbeOutcome::Unauthorized),
        )
        .unwrap();
        let claude = rejected
            .harnesses
            .iter()
            .find(|h| h.harness == "claude-code")
            .unwrap();
        let note = claude.note.clone().unwrap_or_default();
        assert!(note.contains("rejected"), "{claude:?}");
        assert!(note.contains("401"), "{claude:?}");
        assert!(note.contains(TEST_TOKEN_ENV), "{claude:?}");
    }

    /// Other Claude account profiles on the machine that already register the
    /// hub are reconciled too: a literal bearer or MECHA_* references (which
    /// now expand to empty) are rewritten with env references. A profile that
    /// never registered the hub is left byte-for-byte alone.
    #[test]
    fn sibling_claude_profiles_with_stale_violet_entries_are_rewritten_gh_1164() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let mecha = home.join(".claude-alt").join(".claude.json");
        let literal = home.join(".claude-work").join(".claude.json");
        let untouched = home.join(".claude-empty").join(".claude.json");
        for (path, body) in [
            (
                &mecha,
                format!(
                    r#"{{"numStartups": 3, "mcpServers": {{"violet": {{"type": "http", "url": "{}", "headers": {{"Authorization": "Bearer ${{MECHA_SLACK_TOKEN_CASSY_PROXY}}", "{VIOLET_BYPASS_HEADER}": "${{MECHA_VERCEL_BYPASS}}"}}}}}}}}"#,
                    violet_hub_url()
                ),
            ),
            (
                &literal,
                format!(
                    r#"{{"mcpServers": {{"violet": {{"type": "http", "url": "{}", "headers": {{"Authorization": "Bearer xoxb-revoked-literal", "{VIOLET_BYPASS_HEADER}": "${{MECHA_VERCEL_BYPASS}}"}}}}}}}}"#,
                    violet_hub_url()
                ),
            ),
            (
                &untouched,
                r#"{"mcpServers": {"other": {"type": "http", "url": "https://x"}}}"#.to_string(),
            ),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        let untouched_before = std::fs::read_to_string(&untouched).unwrap();

        let mut paths = paths_in(dir.path());
        paths.claude_profiles = discover_claude_profiles(&home, paths.claude_json.as_deref());
        assert_eq!(
            paths.claude_profiles,
            vec![mecha.clone(), untouched.clone(), literal.clone()]
        );
        let env = ready_env();
        let report = run(&test_args(), None, &paths, &env, &FakeProbe(live_tools())).unwrap();

        for (path, reason) in [(&mecha, "MECHA_"), (&literal, "literal bearer")] {
            let entry = report
                .harnesses
                .iter()
                .find(|h| h.path.as_deref() == Some(path.as_path()))
                .unwrap_or_else(|| {
                    panic!("{} not reported: {:?}", path.display(), report.harnesses)
                });
            assert_eq!(entry.state, WriteState::Written, "{entry:?}");
            assert!(
                entry.note.as_deref().unwrap_or_default().contains(reason),
                "{entry:?}"
            );
            let written: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            let headers = &written["mcpServers"][VIOLET_SERVER]["headers"];
            assert_eq!(
                headers["Authorization"],
                format!("Bearer ${{{TEST_TOKEN_ENV}}}")
            );
            assert_eq!(
                headers[VIOLET_BYPASS_HEADER],
                format!("${{{VIOLET_DEFAULT_BYPASS_ENV}}}")
            );
            assert!(
                !std::fs::read_to_string(path)
                    .unwrap()
                    .contains("xoxb-revoked-literal")
            );
        }
        // Unrelated keys survive the rewrite.
        let mecha_doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&mecha).unwrap()).unwrap();
        assert_eq!(mecha_doc["numStartups"], 3);
        assert!(
            !report
                .harnesses
                .iter()
                .any(|h| h.path.as_deref() == Some(untouched.as_path())),
            "{:?}",
            report.harnesses
        );
        assert_eq!(
            std::fs::read_to_string(&untouched).unwrap(),
            untouched_before
        );
    }

    // -----------------------------------------------------------------------
    // cas-a897: `cas integrate violet --channel` maps a Slack channel to this
    // project through the hub's channel→project API
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct FakeChannelHub {
        maps: RefCell<Vec<(String, String, String, String, bool)>>,
        unmaps: RefCell<Vec<(String, String, String)>>,
        map_result: RefCell<Option<std::result::Result<ChannelMapping, HubClientError>>>,
    }

    impl HubClient for FakeChannelHub {
        fn create_client(
            &self,
            _hub_url: &str,
            _cloud_token: &str,
            _label: &str,
        ) -> std::result::Result<(String, Option<String>), HubClientError> {
            unreachable!("channel mapping never mints a client")
        }

        fn fetch_bypass(
            &self,
            _hub_url: &str,
            _cloud_token: &str,
        ) -> std::result::Result<String, HubClientError> {
            unreachable!("channel mapping never reads the bypass")
        }

        fn map_channel(
            &self,
            hub_url: &str,
            cloud_token: &str,
            channel: &str,
            project_id: &str,
            replace: bool,
        ) -> std::result::Result<ChannelMapping, HubClientError> {
            self.maps.borrow_mut().push((
                hub_url.into(),
                cloud_token.into(),
                channel.into(),
                project_id.into(),
                replace,
            ));
            self.map_result.borrow_mut().take().unwrap_or_else(|| {
                Ok(ChannelMapping {
                    channel_id: "C09FCTHCQ2U".into(),
                    channel_name: "violet-internal".into(),
                    project_id: project_id.into(),
                })
            })
        }

        fn unmap_channel(
            &self,
            hub_url: &str,
            cloud_token: &str,
            channel_id: &str,
        ) -> std::result::Result<(), HubClientError> {
            self.unmaps
                .borrow_mut()
                .push((hub_url.into(), cloud_token.into(), channel_id.into()));
            Ok(())
        }
    }

    fn channel_request(channel: &str) -> ChannelRequest {
        ChannelRequest {
            channel: Some(channel.to_string()),
            replace: false,
            remove: None,
        }
    }

    #[test]
    fn channel_flag_maps_the_channel_to_this_project_with_the_cloud_login_cas_a897() {
        let hub = FakeChannelHub::default();
        let line = register_channel_mapping(
            &channel_request("#violet-internal"),
            violet_hub_url(),
            Some("cloud-session-token"),
            Some("github.com/richards-llc/violet_ps"),
            &hub,
        )
        .unwrap()
        .expect("a mapping was requested");
        let maps = hub.maps.borrow();
        assert_eq!(maps.len(), 1);
        let (url, token, channel, project, replace) = &maps[0];
        assert_eq!(url, violet_hub_url());
        assert_eq!(token, "cloud-session-token");
        assert_eq!(channel, "violet-internal", "a leading # is stripped");
        assert_eq!(project, "github.com/richards-llc/violet_ps");
        assert!(!replace);
        assert!(line.contains("#violet-internal (C09FCTHCQ2U)"), "{line}");
        assert!(line.contains("github.com/richards-llc/violet_ps"), "{line}");
        assert!(!line.contains("cloud-session-token"), "{line}");
    }

    #[test]
    fn channel_mapping_failures_name_the_fix_cas_a897() {
        let cases = [
            (HubClientError::NotAMember, "invite @Violet to #violet-internal first"),
            (HubClientError::ChannelMapped, "--channel-replace"),
            (HubClientError::Unauthorized, "Cassy Cloud login"),
        ];
        for (error, expected) in cases {
            let hub = FakeChannelHub::default();
            *hub.map_result.borrow_mut() = Some(Err(error.clone()));
            let message = register_channel_mapping(
                &channel_request("violet-internal"),
                violet_hub_url(),
                Some("cloud-session-token"),
                Some("project"),
                &hub,
            )
            .unwrap_err()
            .to_string();
            assert!(message.contains(expected), "{error:?}: {message}");
        }

        // Replace is only sent when asked for.
        let hub = FakeChannelHub::default();
        let request = ChannelRequest {
            replace: true,
            ..channel_request("violet-internal")
        };
        register_channel_mapping(&request, violet_hub_url(), Some("t"), Some("p"), &hub).unwrap();
        assert!(hub.maps.borrow()[0].4);

        // No Cloud login, no project identity, or a malformed channel never
        // reach the hub.
        let hub = FakeChannelHub::default();
        let no_login = register_channel_mapping(&channel_request("x"), violet_hub_url(), None, Some("p"), &hub);
        assert!(no_login.unwrap_err().to_string().contains("Cassy Cloud login"));
        let no_project = register_channel_mapping(&channel_request("x"), violet_hub_url(), Some("t"), None, &hub);
        assert!(no_project.unwrap_err().to_string().contains("canonical"));
        let malformed =
            register_channel_mapping(&channel_request("a/b?c"), violet_hub_url(), Some("t"), Some("p"), &hub);
        assert!(malformed.is_err());
        assert!(hub.maps.borrow().is_empty());
    }

    #[test]
    fn channel_remove_unmaps_by_channel_id_only_cas_a897() {
        let hub = FakeChannelHub::default();
        let request = ChannelRequest {
            channel: None,
            replace: false,
            remove: Some("C09FCTHCQ2U".into()),
        };
        let line = register_channel_mapping(&request, violet_hub_url(), Some("t"), Some("p"), &hub)
            .unwrap()
            .unwrap();
        assert_eq!(hub.unmaps.borrow()[0].2, "C09FCTHCQ2U");
        assert!(line.contains("unmapped C09FCTHCQ2U"), "{line}");

        let by_name = ChannelRequest {
            remove: Some("violet-internal".into()),
            ..request
        };
        let error = register_channel_mapping(&by_name, violet_hub_url(), Some("t"), Some("p"), &hub)
            .unwrap_err()
            .to_string();
        assert!(error.contains("channel id"), "{error}");
        assert_eq!(hub.unmaps.borrow().len(), 1);

        // Nothing requested: nothing happens.
        let none = ChannelRequest {
            channel: None,
            replace: false,
            remove: None,
        };
        assert!(
            register_channel_mapping(&none, violet_hub_url(), Some("t"), Some("p"), &hub)
                .unwrap()
                .is_none()
        );
    }
}
