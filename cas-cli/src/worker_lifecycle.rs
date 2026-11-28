use std::path::Path;

use cas_types::{Agent, AgentRole, Task, TaskStatus, TaskType};

pub(crate) const INPUT_LIMIT: u64 = 200_000;
const TASK_BINDING: &str = "worker_task_id";
const CHECKPOINT_REQUEST: &str = "worker_checkpoint_requested_at";

pub(crate) fn ensure_assignment(
    tasks: &dyn cas_store::TaskStore,
    worker: &Agent,
    task_id: &str,
) -> Result<(), String> {
    if worker.role != AgentRole::Worker {
        return Ok(());
    }
    let previous = worker
        .metadata
        .get(TASK_BINDING)
        .filter(|id| id.as_str() != task_id)
        .cloned();
    let previous = match previous {
        Some(id) => Some(id),
        None => tasks
            .list(None)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|task| {
                task.id != task_id
                    && !matches!(task.task_type, TaskType::Epic | TaskType::Gate)
                    && task
                        .assignee
                        .as_deref()
                        .is_some_and(|a| a == worker.name || a == worker.id)
            })
            .map(|task| task.id),
    };
    if let Some(previous) = previous {
        return Err(format!(
            "Worker {} belongs to task {previous}; spawn a fresh worker for {task_id}",
            worker.name
        ));
    }
    if worker.metadata.contains_key("worker_retired_task") {
        return Err(format!(
            "Worker {} is retired; spawn a fresh worker for {task_id}",
            worker.name
        ));
    }
    Ok(())
}

pub(crate) fn bind_task(root: &Path, worker_id: &str, task_id: &str) -> crate::Result<()> {
    let agents = crate::store::open_agent_store(root)?;
    let mut worker = agents.get(worker_id)?;
    if worker.role == AgentRole::Worker {
        worker.metadata.insert(TASK_BINDING.into(), task_id.into());
        agents.update(&worker)?;
    }
    Ok(())
}

fn finished(task: &Task) -> bool {
    task.is_terminal()
        || task.status == TaskStatus::AwaitingMerge && !task.deliverables.merge_conflicted
        || task.pending_verification && task.deliverables.parked_branch.is_some()
}

pub(crate) fn retire_after_delivery(root: &Path, task_id: &str) -> crate::Result<()> {
    let tasks = crate::store::open_task_store(root)?;
    let task = tasks.get(task_id)?;
    if !finished(&task) {
        return Ok(());
    }
    let agents = crate::store::open_agent_store(root)?;
    for worker in agents.list(None)?.into_iter().filter(|agent| {
        agent.role == AgentRole::Worker
            && agent.is_alive()
            && task
                .assignee
                .as_deref()
                .is_some_and(|a| a == agent.name || a == agent.id)
    }) {
        retire(root, &worker, &task)?;
    }
    Ok(())
}

fn lifecycle_pending(root: &Path, worker: &Agent) -> crate::Result<bool> {
    let queue = crate::store::open_spawn_queue_store(root)?;
    Ok(
        queue.has_pending_spawn(&worker.name, worker.factory_session.as_deref())?
            || queue.peek(1000)?.iter().any(|request| {
                request.action == cas_store::SpawnAction::Shutdown
                    && request.factory_session.as_deref() == worker.factory_session.as_deref()
                    && (request.worker_names.contains(&worker.name)
                        || request.worker_names.is_empty())
            }),
    )
}

fn retire(root: &Path, worker: &Agent, task: &Task) -> crate::Result<()> {
    let Some(session) = worker.factory_session.as_deref() else {
        return Ok(());
    };
    let _lock = crate::worker_task_queue::worker_lock(root, worker, "lifecycle")
        .map_err(|e| crate::error::CasError::Other(e.to_string()))?;
    let agents = crate::store::open_agent_store(root)?;
    let mut current = agents.get(&worker.id)?;
    if !current.metadata.contains_key("worker_retired_task") {
        current.metadata.insert("worker_retired_task".into(), task.id.clone());
        agents.update(&current)?;
    }
    let queue = crate::store::open_spawn_queue_store(root)?;
    let pending = queue.peek(1000)?.iter().any(|request| {
        request.action == cas_store::SpawnAction::Shutdown
            && request.factory_session.as_deref() == Some(session)
            && (request.worker_names.contains(&worker.name) || request.worker_names.is_empty())
    });
    if !pending {
        queue.enqueue_shutdown(
            None,
            std::slice::from_ref(&worker.name),
            false,
            Some(session),
        )?;
        let _ = cas_factory::notify_daemon(root);
    }
    Ok(())
}

