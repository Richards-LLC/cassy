use crate::mcp::tools::service::imports::*;

impl CasService {
    pub(in crate::mcp::tools::service) async fn system_version(
        &self,
    ) -> Result<CallToolResult, McpError> {
        let version = env!("CARGO_PKG_VERSION");
        let git_hash = option_env!("CAS_GIT_HASH").unwrap_or("unknown");
        let build_date = option_env!("CAS_BUILD_DATE").unwrap_or("unknown");

        let response = serde_json::json!({
            "version": version,
            "git_hash": git_hash,
            "build_date": build_date,
            "full": format!("{} ({} {})", version, git_hash, build_date)
        });

        Ok(Self::success(
            serde_json::to_string_pretty(&response).unwrap(),
        ))
    }

    pub(in crate::mcp::tools::service) async fn system_preflight(
        &self,
    ) -> Result<CallToolResult, McpError> {
        #[cfg(feature = "mcp-proxy")]
        let live_proxy = match self.proxy.as_ref() {
            Some(proxy) => Some(proxy.health_snapshot().await.into()),
            None => None,
        };
        #[cfg(not(feature = "mcp-proxy"))]
        let live_proxy = None;

        let project_root = self.inner.cas_root.parent().unwrap_or(&self.inner.cas_root);
        let report = crate::factory_preflight::collect_factory_preflight(
            project_root,
            &self.inner.cas_root,
            true,
            live_proxy,
        );
        Ok(Self::success(
            serde_json::to_string_pretty(&report).unwrap_or_default(),
        ))
    }

    pub(in crate::mcp::tools::service) async fn system_doctor(
        &self,
        _req: SystemRequest,
    ) -> Result<CallToolResult, McpError> {
        self.inner.cas_doctor().await
    }

    pub(in crate::mcp::tools::service) async fn system_stats(
        &self,
        _req: SystemRequest,
    ) -> Result<CallToolResult, McpError> {
        self.inner.cas_stats().await
    }

    pub(in crate::mcp::tools::service) async fn system_info(
        &self,
        _req: SystemRequest,
    ) -> Result<CallToolResult, McpError> {
        self.inner.cas_system_info().await
    }

    pub(in crate::mcp::tools::service) async fn system_reindex(
        &self,
        req: SystemRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::ReindexRequest;
        let inner_req = ReindexRequest {
            bm25: req.bm25.unwrap_or(false),
            embeddings: req.embeddings.unwrap_or(false),
            missing_only: req.missing_only.unwrap_or(false),
        };
        self.inner.cas_reindex(Parameters(inner_req)).await
    }

    pub(in crate::mcp::tools::service) async fn system_maintenance_run(
        &self,
        req: SystemRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::MaintenanceRunRequest;
        let inner_req = MaintenanceRunRequest {
            force: req.force.unwrap_or(false),
        };
        self.inner.cas_maintenance_run(Parameters(inner_req)).await
    }

    pub(in crate::mcp::tools::service) async fn system_maintenance_status(
        &self,
        _req: SystemRequest,
    ) -> Result<CallToolResult, McpError> {
        self.inner.cas_maintenance_status().await
    }

    pub(in crate::mcp::tools::service) async fn system_config_docs(
        &self,
    ) -> Result<CallToolResult, McpError> {
        use crate::config::registry;
        let markdown = registry().generate_markdown();
        Ok(Self::success(markdown))
    }

