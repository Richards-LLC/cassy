//! Operator-only write policy (cas-3147, GH #1169, GH #1182).
//!
//! Project write roots and one-off per-task write grants widen the factory
//! workspace contract, so only the human operator may set them. Agents run as
//! the operator's Unix user, and nothing in Cassy separates the two at the OS
//! level (see the cas-3147 decision note). The policy is therefore guarded in
//! layers:
//!
//! 1. Project write roots live in the project's `.cas/config.toml` as
//!    `[factory] write_roots` (GH #1182), so `cas config list` shows them.
//!    Only `cas config set factory.write_roots` writes them, through
//!    [`parse_project_write_roots`]; the generic `Config::set` (MCP, the
//!    config editor, reset) refuses the key and `cas config import` keeps the
//!    current value. Per-task grants stay in `.cas/operator/write-policy.toml`,
//!    written only by `cas config grant-write` and the hub. Roots left in that
//!    file by cas-3147 are still honoured until `cas config set
//!    factory.write_roots` migrates them.
//! 2. The operator's own `! cas config set …` inside an agent pane carries
//!    agent environment and no terminal, so setting roots refuses only a
//!    factory worker cgroup ([`worker_cgroup_refusal`]). Grants keep the full
//!    [`operator_context_refusal`] and a typed confirmation.
//! 3. The PreToolUse hook is the gate against agents: it refuses agent tool
//!    calls that run these commands, or that write `.cas/config.toml`,
//!    `.cas/config.yaml` or anything under `.cas/operator/`. It unwraps
//!    command wrappers (`timeout`, `env`, `nice`, `xargs`, …), `$(which cas)`
//!    substitutions, `sh -c` and `eval`, and refuses any non-read-only
//!    command whose text names `factory.write_roots` with `set`, or
//!    `grant-write` / `revoke-write` (covering `python -c`, `perl -e` and
//!    heredocs).
//!
//! This is guardrail-grade, not security-grade. The operator and the agents
//! share a Unix user, so the PreToolUse hook (layer 3) is the actual gate
//! against agents; the CLI checks are defence in depth. A tool-call hook sees
//! only the command text: a `cas config set factory.write_roots` hidden in a
//! script file the agent runs, or assembled at run time, is out of its reach,
//! as is any same-user write made outside tool calls. A hard boundary needs
//! agents under a separate Unix user.
//!
//! # Operator-only actions and role signals (cas-3c26)
//!
//! The same gate guards the other operator-only actions:
//! - `cas hub pair` also needs the operator to confirm the origin and scopes.
//! - `cas hub authorize` refuses agents even with `--yes`.
//! - Launching `cas factory` refuses agents too.
//!
//! [`operator_action_refusal`] checks four signals in turn:
//! 1. agent environment markers;
//! 2. agent or Cassy-server ancestors, which a PTY wrapper such as
//!    `script -qc` does not hide;
//! 3. a factory worker cgroup;
//! 4. an interactive terminal.
//!
//! The PreToolUse hook refuses the same commands from an agent's shell,
//! however they are wrapped. It also refuses any agent command that sets,
//! unsets or clears `CAS_AGENT_ROLE`/`CAS_FACTORY_MODE`, so the role gates
//! that read them see the role the factory gave the agent. That hook refusal
//! is the control for those role gates, which include `cas update`'s worker
//! restriction, `restart_spawn_queue` and `supervisor_override`.
//!
//! The gates themselves still read the environment. Overriding it from the
//! process's cgroup was rejected, because a worker's own test harnesses run
//! in its cgroup and legitimately drop the role (cas-937a). That is
//! indistinguishable from a spoof at the process level.
//!
//! Residual limit (the same as above): every signal is same-user process
//! state.
//! - An agent can write a script file that changes its role environment
//!   and run it, and the hook sees only the script's name.
//! - It can strip its ancestry with a double-forked daemon, or leave its
//!   cgroup through the user's delegated cgroup tree.
//!
//! These are deliberate evasions, not mistakes. Only a separate Unix user
//! for agents closes them.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Policy file, relative to the Cassy root.
pub const OPERATOR_POLICY_FILE: &str = "operator/write-policy.toml";

/// A change a write root or grant permits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperatorWriteMode {
    Create,
    Edit,
    Delete,
}

/// A project-wide write root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorWriteRoot {
    /// Canonical absolute path, resolved when the operator set it.
    pub path: PathBuf,
    pub modes: BTreeSet<OperatorWriteMode>,
}

/// A one-off grant bound to one task; it ends when the task closes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorWriteGrant {
    pub task: String,
    pub path: PathBuf,
    pub modes: BTreeSet<OperatorWriteMode>,
    pub reason: String,
    pub granted_at: String,
    /// Who granted it: `operator-cli`, or `commander-device:<device id>` for a
    /// grant made from a paired device through the hub (cas-ab04).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_by: Option<String>,
}

/// The verified path a grant or revoke arrived through (cas-ab04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantSource {
    /// `cas config grant-write` after the operator gate and confirmation.
    OperatorCli,
    /// The hub, after authenticating the paired device's session.
    CommanderDevice { device_id: String },
}

impl GrantSource {
    pub fn label(&self) -> String {
        match self {
            GrantSource::OperatorCli => "operator-cli".to_string(),
            GrantSource::CommanderDevice { device_id } => format!("commander-device:{device_id}"),
        }
    }
}

/// Record a grant for an open task and note it on the task. Callers must
/// have established the operator first (CLI gate, or hub device session);
/// this function only validates the request.
pub fn record_operator_grant(
    cas_root: &Path,
    task_store: &dyn cas_store::TaskStore,
    task_id: &str,
    raw_path: &str,
    raw_modes: &str,
    reason: &str,
    source: &GrantSource,
) -> anyhow::Result<OperatorWriteGrant> {
    let reason = reason.trim();
    if reason.is_empty() {
        anyhow::bail!("a reason is required: it is recorded on the task");
    }
    let task = task_store
        .get(task_id)
        .map_err(|error| anyhow::anyhow!("task {task_id} not found: {error}"))?;
    if task.status == cas_types::TaskStatus::Closed {
        anyhow::bail!("task {} is closed; a grant ends when its task closes", task.id);
    }
    let path = resolve_operator_path(raw_path, cas_root)?;
    let modes = parse_modes(raw_modes)?;
    let mode_text = modes
        .iter()
        .map(|mode| format!("{mode:?}").to_lowercase())
        .collect::<Vec<_>>()
        .join("+");
    let grant = OperatorWriteGrant {
        task: task.id.clone(),
        path: path.clone(),
        modes,
        reason: reason.to_string(),
        granted_at: chrono::Utc::now().to_rfc3339(),
        granted_by: Some(source.label()),
    };
    let mut policy = load_operator_policy(cas_root)?;
    policy.grants.retain(|existing| !(existing.task == task.id && existing.path == path));
    policy.grants.push(grant.clone());
    save_operator_policy(cas_root, &policy)?;
    task_store.append_note(
        &task.id,
        &format!(
            "[{}] ✅ DECISION operator write grant (cas-3147): {} ({mode_text}) until this task closes, by {}. Reason: {reason}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M"),
            path.display(),
            source.label()
        ),
    )?;
    let path_text = path.display().to_string();
    let by = source.label();
    let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
        cas_root,
        "operator_write_grant",
        &[
            ("task", task.id.as_str()),
            ("path", path_text.as_str()),
            ("mode", mode_text.as_str()),
            ("by", by.as_str()),
        ],
    );
    Ok(grant)
}

