//! Opt-in home-gateway transport for Claude and Codex workers.
//!
//! An overlay on the natively selected account: the worker keeps its account
//! directory (hooks, MCP, sessions) and only its model traffic goes to the
//! hub. Absent or disabled config leaves the launch untouched. A failed
//! spawn-time readiness check drops the overlay, so the worker starts direct
//! on the account native selection already chose, and says so.

use cas_pty::gateway_key::{CODEX_GATEWAY_KEY_ENV, GATEWAY_KEY_FILE_ENV, read_gateway_key};
use cas_types::factory_gateway::FactoryGatewayConfig;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::pty::PtyConfig;

/// Worker transport recorded on the agent row and shown by worker_status.
pub const TRANSPORT_ENV: &str = "CAS_FACTORY_WORKER_TRANSPORT";
const CODEX_PROVIDER: &str = "cas_gateway";

#[derive(Debug, PartialEq, Eq)]
enum Plan {
    Off,
    Gateway { endpoint: String, key_file: PathBuf },
    Fallback(String),
}

/// Route a Claude or Codex worker through the hub when its project opted in.
pub(crate) fn apply(config: &mut PtyConfig, cas_root: Option<&PathBuf>, worker: &str) {
    let cli = config.effective_command().to_string();
    let settings = load(cas_root.map(PathBuf::as_path));
    apply_plan(config, &cli, worker, plan(&settings, &cli, probe));
}

fn apply_plan(config: &mut PtyConfig, cli: &str, worker: &str, plan: Plan) {
    let transport = match plan {
        Plan::Off => return,
        Plan::Fallback(reason) => {
            tracing::warn!(worker, cli, %reason, "factory worker gateway fallback: starting direct");
            format!("direct (gateway fallback: {reason})")
        }
        Plan::Gateway { endpoint, key_file } => {
            tracing::info!(worker, cli, %endpoint, "factory worker gateway transport");
            let client = format!("cassy/{worker}");
            config.env.push((GATEWAY_KEY_FILE_ENV.into(), key_file.to_string_lossy().into_owned()));
            if cli == "codex" {
                config.env.push(("CODEX_INTERNAL_ORIGINATOR_OVERRIDE".into(), client));
                let provider = toml::Value::Table(toml::map::Map::from_iter([
                    ("name".to_string(), toml::Value::String("Cassy gateway".into())),
                    ("base_url".to_string(), toml::Value::String(format!("{endpoint}/v1"))),
                    ("env_key".to_string(), toml::Value::String(CODEX_GATEWAY_KEY_ENV.into())),
                    ("wire_api".to_string(), toml::Value::String("responses".into())),
                ]));
                config.args.extend([
                    "-c".into(),
                    format!("model_provider=\"{CODEX_PROVIDER}\""),
                    "-c".into(),
                    format!("model_providers.{CODEX_PROVIDER}={provider}"),
                ]);
            } else {
                config.env.push(("ANTHROPIC_BASE_URL".into(), endpoint.clone()));
                config.env.push(("CLAUDE_AGENT_SDK_CLIENT_APP".into(), client));
            }
            format!("gateway {endpoint}")
        }
    };
    if cli == "codex" {
        // Codex starts `cas serve` with a restricted environment.
        let value = serde_json::to_string(&transport).expect("serializing a string cannot fail");
        config.args.extend(["-c".into(), format!("mcp_servers.cs.env.{TRANSPORT_ENV}={value}")]);
    }
    config.env.push((TRANSPORT_ENV.into(), transport));
}

/// Hub settings come from the user-level `~/.cas/config.toml`, overridden
/// field by field by the project; `enabled` counts only from the project.
fn load(cas_root: Option<&Path>) -> FactoryGatewayConfig {
    let Some(root) = cas_root else { return FactoryGatewayConfig::default() };
    let project = read_table(&root.join("config.toml"));
    if !project.enabled {
        return FactoryGatewayConfig::default();
    }
    let user = dirs::home_dir()
        .map(|home| read_table(&home.join(".cas/config.toml")))
        .unwrap_or_default();
    FactoryGatewayConfig {
        enabled: true,
        endpoint: project.endpoint.or(user.endpoint),
        key_file: project.key_file.or(user.key_file),
        clients: project.clients.or(user.clients),
    }
}

fn read_table(path: &Path) -> FactoryGatewayConfig {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Factory {
        gateway: FactoryGatewayConfig,
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Document {
        factory: Factory,
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| toml::from_str::<Document>(&raw).ok())
        .map(|document| document.factory.gateway)
        .unwrap_or_default()
}

fn plan(settings: &FactoryGatewayConfig, cli: &str, probe: impl Fn(&str, &str) -> Result<(), String>) -> Plan {
    if !settings.enabled || !matches!(cli, "claude" | "codex") {
        return Plan::Off;
    }
    let routed = settings.clients.as_ref().map_or(true, |clients| clients.iter().any(|c| c.trim() == cli));
    if !routed {
        return Plan::Off;
    }
    let ready = || -> Result<Plan, String> {
        let endpoint = validate_endpoint(settings.endpoint.as_deref().unwrap_or_default())?;
        let key_file = settings
            .key_file
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(|path| PathBuf::from(cas_pty::expand_codex_home(path)))
            .ok_or("factory.gateway.key_file is not set")?;
        let key = read_gateway_key(&key_file)?;
        probe(&endpoint, &key)?;
        Ok(Plan::Gateway { endpoint, key_file })
    };
    ready().unwrap_or_else(Plan::Fallback)
}

/// Accept only loopback and tailnet hubs, normalized without a trailing `/`.
pub fn validate_endpoint(raw: &str) -> Result<String, String> {
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Err("factory.gateway.endpoint is not set".into());
    }
    let url = url::Url::parse(raw).map_err(|error| format!("gateway endpoint is not a URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("gateway endpoint must be a plain http(s) URL".into());
    }
    let private = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || (ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback() || ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
        Some(url::Host::Domain(host)) => {
            let host = host.to_ascii_lowercase();
            host == "localhost" || (host.ends_with(".ts.net") && host.len() > ".ts.net".len())
        }
        None => false,
    };
    if !private {
        return Err("gateway endpoint must be loopback or tailnet (100.64.0.0/10, *.ts.net)".into());
    }
    Ok(raw.to_string())
}

/// Readiness: the hub's model listing must answer 200 for this key.
fn probe(endpoint: &str, key: &str) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .redirects(0)
        .build();
    match agent
        .get(&format!("{endpoint}/v1/models"))
        .set("Authorization", &format!("Bearer {key}"))
        .call()
    {
        Ok(response) if response.status() == 200 => Ok(()),
        Ok(response) => Err(format!("hub readiness answered {}", response.status())),
        Err(ureq::Error::Status(code @ (401 | 403), _)) => Err(format!("hub refused the client key ({code})")),
        Err(ureq::Error::Status(code, _)) => Err(format!("hub readiness answered {code}")),
        Err(ureq::Error::Transport(error)) => Err(format!("hub unreachable ({})", error.kind())),
    }
}

#[cfg(test)]
#[path = "gateway_tests.rs"]
mod tests;
