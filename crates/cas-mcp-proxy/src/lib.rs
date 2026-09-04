pub mod config;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use cas_types::{public_tool_id, public_tool_ids, public_upstream_id, public_upstream_ids};
use rmcp::model::Tool;
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Command;
use tokio::sync::RwLock;

use config::ServerConfig;

/// Result from executing MCP tool calls.
pub struct ExecuteResult {
    /// Text output from the execution.
    pub text: String,
    /// Images returned by the execution.
    pub images: Vec<ImageResult>,
}

/// An image returned from MCP tool execution.
pub struct ImageResult {
    /// Base64-encoded image data.
    pub data: String,
    /// MIME type (e.g., "image/png").
    pub mime_type: String,
}

/// A catalog entry describing a tool from an upstream MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

type McpClientService = RunningService<rmcp::RoleClient, ()>;

/// Registered CAS actor that initiated a proxied upstream call.
///
/// The CAS MCP service constructs this from its registered agent store; it is
/// deliberately separate from the JSON dispatch arguments so an upstream
/// caller cannot nominate its own role, session, or task attribution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProxyCaller {
    /// Stable registered CAS agent identifier.
    pub agent_id: String,
    /// CAS role resolved from the registered agent row.
    pub role: cas_types::AgentRole,
    /// The caller's CAS session identifier.
    pub session_id: String,
    /// Owning factory session when the caller belongs to a factory.
    pub factory_session: Option<String>,
    /// Active task leases held by this caller at dispatch time.
    pub active_task_ids: Vec<String>,
}

/// A successful direct upstream call, exposed to CAS-owned durable side effects.
///
/// The observer runs only after the provider accepted the call. Its return
/// value is intentionally ignored by the transport: turning a local recording
/// failure into an MCP error would invite callers to retry a run-starting tool
/// and spend twice. Implementations must emit their own durable diagnostics.
pub struct ProxyCallEvent<'a> {
    pub caller: &'a ProxyCaller,
    pub server: &'a str,
    pub tool: &'a str,
    pub arguments: &'a Option<serde_json::Map<String, Value>>,
    pub result: &'a Value,
}

pub trait ProxyCallObserver: Send + Sync {
    fn call_succeeded(&self, event: ProxyCallEvent<'_>);
}

/// A policy decision requested before an upstream MCP tool call is forwarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyPolicyDecision {
    Allow,
    /// `reason` must be safe to expose in an operator audit entry and MCP
    /// error. Policies must not copy request arguments or upstream content.
    Deny { reason: String },
}

/// Input presented to a proxy policy.
///
/// Arguments are available to allow provider-specific resource adapters to
/// derive a canonical resource key. They are intentionally excluded from the
/// audit record below so request content is never retained by this seam.
pub struct ProxyPolicyRequest<'a> {
    pub caller: &'a ProxyCaller,
    pub server: &'a str,
    pub tool: &'a str,
    pub arguments: &'a Option<serde_json::Map<String, Value>>,
    pub dispatch_kind: ProxyDispatchKind,
}

/// Server-selected dispatch path. This value is never decoded from tool
/// arguments, so a direct caller cannot nominate the receipted gateway path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyDispatchKind {
    Direct,
    ExternalProductionVerification,
}

/// A single external MCP tool, identified by its parsed server and tool names.
///
/// The `mcp__<server>__<tool>` spelling is only an MCP client-facing encoding.
/// Routing and authorization retain the decoded components so a lookalike server
/// or tool cannot match an allowlist entry by sharing a textual prefix.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExternalToolRoute {
    server: String,
    tool: String,
}

impl ExternalToolRoute {
    /// Construct a route from the components provided to the proxy dispatch
    /// boundary.
    pub fn new(server: impl Into<String>, tool: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            tool: tool.into(),
        }
    }

    /// Parse the client-facing MCP spelling into an exact route.
    ///
    /// This accepts one server separator and preserves any later `__` in the
    /// tool name, which is valid for upstream tool names.
    pub fn parse_mcp_tool_name(name: &str) -> Option<Self> {
        let encoded = name.strip_prefix("mcp__")?;
        let (server, tool) = encoded.split_once("__")?;
        if server.is_empty() || tool.is_empty() {
            return None;
        }
        Some(Self::new(server, tool))
    }

    /// Parse a proxy configuration allowlist entry into its canonical route.
    /// The canonical spelling is `server.tool`; `server:tool`, `server/tool`,
    /// MCP's `mcp__server__tool`, and a bare tool name remain accepted as
    /// compatibility aliases.
    pub fn parse_allowlist_entry(entry: &str) -> Result<Self> {
        let route =
            config::ExternalToolConfig::parse_allowlist_entry(entry).map_err(anyhow::Error::msg)?;
        Ok(Self::new(route.server, route.tool))
    }

    /// Canonical configuration spelling for this route.
    pub fn canonical_entry(&self) -> String {
        format!("{}.{}", self.server, self.tool)
    }

    /// Upstream server component.
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Upstream tool component.
    pub fn tool(&self) -> &str {
        &self.tool
    }
}

/// Authorization seam for upstream MCP calls.
///
/// Policies must return a decision synchronously and must not perform an
/// upstream request themselves. A future resource-lease or delegation policy
/// can use this hook to deny a call before `ProxyEngine` forwards it.
pub trait ProxyPolicy: Send + Sync {
    fn decide(&self, request: &ProxyPolicyRequest<'_>) -> ProxyPolicyDecision;

    /// Return the policy's catalog visibility decision without inventing a
    /// caller identity. Policies that do not filter discovery retain the
    /// default, while the configured external allowlist marks denied tools.
    fn catalog_decision(&self, _server: &str, _tool: &str) -> ProxyPolicyDecision {
        ProxyPolicyDecision::Allow
    }
}

/// Default policy for installations that have not opted into protected tools.
#[derive(Debug, Default)]
pub struct AllowAllProxyPolicy;

impl ProxyPolicy for AllowAllProxyPolicy {
    fn decide(&self, _request: &ProxyPolicyRequest<'_>) -> ProxyPolicyDecision {
        ProxyPolicyDecision::Allow
    }
}

/// Fail-closed allowlist for external MCP routes.
///
/// This policy is deliberately applied to the parsed `server` and `tool`
/// fields in [`ProxyPolicyRequest`]. It does not inspect an MCP tool-name
/// string, so `mcp__viktor_shadow__ask_viktor` cannot inherit permission for
/// `mcp__viktor__ask_viktor` through prefix or substring matching.
#[derive(Debug, Default)]
pub struct ExternalToolAllowlistPolicy {
    allowed_routes: BTreeSet<ExternalToolRoute>,
    supervisor_delegation_routes: BTreeSet<ExternalToolRoute>,
}

impl ExternalToolAllowlistPolicy {
    /// Create a policy allowing exactly the supplied external routes.
    pub fn new(routes: impl IntoIterator<Item = ExternalToolRoute>) -> Self {
        Self {
            allowed_routes: routes.into_iter().collect(),
            supervisor_delegation_routes: BTreeSet::new(),
        }
    }

    /// Require selected allowlisted routes to enter through the registered
    /// supervisor delegation gateway rather than generic proxy execution.
    pub fn with_supervisor_delegation_routes(
        mut self,
        routes: impl IntoIterator<Item = ExternalToolRoute>,
    ) -> Self {
        self.supervisor_delegation_routes = routes.into_iter().collect();
        self
    }

    /// Whether an upstream route is in this exact allowlist.
    pub fn allows(&self, server: &str, tool: &str) -> bool {
        self.allowed_routes
            .contains(&ExternalToolRoute::new(server, tool))
            || self
                .allowed_routes
                .contains(&ExternalToolRoute::new(server, "*"))
            || self
                .allowed_routes
                .contains(&ExternalToolRoute::new("*", tool))
    }

    fn denial_reason(server: &str, tool: &str) -> String {
        format!(
            "external tool is not explicitly allowlisted; add \"{}.{}\" to [proxy].allowlist",
            public_upstream_id(server),
            public_tool_id(tool)
        )
    }
}

impl ProxyPolicy for ExternalToolAllowlistPolicy {
    fn decide(&self, request: &ProxyPolicyRequest<'_>) -> ProxyPolicyDecision {
        if !self.allows(request.server, request.tool) {
            return ProxyPolicyDecision::Deny {
                reason: Self::denial_reason(request.server, request.tool),
            };
        }
        let route = ExternalToolRoute::new(request.server, request.tool);
        if self.supervisor_delegation_routes.contains(&route)
            && (request.dispatch_kind != ProxyDispatchKind::ExternalProductionVerification
                || request.caller.role != cas_types::AgentRole::Supervisor)
        {
            return ProxyPolicyDecision::Deny {
                reason: "external delegation route requires the registered supervisor gateway"
                    .to_string(),
            };
        }
        ProxyPolicyDecision::Allow
    }

    fn catalog_decision(&self, server: &str, tool: &str) -> ProxyPolicyDecision {
        if self.allows(server, tool) {
            ProxyPolicyDecision::Allow
        } else {
            ProxyPolicyDecision::Deny {
                reason: Self::denial_reason(server, tool),
            }
        }
    }
}

/// Request-free record of an authorization decision made by the proxy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProxyPolicyAuditEntry {
    pub timestamp_ms: u64,
    pub caller: ProxyCaller,
    pub server: String,
    pub tool: String,
    pub allowed: bool,
    pub reason: Option<String>,
}

const POLICY_AUDIT_LIMIT: usize = 256;

/// A connected upstream MCP server with its tool catalog.
struct ConnectedServer {
    service: McpClientService,
    tools: Vec<Tool>,
    generation: u64,
    last_successful_call: AtomicU64,
}

const RETRY_BASE_SECS: u64 = 5;
const RETRY_MAX_SECS: u64 = 300;
const CONNECT_TIMEOUT_SECS: u64 = 15;
static SESSION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static CONNECTION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static CALL_COMPLETION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Machine-readable connection state for one optional upstream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpstreamHealth {
    pub name: String,
    pub transport: String,
    pub state: UpstreamState,
    /// The configured stdio executable, only populated when it is missing.
    /// HTTP/SSE endpoints and working commands remain absent from health.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    pub attempts: u32,
    pub consecutive_failures: u32,
    pub tool_count: usize,
    pub last_error_code: Option<String>,
    pub last_attempt_at_ms: Option<u64>,
    pub next_retry_at_ms: Option<u64>,
}

/// Coarse state intentionally excludes URLs, credentials, and response content.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamState {
    Healthy,
    Degraded,
    Backoff,
    ExecutableMissing,
}

/// Per-engine (therefore per MCP session) health snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProxyHealthSnapshot {
    pub session_id: String,
    pub generated_at_ms: u64,
    pub healthy: usize,
    pub degraded: usize,
    pub servers: Vec<UpstreamHealth>,
}