/// Remove a task's grants (all, or the one for `raw_path`) and note it.
pub fn revoke_operator_grants(
    cas_root: &Path,
    task_store: &dyn cas_store::TaskStore,
    task_id: &str,
    raw_path: Option<&str>,
    source: &GrantSource,
) -> anyhow::Result<usize> {
    let path = raw_path
        .map(|raw| resolve_operator_path(raw, cas_root))
        .transpose()?;
    let mut policy = load_operator_policy(cas_root)?;
    let before = policy.grants.len();
    policy.grants.retain(|grant| {
        !(grant.task == task_id && path.as_ref().is_none_or(|path| &grant.path == path))
    });
    let removed = before - policy.grants.len();
    if removed > 0 {
        save_operator_policy(cas_root, &policy)?;
        let _ = task_store.append_note(
            task_id,
            &format!(
                "[{}] ✅ DECISION operator write grant revoked (cas-3147): {removed} grant(s), by {}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                source.label()
            ),
        );
        let by = source.label();
        let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
            cas_root,
            "operator_write_grant_revoked",
            &[("task", task_id), ("by", by.as_str())],
        );
    }
    Ok(removed)
}

/// The whole operator policy file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorWritePolicy {
    #[serde(default)]
    pub roots: Vec<OperatorWriteRoot>,
    #[serde(default)]
    pub grants: Vec<OperatorWriteGrant>,
}

/// Path of the policy file for a Cassy root.
pub fn operator_policy_path(cas_root: &Path) -> PathBuf {
    cas_root.join(OPERATOR_POLICY_FILE)
}

/// Load the policy; a missing file is an empty policy.
pub fn load_operator_policy(cas_root: &Path) -> anyhow::Result<OperatorWritePolicy> {
    let path = operator_policy_path(cas_root);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|error| anyhow::anyhow!("invalid operator write policy {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(OperatorWritePolicy::default()),
        Err(error) => Err(error.into()),
    }
}

/// Replace the policy file atomically.
pub fn save_operator_policy(cas_root: &Path, policy: &OperatorWritePolicy) -> anyhow::Result<()> {
    let path = operator_policy_path(cas_root);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("operator policy path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let text = format!(
        "# Operator write policy (cas-3147). Written only by the operator's own\n\
         # `cas config set factory.write_roots` / `cas config grant-write`.\n\
         # Agents are refused when they try to write this file.\n{}",
        toml::to_string_pretty(policy)?
    );
    let staging = parent.join(format!(".write-policy.{}.tmp", std::process::id()));
    std::fs::write(&staging, text)?;
    std::fs::rename(&staging, &path)?;
    Ok(())
}

/// Parse `create+edit` style modes; empty means the default `create+edit`.
pub fn parse_modes(value: &str) -> anyhow::Result<BTreeSet<OperatorWriteMode>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(BTreeSet::from([OperatorWriteMode::Create, OperatorWriteMode::Edit]));
    }
    value
        .split(['+', ','])
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
        .map(|mode| match mode {
            "create" => Ok(OperatorWriteMode::Create),
            "edit" => Ok(OperatorWriteMode::Edit),
            "delete" => Ok(OperatorWriteMode::Delete),
            other => Err(anyhow::anyhow!("unknown write mode `{other}`; use create, edit or delete")),
        })
        .collect()
}

/// Resolve one operator-supplied root or grant path: `~` expanded, absolute,
/// existing, canonical, and neither `/`, `$HOME`, nor anything that contains
/// or lies inside the operator policy directory.
pub fn resolve_operator_path(raw: &str, cas_root: &Path) -> anyhow::Result<PathBuf> {
    let raw = raw.trim();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let expanded = if raw == "~" {
        home.clone().ok_or_else(|| anyhow::anyhow!("HOME is not set"))?
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.clone().ok_or_else(|| anyhow::anyhow!("HOME is not set"))?.join(rest)
    } else {
        PathBuf::from(raw)
    };
    if !expanded.is_absolute() {
        anyhow::bail!("write root `{raw}` must be an absolute path (or start with ~/)");
    }
    let canonical = expanded
        .canonicalize()
        .map_err(|error| anyhow::anyhow!("write root `{raw}` does not resolve: {error}"))?;
    if canonical.parent().is_none() {
        anyhow::bail!("`/` cannot be a write root");
    }
    if home
        .and_then(|home| home.canonicalize().ok())
        .is_some_and(|home| home == canonical)
    {
        anyhow::bail!("your home directory cannot be a write root; name the folders inside it");
    }
    let cas_root = cas_root.canonicalize().unwrap_or_else(|_| cas_root.to_path_buf());
    let operator_dir = cas_root.join("operator");
    if operator_dir.starts_with(&canonical) || canonical.starts_with(&operator_dir) {
        anyhow::bail!(
            "write root `{}` would contain the operator policy itself ({})",
            canonical.display(),
            operator_dir.display()
        );
    }
    Ok(canonical)
}

/// The legacy path:modes format, superseded by [`parse_project_write_roots`]
/// (GH #1182); kept for the cas-3147 resolution tests.
#[cfg(test)]
/// Parse `cas config set factory.write_roots` values: comma-separated
/// `path[:modes]` entries, for example `~/a:create+edit,~/b`. Each path must
/// exist; it is resolved to a canonical absolute path. `""` clears the roots.
pub fn parse_write_roots(value: &str, cas_root: &Path) -> anyhow::Result<Vec<OperatorWriteRoot>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (path, modes) = match entry.rsplit_once(':') {
                Some((path, modes))
                    if !modes.is_empty()
                        && modes.chars().all(|c| c.is_ascii_lowercase() || c == '+') =>
                {
                    (path, modes)
                }
                _ => (entry, ""),
            };
            Ok(OperatorWriteRoot {
                path: resolve_operator_path(path, cas_root)?,
                modes: parse_modes(modes)?,
            })
        })
        .collect()
}

