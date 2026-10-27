//! Automatic account recovery uses the ordinary recycle queue, with a durable
//! episode decision and a generation check before stopping a worker.
use super::*;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct LimitRecycle {
    pub agent_id: String,
    pub episode: String,
    pub session: String,
    pub old_account: PathBuf,
    pub task_ids: Vec<String>,
    pub cwd: PathBuf,
}

/// The episode survives registration changes when the same transcript resumes.
pub(crate) fn episode_key(session: &str, worker: &str, first_evidence: &str) -> String {
    serde_json::json!([session, worker, first_evidence]).to_string()
}

/// Called on the store worker. A recorded refusal is replayed as well as a
/// queued switch, so restarts cannot turn a single limit into a retry loop.
pub(crate) fn queue_limit_switch(
    root: &Path,
    agent_id: &str,
    occurrence: &str,
) -> anyhow::Result<String> {
    let queue = crate::store::open_spawn_queue_store(root)?;
    if let Some((_, detail)) = queue.limit_episode(occurrence)? {
        return Ok(detail);
    }
    let agents = crate::store::open_agent_store(root)?;
    let agent = agents.get(agent_id)?;
    anyhow::ensure!(
        agent.role == cas_types::AgentRole::Worker,
        "supervisor account cannot auto-switch"
    );
    let session = agent
        .factory_session
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("worker has no factory session"))?;
    let tasks = crate::store::open_task_store(root)?.list(None)?;
    let task_ids: Vec<_> = tasks
        .iter()
        .filter(|t| t.assignee.as_deref() == Some(&agent.name) && !t.is_terminal())
        .map(|t| t.id.clone())
        .collect();
    if task_ids.is_empty() {
        return Ok(format!(
            "Worker {} hit its usage limit; no held task to resume",
            agent.name
        ));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("account home unavailable"))?;
    let cli = worker_cli_from_agent(&agent);
    let provider = match cli {
        cas_mux::SupervisorCli::Claude => crate::accounts::Provider::Claude,
        cas_mux::SupervisorCli::Codex => crate::accounts::Provider::Codex,
        _ => anyhow::bail!("unsupported usage-limit provider"),
    };
    let accounts = crate::account_registry::collect_for(provider, &home, true)?;
    let old = agent
        .metadata
        .get("worker_account_dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            home.join(if cli == cas_mux::SupervisorCli::Claude {
                ".claude"
            } else {
                ".codex"
            })
        });
    let cwd = match resolve_worker_clone_path(root, &agent) {
        WorkerClonePathResolve::Ready(path) => path,
        WorkerClonePathResolve::NotOnDisk { .. } => {
            anyhow::bail!("worker worktree missing; nothing stopped")
        }
    };
    let metadata = LimitRecycle {
        agent_id: agent.id.clone(),
        episode: occurrence.to_owned(),
        session: session.to_owned(),
        old_account: old,
        task_ids,
        cwd,
    };
    let (spec, detail) = plan_switch(
        &agent,
        &metadata,
        &accounts,
        chrono::Utc::now(),
        &crate::account_selection::load_policy(Some(root)),
        preflight_limit_spec,
    );
    let (_, persisted) =
        queue.record_limit_episode(occurrence, &agent.name, session, spec.as_deref(), &detail)?;
    Ok(persisted)
}