pub(crate) fn reconcile(root: &Path, session: &str) -> crate::Result<()> {
    let tasks = crate::store::open_task_store(root)?.list(None)?;
    let agents = crate::store::open_agent_store(root)?;
    for worker in agents.list(None)?.into_iter().filter(|agent| {
        agent.role == AgentRole::Worker
            && agent.factory_session.as_deref() == Some(session)
            && agent.is_alive()
    }) {
        let assigned: Vec<_> = tasks
            .iter()
            .filter(|task| {
                !matches!(task.task_type, TaskType::Epic | TaskType::Gate)
                    && (task
                        .assignee
                        .as_deref()
                        .is_some_and(|a| a == worker.name || a == worker.id)
                        || worker.metadata.get(TASK_BINDING) == Some(&task.id))
            })
            .collect();
        if let Some(task) = assigned.iter().find(|task| finished(task)) {
            retire(root, &worker, task)?;
            continue;
        }
        let path = crate::mcp::tools::service::factory_ops::worker_transcript_path_for_agent(
            root, &worker,
        );
        if let Some(task) = assigned
            .iter()
            .find(|task| matches!(task.status, TaskStatus::InProgress | TaskStatus::Blocked))
        {
            reconcile_context(root, &worker, task, path.as_deref())?;
        }
    }
    Ok(())
}

