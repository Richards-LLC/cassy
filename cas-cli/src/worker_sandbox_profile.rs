//! The Claude worker OS sandbox block for this project (cas-3751a, capability
//! guards stage A). The pure profile lives in `cas_mux::worker_sandbox`; this
//! module supplies the project's resolved roots, the worker's current tasks
//! and protection lists.
//!
//! cas-f157: the profile grants only the worker's current tasks' artifact
//! directories and, at launch, its checkout's own scratch. A launched profile
//! cannot change, so a start of a task it does not grant relaunches the worker
//! through the ordinary recycle path, and the relaunch regenerates it.

use std::path::{Component, Path};

use cas_mux::worker_sandbox::{LaunchInputs, ProfileRoots, canonical_spelling, claude_sandbox_block};
use cas_types::factory_claude::WorkerSandbox;
use cas_types::{Agent, TaskStatus, TaskType};

/// Agent metadata naming the tasks the running worker's profile grants,
/// comma-separated. Absent when the worker runs unsandboxed or its launch
/// could not read its assignment.
pub(crate) const SANDBOX_TASKS_METADATA: &str = "sandbox_tasks";

/// The worker `sandbox` settings block and its launch inputs, or `None` when
/// this host or the project's `factory.claude.worker_sandbox` leaves workers
/// unsandboxed. Only macOS is measured (cas-ac77): sandbox-runtime does not
/// start on the Linux factory host yet.
pub(crate) fn worker_sandbox_settings(
    cas_root: &Path,
    session: Option<&str>,
    worker: &str,
) -> Option<(serde_json::Value, serde_json::Value)> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    profile_for(cas_root, session, worker)
}

fn profile_for(
    cas_root: &Path,
    session: Option<&str>,
    worker: &str,
) -> Option<(serde_json::Value, serde_json::Value)> {
    let config = crate::config::Config::load(cas_root).ok()?;
    let factory = config.factory();
    if factory.claude.worker_sandbox == WorkerSandbox::Off {
        return None;
    }
    let tasks = worker_tasks(cas_root, session, worker)
        .map_err(|error| {
            tracing::warn!(worker, %error, "worker sandbox: assignment unreadable; no task artifact dir is granted");
        })
        .ok();
    let artifacts =
        crate::config::resolved_factory_artifact_paths(cas_root, factory.artifacts_root.as_deref()).project_root;
    let mut roots: Vec<_> =
        tasks.iter().flatten().map(|task| canonical_spelling(&artifacts.join(task))).collect();
    for root in crate::config::operator_policy::effective_write_roots(
        &factory.write_roots,
        &crate::config::operator_policy::user_write_roots(),
        cas_root,
    ) {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let launch = LaunchInputs {
        tasks,
        tmp_root: Some(crate::factory_scratch::test_tmp_root(cas_root)),
        scratch_root: crate::factory_scratch::configured_scratch_root(cas_root),
    };
    Some((
        claude_sandbox_block(&ProfileRoots { write_roots: &roots, cas_root }),
        serde_json::to_value(launch).ok()?,
    ))
}

/// The worker's current tasks: running or held work assigned to it by name
/// or agent id, plus a queue successor it is being relaunched to start.
fn worker_tasks(cas_root: &Path, session: Option<&str>, worker: &str) -> anyhow::Result<Vec<String>> {
    let mut owners = vec![worker.to_string()];
    owners.extend(
        crate::store::open_agent_store(cas_root)?
            .list(None)?
            .into_iter()
            .filter(|agent| {
                agent.name == worker
                    && session.is_none_or(|session| agent.factory_session.as_deref() == Some(session))
            })
            .map(|agent| agent.id),
    );
    let store = crate::store::open_task_store(cas_root)?;
    let mut ids = Vec::new();
    for status in [TaskStatus::InProgress, TaskStatus::Blocked] {
        ids.extend(
            store
                .list(Some(status))?
                .into_iter()
                .filter(|task| {
                    !matches!(task.task_type, TaskType::Epic | TaskType::Gate)
                        && task.assignee.as_deref().is_some_and(|assignee| owners.iter().any(|owner| owner == assignee))
                })
                .map(|task| task.id),
        );
    }
    if let Some(session) = session {
        let queue = cas_store::worker_task_queue_store::SqliteWorkerTaskQueueStore::open(cas_root)?
            .get(session, worker)?;
        if queue.starting {
            ids.extend(queue.current_task);
        }
    }
    // A task id is one path component, so its grant cannot name a parent.
    ids.retain(|id| matches!(Path::new(id).components().collect::<Vec<_>>()[..], [Component::Normal(_)]));
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// Record, from the registering harness's environment, which tasks its
/// launched profile grants.
pub(crate) fn record_launched_tasks(agent: &mut Agent) {
    use cas_mux::worker_sandbox::{SANDBOX_ENV, SHADOW, TASKS_ENV};
    match std::env::var(TASKS_ENV) {
        Ok(tasks) if std::env::var(SANDBOX_ENV).is_ok_and(|mode| mode == SHADOW) => {
            agent.metadata.insert(SANDBOX_TASKS_METADATA.into(), tasks);
        }
        _ => {
            agent.metadata.remove(SANDBOX_TASKS_METADATA);
        }
    }
}

/// cas-f157: after `worker` starts `task_id`, relaunch it when its running
/// profile does not grant that task, so the profile always matches its
/// current assignment. Uses the daemon's recycle request, which keeps the
/// worktree and task bindings and sends the resume brief. Returns whether a
/// relaunch is pending.
pub(crate) fn relaunch_for_started_task(cas_root: &Path, worker: &Agent, task_id: &str) -> anyhow::Result<bool> {
    let Some(granted) = worker.metadata.get(SANDBOX_TASKS_METADATA) else {
        return Ok(false);
    };
    if granted.split(',').any(|id| id == task_id) {
        return Ok(false);
    }
    let Some(session) = worker.factory_session.as_deref() else {
        return Ok(false);
    };
    let spawns = crate::store::open_spawn_queue_store(cas_root)?;
    if spawns.has_pending_spawn(&worker.name, Some(session))? {
        return Ok(true);
    }
    let spec = crate::mcp::tools::service::factory_ops::worker_spec_from_agent(worker);
    cas_factory::validate_explicit(&spec, &cas_factory::CapabilitySnapshot::default())
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    spawns.enqueue_recycle(&worker.name, Some(&serde_json::to_string(&spec)?), Some(session))?;
    let _ = cas_factory::notify_daemon(cas_root);
    Ok(true)
}

#[cfg(test)]
#[path = "worker_sandbox_profile_tests.rs"]
mod tests;
