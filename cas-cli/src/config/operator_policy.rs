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
//! An agent that deliberately obfuscates a same-user write outside its tool
//! calls is out of reach; a hard boundary needs agents under a separate Unix
//! user.

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
}

/// Why this process may not change the operator policy, or `None` when it
/// looks like an operator at a terminal: no agent environment, no agent
/// process among its ancestors, and an interactive terminal.
pub fn operator_context_refusal(context: &InvocationContext) -> Option<String> {
    if let Some(marker) = context.env_names.iter().find(|name| {
        AGENT_ENV_MARKERS.contains(&name.as_str())
            || AGENT_ENV_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
    }) {
        return Some(format!(
            "refused: {marker} is set, so this runs inside an agent session. Write roots and grants are operator-only; run the command yourself from your own terminal."
        ));
    }
    if let Some(ancestor) = context.ancestors.iter().find(|line| agent_ancestor(line)) {
        let program = ancestor.split_whitespace().next().unwrap_or(ancestor);
        return Some(format!(
            "refused: this process descends from an agent or Cassy server ({program}). Write roots and grants are operator-only; run the command yourself from your own terminal."
        ));
    }
    if !(context.stdin_is_terminal && context.stdout_is_terminal) {
        return Some(
            "refused: write roots and grants need an interactive terminal so the operator can confirm them."
                .to_string(),
        );
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
        let refusal = operator_context_refusal(&context(&["HOME"], &["-bash"], false))
            .expect("no terminal");
        assert!(refusal.contains("terminal"), "{refusal}");
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
}