impl ProxyHealthSnapshot {
    /// Normalize every untrusted free-form health field before it crosses a
    /// JSON, cache, log, or preflight boundary.
    pub fn sanitized(mut self) -> Self {
        self.session_id = safe_session_id(&self.session_id);
        let public_names =
            public_upstream_ids(self.servers.iter().map(|server| server.name.as_str()));
        for server in &mut self.servers {
            server.name = public_names
                .get(&server.name)
                .cloned()
                .unwrap_or_else(|| public_upstream_id(&server.name));
            let expose_executable =
                server.state == UpstreamState::ExecutableMissing && server.transport == "stdio";
            server.transport = safe_transport(&server.transport).to_string();
            server.executable = expose_executable
                .then(|| server.executable.take())
                .flatten()
                .as_deref()
                .map(safe_executable);
            server.last_error_code = server
                .last_error_code
                .as_deref()
                .map(safe_error_code);
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureVisibility {
    Error,
    Debug,
}

/// Engine that proxies tool calls to upstream MCP servers.
pub struct ProxyEngine {
    servers: RwLock<HashMap<String, ConnectedServer>>,
    configs: RwLock<HashMap<String, ServerConfig>>,
    health: RwLock<HashMap<String, UpstreamHealth>>,
    session_id: String,
    policy: RwLock<Arc<dyn ProxyPolicy>>,
    observer: RwLock<Option<Arc<dyn ProxyCallObserver>>>,
    policy_audit: Mutex<VecDeque<ProxyPolicyAuditEntry>>,
}

fn validate_configs(configs: &HashMap<String, ServerConfig>) -> Result<()> {
    if configs.values().any(
        |config| matches!(config, ServerConfig::Stdio { command, .. } if command.trim().is_empty()),
    ) {
        anyhow::bail!("stdio proxy command must not be empty");
    }
    Ok(())
}

impl ProxyEngine {
    /// Create a proxy engine by connecting to all configured upstream servers.
    ///
    /// Connection failures are logged and skipped — the engine starts with
    /// whatever servers connected successfully.
    pub async fn from_configs(configs: HashMap<String, ServerConfig>) -> Result<Self> {
        validate_configs(&configs)?;
        Self::from_configs_with_timeout(configs, Duration::from_secs(CONNECT_TIMEOUT_SECS)).await
    }

    async fn from_configs_with_timeout(
        configs: HashMap<String, ServerConfig>,
        connect_timeout: Duration,
    ) -> Result<Self> {
        let session_id = new_session_id();
        let health = configs
            .iter()
            .map(|(name, config)| (name.clone(), initial_health(name, config)))
            .collect();
        let engine = Self {
            servers: RwLock::new(HashMap::new()),
            configs: RwLock::new(configs.clone()),
            health: RwLock::new(health),
            session_id,
            policy: RwLock::new(Arc::new(AllowAllProxyPolicy)),
            observer: RwLock::new(None),
            policy_audit: Mutex::new(VecDeque::new()),
        };

        let mut names: Vec<_> = configs.keys().cloned().collect();
        names.sort();
        futures::future::join_all(names.iter().filter_map(|name| {
            configs.get(name).map(|config| {
                engine.connect_and_record_with_timeout(name, config, now_ms(), connect_timeout)
            })
        }))
        .await;

        Ok(engine)
    }

    /// Install the policy used for subsequent upstream calls.
    ///
    /// Replacing a policy is intentionally explicit and leaves existing audit
    /// records intact for operator inspection.
    pub async fn set_policy(&self, policy: Arc<dyn ProxyPolicy>) {
        *self.policy.write().await = policy;
    }

    /// Install the observer for successful upstream calls.
    pub async fn set_call_observer(&self, observer: Arc<dyn ProxyCallObserver>) {
        *self.observer.write().await = Some(observer);
    }

    /// Return the bounded, redacted authorization audit trail for this proxy
    /// process. Entries never include forwarded arguments or upstream output.
    pub fn policy_audit(&self) -> Vec<ProxyPolicyAuditEntry> {
        self.policy_audit
            .lock()
            .expect("proxy policy audit mutex poisoned")
            .iter()
            .cloned()
            .collect()
    }

    /// Snapshot optional-upstream health without exposing endpoints or secrets.
    pub async fn health_snapshot(&self) -> ProxyHealthSnapshot {
        let health = self.health.read().await;
        let mut servers: Vec<_> = health.values().cloned().collect();
        servers.sort_by(|a, b| a.name.cmp(&b.name));
        let healthy = servers
            .iter()
            .filter(|server| server.state == UpstreamState::Healthy)
            .count();
        ProxyHealthSnapshot {
            session_id: self.session_id.clone(),
            generated_at_ms: now_ms(),
            healthy,
            degraded: servers.len().saturating_sub(healthy),
            servers,
        }
        .sanitized()
    }

    /// Retry due failed upstreams once each. Exponential backoff bounds retries
    /// without blocking startup or hammering optional services.
    pub async fn retry_unhealthy(&self) -> usize {
        self.retry_unhealthy_at(now_ms()).await
    }

    async fn retry_unhealthy_at(&self, now: u64) -> usize {
        let due: Vec<String> = {
            let health = self.health.read().await;
            health
                .iter()
                .filter(|(_, record)| {
                    record.state != UpstreamState::Healthy
                        && record.next_retry_at_ms.is_some_and(|retry| retry <= now)
                })
                .map(|(name, _)| name.clone())
                .collect()
        };
        let configs = self.configs.read().await.clone();
        futures::future::join_all(due.iter().filter_map(|name| {
            configs
                .get(name)
                .map(|config| self.connect_and_record(name, config, now))
        }))
        .await;
        due.len()
    }

    async fn connect_and_record(&self, name: &str, config: &ServerConfig, now: u64) {
        self.connect_and_record_with_timeout(
            name,
            config,
            now,
            Duration::from_secs(CONNECT_TIMEOUT_SECS),
        )
        .await;
    }

    async fn connect_and_record_with_timeout(
        &self,
        name: &str,
        config: &ServerConfig,
        now: u64,
        connect_timeout: Duration,
    ) {
        let public_name = {
            let configs = self.configs.read().await;
            public_upstream_ids(configs.keys().map(String::as_str))
                .get(name)
                .cloned()
                .unwrap_or_else(|| public_upstream_id(name))
        };
        let result = tokio::time::timeout(connect_timeout, connect_server(name, config))
            .await
            .map_err(|_| anyhow::anyhow!("MCP upstream connection timed out"))
            .and_then(std::convert::identity);
        match result {
            Ok(connected) => {
                let tool_count = connected.tools.len();
                self.servers
                    .write()
                    .await
                    .insert(name.to_string(), connected);
                let mut health = self.health.write().await;
                let record = health
                    .entry(name.to_string())
                    .or_insert_with(|| initial_health(name, config));
                record_success(record, tool_count, now);
                tracing::info!(
                    upstream = %public_name,
                    tool_count,
                    proxy_session = %self.session_id,
                    "MCP upstream connected"
                );
            }
            Err(error) => {
                let code = safe_error_code(&classify_error(&error));
                let visibility = {
                    let mut health = self.health.write().await;
                    let record = health
                        .entry(name.to_string())
                        .or_insert_with(|| initial_health(name, config));
                    if code == "executable_missing"
                        && let ServerConfig::Stdio { command, .. } = config
                    {
                        record.executable = Some(command.trim().to_string());
                    }
                    record_failure(record, &code, now)
                };
                match visibility {
                    FailureVisibility::Error if code == "executable_missing" => tracing::error!(
                        upstream = %public_name,
                        error_code = code,
                        proxy_session = %self.session_id,
                        "Optional MCP upstream executable is missing; CAS will continue without retry"
                    ),
                    FailureVisibility::Error => tracing::error!(
                        upstream = %public_name,
                        error_code = code,
                        proxy_session = %self.session_id,
                        "Optional MCP upstream unavailable; CAS will continue and retry"
                    ),
                    FailureVisibility::Debug => tracing::debug!(
                        upstream = %public_name,
                        error_code = code,
                        proxy_session = %self.session_id,
                        "Optional MCP upstream retry failed"
                    ),
                }
            }
        }
    }

    /// Search across all upstream tool catalogs.
    ///
    /// The `query` parameter supports:
    /// - Plain keywords: case-insensitive substring match on tool name + description
    /// - `server:name` prefix: filter to a specific server before matching keywords
    /// - Empty query: returns all tools
    ///
    /// Results are returned as a JSON array of matching tools with server, name,
    /// description, and input_schema fields. If `max_length` is set, the JSON
    /// output is truncated to that many bytes.
    pub async fn search(&self, query: &str, max_length: Option<usize>) -> Result<Value> {
        let servers = self.servers.read().await;
        let configs = self.configs.read().await;
        let public_servers = public_upstream_ids(configs.keys().map(String::as_str));

        // Parse optional server: prefix
        let (server_filter, keywords) = parse_search_query(query);

        // A configured upstream that failed to connect used to look exactly
        // like an unknown server: `server:viktor` simply returned `[]`. That
        // hides credential loss after a daemon restart and strands any
        // durable work that relies on the upstream. Keep ordinary no-match
        // searches empty, but make an explicitly selected, configured and
        // disconnected server a visible gateway failure.
        if let Some(filter) = server_filter.as_deref() {
            let filter = filter.to_ascii_lowercase();
            let unavailable = configs.keys().find(|name| {
                public_servers
                    .get(*name)
                    .is_some_and(|public| public.to_ascii_lowercase() == filter)
                    && !servers.contains_key(*name)
            });
            if let Some(name) = unavailable {
                let public = public_servers
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| public_upstream_id(name));
                anyhow::bail!(
                    "MCP upstream '{public}' is absent: it is configured but not connected; inspect proxy_health and restore its credential before retrying"
                );
            }
        }

        let mut results: Vec<SearchResult> = Vec::new();

        for (server_name, connected) in servers.iter() {
            let public_server = public_servers
                .get(server_name)
                .cloned()
                .unwrap_or_else(|| public_upstream_id(server_name));
            // Apply server filter if present
            if let Some(ref filter) = server_filter {
                let filter = filter.to_lowercase();
                if !public_server.to_lowercase().contains(&filter) {
                    continue;
                }
            }

            let public_tools =
                public_tool_ids(connected.tools.iter().map(|tool| tool.name.as_ref()));
            let policy = self.policy.read().await.clone();
            for tool in &connected.tools {
                if matches_keywords(tool, &keywords) {
                    let policy = match policy.catalog_decision(server_name, tool.name.as_ref()) {
                        ProxyPolicyDecision::Allow => None,
                        ProxyPolicyDecision::Deny { .. } => Some("denied by policy".to_string()),
                    };
                    results.push(SearchResult {
                        server: public_server.clone(),
                        name: public_tools
                            .get(tool.name.as_ref())
                            .cloned()
                            .unwrap_or_else(|| public_tool_id(tool.name.as_ref())),
                        description: tool.description.as_ref().map(|d| d.to_string()),
                        input_schema: serde_json::to_value(&*tool.input_schema).unwrap_or_default(),
                        policy,
                    });
                }
            }
        }

        let mut json = serde_json::to_string_pretty(&results)?;
        if let Some(max) = max_length {
            if json.len() > max {
                json.truncate(max);
            }
        }

        Ok(Value::String(json))
    }

    /// Execute tool calls across upstream MCP servers.
    ///
    /// The `code` parameter supports two formats:
    ///
    /// **JSON dispatch** (preferred):
    /// ```json
    /// { "server": "github", "tool": "list_issues", "args": { "repo": "myorg/app" } }
    /// ```
    ///
    /// **Batch (parallel)** — array of calls:
    /// ```json
    /// [
    ///   { "server": "github", "tool": "list_issues", "args": { "repo": "myorg/app" } },
    ///   { "server": "sentry", "tool": "list_errors", "args": { "project": "backend" } }
    /// ]
    /// ```
    ///
    /// **Dot-call syntax** (fallback):
    /// ```text
    /// server.tool_name({ "param": "value" })
    /// ```
    pub async fn execute(
        &self,
        caller: &ProxyCaller,
        code: &str,
        max_length: Option<usize>,
    ) -> Result<ExecuteResult> {
        let calls = parse_dispatch(code)?;

        let mut text_parts: Vec<String> = Vec::new();
        let mut images: Vec<ImageResult> = Vec::new();

        if calls.len() == 1 {
            let call = &calls[0];
            let result = self
                .call_tool_raw(
                    caller,
                    ProxyDispatchKind::Direct,
                    &call.server,
                    &call.tool,
                    call.args.clone(),
                )
                .await?;
            collect_result(&result, &mut text_parts, &mut images);
        } else {
            // Execute in parallel
            let futures: Vec<_> = calls
                .iter()
                .map(|call| {
                    self.call_tool_raw(
                        caller,
                        ProxyDispatchKind::Direct,
                        &call.server,
                        &call.tool,
                        call.args.clone(),
                    )
                })
                .collect();

            let results = futures::future::join_all(futures).await;

            for (i, result) in results.into_iter().enumerate() {
                match result {
                    Ok(result) => collect_result(&result, &mut text_parts, &mut images),
                    Err(e) => {
                        text_parts.push(format!(
                            "[{}.{} error]: {e}",
                            public_upstream_id(&calls[i].server),
                            public_tool_id(&calls[i].tool)
                        ));
                    }
                }
            }
        }

        let mut text = text_parts.join("\n\n");
        if let Some(max) = max_length {
            if text.len() > max {
                text.truncate(max);
            }
        }

        Ok(ExecuteResult { text, images })
    }

    /// Return the total number of tools across all connected servers.
    pub async fn tool_count(&self) -> usize {
        let servers = self.servers.read().await;
        servers.values().map(|s| s.tools.len()).sum()
    }

    /// Whether a configured upstream has an active connection in this proxy
    /// session. Callers use this only for explicit operator-facing recovery
    /// paths; normal dispatch still goes through the policy and routing gates.
    pub async fn upstream_connected(&self, server: &str) -> bool {
        self.servers.read().await.contains_key(server)
    }

    /// Return catalog entries grouped by server name.
    pub async fn catalog_entries_by_server(&self) -> HashMap<String, Vec<CatalogEntry>> {
        let servers = self.servers.read().await;
        let configs = self.configs.read().await;
        let public_servers = public_upstream_ids(configs.keys().map(String::as_str));
        servers
            .iter()
            .map(|(name, connected)| {
                let public_tools =
                    public_tool_ids(connected.tools.iter().map(|tool| tool.name.as_ref()));
                let entries = connected
                    .tools
                    .iter()
                    .map(|tool| CatalogEntry {
                        name: public_tools
                            .get(tool.name.as_ref())
                            .cloned()
                            .unwrap_or_else(|| public_tool_id(tool.name.as_ref())),
                        description: tool.description.as_ref().map(|d| d.to_string()),
                        input_schema: serde_json::to_value(&*tool.input_schema).unwrap_or_default(),
                    })
                    .collect();
                (
                    public_servers
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| public_upstream_id(name)),
                    entries,
                )
            })
            .collect()
    }

    /// Reload with new server configurations.
    ///
    /// Compares against current connections:
    /// - Removes servers no longer in config
    /// - Connects newly added servers
    /// - Reconnects servers whose config changed
    /// - Leaves unchanged servers connected
    pub async fn reload(&self, configs: HashMap<String, ServerConfig>) -> Result<()> {
        validate_configs(&configs)?;
        let mut servers = self.servers.write().await;
        let old_configs = self.configs.read().await.clone();
        let old_public_names = public_upstream_ids(old_configs.keys().map(String::as_str));
        let new_public_names = public_upstream_ids(configs.keys().map(String::as_str));

        // Remove servers no longer in config
        let current_names: Vec<String> = servers.keys().cloned().collect();
        for name in &current_names {
            if !configs.contains_key(name) {
                if let Some(removed) = servers.remove(name) {
                    let _ = removed.service.cancel().await;
                    tracing::info!(
                        upstream = %old_public_names
                            .get(name)
                            .cloned()
                            .unwrap_or_else(|| public_upstream_id(name)),
                        "MCP upstream disconnected"
                    );
                }
                self.health.write().await.remove(name);
            }
        }
        self.health
            .write()
            .await
            .retain(|name, _| configs.contains_key(name));

        let mut reconnect = Vec::new();
        // Connect new servers and reconnect changed ones
        for (name, config) in &configs {
            if old_configs.get(name) == Some(config) {
                continue;
            }
            if servers.contains_key(name) {
                if let Some(removed) = servers.remove(name) {
                    let _ = removed.service.cancel().await;
                    tracing::info!(
                        upstream = %new_public_names
                            .get(name)
                            .cloned()
                            .unwrap_or_else(|| public_upstream_id(name)),
                        "MCP upstream config changed"
                    );
                }
            }
            self.health
                .write()
                .await
                .insert(name.clone(), initial_health(name, config));
            reconnect.push((name.clone(), config.clone()));
        }
        drop(servers);
        *self.configs.write().await = configs;
        for (name, config) in reconnect {
            self.connect_and_record(&name, &config, now_ms()).await;
        }

        Ok(())
    }

    /// Call a tool on a specific server by name.
    pub async fn call_tool(
        &self,
        caller: &ProxyCaller,
        server_name: &str,
        tool_name: &str,
        arguments: Option<serde_json::Map<String, Value>>,
    ) -> Result<Value> {
        let result = self
            .call_tool_raw(
                caller,
                ProxyDispatchKind::Direct,
                server_name,
                tool_name,
                arguments,
            )
            .await?;

        serde_json::to_value(result).context("failed to serialize tool result")
    }

    /// Call one tool through the receipted external-production verification
    /// path. Only the CAS service should select this dispatch kind.
    pub async fn call_external_production_verification_tool(
        &self,
        caller: &ProxyCaller,
        server_name: &str,
        tool_name: &str,
        arguments: Option<serde_json::Map<String, Value>>,
    ) -> Result<Value> {
        let result = self
            .call_tool_raw(
                caller,
                ProxyDispatchKind::ExternalProductionVerification,
                server_name,
                tool_name,
                arguments,
            )
            .await?;
        serde_json::to_value(result).context("failed to serialize delegated tool result")
    }

    /// Call a tool and return the raw rmcp result (for internal use by execute).
    async fn call_tool_raw(
        &self,
        caller: &ProxyCaller,
        dispatch_kind: ProxyDispatchKind,
        server_name: &str,
        tool_name: &str,
        arguments: Option<serde_json::Map<String, Value>>,
    ) -> Result<rmcp::model::CallToolResult> {
        use rmcp::model::CallToolRequestParams;

        self.authorize(caller, dispatch_kind, server_name, tool_name, &arguments)
            .await?;

        let result = self
            .call_upstream(
                server_name,
                CallToolRequestParams {
                    name: tool_name.to_string().into(),
                    arguments: arguments.clone(),
                    meta: None,
                    task: None,
                },
            )
            .await?;

        // The receipted external-verification lane owns its own polling and
        // lifecycle. Observing it here would create a second inbound watch and
        // duplicate its result into the prompt queue.
        if dispatch_kind == ProxyDispatchKind::Direct
            && let Some(observer) = self.observer.read().await.clone()
        {
            let serialized = serde_json::to_value(&result).unwrap_or(Value::Null);
            observer.call_succeeded(ProxyCallEvent {
                caller,
                server: server_name,
                tool: tool_name,
                arguments: &arguments,
                result: &serialized,
            });
        }

        Ok(result)
    }

    async fn authorize(
        &self,
        caller: &ProxyCaller,
        dispatch_kind: ProxyDispatchKind,
        server_name: &str,
        tool_name: &str,
        arguments: &Option<serde_json::Map<String, Value>>,
    ) -> Result<()> {
        let policy = self.policy.read().await.clone();
        let request = ProxyPolicyRequest {
            caller,
            server: server_name,
            tool: tool_name,
            arguments,
            dispatch_kind,
        };
        let decision = policy.decide(&request);
        let (allowed, reason) = match decision {
            ProxyPolicyDecision::Allow => (true, None),
            ProxyPolicyDecision::Deny { reason } => (false, Some(reason)),
        };
        self.record_policy_decision(caller, server_name, tool_name, allowed, reason.as_deref());

        if allowed {
            tracing::debug!(
                agent_id = %caller.agent_id,
                role = %caller.role,
                server = %public_upstream_id(server_name),
                tool = %public_tool_id(tool_name),
                "MCP proxy policy allowed upstream call"
            );
            Ok(())
        } else {
            let reason = reason.unwrap_or_else(|| "policy denied this call".to_string());
            tracing::warn!(
                agent_id = %caller.agent_id,
                role = %caller.role,
                server = %public_upstream_id(server_name),
                tool = %public_tool_id(tool_name),
                reason = %reason,
                "MCP proxy policy denied upstream call before forwarding"
            );
            anyhow::bail!(
                "proxy policy denied tool '{}' on '{}': {}",
                public_tool_id(tool_name),
                public_upstream_id(server_name),
                reason
            );
        }
    }

    fn record_policy_decision(
        &self,
        caller: &ProxyCaller,
        server: &str,
        tool: &str,
        allowed: bool,
        reason: Option<&str>,
    ) {
        let entry = ProxyPolicyAuditEntry {
            timestamp_ms: now_ms(),
            caller: caller.clone(),
            server: public_upstream_id(server),
            tool: public_tool_id(tool),
            allowed,
            reason: reason.map(str::to_string),
        };
        let mut audit = self
            .policy_audit
            .lock()
            .expect("proxy policy audit mutex poisoned");
        if audit.len() == POLICY_AUDIT_LIMIT {
            audit.pop_front();
        }
        audit.push_back(entry);
    }

    async fn call_upstream(
        &self,
        server_name: &str,
        mut request: rmcp::model::CallToolRequestParams,
    ) -> Result<rmcp::model::CallToolResult> {
        let configs = self.configs.read().await;
        let public_servers = public_upstream_ids(configs.keys().map(String::as_str));
        let requested_public = public_servers
            .get(server_name)
            .cloned()
            .unwrap_or_else(|| public_upstream_id(server_name));
        let resolved_server = resolve_routing_name(server_name, &public_servers, "server")?;
        let resolved_public = public_servers
            .get(&resolved_server)
            .cloned()
            .unwrap_or_else(|| public_upstream_id(&resolved_server));
        drop(configs);
        let servers = self.servers.read().await;
        let server = servers.get(&resolved_server).with_context(|| {
            let mut available: Vec<String> = servers
                .keys()
                .map(|name| {
                    public_servers
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| public_upstream_id(name))
                })
                .collect();
            available.sort();
            format!(
                "MCP upstream '{}' is absent: it is configured but not connected; inspect proxy_health and restore its credential before retrying. Available: {}",
                requested_public,
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            )
        })?;
        let public_tools = public_tool_ids(server.tools.iter().map(|tool| tool.name.as_ref()));
        let requested_tool = request.name.to_string();
        let resolved_tool = resolve_routing_name(&requested_tool, &public_tools, "tool")?;
        let resolved_public_tool = public_tools
            .get(&resolved_tool)
            .cloned()
            .unwrap_or_else(|| public_tool_id(&resolved_tool));
        request.name = resolved_tool.into();
        let generation = server.generation;
        let result = server.service.call_tool(request).await;
        let completion = CALL_COMPLETION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        match result {
            Ok(result) => {
                server
                    .last_successful_call
                    .fetch_max(completion, Ordering::Release);
                Ok(result)
            }
            Err(error) => {
                drop(servers);
                if let Some(code) = classify_live_failure(&error) {
                    self.record_live_failure(&resolved_server, generation, completion, code)
                        .await;
                }
                Err(anyhow::Error::from(error).context(format!(
                    "tool call '{resolved_public_tool}' on '{resolved_public}' failed"
                )))
            }
        }
    }

