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
/// A session's pinned worker route (`gateway` or `direct`), set on the
/// factory daemon by Commander's New Session, or by [`pin_at_start`] for any
/// other start.
pub const ROUTE_ENV: &str = "CAS_FACTORY_GATEWAY";

/// The route a daemon fixed at startup when nothing set [`ROUTE_ENV`].
static STARTUP_ROUTE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// A factory daemon fixes its worker route once, when it starts. Commander's
/// launch already set [`ROUTE_ENV`]; any other start (`cas claude`, `cas
/// factory`) takes the project's setting now, so a later config edit never
/// changes this session's workers. The daemon's re-exec carries
/// [`pinned_route`] forward in [`ROUTE_ENV`].
pub fn pin_at_start(cas_root: &Path) {
    if std::env::var_os(ROUTE_ENV).is_none() {
        STARTUP_ROUTE.get_or_init(|| if merged(cas_root).enabled { "gateway" } else { "direct" });
    }
}

/// This process's pinned worker route: Commander's, else the one fixed at startup.
pub fn pinned_route() -> Option<String> {
    std::env::var(ROUTE_ENV).ok().or_else(|| STARTUP_ROUTE.get().map(|route| route.to_string()))
}
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
    let pin = pinned_route();
    let settings = load(cas_root.map(PathBuf::as_path), pin.as_deref());
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
            // The hub's address stays in the request config only: never logged or recorded (operator, 6:46 PM).
            tracing::info!(worker, cli, "factory worker gateway transport");
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
            "gateway".to_string()
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
/// field by field by the project. A session pin decides `enabled`; without
/// one, only the project's own `enabled` counts.
fn load(cas_root: Option<&Path>, pin: Option<&str>) -> FactoryGatewayConfig {
    let Some(root) = cas_root else { return FactoryGatewayConfig::default() };
    let settings = merged(root);
    let enabled = match pin.map(str::trim) {
        Some("gateway") => true,
        Some("direct") => false,
        _ => settings.enabled,
    };
    if !enabled {
        return FactoryGatewayConfig::default();
    }
    FactoryGatewayConfig { enabled, ..settings }
}

fn merged(root: &Path) -> FactoryGatewayConfig {
    let project = read_table(&root.join("config.toml"));
    let user = dirs::home_dir()
        .map(|home| read_table(&home.join(".cas/config.toml")))
        .unwrap_or_default();
    FactoryGatewayConfig {
        enabled: project.enabled,
        endpoint: project.endpoint.or(user.endpoint),
        key_file: project.key_file.or(user.key_file),
        clients: project.clients.or(user.clients),
        // The dashboard is user-level only.
        dashboard: user.dashboard,
        dashboard_key_file: user.dashboard_key_file,
    }
}

/// What New Session may offer for a project: the route preselected by the
/// project's own setting, the routed worker harnesses, and whether the hub is
/// ready now. Never carries the endpoint or key.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SessionRouteOption {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub preselected: &'static str,
    pub clients: Vec<String>,
}

pub fn session_route_option(cas_root: &Path) -> SessionRouteOption {
    let settings = merged(cas_root);
    let clients = ["claude", "codex"]
        .into_iter()
        .filter(|cli| settings.clients.as_ref().is_none_or(|clients| clients.iter().any(|c| c.trim() == *cli)))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let readiness = if clients.is_empty() {
        Err("factory.gateway.clients routes no Claude or Codex workers".to_string())
    } else {
        ready(&settings, probe).map(|_| ())
    };
    SessionRouteOption {
        available: readiness.is_ok(),
        reason: readiness.err(),
        preselected: if settings.enabled { "gateway" } else { "direct" },
        clients,
    }
}

/// The machine's own hub settings (`~/.cas/config.toml`), independent of any project.
pub fn user_settings() -> FactoryGatewayConfig {
    dirs::home_dir().map(|home| read_table(&home.join(".cas/config.toml"))).unwrap_or_default()
}

/// The same spawn-time readiness check workers get: private endpoint, 0600
/// key file, and a 200 model listing for that key.
pub fn readiness(settings: &FactoryGatewayConfig) -> Result<(), String> {
    ready(settings, probe).map(|_| ())
}

/// `loopback` or `tailnet` for a validated endpoint, so callers can say where
/// the hub is without showing its address.
pub fn host_kind(endpoint: &str) -> Option<&'static str> {
    let host = url::Url::parse(validate_endpoint(endpoint).ok()?.as_str()).ok()?.host()?.to_owned();
    Some(match host {
        url::Host::Ipv4(ip) if ip.is_loopback() => "loopback",
        url::Host::Ipv6(ip) if ip.is_loopback() => "loopback",
        url::Host::Domain(name) if name.eq_ignore_ascii_case("localhost") => "loopback",
        _ => "tailnet",
    })
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
    let routed = settings.clients.as_ref().is_none_or(|clients| clients.iter().any(|c| c.trim() == cli));
    if !routed {
        return Plan::Off;
    }
    ready(settings, probe).unwrap_or_else(Plan::Fallback)
}

fn ready(settings: &FactoryGatewayConfig, probe: impl Fn(&str, &str) -> Result<(), String>) -> Result<Plan, String> {
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