fn same_folder(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

fn plan_switch(
    agent: &cas_types::Agent,
    meta: &LimitRecycle,
    accounts: &[crate::account_registry::Account],
    now: chrono::DateTime<chrono::Utc>,
    policy: &crate::config::AccountsConfig,
    preflight: impl FnOnce(&cas_mux::WorkerSpec) -> Result<(), String>,
) -> (Option<String>, String) {
    let cli = worker_cli_from_agent(agent);
    let provider = if cli == cas_mux::SupervisorCli::Claude {
        crate::accounts::Provider::Claude
    } else {
        crate::accounts::Provider::Codex
    };
    let old_row = accounts
        .iter()
        .find(|a| same_folder(&a.folder, &meta.old_account));
    // Exclude aliases of the exhausted subscription as well as its directory.
    let candidates: Vec<_> = accounts
        .iter()
        .filter(|a| {
            !same_folder(&a.folder, &meta.old_account)
                && !old_row.is_some_and(|old| old.identity.is_some() && old.identity == a.identity)
        })
        .cloned()
        .collect();
    let selection = crate::account_selection::recommend(&candidates, provider, now, policy);
    let tasks = meta.task_ids.join(", ");
    let old_name = old_row
        .map(|a| a.display_name.clone())
        .unwrap_or_else(|| meta.old_account.display().to_string());
    let Some(next) = selection.recommended else {
        let reset = accounts
            .iter()
            .filter(|a| a.provider == provider)
            .flat_map(|a| &a.windows)
            .filter_map(|w| w.resets_at)
            .filter(|t| *t > now)
            .min()
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "unknown (usage/reset unavailable)".into());
        return (
            None,
            format!(
                "Worker {} ({old_name}), task {tasks}: usage limit; no eligible account until {reset}. No restart queued.",
                agent.name
            ),
        );
    };
    let mut spec = worker_spec_from_agent(agent);
    spec.config_dir = Some(next.account.folder.to_string_lossy().into_owned());
    if let Err(error) = preflight(&spec) {
        return (
            None,
            format!(
                "Worker {}, task {tasks}: blocked switch {old_name} -> {} after usage limit: {error}. Existing pane kept.",
                agent.name, next.account.display_name
            ),
        );
    }
    let mut json = serde_json::to_value(&spec).expect("worker spec serializes");
    json["limit_recycle"] = serde_json::to_value(meta).expect("limit metadata serializes");
    (
        Some(json.to_string()),
        format!(
            "Worker {}, task {tasks}: usage limit; queued account switch {old_name} -> {} ({}). Worktree, branch and task bindings kept.",
            agent.name, next.account.display_name, next.reason
        ),
    )
}

pub(crate) fn preflight_limit_spec(spec: &cas_mux::WorkerSpec) -> Result<(), String> {
    cas_factory::validate_explicit(spec, &cas_factory::CapabilitySnapshot::default())
        .map_err(|e| e.to_string())?;
    let dir = spec
        .config_dir
        .as_deref()
        .ok_or("replacement account missing")?;
    match spec.cli {
        cas_mux::SupervisorCli::Claude => preflight_claude_config_dir(dir)?,
        cas_mux::SupervisorCli::Codex => preflight_codex_config_dir(dir)?,
        _ => return Err("unsupported usage-limit provider".into()),
    }
    preflight_account_auth(std::slice::from_ref(spec))?;
    Ok(())
}

/// Revalidate generation and account immediately before stop. No git mutation
/// is used anywhere in the limit path, including dirty and unpushed trees.
pub(crate) fn prepare_limit_resume(
    root: &Path,
    name: &str,
    meta: &LimitRecycle,
    spec: &cas_mux::WorkerSpec,
) -> anyhow::Result<(Option<String>, String)> {
    let agent = crate::store::open_agent_store(root)?.get(&meta.agent_id)?;
    anyhow::ensure!(
        agent.role == cas_types::AgentRole::Worker
            && agent.name == name
            && agent.factory_session.as_deref() == Some(&meta.session)
            && matches!(
                agent.status,
                cas_types::AgentStatus::Active | cas_types::AgentStatus::Idle
            ),
        "worker generation changed; nothing stopped"
    );
    let evidence = worker_usage_limit_evidence(root, &agent);
    anyhow::ensure!(
        matches!(evidence, UsageLimitEvidence::Limited { ref first_evidence }
        if episode_key(&meta.session, name, first_evidence) == meta.episode),
        "limit episode changed; nothing stopped"
    );
    preflight_limit_spec(spec).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(meta.cwd.is_dir(), "worktree missing; nothing stopped");
    spec.cli
        .backend()
        .prepare_workdir(&meta.cwd, spec.config_dir.as_deref())
        .map_err(|e| anyhow::anyhow!("replacement workspace preflight failed before stop: {e}"))?;
    let status =
        run_git(&meta.cwd, &["status", "--short"]).unwrap_or_else(|e| format!("unavailable: {e}"));
    let unpushed = run_git(&meta.cwd, &["log", "--oneline", "@{u}..HEAD"])
        .unwrap_or_else(|e| format!("upstream unavailable: {e}"));
    let branch =
        run_git(&meta.cwd, &["branch", "--show-current"]).unwrap_or_else(|_| "unknown".into());
    let (resume, why) = copy_resume_transcript(root, &agent, meta, spec);
    Ok((
        resume,
        format!(
            "Account limit recovery: {why}.\nKept worktree: {}\nBranch: {branch}\nUncommitted changes:\n{}\nUnpushed commits:\n{}\nContinue the assigned task; preserve these changes.\n",
            meta.cwd.display(),
            if status.is_empty() { "(none)" } else { &status },
            if unpushed.is_empty() {
                "(none)"
            } else {
                &unpushed
            }
        ),
    ))
}