fn reconcile_context(
    root: &Path,
    worker: &Agent,
    task: &Task,
    transcript: Option<&Path>,
) -> crate::Result<()> {
    use crate::mcp::tools::service::factory_ops::{worker_cli_from_agent, worker_context_usage};
    let cli = worker_cli_from_agent(worker);
    if !matches!(
        cli,
        cas_mux::SupervisorCli::Claude | cas_mux::SupervisorCli::Codex
    ) {
        return Ok(());
    }
    let Some(usage) = transcript.and_then(|path| worker_context_usage(root, worker, path)) else {
        return Ok(());
    };
    if usage.input_tokens < INPUT_LIMIT {
        return Ok(());
    }
    let Some(session) = worker.factory_session.as_deref() else {
        return Ok(());
    };
    let _lock = crate::worker_task_queue::worker_lock(root, worker, "lifecycle")
        .map_err(|e| crate::error::CasError::Other(e.to_string()))?;
    if lifecycle_pending(root, worker)? {
        return Ok(());
    }
    let config = crate::config::Config::load(root)?;
    let artifacts =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let handoffs = crate::config::factory_task_artifact_dirs(root, &artifacts, &task.id)
        .map(|dir| dir.join("HANDOFF.md"));
    let agents = crate::store::open_agent_store(root)?;
    let mut current = agents.get(&worker.id)?;
    let requested = current
        .metadata
        .get(CHECKPOINT_REQUEST)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok());
    if let Some(requested) = requested {
        let checkpointed = handoffs.iter().any(|path| {
            std::fs::metadata(path)
                .ok()
                .filter(|metadata| metadata.len() > 0)
                .and_then(|metadata| metadata.modified().ok())
                .is_some_and(|modified| {
                    chrono::DateTime::<chrono::Utc>::from(modified) >= requested
                })
        });
        let clean = worker.metadata.get("clone_path").is_some_and(|path| {
            std::process::Command::new("git")
                .current_dir(path)
                .args(["status", "--porcelain"])
                .output()
                .is_ok_and(|output| output.status.success() && output.stdout.is_empty())
        });
        if checkpointed && clean {
            let spec = crate::mcp::tools::service::factory_ops::worker_spec_from_agent(worker);
            cas_factory::validate_explicit(&spec, &cas_factory::CapabilitySnapshot::default())
                .map_err(|error| crate::error::CasError::Other(error.to_string()))?;
            let recipe = serde_json::to_string(&spec)?;
            crate::store::open_spawn_queue_store(root)?.enqueue_recycle(
                &worker.name,
                Some(&recipe),
                Some(session),
            )?;
            let _ = cas_factory::notify_daemon(root);
        }
        return Ok(());
    }
    let prefix = cli.backend().capabilities().tool_prefix;
    let prompt = format!(
        "Cassy measured {} latest-call input tokens (worker limit {INPUT_LIMIT}). Checkpoint task {} now: commit your work, push unless local_merge, write the current state and next steps to {}, then send {prefix}coordination action=message target=supervisor kind=respawn task_id={} summary=Checkpoint message=<return-contract>. End the turn. Cassy will restart this worker on the same task after the checkpoint; your worktree stays intact.",
        usage.input_tokens,
        task.id,
        handoffs[0].display(),
        task.id,
    );
    crate::store::open_prompt_queue_store(root)?.enqueue_urgent(
        "director",
        &worker.name,
        &prompt,
        Some(session),
        Some("Worker context checkpoint"),
        Some(cas_store::NotificationPriority::High),
        true,
    )?;
    current
        .metadata
        .insert(CHECKPOINT_REQUEST.into(), chrono::Utc::now().to_rfc3339());
    agents.update(&current)?;
    let _ = cas_factory::notify_daemon(root);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        mcp::{CasCore, CasService},
        test_support::TestEnvGuard,
    };
    use rmcp::handler::server::wrapper::Parameters;

    fn fixture(env: &mut TestEnvGuard, cli: &str) -> (std::path::PathBuf, Agent, Task) {
        let project = env.home().join(cli);
        let root = project.join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        env.set("CAS_ROOT", &root);
        env.set("CAS_FACTORY_SESSION", "lifecycle-factory");
        env.set("CAS_FACTORY_MODE", "1");
        env.remove(crate::internal_llm::INTERNAL_LLM_ENV);
        std::fs::write(root.join("config.toml"), format!(
            "[project]\ncanonical_id='lifecycle-fixture'\n[factory]\nartifacts_root={:?}\n[orchestration.auto_prompt]\nenabled=true\non_task_assigned=true\n",
            project.join("artifacts").to_string_lossy(),
        )).unwrap();
        let tasks = crate::store::open_task_store(&root).unwrap();
        tasks.init().unwrap();
        let agents = crate::store::open_agent_store(&root).unwrap();
        agents.init().unwrap();
        let mut worker = Agent::new_with_role(
            format!("{cli}-session"),
            format!("{cli}-worker"),
            AgentRole::Worker,
        );
        worker.factory_session = Some("lifecycle-factory".into());
        worker.metadata.insert("worker_cli".into(), cli.into());
        worker
            .metadata
            .insert("clone_path".into(), project.to_string_lossy().into());
        worker.metadata.insert(
            "worker_model".into(),
            if cli == "codex" {
                "gpt-6.1-sol"
            } else {
                "claude-sonnet-5-5"
            }
            .into(),
        );
        agents.register(&worker).unwrap();
        let mut owner = Agent::new_with_role("owner".into(), "owner".into(), AgentRole::Supervisor);
        owner.factory_session = worker.factory_session.clone();
        agents.register(&owner).unwrap();
        let mut task = Task::new(format!("cas-{cli}"), "Work".into());
        task.assignee = Some(worker.name.clone());
        task.status = TaskStatus::InProgress;
        tasks.add(&task).unwrap();
        (root, worker, task)
    }

    #[tokio::test]
    async fn assignment_and_start_require_a_fresh_worker_cas_8d7a() {
        let mut env = TestEnvGuard::temp_home();
        for cli in ["claude", "codex"] {
            let (root, worker, first) = fixture(&mut env, cli);
            let tasks = crate::store::open_task_store(&root).unwrap();
            let agents = crate::store::open_agent_store(&root).unwrap();
            let core = CasCore::with_daemon(root.clone(), None, None);
            core.set_agent_id_for_testing("owner".into());
            let service = CasService::new(
                core,
                #[cfg(feature = "mcp-proxy")]
                None,
            );
            let second = Task::new(format!("cas-{cli}-second"), "Next".into());
            tasks.add(&second).unwrap();
            for state in [
                TaskStatus::InProgress,
                TaskStatus::AwaitingMerge,
                TaskStatus::Closed,
                TaskStatus::Cancelled,
            ] {
                let mut first = first.clone();
                first.status = state;
                tasks.update(&first).unwrap();
                let request = serde_json::from_value(serde_json::json!({
                    "action":"update", "id":second.id, "assignee":worker.name,
                }))
                .unwrap();
                assert!(service.task(Parameters(request)).await.is_err());
                assert!(tasks.get(&second.id).unwrap().assignee.is_none());
                let mut assigned = second.clone();
                assigned.assignee = Some(worker.name.clone());
                assert!(crate::task_assignment::enqueue(&root, &assigned).is_err());
                let worker_core = CasCore::with_daemon(root.clone(), None, None);
                worker_core.set_agent_id_for_testing(worker.id.clone());
                assert!(
                    worker_core
                        .cas_task_start(Parameters(crate::mcp::tools::IdRequest {
                            id: second.id.clone()
                        }))
                        .await
                        .is_err()
                );
                assert_eq!(tasks.get(&second.id).unwrap().status, TaskStatus::Open);
            }
            let mut fresh = worker.clone();
            fresh.id = format!("{cli}-fresh-session");
            fresh.name = format!("{cli}-fresh-worker");
            agents.register(&fresh).unwrap();
            let request = serde_json::from_value(serde_json::json!({
                "action":"update", "id":second.id, "assignee":fresh.name,
            }))
            .unwrap();
            service.task(Parameters(request)).await.unwrap();
            assert_eq!(
                tasks.get(&second.id).unwrap().assignee.as_deref(),
                Some(fresh.name.as_str())
            );
            for request in [
                serde_json::json!({"action":"update", "assignee":fresh.name, "task_ids":[first.id, second.id]}),
                serde_json::json!({"action":"spawn_workers", "task_ids":[first.id, second.id]}),
            ] {
                if request["action"] == "update" {
                    assert!(
                        service
                            .task(Parameters(serde_json::from_value(request).unwrap()))
                            .await
                            .is_err()
                    );
                } else {
                    assert!(
                        service
                            .factory(Parameters(serde_json::from_value(request).unwrap()))
                            .await
                            .is_err()
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn delivery_and_cancel_retire_workers_without_starting_legacy_tails_cas_8d7a() {
        let mut env = TestEnvGuard::temp_home();
        for cli in ["claude", "codex"] {
            let (root, worker, mut task) = fixture(&mut env, cli);
            let tasks = crate::store::open_task_store(&root).unwrap();
            let core = CasCore::with_daemon(root.clone(), None, None);
            core.set_agent_id_for_testing("owner".into());
            let pending = Task::new(format!("cas-{cli}-tail"), "Legacy tail".into());
            tasks.add(&pending).unwrap();
            let queues =
                cas_store::worker_task_queue_store::SqliteWorkerTaskQueueStore::open(&root)
                    .unwrap();
            let mut queue = queues.get("lifecycle-factory", &worker.name).unwrap();
            queue.current_task = Some(task.id.clone());
            queue.current_started = true;
            queue.pending.push(pending.id.clone());
            queues
                .save("lifecycle-factory", &worker.name, &mut queue)
                .unwrap();
            for status in [
                TaskStatus::InProgress,
                TaskStatus::AwaitingMerge,
                TaskStatus::Closed,
            ] {
                task.status = status;
                tasks.update(&task).unwrap();
                core.advance_queue_after_delivery(&task.id).await.unwrap();
                let requests = crate::store::open_spawn_queue_store(&root)
                    .unwrap()
                    .peek(100)
                    .unwrap();
                assert_eq!(
                    requests.len(),
                    usize::from(status != TaskStatus::InProgress)
                );
                if let Some(request) = requests.first() {
                    assert_eq!(request.action, cas_store::SpawnAction::Shutdown);
                    assert_eq!(request.worker_names, vec![worker.name.clone()]);
                    assert!(!request.force);
                }
                assert_eq!(tasks.get(&pending.id).unwrap().status, TaskStatus::Open);
            }
            let spawns = crate::store::open_spawn_queue_store(&root).unwrap();
            for row in spawns.peek(100).unwrap() {
                spawns.mark_processed(row.id).unwrap();
            }
            task.status = TaskStatus::InProgress;
            tasks.update(&task).unwrap();
            let service = CasService::new(
                core,
                #[cfg(feature = "mcp-proxy")]
                None,
            );
            service
                .task(Parameters(
                    serde_json::from_value(serde_json::json!({
                        "action":"cancel", "id":task.id, "reason":"Disposable task cancelled",
                    }))
                    .unwrap(),
                ))
                .await
                .unwrap();
            assert_eq!(tasks.get(&task.id).unwrap().status, TaskStatus::Cancelled);
            assert_eq!(
                spawns.peek(100).unwrap()[0].action,
                cas_store::SpawnAction::Shutdown
            );
            reconcile(&root, "lifecycle-factory").unwrap();
            assert_eq!(spawns.peek(100).unwrap().len(), 1);
        }
    }

    #[test]
    fn latest_call_requests_checkpoint_then_recycles_same_task_cas_8d7a() {
        let mut env = TestEnvGuard::temp_home();
        for cli in ["claude", "codex"] {
            let (root, worker, task) = fixture(&mut env, cli);
            let project = root.parent().unwrap();
            for args in [
                vec!["init", "-q"],
                vec![
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.test",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "seed",
                ],
            ] {
                assert!(
                    std::process::Command::new("git")
                        .current_dir(project)
                        .args(args)
                        .status()
                        .unwrap()
                        .success()
                );
            }
            std::fs::write(project.join(".git/info/exclude"), ".cas/\nartifacts/\n").unwrap();
            let transcript = root.join("usage.jsonl");
            let prompts = crate::store::open_prompt_queue_store(&root).unwrap();
            let spawns = crate::store::open_spawn_queue_store(&root).unwrap();
            for input in [199_999, 200_000, 200_000] {
                let row = if cli == "codex" {
                    serde_json::json!({"type":"event_msg", "payload":{"type":"token_count", "info":{
                    "total_token_usage":{"input_tokens":9000000}, "last_token_usage":{"input_tokens":input}}}})
                } else {
                    serde_json::json!({"type":"assistant", "message":{"usage":{
                    "input_tokens":100, "cache_creation_input_tokens":100, "cache_read_input_tokens":input - 200}}})
                };
                std::fs::write(&transcript, format!("{row}\n")).unwrap();
                let current = crate::store::open_agent_store(&root)
                    .unwrap()
                    .get(&worker.id)
                    .unwrap();
                reconcile_context(&root, &current, &task, Some(&transcript)).unwrap();
                let rows = prompts
                    .peek_for_targets(&[&worker.name], Some("lifecycle-factory"), 100)
                    .unwrap();
                assert_eq!(rows.len(), usize::from(input >= 200_000));
                if let Some(row) = rows.first() {
                    assert!(row.urgent);
                }
                assert!(spawns.peek(100).unwrap().is_empty());
            }
            let config = crate::config::Config::load(&root).unwrap();
            let artifacts = crate::config::resolved_factory_artifacts_root(
                config.factory().artifacts_root.as_deref(),
            );
            let handoff = crate::config::factory_task_artifact_dirs(&root, &artifacts, &task.id)[0]
                .join("HANDOFF.md");
            std::fs::create_dir_all(handoff.parent().unwrap()).unwrap();
            std::fs::write(&handoff, "Checkpoint saved; resume the same task.").unwrap();
            std::fs::write(project.join("unfinished.rs"), "work").unwrap();
            reconcile_context(&root, &worker, &task, Some(&transcript)).unwrap();
            assert!(spawns.peek(100).unwrap().is_empty());
            std::fs::remove_file(project.join("unfinished.rs")).unwrap();
            reconcile_context(&root, &worker, &task, Some(&transcript)).unwrap();
            reconcile_context(&root, &worker, &task, Some(&transcript)).unwrap();
            let requests = spawns.peek(100).unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].action, cas_store::SpawnAction::Recycle);
            assert_eq!(requests[0].worker_names, vec![worker.name.clone()]);
            assert_eq!(
                crate::store::open_task_store(&root)
                    .unwrap()
                    .get(&task.id)
                    .unwrap()
                    .assignee,
                Some(worker.name.clone())
            );
            let recipe: cas_mux::WorkerSpec =
                serde_json::from_str(requests[0].worker_spec.as_deref().unwrap()).unwrap();
            assert_eq!(
                recipe.cli,
                if cli == "codex" {
                    cas_mux::SupervisorCli::Codex
                } else {
                    cas_mux::SupervisorCli::Claude
                }
            );
            assert!(handoff.is_file());
        }
    }
}
