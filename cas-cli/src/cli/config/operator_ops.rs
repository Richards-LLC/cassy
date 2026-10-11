//! Operator-only write roots and per-task write grants (cas-3147, GH #1169).
//!
//! These commands write `.cas/operator/write-policy.toml`. Each one first
//! refuses any agent context (see [`operator_context_refusal`]) and then asks
//! the operator to confirm at the terminal. The PreToolUse hook separately
//! refuses agent tool calls that run these commands or write the file.

use std::io::{BufRead, Write};
use std::path::Path;

use crate::cli::Cli;
use crate::config::operator_policy::{
    InvocationContext, load_operator_policy, operator_context_refusal, operator_policy_path,
    parse_modes, parse_write_roots, resolve_operator_path, save_operator_policy,
};

/// `cas config grant-write`: a one-off write grant bound to one open task.
///
/// Operator-only, guardrail-grade (not security-grade): agents share your Unix
/// user, so Cassy's tool-call hook is what stops them; this command's own
/// agent checks are defence in depth.
#[derive(clap::Parser)]
#[command(
    after_help = "Guardrail-grade, not security-grade: the operator and the agents share a Unix user. Cassy's PreToolUse hook refuses agents that run this command or write .cas/operator/; the checks here (no agent environment, no agent ancestor, no factory cgroup, a terminal, a typed confirmation) are defence in depth."
)]
pub struct ConfigGrantWriteArgs {
    /// Task the grant is bound to; it ends when the task closes
    #[arg(long)]
    pub task: String,
    /// Directory (or file) outside the worktree the task may write
    #[arg(long)]
    pub path: String,
    /// Modes, e.g. create+edit (default) or create+edit+delete
    #[arg(long, default_value = "create+edit")]
    pub mode: String,
    /// Why the operator grants it; recorded on the task
    #[arg(long)]
    pub reason: String,
}

/// `cas config revoke-write`: remove a task's grants (all, or one path).
#[derive(clap::Parser)]
pub struct ConfigRevokeWriteArgs {
    #[arg(long)]
    pub task: String,
    #[arg(long)]
    pub path: Option<String>,
}

/// cas-0d4f0: refuse to save `after` over `before` when that changes a
/// security-relevant key and `context` (read only when one changes) is not
/// the operator at a terminal. Every config save path calls this.
pub(crate) fn guard_operator_config(
    before: &crate::config::Config,
    after: &crate::config::Config,
    context: impl FnOnce() -> InvocationContext,
) -> anyhow::Result<()> {
    let changed = crate::config::operator_policy::changed_operator_config_keys(before, after);
    if changed.is_empty() {
        return Ok(());
    }
    if let Some(refusal) =
        crate::config::operator_policy::operator_config_refusal(&changed, &context())
    {
        anyhow::bail!(refusal);
    }
    Ok(())
}

fn require_operator() -> anyhow::Result<()> {
    if let Some(refusal) = operator_context_refusal(&InvocationContext::from_process()) {
        anyhow::bail!(refusal);
    }
    Ok(())
}

fn confirm(prompt: &str, expected: &str) -> anyhow::Result<()> {
    let mut out = std::io::stdout();
    write!(out, "{prompt}\nType `{expected}` to confirm: ")?;
    out.flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    if line.trim() != expected {
        anyhow::bail!("not confirmed; nothing changed");
    }
    Ok(())
}

fn record_event(cas_root: &Path, event: &str, fields: &[(&str, &str)]) {
    let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(cas_root, event, fields);
}

/// `cas config set factory.write_roots <value>`.
pub(crate) fn execute_set_write_roots(value: &str, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    require_operator()?;
    let roots = parse_write_roots(value, cas_root)?;
    let listed = if roots.is_empty() {
        "  (none: clears every project write root)".to_string()
    } else {
        roots
            .iter()
            .map(|root| {
                let modes = root.modes.iter().map(|mode| format!("{mode:?}").to_lowercase()).collect::<Vec<_>>().join("+");
                format!("  {} ({modes})", root.path.display())
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    confirm(
        &format!("Project write roots for agents in this project will become:\n{listed}"),
        "ALLOW",
    )?;
    let mut policy = load_operator_policy(cas_root)?;
    policy.roots = roots;
    save_operator_policy(cas_root, &policy)?;
    let paths = policy.roots.iter().map(|root| root.path.display().to_string()).collect::<Vec<_>>().join(",");
    record_event(cas_root, "operator_write_roots_set", &[("roots", paths.as_str())]);
    if cli.json {
        println!("{}", serde_json::json!({"key": "factory.write_roots", "roots": policy.roots.iter().map(|r| r.path.display().to_string()).collect::<Vec<_>>()}));
    } else {
        println!("Set factory.write_roots in {}", operator_policy_path(cas_root).display());
    }
    Ok(())
}

/// `cas config get factory.write_roots`: read-only view of the policy file.
pub(crate) fn execute_get_write_roots(cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let policy = load_operator_policy(cas_root)?;
    let describe = |modes: &std::collections::BTreeSet<_>| {
        modes.iter().map(|mode: &crate::config::operator_policy::OperatorWriteMode| format!("{mode:?}").to_lowercase()).collect::<Vec<_>>().join("+")
    };
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&policy)?);
        return Ok(());
    }
    let roots = policy
        .roots
        .iter()
        .map(|root| format!("{}:{}", root.path.display(), describe(&root.modes)))
        .collect::<Vec<_>>();
    println!("{}", roots.join(","));
    Ok(())
}

