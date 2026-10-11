//! Operator-only write policy (cas-3147, GH #1169).
//!
//! Project write roots and one-off per-task write grants widen the factory
//! workspace contract, so only the human operator may set them. Agents run as
//! the operator's Unix user, and nothing in Cassy separates the two at the OS
//! level (see the cas-3147 decision note). The policy is therefore guarded in
//! layers:
//!
//! 1. It lives in its own file, `.cas/operator/write-policy.toml`, never in
//!    `config.toml`, so `Config::set`, the config TUI and any MCP path that
//!    reaches them cannot write it.
//! 2. Only `cas config set factory.write_roots` and `cas config grant-write`
//!    write the file, and only after [`operator_context_refusal`] finds no
//!    agent context and the operator retypes the path at a terminal.
//! 3. The PreToolUse workspace contract refuses every agent tool call that
//!    writes under `.cas/operator/` or runs those two commands.
//!
//! This is guardrail-grade, not security-grade. The operator and the agents
//! share a Unix user, so the PreToolUse hook (layer 3) is the actual gate
//! against agents; the CLI checks are defence in depth. An agent that
//! deliberately hides a same-user write outside its tool calls is out of
//! reach; a hard boundary needs agents under a separate Unix user.
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
//! that read them see the role the factory gave the agent.
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

/// Environment variables only an agent session carries (cas-3147).
const AGENT_ENV_MARKERS: &[&str] = &[
    "CAS_AGENT_NAME",
    "CAS_AGENT_ROLE",
    "CAS_SESSION_ID",
    "CAS_CLONE_PATH",
    "CAS_SUPERVISOR_NAME",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
    "CODEX_THREAD_ID",
];
const AGENT_ENV_PREFIXES: &[&str] = &["CAS_FACTORY_", "CAS_AGENT_", "CLAUDE_CODE_", "CODEX_SANDBOX"];

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
    "verification.enabled",
    "verification.force_bypass_allowed",
    "slack.transport",
    "factory.supervisor_only_mcp",
    "factory.supervisor_only_env",
    "factory.worker_credential_env",
    "qa.evidence_gate",
    "qa.independent_pass",
    "release.claude_account_allowlist",
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

/// cas-3c26: whether this process runs in a factory worker's cgroup. Read
/// once; a process does not change cgroup by itself.
pub fn process_in_factory_worker_cgroup() -> bool {
    static IN_WORKER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IN_WORKER.get_or_init(|| {
        std::fs::read_to_string("/proc/self/cgroup")
            .map(|cgroup| in_factory_worker_cgroup(&cgroup))
            .unwrap_or(false)
    })
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
            HubCommands::Authorize(_) => Some("Authorizing a Commander pairing (`cas hub authorize`)"),
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
    if let Some(marker) = context.env_names.iter().find(|name| {
        AGENT_ENV_MARKERS.contains(&name.as_str())
            || AGENT_ENV_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
    }) {
        return Some(format!(
            "refused: {marker} is set, so this runs inside an agent session. {subject}; {remedy}."
        ));
    }
    if let Some(ancestor) = context.ancestors.iter().find(|line| agent_ancestor(line)) {
        let program = ancestor.split_whitespace().next().unwrap_or(ancestor);
        return Some(format!(
            "refused: this process descends from an agent or Cassy server ({program}). {subject}; {remedy}."
        ));
    }
    if context
        .cgroup
        .split('/')
        .any(|part| part.starts_with("cas-worker-") || part.starts_with("cas-server-"))
    {
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
        let env_spoof = context(&[], &["bash -c cas hub pair", "claude --dangerously-skip-permissions"], true);
        let refused = operator_action_refusal(action, remedy, &env_spoof).expect("env spoof refused");
        assert!(refused.contains("descends from an agent"), "{refused}");
        assert!(refused.contains(action), "{refused}");

        // `script -qc 'cas hub pair'` from Codex: a terminal, but an agent ancestor.
        let pty = context(&[], &["script -qc cas hub pair", "/usr/bin/bash", "/opt/codex/bin/codex exec"], true);
        assert!(operator_action_refusal(action, remedy, &pty).is_some());

        // A role variable alone is an agent marker.
        let spoofed_role = context(&["CAS_AGENT_ROLE"], &[], true);
        assert!(operator_action_refusal(action, remedy, &spoofed_role).is_some());

        // A worker scope, even with a clean environment and a terminal.
        let mut worker = context(&[], &[], true);
        worker.cgroup = "0::/user.slice/cas-worker-cas-src-mighty-crane-74/cas-private-server-1".into();
        let refused = operator_action_refusal(action, remedy, &worker).expect("worker cgroup refused");
        assert!(refused.contains("worker cgroup"), "{refused}");

        // The operator at a terminal passes; without a terminal it is refused.
        assert_eq!(operator_action_refusal(action, remedy, &context(&[], &["-bash"], true)), None);
        assert!(operator_action_refusal(action, remedy, &context(&[], &["-bash"], false)).is_some());
    }

    /// cas-3c26: `--yes` paths drop only the terminal requirement.
    #[test]
    fn cas_3c26_unattended_operator_paths_still_refuse_agents() {
        let (action, remedy) = ("Authorizing a Commander pairing", "run it yourself");
        assert_eq!(agent_context_refusal(action, remedy, &context(&[], &["cron"], false)), None);
        assert!(agent_context_refusal(action, remedy, &context(&[], &["claude"], false)).is_some());
        assert!(agent_context_refusal(action, remedy, &context(&["CLAUDECODE"], &[], true)).is_some());
    }

    #[test]
    fn cas_3c26_worker_cgroups_are_recognized_and_server_scopes_are_not() {
        assert!(in_factory_worker_cgroup("0::/user.slice/x.scope/cas-worker-cas-src-a-b"));
        assert!(in_factory_worker_cgroup("0::/user.slice/cas-worker-a/cas-private-server-9"));
        assert!(!in_factory_worker_cgroup("0::/user.slice/cas-server-hub"));
        assert!(!in_factory_worker_cgroup("0::/user.slice/user@1000.service/app.slice"));
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
            assert!(operator_only_cas_invocation(&words(line)).is_some(), "{line}");
        }
        for line in ["hub status", "factory status", "task list", "hub", "config get factory.write_roots"] {
            assert_eq!(operator_only_cas_invocation(&words(line)), None, "{line}");
        }
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
            ("verification.enabled", "false"),
            ("verification.force_bypass_allowed", "true"),
            ("slack.transport", "any"),
            ("factory.supervisor_only_mcp", ""),
            ("factory.worker_credential_env", "GH_TOKEN"),
            ("qa.evidence_gate", "false"),
            ("release.claude_account_allowlist", "agent@example.com"),
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
