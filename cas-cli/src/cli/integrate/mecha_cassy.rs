//! `cas integrate mecha-cassy` — one command that makes the MechaCassy Slack
//! hub reachable from every project on a machine (task **cas-8fad**).
//!
//! ## What this replaces
//!
//! Before this command, reaching the hub meant hand-copying a gitignored
//! `.cas/proxy.toml` into each project plus hand-editing two harness config
//! files, so a second machine or a new teammate silently had no Slack path.
//! This command writes the registration **once, at machine scope**
//! (`~/.config/code-mode-mcp/config.toml`), which
//! [`cmcp_core::config::Config::load_merged`] already merges beneath a project
//! `.cas/proxy.toml` and `cas serve` already loads.
//!
//! ## Credential rule
//!
//! Every artifact this module writes references a credential by environment
//! variable **name** (`auth = "env:MECHA_SLACK_TOKEN_<LABEL>"`,
//! `x-vercel-protection-bypass = "env:MECHA_VERCEL_BYPASS"`). A token value is
//! never read into a report, never printed, never written to disk, and never
//! embedded in an error. The only fact this module publishes about a variable
//! is [`EnvState`] — set, empty, or unset.
//!
//! ## Seams
//!
//! [`EnvLookup`] and [`HubProbe`] are traits so the command and the doctor
//! check are exercised against a fake environment and a fake `tools/list`
//! rather than a live hub. [`MachinePaths`] is passed in for the same reason:
//! a test points it at a `tempdir` and asserts on real written files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;

use cmcp_core::config::{
    Config as ProxyConfig, MECHA_CASSY_BYPASS_HEADER, MECHA_CASSY_DEFAULT_BYPASS_ENV,
    MECHA_CASSY_DEFAULT_TOKEN_ENV, MECHA_CASSY_MCP_URL, MECHA_CASSY_SERVER, MECHA_CASSY_TOOLS,
};

use super::fs as ifs;
use super::types::{IntegrationAction, IntegrationOutcome, IntegrationStatus, Platform};

/// Where an operator is told to put the two values. Named in every remedy so
/// the message is actionable without opening the onboarding doc.
pub const CREDENTIALS_HINT: &str =
    "add both to your machine credentials file (the file your login shell exports, e.g. \
     ~/.config/cas/credentials.env) and start a new shell; the hub admin mints \
     MECHA_SLACK_TOKEN_<LABEL> per machine — see docs/MECHA_CASSY_ONBOARDING.md";

// ---------------------------------------------------------------------------
// CLI surface
// ---------------------------------------------------------------------------

#[derive(Args, Debug, Clone, Default)]
pub struct MechaCassyArgs {
    /// Environment variable holding this machine's hub bearer token.
    /// Defaults to `MECHA_SLACK_TOKEN_<LABEL>`.
    #[arg(long, value_name = "NAME")]
    pub token_env: Option<String>,
    /// Environment variable holding the Vercel edge-protection bypass secret.
    #[arg(long, value_name = "NAME", default_value = MECHA_CASSY_DEFAULT_BYPASS_ENV)]
    pub bypass_env: String,
    /// Per-machine client label the hub admin minted a token for (e.g. `LAPTOP`).
    #[arg(long, value_name = "LABEL")]
    pub label: Option<String>,
    /// Hub MCP endpoint. Only needed against a staging hub.
    #[arg(long, value_name = "URL", default_value = MECHA_CASSY_MCP_URL)]
    pub url: String,
    /// Leave the Claude Code and Codex MCP registrations alone.
    #[arg(long)]
    pub no_harness: bool,
    /// Skip the authenticated `tools/list` receipt (offline setup).
    #[arg(long)]
    pub skip_verify: bool,
    /// Report what would change without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

impl MechaCassyArgs {
    /// Bearer variable name: `--token-env` wins, then `--label`, then the
    /// default proxy label. A label is upper-cased and non-alphanumerics are
    /// folded to `_` so `my laptop` and `my-laptop` name the same variable.
    pub fn resolved_token_env(&self) -> String {
        if let Some(explicit) = self.token_env.as_deref().map(str::trim)
            && !explicit.is_empty()
        {
            return explicit.to_string();
        }
        match self.label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            Some(label) => format!("MECHA_SLACK_TOKEN_{}", sanitize_label(label)),
            None => MECHA_CASSY_DEFAULT_TOKEN_ENV.to_string(),
        }
    }
}

fn sanitize_label(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Seams
// ---------------------------------------------------------------------------

/// Read-only view of the process environment. Implementations return the
/// *value* only so emptiness can be distinguished from absence; callers must
/// reduce it to an [`EnvState`] before it reaches a report or the terminal.
pub trait EnvLookup {
    fn get(&self, name: &str) -> Option<String>;
}

pub struct ProcessEnv;

impl EnvLookup for ProcessEnv {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// Whether a named credential variable is usable, without revealing its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvState {
    Set,
    /// Exported but empty — a distinct and common failure (a truncated
    /// credentials file line) that must not be reported as "unset".
    Empty,
    Unset,
}

impl EnvState {
    pub fn of(env: &dyn EnvLookup, name: &str) -> Self {
        match env.get(name) {
            Some(value) if !value.trim().is_empty() => Self::Set,
            Some(_) => Self::Empty,
            None => Self::Unset,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Empty => "set but empty",
            Self::Unset => "unset",
        }
    }

    pub fn is_usable(self) -> bool {
        matches!(self, Self::Set)
    }
}

/// Outcome of an authenticated `tools/list` against the hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum ProbeOutcome {
    /// The hub answered with this exact tool list.
    Tools { tools: Vec<String> },
    /// HTTP 401. Reported with header state only, never a value.
    Unauthorized,
    /// Any other transport failure, carrying the proxy's error code.
    Unreachable { code: String },
    /// Not attempted (`--skip-verify`, or a credential is missing).
    Skipped { reason: String },
}

/// An authenticated `tools/list` against the hub.
pub trait HubProbe {
    fn list_tools(&self, url: &str, token_env: &str, bypass_env: &str) -> ProbeOutcome;
}

/// Live probe: builds the *same* env-referencing server config that is written
/// to disk and lets the proxy resolve both credentials in-process.
pub struct ProxyHubProbe;