/// Resolve one `[factory] write_roots` entry (GH #1182): `~` expanded,
/// absolute, no `..` component, existing and canonical (so a symlink is
/// judged by where it points), and none of: `/`, `$HOME` or an ancestor of
/// it, the project root, or anything that contains or lies inside the
/// project's `.cas/` (so `.cas/config.toml` is never under a root). The
/// user-level store `~/.cas` is the exception: folders inside it may be roots.
pub fn resolve_project_write_root(raw: &str, cas_root: &Path) -> anyhow::Result<PathBuf> {
    let raw = raw.trim();
    if Path::new(raw)
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        anyhow::bail!("write root `{raw}` must not contain `..`;\nname the directory directly");
    }
    let canonical = resolve_operator_path(raw, cas_root)?;
    if let Some(home) = std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|home| home.canonicalize().ok())
        && home.starts_with(&canonical)
    {
        anyhow::bail!(
            "write root `{}` contains your home directory;\nname the folders inside it",
            canonical.display()
        );
    }
    let cas_dir = cas_root.canonicalize().unwrap_or_else(|_| cas_root.to_path_buf());
    // cas-f0ea0: the user-level store (~/.cas) holds the roots for every
    // project, and its own folders (such as a QA rig) may be roots. Its
    // config files and `operator/` stay refused: resolve_operator_path and
    // the hook's policy-path check never admit them.
    let user_store = is_user_store(&cas_dir);
    if cas_dir.starts_with(&canonical) {
        anyhow::bail!(
            "write root `{}` contains this project's Cassy directory\n({});\nname a directory outside the project",
            canonical.display(),
            cas_dir.display()
        );
    }
    if !user_store && canonical.starts_with(&cas_dir) {
        anyhow::bail!(
            "write root `{}` is inside this project's Cassy directory\n({})",
            canonical.display(),
            cas_dir.display()
        );
    }
    Ok(canonical)
}

/// Parse `cas config set factory.write_roots` values (GH #1182): a
/// comma-separated list of directories. Each root permits create, edit and
/// delete beneath it, so the legacy `path:modes` suffix is refused. An
/// empty value clears the roots. Duplicates are dropped, order is kept.
pub fn parse_project_write_roots(value: &str, cas_root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for entry in value.split(',').map(str::trim).filter(|entry| !entry.is_empty()) {
        if let Some((_, modes)) = entry.rsplit_once(':')
            && !modes.is_empty()
            && parse_modes(modes).is_ok()
        {
            anyhow::bail!(
                "write root `{entry}`: roots no longer take modes.\nEach root permits create, edit and delete; use `cas config grant-write`\nfor narrower, per-task access."
            );
        }
        let root = resolve_project_write_root(entry, cas_root)?;
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    Ok(roots)
}

/// The project write roots in effect for the PreToolUse hook (GH #1182):
/// `[factory] write_roots` from the project's config.toml, each re-checked
/// with [`resolve_project_write_root`]. A hand-edited entry that is unsafe or
/// does not resolve admits nothing and is skipped with a warning.
pub fn effective_project_write_roots(configured: &[String], cas_root: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for raw in configured {
        match resolve_project_write_root(raw, cas_root) {
            Ok(root) if !roots.contains(&root) => roots.push(root),
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(root = %raw, %error, "ignoring factory.write_roots entry");
            }
        }
    }
    roots
}

/// Whether `cas_dir` is the user-level store `~/.cas`, whose `config.toml`
/// holds the write roots for every project (cas-f0ea0).
pub fn is_user_store(cas_dir: &Path) -> bool {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    canonical(cas_dir) == canonical(&crate::config::host_config_dir())
}

/// The host whose Downloads folder [`downloads_dir_for`] resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadsHost {
    Linux,
    MacOs,
}

/// The user's Downloads folder (cas-f0ea0). On Linux `XDG_DOWNLOAD_DIR` from
/// `user-dirs.dirs` (its text in `user_dirs`), else `~/Downloads`; on macOS
/// `~/Downloads`. A value that is not absolute or names `$HOME` itself (the
/// XDG way of disabling the folder) falls back to `~/Downloads`.
pub fn downloads_dir_for(host: DownloadsHost, home: &Path, user_dirs: Option<&str>) -> PathBuf {
    let xdg = || {
        // Shell syntax: the last assignment wins; values are `"$HOME/…"` or absolute.
        let value = user_dirs?
            .lines()
            .filter_map(|line| line.trim().strip_prefix("XDG_DOWNLOAD_DIR="))
            .last()?
            .trim()
            .trim_matches('"');
        let path = match value.strip_prefix("$HOME") {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                home.join(rest.trim_start_matches('/'))
            }
            _ => PathBuf::from(value),
        };
        (path.is_absolute() && path != home).then_some(path)
    };
    match host {
        DownloadsHost::Linux => xdg().unwrap_or_else(|| home.join("Downloads")),
        DownloadsHost::MacOs => home.join("Downloads"),
    }
}

/// This host's Downloads folder, from `$HOME` (and on Linux
/// `$XDG_CONFIG_HOME/user-dirs.dirs`, default `~/.config/user-dirs.dirs`).
pub fn host_downloads_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_os = "macos") {
        return Some(downloads_dir_for(DownloadsHost::MacOs, &home, None));
    }
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let user_dirs = std::fs::read_to_string(config_home.join("user-dirs.dirs")).ok();
    Some(downloads_dir_for(DownloadsHost::Linux, &home, user_dirs.as_deref()))
}

/// `[factory] write_roots` from the user-level `~/.cas/config.toml`, which
/// apply to every project (cas-f0ea0). Unreadable config grants nothing.
pub fn user_write_roots() -> Vec<String> {
    match crate::config::Config::load(&crate::config::host_config_dir()) {
        Ok(config) => config.factory().write_roots,
        Err(error) => {
            tracing::warn!(%error, "user-level config unreadable; no user write roots apply");
            Vec::new()
        }
    }
}

/// Every write root in effect for a project (cas-f0ea0): its own
/// `[factory] write_roots` (`project`), the user-level roots (`user`) and the
/// built-in Downloads folder, deduplicated in that order. Each passes
/// [`resolve_project_write_root`] for the project; user-level and built-in
/// roots must also pass it for the user store. A missing Downloads folder is
/// skipped quietly; other unsafe or unresolved entries are skipped with a warning.
pub fn effective_write_roots(project: &[String], user: &[String], cas_root: &Path) -> Vec<PathBuf> {
    let mut roots = effective_project_write_roots(project, cas_root);
    let user_store = crate::config::host_config_dir();
    let downloads = host_downloads_dir()
        .filter(|path| path.is_dir())
        .map(|path| path.display().to_string());
    for raw in user.iter().chain(downloads.as_ref()) {
        match resolve_project_write_root(raw, &user_store)
            .and_then(|_| resolve_project_write_root(raw, cas_root))
        {
            Ok(root) if !roots.contains(&root) => roots.push(root),
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(root = %raw, %error, "ignoring user-level write root");
            }
        }
    }
    roots
}

/// Why this process may not set project write roots (GH #1182): only when
/// it runs inside a Cassy factory worker or server cgroup. The operator's
/// `! cas config set …` in an agent pane carries agent environment and no
/// terminal, so neither is a refusal here; the PreToolUse hook refuses the
/// same command when an agent runs it as a tool call.
pub fn worker_cgroup_refusal(cgroup: &str) -> Option<String> {
    cgroup
        .split(['/', '\n'])
        .any(|part| part.starts_with("cas-worker-") || part.starts_with("cas-server-"))
        .then(|| {
            "refused: this runs inside a Cassy factory worker cgroup.\nWrite roots are operator-only: run `cas config set factory.write_roots ...`\nfrom your own terminal or as `! cas ...` in the supervisor's pane."
                .to_string()
        })
}

