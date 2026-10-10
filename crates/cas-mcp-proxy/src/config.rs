use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Canonical Viktor streamable-HTTP upstream. The credential is resolved at
/// connection time so it is safe to place this managed default on disk.
pub const VIKTOR_MCP_URL: &str = "https://api.viktor.com/mcp";
pub const VIKTOR_API_KEY_ENV: &str = "VIKTOR_API_KEY";
pub const VIKTOR_SERVER: &str = "viktor";
pub const VIKTOR_CONVERSATION_TOOLS: [&str; 9] = [
    "ask_viktor",
    "create_thread",
    "send_message",
    "wait_for_run",
    "get_run",
    "get_run_result",
    "list_threads",
    "list_messages",
    "whoami",
];

/// Canonical Violet hub upstream. Credentials are referenced by environment
/// variable name, never stored in proxy configuration.
pub const VIOLET_SERVER: &str = "violet";
/// The manifest names the canonical endpoint and the former hostname that
/// installed registrations migrate from.
pub use cas_types::violet_compatibility::{
    canonical_violet_credential_name, is_violet_hub_url, violet_compatibility,
    violet_credential_names, violet_hub_url,
};
pub const VIOLET_DEFAULT_TOKEN_ENV: &str = "VIOLET_SLACK_TOKEN_CASSY_PROXY";
pub const VIOLET_DEFAULT_BYPASS_ENV: &str = "VIOLET_VERCEL_BYPASS";
pub const VIOLET_BYPASS_HEADER: &str = "x-vercel-protection-bypass";
pub const VIOLET_TOOLS: [&str; 2] = ["violet_read", "violet_post"];

/// Resolve the Violet/legacy pair without reading or mutating the environment.
/// A custom variable retains its original semantics, including empty values.
pub fn violet_credential_value(
    name: &str,
    mut lookup: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    let names = violet_credential_names(name);
    if names.len() == 1 {
        return lookup(name);
    }
    for candidate in names {
        if let Some(value) = lookup(&candidate).filter(|value| !value.trim().is_empty()) {
            return Some(value);
        }
    }
    lookup(name)
}

/// MCP proxy configuration containing upstream server definitions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub servers: HashMap<String, ServerConfig>,
    /// Exact external routes admitted by the production proxy policy.
    ///
    /// An empty list is intentionally fail-closed: configured upstreams may
    /// connect and advertise tools, but no call is forwarded until its parsed
    /// `(server, tool)` pair appears here.
    #[serde(default)]
    pub allowlist: Vec<ExternalToolConfig>,
    /// Optional supervisor-owned delegation gateways.
    #[serde(default)]
    pub delegation: DelegationConfig,
    /// Per-server access for factory workers (cas-ff74, GH #1005 item 2).
    ///
    /// Written per server as `[servers.vercel] worker_access = "read-only"`
    /// (or as a top-level `[worker_access]` table). A read-only server
    /// forwards a worker's call only when the route is allowlisted *and*
    /// read-only: its MCP annotations say `readOnlyHint = true` and not
    /// `destructiveHint = true`, or it is a known read route
    /// ([`DEFAULT_WORKER_READ_ROUTES`] or `worker_read_routes`). Supervisors
    /// and plain sessions keep the full allowlist.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub worker_access: HashMap<String, WorkerAccess>,
    /// Extra routes a worker may call on a read-only server even when the
    /// upstream does not annotate them as read-only (cas-ff74).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worker_read_routes: Vec<ExternalToolConfig>,
    /// Client-side bound on one upstream tool call, in seconds (cas-53ce,
    /// GH #1168). Unset uses [`crate::DEFAULT_CALL_TIMEOUT_SECS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_timeout_secs: Option<u64>,
}

/// How a factory worker may use one upstream server (cas-ff74).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkerAccess {
    /// Only read-only routes: observability without write access.
    ReadOnly,
}

/// Observability routes a worker may call on a `worker_access = "read-only"`
/// server even when the upstream omits read-only annotations (cas-ff74).
pub const DEFAULT_WORKER_READ_ROUTES: [(&str, &str); 7] = [
    ("vercel", "get_runtime_errors"),
    ("vercel", "get_runtime_logs"),
    ("vercel", "list_deployments"),
    ("vercel", "get_deployment"),
    ("neon", "describe_branch"),
    ("neon", "list_branches"),
    ("neon", "query_logs"),
];