/// `cas config grant-write`.
pub(crate) fn execute_grant_write(args: &ConfigGrantWriteArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    require_operator()?;
    if args.reason.trim().is_empty() {
        anyhow::bail!("--reason is required: it is recorded on the task");
    }
    let task_store = crate::store::open_task_store(cas_root)?;
    let task = task_store.get(&args.task)?;
    if task.status == cas_types::TaskStatus::Closed {
        anyhow::bail!("task {} is closed; a grant ends when its task closes", task.id);
    }
    let path = resolve_operator_path(&args.path, cas_root)?;
    // Validate before asking the operator to confirm.
    parse_modes(&args.mode)?;
    confirm(
        &format!(
            "Grant agents working on {} ({}) write access to {} ({}) until the task closes.\nReason: {}",
            task.id,
            task.title,
            path.display(),
            args.mode,
            args.reason.trim()
        ),
        &path.display().to_string(),
    )?;
    let grant = crate::config::operator_policy::record_operator_grant(
        cas_root,
        task_store.as_ref(),
        &task.id,
        &path.display().to_string(),
        &args.mode,
        &args.reason,
        &crate::config::operator_policy::GrantSource::OperatorCli,
    )?;
    let path = grant.path;
    let path_text = path.display().to_string();
    if cli.json {
        println!("{}", serde_json::json!({"task": task.id, "path": path_text, "mode": args.mode}));
    } else {
        println!("Granted {path_text} ({}) to {} until it closes", args.mode, task.id);
    }
    Ok(())
}

/// `cas config revoke-write`.
pub(crate) fn execute_revoke_write(args: &ConfigRevokeWriteArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    require_operator()?;
    let task_store = crate::store::open_task_store(cas_root)?;
    let removed = crate::config::operator_policy::revoke_operator_grants(
        cas_root,
        task_store.as_ref(),
        &args.task,
        args.path.as_deref(),
        &crate::config::operator_policy::GrantSource::OperatorCli,
    )?;
    if cli.json {
        println!("{}", serde_json::json!({"task": args.task, "removed": removed}));
    } else {
        println!("Revoked {removed} grant(s) for {}", args.task);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::config::read_ops::set_config_value;
    use crate::config::Config;

    fn agent() -> InvocationContext {
        InvocationContext {
            env_names: ["HOME", "CLAUDECODE"].into_iter().map(String::from).collect(),
            ancestors: vec!["-bash".into()],
            ..InvocationContext::default()
        }
    }

    fn operator() -> InvocationContext {
        InvocationContext {
            env_names: ["HOME", "PATH"].into_iter().map(String::from).collect(),
            ancestors: vec!["-bash".into()],
            stdin_is_terminal: true,
            stdout_is_terminal: true,
            cgroup: String::new(),
        }
    }

    /// cas-0d4f0: `cas config set` refuses a security-relevant key from an
    /// agent and leaves config.toml unchanged; ordinary keys stay settable,
    /// and the operator at a terminal can still set the key.
    #[test]
    fn cas_0d4f0_config_set_refuses_operator_keys_from_agents() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let mut config = Config::load(&root).unwrap();
        let error = set_config_value(&mut config, "slack.transport", "any", &root, agent)
            .unwrap_err()
            .to_string();
        assert!(error.contains("slack.transport") && error.contains("operator"), "{error}");
        let mut config = Config::load(&root).unwrap();
        let error = set_config_value(&mut config, "verification.enabled", "false", &root, agent)
            .unwrap_err()
            .to_string();
        assert!(error.contains("verification.enabled"), "{error}");
        assert_ne!(
            Config::load(&root).unwrap().get("verification.enabled").as_deref(),
            Some("false")
        );
        assert_ne!(Config::load(&root).unwrap().get("slack.transport").as_deref(), Some("any"));

        let mut config = Config::load(&root).unwrap();
        set_config_value(&mut config, "issues.repo", "example/project", &root, agent).unwrap();
        assert_eq!(
            Config::load(&root).unwrap().get("issues.repo").as_deref(),
            Some("example/project")
        );

        let mut config = Config::load(&root).unwrap();
        set_config_value(&mut config, "slack.transport", "any", &root, operator).unwrap();
        assert_eq!(Config::load(&root).unwrap().get("slack.transport").as_deref(), Some("any"));
    }

    /// cas-0d4f0: reset, import and the line editor save through the same
    /// guard, so a whole-config replacement cannot relax a guard either.
    #[test]
    fn cas_0d4f0_whole_config_saves_refuse_operator_key_changes() {
        let before = Config::default();
        let mut after = before.clone();
        after.set("verification.force_bypass_allowed", "true").unwrap();
        assert!(guard_operator_config(&before, &after, agent).is_err());
        assert!(guard_operator_config(&before, &after, operator).is_ok());
        let mut ordinary = before.clone();
        ordinary.set("sync.min_helpful", "5").unwrap();
        assert!(
            guard_operator_config(&before, &ordinary, || panic!("context read for ordinary key"))
                .is_ok()
        );
    }
}
