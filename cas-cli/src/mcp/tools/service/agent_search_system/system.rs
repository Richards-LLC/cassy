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
        redact_known_credentials(&stderr)
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !stdout.is_empty() {
            redact_known_credentials(&stdout)
        } else {
            format!("exit status {}", output.status)
        }
    }
}

/// Replace credentials inherited by the worker before any command output or
/// report body is returned. The value is read only long enough to redact it;
/// it is never included in a log, MCP response, or staged artifact.
fn redact_known_credentials(input: &str) -> String {
    let mut redacted = input.to_string();
    for variable in ["GH_TOKEN", "GITHUB_TOKEN", "GIT_ASKPASS"] {
        let Ok(secret) = std::env::var(variable) else {
            continue;
        };
        if !secret.is_empty() {
            redacted = redacted.replace(&secret, "<redacted>");
        }
    }
    redact_token_literals(&redacted)
}

fn redact_token_literals(input: &str) -> String {
    let mut redacted = input.to_string();
    for prefix in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"] {
        let mut search_from = 0;
        while let Some(offset) = redacted[search_from..].find(prefix) {
            let start = search_from + offset;
            let end = redacted[start..]
                .find(|character: char| {
                    character.is_whitespace() || matches!(character, '"' | '\'' | '`')
                })
                .map(|offset| start + offset)
                .unwrap_or(redacted.len());
            redacted.replace_range(start..end, "<redacted>");
            search_from = start + "<redacted>".len();
        }
    }
    redacted
}

fn filename_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if !slug.is_empty() && !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
        if slug.len() >= 64 {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "report".to_string()
    } else {
        slug
    }
}

/// Stage a report on the durable factory artifact root before surfacing a
/// filing failure. `create_new` keeps concurrent failures from overwriting
/// each other's evidence.
fn stage_unfiled_bug_report(
    artifacts_root: &Path,
    task_id: &str,
    title: &str,
    body: &str,
    failure_reason: &str,
) -> Result<std::path::PathBuf, String> {
    let task_dir = artifacts_root
        .join(filename_slug(task_id))
        .join("unfiled-issues");
    std::fs::create_dir_all(&task_dir)
        .map_err(|error| format!("could not create durable unfiled-issues directory: {error}"))?;

    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    let stem = format!("BUG-{}-{timestamp}", filename_slug(title));
    let report = format!(
        "# {title}\n\n{body}\n\n## Filing status\n\n{failure_reason}\n\n\
         This report was retained because automatic GitHub filing did not complete.\n"
    );

    for sequence in 0..100u16 {
        let suffix = if sequence == 0 {
            String::new()
        } else {
            format!("-{sequence}")
        };
        let path = task_dir.join(format!("{stem}{suffix}.md"));
        let Ok(mut file) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        else {
            continue;
        };
        std::io::Write::write_all(&mut file, report.as_bytes())
            .map_err(|error| format!("could not write durable staged report: {error}"))?;
        return Ok(path);
    }

    Err("could not allocate a unique durable staged report filename".to_string())
}

fn current_factory_task_id(core: &crate::mcp::server::CasCore) -> Result<String, String> {
    let agent_id = core
        .get_registered_agent_id_read_only()
        .map_err(|error| format!("registered agent identity unavailable: {}", error.message))?;
    let agent_store = core
        .open_agent_store()
        .map_err(|error| format!("agent store unavailable: {}", error.message))?;
    let task_store = core
        .open_task_store()
        .map_err(|error| format!("task store unavailable: {}", error.message))?;
    let leases = agent_store
        .list_agent_leases(&agent_id)
        .map_err(|error| format!("active task leases unavailable: {error}"))?;

    let mut task_ids = Vec::new();
    for lease in leases {
        let task = task_store
            .get(&lease.task_id)
            .map_err(|error| format!("task {} unavailable: {error}", lease.task_id))?;
        if task.status == cas_types::TaskStatus::InProgress {
            task_ids.push(task.id);
        }
    }
    task_ids.sort();
    task_ids.dedup();
    match task_ids.as_slice() {
        [task_id] => Ok(task_id.clone()),
        [] => Err("registered agent has no active in-progress task lease".to_string()),
        _ => Err(format!(
            "registered agent has {} active in-progress task leases; refusing ambiguous staging target",
            task_ids.len()
        )),
    }
}