/// Environment variables only an agent session carries (cas-3147).
///
/// cas-31ec: exact identity markers only, never a name prefix. Only a
/// variable a harness or the factory sets for the agent processes it spawns
/// counts; operator preferences sharing a namespace must not lock the
/// operator out:
/// - `cas` itself exports the build knobs `CAS_FACTORY_NICE_WORKER` and
///   `CAS_FACTORY_CARGO_BUILD_JOBS` early in every run
///   (`factory::apply_resource_contention_env`), and operators set those and
///   `CAS_FACTORY_NICE_LEVEL` in their shells;
/// - operators export Claude Code preferences such as
///   `CLAUDE_CODE_NATIVE_CURSOR` and `CLAUDE_CODE_NO_FLICKER` from their
///   shell rc;
/// - `CAS_AGENT_MAX_TASKS` is a runtime setting (`config::runtime`).
const AGENT_ENV_MARKERS: &[&str] = &[
    "CAS_AGENT_NAME",
    "CAS_AGENT_ROLE",
    "CAS_AGENT_ID",
    "CAS_SESSION_ID",
    "CAS_CLONE_PATH",
    "CAS_SUPERVISOR_NAME",
    "CAS_FACTORY_MODE",
    "CAS_FACTORY_SESSION",
    "CAS_FACTORY_WORKER_NAMES",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
    "CODEX_THREAD_ID",
];

fn agent_ancestor(command_line: &str) -> bool {
    let mut words = command_line.split_whitespace();
    let Some(program) = words.next() else {
        return false;
    };
    let name = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    match name {
        "claude" | "codex" => true,
        "cas" => matches!(words.next(), Some("serve" | "factory")),
        "node" | "bun" | "deno" => command_line.contains("claude-code") || command_line.contains("@openai/codex"),
        _ => command_line.contains("/codex/") && command_line.contains("codex"),
    }
}

/// What the operator-only commands know about the process that runs them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InvocationContext {
    /// Environment variable names that are set and non-empty.
    pub env_names: BTreeSet<String>,
    /// Command lines of the ancestor processes, nearest first.
    pub ancestors: Vec<String>,
    pub stdin_is_terminal: bool,
    pub stdout_is_terminal: bool,
    /// This process's cgroup path (Linux `/proc/self/cgroup`), if known.
    pub cgroup: String,
}

/// Security-relevant config keys that relax a guard (cas-0d4f0). Like the
/// write roots, only the operator may change them: `cas config set`, reset,
/// import, the line editor and the TUI refuse an agent context, and the
/// PreToolUse hook refuses agent shell commands that set them and every
/// agent write to a Cassy `config.toml`.
pub const OPERATOR_ONLY_CONFIG_KEYS: &[&str] = &[
    "factory.commander_mirror_enabled",
    "verification.enabled",
    "verification.force_bypass_allowed",
    "slack.transport",
    "factory.supervisor_only_mcp",
    "factory.supervisor_only_env",
    "factory.worker_credential_env",
    // cas-d757 (GH #1219): emptying either list lets workers run the
    // operator-authenticated CLIs and tools again.
    "factory.worker_denied_clis",
    "factory.worker_denied_mcp_tools",
    "qa.mode",
    "qa.evidence_gate",
    "qa.independent_pass",
    "release.claude_account_allowlist",
    // cas-ae37c: both the destination and the hook-event switches can send
    // notification text to an external webhook.
    "notifications.webhook_url",
    "notifications.on_permission_prompt",
    "notifications.on_idle_prompt",
    "notifications.on_auth_success",
    // cas-d58d: the operator's privacy rule for same-repo peers; `team`
    // admits other users' supervisors again.
    "peers.same_repo_scope",
];

/// Whether `key` is operator-only (cas-0d4f0), including `factory.write_roots`.
pub fn is_operator_only_config_key(key: &str) -> bool {
    let key = key.trim();
    key == "factory.write_roots" || OPERATOR_ONLY_CONFIG_KEYS.contains(&key)
}

/// The operator-only keys whose value differs between two configs.
pub fn changed_operator_config_keys(
    before: &crate::config::Config,
    after: &crate::config::Config,
) -> Vec<&'static str> {
    OPERATOR_ONLY_CONFIG_KEYS
        .iter()
        .copied()
        .filter(|key| before.get(key) != after.get(key))
        .collect()
}

/// Why this process may not change the operator-only `changed` keys, or
/// `None` when nothing operator-only changes or an operator runs it.
pub fn operator_config_refusal(changed: &[&str], context: &InvocationContext) -> Option<String> {
    if changed.is_empty() {
        return None;
    }
    let keys = changed.join(", ");
    context_refusal(
        context,
        &format!(
            "{keys} {} operator-only because {} a Cassy guard",
            if changed.len() == 1 { "is" } else { "are" },
            if changed.len() == 1 { "it relaxes" } else { "they relax" }
        ),
        &format!("run `cas config set {} <value>` yourself from your own terminal", changed[0]),
    )
}

/// Why this process may not change the operator policy, or `None` when it
/// looks like an operator at a terminal: no agent environment, no agent
/// process among its ancestors, and an interactive terminal.
pub fn operator_context_refusal(context: &InvocationContext) -> Option<String> {
    context_refusal(
        context,
        "Write roots and grants are operator-only",
        "run the command yourself from your own terminal",
    )
}

/// cas-3c26: why this process may not run the operator-only `action`, or
/// `None` when it looks like an operator at an interactive terminal.
pub fn operator_action_refusal(
    action: &str,
    remedy: &str,
    context: &InvocationContext,
) -> Option<String> {
    context_refusal(context, &format!("{action} is operator-only"), remedy)
}

/// cas-3c26: [`operator_action_refusal`] without the interactive-terminal
/// requirement, for operator paths that legitimately run unattended (an
/// explicit `--yes`). An agent context is still refused.
pub fn agent_context_refusal(
    action: &str,
    remedy: &str,
    context: &InvocationContext,
) -> Option<String> {
    let unattended = InvocationContext {
        stdin_is_terminal: true,
        stdout_is_terminal: true,
        ..context.clone()
    };
    context_refusal(&unattended, &format!("{action} is operator-only"), remedy)
}

/// cas-3c26: whether a `/proc/self/cgroup` path places the process in a
/// Cassy factory worker's scope (or a server nested under one).
pub fn in_factory_worker_cgroup(cgroup: &str) -> bool {
    cgroup
        .lines()
        .any(|line| line.split('/').any(|part| part.starts_with("cas-worker-")))
}

/// cas-3c26: the operator-only action a `cas` invocation performs, judged
/// by the real CLI parser (`args` are the words after `cas`).
pub fn operator_only_cas_invocation(args: &[String]) -> Option<&'static str> {
    use crate::cli::Commands;
    use crate::cli::hub::HubCommands;
    use clap::Parser;
    let argv = std::iter::once("cas".to_string()).chain(args.iter().cloned());
    let cli = crate::cli::Cli::try_parse_from(argv).ok()?;
    match cli.command? {
        Commands::Hub(hub) => match hub.command? {
            HubCommands::Pair(_) => Some("Pairing a Commander device (`cas hub pair`)"),
            HubCommands::Authorize(_) => {
                Some("Authorizing a Commander pairing (`cas hub authorize`)")
            }
            _ => None,
        },
        Commands::Factory(factory) if factory.command.is_none() => {
            Some("Launching a factory (`cas factory`)")
        }
        _ => None,
    }
}