impl HubProbe for ProxyHubProbe {
    fn list_tools(&self, url: &str, token_env: &str, bypass_env: &str) -> ProbeOutcome {
        use std::collections::HashMap;

        let server = cmcp_core::config::ServerConfig::Http {
            url: url.to_string(),
            auth: Some(format!("env:{token_env}")),
            headers: HashMap::from([(
                MECHA_CASSY_BYPASS_HEADER.to_string(),
                format!("env:{bypass_env}"),
            )]),
            oauth: false,
        };
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                return ProbeOutcome::Unreachable {
                    code: format!("runtime_unavailable: {error}"),
                };
            }
        };
        runtime.block_on(async move {
            let engine = match cmcp_core::ProxyEngine::from_configs(HashMap::from([(
                MECHA_CASSY_SERVER.to_string(),
                server,
            )]))
            .await
            {
                Ok(engine) => engine,
                Err(error) => {
                    return ProbeOutcome::Unreachable {
                        code: format!("{error}"),
                    };
                }
            };
            let health = engine.health_snapshot().await;
            let record = health
                .servers
                .iter()
                .find(|server| server.name == MECHA_CASSY_SERVER);
            let outcome = match record {
                Some(server) if server.state == cmcp_core::UpstreamState::Healthy => {
                    let catalog = engine.catalog_entries_by_server().await;
                    let tools = catalog
                        .get(MECHA_CASSY_SERVER)
                        .map(|entries| entries.iter().map(|e| e.name.clone()).collect())
                        .unwrap_or_default();
                    ProbeOutcome::Tools { tools }
                }
                Some(server) => match server.last_error_code.as_deref() {
                    Some("authentication_required") => ProbeOutcome::Unauthorized,
                    Some(code) => ProbeOutcome::Unreachable {
                        code: code.to_string(),
                    },
                    None => ProbeOutcome::Unreachable {
                        code: "connection_failed".to_string(),
                    },
                },
                None => ProbeOutcome::Unreachable {
                    code: "not_configured".to_string(),
                },
            };
            engine.shutdown().await;
            outcome
        })
    }
}

// ---------------------------------------------------------------------------
// Machine paths
// ---------------------------------------------------------------------------

/// The three machine-scoped files this command owns.
#[derive(Debug, Clone)]
pub struct MachinePaths {
    /// User-level proxy registration, merged beneath any project `.cas/proxy.toml`.
    pub user_proxy: PathBuf,
    /// `<CLAUDE_CONFIG_DIR|$HOME>/.claude.json`.
    pub claude_json: Option<PathBuf>,
    /// `<CODEX_HOME|$HOME/.codex>/config.toml`.
    pub codex_config: Option<PathBuf>,
}

