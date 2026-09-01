use crate::mcp::tools::service::imports::*;

use std::path::Path;
use std::process::{Command, Output};

const ISSUE_REPO_SETUP: &str =
    "issues.repo is not configured; set it with `cas config set issues.repo owner/name`";

#[derive(Debug, PartialEq, Eq)]
struct BugFilingOutcome {
    url: String,
    degradation: Option<String>,
}

trait BugFilingTransport {
    fn ensure_agent_reported_label(&self, repo: &str) -> Result<(), String>;

    fn create_issue(
        &self,
        repo: &str,
        title: &str,
        body: &str,
        labels: &[&str],
    ) -> Result<String, String>;
}

struct GhBugFilingTransport;

impl BugFilingTransport for GhBugFilingTransport {
    fn ensure_agent_reported_label(&self, repo: &str) -> Result<(), String> {
        let output = Command::new("gh")
            .args([
                "label",
                "create",
                "agent-reported",
                "--repo",
                repo,
                "--color",
                "B60205",
                "--description",
                "Reported by an automated Cassy agent",
                "--force",
            ])
            .output()
            .map_err(|error| format!("Failed to run gh CLI while preparing label: {error}"))?;

        if output.status.success() {
            Ok(())
        } else {
            Err(command_failure_detail(&output))
        }
    }

    fn create_issue(
        &self,
        repo: &str,
        title: &str,
        body: &str,
        labels: &[&str],
    ) -> Result<String, String> {
        let mut command = Command::new("gh");
        command.args([
            "issue", "create", "--repo", repo, "--title", title, "--body", body,
        ]);
        for label in labels {
            command.args(["--label", label]);
        }

        let output = command.output().map_err(|error| {
            format!("Failed to run gh CLI: {error}. Is gh installed and authenticated?")
        })?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(command_failure_detail(&output))
        }
    }
}

fn command_failure_detail(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        stderr
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !stdout.is_empty() {
            stdout
        } else {
            format!("exit status {}", output.status)
        }
    }
}

fn resolve_issue_repo(cas_root: &Path) -> Result<String, String> {
    let config = crate::config::Config::load(cas_root)
        .map_err(|error| format!("Failed to load config: {error}"))?;
    let Some(repo) = config.issues.and_then(|issues| issues.repo) else {
        return Err(ISSUE_REPO_SETUP.to_string());
    };

    let repo = repo.trim();
    if repo.is_empty() {
        return Err(ISSUE_REPO_SETUP.to_string());
    }
    if crate::gh_graphql::split_repo(repo).is_err() {
        return Err(
            "issues.repo must be configured as `owner/name`; set it with `cas config set issues.repo owner/name`"
                .to_string(),
        );
    }

    Ok(repo.to_string())
}

fn file_bug_report<T: BugFilingTransport>(
    transport: &T,
    repo: &str,
    title: &str,
    body: &str,
) -> Result<BugFilingOutcome, String> {
    let label_error = transport.ensure_agent_reported_label(repo).err();
    let labels: &[&str] = if label_error.is_none() {
        &["bug", "agent-reported"]
    } else {
        &[]
    };

    let url = transport.create_issue(repo, title, body, labels).map_err(|error| {
        if let Some(label_error) = label_error.as_deref() {
            format!(
                "Failed to create issue: {error}; agent-reported label setup failed ({label_error})"
            )
        } else {
            format!("Failed to create issue: {error}")
        }
    })?;

    let degradation = label_error.map(|error| {
        format!(
            "Warning: could not prepare the agent-reported label ({error}); issue was filed labeless."
        )
    });

    Ok(BugFilingOutcome { url, degradation })
}