    async fn record_live_failure(
        &self,
        server_name: &str,
        generation: u64,
        failure_completion: u64,
        code: &'static str,
    ) {
        let public_name = {
            let configs = self.configs.read().await;
            public_upstream_ids(configs.keys().map(String::as_str))
                .get(server_name)
                .cloned()
                .unwrap_or_else(|| public_upstream_id(server_name))
        };
        let mut servers = self.servers.write().await;
        let should_remove = servers.get(server_name).is_some_and(|server| {
            live_failure_applies(
                server.generation,
                generation,
                server.last_successful_call.load(Ordering::Acquire),
                failure_completion,
            )
        });
        if !should_remove {
            return;
        }
        let removed = servers
            .remove(server_name)
            .expect("generation checked immediately before removal");
        let visibility = {
            let mut health = self.health.write().await;
            let Some(record) = health.get_mut(server_name) else {
                return;
            };
            record_failure(record, code, now_ms())
        };
        drop(servers);
        let _ = removed.service.cancel().await;
        match visibility {
            FailureVisibility::Error => tracing::error!(
                upstream = %public_name,
                error_code = code,
                proxy_session = %self.session_id,
                "Optional MCP upstream connection failed after startup; retry scheduled"
            ),
            FailureVisibility::Debug => tracing::debug!(
                upstream = %public_name,
                error_code = code,
                proxy_session = %self.session_id,
                "Optional MCP upstream connection failure already recorded"
            ),
        }
    }

