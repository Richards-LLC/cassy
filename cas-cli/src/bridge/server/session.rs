use anyhow::{Result, bail};
use cas_factory::DirectorData;
use cas_store::{EventStore, SqliteEventStore};
use cas_types::Event;

use crate::store::{find_cas_root_from, open_prompt_queue_store};

use crate::bridge::server::types::{
    AgentLatestActivityJson, AgentSummaryJson, StatusJson, TaskSummaryJson, session_json,
};

pub(crate) fn resolve_session_by_name(
    session_name: &str,
) -> Result<crate::ui::factory::SessionInfo> {
    let manager = crate::ui::factory::SessionManager::new();
    manager
        .find_session(Some(session_name))?
        .ok_or_else(|| anyhow::anyhow!("Session '{session_name}' not found"))
}

pub(crate) fn cas_root_for_session_with_fallback(
    session: &crate::ui::factory::SessionInfo,
    fallback: Option<&std::path::Path>,
) -> Result<std::path::PathBuf> {
    if let Some(project_dir) = session.metadata.project_dir.as_ref() {
        let project = std::path::PathBuf::from(project_dir);
        match find_cas_root_from(&project) {
            Ok(root) => return Ok(root),
            Err(_) if fallback.is_some() => {
                // Fall through to fallback root.
            }
            Err(e) => return Err(e.into()),
        }
    }

    let Some(fallback) = fallback else {
        bail!(
            "Session '{}' has no project_dir in metadata; and no --cas-root fallback was provided",
            session.name
        );
    };

    Ok(fallback.to_path_buf())
}

pub(crate) fn filter_events_for_session_agents(
    events: &mut Vec<Event>,
    allowed: &std::collections::HashSet<String>,
) {
    events.retain(|e| {
        e.session_id
            .as_ref()
            .map(|sid| allowed.iter().any(|n| sid.contains(n)))
            .unwrap_or(false)
            || allowed.iter().any(|n| e.entity_id.contains(n))
    });
}

pub(crate) fn allowed_agent_names(
    session: &crate::ui::factory::SessionInfo,
) -> std::collections::HashSet<String> {
    let mut s = std::collections::HashSet::new();
    s.insert(session.metadata.supervisor.name.clone());
    for w in &session.metadata.workers {
        s.insert(w.name.clone());
    }
    s
}

/// One task of a session's status. An awaiting-merge task also carries its
/// delivery tip, read from the task store (cas-566b).
pub(crate) fn task_summary_json(
    t: cas_factory::TaskSummary,
    task_store: Option<&dyn cas_store::TaskStore>,
) -> TaskSummaryJson {
    let tip = (t.status == cas_types::TaskStatus::AwaitingMerge)
        .then(|| task_store?.get(&t.id).ok()?.deliverables.factory_branch_anchor)
        .flatten();
    TaskSummaryJson {
        id: t.id,
        title: t.title,
        status: format!("{:?}", t.status).to_lowercase(),
        priority: t.priority.0,
        assignee: t.assignee,
        task_type: format!("{:?}", t.task_type).to_lowercase(),
        epic: t.epic,
        branch: t.branch,
        updated_at: t.updated_at.map(|at| at.to_rfc3339()),
        tip,
    }
}

/// The task store a status read uses: one per project for the process.
///
/// cas-cb94: `open_task_store` opens one or two raw sync-queue connections
/// (store/detect.rs, cloud/sync_queue/mod.rs `Connection::open`) on every
/// call. Commander reloads status on every machine event without dedup, so a
/// burst held one connection set per in-flight request, and SQLite can keep a
/// closed connection's fd parked while the hub's pinned connection holds its
/// lock: prowl's hub held 8 descriptors on the busiest project's cas.db. A
/// status read only reads, so it needs neither a fresh stack nor the per-open
/// reconcile.
fn status_task_store(cas_root: &std::path::Path) -> Option<std::sync::Arc<dyn cas_store::TaskStore>> {
    crate::store::open_task_store_cached(cas_root).ok()
}