/// `subject` states what is operator-only; `remedy` what the operator runs.
fn context_refusal(context: &InvocationContext, subject: &str, remedy: &str) -> Option<String> {
    if let Some(marker) = context
        .env_names
        .iter()
        .find(|name| AGENT_ENV_MARKERS.contains(&name.as_str()))
    {
        return Some(format!(
            "refused: {marker} is set, so this runs inside an agent session. {subject}; {remedy}. \
             If this is your own terminal, it inherited {marker} from an agent pane: open a fresh \
             terminal and run it there."
        ));
    }
    if let Some(ancestor) = context.ancestors.iter().find(|line| agent_ancestor(line)) {
        let program = ancestor.split_whitespace().next().unwrap_or(ancestor);
        return Some(format!(
            "refused: this process descends from an agent or Cassy server ({program}). {subject}; {remedy}."
        ));
    }
    if worker_cgroup_refusal(&context.cgroup).is_some() {
        return Some(format!(
            "refused: this process runs inside a Cassy factory worker cgroup. {subject}; {remedy}."
        ));
    }
    if !(context.stdin_is_terminal && context.stdout_is_terminal) {
        return Some(format!(
            "refused: {subject}, and a change needs an interactive terminal so the operator can confirm it; {remedy}."
        ));
    }
    None
}

/// cas-0d4f0: refusal for an MCP proxy change that would weaken
/// `factory.supervisor_only_mcp`: removing or replacing a supervisor-only
/// server, or adding the same endpoint under another name, which workers
/// could then call. The operator manages those with `cas mcp` at a terminal.
///
/// cas-1b94: shared by the MCP `proxy_add`/`proxy_remove` tools, the `cas mcp`
/// CLI and the PreToolUse hook, so the three apply one rule.
#[cfg(feature = "mcp-proxy")]
pub fn supervisor_only_mcp_refusal(
    name: &str,
    added: Option<&cmcp_core::config::ServerConfig>,
    existing: &cmcp_core::config::Config,
    supervisor_only: &[String],
) -> Option<String> {
    use cmcp_core::config::ServerConfig;
    fn endpoint(server: &ServerConfig) -> String {
        match server {
            ServerConfig::Stdio { command, args, .. } => format!("{command} {}", args.join(" ")),
            ServerConfig::Http { url, .. } | ServerConfig::Sse { url, .. } => url.clone(),
        }
    }
    let refusal = |what: String| {
        format!(
            "🚫 OPERATOR-ONLY (cas-0d4f0): {what}. factory.supervisor_only_mcp keeps it from workers, so only the operator changes it, with `cas mcp add|remove` from their own terminal. Ask the supervisor to request the operator's approval."
        )
    };
    if supervisor_only
        .iter()
        .any(|only| only == name || cas_types::public_upstream_id(only) == name)
    {
        return Some(refusal(format!("'{name}' is a supervisor-only MCP server")));
    }
    let added = endpoint(added?);
    existing
        .servers
        .iter()
        .find(|(existing_name, server)| {
            supervisor_only.contains(existing_name) && endpoint(server) == added
        })
        .map(|(existing_name, _)| {
            refusal(format!(
                "'{name}' would reach supervisor-only server '{existing_name}' under another name"
            ))
        })
}

/// The `factory.supervisor_only_mcp` server names configured at `cas_root`.
pub fn supervisor_only_mcp_names(cas_root: &Path) -> Vec<String> {
    crate::config::Config::load(cas_root)
        .map(|config| config.factory().worker_policy.supervisor_only_mcp)
        .unwrap_or_default()
}

/// cas-1b94: whether `cas mcp add|remove|import` may make a change that
/// `proxy_refusal` (from [`supervisor_only_mcp_refusal`]) objects to. The
/// operator at their own terminal may; an agent context may not.
pub fn supervisor_only_mcp_cli_refusal(
    proxy_refusal: Option<String>,
    context: &InvocationContext,
) -> Option<String> {
    let refusal = proxy_refusal?;
    context_refusal(
        context,
        "Servers on factory.supervisor_only_mcp are operator-only",
        "the operator runs `cas mcp add|remove` from their own terminal",
    )
    .map(|why| format!("{refusal}\n{why}"))
}

impl InvocationContext {
    /// Describe the current process for [`operator_context_refusal`].
    pub fn from_process() -> Self {
        use std::io::IsTerminal;
        Self {
            env_names: std::env::vars_os()
                .filter(|(_, value)| !value.is_empty())
                .filter_map(|(name, _)| name.into_string().ok())
                .collect(),
            ancestors: process_ancestors(),
            stdin_is_terminal: std::io::stdin().is_terminal(),
            stdout_is_terminal: std::io::stdout().is_terminal(),
            cgroup: std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default(),
        }
    }
}