    /// Gracefully shut down all connected servers.
    pub async fn shutdown(&self) {
        let public_names = {
            let configs = self.configs.read().await;
            public_upstream_ids(configs.keys().map(String::as_str))
        };
        let mut servers = self.servers.write().await;
        for (name, server) in servers.drain() {
            if let Err(e) = server.service.cancel().await {
                eprintln!(
                    "[proxy] Error shutting down '{}': {e}",
                    public_names
                        .get(&name)
                        .cloned()
                        .unwrap_or_else(|| public_upstream_id(&name))
                );
            }
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn new_session_id() -> String {
    format!(
        "proxy-{}-{}-{}",
        std::process::id(),
        now_ms(),
        SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn initial_health(name: &str, config: &ServerConfig) -> UpstreamHealth {
    UpstreamHealth {
        // Keep the routing identity only inside the engine. health_snapshot()
        // projects the complete set immediately before any public boundary.
        name: name.to_string(),
        transport: transport_name(config).to_string(),
        state: UpstreamState::Degraded,
        executable: None,
        attempts: 0,
        consecutive_failures: 0,
        tool_count: 0,
        last_error_code: None,
        last_attempt_at_ms: None,
        next_retry_at_ms: None,
    }
}

fn transport_name(config: &ServerConfig) -> &'static str {
    match config {
        ServerConfig::Stdio { .. } => "stdio",
        ServerConfig::Http { .. } => "http",
        ServerConfig::Sse { .. } => "sse",
    }
}

fn record_success(record: &mut UpstreamHealth, tool_count: usize, now: u64) {
    record.attempts = record.attempts.saturating_add(1);
    record.consecutive_failures = 0;
    record.tool_count = tool_count;
    record.state = UpstreamState::Healthy;
    record.executable = None;
    record.last_error_code = None;
    record.last_attempt_at_ms = Some(now);
    record.next_retry_at_ms = None;
}

fn record_failure(record: &mut UpstreamHealth, error_code: &str, now: u64) -> FailureVisibility {
    record.attempts = record.attempts.saturating_add(1);
    record.consecutive_failures = record.consecutive_failures.saturating_add(1);
    record.tool_count = 0;
    record.last_error_code = Some(error_code.to_string());
    record.last_attempt_at_ms = Some(now);
    if error_code == "executable_missing" {
        record.state = UpstreamState::ExecutableMissing;
        record.next_retry_at_ms = None;
        return FailureVisibility::Error;
    }
    let shift = record.consecutive_failures.saturating_sub(1).min(6);
    let delay_secs = RETRY_BASE_SECS
        .saturating_mul(1_u64 << shift)
        .min(RETRY_MAX_SECS);
    record.next_retry_at_ms =
        Some(now.saturating_add(Duration::from_secs(delay_secs).as_millis() as u64));
    record.state = UpstreamState::Backoff;
    if record.consecutive_failures == 1 {
        FailureVisibility::Error
    } else {
        FailureVisibility::Debug
    }
}

#[derive(Debug)]
struct MissingCredentialError {
    name: String,
}

impl std::fmt::Display for MissingCredentialError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("required MCP upstream credential environment variable is unavailable")
    }
}

impl std::error::Error for MissingCredentialError {}

const MISSING_CREDENTIAL_ENV_PREFIX: &str = "missing_credential_env:";

fn classify_error(error: &anyhow::Error) -> String {
    if let Some(name) = error.chain().find_map(|cause| {
        cause
            .downcast_ref::<MissingCredentialError>()
            .map(|missing| missing.name.as_str())
    }) {
        return format!("{MISSING_CREDENTIAL_ENV_PREFIX}{name}");
    }
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("authrequired")
        || message.contains("auth required")
        || message.contains("unauthorized")
        || message.contains("status 401")
        || message.contains("status: 401")
    {
        "authentication_required".to_string()
    } else if message.contains("unexpectedcontenttype")
        || message.contains("unexpected content type")
    {
        "unexpected_content_type".to_string()
    } else if message.contains("invalid url") || message.contains("scheme is not http") {
        "invalid_url".to_string()
    } else if message.contains("timed out") || message.contains("timeout") {
        "timeout".to_string()
    } else if message.contains("no such file or directory")
        || message.contains("os error 2")
        || message.contains("exit status: 127")
        || message.contains("status 127")
        || message.contains("code 127")
    {
        "executable_missing".to_string()
    } else {
        "connection_failed".to_string()
    }
}

fn classify_live_failure(error: &rmcp::service::ServiceError) -> Option<&'static str> {
    use rmcp::service::ServiceError;
    match error {
        ServiceError::TransportSend(_)
        | ServiceError::TransportClosed
        | ServiceError::UnexpectedResponse => Some("connection_failed"),
        ServiceError::Timeout { .. } => Some("timeout"),
        ServiceError::McpError(_) | ServiceError::Cancelled { .. } => None,
        _ => None,
    }
}

fn safe_session_id(session_id: &str) -> String {
    if session_id.len() <= 96
        && session_id.strip_prefix("proxy-").is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'-')
        })
    {
        session_id.to_string()
    } else {
        "proxy-unknown".to_string()
    }
}

fn resolve_routing_name(
    requested: &str,
    public_names: &std::collections::BTreeMap<String, String>,
    kind: &str,
) -> Result<String> {
    if public_names.contains_key(requested) {
        return Ok(requested.to_string());
    }
    let matches: Vec<&str> = public_names
        .iter()
        .filter_map(|(raw, public)| (public == requested).then_some(raw.as_str()))
        .collect();
    match matches.as_slice() {
        [raw] => Ok((*raw).to_string()),
        [] => Ok(requested.to_string()),
        _ => anyhow::bail!("ambiguous public {kind} identity"),
    }
}

fn safe_transport(transport: &str) -> &'static str {
    match transport {
        "stdio" => "stdio",
        "http" => "http",
        "sse" => "sse",
        _ => "unknown",
    }
}

fn safe_error_code(code: &str) -> String {
    if let Some(name) = code.strip_prefix(MISSING_CREDENTIAL_ENV_PREFIX) {
        return format!(
            "{MISSING_CREDENTIAL_ENV_PREFIX}{}",
            safe_environment_name(name).unwrap_or("unknown")
        );
    }
    match code {
        "authentication_required"
        | "unexpected_content_type"
        | "invalid_url"
        | "timeout"
        | "connection_failed"
        | "executable_missing" => code.to_string(),
        _ => "unknown".to_string(),
    }
}

fn safe_environment_name(name: &str) -> Option<&str> {
    (!name.is_empty()
        && name.len() <= 256
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    .then_some(name)
}

fn live_failure_applies(
    installed_generation: u64,
    failed_generation: u64,
    last_successful_call: u64,
    failure_completion: u64,
) -> bool {
    installed_generation == failed_generation && last_successful_call <= failure_completion
}

/// Resolve a stdio command using the same PATH semantics as process launch.
/// Absolute and relative paths must point at executable files; bare commands
/// are searched through the current process PATH.
pub fn resolve_stdio_executable(command: &str) -> Option<PathBuf> {
    resolve_stdio_executable_at_depth(command, 0)
}

fn resolve_stdio_executable_at_depth(command: &str, depth: u8) -> Option<PathBuf> {
    if depth > 8 {
        return None;
    }
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    let path = Path::new(command);
    let has_path = path.is_absolute()
        || path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty());
    if has_path {
        return is_executable(path, depth).then(|| path.to_path_buf());
    }

    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(command))
        .find(|candidate| is_executable(candidate, depth))
}

