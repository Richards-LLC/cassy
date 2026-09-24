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

    let to_task = |t: cas_factory::TaskSummary| TaskSummaryJson {
        id: t.id,
        title: t.title,
        status: format!("{:?}", t.status).to_lowercase(),
        priority: t.priority.0,
        assignee: t.assignee,
        task_type: format!("{:?}", t.task_type).to_lowercase(),
        epic: t.epic,
        branch: t.branch,
    };

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
                    == Some("ok")
                && e.entity_type == cas_types::EventEntityType::Agent
        });
        if let Some(ev) = found {
            return Ok(Some(ev.id));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    Ok(None)
}