impl MachinePaths {
    /// Resolve from the environment, honouring the per-account overrides the
    /// factory already sets (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`) so a spawned
    /// worker registers into the account it is actually running as.
    pub fn from_env(env: &dyn EnvLookup) -> Result<Self> {
        let user_proxy = cmcp_core::config::Scope::User
            .config_path()
            .context("could not determine the user MCP configuration path")?;
        let home = env.get("HOME").map(PathBuf::from);
        let claude_dir = env
            .get("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| home.clone());
        let codex_dir = env
            .get("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".codex")));
        Ok(Self {
            user_proxy,
            claude_json: claude_dir.map(|d| d.join(".claude.json")),
            codex_config: codex_dir.map(|d| d.join("config.toml")),
        })
    }
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WriteState {
    /// The file did not describe this registration; it now does.
    Written,
    /// Already byte-identical in effect; nothing was rewritten.
    AlreadyCurrent,
    /// Would be written, but `--dry-run` was requested.
    Planned,
    /// Deliberately not touched (`--no-harness`, or the path is unknown).
    Skipped,
}

impl WriteState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::AlreadyCurrent => "already current",
            Self::Planned => "planned (dry run)",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HarnessEntry {
    pub harness: String,
    pub path: Option<PathBuf>,
    pub state: WriteState,
    /// Present when a harness registration could not be attempted.
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MechaCassyReport {
    pub url: String,
    pub token_env: String,
    pub bypass_env: String,
    pub token_env_state: EnvState,
    pub bypass_env_state: EnvState,
    pub registration_path: PathBuf,
    pub registration: WriteState,
    /// Routes the machine registration now admits.
    pub allowlist: Vec<String>,
    pub harnesses: Vec<HarnessEntry>,
    pub probe: ProbeOutcome,
    /// How the hub's live tool list disagrees with the allowlist, if at all.
    pub drift: ToolDrift,
    /// Exact next command or edit, when the operator must do something.
    pub remedy: Option<String>,
}

impl MechaCassyReport {
    pub fn credentials_ready(&self) -> bool {
        self.token_env_state.is_usable() && self.bypass_env_state.is_usable()
    }

    /// Green means: both variables usable, the registration is on disk, and
    /// the hub answered with exactly the allowlisted tools. A skipped probe is
    /// deliberately *not* green — an unverified setup has never been proven.
    pub fn is_green(&self) -> bool {
        self.credentials_ready()
            && matches!(
                self.registration,
                WriteState::Written | WriteState::AlreadyCurrent
            )
            && self.drift.is_empty()
            && matches!(&self.probe, ProbeOutcome::Tools { .. })
    }
}

// ---------------------------------------------------------------------------
// Core
// ---------------------------------------------------------------------------

/// Two very different disagreements between the hub and the allowlist.
///
/// They are kept apart because their consequences differ. A tool the hub
/// offers that no route admits means **every call to it is denied by policy**
/// — the release post fails. An allowlisted name the hub no longer offers is
/// **inert**: dispatch of the live tools still works, the entry is merely
/// stale. Collapsing the two would either cry wolf over harmless clutter or
/// bury a genuine outage inside a cosmetic one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ToolDrift {
    /// Live hub tools with no allowlist route. Blocks dispatch.
    pub unallowlisted: Vec<String>,
    /// Allowlisted routes the hub no longer offers. Inert but stale.
    pub retired: Vec<String>,
}

impl ToolDrift {
    pub fn is_empty(&self) -> bool {
        self.unallowlisted.is_empty() && self.retired.is_empty()
    }

    /// True when something the operator needs is unreachable right now.
    pub fn blocks_dispatch(&self) -> bool {
        !self.unallowlisted.is_empty()
    }

    pub fn describe(&self, live: &[String], allowlisted: &[String]) -> String {
        let mut parts = Vec::new();
        if !self.unallowlisted.is_empty() {
            parts.push(format!(
                "hub offers un-allowlisted {} — calls to it are denied by policy",
                self.unallowlisted.join(", ")
            ));
        }
        if !self.retired.is_empty() {
            parts.push(format!(
                "allowlist still names retired {}",
                self.retired.join(", ")
            ));
        }
        format!(
            "hub tool contract drifted: {} (hub: [{}]; allowlist: [{}])",
            parts.join("; "),
            sorted_unique(live).join(", "),
            sorted_unique(allowlisted).join(", ")
        )
    }
}

fn sorted_unique(values: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = values.iter().map(String::as_str).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Compare the hub's live tool list against the routes the registration
/// admits. Order-insensitive, so re-ordering a file is never reported as drift.
pub fn tool_drift(allowlisted: &[String], live: &[String]) -> ToolDrift {
    let expected = sorted_unique(allowlisted);
    let actual = sorted_unique(live);
    ToolDrift {
        unallowlisted: actual
            .iter()
            .filter(|tool| !expected.contains(tool))
            .map(|tool| (*tool).to_string())
            .collect(),
        retired: expected
            .iter()
            .filter(|tool| !actual.contains(tool))
            .map(|tool| (*tool).to_string())
            .collect(),
    }
}

/// Run the integration. Pure with respect to its seams: every filesystem
/// write goes through `paths`, every credential fact through `env`, and the
/// only network call through `probe`.
pub fn run(
    args: &MechaCassyArgs,
    paths: &MachinePaths,
    env: &dyn EnvLookup,
    probe: &dyn HubProbe,
) -> Result<MechaCassyReport> {
    let token_env = args.resolved_token_env();
    let bypass_env = args.bypass_env.trim().to_string();
    anyhow::ensure!(
        !token_env.is_empty() && !bypass_env.is_empty(),
        "--token-env and --bypass-env must name environment variables"
    );

    let token_env_state = EnvState::of(env, &token_env);
    let bypass_env_state = EnvState::of(env, &bypass_env);
    let credentials_ready = token_env_state.is_usable() && bypass_env_state.is_usable();

    // The registration is written even when a variable is missing: it names
    // variables, holds no secret, and having it on disk is what makes the
    // remedy a one-line credentials-file edit instead of a second setup pass.
    let mut config = ProxyConfig::load_from(&paths.user_proxy)
        .with_context(|| format!("reading {}", paths.user_proxy.display()))?;
    let changed = config.ensure_mecha_cassy_registration(&args.url, &token_env, &bypass_env);
    let registration = if !changed {
        WriteState::AlreadyCurrent
    } else if args.dry_run {
        WriteState::Planned
    } else {
        config
            .save_to(&paths.user_proxy)
            .with_context(|| format!("writing {}", paths.user_proxy.display()))?;
        WriteState::Written
    };
    let allowlist = config.mecha_cassy_allowlisted_tools();

    let harnesses = if args.no_harness {
        vec![
            HarnessEntry {
                harness: "claude-code".to_string(),
                path: paths.claude_json.clone(),
                state: WriteState::Skipped,
                note: Some("--no-harness".to_string()),
            },
            HarnessEntry {
                harness: "codex".to_string(),
                path: paths.codex_config.clone(),
                state: WriteState::Skipped,
                note: Some("--no-harness".to_string()),
            },
        ]
    } else {
        vec![
            register_claude(
                paths.claude_json.as_deref(),
                &args.url,
                &token_env,
                &bypass_env,
                args.dry_run,
            ),
            register_codex(
                paths.codex_config.as_deref(),
                &args.url,
                &token_env,
                &bypass_env,
                args.dry_run,
            ),
        ]
    };

    let probe_outcome = if args.skip_verify {
        ProbeOutcome::Skipped {
            reason: "--skip-verify".to_string(),
        }
    } else if !credentials_ready {
        ProbeOutcome::Skipped {
            reason: format!(
                "{token_env} is {} and {bypass_env} is {}",
                token_env_state.as_str(),
                bypass_env_state.as_str()
            ),
        }
    } else if args.dry_run {
        ProbeOutcome::Skipped {
            reason: "--dry-run".to_string(),
        }
    } else {
        probe.list_tools(&args.url, &token_env, &bypass_env)
    };

    // After a successful write the allowlist is exactly the constant, so any
    // drift here means the hub itself moved — worth reporting either way.
    let (drift, drift_message) = match &probe_outcome {
        ProbeOutcome::Tools { tools } => {
            let drift = tool_drift(&allowlist, tools);
            let message = (!drift.is_empty()).then(|| drift.describe(tools, &allowlist));
            (drift, message)
        }
        _ => (ToolDrift::default(), None),
    };

    let remedy = build_remedy(
        &token_env,
        token_env_state,
        &bypass_env,
        bypass_env_state,
        &probe_outcome,
        drift_message.as_deref(),
    );

    Ok(MechaCassyReport {
        url: args.url.clone(),
        token_env,
        bypass_env,
        token_env_state,
        bypass_env_state,
        registration_path: paths.user_proxy.clone(),
        registration,
        allowlist,
        harnesses,
        probe: probe_outcome,
        drift,
        remedy,
    })
}

fn build_remedy(
    token_env: &str,
    token_state: EnvState,
    bypass_env: &str,
    bypass_state: EnvState,
    probe: &ProbeOutcome,
    drift: Option<&str>,
) -> Option<String> {
    let mut missing = Vec::new();
    if !token_state.is_usable() {
        missing.push(format!("{token_env} ({})", token_state.as_str()));
    }
    if !bypass_state.is_usable() {
        missing.push(format!("{bypass_env} ({})", bypass_state.as_str()));
    }
    if !missing.is_empty() {
        return Some(format!("Set {}; {CREDENTIALS_HINT}", missing.join(" and ")));
    }
    if let Some(drift) = drift {
        return Some(format!(
            "{drift}. Re-run `cas integrate mecha-cassy` to rewrite the allowlist against the \
             hub's current contract."
        ));
    }
    match probe {
        ProbeOutcome::Unauthorized => Some(format!(
            "The hub rejected this machine's bearer (HTTP 401; Authorization: Bearer <set>). Ask \
             the hub admin to mint or re-register a token for this label, then re-export \
             {token_env} and run `cas integrate mecha-cassy` again."
        )),
        ProbeOutcome::Unreachable { code } => Some(format!(
            "The hub did not answer ({code}). Check connectivity, then re-run \
             `cas integrate mecha-cassy`."
        )),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Harness registration
// ---------------------------------------------------------------------------

/// Claude Code: a user-scope HTTP server in the selected profile's
/// `.claude.json`. `${VAR}` is expanded by the client at launch, so this stays
/// a name reference. The whole document is round-tripped through
/// `serde_json::Value`, which preserves every unrelated key (project history,
/// onboarding state) rather than rewriting the file from a partial model.
fn register_claude(
    path: Option<&Path>,
    url: &str,
    token_env: &str,
    bypass_env: &str,
    dry_run: bool,
) -> HarnessEntry {
    let Some(path) = path else {
        return HarnessEntry {
            harness: "claude-code".to_string(),
            path: None,
            state: WriteState::Skipped,
            note: Some("neither CLAUDE_CONFIG_DIR nor HOME is set".to_string()),
        };
    };
    match apply_claude(path, url, token_env, bypass_env, dry_run) {
        Ok(state) => HarnessEntry {
            harness: "claude-code".to_string(),
            path: Some(path.to_path_buf()),
            state,
            note: None,
        },
        Err(error) => HarnessEntry {
            harness: "claude-code".to_string(),
            path: Some(path.to_path_buf()),
            state: WriteState::Skipped,
            note: Some(format!("{error:#}")),
        },
    }
}

fn apply_claude(
    path: &Path,
    url: &str,
    token_env: &str,
    bypass_env: &str,
    dry_run: bool,
) -> Result<WriteState> {
    let mut document: serde_json::Value = if ifs::is_regular_file(path) {
        let raw = ifs::read_capped(path)?;
        if raw.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&raw)
                .with_context(|| format!("parsing {}", path.display()))?
        }
    } else if path.exists() {
        anyhow::bail!("{} is not a regular file", path.display());
    } else {
        serde_json::json!({})
    };

    if !document.is_object() {
        anyhow::bail!("{} is not a JSON object", path.display());
    }
    let desired = serde_json::json!({
        "type": "http",
        "url": url,
        "headers": {
            "Authorization": format!("Bearer ${{{token_env}}}"),
            MECHA_CASSY_BYPASS_HEADER: format!("${{{bypass_env}}}"),
        }
    });
    let servers = document
        .as_object_mut()
        .expect("checked above")
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        anyhow::bail!("{}: mcpServers is not an object", path.display());
    }
    if servers.get(MECHA_CASSY_SERVER) == Some(&desired) {
        return Ok(WriteState::AlreadyCurrent);
    }
    if dry_run {
        return Ok(WriteState::Planned);
    }
    servers
        .as_object_mut()
        .expect("checked above")
        .insert(MECHA_CASSY_SERVER.to_string(), desired);
    let serialized = serde_json::to_string_pretty(&document)?;
    ifs::atomic_write_create_dirs(path, &format!("{serialized}\n"))?;
    Ok(WriteState::Written)
}

/// Codex: an `[mcp_servers.mecha-cassy]` table naming the bearer variable.
/// Edited with `toml_edit` so an operator's 3000-line `config.toml` keeps its
/// comments, ordering, and every unrelated table.
fn register_codex(
    path: Option<&Path>,
    url: &str,
    token_env: &str,
    bypass_env: &str,
    dry_run: bool,
) -> HarnessEntry {
    let Some(path) = path else {
        return HarnessEntry {
            harness: "codex".to_string(),
            path: None,
            state: WriteState::Skipped,
            note: Some("neither CODEX_HOME nor HOME is set".to_string()),
        };
    };
    match apply_codex(path, url, token_env, bypass_env, dry_run) {
        Ok(state) => HarnessEntry {
            harness: "codex".to_string(),
            path: Some(path.to_path_buf()),
            state,
            note: None,
        },
        Err(error) => HarnessEntry {
            harness: "codex".to_string(),
            path: Some(path.to_path_buf()),
            state: WriteState::Skipped,
            note: Some(format!("{error:#}")),
        },
    }
}

fn apply_codex(
    path: &Path,
    url: &str,
    token_env: &str,
    bypass_env: &str,
    dry_run: bool,
) -> Result<WriteState> {
    let raw = if ifs::is_regular_file(path) {
        ifs::read_capped(path)?
    } else if path.exists() {
        anyhow::bail!("{} is not a regular file", path.display());
    } else {
        String::new()
    };
    let mut document: toml_edit::DocumentMut = raw
        .parse()
        .with_context(|| format!("parsing {}", path.display()))?;

    let existing = document
        .get("mcp_servers")
        .and_then(|servers| servers.get(MECHA_CASSY_SERVER));
    let current_matches = existing.is_some_and(|table| {
        table.get("url").and_then(|v| v.as_str()) == Some(url)
            && table
                .get("bearer_token_env_var")
                .and_then(|v| v.as_str())
                == Some(token_env)
            && table
                .get("env_http_headers")
                .and_then(|headers| headers.get(MECHA_CASSY_BYPASS_HEADER))
                .and_then(|v| v.as_str())
                == Some(bypass_env)
    });
    if current_matches {
        return Ok(WriteState::AlreadyCurrent);
    }
    if dry_run {
        return Ok(WriteState::Planned);
    }

    let servers = document["mcp_servers"].or_insert(toml_edit::table());
    if let Some(table) = servers.as_table_mut() {
        table.set_implicit(true);
    }
    let mut headers = toml_edit::InlineTable::new();
    headers.insert(MECHA_CASSY_BYPASS_HEADER, bypass_env.into());
    let entry = servers[MECHA_CASSY_SERVER].or_insert(toml_edit::table());
    entry["url"] = toml_edit::value(url);
    entry["bearer_token_env_var"] = toml_edit::value(token_env);
    entry["env_http_headers"] = toml_edit::value(headers);

    ifs::atomic_write_create_dirs(path, &document.to_string())?;
    Ok(WriteState::Written)
}

// ---------------------------------------------------------------------------
// Doctor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorSeverity {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorRow {
    pub severity: DoctorSeverity,
    pub message: String,
}

/// Read-only status for `cas doctor`. Never writes, never prompts.
///
/// `project_proxy` is the project's `.cas/proxy.toml` when it exists. Because
/// a project file *replaces* rather than widens the machine allowlist, a
/// project that declares its own policy and omits the hub routes is reported
/// as the concrete failure it is, with the routes to add.
pub fn doctor_row(
    project_proxy: Option<&Path>,
    paths: &MachinePaths,
    env: &dyn EnvLookup,
    probe: &dyn HubProbe,
) -> DoctorRow {
    let merged = match ProxyConfig::load_merged_with_sources_from(
        Some(&paths.user_proxy),
        project_proxy,
    ) {
        Ok((config, _)) => config,
        Err(error) => {
            return DoctorRow {
                severity: DoctorSeverity::Error,
                message: format!(
                    "proxy configuration could not be read ({error:#}). Repair it, then run \
                     `cas integrate mecha-cassy`"
                ),
            };
        }
    };

    let Some((token_env, bypass_env)) = merged.mecha_cassy_env_names() else {
        return DoctorRow {
            severity: DoctorSeverity::Warning,
            message: format!(
                "not registered on this machine ({} has no {MECHA_CASSY_SERVER} server). Run \
                 `cas integrate mecha-cassy`",
                paths.user_proxy.display()
            ),
        };
    };
    let Some(token_env) = token_env else {
        return DoctorRow {
            severity: DoctorSeverity::Error,
            message: format!(
                "the {MECHA_CASSY_SERVER} registration does not reference its bearer by \
                 environment-variable name. Run `cas integrate mecha-cassy` to rewrite it as an \
                 env: reference"
            ),
        };
    };
    let bypass_env = bypass_env.unwrap_or_else(|| MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string());

    let token_state = EnvState::of(env, &token_env);
    let bypass_state = EnvState::of(env, &bypass_env);
    let allowlist = merged.mecha_cassy_allowlisted_tools();

    if allowlist.is_empty() {
        let where_from = project_proxy
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| paths.user_proxy.display().to_string());
        return DoctorRow {
            severity: DoctorSeverity::Error,
            message: format!(
                "{token_env} is {}, but no {MECHA_CASSY_SERVER} route is allowlisted: {where_from} \
                 is authoritative for dispatch policy and names none. Run `cas integrate \
                 mecha-cassy` for a machine without a project proxy file, or add {} to that \
                 file's allowlist",
                token_state.as_str(),
                MECHA_CASSY_TOOLS
                    .iter()
                    .map(|tool| format!("\"{MECHA_CASSY_SERVER}.{tool}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
    }

    if !token_state.is_usable() || !bypass_state.is_usable() {
        let mut missing = Vec::new();
        if !token_state.is_usable() {
            missing.push(format!("{token_env} is {}", token_state.as_str()));
        }
        if !bypass_state.is_usable() {
            missing.push(format!("{bypass_env} is {}", bypass_state.as_str()));
        }
        return DoctorRow {
            severity: DoctorSeverity::Error,
            message: format!("{}; {CREDENTIALS_HINT}", missing.join(", ")),
        };
    }

    match probe.list_tools(MECHA_CASSY_MCP_URL, &token_env, &bypass_env) {
        ProbeOutcome::Tools { tools } => {
            let drift = tool_drift(&allowlist, &tools);
            if drift.is_empty() {
                DoctorRow {
                    severity: DoctorSeverity::Ok,
                    message: format!(
                        "registered ({token_env} set, {bypass_env} set); hub answered with {} tool(s): {}",
                        tools.len(),
                        tools.join(", ")
                    ),
                }
            } else {
                DoctorRow {
                    // A hub tool nothing admits is a live outage; a stale entry
                    // for a tool the hub retired is only clutter, so it must
                    // not turn `cas doctor` red for a machine that can post
                    // perfectly well right now.
                    severity: if drift.blocks_dispatch() {
                        DoctorSeverity::Error
                    } else {
                        DoctorSeverity::Warning
                    },
                    message: format!(
                        "{}. Run `cas integrate mecha-cassy` to rewrite the allowlist",
                        drift.describe(&tools, &allowlist)
                    ),
                }
            }
        }
        ProbeOutcome::Unauthorized => DoctorRow {
            severity: DoctorSeverity::Error,
            message: format!(
                "hub rejected this machine (HTTP 401; Authorization: Bearer <set>). Ask the hub \
                 admin to re-mint the token for this label, re-export {token_env}, then run \
                 `cas integrate mecha-cassy`"
            ),
        },
        ProbeOutcome::Unreachable { code } => DoctorRow {
            severity: DoctorSeverity::Warning,
            message: format!(
                "registered, but the hub did not answer ({code}); run `cas integrate mecha-cassy` \
                 once connectivity is back"
            ),
        },
        ProbeOutcome::Skipped { reason } => DoctorRow {
            severity: DoctorSeverity::Warning,
            message: format!("registered, but not verified ({reason})"),
        },
    }
}

// ---------------------------------------------------------------------------
// Command entry point
// ---------------------------------------------------------------------------

pub fn execute(args: &MechaCassyArgs, json: bool) -> Result<IntegrationOutcome> {
    let env = ProcessEnv;
    let paths = MachinePaths::from_env(&env)?;
    let report = run(args, &paths, &env, &ProxyHubProbe)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }

    let status = if !report.credentials_ready() || !report.drift.is_empty() {
        IntegrationStatus::Stale
    } else {
        match (&report.probe, report.registration) {
            (ProbeOutcome::Unauthorized, _) | (ProbeOutcome::Unreachable { .. }, _) => {
                IntegrationStatus::TransportError
            }
            (_, WriteState::AlreadyCurrent) => IntegrationStatus::AlreadyConfigured,
            _ => IntegrationStatus::Configured,
        }
    };

    let mut outcome = IntegrationOutcome::new(
        Platform::MechaCassy,
        IntegrationAction::Init,
        status,
    );
    outcome.summary.push(format!("hub: {}", report.url));
    outcome.summary.push(format!(
        "credentials: {} {}, {} {}",
        report.token_env,
        report.token_env_state.as_str(),
        report.bypass_env,
        report.bypass_env_state.as_str()
    ));
    outcome.summary.push(format!(
        "machine registration: {} ({})",
        report.registration.as_str(),
        report.registration_path.display()
    ));
    outcome
        .summary
        .push(format!("allowlist: {}", report.allowlist.join(", ")));
    for harness in &report.harnesses {
        outcome.summary.push(format!(
            "{}: {}{}",
            harness.harness,
            harness.state.as_str(),
            harness
                .note
                .as_deref()
                .map(|note| format!(" ({note})"))
                .unwrap_or_default()
        ));
    }
    match &report.probe {
        ProbeOutcome::Tools { tools } => outcome.summary.push(format!(
            "authenticated tools/list: {} tool(s): {}",
            tools.len(),
            tools.join(", ")
        )),
        ProbeOutcome::Unauthorized => outcome
            .summary
            .push("authenticated tools/list: refused (HTTP 401; Authorization: Bearer <set>)".to_string()),
        ProbeOutcome::Unreachable { code } => outcome
            .summary
            .push(format!("authenticated tools/list: unreachable ({code})")),
        ProbeOutcome::Skipped { reason } => outcome
            .summary
            .push(format!("authenticated tools/list: skipped ({reason})")),
    }
    if let Some(remedy) = &report.remedy {
        outcome.summary.push(format!("next: {remedy}"));
    }
    if matches!(report.registration, WriteState::Written) {
        outcome.files.push(report.registration_path.clone());
    }
    for harness in &report.harnesses {
        if matches!(harness.state, WriteState::Written)
            && let Some(path) = &harness.path
        {
            outcome.files.push(path.clone());
        }
    }

    // Refuse loudly on a rejected credential so a scripted setup fails here
    // rather than at the first release post.
    if matches!(report.probe, ProbeOutcome::Unauthorized) {
        for line in &outcome.summary {
            println!("  {line}");
        }
        anyhow::bail!(
            "MechaCassy refused this machine's credential (HTTP 401). Nothing was verified; the \
             registration on disk still names {} and holds no secret.",
            report.token_env
        );
    }
    Ok(outcome)
}

/// Convenience used by `cas doctor`: resolve machine paths from the real
/// environment and produce the row.
pub fn doctor_row_from_env(project_proxy: Option<&Path>) -> Option<DoctorRow> {
    let env = ProcessEnv;
    let paths = MachinePaths::from_env(&env).ok()?;
    Some(doctor_row(project_proxy, &paths, &env, &ProxyHubProbe))
}

/// A stable, order-independent view of the credential-bearing strings a
/// generated artifact contains. Used by the leak tests.
#[cfg(test)]
fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const FAKE_TOKEN: &str = "xoxb-fake-secret-value-do-not-leak";
    const FAKE_BYPASS: &str = "bypass-secret-do-not-leak";

    struct FakeEnv(HashMap<String, String>);

    impl FakeEnv {
        fn with(pairs: &[(&str, &str)]) -> Self {
            Self(
                pairs
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            )
        }
    }

    impl EnvLookup for FakeEnv {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    struct FakeProbe(ProbeOutcome);

    impl HubProbe for FakeProbe {
        fn list_tools(&self, _url: &str, _token_env: &str, _bypass_env: &str) -> ProbeOutcome {
            self.0.clone()
        }
    }

    fn live_tools() -> ProbeOutcome {
        ProbeOutcome::Tools {
            tools: MECHA_CASSY_TOOLS.iter().map(|t| t.to_string()).collect(),
        }
    }

    fn paths_in(dir: &Path) -> MachinePaths {
        MachinePaths {
            user_proxy: dir.join("config").join("code-mode-mcp").join("config.toml"),
            claude_json: Some(dir.join("home").join(".claude.json")),
            codex_config: Some(dir.join("home").join(".codex").join("config.toml")),
        }
    }

    fn ready_env() -> FakeEnv {
        FakeEnv::with(&[
            (MECHA_CASSY_DEFAULT_TOKEN_ENV, FAKE_TOKEN),
            (MECHA_CASSY_DEFAULT_BYPASS_ENV, FAKE_BYPASS),
        ])
    }

    #[test]
    fn label_selects_the_per_machine_bearer_variable() {
        let mut args = MechaCassyArgs {
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            ..Default::default()
        };
        assert_eq!(args.resolved_token_env(), MECHA_CASSY_DEFAULT_TOKEN_ENV);

        args.label = Some("daniel-laptop".to_string());
        assert_eq!(
            args.resolved_token_env(),
            "MECHA_SLACK_TOKEN_DANIEL_LAPTOP"
        );

        // An explicit --token-env always wins over a label.
        args.token_env = Some("MECHA_SLACK_TOKEN_CI".to_string());
        assert_eq!(args.resolved_token_env(), "MECHA_SLACK_TOKEN_CI");
    }

    #[test]
    fn non_interactive_run_writes_env_reference_only_artifacts_and_prints_the_tool_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = MechaCassyArgs {
            label: Some("laptop".to_string()),
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            ..Default::default()
        };
        let env = FakeEnv::with(&[
            ("MECHA_SLACK_TOKEN_LAPTOP", FAKE_TOKEN),
            (MECHA_CASSY_DEFAULT_BYPASS_ENV, FAKE_BYPASS),
        ]);

        let report = run(&args, &paths, &env, &FakeProbe(live_tools())).unwrap();
        assert!(report.is_green(), "{report:?}");
        assert_eq!(report.registration, WriteState::Written);
        assert_eq!(report.allowlist, MECHA_CASSY_TOOLS);
        assert_eq!(report.token_env, "MECHA_SLACK_TOKEN_LAPTOP");
        assert_eq!(report.token_env_state, EnvState::Set);
        assert_eq!(report.remedy, None);
        assert_eq!(
            report.probe,
            ProbeOutcome::Tools {
                tools: vec!["mecha_read".to_string(), "mecha_post".to_string()]
            }
        );
        assert!(
            report
                .harnesses
                .iter()
                .all(|h| h.state == WriteState::Written),
            "{:?}",
            report.harnesses
        );

        // Every artifact names variables and holds no value.
        let secrets = [FAKE_TOKEN, FAKE_BYPASS];
        for path in [
            paths.user_proxy.clone(),
            paths.claude_json.clone().unwrap(),
            paths.codex_config.clone().unwrap(),
        ] {
            let written = std::fs::read_to_string(&path).unwrap();
            assert!(
                !contains_any(&written, &secrets),
                "{} leaked a credential value",
                path.display()
            );
            assert!(written.contains("MECHA_SLACK_TOKEN_LAPTOP"), "{written}");
        }
        // …and neither does the report that becomes terminal/JSON output.
        let rendered = serde_json::to_string(&report).unwrap();
        assert!(!contains_any(&rendered, &secrets));

        // Idempotent: a second run rewrites nothing.
        let second = run(&args, &paths, &env, &FakeProbe(live_tools())).unwrap();
        assert_eq!(second.registration, WriteState::AlreadyCurrent);
        assert!(
            second
                .harnesses
                .iter()
                .all(|h| h.state == WriteState::AlreadyCurrent),
            "{:?}",
            second.harnesses
        );
    }

    #[test]
    fn missing_variable_names_the_variable_and_the_file_without_probing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = MechaCassyArgs {
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            ..Default::default()
        };
        let env = FakeEnv::with(&[(MECHA_CASSY_DEFAULT_TOKEN_ENV, "   ")]);

        let report = run(&args, &paths, &env, &FakeProbe(live_tools())).unwrap();
        assert!(!report.is_green());
        assert_eq!(report.token_env_state, EnvState::Empty);
        assert_eq!(report.bypass_env_state, EnvState::Unset);
        // The probe is never attempted with a known-bad credential.
        assert!(matches!(report.probe, ProbeOutcome::Skipped { .. }));
        let remedy = report.remedy.unwrap();
        assert!(remedy.contains(MECHA_CASSY_DEFAULT_TOKEN_ENV), "{remedy}");
        assert!(remedy.contains("set but empty"), "{remedy}");
        assert!(remedy.contains(MECHA_CASSY_DEFAULT_BYPASS_ENV), "{remedy}");
        assert!(remedy.contains("credentials file"), "{remedy}");
        // The registration is still written, so the fix is a one-line edit.
        assert_eq!(report.registration, WriteState::Written);
    }

    #[test]
    fn unauthorized_probe_reports_redacted_header_state_and_a_mint_remedy() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = MechaCassyArgs {
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            ..Default::default()
        };

        let report = run(
            &args,
            &paths,
            &ready_env(),
            &FakeProbe(ProbeOutcome::Unauthorized),
        )
        .unwrap();
        assert!(!report.is_green());
        let remedy = report.remedy.unwrap();
        assert!(remedy.contains("401"), "{remedy}");
        assert!(remedy.contains("Bearer <set>"), "{remedy}");
        assert!(!remedy.contains(FAKE_TOKEN));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = MechaCassyArgs {
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            dry_run: true,
            ..Default::default()
        };
        let report = run(&args, &paths, &ready_env(), &FakeProbe(live_tools())).unwrap();
        assert_eq!(report.registration, WriteState::Planned);
        assert!(!paths.user_proxy.exists());
        assert!(!paths.claude_json.unwrap().exists());
        assert!(!paths.codex_config.unwrap().exists());
    }

    #[test]
    fn codex_registration_preserves_unrelated_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let codex = paths.codex_config.clone().unwrap();
        std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
        std::fs::write(
            &codex,
            "# operator comment worth keeping\nmodel = \"gpt-5\"\n\n\
             [mcp_servers.other]\ncommand = \"other-mcp\"\n",
        )
        .unwrap();

        let args = MechaCassyArgs {
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            ..Default::default()
        };
        run(&args, &paths, &ready_env(), &FakeProbe(live_tools())).unwrap();

        let written = std::fs::read_to_string(&codex).unwrap();
        assert!(written.contains("# operator comment worth keeping"), "{written}");
        assert!(written.contains("[mcp_servers.other]"), "{written}");
        assert!(
            written.contains("bearer_token_env_var = \"MECHA_SLACK_TOKEN_CASSY_PROXY\""),
            "{written}"
        );
        let parsed: toml::Value = toml::from_str(&written).unwrap();
        assert_eq!(
            parsed["mcp_servers"]["mecha-cassy"]["env_http_headers"]
                [MECHA_CASSY_BYPASS_HEADER]
                .as_str(),
            Some(MECHA_CASSY_DEFAULT_BYPASS_ENV)
        );
    }

