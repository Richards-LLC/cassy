use anyhow::{Result, bail};
use cas_factory::DirectorData;
use cas_store::{EventStore, SqliteEventStore};
use cas_types::Event;

use crate::store::{find_cas_root_ignoring_env, open_prompt_queue_store};

use crate::bridge::server::types::{
    AgentLatestActivityJson, AgentSummaryJson, AgentTrailJson, EpicProgressJson, StatusJson,
    TaskSummaryJson, session_json,
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
        // A hub serves several projects; its launch environment does not own
        // the selected session's agents, lifecycle or operator queue.
        match find_cas_root_ignoring_env(&project) {
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
        .then(|| {
            task_store?
                .get(&t.id)
                .ok()?
                .deliverables
                .factory_branch_anchor
        })
        .flatten();
    TaskSummaryJson {
        id: t.id,
        title: t.title,
        status: format!("{:?}", t.status).to_lowercase(),
        priority: t.priority.0,
        assignee: t.assignee,
        epic_verification_owner: t.epic_verification_owner,
        task_type: format!("{:?}", t.task_type).to_lowercase(),
        epic_id: t.epic.clone(),
        epic: t.epic,
        delivery_state: None,
        merge_check_pending: t.status == cas_types::TaskStatus::AwaitingMerge,
        progress: None,
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
fn status_task_store(
    cas_root: &std::path::Path,
) -> Option<std::sync::Arc<dyn cas_store::TaskStore>> {
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

    let task_store =
        status_task_store(cas_root).ok_or_else(|| anyhow::anyhow!("task store unavailable"))?;
    // Staffing and task rows must describe the same decoded snapshot. A
    // second status-filtered read can omit a supported legacy status spelling
    // or observe a reassignment after the rows above were loaded.
    let held: Vec<_> = data.in_progress_tasks.iter()
        .chain(&data.ready_tasks)
        .chain(&data.epic_tasks)
        .filter(|task| matches!(task.status,
            cas_types::TaskStatus::InProgress | cas_types::TaskStatus::Open
            | cas_types::TaskStatus::Blocked | cas_types::TaskStatus::AwaitingMerge))
        .collect();
    let agents = session_agents(session, cas_root, task_store.as_ref(), &held)?;

    let to_task = |t: cas_factory::TaskSummary| task_summary_json(t, Some(task_store.as_ref()));

    let queue = open_prompt_queue_store(cas_root)?;
    let pending = queue.pending_count()?;

    let mut status = StatusJson {
        schema_version: 1,
        supervisor_phase: crate::ui::factory::daemon::runtime::work_events::supervisor_phase_for_session(
            cas_root,
            std::path::Path::new(session.metadata.project_dir.as_deref().unwrap_or(".")),
            &session.name,
            &session.metadata.supervisor.name,
        ),
        session: session_json(session),
        prompt_queue_pending: pending,
        activity,
        agents,
        tasks_ready: data.ready_tasks.into_iter().map(to_task).collect(),
        tasks_in_progress: data.in_progress_tasks.into_iter().map(to_task).collect(),
        epics: data.epic_tasks.into_iter().map(to_task).collect(),
        focused_epic: crate::ops::fleet::pinned_epic(&session.name),
    };
    project_deliveries(&mut status, cas_root, Some(task_store.as_ref()));
    Ok(status)
}

/// Read the selected session directly, independent of the hub process's own
/// CAS_FACTORY_SESSION. Reuse worker_status's authoritative registrations,
/// task-store identity matching, and bounded harness liveness classifier.
fn session_agents(
    session: &crate::ui::factory::SessionInfo,
    root: &std::path::Path,
    tasks: &dyn cas_store::TaskStore,
    held: &[&cas_factory::TaskSummary],
) -> Result<Vec<AgentSummaryJson>> {
    use crate::mcp::tools::service::factory_ops::{
        dedupe_authoritative_agents, worker_liveness_for_agent,
    };
    use cas_types::{AgentRole, TaskStatus};
    let registrations: Vec<_> = crate::store::open_agent_store(root)?
        .list(None)?
        .into_iter()
        .filter(|agent| {
            matches!(
                agent.role,
                AgentRole::Worker | AgentRole::Supervisor | AgentRole::Director
            ) && agent.visible_to_factory_session(Some(&session.name))
        })
        .collect();
    let (agents, _) = dedupe_authoritative_agents(registrations.clone());
    let events = SqliteEventStore::open(root)?;
    let stall_secs = crate::config::Config::load(root)
        .unwrap_or_default()
        .factory()
        .stall_threshold_secs as i64;
    let now = chrono::Utc::now();
    agents
        .into_iter()
        .map(|agent| {
            let latest = events.latest_worker_activity(&agent.id, &agent.name)?;
            // Task ownership survives a harness recycle. Keep retired IDs
            // only for this logical worker in the selected session; a same-
            // name registration in another session is not an alias.
            let aliases: std::collections::HashSet<_> = registrations
                .iter()
                .filter(|registration| {
                    registration.name == agent.name && registration.role == agent.role
                })
                .map(|registration| registration.id.as_str())
                .chain(std::iter::once(agent.name.as_str()))
                .collect();
            let task = held
                .iter()
                .filter(|task| {
                    task.assignee.as_deref().is_some_and(|owner| aliases.contains(owner))
                })
                .map(|task| {
                    // A batch note or merge receipt changes updated_at without
                    // assigning new work. Use the same start evidence as the
                    // visible trail; creation is a stable legacy fallback.
                    let work_at = task_start_event(&events, &task.id)?
                        .map(|event| event.created_at)
                        .or_else(|| tasks.get(&task.id).ok().map(|record| record.created_at));
                    Ok((*task, work_at))
                })
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .min_by_key(|(task, work_at)| {
                    (
                        match task.status {
                            TaskStatus::InProgress => 0,
                            TaskStatus::Open => 1,
                            TaskStatus::Blocked => 2,
                            _ => 3,
                        },
                        std::cmp::Reverse(*work_at),
                        task.id.clone(),
                    )
                })
                .map(|(task, _)| task);
            // Full records supply lifecycle receipts only. Missing details
            // cannot erase the assignment already present in this response.
            let full_task = task.and_then(|task| tasks.get(&task.id).ok());
            let (task_started_at, recent_trail) = worker_task_facts(root, &events, full_task.as_ref())?;
            Ok(AgentSummaryJson {
                role: agent.role.to_string(),
                harness: agent.metadata.get("worker_cli").cloned(),
                model: agent.metadata.get("worker_model").cloned(),
                task_started_at,
                recent_trail,
                generation: crate::ops::fleet::worker_generation(root, &session.name, &agent.name)
                    .ok()
                    .flatten(),
                liveness: (agent.role == AgentRole::Worker).then(|| {
                    worker_liveness_for_agent(root, &agent, now, stall_secs)
                        .state
                        .as_str()
                        .to_string()
                }),
                current_task: task.map(|task| task.id.clone()),
                latest_activity: latest.map(|event| AgentLatestActivityJson {
                    summary: event.summary,
                    created_at_rfc3339: event.created_at.to_rfc3339(),
                }),
                last_heartbeat_rfc3339: Some(agent.last_heartbeat.to_rfc3339()),
                id: agent.id,
                name: agent.name,
                status: agent.status.to_string(),
            })
        })
        .collect()
}

/// Shared, bounded task-start evidence for current-work ordering and its trail.
fn task_start_event(events: &dyn EventStore, id: &str) -> Result<Option<Event>> {
    Ok(events.list_for_entity(cas_types::EventEntityType::Task, id, 200)?
        .into_iter()
        .find(|event| event.event_type == cas_types::EventType::TaskStarted))
}

/// Only durable lifecycle records become Team marks. Bounded event history can
/// omit a start; registration/heartbeat and arbitrary notes never stand in for it.
fn worker_task_facts(
    root: &std::path::Path,
    events: &dyn EventStore,
    task: Option<&cas_types::Task>,
) -> Result<(Option<String>, Vec<AgentTrailJson>)> {
    use cas_types::{MergeCiAdmission, VerificationStatus};
    let Some(task) = task else {
        return Ok((None, Vec::new()));
    };
    let started = task_start_event(events, &task.id)?;
    let start_at = started.as_ref().map(|event| event.created_at.to_rfc3339());
    let mut trail = Vec::new();
    if let Some(event) = started.as_ref() {
        trail.push(AgentTrailJson {
            step: "started",
            at: start_at.clone(),
            reference: format!("event:{}", event.id),
        });
    }
    let delivery = &task.deliverables;
    if let Some(branch) = delivery.parked_branch.as_ref() {
        trail.push(AgentTrailJson {
            step: "pushed",
            at: None,
            reference: branch.clone(),
        });
    }
    if delivery.merge_ci_admission == Some(MergeCiAdmission::Green)
        && let Some(sha) = delivery.green_merge_commit.as_ref()
    {
        trail.push(AgentTrailJson {
            step: "ci_green",
            at: None,
            reference: sha.clone(),
        });
    }
    if let Some(sha) = delivery
        .merge_commit
        .as_ref()
        .or(delivery.delivery_pr_merge_commit.as_ref())
    {
        trail.push(AgentTrailJson {
            step: "merged",
            at: delivery.delivery_pr_merged_at.map(|at| at.to_rfc3339()),
            reference: sha.clone(),
        });
    }
    // A verdict from an earlier task cycle cannot verify the current work.
    if let Some(started) = started
        && let Ok(store) = crate::store::open_verification_store(root)
        && let Ok(Some(verdict)) = store.get_latest_for_task(&task.id)
        && verdict.status == VerificationStatus::Approved
        && verdict.created_at >= started.created_at
    {
        trail.push(AgentTrailJson {
            step: "verified",
            at: Some(verdict.created_at.to_rfc3339()),
            reference: verdict.id,
        });
    }
    Ok((start_at, trail))
}

/// A live lane can still be at its spawn base before any work is delivered.
/// Its zero-ahead count is not a merge receipt for the new assignment.
fn has_recorded_delivery(task: &cas_types::Task) -> bool {
    let delivery = &task.deliverables;
    delivery.factory_branch_anchor.is_some()
        || delivery.parked_branch.is_some()
        || delivery.merge_commit.is_some()
        || delivery.delivery_pr_merge_commit.is_some()
        || delivery.integration_batch.is_some()
}

/// Optional evidence never turns a timed-out/unreadable proof into zero work.
/// Prioritize the focused/session-active epic and bound the overall read.
/// Team's ancestry-based display does not run or cache mutation-gate proofs.
fn project_deliveries(
    status: &mut StatusJson,
    root: &std::path::Path,
    store: Option<&dyn cas_store::TaskStore>,
) {
    use crate::mcp::tools::core::task::lifecycle::close_ops::{
        EpicStatusOptions, collect_epic_ancestry_merges,
        collect_epic_branch_statuses_with_options, git_commit_is_ancestor, last_commit_unix,
    };
    let Some(store) = store else { return };
    let started = std::time::Instant::now();
    let now = chrono::Utc::now();
    let mut merged_ids = std::collections::HashSet::new();
    let mut checked_ids = std::collections::HashSet::new();
    // Team selects among the selected supervisor's live epics. Prefer the one
    // staffed in this conversation before spending the bounded read on the
    // project-wide historical catalog. Registrations and tasks may use either
    // the supervisor's friendly name or its session ID.
    let supervisors: std::collections::HashSet<_> =
        std::iter::once(status.session.supervisor.as_str())
            .chain(
                status
                    .agents
                    .iter()
                    .filter(|agent| agent.name == status.session.supervisor)
                    .map(|agent| agent.id.as_str()),
            )
            .collect();
    let staffed_tasks: std::collections::HashSet<_> = status
        .agents
        .iter()
        .filter_map(|agent| agent.current_task.as_deref())
        .collect();
    let active_epic = status
        .epics
        .iter()
        .filter(|epic| {
            matches!(
                epic.status.as_str(),
                "open" | "inprogress" | "blocked" | "awaitingmerge"
            )
        })
        .filter(|epic| {
            [
                epic.assignee.as_deref(),
                epic.epic_verification_owner.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|owner| supervisors.contains(owner))
        })
        .min_by_key(|epic| {
            std::cmp::Reverse(
                status
                    .tasks_in_progress
                    .iter()
                    .filter(|task| {
                        task.status == "inprogress"
                            && task.epic_id.as_deref() == Some(epic.id.as_str())
                            && staffed_tasks.contains(task.id.as_str())
                    })
                    .count(),
            )
        })
        .map(|epic| epic.id.as_str());
    let mut order: Vec<_> = (0..status.epics.len()).collect();
    order.sort_by_key(|&index| {
        let epic = Some(status.epics[index].id.as_str());
        (status.focused_epic.as_deref() != epic, active_epic != epic)
    });
    for index in order {
        if started.elapsed() >= std::time::Duration::from_secs(2) {
            break;
        }
        let epic = &mut status.epics[index];
        let Ok(record) = store.get(&epic.id) else {
            continue;
        };
        let context = match record
            .deliverables
            .work_target
            .as_ref()
            .map(|target| {
                crate::mcp::tools::core::task::repo_context::resolve_repo_context(root, target)
            })
            .transpose()
        {
            Ok(context) => context,
            Err(_) => continue,
        };
        let repo = context
            .as_ref()
            .map(|context| context.repo_root.as_path())
            .unwrap_or_else(|| root.parent().unwrap_or(root));
        let target = context
            .as_ref()
            .map(|context| context.target_branch.as_str())
            .or(record.branch.as_deref())
            .unwrap_or("master");
        let Ok(children) = store.get_subtasks(&epic.id) else {
            continue;
        };
        let children: Vec<_> = children
            .into_iter()
            .filter(|task| task.status != cas_types::TaskStatus::Cancelled)
            .collect();
        // Undelivered assignments contribute to total, but their worker's
        // reused lane cannot establish a merge for this task. Do not spend
        // the bounded proof budget measuring unrelated work on that lane.
        let deliveries: Vec<_> = children
            .iter()
            .filter(|task| task.has_delivery_to_integrate() && has_recorded_delivery(task))
            .cloned()
            .collect();
        let Some(merged_children) = collect_epic_ancestry_merges(
            &deliveries,
            target,
            repo,
            std::time::Duration::from_millis(250),
        ) else {
            continue;
        };
        let mut progress = EpicProgressJson {
            total: children.len(),
            merged: 0,
            last_merged_at: None,
            merges_per_hour: 0,
        };
        let mut last = None;
        for child in &deliveries {
            if !merged_children.contains(&child.id) {
                continue;
            }
            checked_ids.insert(child.id.clone());
            merged_ids.insert(child.id.clone());
            progress.merged += 1;
            let at = child.deliverables.delivery_pr_merged_at.or_else(|| {
                child
                    .deliverables
                    .merge_commit
                    .as_deref()
                    .or(child.deliverables.delivery_pr_merge_commit.as_deref())
                    .or_else(|| {
                        // Factory integration records the actual merge in its
                        // green admission receipt rather than merge_commit.
                        let delivery = &child.deliverables;
                        let commit = delivery.green_merge_commit.as_deref()?;
                        let child_target = delivery.work_target.as_ref()
                            .map(|target| target.target_branch.as_str())
                            .unwrap_or(target);
                        (delivery.merge_ci_admission == Some(cas_types::MergeCiAdmission::Green)
                            && (git_commit_is_ancestor(repo, commit, child_target)
                                || git_commit_is_ancestor(repo, commit, &format!("origin/{child_target}"))))
                            .then_some(commit)
                    })
                    .and_then(|commit| last_commit_unix(repo, commit))
                    .and_then(|unix| chrono::DateTime::<chrono::Utc>::from_timestamp(unix, 0))
            });
            if let Some(at) = at {
                last = Some(last.map_or(at, |previous: chrono::DateTime<chrono::Utc>| {
                    previous.max(at)
                }));
                if at <= now && at > now - chrono::Duration::hours(1) {
                    progress.merges_per_hour += 1;
                }
            }
        }
        progress.last_merged_at = last.map(|at| at.to_rfc3339());
        epic.progress = Some(progress);
    }
    // A task without an epic still has an explicit delivery target. Reuse the
    // very same collector, rather than trusting its parked lifecycle status.
    for task in status
        .tasks_in_progress
        .iter_mut()
        .chain(status.tasks_ready.iter_mut())
    {
        if checked_ids.contains(&task.id) {
            task.merge_check_pending = false;
        }
        if merged_ids.contains(&task.id) {
            task.delivery_state = Some("merged");
            continue;
        }
        if task.epic.is_some() || started.elapsed() >= std::time::Duration::from_secs(2) {
            continue;
        }
        let Ok(record) = store.get(&task.id) else {
            continue;
        };
        if !record.has_delivery_to_integrate() || !has_recorded_delivery(&record) {
            continue;
        }
        let Some(context) = record.deliverables.work_target.as_ref().and_then(|target| {
            crate::mcp::tools::core::task::repo_context::resolve_repo_context(root, target).ok()
        }) else {
            continue;
        };
        let collection = collect_epic_branch_statuses_with_options(
            std::slice::from_ref(&record),
            &context.target_branch,
            &context.repo_root,
            EpicStatusOptions {
                offset: 0,
                limit: Some(1),
                summary: false,
                budget: std::time::Duration::from_millis(250),
            },
        );
        if collection
            .statuses
            .first()
            .is_some_and(|row| !row.refs_unresolved && row.content_check_error.is_none())
        {
            task.merge_check_pending = false;
        }
        if collection.statuses.first().is_some_and(|row| {
            !row.refs_unresolved
                && (row.recorded_anchor.is_some() || row.factory_branch.is_some())
                && row.unmerged_count == 0
                && row.dropped_paths.is_empty()
                && row.content_check_error.is_none()
        }) {
            task.delivery_state = Some("merged");
        }
    }
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

    fn fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        crate::ui::factory::SessionInfo,
    ) {
        let project = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(project.path()).unwrap();
        let metadata = crate::ui::factory::create_metadata(
            "selected-session",
            std::process::id(),
            "supervisor",
            &["worker-a".into()],
            None,
            Some(project.path().to_str().unwrap()),
            None,
        );
        let session = crate::ui::factory::SessionInfo {
            name: "selected-session".into(),
            metadata,
            is_running: true,
            socket_exists: false,
        };
        (project, root, session)
    }

    #[test]
    fn selected_session_phase_ignores_another_projects_ambient_root() {
        use cas_store::AgentStore;
        use cas_types::{Agent, AgentRole};
        let mut env = crate::test_env_guard::TestEnvGuard::temp_home();
        let (_project, root, session) = fixture();
        let other = tempfile::tempdir().unwrap();
        let other_root = crate::store::init_cas_dir(other.path()).unwrap();
        let account = tempfile::tempdir().unwrap();
        let rollouts = account.path().join("sessions/2026/10/29");
        std::fs::create_dir_all(&rollouts).unwrap();
        let started = "2026-10-29T06:04:27Z";
        std::fs::write(rollouts.join("rollout-phase.jsonl"), format!("{}\n{}\n",
            serde_json::json!({"type":"session_meta","payload":{"cwd":session.metadata.project_dir,"originator":"codex_cli_rs","source":"cli"}}),
            serde_json::json!({"timestamp":started,"type":"response_item","payload":{"type":"function_call","call_id":"native-call","name":"exec_command","arguments":"{\"cmd\":\"private command\"}"}}),
        )).unwrap();
        let mut supervisor = Agent::new_with_role("codex-supervisor".into(), "supervisor".into(), AgentRole::Supervisor);
        supervisor.factory_session = Some(session.name.clone());
        supervisor.metadata.insert("supervisor_cli".into(), "codex".into());
        supervisor.metadata.insert("supervisor_account_dir".into(), account.path().to_string_lossy().into_owned());
        crate::store::open_agent_store(&root).unwrap().register(&supervisor).unwrap();
        env.set("CAS_ROOT", other_root.to_str().unwrap());
        let selected = cas_root_for_session_with_fallback(&session, None).unwrap();
        let status = serde_json::to_value(build_status_json(&session, &selected, 20).unwrap()).unwrap();
        assert_eq!(status["supervisor_phase"]["kind"], "tool", "selected session must read its own registered native source: {status}");
        assert_eq!(status["supervisor_phase"]["lifecycle"]["started_at"], "2026-10-29T06:04:27+00:00");
        assert!(!status["supervisor_phase"].to_string().contains("private command"));
    }

    #[test]
    fn status_projects_selected_session_worker_from_worker_status_evidence() {
        use cas_store::AgentStore;
        use cas_types::{Agent, AgentRole, Task, TaskStatus};
        let mut home = crate::test_env_guard::TestEnvGuard::temp_home();
        let (_project, root, mut session) = fixture();
        session.metadata.workers.clear();
        home.set("CAS_FACTORY_SESSION", "other-hub-session");
        let mut worker =
            Agent::new_with_role("worker-id".into(), "worker-a".into(), AgentRole::Worker);
        worker.factory_session = Some(session.name.clone());
        // A real exited PID yields the same deterministic verdict in the hub
        // and worker_status. A fresh registry heartbeat cannot hide death.
        worker.pid = Some(i32::MAX as u32);
        worker.metadata.insert("worker_cli".into(), "codex".into());
        worker
            .metadata
            .insert("worker_model".into(), "gpt-6.1".into());
        crate::store::open_agent_store(&root)
            .unwrap()
            .register(&worker)
            .unwrap();
        let store = crate::store::open_task_store(&root).unwrap();
        let mut task = Task::new("cas-current".into(), "Current worker task".into());
        task.status = TaskStatus::InProgress;
        task.assignee = Some(worker.id.clone());
        store.add(&task).unwrap();
        let started = chrono::Utc::now() - chrono::Duration::minutes(42);
        let mut event = Event::new(
            cas_types::EventType::TaskStarted,
            cas_types::EventEntityType::Task,
            &task.id,
            "Raw details must not leak",
        );
        event.created_at = started;
        event.id = SqliteEventStore::open(&root)
            .unwrap()
            .record(&event)
            .unwrap();
        let status = serde_json::to_value(build_status_json(&session, &root, 20).unwrap()).unwrap();
        let projected = status["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["name"] == "worker-a")
            .expect("selected worker must be reported");
        assert_eq!(projected["current_task"], task.id);
        assert_eq!(projected["harness"], "codex");
        assert_eq!(projected["model"], "gpt-6.1");
        assert_eq!(projected["task_started_at"], started.to_rfc3339());
        assert_eq!(
            projected["recent_trail"],
            serde_json::json!([{"step":"started", "at": started.to_rfc3339(), "reference": format!("event:{}", event.id)}])
        );
        let expected = crate::mcp::tools::service::factory_ops::worker_liveness_for_agent(
            &root,
            &worker,
            chrono::Utc::now(),
            300,
        );
        assert_eq!(projected["liveness"], expected.state.as_str());
        assert_eq!(projected["liveness"], "dead");
    }

    #[test]
    fn executing_workers_current_task_agrees_with_the_same_status_payload() {
        use cas_types::{Agent, AgentRole, Task, TaskStatus};
        let _home = crate::test_env_guard::TestEnvGuard::temp_home();
        let (project, root, session) = fixture();
        let account = tempfile::tempdir().unwrap();
        let rollouts = account.path().join("sessions/2026/10/29");
        std::fs::create_dir_all(&rollouts).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let native = [
            serde_json::json!({"type":"session_meta", "payload":{"id":"worker-native-session", "cwd":project.path(), "source":"cli", "originator":"codex_cli_rs"}}),
            serde_json::json!({"timestamp":now, "type":"event_msg", "payload":{"type":"task_started"}}),
        ];
        std::fs::write(rollouts.join("rollout-worker-native-session.jsonl"),
            native.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
        let mut worker = Agent::new_with_role("worker-native-session".into(), "worker-a".into(), AgentRole::Worker);
        worker.metadata.insert("worker_cli".into(), "codex".into());
        worker.metadata.insert("clone_path".into(), project.path().to_string_lossy().into_owned());
        worker.metadata.insert("worker_account_dir".into(), account.path().to_string_lossy().into_owned());
        worker.factory_session = Some(session.name.clone());
        crate::store::open_agent_store(&root).unwrap().register(&worker).unwrap();
        let tasks = crate::store::open_task_store(&root).unwrap();
        let mut task = Task::new("cas-current-executing".into(), "Current executing task".into());
        task.status = TaskStatus::InProgress;
        task.assignee = Some(worker.name.clone());
        tasks.add(&task).unwrap();
        // Both spellings decode to the supported domain status. The legacy
        // spelling exposes separate filtered/unfiltered read disagreement.
        let db = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        for stored_status in ["in_progress", "inprogress"] {
            db.execute("UPDATE tasks SET status = ?1 WHERE id = ?2", rusqlite::params![stored_status, task.id]).unwrap();
            let status = build_status_json(&session, &root, 20).unwrap();
            let reported = status.tasks_in_progress.iter().find(|row| row.id == task.id).unwrap();
            assert_eq!(reported.assignee.as_deref(), Some(worker.name.as_str()));
            let agent = status.agents.iter().find(|agent| agent.name == worker.name).unwrap();
            assert_eq!(agent.liveness.as_deref(), Some("executing"), "native turn evidence: {stored_status}");
            assert_eq!(agent.current_task.as_deref(), Some(task.id.as_str()), "same response: {stored_status}");
        }
    }

    #[test]
    fn staffing_reassignment_replaces_parked_work_after_registration_recycle() {
        use cas_types::{Agent, AgentRole, AgentStatus, Task, TaskStatus};
        let _home = crate::test_env_guard::TestEnvGuard::temp_home();
        let (_project, root, session) = fixture();
        let agents = crate::store::open_agent_store(&root).unwrap();
        let tasks = crate::store::open_task_store(&root).unwrap();
        let mut previous = Agent::new_with_role("old-worker".into(), "worker-a".into(), AgentRole::Worker);
        previous.factory_session = Some(session.name.clone());
        previous.status = AgentStatus::Shutdown;
        previous.last_heartbeat = chrono::Utc::now() - chrono::Duration::minutes(2);
        agents.register(&previous).unwrap();
        let mut current = previous.clone();
        current.id = "new-worker".into();
        current.status = AgentStatus::Active;
        current.last_heartbeat = chrono::Utc::now();
        agents.register(&current).unwrap();
        let mut parked = Task::new("cas-old-delivery".into(), "Previous delivery".into());
        parked.assignee = Some(current.name.clone());
        parked.status = TaskStatus::AwaitingMerge;
        tasks.add(&parked).unwrap();
        let mut next = Task::new("cas-next-assignment".into(), "New assignment".into());
        next.assignee = Some(current.name.clone());
        tasks.add(&next).unwrap();
        for owner in [&current.name, &current.id, &previous.id] {
            next.assignee = Some(owner.clone());
            for state in [TaskStatus::Open, TaskStatus::InProgress] {
                next.status = state;
                tasks.update(&next).unwrap();
                let status = build_status_json(&session, &root, 20).unwrap();
                let worker = status.agents.iter().find(|agent| agent.name == current.name).unwrap();
                assert_eq!(worker.current_task.as_deref(), Some(next.id.as_str()), "{owner} / {state}");
            }
        }
        // A fresher same-name registration in another factory is neither
        // this session's authoritative worker nor an ownership alias.
        let mut foreign = current.clone();
        foreign.id = "foreign-worker".into();
        foreign.factory_session = Some("other-session".into());
        foreign.last_heartbeat += chrono::Duration::seconds(1);
        agents.register(&foreign).unwrap();
        next.assignee = Some(foreign.id);
        tasks.update(&next).unwrap();
        let status = build_status_json(&session, &root, 20).unwrap();
        let worker = status.agents.iter().find(|agent| agent.name == current.name).unwrap();
        assert_eq!(worker.id, current.id);
        assert_eq!(worker.current_task.as_deref(), Some(parked.id.as_str()));
    }

    #[test]
    fn newer_parked_work_wins_over_an_older_integrated_delivery_after_a_note() {
        use cas_types::{Agent, AgentRole, Dependency, DependencyType, EventEntityType, EventType, Task, TaskStatus, TaskType};
        let _home = crate::test_env_guard::TestEnvGuard::temp_home();
        let (project, root, session) = fixture();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(project.path()).output().unwrap();
            assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.test"]);
        git(&["commit", "--allow-empty", "-m", "Integrated older delivery"]);
        let integrated = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-b", "factory/pending"]);
        git(&["commit", "--allow-empty", "-m", "Newer parked delivery"]);
        let pending = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "main"]);
        let mut worker = Agent::new_with_role("worker-id".into(), "worker-a".into(), AgentRole::Worker);
        worker.factory_session = Some(session.name.clone());
        crate::store::open_agent_store(&root).unwrap().register(&worker).unwrap();
        let tasks = crate::store::open_task_store(&root).unwrap();
        let mut epic = Task::new("cas-release".into(), "Release".into());
        epic.task_type = TaskType::Epic;
        epic.branch = Some("main".into());
        tasks.add(&epic).unwrap();
        let now = chrono::Utc::now();
        let events = SqliteEventStore::open(&root).unwrap();
        for (id, title, anchor, minutes, updated) in [
            ("cas-old-parked", "Older integrated delivery", integrated.as_str(), 90, now),
            ("cas-new-parked", "Latest parked work", pending.as_str(), 15, now - chrono::Duration::minutes(10)),
        ] {
            let mut task = Task::new(id.into(), title.into());
            task.created_at = now - chrono::Duration::minutes(minutes + 10);
            task.updated_at = updated;
            task.status = TaskStatus::AwaitingMerge;
            task.assignee = Some(worker.name.clone());
            task.deliverables.factory_branch_anchor = Some(anchor.into());
            if minutes == 90 { task.deliverables.merge_commit = Some(integrated.clone()); }
            tasks.add(&task).unwrap();
            tasks.add_dependency(&Dependency { from_id:id.into(), to_id:epic.id.clone(), dep_type:DependencyType::ParentChild, created_at:now, created_by:None }).unwrap();
            let mut start = Event::new(EventType::TaskStarted, EventEntityType::Task, id, "Started assigned work");
            start.created_at = now - chrono::Duration::minutes(minutes);
            events.record(&start).unwrap();
        }
        let status = build_status_json(&session, &root, 20).unwrap();
        let old = status.tasks_in_progress.iter().find(|task| task.id == "cas-old-parked").unwrap();
        assert_eq!(old.delivery_state, Some("merged"), "real Git proves the old delivery is integrated");
        let current = status.agents.iter().find(|agent| agent.name == worker.name).unwrap();
        assert_eq!(current.current_task.as_deref(), Some("cas-new-parked"));
        let row = status.tasks_in_progress.iter().find(|task| Some(task.id.as_str()) == current.current_task.as_deref()).unwrap();
        assert_eq!(row.title, "Latest parked work", "lane task identity and title agree");
        let mut active = Task::new("cas-still-active".into(), "In-progress work".into());
        active.assignee = Some(worker.name.clone());
        active.status = TaskStatus::InProgress;
        tasks.add(&active).unwrap();
        let status = build_status_json(&session, &root, 20).unwrap();
        assert_eq!(status.agents.iter().find(|agent| agent.name == worker.name).unwrap().current_task.as_deref(), Some(active.id.as_str()));
    }

    #[test]
    fn team_trail_requires_recorded_delivery_and_current_approved_verdict() {
        let _home = crate::test_env_guard::TestEnvGuard::temp_home();
        let (_project, root, _session) = fixture();
        let events = SqliteEventStore::open(&root).unwrap();
        let mut task = cas_types::Task::new("cas-trail".into(), "Trail".into());
        task.status = cas_types::TaskStatus::Closed;
        task.deliverables.factory_branch_anchor = Some("local-commit".into());
        assert!(
            worker_task_facts(&root, &events, Some(&task))
                .unwrap()
                .1
                .is_empty()
        );
        let mut start = Event::new(
            cas_types::EventType::TaskStarted,
            cas_types::EventEntityType::Task,
            &task.id,
            "Started",
        );
        start.created_at = chrono::Utc::now() - chrono::Duration::minutes(10);
        events.record(&start).unwrap();
        task.deliverables.parked_branch = Some("factory/worker".into());
        task.deliverables.merge_commit = Some("merge-sha".into());
        task.deliverables.green_merge_commit = Some("merge-sha".into());
        task.deliverables.merge_ci_admission = Some(cas_types::MergeCiAdmission::Advisory);
        let store = crate::store::open_verification_store(&root).unwrap();
        let mut verdict = cas_types::Verification::new("ver-old".into(), task.id.clone());
        verdict.created_at = start.created_at - chrono::Duration::minutes(1);
        store.add(&verdict).unwrap();
        let steps = |task: &cas_types::Task| {
            worker_task_facts(&root, &events, Some(task))
                .unwrap()
                .1
                .into_iter()
                .map(|mark| mark.step)
                .collect::<Vec<_>>()
        };
        assert_eq!(steps(&task), ["started", "pushed", "merged"]);
        task.deliverables.merge_ci_admission = Some(cas_types::MergeCiAdmission::Green);
        verdict.id = "ver-current".into();
        verdict.created_at = chrono::Utc::now();
        store.add(&verdict).unwrap();
        assert_eq!(
            steps(&task),
            ["started", "pushed", "ci_green", "merged", "verified"]
        );
        verdict.status = cas_types::VerificationStatus::Rejected;
        store.update(&verdict).unwrap();
        assert_eq!(steps(&task), ["started", "pushed", "ci_green", "merged"]);
    }

    #[test]
    fn cas_9d2a_unpinned_status_projects_staffed_epic_before_historical_epics() {
        use cas_store::AgentStore;
        use cas_types::{Dependency, DependencyType, Task, TaskStatus, TaskType};
        use crate::mcp::tools::core::task::lifecycle::close_ops::{
            EpicStatusOptions, collect_epic_branch_statuses_with_options,
        };
        let mut home = crate::test_env_guard::TestEnvGuard::temp_home();
        let (project, root, session) = fixture();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(project.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "fixture@example.test"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["commit", "--allow-empty", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        let mut anchors = Vec::new();
        for index in 0..35 {
            if index == 34 {
                git(&["checkout", "-b", "factory/pending"]);
            }
            let file = format!("delivery-{index}");
            std::fs::write(project.path().join(&file), format!("delivery {index}\n")).unwrap();
            git(&["add", &file]);
            git(&["commit", "-m", &file]);
            anchors.push(git(&["rev-parse", "HEAD"]));
        }
        git(&["checkout", "main"]);
        git(&["branch", "factory/fresh-worker"]);
        git(&["branch", "historical"]);
        let agents = crate::store::open_agent_store(&root).unwrap();
        for (id, name, role) in [
            (
                "supervisor-id",
                "supervisor",
                cas_types::AgentRole::Supervisor,
            ),
            ("worker-id", "worker-a", cas_types::AgentRole::Worker),
        ] {
            let mut agent = cas_types::Agent::new_with_role(id.into(), name.into(), role);
            agent.factory_session = Some(session.name.clone());
            agents.register(&agent).unwrap();
        }
        let store = crate::store::open_task_store(&root).unwrap();
        // These higher-priority historical epics precede the active release
        // in the project catalog. Trace actual Git reads rather than depending
        // on a particular machine exhausting the two-second budget.
        for index in 0..296 {
            let mut historical = Task::new(format!("cas-history-{index}"), "Old release".into());
            historical.task_type = TaskType::Epic;
            historical.status = TaskStatus::Closed;
            historical.priority = cas_types::Priority(0);
            historical.branch = Some("historical".into());
            if index == 0 {
                // An idle epic with the same supervisor must not win over
                // the release that actually has a staffed in-progress task.
                historical.status = TaskStatus::Open;
                historical.epic_verification_owner = Some("supervisor-id".into());
            }
            store.add(&historical).unwrap();
        }
        let mut epic = Task::new("cas-epic".into(), "Epic".into());
        epic.task_type = TaskType::Epic;
        epic.branch = Some("main".into());
        epic.epic_verification_owner = Some("supervisor-id".into());
        store.add(&epic).unwrap();
        let mut children = Vec::new();
        for (index, anchor) in anchors.iter().enumerate() {
            let id = if index == 0 { "cas-landed".into() } else { format!("cas-delivery-{index}") };
            let mut child = Task::new(id, format!("Delivery {index}"));
            child.status = TaskStatus::AwaitingMerge;
            child.deliverables.factory_branch_anchor = Some(anchor.clone());
            if index == 0 {
                child.deliverables.merge_commit = Some(anchor.clone());
            } else {
                child.deliverables.green_merge_commit = Some(anchor.clone());
                child.deliverables.merge_ci_admission = Some(if index == 1 {
                    cas_types::MergeCiAdmission::Advisory
                } else {
                    cas_types::MergeCiAdmission::Green
                });
            }
            children.push(child);
        }
        for (id, status, assignee) in [
            ("cas-open", TaskStatus::Open, Some("fresh-worker")),
            ("cas-active", TaskStatus::InProgress, Some("worker-id")),
            ("cas-cancel", TaskStatus::Cancelled, None),
        ] {
            let mut child = Task::new(id.into(), id.into());
            child.status = status;
            child.assignee = assignee.map(str::to_string);
            children.push(child);
        }
        for child in &children {
            store.add(child).unwrap();
            store.add_dependency(&Dependency {
                from_id: child.id.clone(),
                to_id: epic.id.clone(),
                dep_type: DependencyType::ParentChild,
                created_at: chrono::Utc::now(),
                created_by: None,
            }).unwrap();
        }
        let trace = project.path().join("status-git.trace");
        home.set("GIT_TRACE", &trace);
        // Like the native snapshot: 297 epics, 35 distinct delivery anchors,
        // and no focus. The first refresh must contain the current header.
        let status = serde_json::to_value(build_status_json(&session, &root, 100).unwrap()).unwrap();
        assert!(status["focused_epic"].is_null());
        assert_eq!(status["epics"].as_array().unwrap().len(), 297);
        let reads = std::fs::read_to_string(trace).unwrap();
        let first_epic_read = reads
            .lines()
            .find(|line| line.contains("for-each-ref") && line.contains("committerdate:unix"))
            .expect("status must collect real Git delivery evidence");
        assert!(
            first_epic_read.contains("refs/heads/main"),
            "the selected supervisor's staffed epic must be measured before history: {first_epic_read}"
        );
        assert_eq!(
            reads.lines().filter(|line| line.contains("git rev-list") && line.contains("--not")).count(),
            1,
            "all 35 anchors must share one ancestry pass"
        );
        let landed = status["tasks_in_progress"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == "cas-landed")
            .unwrap();
        assert_eq!(landed["epic_id"], epic.id);
        assert_eq!(landed["delivery_state"], "merged", "{status:#}");
        let progress = &status["epics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == epic.id)
            .unwrap()["progress"];
        assert_eq!(progress["total"], 37);
        assert_eq!(progress["merged"], 34);
        assert_eq!(progress["merges_per_hour"], 33);
        assert!(
            chrono::DateTime::parse_from_rfc3339(progress["last_merged_at"].as_str().unwrap()).is_ok()
        );
        assert!(
            status["tasks_ready"]
                .as_array()
                .unwrap()
                .iter()
                .all(|task| task.get("delivery_state").is_none())
        );
        // Read-only live verdicts cannot authorize a close-gate decision.
        let gate = collect_epic_branch_statuses_with_options(
            &store.get_subtasks(&epic.id).unwrap(),
            "main",
            project.path(),
            EpicStatusOptions {
                offset: 0,
                limit: None,
                summary: false,
                budget: std::time::Duration::ZERO,
            },
        );
        assert!(
            gate.statuses.is_empty(),
            "projection proofs must stay out of the close cache"
        );
        // A moved delivery target must invalidate completed live proofs.
        git(&["update-ref", "refs/heads/main", &base]);
        let changed = serde_json::to_value(build_status_json(&session, &root, 20).unwrap()).unwrap();
        let progress = &changed["epics"]
            .as_array().unwrap().iter()
            .find(|item| item["id"] == epic.id).unwrap()["progress"];
        assert_eq!(progress["total"], 37);
        assert_eq!(progress["merged"], 0, "moved refs must not reuse a live merge verdict");
        assert!(progress["last_merged_at"].is_null());
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

        // The status catalog contains all project epics, including foreign and
        // terminal ones. Preserve ownership so Commander can scope honestly.
        let mut supervisor = cas_types::Agent::new_with_role(
            "supervisor-registration".into(), "supervisor".into(), cas_types::AgentRole::Supervisor,
        );
        supervisor.factory_session = Some(name.into());
        crate::store::open_agent_store(&cas_root).unwrap().register(&supervisor).unwrap();
        let store = crate::store::open_task_store(&cas_root).unwrap();
        for (id, state, owner, verifier) in [
            (
                "cas-owned",
                cas_types::TaskStatus::InProgress,
                Some("supervisor"),
                None,
            ),
            (
                "cas-foreign",
                cas_types::TaskStatus::Open,
                Some("other"),
                None,
            ),
            (
                "cas-closed",
                cas_types::TaskStatus::Closed,
                None,
                Some("supervisor"),
            ),
        ] {
            let mut epic = cas_types::Task::new(id.into(), id.into());
            epic.task_type = cas_types::TaskType::Epic;
            epic.status = state;
            epic.assignee = owner.map(str::to_string);
            epic.epic_verification_owner = verifier.map(str::to_string);
            store.add(&epic).unwrap();
        }
        let status_json =
            || serde_json::to_value(build_status_json(&session, &cas_root, 20).unwrap()).unwrap();
        let status = status_json();
        assert_eq!(status["agents"].as_array().unwrap().iter()
            .find(|agent| agent["name"] == "supervisor").unwrap()["current_task"], "cas-owned");
        let epics = status["epics"].as_array().unwrap();
        assert_eq!(epics.len(), 3);
        for (id, state, owner, verifier) in [
            ("cas-owned", "inprogress", Some("supervisor"), None),
            ("cas-foreign", "open", Some("other"), None),
            ("cas-closed", "closed", None, Some("supervisor")),
        ] {
            let epic = epics.iter().find(|epic| epic["id"] == id).unwrap();
            assert_eq!(epic["status"], state);
            assert_eq!(epic["assignee"].as_str(), owner);
            assert_eq!(epic["epic_verification_owner"].as_str(), verifier);
        }
        assert!(
            status
                .get("focused_epic")
                .is_some_and(serde_json::Value::is_null),
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
        assert!(std::sync::Arc::ptr_eq(
            &first,
            &status_task_store(&cas_root).unwrap()
        ));
        let mut finished = store.get("cas-owned").unwrap();
        finished.status = cas_types::TaskStatus::Closed;
        store.update(&finished).unwrap();
        assert!(status_json()["agents"].as_array().unwrap().iter()
            .find(|agent| agent["name"] == "supervisor").unwrap()["current_task"].is_null());
    }
}