    pub(in crate::mcp::tools::service) async fn system_config_search(
        &self,
        req: SystemRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::config::registry;

        let query = req.query.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "query is required for config_search action",
            )
        })?;

        let results = registry().search(&query);

        if results.is_empty() {
            return Ok(Self::success(format!(
                "No config options matching '{query}'"
            )));
        }

        let mut output = format!(
            "Found {} config option(s) matching '{}':\n\n",
            results.len(),
            query
        );

        for meta in results {
            output.push_str(&format!("### {}\n", meta.key));
            output.push_str(&format!("**{}**\n\n", meta.name));
            output.push_str(&format!("{}\n\n", meta.description));
            output.push_str(&format!("- Type: `{}`\n", meta.value_type.name()));
            output.push_str(&format!("- Default: `{}`\n", meta.default));
            if !meta.keywords.is_empty() {
                output.push_str(&format!("- Keywords: {}\n", meta.keywords.join(", ")));
            }
            if !meta.use_cases.is_empty() {
                output.push_str("- Use cases:\n");
                for use_case in meta.use_cases {
                    output.push_str(&format!("  - {use_case}\n"));
                }
            }
            output.push_str("\n---\n\n");
        }

        Ok(Self::success(output))
    }

    pub(in crate::mcp::tools::service) async fn system_report_cas_bug(
        &self,
        req: SystemRequest,
    ) -> Result<CallToolResult, McpError> {
        let title = req.title.ok_or_else(|| {
            Self::error(ErrorCode::INVALID_PARAMS, "title required for bug report")
        })?;
        let description = req.description.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "description required for bug report",
            )
        })?;

        let home = std::env::var("HOME").unwrap_or_default();
        let anonymize = |input: &str| -> String {
            if !home.is_empty() {
                input.replace(&home, "~")
            } else {
                input.to_string()
            }
        };

        let title = anonymize(&title);
        let description = anonymize(&description);
        let expected = req.expected.map(|value| anonymize(&value));
        let actual = req.actual.map(|value| anonymize(&value));

        let version = env!("CARGO_PKG_VERSION");
        let os_info = std::env::consts::OS;
        let arch = std::env::consts::ARCH;

        let body = format!(
            r#"## Description
{description}

## Expected Behavior
{expected}

## Actual Behavior
{actual}

## Environment
- **CAS Version**: {version}
- **OS**: {os_info}
- **Arch**: {arch}

---
*Reported by agent via `mcp__cas__system action=report_cas_bug`*
*Home directory paths have been automatically anonymized*
"#,
            description = description,
            expected = expected.as_deref().unwrap_or("Not specified"),
            actual = actual.as_deref().unwrap_or("Not specified"),
            version = version,
            os_info = os_info,
            arch = arch,
        );

        let output = std::process::Command::new("gh")
            .args([
                "issue",
                "create",
                "--repo",
                "pippenz/cas",
                "--title",
                &title,
                "--body",
                &body,
                "--label",
                "bug,agent-reported",
            ])
            .output()
            .map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to run gh CLI: {error}. Is gh installed and authenticated?"),
                )
            })?;

        if output.status.success() {
            let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(Self::success(format!(
                "Bug report created: {url}\n\nNote: Home directory paths were auto-anonymized. \
                Please verify the issue doesn't contain sensitive project data."
            )))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to create issue: {stderr}"),
            ))
        }
    }

    // ========================================================================
    // Proxy Management Actions (requires mcp-proxy feature)
    // ========================================================================

    #[cfg(feature = "mcp-proxy")]
    pub(in crate::mcp::tools::service) async fn system_proxy_add(
        &self,
        req: SystemRequest,
    ) -> Result<CallToolResult, McpError> {
        use cmcp_core::config::{Config, ServerConfig};
        use std::collections::HashMap;

        let name = req.name.ok_or_else(|| {
            Self::error(ErrorCode::INVALID_PARAMS, "name is required for proxy_add")
        })?;

        let transport = req.transport.as_deref().unwrap_or("stdio");

        let server_config = match transport {
            "stdio" => {
                let command = req.command.ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "command is required for stdio transport",
                    )
                })?;
                let args: Vec<String> = req
                    .args
                    .as_deref()
                    .map(|s| serde_json::from_str(s).unwrap_or_default())
                    .unwrap_or_default();
                let env: HashMap<String, String> = req
                    .env
                    .as_deref()
                    .map(|s| serde_json::from_str(s).unwrap_or_default())
                    .unwrap_or_default();
                ServerConfig::Stdio { command, args, env }
            }
            "http" => {
                let url = req.url.ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "url is required for http transport",
                    )
                })?;
                ServerConfig::Http {
                    url,
                    auth: req.auth,
                    headers: HashMap::new(),
                    oauth: false,
                }
            }
            "sse" => {
                let url = req.url.ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "url is required for sse transport",
                    )
                })?;
                ServerConfig::Sse {
                    url,
                    auth: req.auth,
                    headers: HashMap::new(),
                    oauth: false,
                }
            }
            other => {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!("Unknown transport '{other}'. Use: stdio, http, or sse"),
                ));
            }
        };

        let proxy_path = self.inner.cas_root.join("proxy.toml");
        let mut config = Config::load_from(&proxy_path).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to load proxy config: {e}"),
            )
        })?;

        let is_update = config.servers.contains_key(&name);
        config.add_server(name.clone(), server_config);
        config.save_to(&proxy_path).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to save proxy config: {e}"),
            )
        })?;

        let verb = if is_update { "Updated" } else { "Added" };
        Ok(Self::success(format!(
            "{verb} MCP server '{name}' ({transport} transport). Restart `cas serve` to connect."
        )))
    }

    #[cfg(feature = "mcp-proxy")]
    pub(in crate::mcp::tools::service) async fn system_proxy_remove(
        &self,
        req: SystemRequest,
    ) -> Result<CallToolResult, McpError> {
        use cmcp_core::config::Config;

        let name = req.name.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "name is required for proxy_remove",
            )
        })?;

        let proxy_path = self.inner.cas_root.join("proxy.toml");
        let mut config = Config::load_from(&proxy_path).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to load proxy config: {e}"),
            )
        })?;

        if !config.remove_server(&name) {
            return Ok(Self::success(format!(
                "Server '{name}' not found in proxy config"
            )));
        }

        config.save_to(&proxy_path).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to save proxy config: {e}"),
            )
        })?;

        Ok(Self::success(format!(
            "Removed MCP server '{name}'. Restart `cas serve` to disconnect."
        )))
    }

    #[cfg(feature = "mcp-proxy")]
    pub(in crate::mcp::tools::service) async fn system_proxy_list(
        &self,
        _req: SystemRequest,
    ) -> Result<CallToolResult, McpError> {
        use cmcp_core::config::Config;

        let proxy_path = self.inner.cas_root.join("proxy.toml");
        let config = Config::load_from(&proxy_path).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to load proxy config: {e}"),
            )
        })?;

        if config.servers.is_empty() {
            return Ok(Self::success(
                "No upstream MCP servers configured.\n\nAdd one with:\n  \
                 mcp__cas__system action=proxy_add name=<name> command=<cmd>\n  \
                 cas mcp add <name> <command>",
            ));
        }

        let servers: Vec<serde_json::Value> = config
            .servers
            .iter()
            .map(|(name, cfg)| {
                let mut obj = serde_json::to_value(cfg).unwrap_or_default();
                if let serde_json::Value::Object(ref mut m) = obj {
                    m.insert("name".to_string(), serde_json::json!(name));
                }
                obj
            })
            .collect();

        let response = serde_json::json!({
            "config_path": proxy_path.display().to_string(),
            "count": servers.len(),
            "servers": servers,
        });

        Ok(Self::success(
            serde_json::to_string_pretty(&response).unwrap_or_default(),
        ))
    }

    #[cfg(feature = "mcp-proxy")]
    pub(in crate::mcp::tools::service) async fn system_proxy_health(
        &self,
        _req: SystemRequest,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = match self.proxy.as_ref() {
            Some(proxy) => serde_json::to_value(proxy.health_snapshot().await).map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to serialize MCP proxy health: {error}"),
                )
            }),
            None => {
                let path = self.inner.cas_root.join("proxy_health.json");
                std::fs::read_to_string(&path)
                    .map_err(|error| {
                        Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!(
                                "MCP proxy health is unavailable (no active proxy and no cache): {error}"
                            ),
                        )
                    })
                    .and_then(|json| {
                        serde_json::from_str(&json).map_err(|error| {
                            Self::error(
                                ErrorCode::INTERNAL_ERROR,
                                format!("MCP proxy health cache is invalid: {error}"),
                            )
                        })
                    })
            }
        }?;

        Ok(Self::success(
            serde_json::to_string_pretty(&snapshot).unwrap_or_default(),
        ))
    }
}
