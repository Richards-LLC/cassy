//! Credential-safe provisioning status for the managed Viktor gateway.

use std::path::Path;

use clap::Args;
use serde::Serialize;

use crate::cli::Cli;

#[derive(Args, Debug, Clone, Default)]
pub struct ViktorArgs {}

#[derive(Serialize)]
struct ViktorReport {
    credential_env: &'static str,
    credential_present: bool,
    user_config: String,
    project_config: Option<String>,
    project_policy: &'static str,
    startup_action: &'static str,
    reply_delivery: &'static str,
    upstream_status: String,
    watched_runs_pending: usize,
    pending_run_ids: Vec<String>,
    inbound_questions_pending: usize,
    pending_inbound_message_ids: Vec<String>,
}

/// Print the configuration contract without reading or displaying the API key.
pub fn execute(_args: &ViktorArgs, cli: &Cli, cas_root: Option<&Path>) -> anyhow::Result<()> {
    #[cfg(feature = "mcp-proxy")]
    let user_config = cmcp_core::config::Scope::User
        .config_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "<user MCP config path unavailable>".to_string());
    #[cfg(not(feature = "mcp-proxy"))]
    let user_config = "<requires mcp-proxy feature>".to_string();

    let project_config = cas_root
        .map(|root| root.join("proxy.toml"))
        .filter(|path| path.exists())
        .map(|path| path.display().to_string());
    let watches = cas_root
        .and_then(|root| cas_store::SqliteViktorWatchStore::open(root).ok())
        .and_then(|store| store.list_live().ok())
        .unwrap_or_default();
    let inbound = cas_root
        .and_then(|root| cas_store::SqliteViktorInboundStore::open(root).ok())
        .and_then(|store| store.list_pending(32).ok())
        .unwrap_or_default();
    #[cfg(feature = "mcp-proxy")]
    let upstream_status = cas_root
        .and_then(|root| crate::mcp::read_proxy_health_cache(root).ok())
        .and_then(|bytes| serde_json::from_slice::<cmcp_core::ProxyHealthSnapshot>(&bytes).ok())
        .and_then(|snapshot| {
            snapshot
                .servers
                .into_iter()
                .find(|server| server.name.eq_ignore_ascii_case("viktor"))
        })
        .map(|server| {
            if server.state == cmcp_core::UpstreamState::Healthy {
                "connected".to_string()
            } else {
                format!(
                    "upstream absent ({:?}; {})",
                    server.state,
                    server
                        .last_error_code
                        .unwrap_or_else(|| "unknown".to_string())
                )
            }
        })
        .unwrap_or_else(|| "not observed by this project daemon".to_string());
    #[cfg(not(feature = "mcp-proxy"))]
    let upstream_status = "requires mcp-proxy feature".to_string();
    let report = ViktorReport {
        credential_env: "VIKTOR_API_KEY",
        credential_present: std::env::var_os("VIKTOR_API_KEY").is_some(),
        user_config,
        project_config,
        project_policy: "an existing .cas/proxy.toml opts out of the managed Viktor default; explicitly configure the sanctioned Viktor server and routes before direct calls",
        startup_action: "run cas serve without a project proxy config; startup refreshes the credential-reference-only managed Viktor upstream",
        reply_delivery: "run-starting calls are durably watched by CAS; replies and Viktor-originated questions arrive as inbound notifications (origin=viktor), and disconnected/no-live states remain durable instead of being dropped",
        upstream_status,
        watched_runs_pending: watches.len(),
        pending_run_ids: watches.into_iter().map(|watch| watch.run_id).collect(),
        inbound_questions_pending: inbound.len(),
        pending_inbound_message_ids: inbound
            .into_iter()
            .map(|message| message.message_id)
            .collect(),
    };

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Viktor provisioning");
        println!(
            "  credential: {} ({})",
            report.credential_env,
            if report.credential_present {
                "set"
            } else {
                "not set"
            }
        );
        println!("  user config: {}", report.user_config);
        println!(
            "  project policy: {}",
            report
                .project_config
                .as_deref()
                .unwrap_or("none; managed user policy applies")
        );
        println!("  start: {}", report.startup_action);
        println!("  upstream: {}", report.upstream_status);
        println!(
            "  watched runs pending: {}{}",
            report.watched_runs_pending,
            if report.pending_run_ids.is_empty() {
                String::new()
            } else {
                format!(" ({})", report.pending_run_ids.join(", "))
            }
        );
        println!(
            "  inbound questions pending: {}{}",
            report.inbound_questions_pending,
            if report.pending_inbound_message_ids.is_empty() {
                String::new()
            } else {
                format!(" ({})", report.pending_inbound_message_ids.join(", "))
            }
        );
        println!("  replies: {}", report.reply_delivery);
    }
    Ok(())
}
