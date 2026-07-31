use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// MCP proxy configuration containing upstream server definitions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub servers: HashMap<String, ServerConfig>,
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
        let user_path = Scope::User.config_path().ok();
        Self::load_merged_from(user_path.as_deref(), project_path)
    }

    fn load_merged_from(user_path: Option<&Path>, project_path: Option<&Path>) -> Result<Config> {
        let mut merged = match user_path {
            Some(path) => Config::load_from(path)?,
            None => Config::default(),
        };
        if let Some(path) = project_path {
            let project = Config::load_from(path)?;
            for (name, server) in project.servers {
                merged.servers.insert(name, server);
            }
        }

        Ok(merged)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip() {
        let mut config = Config::default();
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
    }

    #[test]
    fn load_merged_rejects_malformed_or_unreadable_project_config() {
        let dir = tempfile::tempdir().unwrap();
        let malformed = dir.path().join("malformed.toml");
        std::fs::write(&malformed, "[[not valid").unwrap();
        let error = Config::load_merged_from(None, Some(&malformed)).unwrap_err();
        assert!(error.to_string().contains("failed to parse"));

        let unreadable = dir.path().join("directory-not-file");
        std::fs::create_dir(&unreadable).unwrap();
        let error = Config::load_merged_from(None, Some(&unreadable)).unwrap_err();
        assert!(error.to_string().contains("failed to read"));

        let error = Config::load_merged_from(Some(&malformed), None).unwrap_err();
        assert!(error.to_string().contains("failed to parse"));
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
}
