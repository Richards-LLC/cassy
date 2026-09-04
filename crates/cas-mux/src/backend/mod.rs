//! Interactive CLI backend adapters.

mod claude;
mod codex;
mod grok;
mod opencode;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::harness::{HarnessCapabilities, SupervisorCli};
use crate::pty::{PtyConfig, TeamsSpawnConfig};
use crate::{Effort, Result};

pub(crate) use claude::CLAUDE;
pub(crate) use codex::CODEX;
pub(crate) use grok::GROK;
pub(crate) use opencode::OPENCODE;

/// Inputs needed to build one worker CLI process.
pub struct WorkerLaunchConfig<'a> {
    pub name: &'a str,
    pub cwd: PathBuf,
    pub cas_root: Option<&'a PathBuf>,
    pub supervisor_name: &'a str,
    pub supervisor_cli: SupervisorCli,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub config_dir: Option<&'a str>,
    pub config_dir_source: Option<&'a str>,
    /// Independent requester secure-storage selector. The outer `Option`
    /// distinguishes legacy derivation from a captured selector; the inner
    /// `Option` preserves unset versus an explicitly empty value.
    pub secure_storage_dir: Option<Option<&'a str>>,
    pub teams: Option<&'a TeamsSpawnConfig>,
    pub active_workers: Option<usize>,
}

/// Inputs needed to build one supervisor CLI process.
pub struct SupervisorLaunchConfig<'a> {
    pub name: &'a str,
    pub cwd: PathBuf,
    pub cas_root: Option<&'a PathBuf>,
    pub worker_cli: SupervisorCli,
    pub worker_names: &'a [String],
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub teams: Option<&'a TeamsSpawnConfig>,
}

/// All behavior that varies between interactive CLI backends.
///
/// To add a fourth backend, add `backend/<name>.rs` with one [`Backend`]
/// implementation, register its module/static and `SupervisorCli` selector arm
/// in `backend/mod.rs`, then add the enum variant plus its parse arm in
/// `harness.rs`. Shared spawn, injection, and interrupt code needs no new
/// backend-specific branches.
pub trait Backend: Sync {
    /// Stable CLI name used in argv, environment metadata, and serialization.
    fn name(&self) -> &'static str;

    fn capabilities(&self) -> HarnessCapabilities;

    /// Map a shared effort level to this CLI's accepted value spelling.
    fn effort_arg(&self, effort: Effort) -> &'static str;

    fn build_worker_config(&self, launch: WorkerLaunchConfig<'_>) -> PtyConfig;

    fn build_supervisor_config(&self, launch: SupervisorLaunchConfig<'_>) -> PtyConfig;

    /// Complete any backend-specific launch precondition before spawning.
    fn prepare_workdir(&self, _cwd: &Path, _config_dir: Option<&str>) -> Result<()> {
        Ok(())
    }

    /// Inject a factory-session identifier into the CLI and its MCP child.
    fn push_factory_session(&self, config: &mut PtyConfig, session: &str) {
        push_plain_factory_session(config, session);
    }

    /// Bytes that cancel the current in-flight turn for this CLI.
    fn turn_cancel_bytes(&self) -> &'static [u8];

    /// Whether this backend exposes the `events.jsonl` turn-completion stream.
    fn has_turn_event_stream(&self) -> bool {
        false
    }
}

impl SupervisorCli {
    /// Resolve this serialized selector to its backend implementation.
    pub fn backend(self) -> &'static dyn Backend {
        match self {
            Self::Claude => &CLAUDE,
            Self::Codex => &CODEX,
            Self::Grok => &GROK,
            Self::OpenCode => &OPENCODE,
        }
    }
}

pub(super) fn finish_worker_config(
    config: &mut PtyConfig,
    supervisor_cli: SupervisorCli,
    active_workers: Option<usize>,
    account_dir: Option<&str>,
) {
    config.apply_worker_build_concurrency(active_workers);
    config.env.push((
        "CAS_FACTORY_SUPERVISOR_CLI".to_string(),
        supervisor_cli.backend().name().to_string(),
    ));
    if let Some(account_dir) = account_dir {
        config.env.push((
            "CAS_FACTORY_WORKER_ACCOUNT_DIR".to_string(),
            account_dir.to_string(),
        ));
    }
    config.env.extend(machine_registration_credentials());
}

/// Pass the supervisor's machine-registration credentials to each worker.
///
/// The MCP proxy configuration stores only `env:VARIABLE` references. Workers
/// run in panes whose environment is assembled explicitly, so the supervisor
/// must resolve the references before spawning them. This intentionally reads
/// only the managed MechaCassy registration: unrelated upstream credentials
/// must not be copied into every worker environment.
fn machine_registration_credentials() -> Vec<(String, String)> {
    let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    else {
        return Vec::new();
    };
    let path = config_home.join("code-mode-mcp").join("config.toml");
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let document = match toml::from_str::<toml::Value>(&contents) {
        Ok(document) => document,
        Err(_) => return Vec::new(),
    };
    let Some(server) = document
        .get("servers")
        .and_then(toml::Value::as_table)
        .and_then(|servers| servers.get("mecha-cassy"))
    else {
        return Vec::new();
    };

    let mut names = BTreeSet::new();
    collect_env_references(server, &mut names);
    names
        .into_iter()
        .filter_map(|name| {
            std::env::var_os(&name)
                .and_then(|value| value.into_string().ok().map(|value| (name, value)))
        })
        .collect()
}

fn collect_env_references(value: &toml::Value, names: &mut BTreeSet<String>) {
    match value {
        toml::Value::String(value) => {
            if let Some(name) = value.strip_prefix("env:").filter(|name| !name.is_empty()) {
                names.insert(name.to_string());
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                collect_env_references(value, names);
            }
        }
        toml::Value::Table(values) => {
            for value in values.values() {
                collect_env_references(value, names);
            }
        }
        toml::Value::Boolean(_)
        | toml::Value::Datetime(_)
        | toml::Value::Integer(_)
        | toml::Value::Float(_) => {}
    }
}

pub(super) fn finish_supervisor_config(
    config: &mut PtyConfig,
    backend_name: &str,
    worker_names: &[String],
) {
    config.env.push((
        "CAS_FACTORY_SUPERVISOR_CLI".to_string(),
        backend_name.to_string(),
    ));
    if !worker_names.is_empty() {
        config.env.push((
            "CAS_FACTORY_WORKER_NAMES".to_string(),
            worker_names.join(","),
        ));
    }
}

pub(super) fn push_plain_factory_session(config: &mut PtyConfig, session: &str) {
    config
        .env
        .push(("CAS_FACTORY_SESSION".to_string(), session.to_string()));
}

pub(super) fn sanitize_toml_arg(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