    #[test]
    fn claude_registration_preserves_unrelated_keys() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let claude = paths.claude_json.clone().unwrap();
        std::fs::create_dir_all(claude.parent().unwrap()).unwrap();
        std::fs::write(
            &claude,
            r#"{"numStartups":42,"projects":{"/tmp/x":{"allowedTools":[]}},
                "mcpServers":{"playwright":{"type":"stdio","command":"npx"}}}"#,
        )
        .unwrap();

        let args = MechaCassyArgs {
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            ..Default::default()
        };
        run(&args, &paths, &ready_env(), &FakeProbe(live_tools())).unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&claude).unwrap()).unwrap();
        assert_eq!(written["numStartups"], 42);
        assert!(written["projects"]["/tmp/x"].is_object());
        assert_eq!(written["mcpServers"]["playwright"]["command"], "npx");
        assert_eq!(
            written["mcpServers"]["mecha-cassy"]["headers"]["Authorization"],
            "Bearer ${MECHA_SLACK_TOKEN_CASSY_PROXY}"
        );
    }

    #[test]
    fn drift_names_both_the_retired_and_the_new_tool() {
        // A full rename is both halves at once, and it blocks dispatch.
        let allowlist = vec![
            "slack_post_message".to_string(),
            "slack_read_channel".to_string(),
        ];
        let live = vec!["mecha_post".to_string(), "mecha_read".to_string()];
        let drift = tool_drift(&allowlist, &live);
        assert_eq!(drift.unallowlisted, vec!["mecha_post", "mecha_read"]);
        assert_eq!(
            drift.retired,
            vec!["slack_post_message", "slack_read_channel"]
        );
        assert!(drift.blocks_dispatch());
        let described = drift.describe(&live, &allowlist);
        assert!(described.contains("denied by policy"), "{described}");
        assert!(described.contains("slack_post_message"), "{described}");

        // A stale leftover next to the live routes is NOT an outage: every hub
        // tool is still admitted, so this must not block dispatch.
        let cluttered = vec![
            "mecha_read".to_string(),
            "mecha_post".to_string(),
            "slack_upload_file".to_string(),
        ];
        let stale = tool_drift(&cluttered, &live);
        assert!(stale.unallowlisted.is_empty());
        assert_eq!(stale.retired, vec!["slack_upload_file"]);
        assert!(!stale.blocks_dispatch());

        assert!(
            tool_drift(
                &["mecha_read".to_string(), "mecha_post".to_string()],
                &live,
            )
            .is_empty(),
            "order must not be treated as drift"
        );
    }

    #[test]
    fn doctor_is_green_only_when_registered_exported_and_verified() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();

        // Unregistered machine: a warning that names the one command.
        let row = doctor_row(None, &paths, &env, &FakeProbe(live_tools()));
        assert_eq!(row.severity, DoctorSeverity::Warning);
        assert!(row.message.contains("cas integrate mecha-cassy"), "{row:?}");

        let args = MechaCassyArgs {
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            no_harness: true,
            ..Default::default()
        };
        run(&args, &paths, &env, &FakeProbe(live_tools())).unwrap();

        let row = doctor_row(None, &paths, &env, &FakeProbe(live_tools()));
        assert_eq!(row.severity, DoctorSeverity::Ok, "{row:?}");
        assert!(row.message.contains("mecha_read"), "{row:?}");
        assert!(!row.message.contains(FAKE_TOKEN));
    }

    #[test]
    fn doctor_is_red_with_the_exact_remedy_when_a_variable_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let args = MechaCassyArgs {
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            no_harness: true,
            ..Default::default()
        };
        run(&args, &paths, &ready_env(), &FakeProbe(live_tools())).unwrap();

        let row = doctor_row(
            None,
            &paths,
            &FakeEnv::with(&[(MECHA_CASSY_DEFAULT_BYPASS_ENV, FAKE_BYPASS)]),
            &FakeProbe(live_tools()),
        );
        assert_eq!(row.severity, DoctorSeverity::Error);
        assert!(row.message.contains(MECHA_CASSY_DEFAULT_TOKEN_ENV), "{row:?}");
        assert!(row.message.contains("unset"), "{row:?}");
        assert!(row.message.contains("credentials file"), "{row:?}");
    }

    #[test]
    fn doctor_is_red_when_the_hub_tool_contract_drifts() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = MechaCassyArgs {
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            no_harness: true,
            ..Default::default()
        };
        run(&args, &paths, &env, &FakeProbe(live_tools())).unwrap();

        let row = doctor_row(
            None,
            &paths,
            &env,
            &FakeProbe(ProbeOutcome::Tools {
                tools: vec!["mecha_read".to_string(), "mecha_broadcast".to_string()],
            }),
        );
        assert_eq!(row.severity, DoctorSeverity::Error);
        assert!(row.message.contains("mecha_broadcast"), "{row:?}");
        assert!(row.message.contains("cas integrate mecha-cassy"), "{row:?}");
    }

    /// A machine whose project file still lists the retired `slack_*` names
    /// alongside the live ones can post today: every hub tool is admitted. It
    /// must read amber, not red — otherwise `cas doctor` reports an outage
    /// where there is only clutter.
    #[test]
    fn doctor_is_amber_not_red_for_stale_entries_that_still_admit_every_hub_tool() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = MechaCassyArgs {
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            no_harness: true,
            ..Default::default()
        };
        run(&args, &paths, &env, &FakeProbe(live_tools())).unwrap();

        let project = dir.path().join("proxy.toml");
        std::fs::write(
            &project,
            "allowlist = [\"mecha-cassy.mecha_read\", \"mecha-cassy.mecha_post\", \
             \"mecha-cassy.slack_upload_file\"]\n",
        )
        .unwrap();

        let row = doctor_row(Some(&project), &paths, &env, &FakeProbe(live_tools()));
        assert_eq!(row.severity, DoctorSeverity::Warning, "{row:?}");
        assert!(row.message.contains("slack_upload_file"), "{row:?}");
        assert!(!row.message.contains("denied by policy"), "{row:?}");
    }

    #[test]
    fn doctor_reports_a_project_proxy_file_that_shadows_the_machine_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = MechaCassyArgs {
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            no_harness: true,
            ..Default::default()
        };
        run(&args, &paths, &env, &FakeProbe(live_tools())).unwrap();

        let project = dir.path().join("proxy.toml");
        std::fs::write(&project, "allowlist = [\"neon.run_sql\"]\n").unwrap();

        let row = doctor_row(Some(&project), &paths, &env, &FakeProbe(live_tools()));
        assert_eq!(row.severity, DoctorSeverity::Error);
        assert!(row.message.contains("authoritative"), "{row:?}");
        assert!(row.message.contains("mecha-cassy.mecha_read"), "{row:?}");
        assert!(
            row.message.contains(&project.display().to_string()),
            "{row:?}"
        );
    }

    #[test]
    fn unauthorized_hub_never_reaches_the_ok_row() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        let env = ready_env();
        let args = MechaCassyArgs {
            bypass_env: MECHA_CASSY_DEFAULT_BYPASS_ENV.to_string(),
            url: MECHA_CASSY_MCP_URL.to_string(),
            no_harness: true,
            ..Default::default()
        };
        run(&args, &paths, &env, &FakeProbe(live_tools())).unwrap();

        let row = doctor_row(None, &paths, &env, &FakeProbe(ProbeOutcome::Unauthorized));
        assert_eq!(row.severity, DoctorSeverity::Error);
        assert!(row.message.contains("401"), "{row:?}");
        assert!(!row.message.contains(FAKE_TOKEN));
    }
}