/// Lift `worker_access` written inside a `[servers.<name>]` table into
/// [`Config::worker_access`]. Server definitions are an internally tagged
/// enum that ignores unknown keys, so the per-server spelling is read from the
/// raw document. A per-server value wins over a top-level entry.
fn lift_server_worker_access(content: &str, config: &mut Config) -> Result<()> {
    let document: toml::Table = toml::from_str(content).context("failed to parse proxy config")?;
    let Some(servers) = document.get("servers").and_then(toml::Value::as_table) else {
        return Ok(());
    };
    for (name, server) in servers {
        let Some(access) = server.get("worker_access") else {
            continue;
        };
        let access = match access.as_str() {
            Some("read-only") => WorkerAccess::ReadOnly,
            _ => anyhow::bail!(
                "servers.{name}.worker_access must be \"read-only\" (the only supported mode)"
            ),
        };
        config.worker_access.insert(name.clone(), access);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolConfig {
    pub server: String,
    pub tool: String,
    /// GH #988: a `supervisor:<server>.<tool>` entry. The route is callable
    /// by supervisors (and a plain non-factory session) and refused for
    /// factory workers.
    pub supervisor_only: bool,
}

/// Prefix of a role-scoped allowlist entry (GH #988).
pub const SUPERVISOR_ALLOWLIST_PREFIX: &str = "supervisor:";

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ExternalToolConfigInput {
    Structured { server: String, tool: String },
    Entry(String),
}

impl ExternalToolConfig {
    /// Parse the canonical `server.tool` spelling and the historical
    /// separator aliases accepted in project proxy files. A bare tool is
    /// retained as a tool-only route (`*.tool`) for compatibility; new files
    /// should use an explicit server or `server.*` wildcard.
    pub fn parse_allowlist_entry(entry: &str) -> Result<Self, String> {
        let entry = entry.trim();
        if entry.is_empty() {
            return Err("allowlist entry must not be empty".to_string());
        }

        // GH #988: `supervisor:<route>` scopes a route to supervisors. It is
        // unambiguous: tool names cannot contain a separator, so a legacy
        // `supervisor:tool` (server "supervisor") has no second separator and
        // keeps its old meaning.
        if let Some(route) = entry.strip_prefix(SUPERVISOR_ALLOWLIST_PREFIX) {
            if route.contains(['.', ':', '/']) || route.starts_with("mcp__") {
                let mut parsed = Self::parse_allowlist_entry(route)
                    .map_err(|_| format!("invalid allowlist entry {entry:?}"))?;
                if parsed.supervisor_only {
                    return Err(format!("invalid allowlist entry {entry:?}"));
                }
                parsed.supervisor_only = true;
                return Ok(parsed);
            }
        }

        if let Some(encoded) = entry.strip_prefix("mcp__") {
            let Some((server, tool)) = encoded.split_once("__") else {
                return Err(format!("invalid allowlist entry {entry:?}"));
            };
            return Self::from_parts(server, tool, entry);
        }

        let Some(separator) = entry.find(|character| matches!(character, '.' | ':' | '/')) else {
            return Self::from_parts("*", entry, entry);
        };
        let (server, tool) = entry.split_at(separator);
        let tool = &tool[1..];
        Self::from_parts(server, tool, entry)
    }

    fn from_parts(server: &str, tool: &str, original: &str) -> Result<Self, String> {
        if server.is_empty()
            || tool.is_empty()
            || server
                .chars()
                .any(|character| matches!(character, '.' | ':' | '/'))
            || tool
                .chars()
                .any(|character| matches!(character, '.' | ':' | '/'))
            || (server == "*" && tool == "*")
        {
            return Err(format!("invalid allowlist entry {original:?}"));
        }
        Ok(Self {
            server: server.to_string(),
            tool: tool.to_string(),
            supervisor_only: false,
        })
    }

    pub fn canonical_entry(&self) -> String {
        let scope = if self.supervisor_only {
            SUPERVISOR_ALLOWLIST_PREFIX
        } else {
            ""
        };
        format!("{scope}{}.{}", self.server, self.tool)
    }
}

impl<'de> Deserialize<'de> for ExternalToolConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match ExternalToolConfigInput::deserialize(deserializer)? {
            ExternalToolConfigInput::Structured { server, tool } => {
                Self::from_parts(&server, &tool, &format!("{server}.{tool}"))
                    .map_err(D::Error::custom)
            }
            ExternalToolConfigInput::Entry(entry) => {
                Self::parse_allowlist_entry(&entry).map_err(D::Error::custom)
            }
        }
    }
}

impl Serialize for ExternalToolConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.canonical_entry())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DelegationConfig {
    #[serde(default)]
    pub external_production_verification: Option<ExternalProductionVerificationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalProductionVerificationConfig {
    pub server: String,
    #[serde(default = "default_start_tool")]
    pub start_tool: String,
    #[serde(default = "default_wait_tool")]
    pub wait_tool: String,
    #[serde(default = "default_reserved_amount")]
    pub reserved_amount: u64,
    #[serde(default = "default_max_per_run")]
    pub max_per_run: u64,
    #[serde(default = "default_max_active_per_factory_session")]
    pub max_active_per_factory_session: u64,
    #[serde(default = "default_max_active_per_epic")]
    pub max_active_per_epic: u64,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

fn default_start_tool() -> String {
    "ask_viktor".to_string()
}

fn default_wait_tool() -> String {
    "wait_for_run".to_string()
}

fn default_reserved_amount() -> u64 {
    1
}

fn default_max_per_run() -> u64 {
    1
}

fn default_max_active_per_factory_session() -> u64 {
    4
}

fn default_max_active_per_epic() -> u64 {
    2
}

fn default_timeout_seconds() -> u64 {
    120
}

/// Configuration for a single upstream MCP server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum ServerConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    Http {
        url: String,
        #[serde(default)]
        auth: Option<String>,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        oauth: bool,
    },
    Sse {
        url: String,
        #[serde(default)]
        auth: Option<String>,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        oauth: bool,
    },
}

/// Configuration scope.
pub enum Scope {
    User,
}

impl Scope {
    /// Returns the config file path for this scope.
    pub fn config_path(&self) -> Result<PathBuf> {
        match self {
            Scope::User => {
                let config_dir =
                    dirs_config_dir().context("could not determine user config directory")?;
                Ok(config_dir.join("code-mode-mcp").join("config.toml"))
            }
        }
    }
}

/// Platform-appropriate config directory (~/.config on Linux/macOS).
fn dirs_config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

impl Config {
    /// The client-side upstream call timeout (cas-53ce): the configured
    /// `call_timeout_secs` (at least one second), else 90 s.
    pub fn call_timeout(&self) -> std::time::Duration {
        let _ = self.call_timeout_secs;
        std::time::Duration::from_secs(crate::DEFAULT_CALL_TIMEOUT_SECS)
    }

