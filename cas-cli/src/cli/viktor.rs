//! Credential-safe provisioning for the managed Viktor gateway.

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::Context;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::cli::Cli;

#[derive(Args, Debug, Clone, Default)]
pub struct ViktorArgs {
    #[command(subcommand)]
    command: Option<ViktorCommand>,
}

#[derive(Subcommand, Debug, Clone)]
enum ViktorCommand {
    /// Validate and save an operator-issued API key for this machine.
    Key,
}

#[derive(Debug, Deserialize, Serialize)]
struct StoredCredential {
    api_key: String,
}

#[derive(Serialize)]
struct ViktorReport {
    credential_env: &'static str,
    credential_present: bool,
    credential_source: &'static str,
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
pub fn execute(args: &ViktorArgs, cli: &Cli, cas_root: Option<&Path>) -> anyhow::Result<()> {
    if matches!(args.command, Some(ViktorCommand::Key)) {
        return save_operator_key(&read_operator_key()?);
    }

    let env_credential_present = std::env::var_os("VIKTOR_API_KEY").is_some();
    let stored_credential_present = load_stored_key().ok().flatten().is_some();
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
        credential_present: env_credential_present || stored_credential_present,
        credential_source: if env_credential_present {
            "environment"
        } else if stored_credential_present {
            "machine key store"
        } else {
            "not configured"
        },
        user_config,
        project_config,
        project_policy: "an existing .cas/proxy.toml opts out of the managed Viktor default; explicitly configure the sanctioned Viktor server and routes before direct calls",
        startup_action: "run `cas viktor key` once, paste the operator-issued key when prompted, then start a new CAS session; cas serve loads the machine-scoped credential and refreshes the managed Viktor upstream",
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
        if !report.credential_present {
            println!("  setup: cas viktor key");
            println!("  key source: get an operator-issued key from the Viktor operator");
        } else {
            println!("  credential source: {}", report.credential_source);
        }
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

/// Load the validated, machine-scoped credential for the in-memory `cas serve`
/// proxy configuration. The on-disk managed config remains a credential reference,
/// and the key never enters project state or process-wide environment variables.
pub(crate) fn load_machine_credential() -> anyhow::Result<Option<String>> {
    load_stored_key()
}

fn save_operator_key(api_key: &str) -> anyhow::Result<()> {
    let api_key = api_key.trim();
    anyhow::ensure!(
        !api_key.is_empty(),
        "Viktor API key is empty. Ask the operator for a key, then run `cas viktor key`."
    );

    validate_key(api_key)?;
    let path = credential_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("creating Viktor credential directory {}", parent.display())
        })?;
    }
    let serialized = toml::to_string(&StoredCredential {
        api_key: api_key.to_string(),
    })?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .with_context(|| {
            format!(
                "writing machine-scoped Viktor credential {}",
                path.display()
            )
        })?;
    file.write_all(serialized.as_bytes())?;
    file.sync_all()?;
    restrict_credential_permissions(&path)?;

    println!("Viktor key validated and saved for this machine.");
    println!("Start a new CAS session to connect the managed Viktor gateway.");
    Ok(())
}

/// Validate only the upstream MCP handshake and tool listing. Neither action
/// starts a Viktor run, so this setup check does not consume run credits.
fn validate_key(api_key: &str) -> anyhow::Result<()> {
    let config = cmcp_core::config::ServerConfig::Http {
        url: cmcp_core::config::VIKTOR_MCP_URL.to_string(),
        auth: Some(api_key.to_string()),
        headers: HashMap::new(),
        oauth: false,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("creating the Viktor setup runtime")?;
    let health = runtime.block_on(async {
        let engine = cmcp_core::ProxyEngine::from_configs(HashMap::from([(
            cmcp_core::config::VIKTOR_SERVER.to_string(),
            config,
        )]))
        .await?;
        Ok::<_, anyhow::Error>(engine.health_snapshot().await)
    })?;
    let viktor = health.servers.iter().find(|server| server.name == "viktor");
    if viktor.is_some_and(|server| server.state == cmcp_core::UpstreamState::Healthy) {
        return Ok(());
    }
    let reason = viktor
        .and_then(|server| server.last_error_code.as_deref())
        .unwrap_or("connection_failed");
    if reason == "authentication_required" {
        anyhow::bail!(
            "Viktor rejected this API key (invalid or expired); it was not saved. Ask the operator for a current key and run `cas viktor key` once."
        );
    }
    anyhow::bail!(
        "Viktor key could not be validated ({reason}); it was not saved. Check your connection and try the same key later."
    )
}

fn credential_path() -> anyhow::Result<PathBuf> {
    crate::config::global_cas_dir()
        .map(|directory| directory.join("viktor.toml"))
        .context("could not determine the machine-scoped Cassy configuration directory")
}

fn load_stored_key() -> anyhow::Result<Option<String>> {
    let path = credential_path()?;
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let credential: StoredCredential = toml::from_str(&content).with_context(|| {
        format!(
            "parsing machine-scoped Viktor credential {}",
            path.display()
        )
    })?;
    let key = credential.api_key.trim();
    anyhow::ensure!(
        !key.is_empty(),
        "machine-scoped Viktor credential is empty; run `cas viktor key`"
    );
    Ok(Some(key.to_string()))
}

fn read_operator_key() -> anyhow::Result<String> {
    print!("Paste the operator-issued Viktor API key: ");
    io::stdout().flush()?;
    let mut key = String::new();
    io::stdin()
        .read_line(&mut key)
        .context("reading the operator-issued Viktor API key")?;
    Ok(key)
}

#[cfg(unix)]
fn restrict_credential_permissions(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).with_context(|| {
        format!(
            "restricting Viktor credential permissions on {}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn restrict_credential_permissions(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}
