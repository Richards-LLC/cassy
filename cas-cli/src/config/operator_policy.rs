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
    let _ = cas_root;
    Ok(OperatorWritePolicy::default())
}

/// Replace the policy file atomically.
pub fn save_operator_policy(cas_root: &Path, policy: &OperatorWritePolicy) -> anyhow::Result<()> {
    let _ = (cas_root, policy);
    Ok(())
}

/// Parse `create+edit` style modes; empty means the default `create+edit`.
pub fn parse_modes(value: &str) -> anyhow::Result<BTreeSet<OperatorWriteMode>> {
    let _ = value;
    Ok(BTreeSet::new())
}

/// Parse `cas config set factory.write_roots` values: comma-separated
/// `path[:modes]` entries, for example `~/a:create+edit,~/b`. Each path must
/// exist; it is resolved to a canonical absolute path. `""` clears the roots.
pub fn parse_write_roots(value: &str, cas_root: &Path) -> anyhow::Result<Vec<OperatorWriteRoot>> {
    let _ = (value, cas_root);
    Ok(Vec::new())
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
    let _ = context;
    None
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