    /// Load config from a specific TOML file. Returns empty Config if file is missing.
    pub fn load_from(path: &Path) -> Result<Config> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Config::default());
            }
            Err(e) => {
                return Err(e).with_context(|| format!("failed to read {}", path.display()));
            }
        };

        if content.trim().is_empty() {
            return Ok(Config::default());
        }

        let mut config: Config = toml::from_str(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        lift_server_worker_access(&content, &mut config)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(config)
    }

    /// Load and merge project config with user config (~/.config/code-mode-mcp/config.toml).
    /// Project config takes precedence over user config.
    pub fn load_merged(project_path: Option<&Path>) -> Result<Config> {
        let (config, _) = Self::load_merged_with_sources(project_path)?;
        Ok(config)
    }

    /// Load and merge project config with user config, retaining the source
    /// path that supplied each final server definition. Project config takes
    /// precedence over user config, so an overridden server is attributed to
    /// the project file.
    pub fn load_merged_with_sources(
        project_path: Option<&Path>,
    ) -> Result<(Config, HashMap<String, PathBuf>)> {
        let user_path = Scope::User.config_path().ok();
        Self::load_merged_with_sources_from(user_path.as_deref(), project_path)
    }

    /// Merge two explicit paths. Public so a caller that owns both locations
    /// — `cas integrate violet` and its doctor row, which must reason
    /// about the machine file *and* the project file by hand — can ask the
    /// same question the runtime asks, and so tests never depend on the real
    /// user config directory.
    pub fn load_merged_with_sources_from(
        user_path: Option<&Path>,
        project_path: Option<&Path>,
    ) -> Result<(Config, HashMap<String, PathBuf>)> {
        let (mut merged, mut sources) = match user_path {
            Some(path) => {
                let mut config = Config::load_from(path)?;
                config.retire_legacy_hub_registration();
                let sources = config
                    .servers
                    .keys()
                    .map(|name| (name.clone(), path.to_path_buf()))
                    .collect();
                (config, sources)
            }
            None => (Config::default(), HashMap::new()),
        };
        if let Some(path) = project_path {
            let mut project = Config::load_from(path)?;
            project.retire_legacy_hub_registration();
            for (name, server) in project.servers {
                merged.servers.insert(name.clone(), server);
                sources.insert(name, path.to_path_buf());
            }
            // Security policy is not union-merged. When a project config is
            // present it is authoritative, including an omitted/empty list;
            // a broader user config must not silently widen project dispatch.
            merged.allowlist = project.allowlist;
            merged.delegation = project.delegation;
            // Worker access narrows the allowlist, so it follows the same
            // rule: the project file is authoritative (cas-ff74).
            merged.worker_access = project.worker_access;
            merged.worker_read_routes = project.worker_read_routes;
        }

        Ok((merged, sources))
    }

    /// Save config to a TOML file.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self).context("failed to serialize config")?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }

        std::fs::write(path, content)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    /// Add or replace a server configuration.
    pub fn add_server(&mut self, name: String, config: ServerConfig) {
        self.servers.insert(name, config);
    }

    /// Remove a server configuration. Returns true if it existed.
    pub fn remove_server(&mut self, name: &str) -> bool {
        self.servers.remove(name).is_some()
    }

    /// An alias must inherit worker restrictions, not provide a second spelling
    /// that bypasses a read-only registration.
    fn mirror_hub_worker_policy(
        &mut self,
        source: &str,
        target: &str,
        source_tools: [&str; 2],
        target_tools: [&str; 2],
    ) -> bool {
        let mut changed = false;
        if let Some(access) = self.worker_access.get(source).copied()
            && !self.worker_access.contains_key(target)
        {
            self.worker_access.insert(target.to_string(), access);
            changed = true;
        }
        let reads: Vec<_> = self
            .worker_read_routes
            .iter()
            .filter_map(|route| {
                if route.server != source {
                    return None;
                }
                let index = source_tools.iter().position(|tool| *tool == route.tool)?;
                Some(ExternalToolConfig {
                    server: target.to_string(),
                    tool: target_tools[index].to_string(),
                    supervisor_only: route.supervisor_only,
                })
            })
            .collect();
        for route in reads {
            if !self.worker_read_routes.contains(&route) {
                self.worker_read_routes.push(route);
                changed = true;
            }
        }
        changed
    }

    /// Install the credential-free Viktor default in a user-scoped config.
    ///
    /// A pre-existing Viktor server is operator-owned and therefore retained,
    /// while its dispatch surface is refreshed to the deliberately small
    /// conversation contract. Project configuration remains authoritative for
    /// policy because [`Self::load_merged`] replaces (rather than unions) the
    /// user allowlist when `.cas/proxy.toml` exists.
    pub fn ensure_viktor_managed_default(&mut self) -> bool {
        let mut changed = false;
        if !self.servers.contains_key(VIKTOR_SERVER) {
            self.servers.insert(
                VIKTOR_SERVER.to_string(),
                ServerConfig::Http {
                    url: VIKTOR_MCP_URL.to_string(),
                    auth: Some(format!("env:{VIKTOR_API_KEY_ENV}")),
                    headers: HashMap::new(),
                    oauth: false,
                },
            );
            changed = true;
        }

        let desired = VIKTOR_CONVERSATION_TOOLS
            .iter()
            .map(|tool| ExternalToolConfig {
                server: VIKTOR_SERVER.to_string(),
                tool: (*tool).to_string(),
                supervisor_only: false,
            })
            .collect::<Vec<_>>();
        if self
            .allowlist
            .iter()
            .filter(|route| route.server == VIKTOR_SERVER)
            .ne(desired.iter())
        {
            self.allowlist.retain(|route| route.server != VIKTOR_SERVER);
            self.allowlist.extend(desired);
            changed = true;
        }
        changed
    }

    /// Install (or correct) the Violet hub registration, referencing both
    /// credentials by environment-variable name only.
    ///
    /// Unlike [`Self::ensure_viktor_managed_default`], a pre-existing server
    /// entry is *replaced*: the operator ran `cas integrate violet` with
    /// explicit variable names, so those names are authoritative. The
    /// allowlist keeps unrelated routes untouched while the
    /// Violet routes are reduced to exactly [`VIOLET_TOOLS`], which
    /// is what evicts the retired `slack_*` entries from an older machine.
    ///
    /// Returns `true` when anything changed, so callers can report
    /// "already configured" without rewriting the file.
    pub fn ensure_violet_registration(
        &mut self,
        url: &str,
        token_env: &str,
        bypass_env: &str,
    ) -> bool {
        let desired_server = ServerConfig::Http {
            url: url.to_string(),
            auth: Some(format!("env:{token_env}")),
            headers: HashMap::from([(
                VIOLET_BYPASS_HEADER.to_string(),
                format!("env:{bypass_env}"),
            )]),
            oauth: false,
        };
        let mut changed = false;
        changed |= self.retire_legacy_hub_registration();
        if self.servers.get(VIOLET_SERVER) != Some(&desired_server) {
            self.servers.insert(VIOLET_SERVER.into(), desired_server);
            changed = true;
        }
        let desired_routes: Vec<_> = VIOLET_TOOLS
            .iter()
            .map(|tool| ExternalToolConfig {
                server: VIOLET_SERVER.into(),
                tool: (*tool).into(),
                supervisor_only: false,
            })
            .collect();
        if self
            .allowlist
            .iter()
            .filter(|route| route.server == VIOLET_SERVER)
            .ne(desired_routes.iter())
        {
            self.allowlist.retain(|route| route.server != VIOLET_SERVER);
            self.allowlist.extend(desired_routes);
            changed = true;
        }
        changed
    }

    /// Move only the retired production hub to Violet, then bring the
    /// production registration onto the canonical endpoint and credential
    /// names. Custom upstreams are operator-owned. Restrictive policy survives.
    pub fn retire_legacy_hub_registration(&mut self) -> bool {
        let retired = self.retire_legacy_hub_server();
        self.modernize_violet_registration() | retired
    }

    fn retire_legacy_hub_server(&mut self) -> bool {
        let contract = violet_compatibility();
        let retired = &contract.retired_server;
        let is_production = self.servers.get(retired).is_some_and(|server| {
            matches!(server, ServerConfig::Http { url, .. } | ServerConfig::Sse { url, .. } if is_violet_hub_url(url))
        });
        if !is_production {
            return false;
        }
        self.mirror_hub_worker_policy(
            retired,
            VIOLET_SERVER,
            [&contract.retired_tools[0], &contract.retired_tools[1]],
            VIOLET_TOOLS,
        );
        let server = self.servers.remove(retired).expect("checked above");
        self.servers.entry(VIOLET_SERVER.into()).or_insert(server);
        if let Some(access) = self.worker_access.remove(retired) {
            self.worker_access.insert(VIOLET_SERVER.into(), access);
        }
        for routes in [&mut self.allowlist, &mut self.worker_read_routes] {
            for route in routes.iter_mut().filter(|route| &route.server == retired) {
                route.server = VIOLET_SERVER.into();
                if let Some(index) = contract
                    .retired_tools
                    .iter()
                    .position(|tool| tool == &route.tool)
                {
                    route.tool = VIOLET_TOOLS[index].into();
                }
            }
            let mut seen = Vec::new();
            routes.retain(|route| {
                if seen.contains(route) {
                    false
                } else {
                    seen.push(route.clone());
                    true
                }
            });
        }
        true
    }

    /// Point a production-hub `violet` registration at the canonical URL and
    /// rename installed-machine credential references (`auth` and header
    /// `env:` values) to their `VIOLET_*` names. Credential resolution falls
    /// back to the legacy variables, so a machine whose credentials file still
    /// holds them keeps working. Custom endpoints and custom names are left
    /// alone. Returns `true` when anything changed.
    pub fn modernize_violet_registration(&mut self) -> bool {
        let Some(
            ServerConfig::Http {
                url, auth, headers, ..
            }
            | ServerConfig::Sse {
                url, auth, headers, ..
            },
        ) = self.servers.get_mut(VIOLET_SERVER)
        else {
            return false;
        };
        if !is_violet_hub_url(url) {
            return false;
        }
        let mut changed = false;
        if url.as_str() != violet_hub_url() {
            *url = violet_hub_url().to_string();
            changed = true;
        }
        for value in auth.iter_mut().chain(headers.values_mut()) {
            if let Some(name) = value.strip_prefix("env:") {
                let canonical = canonical_violet_credential_name(name);
                if canonical != name {
                    *value = format!("env:{canonical}");
                    changed = true;
                }
            }
        }
        changed
    }

    /// Migrate an existing file without adding routes to a project's policy.
    pub fn retire_legacy_hub_file(path: &Path) -> Result<bool> {
        if !path.exists() {
            return Ok(false);
        }
        let mut config = Self::load_from(path)?;
        let changed = config.retire_legacy_hub_registration();
        if changed {
            config.save_to(path)?;
        }
        Ok(changed)
    }

    /// Bootstrap a fresh machine, or retire its former production registration.
    pub fn refresh_violet_managed_default(path: &Path) -> Result<bool> {
        let mut config = Self::load_from(path)?;
        let mut changed = config.retire_legacy_hub_registration();
        if !config.servers.contains_key(VIOLET_SERVER) {
            changed |= config.ensure_violet_registration(
                violet_hub_url(),
                VIOLET_DEFAULT_TOKEN_ENV,
                VIOLET_DEFAULT_BYPASS_ENV,
            );
        }
        if changed {
            config.save_to(path)?;
        }
        Ok(changed)
    }

    pub fn violet_allowlisted_tools(&self) -> Vec<String> {
        self.allowlist
            .iter()
            .filter(|route| route.server == VIOLET_SERVER)
            .map(|route| route.tool.clone())
            .collect()
    }

    pub fn violet_env_names(&self) -> Option<(Option<String>, Option<String>)> {
        let ServerConfig::Http { auth, headers, .. } = self.servers.get(VIOLET_SERVER)? else {
            return Some((None, None));
        };
        Some((
            auth.as_deref()
                .and_then(|value| value.strip_prefix("env:"))
                .map(str::to_string),
            headers
                .get(VIOLET_BYPASS_HEADER)
                .and_then(|value| value.strip_prefix("env:"))
                .map(str::to_string),
        ))
    }

    /// Refresh the user-scoped managed Viktor default without copying a
    /// credential into configuration.
    pub fn refresh_viktor_managed_default(path: &Path) -> Result<bool> {
        let mut config = Self::load_from(path)?;
        let changed = config.ensure_viktor_managed_default();
        if changed {
            config.save_to(path)?;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip() {
        let mut config = Config::default();
        config.allowlist.push(ExternalToolConfig {
            server: "test-http".to_string(),
            tool: "inspect".to_string(),
            supervisor_only: false,
        });
        config.delegation.external_production_verification =
            Some(ExternalProductionVerificationConfig {
                server: "test-http".to_string(),
                start_tool: "inspect".to_string(),
                wait_tool: "wait".to_string(),
                reserved_amount: 1,
                max_per_run: 1,
                max_active_per_factory_session: 4,
                max_active_per_epic: 2,
                timeout_seconds: 30,
            });
        config.add_server(
            "test-stdio".to_string(),
            ServerConfig::Stdio {
                command: "npx".to_string(),
                args: vec!["my-mcp-server".to_string()],
                env: HashMap::from([("KEY".to_string(), "value".to_string())]),
            },
        );
        config.add_server(
            "test-http".to_string(),
            ServerConfig::Http {
                url: "https://example.com/mcp".to_string(),
                auth: Some("token123".to_string()),
                headers: HashMap::new(),
                oauth: false,
            },
        );
        config.add_server(
            "test-sse".to_string(),
            ServerConfig::Sse {
                url: "https://example.com/sse".to_string(),
                auth: None,
                headers: HashMap::from([("X-Custom".to_string(), "val".to_string())]),
                oauth: true,
            },
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        config.save_to(&path).unwrap();
        let loaded = Config::load_from(&path).unwrap();

        assert_eq!(config, loaded);
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let config = Config::load_from(Path::new("/nonexistent/config.toml")).unwrap();
        assert!(config.servers.is_empty());
        assert!(config.allowlist.is_empty());
        assert!(config.delegation.external_production_verification.is_none());
    }

    #[test]
    fn load_merged_rejects_malformed_or_unreadable_project_config() {
        let dir = tempfile::tempdir().unwrap();
        let malformed = dir.path().join("malformed.toml");
        std::fs::write(&malformed, "[[not valid").unwrap();
        let error = Config::load_merged_with_sources_from(None, Some(&malformed)).unwrap_err();
        assert!(error.to_string().contains("failed to parse"));

        let unreadable = dir.path().join("directory-not-file");
        std::fs::create_dir(&unreadable).unwrap();
        let error = Config::load_merged_with_sources_from(None, Some(&unreadable)).unwrap_err();
        assert!(error.to_string().contains("failed to read"));

        let error = Config::load_merged_with_sources_from(Some(&malformed), None).unwrap_err();
        assert!(error.to_string().contains("failed to parse"));
    }

    #[test]
    fn load_merged_with_sources_tracks_user_and_project_server_origins() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = dir.path().join("project.toml");

        let mut user_config = Config::default();
        user_config.add_server(
            "user-only".to_string(),
            ServerConfig::Stdio {
                command: "/user-only".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
            },
        );
        user_config.add_server(
            "shared".to_string(),
            ServerConfig::Stdio {
                command: "/user-shared".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
            },
        );
        user_config.save_to(&user).unwrap();

        let mut project_config = Config::default();
        project_config.add_server(
            "shared".to_string(),
            ServerConfig::Stdio {
                command: "/project-shared".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
            },
        );
        project_config.add_server(
            "project-only".to_string(),
            ServerConfig::Stdio {
                command: "/project-only".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
            },
        );
        project_config.save_to(&project).unwrap();

        let (merged, sources) =
            Config::load_merged_with_sources_from(Some(&user), Some(&project)).unwrap();
        assert!(matches!(
            merged.servers.get("shared"),
            Some(ServerConfig::Stdio { command, .. }) if command == "/project-shared"
        ));
        assert_eq!(sources.get("user-only"), Some(&user));
        assert_eq!(sources.get("shared"), Some(&project));
        assert_eq!(sources.get("project-only"), Some(&project));
    }

    /// cas-ff74: `worker_access = "read-only"` written inside a server table
    /// is read into `Config::worker_access`, extra read routes parse like
    /// allowlist entries, and the project file is authoritative on merge.
    #[test]
    fn per_server_worker_access_is_read_and_project_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = dir.path().join("project.toml");
        std::fs::write(
            &user,
            r#"
[servers.neon]
transport = "http"
url = "https://mcp.neon.tech/mcp"
auth = "env:NEON_API_KEY"
worker_access = "read-only"
"#,
        )
        .unwrap();
        std::fs::write(
            &project,
            r#"
allowlist = ["vercel.*"]
worker_read_routes = ["vercel.get_project"]

[servers.vercel]
transport = "http"
url = "https://mcp.vercel.com"
auth = "env:VERCEL_TOKEN"
worker_access = "read-only"
"#,
        )
        .unwrap();

        let project_only = Config::load_from(&project).unwrap();
        assert_eq!(
            project_only.worker_access.get("vercel"),
            Some(&WorkerAccess::ReadOnly)
        );
        assert_eq!(
            project_only
                .worker_read_routes
                .iter()
                .map(ExternalToolConfig::canonical_entry)
                .collect::<Vec<_>>(),
            vec!["vercel.get_project".to_string()]
        );
        // The server definition itself still parses as before.
        assert!(matches!(
            project_only.servers.get("vercel"),
            Some(ServerConfig::Http { url, .. }) if url == "https://mcp.vercel.com"
        ));

        let (merged, _) =
            Config::load_merged_with_sources_from(Some(&user), Some(&project)).unwrap();
        assert_eq!(
            merged.worker_access,
            HashMap::from([("vercel".to_string(), WorkerAccess::ReadOnly)]),
            "the project file replaces the user's worker access, like the allowlist"
        );

        let user_only = Config::load_merged_with_sources_from(Some(&user), None)
            .unwrap()
            .0;
        assert_eq!(
            user_only.worker_access.get("neon"),
            Some(&WorkerAccess::ReadOnly)
        );

        // A saved config keeps it (top-level table) and loads it back.
        let saved = dir.path().join("saved.toml");
        project_only.save_to(&saved).unwrap();
        assert_eq!(Config::load_from(&saved).unwrap(), project_only);
    }

    #[test]
    fn unknown_worker_access_mode_is_a_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proxy.toml");
        std::fs::write(
            &path,
            r#"
[servers.vercel]
transport = "http"
url = "https://mcp.vercel.com"
worker_access = "write"
"#,
        )
        .unwrap();
        let error = format!("{:#}", Config::load_from(&path).unwrap_err());
        assert!(error.contains("worker_access"), "{error}");
    }

    #[test]
    fn project_security_policy_replaces_instead_of_widens_user_policy() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = dir.path().join("project.toml");
        std::fs::write(
            &user,
            r#"
[[allowlist]]
server = "personal"
tool = "write_everything"

[delegation.external_production_verification]
server = "personal"
"#,
        )
        .unwrap();
        std::fs::write(
            &project,
            r#"
[[allowlist]]
server = "viktor"
tool = "ask_viktor"
"#,
        )
        .unwrap();

        let (merged, _) =
            Config::load_merged_with_sources_from(Some(&user), Some(&project)).unwrap();
        assert_eq!(
            merged.allowlist,
            vec![ExternalToolConfig {
                server: "viktor".to_string(),
                tool: "ask_viktor".to_string(),
                supervisor_only: false,
            }]
        );
        assert!(merged.delegation.external_production_verification.is_none());
    }

    #[test]
    fn allowlist_accepts_canonical_and_legacy_string_route_spellings() {
        let config: Config = toml::from_str(
            r#"
allowlist = ["neon.run_sql", "neon:write", "neon/read", "run_sql", "neon.*"]
"#,
        )
        .unwrap();

        assert_eq!(
            config.allowlist,
            vec![
                ExternalToolConfig {
                    server: "neon".to_string(),
                    tool: "run_sql".to_string(),
                    supervisor_only: false,
                },
                ExternalToolConfig {
                    server: "neon".to_string(),
                    tool: "write".to_string(),
                    supervisor_only: false,
                },
                ExternalToolConfig {
                    server: "neon".to_string(),
                    tool: "read".to_string(),
                    supervisor_only: false,
                },
                ExternalToolConfig {
                    server: "*".to_string(),
                    tool: "run_sql".to_string(),
                    supervisor_only: false,
                },
                ExternalToolConfig {
                    server: "neon".to_string(),
                    tool: "*".to_string(),
                    supervisor_only: false,
                },
            ]
        );

        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("allowlist = ["));
        assert!(serialized.contains("neon.run_sql"));
    }

    /// GH #988: `supervisor:<route>` is a role-scoped entry that round-trips,
    /// while a legacy `supervisor:tool` (server "supervisor") keeps its
    /// meaning.
    #[test]
    fn allowlist_parses_supervisor_scoped_entries() {
        let config: Config = toml::from_str(
            r#"allowlist = ["supervisor:neon.*", "supervisor:neon:run_sql", "supervisor:mcp__neon__list_projects", "supervisor:ask", "viktor.ask_viktor"]"#,
        )
        .unwrap();
        let parsed = config
            .allowlist
            .iter()
            .map(|route| {
                (
                    route.server.as_str(),
                    route.tool.as_str(),
                    route.supervisor_only,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            parsed,
            vec![
                ("neon", "*", true),
                ("neon", "run_sql", true),
                ("neon", "list_projects", true),
                ("supervisor", "ask", false),
                ("viktor", "ask_viktor", false),
            ]
        );
        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("\"supervisor:neon.*\""), "{serialized}");
        let reparsed: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(reparsed.allowlist, config.allowlist);
        for bad in [
            "supervisor:neon.",
            "supervisor:supervisor:neon.*",
            "supervisor:neon:*:x",
        ] {
            let source = format!("allowlist = [\"{bad}\"]");
            assert!(
                toml::from_str::<Config>(&source).is_err(),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn allowlist_rejects_empty_or_malformed_string_route_spellings() {
        for source in [
            "allowlist = [\"\"]",
            "allowlist = [\"neon.\"]",
            "allowlist = [\"neon:*:run_sql\"]",
            "allowlist = [\"*\"]",
        ] {
            let error = toml::from_str::<Config>(source).unwrap_err();
            assert!(
                error.to_string().contains("allowlist entry"),
                "{source}: {error}"
            );
        }
    }

    #[test]
    fn add_and_remove_server() {
        let mut config = Config::default();
        config.add_server(
            "srv".to_string(),
            ServerConfig::Stdio {
                command: "cmd".to_string(),
                args: vec![],
                env: HashMap::new(),
            },
        );
        assert!(config.servers.contains_key("srv"));
        assert!(config.remove_server("srv"));
        assert!(!config.remove_server("srv"));
    }

    #[test]
    fn scope_user_config_path() {
        let path = Scope::User.config_path().unwrap();
        assert!(path.ends_with("code-mode-mcp/config.toml"));
    }

    #[test]
    fn managed_violet_default_registers_only_violet_without_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(Config::refresh_violet_managed_default(&path).unwrap());
        assert!(!Config::refresh_violet_managed_default(&path).unwrap());
        let config = Config::load_from(&path).unwrap();
        assert!(config.servers.contains_key(VIOLET_SERVER));
        assert!(
            !config
                .servers
                .contains_key(&violet_compatibility().retired_server)
        );
        assert_eq!(config.violet_allowlisted_tools(), VIOLET_TOOLS);
        assert_eq!(
            config.violet_env_names(),
            Some((
                Some(VIOLET_DEFAULT_TOKEN_ENV.to_string()),
                Some(VIOLET_DEFAULT_BYPASS_ENV.to_string())
            ))
        );
        let raw = std::fs::read_to_string(path).unwrap();
        assert!(
            raw.contains("url = \"https://violet-hub.vercel.app/mcp/slack\""),
            "{raw}"
        );
        assert!(
            raw.contains("auth = \"env:VIOLET_SLACK_TOKEN_CASSY_PROXY\""),
            "{raw}"
        );
        assert!(raw.contains("env:VIOLET_VERCEL_BYPASS"));
        assert_no_legacy_hub_vocabulary(&raw);
    }

    fn assert_no_legacy_hub_vocabulary(text: &str) {
        let contract = violet_compatibility();
        for legacy in [
            contract.legacy_hub_url.as_str(),
            contract.legacy_token_prefix.as_str(),
            contract.legacy_bypass_env.as_str(),
            contract.retired_server.as_str(),
        ] {
            assert!(!text.contains(legacy), "{legacy} survived in:\n{text}");
        }
    }

    fn legacy_violet_registration(server: &str, url: &str) -> String {
        let contract = violet_compatibility();
        format!(
            r#"allowlist = ["violet.violet_read", "violet.violet_post"]

[servers.{server}]
transport = "http"
url = "{url}"
auth = "env:{token}_CASSY_PROXY"

[servers.{server}.headers]
x-vercel-protection-bypass = "env:{bypass}"
"#,
            token = contract.legacy_token_prefix,
            bypass = contract.legacy_bypass_env,
        )
    }

    #[test]
    fn installed_violet_registration_moves_to_violet_hub_and_violet_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let legacy_url = &violet_compatibility().legacy_hub_url;
        std::fs::write(&path, legacy_violet_registration(VIOLET_SERVER, legacy_url)).unwrap();

        assert!(Config::refresh_violet_managed_default(&path).unwrap());
        assert!(
            !Config::refresh_violet_managed_default(&path).unwrap(),
            "the migration is idempotent"
        );
        let raw = std::fs::read_to_string(&path).unwrap();
        assert_no_legacy_hub_vocabulary(&raw);
        let config = Config::load_from(&path).unwrap();
        assert_eq!(
            config.servers.get(VIOLET_SERVER),
            Some(&ServerConfig::Http {
                url: violet_hub_url().to_string(),
                auth: Some(format!("env:{VIOLET_DEFAULT_TOKEN_ENV}")),
                headers: HashMap::from([(
                    VIOLET_BYPASS_HEADER.to_string(),
                    format!("env:{VIOLET_DEFAULT_BYPASS_ENV}"),
                )]),
                oauth: false,
            })
        );
        assert_eq!(config.violet_allowlisted_tools(), VIOLET_TOOLS);
    }

    #[test]
    fn retired_server_on_either_hostname_becomes_canonical_violet() {
        let contract = violet_compatibility();
        for url in [contract.legacy_hub_url.as_str(), violet_hub_url()] {
            let mut config: Config =
                toml::from_str(&legacy_violet_registration(&contract.retired_server, url)).unwrap();
            assert!(config.retire_legacy_hub_registration());
            assert!(!config.retire_legacy_hub_registration());
            assert_no_legacy_hub_vocabulary(&toml::to_string(&config).unwrap());
            assert_eq!(
                config.violet_env_names(),
                Some((
                    Some(VIOLET_DEFAULT_TOKEN_ENV.to_string()),
                    Some(VIOLET_DEFAULT_BYPASS_ENV.to_string())
                ))
            );
        }
    }

    #[test]
    fn custom_violet_endpoint_keeps_its_url_and_credential_names() {
        let source = legacy_violet_registration(VIOLET_SERVER, "https://staging.example/mcp/slack");
        let mut config: Config = toml::from_str(&source).unwrap();
        let before = config.clone();
        assert!(!config.retire_legacy_hub_registration());
        assert_eq!(config, before);
    }

    #[test]
    fn merged_load_resolves_an_unmigrated_project_file_to_violet_hub() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proxy.toml");
        let legacy_url = &violet_compatibility().legacy_hub_url;
        std::fs::write(
            &project,
            legacy_violet_registration(VIOLET_SERVER, legacy_url),
        )
        .unwrap();
        let (merged, _) = Config::load_merged_with_sources_from(None, Some(&project)).unwrap();
        assert_eq!(
            merged.violet_env_names(),
            Some((
                Some(VIOLET_DEFAULT_TOKEN_ENV.to_string()),
                Some(VIOLET_DEFAULT_BYPASS_ENV.to_string())
            ))
        );
        assert!(matches!(
            merged.servers.get(VIOLET_SERVER),
            Some(ServerConfig::Http { url, .. }) if url == violet_hub_url()
        ));
    }

    #[test]
    fn legacy_hub_alias_preserves_credentials_endpoint_and_policy_scope() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("config.toml");
        std::fs::write(
            &user,
            format!(
                r#"
allowlist = ["supervisor:{retired}.{read}"]
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
                url = violet_hub_url(),
                token = violet_credential_names("VIOLET_SLACK_TOKEN_LAPTOP")[1]
            ),
        )
        .unwrap();
        assert!(Config::refresh_violet_managed_default(&user).unwrap());
        let (merged, sources) = Config::load_merged_with_sources_from(Some(&user), None).unwrap();
        assert!(merged.servers.contains_key(VIOLET_SERVER));
        assert!(
            !merged
                .servers
                .contains_key(&violet_compatibility().retired_server)
        );
        assert_eq!(sources.get(VIOLET_SERVER), Some(&user));
        assert_eq!(merged.violet_allowlisted_tools(), ["violet_read"]);
        assert_eq!(
            merged.worker_access.get(VIOLET_SERVER),
            Some(&WorkerAccess::ReadOnly)
        );
        assert!(
            merged
                .worker_read_routes
                .iter()
                .any(|route| route.server == VIOLET_SERVER && route.tool == "violet_read")
        );
        assert!(
            merged
                .allowlist
                .iter()
                .find(|route| route.server == VIOLET_SERVER)
                .unwrap()
                .supervisor_only
        );
        let project = dir.path().join("project.toml");
        std::fs::write(&project, "allowlist = []\n").unwrap();
        let (merged, _) =
            Config::load_merged_with_sources_from(Some(&user), Some(&project)).unwrap();
        assert!(
            merged.allowlist.is_empty(),
            "project opt-out stays fail-closed"
        );
    }

    #[test]
    fn violet_credentials_prefer_new_values_and_fall_back_for_old_registrations() {
        for (primary, legacy) in [
            (
                "VIOLET_SLACK_TOKEN_LAPTOP",
                violet_credential_names("VIOLET_SLACK_TOKEN_LAPTOP")[1].as_str(),
            ),
            (
                VIOLET_DEFAULT_BYPASS_ENV,
                violet_compatibility().legacy_bypass_env.as_str(),
            ),
        ] {
            for requested in [primary, legacy] {
                let mut values = HashMap::from([(legacy.to_string(), "old".to_string())]);
                assert_eq!(
                    violet_credential_value(requested, |name| values.get(name).cloned()).as_deref(),
                    Some("old")
                );
                values.insert(primary.to_string(), "new".to_string());
                assert_eq!(
                    violet_credential_value(requested, |name| values.get(name).cloned()).as_deref(),
                    Some("new")
                );
                values.insert(primary.to_string(), "".to_string());
                assert_eq!(
                    violet_credential_value(requested, |name| values.get(name).cloned()).as_deref(),
                    Some("old")
                );
            }
        }
        assert_eq!(violet_credential_names("CUSTOM_TOKEN"), ["CUSTOM_TOKEN"]);
        assert_eq!(
            violet_credential_value("CUSTOM_TOKEN", |_| Some(String::new())),
            Some(String::new())
        );
    }

    #[test]
    fn managed_viktor_default_is_credential_free_and_exactly_allowlisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[[allowlist]]
