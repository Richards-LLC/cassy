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

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LimitSwitchDecision {
    pub detail: String,
    /// Taken from the durable request id, including when a lifecycle request
    /// prevented queueing or this episode is replayed after a restart.
    pub switch_queued: bool,
}

impl From<(Option<i64>, String)> for LimitSwitchDecision {
    fn from((request_id, detail): (Option<i64>, String)) -> Self {
        Self {
            detail,
            switch_queued: request_id.is_some(),
        }
    }
}

/// Called on the store worker. A recorded refusal is replayed as well as a
/// queued switch, so restarts cannot turn a single limit into a retry loop.
pub(crate) fn queue_limit_switch(
    root: &Path,
    agent_id: &str,
    occurrence: &str,
) -> anyhow::Result<LimitSwitchDecision> {
    queue_limit_switch_with(
        root,
        agent_id,
        occurrence,
        chrono::Utc::now(),
        |provider| {
            let home = dirs::home_dir().ok_or_else(|| std::io::Error::other("home unavailable"))?;
            crate::account_registry::collect_for(provider, &home, true)
        },
        preflight_limit_spec,
    )
}

fn queue_limit_switch_with(
    root: &Path,
    agent_id: &str,
    occurrence: &str,
    now: chrono::DateTime<chrono::Utc>,
    collect: impl FnOnce(
        crate::accounts::Provider,
    ) -> std::io::Result<Vec<crate::account_registry::Account>>,
    preflight: impl FnOnce(&cas_mux::WorkerSpec) -> Result<(), String>,
) -> anyhow::Result<LimitSwitchDecision> {
    let queue = crate::store::open_spawn_queue_store(root)?;
    if let Some(recorded) = queue.limit_episode(occurrence)? {
        return Ok(recorded.into());
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
        return Ok(LimitSwitchDecision {
            detail: format!("Worker {} hit its usage limit; no held task to resume", agent.name),
            switch_queued: false,
        });
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("account home unavailable"))?;
    let cli = worker_cli_from_agent(&agent);
    let provider = match cli {
        cas_mux::SupervisorCli::Claude => crate::accounts::Provider::Claude,
        cas_mux::SupervisorCli::Codex => crate::accounts::Provider::Codex,
        _ => anyhow::bail!("unsupported usage-limit provider"),
    };
    let accounts = collect(provider)?;
    let old = agent
        .metadata
        .get("worker_account_dir")
        .map(|raw| normalized_worker_account_path(raw))
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
        now,
        &crate::account_selection::load_policy(Some(root)),
        preflight,
    );
    Ok(queue
        .record_limit_episode(occurrence, &agent.name, session, spec.as_deref(), &detail)?
        .into())
}