#[cfg(feature = "mcp-proxy")]
fn parse_proxy_health_cache(json: &str) -> serde_json::Result<serde_json::Value> {
    serde_json::from_str::<cmcp_core::ProxyHealthSnapshot>(json)
        .map(cmcp_core::ProxyHealthSnapshot::sanitized)
        .and_then(serde_json::to_value)
}

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
        let project_root = self.inner.cas_root.parent().unwrap_or(&self.inner.cas_root);
        let report = crate::factory_preflight::collect_factory_preflight(
            project_root,
            &self.inner.cas_root,
            true,
            None,
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
- **Cassy Version**: {version}
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

        let repo = resolve_issue_repo(&self.inner.cas_root)
            .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))?;
        let outcome = file_bug_report(&GhBugFilingTransport, &repo, &title, &body)
            .map_err(|message| Self::error(ErrorCode::INTERNAL_ERROR, message))?;
        let degradation = outcome
            .degradation
            .map(|message| format!("\n\n{message}"))
            .unwrap_or_default();

        Ok(Self::success(format!(
            "Bug report created: {}\n\nNote: Home directory paths were auto-anonymized. \
            Please verify the issue doesn't contain sensitive project data.{}",
            outcome.url, degradation
        )))
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

        let (raw_name, is_update) = match cas_types::resolve_public_upstream_id(
            config.servers.keys().map(String::as_str),
            &name,
        ) {
            cas_types::PublicUpstreamIdResolution::Found { raw_name, .. } => (raw_name, true),
            cas_types::PublicUpstreamIdResolution::NotFound
                if config.servers.contains_key(&name)
                    && !cas_types::is_generated_public_upstream_id(&name) =>
            {
                (name, true)
            }
            cas_types::PublicUpstreamIdResolution::NotFound
                if cas_types::is_generated_public_upstream_id(&name) =>
            {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "Server identifier was not found; run proxy_list again",
                ));
            }
            cas_types::PublicUpstreamIdResolution::NotFound => (name, false),
            cas_types::PublicUpstreamIdResolution::Ambiguous => {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "Server identifier is ambiguous; run proxy_list again",
                ));
            }
        };
        config.add_server(raw_name.clone(), server_config);
        config.save_to(&proxy_path).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to save proxy config: {e}"),
            )
        })?;

        let verb = if is_update { "Updated" } else { "Added" };
        let public_name = cas_types::public_upstream_ids(config.servers.keys().map(String::as_str))
            .remove(&raw_name)
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    "Updated server is missing from the public identity projection",
                )
            })?;
        Ok(Self::success(format!(
            "{verb} MCP server '{public_name}' ({transport} transport). Restart `cas serve` to connect."
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

        let public_names =
            cas_types::public_upstream_ids(config.servers.keys().map(String::as_str));
        let resolved = match cas_types::resolve_public_upstream_id(
            config.servers.keys().map(String::as_str),
            &name,
        ) {
            cas_types::PublicUpstreamIdResolution::Found {
                raw_name,
                public_name,
            } => Some((raw_name, public_name)),
            cas_types::PublicUpstreamIdResolution::NotFound
                if config.servers.contains_key(&name)
                    && !cas_types::is_generated_public_upstream_id(&name) =>
            {
                Some((
                    name.clone(),
                    public_names
                        .get(&name)
                        .cloned()
                        .unwrap_or_else(|| cas_types::public_upstream_id(&name)),
                ))
            }
            cas_types::PublicUpstreamIdResolution::NotFound => None,
            cas_types::PublicUpstreamIdResolution::Ambiguous => {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "Server identifier is ambiguous; run proxy_list again",
                ));
            }
        };
        let Some((raw_name, public_name)) = resolved else {
            return Ok(Self::success("Server identifier not found in proxy config"));
        };

        debug_assert!(config.remove_server(&raw_name));

        config.save_to(&proxy_path).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to save proxy config: {e}"),
            )
        })?;

        Ok(Self::success(format!(
            "Removed MCP server '{public_name}'. Restart `cas serve` to disconnect."
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

        let public_names =
            cas_types::public_upstream_ids(config.servers.keys().map(String::as_str));
        let servers: Vec<serde_json::Value> = config
            .servers
            .iter()
            .map(|(name, cfg)| {
                let mut obj = serde_json::to_value(cfg).unwrap_or_default();
                if let serde_json::Value::Object(ref mut m) = obj {
                    m.insert(
                        "name".to_string(),
                        serde_json::json!(
                            public_names
                                .get(name)
                                .cloned()
                                .unwrap_or_else(|| cas_types::public_upstream_id(name))
                        ),
                    );
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
        let json = crate::mcp::read_proxy_health_cache(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("MCP proxy health is unavailable: {error}"),
            )
        })?;
        let json = String::from_utf8(json).map_err(|_| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                "MCP proxy health cache is invalid",
            )
        })?;
        let snapshot = parse_proxy_health_cache(&json).map_err(|_| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                "MCP proxy health cache is invalid",
            )
        })?;

        Ok(Self::success(
            serde_json::to_string_pretty(&snapshot).unwrap_or_default(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{BugFilingTransport, file_bug_report, resolve_issue_repo};

    #[derive(Debug)]
    struct FakeTransport {
        label_result: Result<(), String>,
        issue_url: String,
        labels_seen: std::cell::RefCell<Vec<String>>,
    }

    impl BugFilingTransport for FakeTransport {
        fn ensure_agent_reported_label(&self, _repo: &str) -> Result<(), String> {
            self.label_result.clone()
        }

        fn create_issue(
            &self,
            _repo: &str,
            _title: &str,
            _body: &str,
            labels: &[&str],
        ) -> Result<String, String> {
            self.labels_seen
                .borrow_mut()
                .extend(labels.iter().map(|label| (*label).to_string()));
            Ok(self.issue_url.clone())
        }
    }

    #[test]
    fn missing_agent_label_files_labeless_and_reports_degradation() {
        let transport = FakeTransport {
            label_result: Err("permission denied".to_string()),
            issue_url: "https://github.com/example/cassy/issues/1".to_string(),
            labels_seen: std::cell::RefCell::new(Vec::new()),
        };

        let outcome = file_bug_report(&transport, "example/cassy", "title", "body")
            .expect("issue creation should continue without the cosmetic label");

        assert_eq!(outcome.url, "https://github.com/example/cassy/issues/1");
        assert!(
            outcome
                .degradation
                .as_deref()
                .is_some_and(|message| message.contains("filed labeless"))
        );
        assert!(transport.labels_seen.borrow().is_empty());
    }

    #[test]
    fn available_agent_label_is_applied_with_the_bug_label() {
        let transport = FakeTransport {
            label_result: Ok(()),
            issue_url: "https://github.com/example/cassy/issues/2".to_string(),
            labels_seen: std::cell::RefCell::new(Vec::new()),
        };

        let outcome = file_bug_report(&transport, "example/cassy", "title", "body")
            .expect("issue creation should succeed");

        assert_eq!(outcome.url, "https://github.com/example/cassy/issues/2");
        assert!(outcome.degradation.is_none());
        assert_eq!(
            &*transport.labels_seen.borrow(),
            &["bug".to_string(), "agent-reported".to_string()]
        );
    }

    #[test]
    fn unset_issue_repo_refuses_without_an_implicit_target() {
        let temp = tempfile::tempdir().expect("temporary config directory");
        let error = resolve_issue_repo(temp.path()).expect_err("unset repo must refuse");

        assert!(error.contains("issues.repo"));
        assert!(error.contains("cas config set issues.repo owner/name"));
    }

    #[test]
    fn configured_issue_repo_is_the_only_filing_target() {
        let temp = tempfile::tempdir().expect("temporary config directory");
        let mut config = crate::config::Config::default();
        config
            .set("issues.repo", "example/project")
            .expect("repo setting should be valid");
        config
            .save(temp.path())
            .expect("project config should be saved");

        assert_eq!(
            resolve_issue_repo(temp.path()).expect("configured repo should resolve"),
            "example/project"
        );
    }

    #[cfg(feature = "mcp-proxy")]
    use super::parse_proxy_health_cache;

    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn forged_cached_health_is_sanitized_before_system_json() {
        let raw_name = "https://user:token@example.invalid/private";
        let raw_session = "/home/operator/secret-session";
        let forged = cmcp_core::ProxyHealthSnapshot {
            session_id: raw_session.to_string(),
            generated_at_ms: 42,
            healthy: 0,
            degraded: 1,
            servers: vec![cmcp_core::UpstreamHealth {
                name: raw_name.to_string(),
                transport: "Bearer cache-secret".to_string(),
                state: cmcp_core::UpstreamState::Backoff,
                executable: None,
                attempts: 1,
                consecutive_failures: 1,
                tool_count: 0,
                last_error_code: Some("token=cache-secret\ncontrol".to_string()),
                last_attempt_at_ms: Some(40),
                next_retry_at_ms: Some(50),
            }],
        };
        let health = parse_proxy_health_cache(&serde_json::to_string(&forged).unwrap()).unwrap();
        let json = serde_json::to_string(&health).unwrap();

        assert_eq!(health["session_id"], "proxy-unknown");
        assert_eq!(health["servers"][0]["transport"], "unknown");
        assert_eq!(health["servers"][0]["last_error_code"], "unknown");
        assert!(
            health["servers"][0]["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("upstream-") && name.len() == 41)
        );
        for forbidden in [
            raw_name,
            raw_session,
            "Bearer cache-secret",
            "token=cache-secret",
            "control",
        ] {
            assert!(!json.contains(forbidden), "{forbidden:?} leaked: {json}");
        }
    }
}
