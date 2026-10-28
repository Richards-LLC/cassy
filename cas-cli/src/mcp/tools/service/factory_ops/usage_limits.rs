//! Report a harness usage limit without changing its account or restarting it.
use super::*;
use std::path::Path;

/// The episode survives registration changes when the same transcript resumes.
pub(crate) fn episode_key(session: &str, worker: &str, first_evidence: &str) -> String {
    serde_json::json!([session, worker, first_evidence]).to_string()
}

/// Called on the store worker. Only the exhausted account is read: there is no
/// replacement selection, recipe mutation, transcript copy or lifecycle request.
pub(crate) fn record_usage_limit(
    root: &Path,
    agent_id: &str,
    occurrence: &str,
) -> anyhow::Result<String> {
    let agent = crate::store::open_agent_store(root)?.get(agent_id)?;
    anyhow::ensure!(agent.role == cas_types::AgentRole::Worker, "not a worker");
    let cli = worker_cli_from_agent(&agent);
    let provider = match cli {
        cas_mux::SupervisorCli::Claude => crate::accounts::Provider::Claude,
        cas_mux::SupervisorCli::Codex => crate::accounts::Provider::Codex,
        _ => anyhow::bail!("unsupported usage-limit provider"),
    };
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("account home unavailable"))?;
    let folder = agent
        .metadata
        .get("worker_account_dir")
        .map(|raw| normalized_worker_account_path(raw))
        .unwrap_or_else(|| {
            home.join(if provider == crate::accounts::Provider::Claude {
                ".claude"
            } else {
                ".codex"
            })
        });
    let account = crate::account_registry::account(provider, &home, folder);
    let account_name = if account.display_name == account.name {
        account.name.clone()
    } else {
        format!("{} ({})", account.display_name, account.name)
    };
    let task_ids: Vec<_> = crate::store::open_task_store(root)?
        .list(None)?
        .into_iter()
        .filter(|task| task.assignee.as_deref() == Some(&agent.name) && !task.is_terminal())
        .map(|task| task.id)
        .collect();
    let tasks = if task_ids.is_empty() {
        "none held".to_owned()
    } else {
        task_ids.join(", ")
    };
    let mut detail = crate::cloud::presence::wrap_line(&format!(
        "Worker {} ({} account {account_name}), task {tasks}: usage limit; waiting for reset. No restart queued.",
        agent.name,
        provider.name()
    ));
    let usage = crate::cli::limits::account_usage(provider, &account.folder);
    let now = chrono::Utc::now();
    if usage.windows.is_empty() {
        detail.push_str("reset time unavailable (provider usage unavailable).\n");
    }
    for window in usage.windows {
        detail.push_str(&crate::cli::limits::window_summary(&window, now));
        detail.push('\n');
        match window.resets_at {
            Some(reset) => detail.push_str(&format!(
                "{} reset: {}\n",
                window.name,
                reset
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %I:%M %p %Z")
            )),
            None => detail.push_str(&format!("{} reset time unavailable.\n", window.name)),
        }
    }
    let detail = detail.trim_end().to_owned();
    // Keep historical episode rows, including their old request IDs, compatible.
    // A fresh notice always describes the current report-only policy rather than
    // replaying an obsolete 'queued switch' decision from a previous release.
    crate::store::open_spawn_queue_store(root)?.record_limit_episode(occurrence, &detail)?;
    Ok(detail)
}