fn filing_failure_reason(error: &str, task_identity_warning: Option<&str>) -> String {
    let safe_error = redact_known_credentials(error);
    if safe_error.to_ascii_lowercase().contains("issues.repo") {
        let mut reason = format!(
            "GitHub filing failed: {safe_error}\nConfigure the receiving issue target with \
             `cas config set issues.repo owner/name` before retrying."
        );
        if let Some(warning) = task_identity_warning {
            reason.push_str(&format!(
                "\nTask identity warning: {}. The report was placed under `unassigned`.",
                redact_known_credentials(warning)
            ));
        }
        return reason;
    }
    let mut reason = format!(
        "GitHub filing failed: {}\nGitHub credential missing, rejected, or unauthorized. Required GitHub credential: `GH_TOKEN` or `GITHUB_TOKEN` \
         (or a valid authenticated `gh` account). Factory spawn does not forward either \
         environment variable explicitly, so a worker must fail closed when its inherited \
         credential is absent, rejected, or unauthorized.",
        safe_error
    );
    if let Some(warning) = task_identity_warning {
        reason.push_str(&format!(
            "\nTask identity warning: {}. The report was placed under `unassigned`.",
            redact_known_credentials(warning)
        ));
    }
    reason
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

        let title = redact_known_credentials(&anonymize(&title));
        let description = redact_known_credentials(&anonymize(&description));
        let expected = req
            .expected
            .map(|value| redact_known_credentials(&anonymize(&value)));
        let actual = req
            .actual
            .map(|value| redact_known_credentials(&anonymize(&value)));

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

        let (task_id, task_identity_warning) = match current_factory_task_id(&self.inner) {
            Ok(task_id) => (task_id, None),
            Err(warning) => ("unassigned".to_string(), Some(warning)),
        };
        let filing_result = resolve_issue_repo(&self.inner.cas_root).and_then(|repo| {
            file_bug_report(&GhBugFilingTransport, &repo, &title, &body)
                .map_err(|message| message.to_string())
        });
        let outcome = match filing_result {
            Ok(outcome) => outcome,
            Err(error) => {
                let failure_reason =
                    filing_failure_reason(&error, task_identity_warning.as_deref());
                let artifacts_root = crate::config::resolved_factory_artifacts_root(
                    self.inner.load_config().factory().artifacts_root.as_deref(),
                );
                let path = stage_unfiled_bug_report(
                    &artifacts_root,
                    &task_id,
                    &title,
                    &body,
                    &failure_reason,
                )
                .map_err(|stage_error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!(
                            "Bug report was not filed and durable staging failed: {stage_error}. \
                             Original filing failure: {failure_reason}"
                        ),
                    )
                })?;
                return Ok(Self::success(format!(
                    "Bug report was not filed. {failure_reason}\nStaged report: {}\n\n\
                     File the staged report after GitHub credentials are available.",
                    path.display()
                )));
            }
        };
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
    use super::{
        BugFilingTransport, file_bug_report, filing_failure_reason, resolve_issue_repo,
        stage_unfiled_bug_report,
    };

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

    struct FailingTransport;

    impl BugFilingTransport for FailingTransport {
        fn ensure_agent_reported_label(&self, _repo: &str) -> Result<(), String> {
            Ok(())
        }

        fn create_issue(
            &self,
            _repo: &str,
            _title: &str,
            _body: &str,
            _labels: &[&str],
        ) -> Result<String, String> {
            Err("HTTP 401: Bearer ghp_private-token-value".to_string())
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

    #[test]
    fn failed_filing_stages_an_anonymized_task_scoped_report() {
        let temp = tempfile::tempdir().expect("temporary artifacts directory");
        let filing_error = file_bug_report(&FailingTransport, "example/cassy", "title", "body")
            .expect_err("an unauthorized filing must fail closed");
        let failure_reason = filing_failure_reason(&filing_error, None);
        let path = stage_unfiled_bug_report(
            temp.path(),
            "cas-a178",
            "worker cannot file bug",
            "Description has a private path ~/project and no credential value.",
            &failure_reason,
        )
        .expect("the durable fallback should be writable");

        assert_eq!(
            path.parent().and_then(|parent| parent.file_name()),
            Some(std::ffi::OsStr::new("unfiled-issues"))
        );
        assert!(path.starts_with(temp.path().join("cas-a178")));
        let report = std::fs::read_to_string(&path).expect("staged report");
        assert!(report.contains("worker cannot file bug"));
        assert!(report.contains("GitHub credential missing, rejected, or unauthorized"));
        assert!(report.contains("Required GitHub credential: `GH_TOKEN` or `GITHUB_TOKEN`"));
        assert!(!report.contains("ghp_private-token-value"));
        assert!(!report.contains("private-token-value"));
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
                last_error: Some("token=cache-secret\ncontrol".to_string()),
                last_attempt_at_ms: Some(40),
                next_retry_at_ms: Some(50),
            }],
        };
        let health = parse_proxy_health_cache(&serde_json::to_string(&forged).unwrap()).unwrap();
        let json = serde_json::to_string(&health).unwrap();

        assert_eq!(health["session_id"], "proxy-unknown");
        assert_eq!(health["servers"][0]["transport"], "unknown");
        assert_eq!(health["servers"][0]["last_error_code"], "unknown");
        assert_eq!(
            health["servers"][0]["last_error"],
            "token=[redacted] control"
        );
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
        ] {
            assert!(!json.contains(forbidden), "{forbidden:?} leaked: {json}");
        }
    }
}