pub(crate) fn build_status_json(
    session: &crate::ui::factory::SessionInfo,
    cas_root: &std::path::Path,
    activity_limit: usize,
) -> Result<StatusJson> {
    let allowed = allowed_agent_names(session);
    let data = DirectorData::load_for_project(
        cas_root,
        None,
        false,
        crate::cloud::resolve_canonical_id(cas_root).as_deref(),
    )?;

    let mut activity = data.activity;
    filter_events_for_session_agents(&mut activity, &allowed);
    let activity_limit = activity_limit.clamp(1, 200);
    if activity.len() > activity_limit {
        activity.truncate(activity_limit);
    }

    let agents: Vec<AgentSummaryJson> = data
        .agents
        .into_iter()
        .filter(|a| allowed.contains(&a.name))
        .map(|a| AgentSummaryJson {
            generation: crate::ops::fleet::worker_generation(cas_root, &session.name, &a.name)
                .ok()
                .flatten(),
            id: a.id,
            name: a.name,
            status: format!("{:?}", a.status).to_lowercase(),
            current_task: a.current_task,
            latest_activity: a.latest_activity.map(|(s, ts)| AgentLatestActivityJson {
                summary: s,
                created_at_rfc3339: ts.to_rfc3339(),
            }),
            last_heartbeat_rfc3339: a.last_heartbeat.map(|ts| ts.to_rfc3339()),
        })
        .collect();

    let task_store = status_task_store(cas_root);
    let to_task = |t: cas_factory::TaskSummary| task_summary_json(t, task_store.as_deref());

    let queue = open_prompt_queue_store(cas_root)?;
    let pending = queue.pending_count()?;

    Ok(StatusJson {
        schema_version: 1,
        session: session_json(session),
        prompt_queue_pending: pending,
        activity,
        agents,
        tasks_ready: data.ready_tasks.into_iter().map(to_task).collect(),
        tasks_in_progress: data.in_progress_tasks.into_iter().map(to_task).collect(),
        epics: data.epic_tasks.into_iter().map(to_task).collect(),
        focused_epic: crate::ops::fleet::pinned_epic(&session.name),
    })
}