fn copy_resume_transcript(
    root: &Path,
    agent: &cas_types::Agent,
    meta: &LimitRecycle,
    spec: &cas_mux::WorkerSpec,
) -> (Option<String>, String) {
    let result = (|| -> anyhow::Result<String> {
        let source = worker_transcript_path_for_agent(root, agent)
            .ok_or_else(|| anyhow::anyhow!("transcript unavailable"))?;
        let relative = source.strip_prefix(&meta.old_account)?;
        anyhow::ensure!(
            !relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
            "invalid transcript path"
        );
        let id = if spec.cli == cas_mux::SupervisorCli::Codex {
            use std::io::BufRead;
            let line = std::io::BufReader::new(std::fs::File::open(&source)?)
                .lines()
                .next()
                .transpose()?
                .unwrap_or_default();
            let record: serde_json::Value = serde_json::from_str(&line)?;
            record
                .pointer("/payload/id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Codex session id unavailable"))?
                .to_owned()
        } else {
            agent
                .cc_session_id
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Claude session id unavailable"))?
        };
        let target = Path::new(
            spec.config_dir
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("replacement missing"))?,
        )
        .join(relative);
        std::fs::create_dir_all(
            target
                .parent()
                .ok_or_else(|| anyhow::anyhow!("transcript parent missing"))?,
        )?;
        // Never overwrite a transcript already present in the destination.
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        std::io::copy(&mut std::fs::File::open(&source)?, &mut output)?;
        Ok(id)
    })();
    match result {
        Ok(id) => (Some(id), "continuing copied harness session".into()),
        Err(error) => (
            None,
            format!(
                "fresh start on the same task because transcript continuation was unavailable: {error}"
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paired_provider_switch_excludes_limited_identity_and_persists_recipe_or_refusal() {
        for (provider, cli) in [
            (crate::accounts::Provider::Claude, "claude"),
            (crate::accounts::Provider::Codex, "codex"),
        ] {
            let mut agent = cas_types::Agent::new_with_role(
                "old-agent".into(),
                "worker".into(),
                cas_types::AgentRole::Worker,
            );
            agent.metadata.insert("worker_cli".into(), cli.into());
            let old = crate::account_selection::tests::row(provider, "old", 1, 10., 10.);
            let mut alias = crate::account_selection::tests::row(provider, "alias", 2, 10., 10.);
            let mut old = old;
            old.identity = Some("limited-subscription".into());
            alias.identity = old.identity.clone();
            let next = crate::account_selection::tests::row(provider, "next", 8, 20., 20.);
            let meta = LimitRecycle {
                agent_id: agent.id.clone(),
                episode: episode_key("session", "worker", "first"),
                session: "session".into(),
                old_account: old.folder.clone(),
                task_ids: vec!["cas-task".into()],
                cwd: PathBuf::from("/kept/worktree"),
            };
            let accounts = vec![old, alias, next.clone()];
            let now = chrono::DateTime::parse_from_rfc3339("2026-10-27T12:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc);
            let (json, detail) =
                plan_switch(&agent, &meta, &accounts, now, &Default::default(), |spec| {
                    assert_eq!(spec.config_dir.as_deref(), next.folder.to_str());
                    Ok(())
                });
            let json: serde_json::Value = serde_json::from_str(&json.unwrap()).unwrap();
            assert_eq!(json["limit_recycle"]["cwd"], "/kept/worktree");
            assert_eq!(json["limit_recycle"]["task_ids"][0], "cas-task");
            assert!(detail.contains("old -> next"), "{detail}");
            let (json, detail) =
                plan_switch(&agent, &meta, &accounts, now, &Default::default(), |_| {
                    Err("fixture account invalid".into())
                });
            assert!(json.is_none());
            assert!(detail.contains("blocked switch"));
            let (json, detail) = plan_switch(
                &agent,
                &meta,
                &accounts[..2],
                now,
                &Default::default(),
                |_| panic!("no eligible account must not preflight"),
            );
            assert!(json.is_none());
            assert!(
                detail.contains("no eligible account until 2026-10-27T13:00:00+00:00"),
                "{detail}"
            );
            // A new registration resuming the same refusal uses the same durable key.
            assert_eq!(episode_key("session", "worker", "first"), meta.episode);
        }
    }
}
