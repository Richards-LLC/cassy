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
    let report = ViktorReport {
        credential_env: "VIKTOR_API_KEY",
        credential_present: std::env::var_os("VIKTOR_API_KEY").is_some(),
        user_config,
        project_config,
        project_policy: "an existing .cas/proxy.toml opts out of the managed Viktor default; explicitly configure the sanctioned Viktor server and routes before direct calls",
        startup_action: "run cas serve without a project proxy config; startup refreshes the credential-reference-only managed Viktor upstream",
        reply_delivery: "run-starting calls are watched by CAS; replies arrive as inbound notifications (origin=viktor), so agents must not poll",
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
        println!("  replies: {}", report.reply_delivery);
    }
    Ok(())
}
