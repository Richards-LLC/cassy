//! Interactive CLI backend adapters.

mod claude;
mod codex;
mod grok;
mod opencode;

use std::collections::{BTreeMap, BTreeSet};
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
    cas_root: Option<&PathBuf>,
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
    config.env.extend(proxy_credential_environment(cas_root));
}

/// Pass configured proxy credentials to each worker.
///
/// The MCP proxy configuration stores only `env:VARIABLE` or `${VARIABLE}`
/// references. Workers run in panes whose environment is assembled
/// explicitly, so the supervisor must resolve those references before
/// spawning them. Both the user config and the project-scoped `.cas/proxy.toml`
/// are read because project definitions override user definitions at runtime.
///
/// A factory daemon is often started by a desktop launcher or a non-login
/// service, so its environment is not guaranteed to contain credentials that
/// the supervisor's login shell had sourced. Resolve the same private
/// credentials file and shell profile used by `cas integrate mecha-cassy` as a
/// fallback, while keeping an explicitly exported value authoritative.
fn proxy_credential_environment(cas_root: Option<&PathBuf>) -> Vec<(String, String)> {
    let mut names = BTreeSet::new();
    let mut values = BTreeMap::new();
    let mut paths = Vec::new();
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    {
        paths.push(config_home.join("code-mode-mcp").join("config.toml"));
    }
    if let Some(cas_root) = cas_root {
        paths.push(cas_root.join("proxy.toml"));
    }
    for path in paths {
        let Ok(contents) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(document) = toml::from_str::<toml::Value>(&contents) else {
            continue;
        };
        collect_env_references(&document, &mut names);
    }
    for path in credential_source_paths() {
        load_shell_environment(&path, &mut values, &mut BTreeSet::new(), 0);
    }
    names
        .into_iter()
        .filter_map(|name| {
            std::env::var(&name)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| {
                    values
                        .remove(&name)
                        .filter(|value| !value.trim().is_empty())
                })
                .map(|value| (name, value))
        })
        .collect()
}

/// Return the credentials file and login profile locations that a normal
/// `cas integrate mecha-cassy` invocation uses. The explicit override wins,
/// then XDG, then the HOME default; the profile is included so a profile can
/// source an operator-selected credentials file outside those defaults.
fn credential_source_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);

    if let Some(path) = std::env::var_os("CAS_CREDENTIALS_FILE")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        paths.push(path);
    } else if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        paths.push(config_home.join("cas").join("credentials.env"));
    } else if let Some(home) = &home {
        paths.push(home.join(".config").join("cas").join("credentials.env"));
    }

    if let Some(home) = home {
        let shell = std::env::var_os("SHELL");
        paths.push(login_profile_path(&home, shell.as_deref()));
    }
    paths
}

fn login_profile_path(home: &Path, shell: Option<&std::ffi::OsStr>) -> PathBuf {
    let shell_name = shell
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if shell_name == "zsh" {
        return home.join(".zprofile");
    }
    if shell_name == "bash" && home.join(".bash_profile").exists() {
        return home.join(".bash_profile");
    }
    home.join(".profile")
}

fn valid_environment_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().enumerate().all(|(index, byte)| {
            byte == b'_'
                || (byte.is_ascii_alphanumeric() && (index > 0 || byte.is_ascii_alphabetic()))
        })
}

/// Parse the simple `export NAME='value'` form emitted by the integration
/// writer. The unquoted/double-quoted forms cover hand-maintained profiles as
/// well; arbitrary shell is intentionally not executed while resolving
/// credentials.
fn shell_assignment(line: &str) -> Option<(String, String)> {
    let mut line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    if let Some(rest) = line.strip_prefix("export") {
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        line = rest.trim_start();
    }
    let (name, raw_value) = line.split_once('=')?;
    let name = name.trim();
    if !valid_environment_name(name) {
        return None;
    }
    let raw_value = raw_value.trim();
    let value = if raw_value.starts_with('\'') && raw_value.ends_with('\'') {
        raw_value[1..raw_value.len() - 1].replace("'\\''", "'")
    } else if raw_value.starts_with('"') && raw_value.ends_with('"') {
        raw_value[1..raw_value.len() - 1].replace("\\\"", "\"")
    } else {
        raw_value.to_string()
    };
    Some((name.to_string(), value))
}

fn sourced_profile_path(line: &str) -> Option<PathBuf> {
    let value = line
        .split_once("&& . ")
        .map(|(_, value)| value.trim())
        .or_else(|| line.trim().strip_prefix(". ").map(str::trim))
        .or_else(|| line.trim().strip_prefix("source ").map(str::trim))?;
    let value = value.split_whitespace().next()?;
    let value = if value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("'\\''", "'")
    } else if value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].replace("\\\"", "\"")
    } else {
        value.to_string()
    };
    (!value.is_empty()).then(|| PathBuf::from(value))
}

/// Read profile assignments without executing operator shell code. Follow
/// sourced files only through explicit path tokens and cap recursion to avoid
/// cycles in mutually-sourced profiles.
fn load_shell_environment(
    path: &Path,
    values: &mut BTreeMap<String, String>,
    visited: &mut BTreeSet<PathBuf>,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    let identity = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(identity) {
        return;
    }
    let Ok(contents) = std::fs::read_to_string(path) else {
        return;
    };
    for line in contents.lines() {
        if let Some((name, value)) = shell_assignment(line) {
            values.insert(name, value);
        }
        if let Some(source) = sourced_profile_path(line) {
            load_shell_environment(&source, values, visited, depth + 1);
        }
    }
}

fn collect_env_references(value: &toml::Value, names: &mut BTreeSet<String>) {
    match value {
        toml::Value::String(value) => {
            let name = value
                .strip_prefix("env:")
                .filter(|name| !name.is_empty())
                .or_else(|| {
                    value
                        .strip_prefix("${")
                        .and_then(|value| value.strip_suffix('}'))
                        .filter(|name| !name.is_empty())
                });
            if let Some(name) = name {
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
