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
    InvocationContext, OperatorWriteGrant, load_operator_policy, operator_context_refusal,
    operator_policy_path, parse_modes, parse_write_roots, resolve_operator_path,
    save_operator_policy,
};

/// `cas config grant-write`: a one-off write grant bound to one open task.
#[derive(clap::Parser)]
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
    let modes = parse_modes(&args.mode)?;
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
    let granted_at = chrono::Utc::now().to_rfc3339();
    let mut policy = load_operator_policy(cas_root)?;
    policy.grants.retain(|grant| !(grant.task == task.id && grant.path == path));
    policy.grants.push(OperatorWriteGrant {
        task: task.id.clone(),
        path: path.clone(),
        modes,
        reason: args.reason.trim().to_string(),
        granted_at: granted_at.clone(),
    });
    save_operator_policy(cas_root, &policy)?;
    let note = format!(
        "[{}] ✅ DECISION operator write grant (cas-3147): {} ({}) until this task closes. Reason: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M"),
        path.display(),
        args.mode,
        args.reason.trim()
    );
    task_store.append_note(&task.id, &note)?;
    let path_text = path.display().to_string();
    record_event(
        cas_root,
        "operator_write_grant",
        &[("task", task.id.as_str()), ("path", path_text.as_str()), ("mode", args.mode.as_str())],
    );
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
    let path = args
        .path
        .as_deref()
        .map(|path| resolve_operator_path(path, cas_root))
        .transpose()?;
    let mut policy = load_operator_policy(cas_root)?;
    let before = policy.grants.len();
    policy.grants.retain(|grant| {
        !(grant.task == args.task && path.as_ref().is_none_or(|path| &grant.path == path))
    });
    let removed = before - policy.grants.len();
    save_operator_policy(cas_root, &policy)?;
    if removed > 0
        && let Ok(task_store) = crate::store::open_task_store(cas_root)
    {
        let _ = task_store.append_note(
            &args.task,
            &format!(
                "[{}] ✅ DECISION operator write grant revoked (cas-3147): {removed} grant(s)",
                chrono::Utc::now().format("%Y-%m-%d %H:%M")
            ),
        );
    }
    if cli.json {
        println!("{}", serde_json::json!({"task": args.task, "removed": removed}));
    } else {
        println!("Revoked {removed} grant(s) for {}", args.task);
    }
    Ok(())
}