fn same_folder(a: &Path, b: &Path) -> bool {
    normalized_worker_account_path(&a.to_string_lossy())
        == normalized_worker_account_path(&b.to_string_lossy())
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
        .map(|a| {
            if a.display_name == a.name {
                a.name.clone()
            } else {
                format!("{} ({})", a.display_name, a.name)
            }
        })
        .unwrap_or_else(|| {
            let folder = meta.old_account.file_name().and_then(|name| name.to_str()).unwrap_or("unknown account");
            folder.strip_prefix(provider.prefix())
                .unwrap_or(if folder == ".claude" || folder == ".codex" { "main" } else { folder })
                .to_owned()
        });
    let Some(next) = selection.recommended else {
        // Predict each account's transition through the same policy used to
        // select it. A stale under-threshold snapshot for the limited account
        // cannot become eligible because an unrelated account resets.
        let reset = accounts.iter().filter(|a| a.provider == provider)
            .filter_map(|account| {
                let row = std::slice::from_ref(account);
                if crate::account_selection::recommend(row, provider, now, policy).recommended.is_some() {
                    return None;
                }
                let resets = account.windows.iter()
                    .filter_map(|window| window.resets_at)
                    .filter(|at| *at > now)
                    .collect::<std::collections::BTreeSet<_>>();
                resets.into_iter().find(|at| {
                    crate::account_selection::recommend(row, provider, *at, policy).recommended.is_some()
                })
            })
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
    let next_name = if next.account.display_name == next.account.name {
        next.account.name.clone()
    } else {
        format!("{} ({})", next.account.display_name, next.account.name)
    };
    let mut spec = worker_spec_from_agent(agent);
    spec.config_dir = Some(next.account.folder.to_string_lossy().into_owned());
    if let Err(error) = preflight(&spec) {
        return (
            None,
            format!(
                "Worker {}, task {tasks}: blocked switch {old_name} -> {} after usage limit: {error}. Existing pane kept.",
                agent.name, next_name
            ),
        );
    }
    let mut json = serde_json::to_value(&spec).expect("worker spec serializes");
    json["limit_recycle"] = serde_json::to_value(meta).expect("limit metadata serializes");
    (
        Some(json.to_string()),
        format!(
            "Worker {}, task {tasks}: usage limit; queued account switch {old_name} -> {} ({}). Worktree, branch and task bindings kept.",
            agent.name, next_name, next.reason
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
    let current_account = agent
        .metadata
        .get("worker_account_dir")
        .map(|raw| normalized_worker_account_path(raw))
        .unwrap_or_else(|| meta.old_account.clone());
    anyhow::ensure!(
        same_folder(&current_account, &meta.old_account),
        "worker account changed; nothing stopped"
    );
    let held = crate::store::open_task_store(root)?.list(None)?;
    anyhow::ensure!(
        held.iter()
            .any(|task| task.assignee.as_deref() == Some(name)
                && !task.is_terminal()
                && meta.task_ids.contains(&task.id)),
        "task binding changed; nothing stopped"
    );
    let evidence = worker_usage_limit_evidence(root, &agent);
    anyhow::ensure!(
        matches!(evidence, UsageLimitEvidence::Limited { ref first_evidence }
        if episode_key(&meta.session, name, first_evidence) == meta.episode),
        "limit episode changed; nothing stopped"
    );
    let provider = if spec.cli == cas_mux::SupervisorCli::Claude {
        crate::accounts::Provider::Claude
    } else {
        crate::accounts::Provider::Codex
    };
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("account home unavailable"))?;
    let selected = spec
        .config_dir
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("replacement missing"))?;
    let rows: Vec<_> = crate::account_registry::collect_for(provider, &home, true)?
        .into_iter()
        .filter(|row| same_folder(&row.folder, Path::new(selected)))
        .collect();
    anyhow::ensure!(
        crate::account_selection::recommend(
            &rows,
            provider,
            chrono::Utc::now(),
            &crate::account_selection::load_policy(Some(root))
        )
        .recommended
        .is_some(),
        "replacement account no longer eligible; nothing stopped"
    );
    preflight_limit_spec(spec).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(meta.cwd.is_dir(), "worktree missing; nothing stopped");
    spec.cli
        .backend()
        .prepare_workdir(&meta.cwd, spec.config_dir.as_deref())
        .map_err(|e| anyhow::anyhow!("replacement workspace preflight failed before stop: {e}"))?;
    let state = resume_git_state(&meta.cwd);
    let (resume, why) = copy_resume_transcript(root, &agent, meta, spec);
    Ok((
        resume,
        format!(
            "Account limit recovery: {why}.\n{state}\nContinue the assigned task; preserve these changes.\n"
        ),
    ))
}

fn resume_git_state(cwd: &Path) -> String {
    let status = run_git(cwd, &["status", "--short"]);
    let unpushed = run_git(cwd, &["log", "--oneline", "@{u}..HEAD"])
        .or_else(|_| run_git(cwd, &["log", "--oneline", "HEAD", "--not", "--remotes"]));
    let branch = run_git(cwd, &["branch", "--show-current"]).unwrap_or_else(|_| "unknown".into());
    let render = |result: Result<String, String>| match result {
        Ok(value) => format!(
            "{}\n{}",
            value.lines().count(),
            if value.is_empty() { "(none)" } else { &value }
        ),
        Err(error) => format!("unknown\nunavailable: {error}"),
    };
    format!(
        "Kept worktree: {}\nBranch: {branch}\nUncommitted changes: {}\nUnpushed commits (upstream, or remote refs when unset): {}\n",
        cwd.display(),
        render(status),
        render(unpushed)
    )
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
        let copied = std::io::copy(&mut std::fs::File::open(&source)?, &mut output);
        if let Err(error) = copied {
            drop(output);
            let _ = std::fs::remove_file(&target);
            return Err(error.into());
        }
        output.sync_all()?;
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
    fn paired_provider_transcripts_copy_before_stop_and_existing_targets_are_not_overwritten() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "HOME",
            home.path().to_str().unwrap(),
        )]);
        let cwd = home.path().join("worktree");
        std::fs::create_dir_all(&cwd).unwrap();
        let id = "019f84af-3121-7950-ba14-b01db2dad6c7";
        for cli in [
            cas_mux::SupervisorCli::Claude,
            cas_mux::SupervisorCli::Codex,
        ] {
            let old = home.path().join(format!(".{}-old", cli.backend().name()));
            let next = home.path().join(format!(".{}-next", cli.backend().name()));
            let relative = if cli == cas_mux::SupervisorCli::Claude {
                PathBuf::from("projects")
                    .join(escaped_project_slug(cwd.to_str().unwrap()))
                    .join(format!("{id}.jsonl"))
            } else {
                PathBuf::from(format!("sessions/2026/10/27/rollout-{id}.jsonl"))
            };
            let source = old.join(&relative);
            std::fs::create_dir_all(source.parent().unwrap()).unwrap();
            let body = format!(
                "{}\n",
                serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":cwd,"timestamp":"2026-10-27T12:00:00Z"}})
            );
            std::fs::write(&source, &body).unwrap();
            let mut agent = cas_types::Agent::new_with_role(
                "agent".into(),
                "worker".into(),
                cas_types::AgentRole::Worker,
            );
            agent.cc_session_id = Some(id.into());
            agent
                .metadata
                .insert("worker_cli".into(), cli.backend().name().into());
            agent.metadata.insert(
                "worker_account_dir".into(),
                format!("~/.{}-old", cli.backend().name()),
            );
            agent
                .metadata
                .insert("clone_path".into(), cwd.to_string_lossy().into());
            assert!(same_folder(
                Path::new(agent.metadata.get("worker_account_dir").unwrap()),
                &old
            ));
            let meta = LimitRecycle {
                agent_id: agent.id.clone(),
                episode: "episode".into(),
                session: "session".into(),
                old_account: old,
                task_ids: vec!["cas-held".into()],
                cwd: cwd.clone(),
            };
            let mut spec = worker_spec_from_agent(&agent);
            spec.config_dir = Some(next.to_string_lossy().into());
            let (resume, why) = copy_resume_transcript(home.path(), &agent, &meta, &spec);
            assert_eq!(resume.as_deref(), Some(id), "{cli:?}: {why}");
            assert_eq!(std::fs::read_to_string(next.join(&relative)).unwrap(), body);
            assert_eq!(std::fs::read_to_string(&source).unwrap(), body);
            let (resume, why) = copy_resume_transcript(home.path(), &agent, &meta, &spec);
            assert!(resume.is_none());
            assert!(why.contains("fresh start on the same task"));
            assert_eq!(std::fs::read_to_string(next.join(&relative)).unwrap(), body);
        }
    }

    #[test]
    fn fixture_queue_keeps_dirty_worktree_task_branch_and_dedupes_after_reopen() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("project");
            std::fs::create_dir_all(&project).unwrap();
            let root = crate::store::init_cas_dir(&project).unwrap();
            let cwd = root.join("worktrees/worker");
            std::fs::create_dir_all(&cwd).unwrap();
            for args in [
                &["init"][..],
                &["config", "user.name", "Fixture"],
                &["config", "user.email", "fixture@example.invalid"],
                &["commit", "--allow-empty", "-m", "unpublished"],
                &["checkout", "-b", "factory/held-task"],
            ] {
                assert!(
                    std::process::Command::new("git")
                        .args(args)
                        .current_dir(&cwd)
                        .output()
                        .unwrap()
                        .status
                        .success()
                );
            }
            std::fs::write(cwd.join("uncommitted.txt"), b"keep every byte\0").unwrap();
            let mut agent = cas_types::Agent::new_with_role(
                "old-agent".into(),
                "worker".into(),
                cas_types::AgentRole::Worker,
            );
            agent.factory_session = Some("session".into());
            agent.metadata.insert("worker_cli".into(), "codex".into());
            agent
                .metadata
                .insert("clone_path".into(), cwd.to_string_lossy().into());
            let provider = crate::accounts::Provider::Codex;
            let old = crate::account_selection::tests::row(provider, "old", 1, 100., 100.);
            let next = crate::account_selection::tests::row(provider, "next", 8, 20., 20.);
            agent.metadata.insert(
                "worker_account_dir".into(),
                old.folder.to_string_lossy().into(),
            );
            let agents = crate::store::open_agent_store(&root).unwrap();
            agents.register(&agent).unwrap();
            let mut task = cas_types::Task::new("cas-held".into(), "unfinished".into());
            task.assignee = Some("worker".into());
            task.status = cas_types::TaskStatus::InProgress;
            crate::store::open_task_store(&root)
                .unwrap()
                .add(&task)
                .unwrap();
            let now = "2026-10-27T12:00:00Z".parse().unwrap();
            let episode = episode_key("session", "worker", "first");
            let detail = queue_limit_switch_with(
                &root,
                &agent.id,
                &episode,
                now,
                |_| Ok(vec![old, next]),
                |_| Ok(()),
            )
            .unwrap();
            assert!(detail.detail.contains("old -> next"));
            assert!(detail.switch_queued);
            let queue = crate::store::open_spawn_queue_store(&root).unwrap();
            let rows = queue.peek(10).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].action, cas_store::SpawnAction::Recycle);
            assert_eq!(rows[0].worker_names, vec!["worker"]);
            let replay = queue_limit_switch_with(
                &root,
                &agent.id,
                &episode,
                now,
                |_| panic!("persisted episode must not rescan accounts"),
                |_| panic!("persisted episode must not preflight"),
            )
            .unwrap();
            assert_eq!(detail, replay);
            assert_eq!(queue.peek(10).unwrap().len(), 1);
            let state = resume_git_state(&cwd);
            assert!(state.contains("Uncommitted changes: 1"), "{state}");
            assert!(
                state.contains("Unpushed commits (upstream, or remote refs when unset): 1"),
                "{state}"
            );
            assert!(
                state.contains("uncommitted.txt") && state.contains("unpublished"),
                "{state}"
            );
            assert_eq!(
                std::fs::read(cwd.join("uncommitted.txt")).unwrap(),
                b"keep every byte\0"
            );
            assert_eq!(
                run_git(&cwd, &["branch", "--show-current"]).unwrap(),
                "factory/held-task"
            );
            assert_eq!(
                crate::store::open_task_store(&root)
                    .unwrap()
                    .get("cas-held")
                    .unwrap()
                    .assignee
                    .as_deref(),
                Some("worker")
            );
            // Neither direct queueing nor stale queued metadata can target a supervisor.
            agent.role = cas_types::AgentRole::Supervisor;
            agents.update(&agent).unwrap();
            assert!(
                queue_limit_switch_with(
                    &root,
                    &agent.id,
                    "new-episode",
                    now,
                    |_| panic!(),
                    |_| panic!()
                )
                .unwrap_err()
                .to_string()
                .contains("supervisor")
            );
            let value: serde_json::Value =
                serde_json::from_str(rows[0].worker_spec.as_deref().unwrap()).unwrap();
            let meta: LimitRecycle =
                serde_json::from_value(value["limit_recycle"].clone()).unwrap();
            let spec = serde_json::from_value(value).unwrap();
            assert!(prepare_limit_resume(&root, "worker", &meta, &spec).is_err());
        });
    }

    #[test]
    fn paired_provider_switch_excludes_limited_identity_and_persists_recipe_or_refusal() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
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
            old.identity = Some(crate::account_registry::AccountIdentity {
                account_id: Some("limited-subscription".into()),
                ..Default::default()
            });
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
            let mut exhausted = accounts[..2].to_vec();
            for account in &mut exhausted {
                for window in &mut account.windows { window.used_percent = Some(100.); }
            }
            let (json, detail) = plan_switch(
                &agent,
                &meta,
                &exhausted,
                now,
                &Default::default(),
                |_| panic!("no eligible account must not preflight"),
            );
            assert!(json.is_none());
            assert!(
                detail.contains("no eligible account until 2026-10-27T15:00:00+00:00"),
                "{detail}"
            );
            // No registered row for the old account: still use its known alias.
            let mut unregistered = meta.clone();
            unregistered.old_account = PathBuf::from(format!("/private/account-home/{}legacy@example.invalid", provider.prefix()));
            let (_, detail) = plan_switch(&agent, &unregistered, std::slice::from_ref(&next), now, &Default::default(), |_| Ok(()));
            let mut notice_errors = Vec::new();
            if !detail.contains("legacy@example.invalid -> next") || detail.contains("/private/account-home") {
                notice_errors.push(detail);
            }

            // Eligibility needs every blocking window to reset. Ignore accounts
            // whose login/disabled/duplicate state makes their early reset unusable.
            let mut early_reset = next.clone();
            for window in &mut early_reset.windows {
                window.resets_at = Some(now + chrono::Duration::minutes(30));
            }
            let mut disabled = early_reset.clone();
            disabled.flags.disabled = true;
            let mut logged_out = early_reset.clone();
            logged_out.login_state = crate::account_registry::LoginState::LoggedOut;
            let mut stale = early_reset.clone();
            stale.flags.stale_login = true;
            let mut duplicate = early_reset.clone();
            duplicate.flags.duplicate_of = Some(PathBuf::from("/canonical"));
            let mut unknown = next.clone();
            unknown.windows.clear();
            for (five, weekly, hours, threshold) in [(20., 85., 8, 80), (95., 20., 3, 90), (95., 95., 8, 90)] {
                let blocked = crate::account_selection::tests::row(provider, "blocked", 8, weekly, five);
                // The transcript can be newer than the limited account's
                // under-threshold quota snapshot. It is excluded for switching,
                // and another account's early reset must not make it "recover".
                let rows = [blocked, disabled.clone(), logged_out.clone(), stale.clone(), duplicate.clone(), unknown.clone(), accounts[0].clone()];
                let policy = crate::config::AccountsConfig { threshold, ..Default::default() };
                let (_, detail) = plan_switch(&agent, &meta, &rows, now, &policy, |_| panic!("none eligible"));
                let reset = (now + chrono::Duration::hours(hours)).to_rfc3339();
                if !detail.contains(&format!("no eligible account until {reset}")) {
                    notice_errors.push(format!("expected {reset}: {detail}"));
                }
            }
            assert!(notice_errors.is_empty(), "{cli}: {notice_errors:#?}");
            // A new registration resuming the same refusal uses the same durable key.
            assert_eq!(episode_key("session", "worker", "first"), meta.episode);
        }
    }
}