fn is_executable(path: &Path, depth: u8) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }

        // `Command::new` asks the kernel to resolve a script's `#!`
        // interpreter. A script can therefore be executable while still
        // failing with ENOENT when its interpreter was removed; validate that
        // second hop so doctor reports the same stale-path failure.
        let Ok(contents) = std::fs::read(path) else {
            return true;
        };
        let Some(first_line) = contents.split(|byte| *byte == b'\n').next() else {
            return true;
        };
        let Ok(first_line) = std::str::from_utf8(first_line) else {
            return true;
        };
        let Some(shebang) = first_line.strip_prefix("#!") else {
            return true;
        };
        let mut words = shebang.split_whitespace();
        let Some(interpreter) = words.next() else {
            return false;
        };
        let interpreter = if interpreter.ends_with("/env") {
            words.find(|word| !word.starts_with('-'))
        } else {
            Some(interpreter)
        };
        interpreter.is_some_and(|interpreter| {
            resolve_stdio_executable_at_depth(interpreter, depth + 1).is_some()
        })
    }
    #[cfg(not(unix))]
    {
        let _ = depth;
        true
    }
}

fn safe_executable(executable: &str) -> String {
    if executable.len() <= 4096 && executable.chars().all(|character| !character.is_control()) {
        executable.to_string()
    } else {
        "unknown".to_string()
    }
}

/// Connect to a single upstream MCP server and discover its tools.
async fn connect_server(name: &str, config: &ServerConfig) -> Result<ConnectedServer> {
    use rmcp::service::ServiceExt;

    let service: McpClientService = match config {
        ServerConfig::Stdio { command, args, env } => {
            let cmd = Command::new(command);
            let env_clone = env.clone();
            let args_clone = args.clone();
            let transport = TokioChildProcess::new(cmd.configure(move |cmd| {
                cmd.args(&args_clone);
                for (k, v) in &env_clone {
                    cmd.env(k, v);
                }
            }))
            .with_context(|| format!("failed to spawn stdio process for '{name}'"))?;

            ().serve(transport)
                .await
                .with_context(|| format!("failed to initialize MCP client for '{name}'"))?
        }

        ServerConfig::Http {
            url, auth, headers, ..
        } => {
            use rmcp::transport::StreamableHttpClientTransport;

            let cfg = http_transport_config(url, auth.as_deref(), headers)?;

            let transport = StreamableHttpClientTransport::from_config(cfg);

            ().serve(transport)
                .await
                .with_context(|| format!("failed to connect HTTP MCP client for '{name}'"))?
        }

        ServerConfig::Sse {
            url, auth, headers, ..
        } => {
            use rmcp::transport::StreamableHttpClientTransport;

            let cfg = http_transport_config(url, auth.as_deref(), headers)?;

            let transport = StreamableHttpClientTransport::from_config(cfg);

            ().serve(transport)
                .await
                .with_context(|| format!("failed to connect SSE MCP client for '{name}'"))?
        }
    };

    // Discover tools from the server
    let tools_result = service
        .list_tools(Default::default())
        .await
        .with_context(|| format!("failed to list tools from '{name}'"))?;

    Ok(ConnectedServer {
        service,
        tools: tools_result.tools,
        generation: CONNECTION_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        last_successful_call: AtomicU64::new(0),
    })
}

fn http_transport_config(
    url: &str,
    auth: Option<&str>,
    headers: &HashMap<String, String>,
) -> Result<rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig> {
    use http::{HeaderName, HeaderValue};
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let mut custom_headers = HashMap::new();
    for (name, value) in headers {
        let name =
            HeaderName::from_bytes(name.as_bytes()).context("invalid MCP upstream header name")?;
        let value = resolve_credential(value)?;
        let value = HeaderValue::from_str(&value).context("invalid MCP upstream header value")?;
        custom_headers.insert(name, value);
    }

    let mut config = StreamableHttpClientTransportConfig::with_uri(Arc::<str>::from(url));
    config.custom_headers = custom_headers;
    if let Some(auth) = auth {
        // RMCP owns the Authorization header and applies the Bearer scheme itself.
        // Supplying a pre-prefixed value sends `Bearer Bearer <token>`, which a
        // conforming streamable HTTP server rejects during initialization.
        config.auth_header = Some(resolve_credential(auth)?);
    }
    Ok(config)
}

fn resolve_credential(value: &str) -> Result<String> {
    let env_name = value
        .strip_prefix("env:")
        .or_else(|| value.strip_prefix("${").and_then(|v| v.strip_suffix('}')));
    match env_name {
        Some(name) if !name.is_empty() => std::env::var(name).map_err(|_| {
            anyhow::Error::new(MissingCredentialError {
                name: name.to_string(),
            })
        }),
        _ => Ok(value.to_string()),
    }
}

/// A search result entry including the server name.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SearchResult {
    server: String,
    name: String,
    description: Option<String>,
    input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<String>,
}

/// Parse a search query into an optional server filter and keyword tokens.
///
/// Supports `server:name keyword1 keyword2` syntax.
fn parse_search_query(query: &str) -> (Option<String>, Vec<String>) {
    let mut server_filter = None;
    let mut keywords = Vec::new();

    for token in query.split_whitespace() {
        if let Some(server) = token.strip_prefix("server:") {
            server_filter = Some(server.to_string());
        } else {
            keywords.push(token.to_lowercase());
        }
    }

    (server_filter, keywords)
}

/// A parsed tool call dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolCall {
    server: String,
    tool: String,
    #[serde(default)]
    args: Option<serde_json::Map<String, Value>>,
}

/// Parse the `code` parameter into one or more tool calls.
///
/// Tries JSON dispatch first, then falls back to dot-call syntax.
fn parse_dispatch(code: &str) -> Result<Vec<ToolCall>> {
    let trimmed = code.trim();

    // Try JSON array
    if trimmed.starts_with('[') {
        let calls: Vec<ToolCall> = serde_json::from_str(trimmed)
            .context("failed to parse batch dispatch as JSON array")?;
        if calls.is_empty() {
            anyhow::bail!("empty dispatch array");
        }
        return Ok(calls);
    }

    // Try JSON object
    if trimmed.starts_with('{') {
        let call: ToolCall =
            serde_json::from_str(trimmed).context("failed to parse dispatch as JSON object")?;
        return Ok(vec![call]);
    }

    // Fall back to dot-call syntax: server.tool_name({ ... })
    parse_dot_syntax(trimmed)
}

/// Parse `server.tool_name({ "param": "value" })` syntax.
fn parse_dot_syntax(code: &str) -> Result<Vec<ToolCall>> {
    let dot_pos = code
        .find('.')
        .context("invalid syntax: expected 'server.tool(args)' or JSON dispatch.\n\nExamples:\n  github.list_issues({\"repo\": \"myorg/app\"})\n  {\"server\": \"github\", \"tool\": \"list_issues\", \"args\": {\"repo\": \"myorg/app\"}}")?;

    let server = &code[..dot_pos];
    let rest = &code[dot_pos + 1..];

    // Find the tool name (everything before the first '(')
    let paren_pos = rest.find('(');

    let (tool, args) = if let Some(pos) = paren_pos {
        let tool_name = &rest[..pos];
        let args_str = rest[pos..].trim();

        // Strip surrounding parens
        let args_inner = args_str
            .strip_prefix('(')
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(args_str)
            .trim();

        let args = if args_inner.is_empty() {
            None
        } else {
            let parsed: serde_json::Map<String, Value> = serde_json::from_str(args_inner)
                .with_context(|| format!("failed to parse arguments as JSON: {args_inner}"))?;
            Some(parsed)
        };

        (tool_name.to_string(), args)
    } else {
        (rest.trim().to_string(), None)
    };

    Ok(vec![ToolCall {
        server: server.to_string(),
        tool,
        args,
    }])
}

/// Extract text and images from an rmcp CallToolResult.
fn collect_result(
    result: &rmcp::model::CallToolResult,
    text_parts: &mut Vec<String>,
    images: &mut Vec<ImageResult>,
) {
    use rmcp::model::RawContent;

    for content in &result.content {
        match &content.raw {
            RawContent::Text(t) => {
                text_parts.push(t.text.clone());
            }
            RawContent::Image(img) => {
                images.push(ImageResult {
                    data: img.data.clone(),
                    mime_type: img.mime_type.clone(),
                });
            }
            _ => {
                // Resource, Audio, ResourceLink — serialize as JSON text
                if let Ok(json) = serde_json::to_string_pretty(&content) {
                    text_parts.push(json);
                }
            }
        }
    }
}

