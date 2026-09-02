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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolConfig {
    pub server: String,
    pub tool: String,
}

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
        })
    }

    pub fn canonical_entry(&self) -> String {
        format!("{}.{}", self.server, self.tool)
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
                let config_dir = dirs_config_dir()
                    .context("could not determine user config directory")?;
                Ok(config_dir.join("code-mode-mcp").join("config.toml"))
            }
        }
    }
}

/// Platform-appropriate config directory (~/.config on Linux/macOS).
fn dirs_config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
        })
}

impl Config {
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

        let config: Config = toml::from_str(&content)
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

    fn load_merged_with_sources_from(
        user_path: Option<&Path>,
        project_path: Option<&Path>,
    ) -> Result<(Config, HashMap<String, PathBuf>)> {
        let (mut merged, mut sources) = match user_path {
            Some(path) => {
                let config = Config::load_from(path)?;
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
            let project = Config::load_from(path)?;
            for (name, server) in project.servers {
                merged.servers.insert(name.clone(), server);
                sources.insert(name, path.to_path_buf());
            }
            // Security policy is not union-merged. When a project config is
            // present it is authoritative, including an omitted/empty list;
            // a broader user config must not silently widen project dispatch.
            merged.allowlist = project.allowlist;
            merged.delegation = project.delegation;
        }

        Ok((merged, sources))
    }

    /// Save config to a TOML file.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self)
            .context("failed to serialize config")?;

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
                },
                ExternalToolConfig {
                    server: "neon".to_string(),
                    tool: "write".to_string(),
                },
                ExternalToolConfig {
                    server: "neon".to_string(),
                    tool: "read".to_string(),
                },
                ExternalToolConfig {
                    server: "*".to_string(),
                    tool: "run_sql".to_string(),
                },
                ExternalToolConfig {
                    server: "neon".to_string(),
                    tool: "*".to_string(),
                },
            ]
        );

        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("allowlist = ["));
        assert!(serialized.contains("neon.run_sql"));
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
            assert!(error.to_string().contains("allowlist entry"), "{source}: {error}");
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