server = "github"
tool = "list_issues"

[[allowlist]]
server = "viktor"
tool = "get_file_download_url"
"#,
        )
        .unwrap();

        assert!(Config::refresh_viktor_managed_default(&path).unwrap());
        assert!(!Config::refresh_viktor_managed_default(&path).unwrap());
        let config = Config::load_from(&path).unwrap();
        assert_eq!(
            config.servers.get(VIKTOR_SERVER),
            Some(&ServerConfig::Http {
                url: VIKTOR_MCP_URL.to_string(),
                auth: Some(format!("env:{VIKTOR_API_KEY_ENV}")),
                headers: HashMap::new(),
                oauth: false,
            })
        );
        assert_eq!(
            config
                .allowlist
                .iter()
                .filter(|route| route.server == VIKTOR_SERVER)
                .map(|route| route.tool.as_str())
                .collect::<Vec<_>>(),
            VIKTOR_CONVERSATION_TOOLS
        );
        assert!(
            config
                .allowlist
                .iter()
                .any(|route| { route.server == "github" && route.tool == "list_issues" })
        );
        assert!(!toml::to_string(&config).unwrap().contains("zt_live"));
    }

    #[test]
    fn user_level_violet_registration_reaches_a_project_without_its_own_proxy_file() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let mut user_config = Config::default();
        user_config.ensure_violet_registration(
            violet_hub_url(),
            VIOLET_DEFAULT_TOKEN_ENV,
            VIOLET_DEFAULT_BYPASS_ENV,
        );
        user_config.save_to(&user).unwrap();

        // No project `.cas/proxy.toml`: the machine registration is the whole
        // policy, so any project on this machine can dispatch both hub tools.
        let (merged, sources) = Config::load_merged_with_sources_from(Some(&user), None).unwrap();
        assert!(
            !merged
                .servers
                .contains_key(&violet_compatibility().retired_server)
        );
        assert_eq!(merged.violet_allowlisted_tools(), VIOLET_TOOLS);
        assert_eq!(sources.get(VIOLET_SERVER), Some(&user));
    }

    #[test]
    fn project_proxy_file_inherits_the_user_hub_server_but_not_its_routes() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.toml");
        let project = dir.path().join("project.toml");
        let mut user_config = Config::default();
        user_config.ensure_violet_registration(
            violet_hub_url(),
            VIOLET_DEFAULT_TOKEN_ENV,
            VIOLET_DEFAULT_BYPASS_ENV,
        );
        user_config.save_to(&user).unwrap();
        std::fs::write(
            &project,
            r#"
allowlist = ["neon.run_sql"]
"#,
        )
        .unwrap();

        let (merged, sources) =
            Config::load_merged_with_sources_from(Some(&user), Some(&project)).unwrap();
        // The upstream itself is machine-wide: the project inherits it.
        assert!(
            !merged
                .servers
                .contains_key(&violet_compatibility().retired_server)
        );
        assert_eq!(sources.get(VIOLET_SERVER), Some(&user));
        // Dispatch policy is not widened by a machine file — a project that
        // declares its own allowlist must name the hub routes itself. This is
        // the exact condition `cas doctor`'s Violet row has to report.
        assert!(merged.violet_allowlisted_tools().is_empty());
    }

    #[test]
    fn managed_viktor_default_preserves_an_operator_owned_upstream() {
        let mut config = Config::default();
        config.add_server(
            VIKTOR_SERVER.to_string(),
            ServerConfig::Http {
                url: "https://operator.example/mcp".to_string(),
                auth: Some("env:OPERATOR_VIKTOR_KEY".to_string()),
                headers: HashMap::new(),
                oauth: false,
            },
        );
        assert!(config.ensure_viktor_managed_default());
        assert!(matches!(
            config.servers.get(VIKTOR_SERVER),
            Some(ServerConfig::Http { url, auth, .. })
                if url == "https://operator.example/mcp"
                    && auth.as_deref() == Some("env:OPERATOR_VIKTOR_KEY")
        ));
    }
}