pub(crate) fn wait_for_supervisor_ack(
    cas_root: &std::path::Path,
    message_id: i64,
    timeout_ms: u64,
) -> Result<Option<i64>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let store = SqliteEventStore::open(cas_root)?;
    while std::time::Instant::now() < deadline {
        let recent = store.list_by_type(cas_types::EventType::SupervisorInjected, 25)?;
        let found = recent.into_iter().find(|e| {
            e.metadata
                .as_ref()
                .and_then(|m| m.get("prompt_id"))
                .and_then(|v| v.as_i64())
                == Some(message_id)
                && e.metadata
                    .as_ref()
                    .and_then(|m| m.get("status"))
                    .and_then(|v| v.as_str())
                    .is_some_and(
                            crate::ui::factory::daemon::runtime::queue_and_events::injection_status_reached_transport,
                        )
                && e.entity_type == cas_types::EventEntityType::Agent
        });
        if let Some(ev) = found {
            return Ok(Some(ev.id));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    Ok(None)
}

#[cfg(test)]
mod focused_epic_tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, crate::ui::factory::SessionInfo) {
        let project = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(project.path()).unwrap();
        let metadata = crate::ui::factory::create_metadata(
            "selected-session", std::process::id(), "supervisor", &["worker-a".into()],
            None, Some(project.path().to_str().unwrap()), None,
        );
        let session = crate::ui::factory::SessionInfo {
            name: "selected-session".into(), metadata, is_running: true, socket_exists: false,
        };
        (project, root, session)
    }

    #[test]
    fn status_projects_selected_session_worker_from_worker_status_evidence() {
        use cas_store::AgentStore;
        use cas_types::{Agent, AgentRole, Task, TaskStatus};
        let _home = crate::test_env_guard::TestEnvGuard::temp_home();
        let (_project, root, session) = fixture();
        let mut worker = Agent::new_with_role("worker-id".into(), "worker-a".into(), AgentRole::Worker);
        worker.factory_session = Some(session.name.clone());
        // A real exited PID yields the same deterministic verdict in the hub
        // and worker_status. A fresh registry heartbeat cannot hide death.
        worker.pid = Some(i32::MAX as u32);
        crate::store::open_agent_store(&root).unwrap().register(&worker).unwrap();
        let store = crate::store::open_task_store(&root).unwrap();
        let mut task = Task::new("cas-current".into(), "Current worker task".into());
        task.status = TaskStatus::InProgress;
        task.assignee = Some(worker.id.clone());
        store.add(&task).unwrap();
        let status = serde_json::to_value(build_status_json(&session, &root, 20).unwrap()).unwrap();
        let projected = status["agents"].as_array().unwrap().iter()
            .find(|agent| agent["name"] == "worker-a").expect("selected worker must be reported");
        assert_eq!(projected["current_task"], task.id);
        let expected = crate::mcp::tools::service::factory_ops::worker_liveness_for_agent(
            &root, &worker, chrono::Utc::now(), 300,
        );
        assert_eq!(projected["liveness"], expected.state.as_str());
        assert_eq!(projected["liveness"], "dead");
    }

    #[test]
    fn status_delivery_and_epic_progress_use_epic_status_containment() {
        use cas_types::{Task, TaskStatus, TaskType, Dependency, DependencyType};
        let _home = crate::test_env_guard::TestEnvGuard::temp_home();
        let (project, root, session) = fixture();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(project.path()).output().unwrap();
            assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "fixture@example.test"]);
        git(&["config", "user.name", "Fixture"]);
        std::fs::write(project.path().join("file"), "delivered\n").unwrap();
        git(&["add", "file"]); git(&["commit", "-m", "delivery"]);
        let anchor = git(&["rev-parse", "HEAD"]);
        let store = crate::store::open_task_store(&root).unwrap();
        let mut epic = Task::new("cas-epic".into(), "Epic".into());
        epic.task_type = TaskType::Epic; epic.branch = Some("main".into());
        store.add(&epic).unwrap();
        for (id, status, delivered) in [("cas-landed", TaskStatus::AwaitingMerge, true),
            ("cas-open", TaskStatus::Open, false), ("cas-cancel", TaskStatus::Cancelled, false)] {
            let mut child = Task::new(id.into(), id.into()); child.status = status;
            if delivered {
                child.deliverables.factory_branch_anchor = Some(anchor.clone());
                child.deliverables.merge_commit = Some(anchor.clone());
            }
            store.add(&child).unwrap();
            store.add_dependency(&Dependency { from_id: id.into(), to_id: epic.id.clone(),
                dep_type: DependencyType::ParentChild, created_at: chrono::Utc::now(), created_by: None }).unwrap();
        }
        let status = serde_json::to_value(build_status_json(&session, &root, 20).unwrap()).unwrap();
        let landed = status["tasks_in_progress"].as_array().unwrap().iter().find(|task| task["id"] == "cas-landed").unwrap();
        assert_eq!(landed["epic_id"], epic.id);
        assert_eq!(landed["delivery_state"], "merged");
        let progress = &status["epics"][0]["progress"];
        assert_eq!(progress["total"], 2);
        assert_eq!(progress["merged"], 1);
        assert_eq!(progress["merges_per_hour"], 1);
        assert!(chrono::DateTime::parse_from_rfc3339(progress["last_merged_at"].as_str().unwrap()).is_ok());
        assert!(status["tasks_ready"].as_array().unwrap().iter()
            .all(|task| task.get("delivery_state").is_none()));
    }

    /// cas-9b08: status names the session's pinned epic (null when
    /// unpinned), so Commander can state it as focus_epic's precondition.
    #[test]
    fn status_reports_the_pinned_epic_or_null_cas_9b08() {
        let _home = crate::test_env_guard::TestEnvGuard::temp_home();
        let parent = std::env::temp_dir().canonicalize().unwrap();
        let project = tempfile::tempdir_in(parent).unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let name = "factory-focus-9b08";
        let metadata = crate::ui::factory::create_metadata(
            name,
            std::process::id(),
            "supervisor",
            &[],
            None,
            Some(project.path().to_str().unwrap()),
            None,
        );
        let path = crate::ui::factory::metadata_path(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string(&metadata).unwrap()).unwrap();
        let session = crate::ui::factory::SessionInfo {
            name: name.to_string(),
            metadata,
            is_running: true,
            socket_exists: false,
        };

        let status_json = || {
            serde_json::to_value(build_status_json(&session, &cas_root, 20).unwrap()).unwrap()
        };
        let status = status_json();
        assert!(
            status.get("focused_epic").is_some_and(serde_json::Value::is_null),
            "{status}"
        );

        crate::ui::factory::persist_session_metadata_pinned_epic_id_at(
            &path,
            Some("cas-epic-9b08"),
        )
        .unwrap();
        assert_eq!(status_json()["focused_epic"], "cas-epic-9b08");

        // cas-cb94: every status read shares the project's one task store.
        let first = status_task_store(&cas_root).unwrap();
        assert!(std::sync::Arc::ptr_eq(&first, &status_task_store(&cas_root).unwrap()));
    }
}
