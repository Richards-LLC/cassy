pub mod config;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rmcp::model::Tool;
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
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
        for server in &mut self.servers {
            server.name = safe_upstream_id(&server.name);
            server.transport = safe_transport(&server.transport).to_string();
            server.last_error_code = server
                .last_error_code
                .as_deref()
                .map(safe_error_code)
                .map(str::to_string);
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
}

impl ProxyEngine {
    /// Create a proxy engine by connecting to all configured upstream servers.
    ///
    /// Connection failures are logged and skipped — the engine starts with
    /// whatever servers connected successfully.
    pub async fn from_configs(configs: HashMap<String, ServerConfig>) -> Result<Self> {
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
                    upstream = %safe_upstream_id(name),
                    tool_count,
                    proxy_session = %self.session_id,
                    "MCP upstream connected"
                );
            }
            Err(error) => {
                let code = classify_error(&error);
                let visibility = {
                    let mut health = self.health.write().await;
                    let record = health
                        .entry(name.to_string())
                        .or_insert_with(|| initial_health(name, config));
                    record_failure(record, code, now)
                };
                match visibility {
                    FailureVisibility::Error => tracing::error!(
                        upstream = %safe_upstream_id(name),
                        error_code = code,
                        proxy_session = %self.session_id,
                        "Optional MCP upstream unavailable; CAS will continue and retry"
                    ),
                    FailureVisibility::Debug => tracing::debug!(
                        upstream = %safe_upstream_id(name),
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

        // Parse optional server: prefix
        let (server_filter, keywords) = parse_search_query(query);

        let mut results: Vec<SearchResult> = Vec::new();

        for (server_name, connected) in servers.iter() {
            // Apply server filter if present
            if let Some(ref filter) = server_filter {
                if !server_name.to_lowercase().contains(&filter.to_lowercase()) {
                    continue;
                }
            }

            for tool in &connected.tools {
                if matches_keywords(tool, &keywords) {
                    results.push(SearchResult {
                        server: server_name.clone(),
                        name: tool.name.to_string(),
                        description: tool.description.as_ref().map(|d| d.to_string()),
                        input_schema: serde_json::to_value(&*tool.input_schema).unwrap_or_default(),
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
    pub async fn execute(&self, code: &str, max_length: Option<usize>) -> Result<ExecuteResult> {
        let calls = parse_dispatch(code)?;

        let mut text_parts: Vec<String> = Vec::new();
        let mut images: Vec<ImageResult> = Vec::new();

        if calls.len() == 1 {
            let call = &calls[0];
            let result = self
                .call_tool_raw(&call.server, &call.tool, call.args.clone())
                .await?;
            collect_result(&result, &mut text_parts, &mut images);
        } else {
            // Execute in parallel
            let futures: Vec<_> = calls
                .iter()
                .map(|call| self.call_tool_raw(&call.server, &call.tool, call.args.clone()))
                .collect();

            let results = futures::future::join_all(futures).await;

            for (i, result) in results.into_iter().enumerate() {
                match result {
                    Ok(result) => collect_result(&result, &mut text_parts, &mut images),
                    Err(e) => {
                        text_parts.push(format!(
                            "[{}.{} error]: {e}",
                            calls[i].server, calls[i].tool
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

    /// Return catalog entries grouped by server name.
    pub async fn catalog_entries_by_server(&self) -> HashMap<String, Vec<CatalogEntry>> {
        let servers = self.servers.read().await;
        servers
            .iter()
            .map(|(name, connected)| {
                let entries = connected
                    .tools
                    .iter()
                    .map(|tool| CatalogEntry {
                        name: tool.name.to_string(),
                        description: tool.description.as_ref().map(|d| d.to_string()),
                        input_schema: serde_json::to_value(&*tool.input_schema).unwrap_or_default(),
                    })
                    .collect();
                (name.clone(), entries)
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
        let mut servers = self.servers.write().await;
        let old_configs = self.configs.read().await.clone();

        // Remove servers no longer in config
        let current_names: Vec<String> = servers.keys().cloned().collect();
        for name in &current_names {
            if !configs.contains_key(name) {
                if let Some(removed) = servers.remove(name) {
                    let _ = removed.service.cancel().await;
                    tracing::info!(
                        upstream = %safe_upstream_id(name),
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
                        upstream = %safe_upstream_id(name),
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
        server_name: &str,
        tool_name: &str,
        arguments: Option<serde_json::Map<String, Value>>,
    ) -> Result<Value> {
        use rmcp::model::CallToolRequestParams;

        let result = self
            .call_upstream(
                server_name,
                CallToolRequestParams {
                    name: tool_name.to_string().into(),
                    arguments,
                    meta: None,
                    task: None,
                },
            )
            .await
            .with_context(|| {
                format!(
                    "tool call '{tool_name}' on '{}' failed",
                    safe_upstream_id(server_name)
                )
            })?;

        serde_json::to_value(result).context("failed to serialize tool result")
    }

    /// Call a tool and return the raw rmcp result (for internal use by execute).
    async fn call_tool_raw(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Option<serde_json::Map<String, Value>>,
    ) -> Result<rmcp::model::CallToolResult> {
        use rmcp::model::CallToolRequestParams;

        self.call_upstream(
            server_name,
            CallToolRequestParams {
                name: tool_name.to_string().into(),
                arguments,
                meta: None,
                task: None,
            },
        )
        .await
        .with_context(|| {
            format!(
                "tool call '{tool_name}' on '{}' failed",
                safe_upstream_id(server_name)
            )
        })
    }

    async fn call_upstream(
        &self,
        server_name: &str,
        request: rmcp::model::CallToolRequestParams,
    ) -> Result<rmcp::model::CallToolResult> {
        let servers = self.servers.read().await;
        let server = servers.get(server_name).with_context(|| {
            let mut available: Vec<String> =
                servers.keys().map(|name| safe_upstream_id(name)).collect();
            available.sort();
            format!(
                "server '{}' not connected. Available: {}",
                safe_upstream_id(server_name),
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            )
        })?;
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
                    self.record_live_failure(server_name, generation, completion, code)
                        .await;
                }
                Err(error.into())
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
                upstream = %safe_upstream_id(server_name),
                error_code = code,
                proxy_session = %self.session_id,
                "Optional MCP upstream connection failed after startup; retry scheduled"
            ),
            FailureVisibility::Debug => tracing::debug!(
                upstream = %safe_upstream_id(server_name),
                error_code = code,
                proxy_session = %self.session_id,
                "Optional MCP upstream connection failure already recorded"
            ),
        }
    }

    /// Gracefully shut down all connected servers.
    pub async fn shutdown(&self) {
        let mut servers = self.servers.write().await;
        for (name, server) in servers.drain() {
            if let Err(e) = server.service.cancel().await {
                eprintln!(
                    "[proxy] Error shutting down '{}': {e}",
                    safe_upstream_id(&name)
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
        name: safe_upstream_id(name),
        transport: transport_name(config).to_string(),
        state: UpstreamState::Degraded,
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

fn classify_error(error: &anyhow::Error) -> &'static str {
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("authrequired")
        || message.contains("auth required")
        || message.contains("unauthorized")
        || message.contains("status 401")
        || message.contains("status: 401")
    {
        "authentication_required"
    } else if message.contains("unexpectedcontenttype")
        || message.contains("unexpected content type")
    {
        "unexpected_content_type"
    } else if message.contains("invalid url") || message.contains("scheme is not http") {
        "invalid_url"
    } else if message.contains("timed out") || message.contains("timeout") {
        "timeout"
    } else {
        "connection_failed"
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

fn safe_upstream_id(name: &str) -> String {
    if is_generated_upstream_id(name) {
        return name.to_string();
    }
    let digest = Sha256::digest(name.as_bytes());
    format!("upstream-{}", hex_prefix(&digest, 16))
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

fn is_generated_upstream_id(name: &str) -> bool {
    name.strip_prefix("upstream-").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn hex_prefix(bytes: &[u8], take: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(take.saturating_mul(2));
    for byte in bytes.iter().take(take) {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn safe_transport(transport: &str) -> &'static str {
    match transport {
        "stdio" => "stdio",
        "http" => "http",
        "sse" => "sse",
        _ => "unknown",
    }
}

fn safe_error_code(code: &str) -> &'static str {
    match code {
        "authentication_required" => "authentication_required",
        "unexpected_content_type" => "unexpected_content_type",
        "invalid_url" => "invalid_url",
        "timeout" => "timeout",
        "connection_failed" => "connection_failed",
        _ => "unknown",
    }
}

fn live_failure_applies(
    installed_generation: u64,
    failed_generation: u64,
    last_successful_call: u64,
    failure_completion: u64,
) -> bool {
    installed_generation == failed_generation && last_successful_call <= failure_completion
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
        config.auth_header = Some(format!("Bearer {}", resolve_credential(auth)?));
    }
    Ok(config)
}

fn resolve_credential(value: &str) -> Result<String> {
    let env_name = value
        .strip_prefix("env:")
        .or_else(|| value.strip_prefix("${").and_then(|v| v.strip_suffix('}')));
    match env_name {
        Some(name) if !name.is_empty() => std::env::var(name)
            .context("required MCP upstream credential environment variable is unavailable"),
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

    fn hanging_http_upstream(hold: Duration) -> (ServerConfig, std::thread::JoinHandle<()>) {
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
    /// `invalid URL, scheme is not http`. Enabling rmcp's `reqwest` feature adds rustls and
    /// lets the transport actually attempt the TLS handshake.
    ///
    /// This test points at an unreachable local port so no network is required. It passes as
    /// long as the error path is a *connection* failure rather than reqwest's scheme
    /// rejection — i.e. TLS support is compiled in.
    #[tokio::test(flavor = "current_thread")]
    async fn https_upstream_is_not_rejected_by_scheme() {
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
    fn imported_http_headers_and_literal_or_env_auth_are_applied_as_bearer_tokens() {
        let config = http_transport_config(
            "https://example.invalid/mcp",
            Some("literal-token"),
            &HashMap::from([("X-Api-Key".to_string(), "literal-key".to_string())]),
        )
        .expect("valid HTTP config");

        assert_eq!(config.auth_header.as_deref(), Some("Bearer literal-token"));
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

        let expected_auth = format!("Bearer {env_secret}");
        assert_eq!(
            env_config.auth_header.as_deref(),
            Some(expected_auth.as_str())
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
            assert!(is_generated_upstream_id(&server.name));
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
    async fn retries_only_when_backoff_is_due_and_suppresses_repeated_error_visibility() {
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
        let due = first
            .next_retry_at_ms
            .expect("failed upstream must back off");

        assert_eq!(engine.retry_unhealthy_at(due - 1).await, 0);
        assert_eq!(
            engine.health_snapshot().await.servers[0].attempts,
            1,
            "retry before the deadline must be suppressed"
        );

        assert_eq!(engine.retry_unhealthy_at(due).await, 1);
        let repeated = engine.health_snapshot().await.servers.remove(0);
        assert_eq!(repeated.attempts, 2);
        assert_eq!(repeated.consecutive_failures, 2);
        assert!(
            repeated.next_retry_at_ms.unwrap() >= due + 10_000,
            "the second production-path failure must advance exponential backoff"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_optional_upstreams_share_one_bounded_startup_window() {
        let hold = Duration::from_millis(250);
        let timeout = Duration::from_millis(75);
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
            elapsed < Duration::from_millis(130),
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