/// Check if a tool matches all keyword tokens (case-insensitive substring on name + description).
fn matches_keywords(tool: &Tool, keywords: &[String]) -> bool {
    if keywords.is_empty() {
        return true;
    }

    let name_lower = tool.name.to_lowercase();
    let desc_lower = tool
        .description
        .as_ref()
        .map(|d| d.to_lowercase())
        .unwrap_or_default();

    keywords
        .iter()
        .all(|kw| name_lower.contains(kw.as_str()) || desc_lower.contains(kw.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::Tool;
    use std::borrow::Cow;
    use std::sync::Arc;

    fn install_test_crypto_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    fn make_tool(name: &str, description: &str) -> Tool {
        Tool {
            name: Cow::Owned(name.to_string()),
            title: None,
            description: Some(Cow::Owned(description.to_string())),
            input_schema: Arc::new(serde_json::Map::new()),
            output_schema: None,
            annotations: None,
            icons: None,
            meta: None,
            execution: None,
        }
    }

    #[derive(Debug)]
    struct DenyWorkerPolicy;

    impl ProxyPolicy for DenyWorkerPolicy {
        fn decide(&self, request: &ProxyPolicyRequest<'_>) -> ProxyPolicyDecision {
            assert_eq!(request.caller.agent_id, "registered-worker");
            assert_eq!(request.caller.role, cas_types::AgentRole::Worker);
            assert_eq!(request.caller.session_id, "registered-worker");
            assert_eq!(request.caller.factory_session.as_deref(), Some("factory-1"));
            assert_eq!(request.caller.active_task_ids, ["cas-8750"]);
            assert_eq!(
                request.arguments.as_ref().unwrap()["resource"],
                "customer-42"
            );
            ProxyPolicyDecision::Deny {
                reason: "resource lease is held by another worker".to_string(),
            }
        }
    }

    fn registered_worker_caller() -> ProxyCaller {
        ProxyCaller {
            agent_id: "registered-worker".to_string(),
            role: cas_types::AgentRole::Worker,
            session_id: "registered-worker".to_string(),
            factory_session: Some("factory-1".to_string()),
            active_task_ids: vec!["cas-8750".to_string()],
        }
    }

    #[test]
    fn parses_external_mcp_tool_names_into_server_and_tool_components() {
        let viktor = ExternalToolRoute::parse_mcp_tool_name("mcp__viktor__ask_viktor")
            .expect("Viktor tool name should parse");
        assert_eq!(viktor.server(), "viktor");
        assert_eq!(viktor.tool(), "ask_viktor");

        let tool_with_separator =
            ExternalToolRoute::parse_mcp_tool_name("mcp__foreign__read__metadata")
                .expect("tool names may contain a later separator");
        assert_eq!(tool_with_separator.server(), "foreign");
        assert_eq!(tool_with_separator.tool(), "read__metadata");

        assert!(ExternalToolRoute::parse_mcp_tool_name("mcp__viktor__").is_none());
        assert!(ExternalToolRoute::parse_mcp_tool_name("viktor__ask_viktor").is_none());
    }

    #[test]
    fn external_tool_allowlist_requires_exact_parsed_server_and_tool() {
        let policy =
            ExternalToolAllowlistPolicy::new([ExternalToolRoute::new("viktor", "ask_viktor")]);
        let caller = registered_worker_caller();
        let arguments = None;

        let decision = |server, tool| {
            policy.decide(&ProxyPolicyRequest {
                caller: &caller,
                server,
                tool,
                arguments: &arguments,
                dispatch_kind: ProxyDispatchKind::Direct,
            })
        };

        assert_eq!(decision("viktor", "ask_viktor"), ProxyPolicyDecision::Allow);
        for (server, tool) in [
            // Shares the trusted server name as a substring but is not it.
            ("viktor-shadow", "ask_viktor"),
            // Shares the trusted tool name as a prefix but is not it.
            ("viktor", "ask_viktor_with_full_context"),
            // A wholly foreign external tool is not implicitly permitted.
            ("foreign", "read_file"),
        ] {
            assert_eq!(
                decision(server, tool),
                ProxyPolicyDecision::Deny {
                    reason: ExternalToolAllowlistPolicy::denial_reason(server, tool)
                },
                "allowlist must compare parsed components exactly for {server}.{tool}"
            );
        }
    }

    #[test]
    fn external_tool_allowlist_normalizes_aliases_and_supports_server_wildcards() {
        let routes = ["neon.run_sql", "neon:write", "neon/read", "run_sql", "github.*"]
        .into_iter()
        .map(ExternalToolRoute::parse_allowlist_entry)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        let policy = ExternalToolAllowlistPolicy::new(routes);

        assert!(policy.allows("neon", "run_sql"));
        assert!(policy.allows("neon", "write"));
        assert!(policy.allows("neon", "read"));
        assert!(policy.allows("other", "run_sql"));
        assert!(policy.allows("github", "list_issues"));
        assert!(!policy.allows("github-shadow", "list_issues"));
        assert!(!policy.allows("neon", "run_sql_with_full_context"));
    }

    #[test]
    fn external_tool_allowlist_denial_names_canonical_entry_to_add() {
        let policy = ExternalToolAllowlistPolicy::default();
        let caller = registered_worker_caller();
        let arguments = None;
        assert_eq!(
            policy.decide(&ProxyPolicyRequest {
                caller: &caller,
                server: "neon",
                tool: "run_sql",
                arguments: &arguments,
                dispatch_kind: ProxyDispatchKind::Direct,
            }),
            ProxyPolicyDecision::Deny {
                reason: "external tool is not explicitly allowlisted; add \"neon.run_sql\" to [proxy].allowlist".to_string(),
            }
        );
    }

    #[test]
    fn external_tool_allowlist_catalog_decision_marks_denied_tools() {
        let policy = ExternalToolAllowlistPolicy::default();
        assert_eq!(
            policy.catalog_decision("neon", "run_sql"),
            ProxyPolicyDecision::Deny {
                reason: "external tool is not explicitly allowlisted; add \"neon.run_sql\" to [proxy].allowlist".to_string(),
            }
        );
    }

    #[test]
    fn delegation_routes_require_internal_supervisor_dispatch_kind() {
        let policy = ExternalToolAllowlistPolicy::new([
            ExternalToolRoute::new("viktor", "ask_viktor"),
            ExternalToolRoute::new("github", "list_issues"),
        ])
        .with_supervisor_delegation_routes([ExternalToolRoute::new("viktor", "ask_viktor")]);
        let arguments = None;
        let mut caller = registered_worker_caller();
        let decide = |caller: &ProxyCaller, dispatch_kind, server, tool| {
            policy.decide(&ProxyPolicyRequest {
                caller,
                server,
                tool,
                arguments: &arguments,
                dispatch_kind,
            })
        };

        assert_eq!(
            decide(&caller, ProxyDispatchKind::Direct, "viktor", "ask_viktor"),
            ProxyPolicyDecision::Deny {
                reason: "external delegation route requires the registered supervisor gateway"
                    .to_string()
            }
        );
        caller.role = cas_types::AgentRole::Supervisor;
        assert_eq!(
            decide(
                &caller,
                ProxyDispatchKind::ExternalProductionVerification,
                "viktor",
                "ask_viktor"
            ),
            ProxyPolicyDecision::Allow
        );
        assert_eq!(
            decide(&caller, ProxyDispatchKind::Direct, "github", "list_issues"),
            ProxyPolicyDecision::Allow,
            "ordinary allowlisted routes remain available through generic execution"
        );
    }

    #[tokio::test]
    async fn policy_denial_is_audited_before_any_upstream_routing() {
        let engine = ProxyEngine::from_configs(HashMap::new()).await.unwrap();
        engine.set_policy(Arc::new(DenyWorkerPolicy)).await;

        let error = match engine
            .execute(
                &registered_worker_caller(),
                r#"{"server":"unconfigured","tool":"send","args":{"resource":"customer-42"}}"#,
                None,
            )
            .await
        {
            Ok(_) => panic!("denied policy call must not reach upstream routing"),
            Err(error) => error,
        };

        assert_eq!(
            error.to_string(),
            "proxy policy denied tool 'send' on 'unconfigured': resource lease is held by another worker"
        );
        let audit = engine.policy_audit();
        assert_eq!(audit.len(), 1);
        assert!(!audit[0].allowed);
        assert_eq!(audit[0].caller, registered_worker_caller());
        assert_eq!(audit[0].server, "unconfigured");
        assert_eq!(audit[0].tool, "send");
        assert_eq!(
            audit[0].reason.as_deref(),
            Some("resource lease is held by another worker")
        );
    }

    #[tokio::test]
    async fn external_allowlist_denies_lookalike_server_before_upstream_routing() {
        let engine = ProxyEngine::from_configs(HashMap::new()).await.unwrap();
        engine
            .set_policy(Arc::new(ExternalToolAllowlistPolicy::new([
                ExternalToolRoute::new("viktor", "ask_viktor"),
            ])))
            .await;

        let result = engine
            .execute(
                &registered_worker_caller(),
                r#"{"server":"viktor-shadow","tool":"ask_viktor"}"#,
                None,
            )
            .await;
        let error = match result {
            Ok(_) => panic!("lookalike server must be denied before routing"),
            Err(error) => error,
        };

        assert_eq!(
            error.to_string(),
            "proxy policy denied tool 'ask_viktor' on 'viktor-shadow': external tool is not explicitly allowlisted; add \"viktor-shadow.ask_viktor\" to [proxy].allowlist"
        );
        let audit = engine.policy_audit();
        assert_eq!(audit.len(), 1);
        assert!(!audit[0].allowed);
        assert_eq!(audit[0].server, "viktor-shadow");
        assert_eq!(audit[0].tool, "ask_viktor");
    }

    #[tokio::test]
    async fn disconnected_configured_viktor_is_loud_in_search_and_execute() {
        let config = ServerConfig::Http {
            url: "https://example.invalid/mcp".to_string(),
            auth: Some("env:CAS_TEST_MISSING_VIKTOR_KEY_8563".to_string()),
            headers: HashMap::new(),
            oauth: false,
        };
        let engine = ProxyEngine::from_configs(HashMap::from([("viktor".to_string(), config)]))
            .await
            .unwrap();
        engine
            .set_policy(Arc::new(ExternalToolAllowlistPolicy::new([
                ExternalToolRoute::new("viktor", "ask_viktor"),
            ])))
            .await;

        let search = engine.search("server:viktor", None).await.unwrap_err();
        assert!(
            search.to_string().contains("upstream 'viktor' is absent"),
            "configured but disconnected discovery must not read as an empty catalog: {search}"
        );
        let execute = match engine
            .execute(
                &registered_worker_caller(),
                r#"{"server":"viktor","tool":"ask_viktor","args":{}}"#,
                None,
            )
            .await
        {
            Ok(_) => panic!("disconnected Viktor execution must fail loudly"),
            Err(error) => error,
        };
        assert!(
            execute.to_string().contains("upstream 'viktor' is absent"),
            "configured but disconnected execution must identify the absent upstream: {execute}"
        );
        assert!(!engine.upstream_connected("viktor").await);
    }

    fn hanging_http_upstream(hold: Duration) -> (ServerConfig, std::thread::JoinHandle<()>) {
        install_test_crypto_provider();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            if let Ok((_stream, _peer)) = listener.accept() {
                std::thread::sleep(hold);
            }
        });
        (
            ServerConfig::Http {
                url: format!("http://{address}/mcp"),
                auth: None,
                headers: HashMap::new(),
                oauth: false,
            },
            server,
        )
    }

    #[tokio::test]
    async fn invalid_stdio_command_fails_start_and_reload_before_mutation() {
        let invalid = HashMap::from([(
            "unsafe/server".to_string(),
            ServerConfig::Stdio {
                command: "   ".to_string(),
                args: vec!["--token=secret".to_string()],
                env: HashMap::from([("TOKEN".to_string(), "secret".to_string())]),
            },
        )]);
        assert!(ProxyEngine::from_configs(invalid.clone()).await.is_err());

        let engine = ProxyEngine::from_configs(HashMap::new()).await.unwrap();
        assert!(engine.reload(invalid).await.is_err());
        assert_eq!(engine.tool_count().await, 0);
        assert!(engine.health_snapshot().await.servers.is_empty());
    }

    #[test]
    fn parse_query_plain_keywords() {
        let (server, kw) = parse_search_query("screenshot capture");
        assert!(server.is_none());
        assert_eq!(kw, vec!["screenshot", "capture"]);
    }

    #[test]
    fn parse_query_with_server_filter() {
        let (server, kw) = parse_search_query("server:github issue create");
        assert_eq!(server, Some("github".to_string()));
        assert_eq!(kw, vec!["issue", "create"]);
    }

    #[test]
    fn parse_query_empty() {
        let (server, kw) = parse_search_query("");
        assert!(server.is_none());
        assert!(kw.is_empty());
    }

    #[test]
    fn ambiguous_public_routing_fails_closed_without_candidate_names() {
        let public_names = std::collections::BTreeMap::from([
            ("https://secret-one.invalid".to_string(), "upstream-same".to_string()),
            ("/home/operator/secret-two".to_string(), "upstream-same".to_string()),
        ]);
        let error = resolve_routing_name("upstream-same", &public_names, "server").unwrap_err();
        assert_eq!(error.to_string(), "ambiguous public server identity");
        assert!(!error.to_string().contains("secret"));
        assert!(!error.to_string().contains("/home"));
    }

    #[test]
    fn matches_keywords_empty_matches_all() {
        let tool = make_tool("anything", "some description");
        assert!(matches_keywords(&tool, &[]));
    }

    #[test]
    fn matches_keywords_name_match() {
        let tool = make_tool("take_screenshot", "Captures a screenshot");
        let keywords = vec!["screenshot".to_string()];
        assert!(matches_keywords(&tool, &keywords));
    }

    #[test]
    fn matches_keywords_description_match() {
        let tool = make_tool("capture", "Takes a screenshot of the page");
        let keywords = vec!["screenshot".to_string()];
        assert!(matches_keywords(&tool, &keywords));
    }

    #[test]
    fn matches_keywords_case_insensitive() {
        let tool = make_tool("TakeScreenshot", "CAPTURES A SCREENSHOT");
        let keywords = vec!["screenshot".to_string()];
        assert!(matches_keywords(&tool, &keywords));
    }

    #[test]
    fn matches_keywords_all_must_match() {
        let tool = make_tool("create_issue", "Create a GitHub issue");
        let keywords = vec!["create".to_string(), "issue".to_string()];
        assert!(matches_keywords(&tool, &keywords));

        let keywords_no_match = vec!["create".to_string(), "screenshot".to_string()];
        assert!(!matches_keywords(&tool, &keywords_no_match));
    }

    #[test]
    fn matches_keywords_no_match() {
        let tool = make_tool("list_files", "List files in a directory");
        let keywords = vec!["screenshot".to_string()];
        assert!(!matches_keywords(&tool, &keywords));
    }

    // ── Dispatch parsing tests ──────────────────────────────────────

    #[test]
    fn parse_dispatch_json_single() {
        let calls = parse_dispatch(
            r#"{"server": "github", "tool": "list_issues", "args": {"repo": "myorg/app"}}"#,
        )
        .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "github");
        assert_eq!(calls[0].tool, "list_issues");
        assert!(calls[0].args.is_some());
        assert_eq!(calls[0].args.as_ref().unwrap()["repo"], "myorg/app");
    }

    #[test]
    fn parse_dispatch_json_batch() {
        let calls = parse_dispatch(
            r#"[
                {"server": "github", "tool": "list_issues", "args": {"repo": "app"}},
                {"server": "sentry", "tool": "list_errors", "args": {"project": "be"}}
            ]"#,
        )
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].server, "github");
        assert_eq!(calls[1].server, "sentry");
    }

    #[test]
    fn parse_dispatch_dot_syntax_with_args() {
        let calls = parse_dispatch(r#"github.list_issues({"repo": "myorg/app"})"#).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "github");
        assert_eq!(calls[0].tool, "list_issues");
        assert!(calls[0].args.is_some());
    }

    #[test]
    fn parse_dispatch_dot_syntax_no_args() {
        let calls = parse_dispatch("github.list_repos()").unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "github");
        assert_eq!(calls[0].tool, "list_repos");
        assert!(calls[0].args.is_none());
    }

    #[test]
    fn parse_dispatch_dot_syntax_no_parens() {
        let calls = parse_dispatch("github.list_repos").unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "github");
        assert_eq!(calls[0].tool, "list_repos");
        assert!(calls[0].args.is_none());
    }

    #[test]
    fn parse_dispatch_invalid_no_dot() {
        let result = parse_dispatch("just_a_word");
        assert!(result.is_err());
    }

    #[test]
    fn parse_dispatch_json_no_args() {
        let calls = parse_dispatch(r#"{"server": "github", "tool": "list_repos"}"#).unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].args.is_none());
    }

    /// Regression for cas-a62f: the MCP proxy must accept https:// upstream URLs.
    ///
    /// Before the fix, rmcp's `transport-streamable-http-client-reqwest` feature pulled in
    /// `reqwest` with `default-features = false` and no TLS backend, so any https upstream
    /// (Vercel, Context7, GitHub Copilot, …) failed immediately with
    /// `invalid URL, scheme is not http`. Enabling rmcp's `reqwest-tls-no-provider` feature adds
    /// rustls transport support while leaving CAS's process-wide ring provider in control, and
    /// lets the transport actually attempt the TLS handshake.
    ///
    /// This test points at an unreachable local port so no network is required. It passes as
    /// long as the error path is a *connection* failure rather than reqwest's scheme
    /// rejection — i.e. TLS support is compiled in.
    #[tokio::test(flavor = "current_thread")]
    async fn https_upstream_is_not_rejected_by_scheme() {
        install_test_crypto_provider();
        let mut configs = HashMap::new();
        configs.insert(
            "https-regression".to_string(),
            ServerConfig::Http {
                url: "https://127.0.0.1:1/mcp".to_string(),
                auth: None,
                headers: Default::default(),
                oauth: false,
            },
        );

        // from_configs swallows per-server errors and logs to stderr, so drive
        // connect_server directly to inspect the error.
        let config = configs.remove("https-regression").unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            connect_server("https-regression", &config),
        )
        .await
        .expect("connect attempt should not hang");

        let err = match result {
            Ok(_) => panic!("connect to 127.0.0.1:1 unexpectedly succeeded"),
            Err(e) => e,
        };
        let msg = format!("{err:#}");
        assert!(
            !msg.contains("scheme is not http"),
            "https upstream was rejected by reqwest scheme check — rmcp TLS feature is missing. \
             error chain: {msg}"
        );
    }

    #[test]
    fn imported_http_headers_and_literal_or_env_auth_are_forwarded_as_raw_bearer_tokens() {
        let config = http_transport_config(
            "https://example.invalid/mcp",
            Some("literal-token"),
            &HashMap::from([("X-Api-Key".to_string(), "literal-key".to_string())]),
        )
        .expect("valid HTTP config");

        assert_eq!(config.auth_header.as_deref(), Some("literal-token"));
        assert_eq!(
            config
                .custom_headers
                .get(&http::header::HeaderName::from_static("x-api-key"))
                .and_then(|value| value.to_str().ok()),
            Some("literal-key")
        );

        let env_name = format!(
            "CAS_PROXY_AUTH_TEST_{}_{}",
            std::process::id(),
            SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let env_secret = "env-backed-token";
        unsafe { std::env::set_var(&env_name, env_secret) };
        let env_config = http_transport_config(
            "https://example.invalid/mcp",
            Some(&format!("env:{env_name}")),
            &HashMap::new(),
        )
        .expect("valid env-backed auth");
        unsafe { std::env::remove_var(&env_name) };

        assert_eq!(
            env_config.auth_header.as_deref(),
            Some(env_secret)
        );
        let health = serde_json::to_string(&ProxyHealthSnapshot {
            session_id: "redaction-test".to_string(),
            generated_at_ms: 0,
            healthy: 0,
            degraded: 0,
            servers: Vec::new(),
        })
        .unwrap();
        assert!(!health.contains(env_secret));
        assert!(!health.contains(&env_name));
    }

    #[test]
    fn missing_imported_credential_is_fail_closed_and_redacted() {
        let missing = format!(
            "CAS_PROXY_MISSING_{}_{}",
            std::process::id(),
            SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let error = resolve_credential(&format!("env:{missing}"))
            .expect_err("missing environment credential must fail");
        let message = format!("{error:#}");
        assert!(
            !message.contains(&missing),
            "environment key must be redacted"
        );
        assert!(
            !message.contains("env:"),
            "credential reference must be redacted"
        );
    }

    #[tokio::test]
    async fn health_names_missing_credential_variable_without_exposing_value() {
        let missing = format!(
            "CAS_PROXY_MISSING_HEALTH_{}_{}",
            std::process::id(),
            SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let secret = "must-never-appear-in-health";
        let config = ServerConfig::Http {
            url: "https://example.invalid/mcp".to_string(),
            auth: Some(format!("env:{missing}")),
            headers: HashMap::new(),
            oauth: false,
        };
        let engine = ProxyEngine::from_configs(HashMap::from([("mecha-cassy".to_string(), config)]))
            .await
            .unwrap();
        let snapshot = engine.health_snapshot().await;
        let server = snapshot
            .servers
            .iter()
            .find(|server| server.name == "mecha-cassy")
            .expect("configured upstream health must be present");
        let expected_code = format!("{MISSING_CREDENTIAL_ENV_PREFIX}{missing}");
        assert_eq!(server.last_error_code.as_deref(), Some(expected_code.as_str()));
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains(&missing));
        assert!(!json.contains(secret));
        assert!(!json.contains("connection_failed"));
    }

    #[test]
    fn health_backoff_visibility_and_recovery_are_bounded() {
        let config = ServerConfig::Http {
            url: "https://example.invalid/mcp".to_string(),
            auth: None,
            headers: HashMap::new(),
            oauth: false,
        };
        let mut health = initial_health("optional", &config);

        assert_eq!(
            record_failure(&mut health, "authentication_required", 1_000),
            FailureVisibility::Error
        );
        assert_eq!(health.next_retry_at_ms, Some(6_000));
        assert_eq!(
            record_failure(&mut health, "authentication_required", 6_000),
            FailureVisibility::Debug
        );
        assert_eq!(health.next_retry_at_ms, Some(16_000));

        for attempt in 0..20 {
            record_failure(&mut health, "connection_failed", 20_000 + attempt * 1_000);
        }
        assert!(
            health.next_retry_at_ms.unwrap() <= 20_000 + 19_000 + 300_000,
            "backoff must cap at five minutes"
        );

        record_success(&mut health, 7, 400_000);
        assert_eq!(health.state, UpstreamState::Healthy);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.tool_count, 7);
        assert_eq!(health.next_retry_at_ms, None);
        assert_eq!(health.last_error_code, None);
    }

    #[test]
    fn upstream_failures_are_classified_without_exposing_content() {
        assert_eq!(
            classify_error(&anyhow::anyhow!("AuthRequired: bearer token omitted")),
            "authentication_required"
        );
        assert_eq!(
            classify_error(&anyhow::anyhow!(
                "UnexpectedContentType: received text/plain body=private"
            )),
            "unexpected_content_type"
        );
        assert_eq!(
            classify_live_failure(&rmcp::service::ServiceError::McpError(
                rmcp::ErrorData::invalid_params("normal tool failure", None)
            )),
            None,
            "application-level MCP errors are not connection failures"
        );
        assert_eq!(
            classify_live_failure(&rmcp::service::ServiceError::TransportClosed),
            Some("connection_failed")
        );
    }

    #[test]
    fn unsafe_health_fields_are_cryptographically_pseudonymized_and_allowlisted() {
        let first_raw = "https://user:token@example.invalid/private";
        let second_raw = "/home/operator/.config/secret-token";
        let snapshot = ProxyHealthSnapshot {
            session_id: "https://token@example.invalid/session".to_string(),
            generated_at_ms: 1,
            healthy: 0,
            degraded: 2,
            servers: vec![
                UpstreamHealth {
                    name: first_raw.to_string(),
                    transport: "Bearer private".to_string(),
                    state: UpstreamState::Backoff,
                    executable: None,
                    attempts: 1,
                    consecutive_failures: 1,
                    tool_count: 0,
                    last_error_code: Some("token=private\ncontrol".to_string()),
                    last_attempt_at_ms: Some(1),
                    next_retry_at_ms: Some(5_001),
                },
                UpstreamHealth {
                    name: second_raw.to_string(),
                    transport: "http".to_string(),
                    state: UpstreamState::Backoff,
                    executable: None,
                    attempts: 1,
                    consecutive_failures: 1,
                    tool_count: 0,
                    last_error_code: Some("timeout".to_string()),
                    last_attempt_at_ms: Some(1),
                    next_retry_at_ms: Some(5_001),
                },
            ],
        }
        .sanitized();

        assert_ne!(snapshot.servers[0].name, snapshot.servers[1].name);
        assert_eq!(snapshot.session_id, "proxy-unknown");
        for server in &snapshot.servers {
            assert!(server.name.starts_with("upstream-"));
            assert_eq!(server.name.len(), "upstream-".len() + 32);
        }
        assert_eq!(snapshot.servers[0].transport, "unknown");
        assert_eq!(
            snapshot.servers[0].last_error_code.as_deref(),
            Some("unknown")
        );
        assert_eq!(snapshot.servers[1].transport, "http");
        assert_eq!(
            snapshot.servers[1].last_error_code.as_deref(),
            Some("timeout")
        );
        assert_eq!(
            snapshot.clone().sanitized(),
            snapshot,
            "sanitization must be idempotent for cached snapshots"
        );
        let json = serde_json::to_string(&snapshot).unwrap();
        for forbidden in [
            first_raw,
            second_raw,
            "Bearer private",
            "token=private",
            "control",
            "https://token@example.invalid/session",
        ] {
            assert!(!json.contains(forbidden), "{forbidden:?} leaked: {json}");
        }
    }

    #[test]
    fn health_projection_preserves_safe_names_and_disambiguates_forged_collisions() {
        let unsafe_name = "https://token@example.invalid/private";
        let colliding_name = public_upstream_id(unsafe_name);
        let snapshot = ProxyHealthSnapshot {
            session_id: "proxy-1-2-3".to_string(),
            generated_at_ms: 1,
            healthy: 3,
            degraded: 0,
            servers: ["github", unsafe_name, colliding_name.as_str()]
                .into_iter()
                .map(|name| UpstreamHealth {
                    name: name.to_string(),
                    transport: "http".to_string(),
                    state: UpstreamState::Healthy,
                    executable: None,
                    attempts: 1,
                    consecutive_failures: 0,
                    tool_count: 1,
                    last_error_code: None,
                    last_attempt_at_ms: Some(1),
                    next_retry_at_ms: None,
                })
                .collect(),
        }
        .sanitized();

        assert_eq!(snapshot.servers[0].name, "github");
        assert_ne!(snapshot.servers[1].name, snapshot.servers[2].name);
        assert!(
            snapshot.servers[1]
                .name
                .starts_with("upstream-disambiguated-")
        );
        assert!(
            snapshot.servers[2]
                .name
                .starts_with("upstream-disambiguated-")
        );
        assert_eq!(snapshot.clone().sanitized(), snapshot);
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains(unsafe_name));
    }

    #[test]
    fn live_failure_ordering_respects_generation_and_recorded_success() {
        assert!(
            live_failure_applies(7, 7, 10, 11),
            "same-generation failure after the last success may degrade"
        );
        assert!(
            !live_failure_applies(7, 7, 12, 11),
            "success recorded before transition lock must suppress stale failure"
        );
        assert!(
            !live_failure_applies(8, 7, 0, 11),
            "completion from a removed generation cannot affect its replacement"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_stdio_executable_is_terminal_and_visible_without_retry() {
        let config = ServerConfig::Stdio {
            command: "cas-command-that-does-not-exist-for-proxy-test".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
        };
        let engine = ProxyEngine::from_configs(HashMap::from([("optional".to_string(), config)]))
            .await
            .unwrap();

        let first = engine.health_snapshot().await.servers.remove(0);
        assert_eq!(first.attempts, 1);
        assert_eq!(first.consecutive_failures, 1);
        assert_eq!(first.state, UpstreamState::ExecutableMissing);
        assert_eq!(first.last_error_code.as_deref(), Some("executable_missing"));
        assert_eq!(first.next_retry_at_ms, None);
        assert_eq!(first.executable.as_deref(), Some("cas-command-that-does-not-exist-for-proxy-test"));
        assert_eq!(engine.retry_unhealthy().await, 0);
        assert_eq!(engine.health_snapshot().await.servers[0].attempts, 1);
    }

    #[test]
    fn health_sanitization_only_exposes_missing_stdio_executables() {
        let snapshot = ProxyHealthSnapshot {
            session_id: "proxy-test".to_string(),
            generated_at_ms: 1,
            healthy: 1,
            degraded: 2,
            servers: vec![
                UpstreamHealth {
                    name: "missing-stdio".to_string(),
                    transport: "stdio".to_string(),
                    state: UpstreamState::ExecutableMissing,
                    executable: Some("/opt/stale/mcp-server".to_string()),
                    attempts: 1,
                    consecutive_failures: 1,
                    tool_count: 0,
                    last_error_code: Some("executable_missing".to_string()),
                    last_attempt_at_ms: Some(1),
                    next_retry_at_ms: None,
                },
                UpstreamHealth {
                    name: "forged-http".to_string(),
                    transport: "http".to_string(),
                    state: UpstreamState::ExecutableMissing,
                    executable: Some("/should/not/escape".to_string()),
                    attempts: 1,
                    consecutive_failures: 1,
                    tool_count: 0,
                    last_error_code: Some("executable_missing".to_string()),
                    last_attempt_at_ms: Some(1),
                    next_retry_at_ms: None,
                },
                UpstreamHealth {
                    name: "healthy-stdio".to_string(),
                    transport: "stdio".to_string(),
                    state: UpstreamState::Healthy,
                    executable: Some("/should/not/escape".to_string()),
                    attempts: 1,
                    consecutive_failures: 0,
                    tool_count: 1,
                    last_error_code: None,
                    last_attempt_at_ms: Some(1),
                    next_retry_at_ms: None,
                },
            ],
        }
        .sanitized();

        assert_eq!(
            snapshot.servers[0].executable.as_deref(),
            Some("/opt/stale/mcp-server")
        );
        assert_eq!(snapshot.servers[1].executable, None);
        assert_eq!(snapshot.servers[2].executable, None);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_stdio_executable_checks_relative_paths_and_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("nested").join("stdio-server");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let path_with_parent = executable
            .parent()
            .unwrap()
            .join("..")
            .join("nested")
            .join("stdio-server");
        assert_eq!(
            resolve_stdio_executable(&path_with_parent.to_string_lossy()),
            Some(path_with_parent)
        );

        let non_executable = temp.path().join("not-executable");
        std::fs::write(&non_executable, "not executable").unwrap();
        assert!(resolve_stdio_executable(&non_executable.to_string_lossy()).is_none());

        let stale_interpreter = temp.path().join("stale-interpreter");
        std::fs::write(&stale_interpreter, "#!/missing/interpreter\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(&stale_interpreter).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&stale_interpreter, permissions).unwrap();
        assert!(resolve_stdio_executable(&stale_interpreter.to_string_lossy()).is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_optional_upstreams_share_one_bounded_startup_window() {
        // Wide margins on purpose: with a 400ms per-upstream timeout, serial
        // startup would take >=800ms while concurrent startup finishes near
        // 400ms, so the 700ms assertion below keeps ~300ms of scheduler slack
        // on both sides. The previous 75ms/130ms constants left only ~55ms of
        // slack and flaked on saturated CI runners (runs 32070409371 and
        // 32075974620, 2026-08-17).
        let hold = Duration::from_millis(1_000);
        let timeout = Duration::from_millis(400);
        let (first, first_server) = hanging_http_upstream(hold);
        let (second, second_server) = hanging_http_upstream(hold);
        let started = std::time::Instant::now();

        let engine = ProxyEngine::from_configs_with_timeout(
            HashMap::from([("first".to_string(), first), ("second".to_string(), second)]),
            timeout,
        )
        .await
        .unwrap();
        let elapsed = started.elapsed();
        let snapshot = engine.health_snapshot().await;

        assert!(
            elapsed < Duration::from_millis(700),
            "two optional upstreams must time out concurrently, not serially: {elapsed:?}"
        );
        assert_eq!(snapshot.degraded, 2);
        assert!(
            snapshot
                .servers
                .iter()
                .all(|server| server.last_error_code.as_deref() == Some("timeout"))
        );

        first_server.join().unwrap();
        second_server.join().unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn health_is_isolated_per_proxy_session() {
        let config = ServerConfig::Stdio {
            command: "cas-command-that-does-not-exist-for-session-test".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
        };
        let first = ProxyEngine::from_configs(HashMap::from([("optional".to_string(), config)]))
            .await
            .unwrap();
        let second = ProxyEngine::from_configs(HashMap::new()).await.unwrap();
        let first = first.health_snapshot().await;
        let second = second.health_snapshot().await;
        assert_ne!(first.session_id, second.session_id);
        assert_eq!(first.servers[0].attempts, 1);
        assert!(second.servers.is_empty());
    }

    #[test]
    fn health_snapshot_serialization_contains_no_config_or_secret_fields() {
        let snapshot = ProxyHealthSnapshot {
            session_id: "proxy-test".to_string(),
            generated_at_ms: 1,
            healthy: 0,
            degraded: 1,
            servers: vec![UpstreamHealth {
                name: "github".to_string(),
                transport: "http".to_string(),
                state: UpstreamState::Backoff,
                executable: None,
                attempts: 1,
                consecutive_failures: 1,
                tool_count: 0,
                last_error_code: Some("authentication_required".to_string()),
                last_attempt_at_ms: Some(1),
                next_retry_at_ms: Some(5_001),
            }],
        };
        let json = serde_json::to_value(snapshot).unwrap();
        let server = json["servers"][0].as_object().unwrap();
        assert!(!server.contains_key("url"));
        assert!(!server.contains_key("auth"));
        assert!(!server.contains_key("headers"));
        assert!(!server.contains_key("content"));
    }
}