/// Command lines of this process's ancestors, nearest first.
fn process_ancestors() -> Vec<String> {
    let mut ancestors = Vec::new();
    #[cfg(target_os = "linux")]
    {
        let mut pid = std::process::id();
        for _ in 0..64 {
            let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
                break;
            };
            let Some(parent) = status
                .lines()
                .find_map(|line| line.strip_prefix("PPid:"))
                .and_then(|value| value.trim().parse::<u32>().ok())
            else {
                break;
            };
            if parent <= 1 {
                break;
            }
            let command_line = std::fs::read(format!("/proc/{parent}/cmdline"))
                .map(|bytes| {
                    String::from_utf8_lossy(&bytes)
                        .split('\0')
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            ancestors.push(command_line);
            pid = parent;
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut pid = std::process::id();
        for _ in 0..64 {
            let Ok(output) = std::process::Command::new("ps")
                .args(["-o", "ppid=", "-p", &pid.to_string()])
                .output()
            else {
                break;
            };
            let Some(parent) = String::from_utf8_lossy(&output.stdout).trim().parse::<u32>().ok() else {
                break;
            };
            if parent <= 1 {
                break;
            }
            let command_line = std::process::Command::new("ps")
                .args(["-o", "command=", "-p", &parent.to_string()])
                .output()
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                .unwrap_or_default();
            ancestors.push(command_line);
            pid = parent;
        }
    }
    ancestors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(env: &[&str], ancestors: &[&str], tty: bool) -> InvocationContext {
        InvocationContext {
            env_names: env.iter().map(|name| name.to_string()).collect(),
            ancestors: ancestors.iter().map(|line| line.to_string()).collect(),
            stdin_is_terminal: tty,
            stdout_is_terminal: tty,
            cgroup: "0::/user.slice/user@1000.service/app.slice/app-org.kde.konsole-1.scope/tab(2).scope".into(),
        }
    }

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    /// cas-3c26: an agent that unsets every agent variable (`env -u …`) or
    /// gives itself a terminal (`script -qc …`) still descends from its
    /// harness, and a worker still runs in its cgroup; each is refused. An
    /// operator's terminal passes.
    #[test]
    fn cas_3c26_operator_actions_refuse_env_spoof_pty_wrappers_and_worker_cgroups() {
        let action = "Pairing a Commander device";
        let remedy = "run `cas hub pair` yourself";
        // `env -u CAS_AGENT_ROLE -u CLAUDECODE … cas hub pair` from Claude Code.
        let env_spoof = context(
            &[],
            &[
                "bash -c cas hub pair",
                "claude --dangerously-skip-permissions",
            ],
            true,
        );
        let refused =
            operator_action_refusal(action, remedy, &env_spoof).expect("env spoof refused");
        assert!(refused.contains("descends from an agent"), "{refused}");
        assert!(refused.contains(action), "{refused}");

        // `script -qc 'cas hub pair'` from Codex: a terminal, but an agent ancestor.
        let pty = context(
            &[],
            &[
                "script -qc cas hub pair",
                "/usr/bin/bash",
                "/opt/codex/bin/codex exec",
            ],
            true,
        );
        assert!(operator_action_refusal(action, remedy, &pty).is_some());

        // A role variable alone is an agent marker.
        let spoofed_role = context(&["CAS_AGENT_ROLE"], &[], true);
        assert!(operator_action_refusal(action, remedy, &spoofed_role).is_some());

        // A worker scope, even with a clean environment and a terminal.
        let mut worker = context(&[], &[], true);
        worker.cgroup =
            "0::/user.slice/cas-worker-cas-src-mighty-crane-74/cas-private-server-1".into();
        let refused =
            operator_action_refusal(action, remedy, &worker).expect("worker cgroup refused");
        assert!(refused.contains("worker cgroup"), "{refused}");

        // The operator at a terminal passes; without a terminal it is refused.
        assert_eq!(
            operator_action_refusal(action, remedy, &context(&[], &["-bash"], true)),
            None
        );
        assert!(
            operator_action_refusal(action, remedy, &context(&[], &["-bash"], false)).is_some()
        );
    }

    /// cas-3c26: every operator-only CLI path shares one gate, so the
    /// write-policy (cas-3147), guarded-config (cas-0d4f0) and supervisor-only
    /// MCP (cas-1b94) checks refuse the same spoofed contexts.
    #[test]
    fn cas_3c26_every_operator_only_path_refuses_the_same_spoofed_contexts() {
        let mut worker = context(&[], &[], true);
        worker.cgroup = "0::/user.slice/cas-worker-cas-src-a-b".into();
        let spoofed = [
            context(&[], &["bash -c cas config set x", "claude"], true),
            context(
                &[],
                &[
                    "script -qc cas mcp remove github",
                    "/opt/codex/bin/codex exec",
                ],
                true,
            ),
            context(&["CAS_AGENT_ROLE"], &[], true),
            worker,
        ];
        for spoof in &spoofed {
            assert!(operator_context_refusal(spoof).is_some(), "{spoof:?}");
            assert!(
                operator_config_refusal(&["verification.enabled"], spoof).is_some(),
                "{spoof:?}"
            );
            assert!(
                supervisor_only_mcp_cli_refusal(Some("supervisor-only".into()), spoof).is_some(),
                "{spoof:?}"
            );
            assert!(
                operator_action_refusal("Pairing", "run it", spoof).is_some(),
                "{spoof:?}"
            );
        }
        let operator = context(&[], &["-bash"], true);
        assert_eq!(operator_context_refusal(&operator), None);
        assert_eq!(
            operator_config_refusal(&["verification.enabled"], &operator),
            None
        );
        assert_eq!(
            supervisor_only_mcp_cli_refusal(Some("x".into()), &operator),
            None
        );
    }

    /// cas-3c26: `--yes` paths drop only the terminal requirement.
    #[test]
    fn cas_3c26_unattended_operator_paths_still_refuse_agents() {
        let (action, remedy) = ("Authorizing a Commander pairing", "run it yourself");
        assert_eq!(
            agent_context_refusal(action, remedy, &context(&[], &["cron"], false)),
            None
        );
        assert!(agent_context_refusal(action, remedy, &context(&[], &["claude"], false)).is_some());
        assert!(
            agent_context_refusal(action, remedy, &context(&["CLAUDECODE"], &[], true)).is_some()
        );
    }

    #[test]
    fn cas_3c26_worker_cgroups_are_recognized_and_server_scopes_are_not() {
        assert!(in_factory_worker_cgroup(
            "0::/user.slice/x.scope/cas-worker-cas-src-a-b"
        ));
        assert!(in_factory_worker_cgroup(
            "0::/user.slice/cas-worker-a/cas-private-server-9"
        ));
        assert!(!in_factory_worker_cgroup("0::/user.slice/cas-server-hub"));
        assert!(!in_factory_worker_cgroup(
            "0::/user.slice/user@1000.service/app.slice"
        ));
        assert!(!in_factory_worker_cgroup(""));
    }

    /// cas-3c26: operator-only `cas` invocations, by the real CLI parser.
    #[test]
    fn cas_3c26_operator_only_cas_invocations_are_classified_by_the_cli_parser() {
        for line in [
            "hub pair --origin https://commander.example",
            "hub pair --origin https://x --scopes machine:read,pane:input,message:send",
            "hub authorize K7MW-4H2Q --yes",
            "factory",
            "factory --workers 3",
            "--verbose factory -w 2",
        ] {
            assert!(
                operator_only_cas_invocation(&words(line)).is_some(),
                "{line}"
            );
        }
        for line in [
            "hub status",
            "factory status",
            "task list",
            "hub",
            "config get factory.write_roots",
        ] {
            assert_eq!(operator_only_cas_invocation(&words(line)), None, "{line}");
        }
    }

    /// cas-31ec: the operator build knobs `cas` exports (or the operator sets)
    /// and the operator's own harness preferences are not agent markers. An operator terminal carrying them may launch a
    /// factory, pair and authorize; a real agent context is still refused by
    /// its identity variable, harness ancestor or worker cgroup.
    #[test]
    fn cas_31ec_operator_build_knobs_do_not_mark_an_agent() {
        // Build knobs plus the operator's own Claude Code preferences
        // (~/.bashrc) and runtime settings: none is an agent identity.
        let knobs = [
            "HOME",
            "PATH",
            "TERM",
            "CAS_FACTORY_NICE_WORKER",
            "CAS_FACTORY_CARGO_BUILD_JOBS",
            "CAS_FACTORY_NICE_LEVEL",
            "CLAUDE_CODE_NATIVE_CURSOR",
            "CLAUDE_CODE_NO_FLICKER",
            "CAS_AGENT_MAX_TASKS",
            "CODEX_HOME",
        ];
        let operator = context(&knobs, &["-bash", "/usr/bin/konsole", "/lib/systemd/systemd --user"], true);
        for (action, remedy) in [
            ("Launching a factory", "run `cas factory` yourself from your own terminal"),
            ("Pairing a Commander device (`cas hub pair`)", "run `cas hub pair` yourself"),
            ("Authorizing a Commander pairing (`cas hub authorize`)", "run `cas hub authorize` yourself"),
        ] {
            assert_eq!(operator_action_refusal(action, remedy, &operator), None, "{action}");
            assert_eq!(agent_context_refusal(action, remedy, &operator), None, "{action}");
        }
        assert_eq!(operator_context_refusal(&operator), None);

        for marker in [
            "CAS_AGENT_ROLE",
            "CAS_AGENT_ID",
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CODEX_SANDBOX",
            "CODEX_SANDBOX_NETWORK_DISABLED",
            "CAS_FACTORY_MODE",
            "CAS_FACTORY_SESSION",
        ] {
            let mut env = knobs.to_vec();
            env.push(marker);
            let refusal = operator_action_refusal("Launching a factory", "run `cas factory`", &context(&env, &["-bash"], true))
                .unwrap_or_else(|| panic!("{marker} marks an agent"));
            assert!(refusal.contains(&format!("{marker} is set")), "{refusal}");
            assert!(refusal.contains("open a fresh terminal"), "{refusal}");
        }
        let under_harness = context(&knobs, &["bash -c cas factory", "claude --dangerously-skip-permissions"], true);
        assert!(
            operator_action_refusal("Launching a factory", "run `cas factory`", &under_harness)
                .is_some_and(|refusal| refusal.contains("descends from an agent"))
        );
        let mut in_worker = context(&knobs, &["-bash"], true);
        in_worker.cgroup = "0::/user.slice/app.slice/cas-worker-cassy-zen-viper-42-subtle-gopher-35".into();
        assert!(
            operator_action_refusal("Launching a factory", "run `cas factory`", &in_worker)
                .is_some_and(|refusal| refusal.contains("cgroup"))
        );
    }

    /// cas-3147: the operator-only commands refuse any process that carries
    /// an agent's environment, descends from an agent harness or Cassy
    /// server, or has no interactive terminal; an operator's terminal passes.
    #[test]
    fn cas_3147_operator_context_refuses_every_agent_signal() {
        let operator = context(&["HOME", "PATH", "TERM"], &["-bash", "/usr/bin/konsole", "/lib/systemd/systemd --user"], true);
        assert_eq!(operator_context_refusal(&operator), None);

        for marker in [
            "CAS_AGENT_NAME",
            "CAS_AGENT_ROLE",
            "CAS_SESSION_ID",
            "CAS_FACTORY_MODE",
            "CAS_CLONE_PATH",
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CODEX_SANDBOX",
            "CODEX_THREAD_ID",
        ] {
            let refusal = operator_context_refusal(&context(&["HOME", marker], &["-bash"], true))
                .unwrap_or_else(|| panic!("{marker} marks an agent"));
            assert!(refusal.contains(marker), "{refusal}");
        }
        for ancestor in [
            "/home/u/.local/bin/claude --dangerously-skip-permissions",
            "node /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            "/home/u/.codex/packages/standalone/codex --yolo",
            "cas serve",
            "/home/u/.local/bin/cas factory daemon --session s",
        ] {
            let refusal = operator_context_refusal(&context(&["HOME"], &["bash", ancestor, "-bash"], true))
                .unwrap_or_else(|| panic!("{ancestor} is an agent ancestor"));
            assert!(refusal.contains("agent"), "{refusal}");
        }
        let mut in_worker_scope = context(&["HOME"], &["-bash"], true);
        in_worker_scope.cgroup =
            "0::/user.slice/app.slice/tab(2).scope/cas-worker-cassy-fierce-octopus-14-silent-koala-69".into();
        let refusal = operator_context_refusal(&in_worker_scope).expect("factory worker cgroup");
        assert!(refusal.contains("cgroup"), "{refusal}");
        let refusal = operator_context_refusal(&context(&["HOME"], &["-bash"], false))
            .expect("no terminal");
        assert!(refusal.contains("terminal"), "{refusal}");
    }

    /// cas-0d4f0: a change to a security-relevant key is refused from any
    /// agent context and allowed for the operator at a terminal; ordinary
    /// keys never need the operator.
    #[test]
    fn cas_0d4f0_operator_only_keys_refuse_agent_contexts() {
        let before = crate::config::Config::default();
        let mut after = before.clone();
        after.set("sync.min_helpful", "5").unwrap();
        assert!(changed_operator_config_keys(&before, &after).is_empty());
        let agent = context(&["HOME", "CLAUDECODE"], &["-bash"], false);
        assert_eq!(operator_config_refusal(&[], &agent), None);

        for (key, value) in [
            ("factory.commander_mirror_enabled", "false"),
            ("verification.enabled", "false"),
            ("verification.force_bypass_allowed", "true"),
            ("slack.transport", "any"),
            ("factory.supervisor_only_mcp", ""),
            ("factory.worker_credential_env", "GH_TOKEN"),
            ("factory.worker_denied_clis", ""),
            ("factory.worker_denied_mcp_tools", ""),
            ("qa.evidence_gate", "false"),
            ("qa.mode", "batch"),
            ("release.claude_account_allowlist", "agent@example.com"),
            ("peers.same_repo_scope", "own"),
        ] {
            assert!(is_operator_only_config_key(key), "{key}");
            let mut changed = before.clone();
            changed.set(key, value).unwrap();
            if changed.get(key) == before.get(key) {
                // The default already equals this value; flip it instead.
                changed.set(key, "true").unwrap();
            }
            let keys = changed_operator_config_keys(&before, &changed);
            assert_eq!(keys, [key], "{key}");
            let refusal = operator_config_refusal(&keys, &agent).expect("agent is refused");
            assert!(refusal.contains(key) && refusal.contains("operator"), "{refusal}");
            let operator = context(&["HOME", "PATH"], &["-bash"], true);
            assert_eq!(operator_config_refusal(&keys, &operator), None);
        }
        assert!(is_operator_only_config_key("factory.write_roots"));
        assert!(!is_operator_only_config_key("issues.repo"));
    }

    #[test]
    fn cas_3147_write_roots_parse_modes_canonicalize_and_refuse_unsafe_roots() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let cas_root = base.join("project/.cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        let requests = base.join("config/docs/requests");
        let autostart = base.join("autostart");
        std::fs::create_dir_all(&requests).unwrap();
        std::fs::create_dir_all(&autostart).unwrap();
        std::os::unix::fs::symlink(&requests, base.join("requests-link")).unwrap();

        let roots = parse_write_roots(
            &format!(
                "{}, {}:create+edit+delete, {}/../../../autostart",
                base.join("requests-link").display(),
                autostart.display(),
                requests.display()
            ),
            &cas_root,
        )
        .unwrap();
        assert_eq!(roots.len(), 3);
        assert_eq!(roots[0].path, requests, "symlinks resolve to the real directory");
        assert_eq!(
            roots[0].modes,
            BTreeSet::from([OperatorWriteMode::Create, OperatorWriteMode::Edit]),
            "default is create+edit"
        );
        assert!(roots[1].modes.contains(&OperatorWriteMode::Delete));
        assert_eq!(roots[2].path, autostart, "`..` is resolved, not trusted lexically");
        assert!(parse_write_roots("", &cas_root).unwrap().is_empty(), "empty clears");

        for unsafe_root in [
            "/".to_string(),
            "relative/dir".to_string(),
            base.join("missing").display().to_string(),
            base.join("project").display().to_string(),
            cas_root.join("operator").display().to_string(),
            format!("{}:create+bogus", requests.display()),
        ] {
            assert!(
                parse_write_roots(&unsafe_root, &cas_root).is_err(),
                "{unsafe_root} must be refused"
            );
        }
    }

    #[test]
    fn cas_3147_policy_round_trips_and_a_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_operator_policy(dir.path()).unwrap(), OperatorWritePolicy::default());
        let policy = OperatorWritePolicy {
            roots: vec![OperatorWriteRoot {
                path: dir.path().join("r"),
                modes: BTreeSet::from([OperatorWriteMode::Create]),
            }],
            grants: vec![OperatorWriteGrant {
                task: "cas-1169".into(),
                path: dir.path().join("g"),
                modes: BTreeSet::from([OperatorWriteMode::Edit]),
                reason: "INGEST request files".into(),
                granted_at: "2026-10-10T18:00:00Z".into(),
                granted_by: Some("operator-cli".into()),
            }],
        };
        save_operator_policy(dir.path(), &policy).unwrap();
        assert!(operator_policy_path(dir.path()).is_file());
        assert_eq!(load_operator_policy(dir.path()).unwrap(), policy);
    }

    /// cas-3147: `Config::set` (behind `cas config set`, the config TUI and
    /// any agent path that reaches it) can never set write roots; only the
    /// operator CLI path writes the separate policy file.
    #[test]
    fn cas_3147_config_set_refuses_write_roots() {
        let mut config = crate::config::Config::default();
        let error = config.set("factory.write_roots", "/tmp").unwrap_err().to_string();
        assert!(error.contains("operator"), "{error}");
    }

    /// GH #1182: setting project roots refuses only a factory worker or
    /// server cgroup, never the agent environment or missing terminal of the
    /// operator's `! cas …` in a pane; the hook skips hand-edited entries that
    /// are unsafe or do not resolve.
    #[test]
    fn gh_1182_project_roots_refuse_worker_cgroups_and_skip_unsafe_entries() {
        assert_eq!(worker_cgroup_refusal("0::/user.slice/app.slice/app-org.kde.konsole-1.scope/tab(2).scope"), None);
        assert!(worker_cgroup_refusal("0::/user.slice/tab(2).scope/cas-worker-cassy-a-b").is_some());
        assert!(worker_cgroup_refusal("0::/user.slice/cas-server-cassy-daemon").is_some());

        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CLAUDECODE", "1");
        let home = env.home().to_path_buf();
        let cas_root = home.join("projects/a/.cas");
        let models = home.join("models");
        std::fs::create_dir_all(&cas_root).unwrap();
        std::fs::create_dir_all(&models).unwrap();
        let configured = [
            "~/models".to_string(),
            models.display().to_string(),
            "/".to_string(),
            home.display().to_string(),
            home.join("projects/a").display().to_string(),
            cas_root.display().to_string(),
            home.join("missing").display().to_string(),
        ];
        assert_eq!(effective_project_write_roots(&configured, &cas_root), vec![models]);
    }

    /// cas-f0ea0: Linux reads XDG_DOWNLOAD_DIR from user-dirs.dirs and falls
    /// back to ~/Downloads when it is absent, relative or `$HOME` itself;
    /// macOS always uses ~/Downloads.
    #[test]
    fn cas_f0ea0_downloads_dir_resolves_for_linux_and_macos() {
        use DownloadsHost::{Linux, MacOs};
        let home = Path::new("/home/ada");
        let fallback = home.join("Downloads");
        let user_dirs = "# written by xdg-user-dirs-update\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Téléchargements\"\n";
        assert_eq!(downloads_dir_for(Linux, home, Some(user_dirs)), home.join("Téléchargements"));
        assert_eq!(
            downloads_dir_for(Linux, home, Some("XDG_DOWNLOAD_DIR=\"/mnt/dl\"\n")),
            Path::new("/mnt/dl")
        );
        for fallback_case in [None, Some(""), Some("XDG_DOWNLOAD_DIR=\"$HOME/\"\n"), Some("XDG_DOWNLOAD_DIR=\"dl\"\n")] {
            assert_eq!(downloads_dir_for(Linux, home, fallback_case), fallback, "{fallback_case:?}");
        }
        assert_eq!(downloads_dir_for(MacOs, home, Some(user_dirs)), fallback);
    }

    /// cas-ab04: the shared grant path validates the task and path, records
    /// who granted it (a Commander device id), notes it on the task, refuses
    /// closed tasks and empty reasons, and revoke removes it with a note.
    #[test]
    fn cas_ab04_record_and_revoke_grant_record_source_and_task_notes() {
        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let granted = project.path().canonicalize().unwrap().join("requests");
        std::fs::create_dir_all(&granted).unwrap();
        let tasks = crate::store::open_task_store_local(&cas_root).unwrap();
        let mut task = cas_types::Task::new("cas-g1".into(), "ingest".into());
        task.status = cas_types::TaskStatus::InProgress;
        tasks.add(&task).unwrap();
        let device = GrantSource::CommanderDevice { device_id: "dev-phone-1".into() };

        let grant = record_operator_grant(
            &cas_root, tasks.as_ref(), "cas-g1", &granted.display().to_string(), "create+edit", "INGEST files", &device,
        )
        .unwrap();
        assert_eq!(grant.granted_by.as_deref(), Some("commander-device:dev-phone-1"));
        assert_eq!(grant.path, granted);
        let policy = load_operator_policy(&cas_root).unwrap();
        assert_eq!(policy.grants, vec![grant.clone()]);
        let notes = tasks.get("cas-g1").unwrap().notes;
        assert!(notes.contains("operator write grant") && notes.contains("dev-phone-1"), "{notes}");

        assert!(record_operator_grant(&cas_root, tasks.as_ref(), "cas-g1", &granted.display().to_string(), "", " ", &device).is_err(), "a reason is required");
        assert!(record_operator_grant(&cas_root, tasks.as_ref(), "cas-missing", &granted.display().to_string(), "", "r", &device).is_err());

        assert_eq!(revoke_operator_grants(&cas_root, tasks.as_ref(), "cas-g1", None, &device).unwrap(), 1);
        assert!(load_operator_policy(&cas_root).unwrap().grants.is_empty());
        assert!(tasks.get("cas-g1").unwrap().notes.contains("revoked"));

        task.status = cas_types::TaskStatus::Closed;
        tasks.update(&task).unwrap();
        assert!(
            record_operator_grant(&cas_root, tasks.as_ref(), "cas-g1", &granted.display().to_string(), "", "r", &device).is_err(),
            "a closed task cannot be granted"
        );
    }

}
