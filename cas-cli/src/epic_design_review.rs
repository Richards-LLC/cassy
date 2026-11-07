//! Epic-scoped staffing pauses for repeated stakeholder defect reports.
//!
//! Reports are Bug tasks labelled `stakeholder-reported`, priority P0/P1.
//! Their creation time is the report time, including subsequently closed bugs.
//! A pause latches until an operator records a reasoned lift; existing reports
//! are acknowledged by that lift and only newer reports can trigger again.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use cas_types::{DependencyType, Task, TaskType};
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

const STATE_FILE: &str = "epic-design-reviews.json";
const LIFTS_FILE: &str = "operator/epic-design-review-lifts.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pause {
    triggered_at: DateTime<Utc>,
    reports: Vec<String>,
    count: usize,
    window_hours: u32,
}

impl Pause {
    fn message(&self, epic: &str) -> String {
        format!(
            "Epic {epic}: staffing paused for a design review against the plan.\nTrigger: {} stakeholder-reported P0/P1 bugs within {} hours: {}.\nOperator: cas factory design-review-lift {epic} --reason '<review outcome>'",
            self.count,
            self.window_hours,
            self.reports.join(", "),
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Lift {
    at: DateTime<Utc>,
    reason: String,
}

fn read_map<T: serde::de::DeserializeOwned>(path: &Path) -> Result<BTreeMap<String, T>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("Invalid {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error.into()),
    }
}

fn write_map<T: Serialize>(path: &Path, state: &BTreeMap<String, T>) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .context("Design review state needs a parent")?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec_pretty(state)?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

fn lock(root: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(root)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("epic-design-reviews.lock"))?;
    file.lock_exclusive()?;
    Ok(file)
}

struct Graph {
    tasks: HashMap<String, Task>,
    parents: HashMap<String, Vec<String>>,
}

impl Graph {
    fn load(store: &dyn cas_store::TaskStore, task_id: &str) -> Result<Self> {
        // Read only this task's ancestors and their subtrees. An unrelated
        // malformed backlog row must not stop another epic's staffing.
        let mut graph = Self {
            tasks: HashMap::new(),
            parents: HashMap::new(),
        };
        let mut pending = vec![task_id.to_string()];
        while let Some(id) = pending.pop() {
            if graph.tasks.contains_key(&id) {
                continue;
            }
            let task = store.get(&id).map_err(|error| anyhow::anyhow!(
                "task_id {id} could not be read (no such task, or task store unavailable): {error}"))?;
            let parents: Vec<_> = store
                .get_dependencies(&id)?
                .into_iter()
                .filter(|edge| edge.dep_type == DependencyType::ParentChild)
                .map(|edge| edge.to_id)
                .collect();
            pending.extend(parents.iter().cloned());
            graph.parents.insert(id.clone(), parents);
            graph.tasks.insert(id, task);
        }
        pending = graph
            .tasks
            .values()
            .filter(|task| task.task_type == TaskType::Epic)
            .map(|task| task.id.clone())
            .collect();
        let mut expanded = HashSet::new();
        while let Some(parent) = pending.pop() {
            if !expanded.insert(parent.clone()) {
                continue;
            }
            for child in store.get_subtasks(&parent)? {
                let parents = graph.parents.entry(child.id.clone()).or_default();
                if !parents.contains(&parent) {
                    parents.push(parent.clone());
                }
                pending.push(child.id.clone());
                graph.tasks.insert(child.id.clone(), child);
            }
        }
        Ok(graph)
    }

    fn epics(&self, id: &str) -> HashSet<String> {
        let mut pending = vec![id.to_string()];
        let mut seen = HashSet::new();
        let mut epics = HashSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if self
                .tasks
                .get(&id)
                .is_some_and(|task| task.task_type == TaskType::Epic)
            {
                epics.insert(id.clone());
            }
            pending.extend(self.parents.get(&id).into_iter().flatten().cloned());
        }
        epics
    }
}

/// Persist triggers on the mutation that supplies the Nth report. Admission
/// repeats this evaluation to cover sync/imports and already-queued spawns.
/// Returning a pause is not a failed report creation: the report must survive.
fn refresh_at(root: &Path, task_id: &str, now: DateTime<Utc>) -> Result<Option<(String, Pause)>> {
    let _lock = lock(root)?;
    let config = crate::config::Config::load(root)?.factory();
    ensure!(
        (1..=1000).contains(&config.design_review_bug_threshold),
        "factory.design_review_bug_threshold must be between 1 and 1000"
    );
    ensure!(
        (1..=8760).contains(&config.design_review_window_hours),
        "factory.design_review_window_hours must be between 1 and 8760"
    );
    let store = crate::store::open_task_store(root)?;
    let graph = Graph::load(store.as_ref(), task_id)?;
    ensure!(
        graph.tasks.contains_key(task_id),
        "Task {task_id} not found while checking epic design review"
    );
    let mut epics: Vec<_> = graph.epics(task_id).into_iter().collect();
    epics.sort();
    if epics.is_empty() {
        return Ok(None);
    }
    let mut state: BTreeMap<String, Pause> = read_map(&root.join(STATE_FILE))?;
    let lifts: BTreeMap<String, Lift> = read_map(&root.join(LIFTS_FILE))?;
    let mut first = None;
    for epic in epics {
        let lifted_at = lifts.get(&epic).map(|lift| lift.at);
        let active = state
            .get(&epic)
            .filter(|pause| lifted_at.is_none_or(|at| pause.triggered_at > at))
            .cloned();
        let pause = match active {
            Some(pause) => Some(pause),
            None => {
                let cutoff = now - Duration::hours(i64::from(config.design_review_window_hours));
                let mut reports: Vec<_> = graph
                    .tasks
                    .values()
                    .filter(|task| {
                        task.task_type == TaskType::Bug
                            && (0..=1).contains(&task.priority.0)
                            && task
                                .labels
                                .iter()
                                .any(|label| label == "stakeholder-reported")
                            && task.created_at >= cutoff
                            && task.created_at <= now
                            && lifted_at.is_none_or(|at| task.created_at > at)
                            && graph.epics(&task.id).contains(&epic)
                    })
                    .map(|task| task.id.clone())
                    .collect();
                reports.sort();
                if reports.len() >= config.design_review_bug_threshold as usize {
                    let pause = Pause {
                        triggered_at: now,
                        count: reports.len(),
                        reports,
                        window_hours: config.design_review_window_hours,
                    };
                    state.insert(epic.clone(), pause.clone());
                    write_map(&root.join(STATE_FILE), &state)?;
                    store.append_note(
                        &epic,
                        &format!(
                            "[{}] DESIGN REVIEW: {}",
                            now.format("%Y-%m-%d %H:%M"),
                            pause.message(&epic)
                        ),
                    )?;
                    Some(pause)
                } else {
                    None
                }
            }
        };
        if let Some(pause) = pause {
            notify_supervisor(
                root,
                graph.tasks.get(&epic).context("Missing epic")?,
                &pause,
            )?;
            if first.is_none() {
                first = Some((epic, pause));
            }
        }
    }
    Ok(first)
}

fn notify_supervisor(root: &Path, epic: &Task, pause: &Pause) -> Result<()> {
    let agents = crate::store::open_agent_store(root)?.list(None)?;
    let owner = epic.epic_verification_owner.as_deref();
    let owner_resolved = agents.iter().any(|agent| {
        owner.is_some_and(|owner| {
            owner == agent.id
                || owner == agent.name
                || Some(owner) == agent.cc_session_id.as_deref()
        })
    });
    let queue = crate::store::open_prompt_queue_store(root)?;
    for supervisor in agents.iter().filter(|agent| {
        agent.role == cas_types::AgentRole::Supervisor
            && agent.is_alive()
            && (!owner_resolved
                || owner.is_some_and(|owner| {
                    owner == agent.id
                        || owner == agent.name
                        || Some(owner) == agent.cc_session_id.as_deref()
                }))
    }) {
        queue.enqueue_idempotent(
            "epic-design-review",
            &supervisor.name,
            &pause.message(&epic.id),
            supervisor.factory_session.as_deref(),
            Some("Epic staffing paused: design review required"),
            Some(cas_store::NotificationPriority::High),
            &format!(
                "epic-design-review:{}:{}:{}",
                epic.id,
                pause.triggered_at.to_rfc3339(),
                supervisor.id
            ),
            Some(&cas_store::QueueOrigin::Daemon),
        )?;
    }
    let _ = cas_factory::notify_daemon(root);
    Ok(())
}

pub(crate) fn observe(root: &Path, task_id: &str) -> Result<()> {
    refresh_at(root, task_id, Utc::now()).map(|_| ())
}

pub(crate) fn ensure_staffing_allowed(root: &Path, task_id: &str) -> Result<()> {
    crate::mcp::tools::core::workflow::release_freeze::check_staffing(root, task_id)?;
    if let Some((epic, pause)) = refresh_at(root, task_id, Utc::now())? {
        anyhow::bail!(pause.message(&epic));
    }
    Ok(())
}

fn record_lift(root: &Path, epic_id: &str, reason: &str, now: DateTime<Utc>) -> Result<()> {
    ensure!(
        !reason.trim().is_empty(),
        "--reason must explain the design review outcome and why staffing can resume"
    );
    let _lock = lock(root)?;
    let store = crate::store::open_task_store(root)?;
    ensure!(
        store.get(epic_id)?.task_type == TaskType::Epic,
        "{epic_id} is not an epic"
    );
    let state: BTreeMap<String, Pause> = read_map(&root.join(STATE_FILE))?;
    let mut lifts: BTreeMap<String, Lift> = read_map(&root.join(LIFTS_FILE))?;
    let pause = state
        .get(epic_id)
        .context("Epic has no design review pause to lift")?;
    ensure!(
        lifts
            .get(epic_id)
            .is_none_or(|lift| pause.triggered_at > lift.at),
        "Epic design review pause is already lifted"
    );
    store.append_note(
        epic_id,
        &format!(
            "[{}] DECISION operator lifted epic design review pause: {}",
            now.format("%Y-%m-%d %H:%M"),
            reason.trim()
        ),
    )?;
    lifts.insert(
        epic_id.to_string(),
        Lift {
            at: now,
            reason: reason.trim().to_string(),
        },
    );
    write_map(&root.join(LIFTS_FILE), &lifts)
}

/// The operator CLI is the sole lift writer. The lift lives under operator/
/// so neither generic task patches nor agent shell writes can clear a pause.
pub(crate) fn lift(root: &Path, epic_id: &str, reason: &str) -> Result<()> {
    use crate::config::operator_policy::{InvocationContext, operator_action_refusal};
    if let Some(refusal) = operator_action_refusal(
        "Lifting an epic design review pause",
        "run `cas factory design-review-lift` yourself from your own terminal",
        &InvocationContext::from_process(),
    ) {
        anyhow::bail!(crate::cloud::presence::wrap_line(&refusal).trim_end().to_string());
    }
    record_lift(root, epic_id, reason, Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::CasCore;
    use crate::mcp::tools::{TaskCreateRequest, TaskStartRequest};
    use cas_types::{Agent, Dependency, DependencyType, Priority, Task, TaskStatus, TaskType};
    use chrono::{Duration, Utc};
    use rmcp::handler::server::wrapper::Parameters;

    #[tokio::test]
    async fn nth_stakeholder_bug_pauses_only_its_epic() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(project.path()).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        let tasks = core.open_task_store().unwrap();
        tasks.init().unwrap();
        core.open_agent_store()
            .unwrap()
            .register(&Agent::new("review-caller".into(), "caller".into()))
            .unwrap();
        core.set_agent_id_for_testing("review-caller".into());
        let mut supervisor = Agent::new_with_role(
            "review-lead".into(),
            "lead".into(),
            cas_types::AgentRole::Supervisor,
        );
        supervisor.factory_session = Some("design-review-session".into());
        core.open_agent_store()
            .unwrap()
            .register(&supervisor)
            .unwrap();
        for id in ["epic-risk", "epic-other"] {
            let mut epic = Task::new(id.into(), id.into());
            epic.task_type = TaskType::Epic;
            tasks.add(&epic).unwrap();
        }
        let add_child = |id: &str, epic: &str, hours: i64, priority: i32, stakeholder: bool| {
            let mut task = Task::new(id.into(), id.into());
            task.task_type = TaskType::Bug;
            task.priority = Priority(priority);
            task.created_at = Utc::now() - Duration::hours(hours);
            if stakeholder {
                task.labels.push("stakeholder-reported".into());
            }
            tasks.add(&task).unwrap();
            tasks
                .add_dependency(&Dependency::new(
                    id.into(),
                    epic.into(),
                    DependencyType::ParentChild,
                ))
                .unwrap();
        };
        for id in ["report-1", "report-2", "report-3", "report-4"] {
            add_child(id, "epic-risk", 1, 0, true);
        }
        let mut closed_report = tasks.get("report-4").unwrap();
        closed_report.status = TaskStatus::Closed;
        tasks.update(&closed_report).unwrap();
        // Neither an expired report, a P2 nor an internal finding is the fifth report.
        add_child("expired", "epic-risk", 49, 0, true);
        add_child("future", "epic-risk", -1, 0, true);
        add_child("minor", "epic-risk", 0, 2, true);
        add_child("internal", "epic-risk", 0, 0, false);
        add_child("first-start", "epic-risk", 0, 2, false);
        let start = |id: &str| TaskStartRequest {
            id: id.into(),
            brief: Some(true),
            force: None,
        };
        core.cas_task_start_with_options(Parameters(start("first-start")))
            .await
            .unwrap();
        let report: TaskCreateRequest = serde_json::from_value(serde_json::json!({
            "title": "Stakeholder checkout tax is applied twice", "task_type": "bug", "priority": 1,
            "risk": "none", "labels": "stakeholder-reported", "epic": "epic-risk"
        }))
        .unwrap();
        core.cas_task_create(Parameters(report)).await.unwrap();
        let report_id = tasks
            .list(None)
            .unwrap()
            .into_iter()
            .find(|task| task.title == "Stakeholder checkout tax is applied twice")
            .unwrap()
            .id;
        let paused: BTreeMap<String, Pause> = read_map(&root.join(STATE_FILE)).unwrap();
        assert!(
            paused["epic-risk"].reports.contains(&report_id),
            "the report mutation must latch the pause before any start"
        );
        add_child("next-start", "epic-risk", 0, 2, false);
        let refusal = core
            .cas_task_start_with_options(Parameters(start("next-start")))
            .await
            .expect_err("the fifth stakeholder P0/P1 must stop new starts");
        assert!(
            refusal.message.contains("epic-risk")
                && refusal.message.contains(&report_id),
            "{refusal}"
        );
        assert_eq!(tasks.get("next-start").unwrap().status, TaskStatus::Open);
        add_child("unrelated", "epic-other", 0, 2, false);
        core.cas_task_start_with_options(Parameters(start("unrelated")))
            .await
            .unwrap();

        let update = serde_json::from_value(serde_json::json!({
            "id": "next-start", "status": "in_progress"
        }))
        .unwrap();
        let update_error = core.cas_task_update(Parameters(update)).await.unwrap_err();

        add_child("moving-start", "epic-other", 0, 2, false);
        let update = serde_json::from_value(serde_json::json!({
            "id": "moving-start", "status": "in_progress", "epic": "epic-risk"
        }))
        .unwrap();
        let move_error = core.cas_task_update(Parameters(update)).await.unwrap_err();

        assert_eq!(
            tasks.get_parent_epic("moving-start").unwrap().unwrap().id,
            "epic-other",
            "a refused start-and-retarget must preserve epic membership"
        );

        let service_core = CasCore::with_daemon(root.clone(), None, None);
        #[cfg(feature = "mcp-proxy")]
        let service = crate::mcp::CasService::new(service_core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = crate::mcp::CasService::new(service_core);
        for fields in [
            serde_json::json!({"task_id": "next-start"}),
            serde_json::json!({"task_ids": ["unrelated", "next-start"]}),
        ] {
            let mut request = fields;
            request["action"] = serde_json::json!("spawn_workers");
            request["force"] = serde_json::json!(true);
            let error = service
                .factory_spawn_workers_in(
                    &crate::ops::fleet::FleetContext::operator("design-review-session"),
                    serde_json::from_value(request).unwrap(),
                )
                .await
                .unwrap_err();
        }
        let queue = crate::store::open_prompt_queue_store(&root).unwrap();
        let notices = queue
            .peek_unseen_for_recipient("lead", Some("design-review-session"), 20)
            .unwrap();
        assert_eq!(
            notices
                .iter()
                .filter(|notice| notice
                    .prompt
                    .contains("staffing paused for a design review"))
                .count(),
            1,
            "repeated admission checks must not spam the supervisor"
        );
        assert!(
            lift(&root, "epic-risk", "reviewed").is_err(),
            "agent contexts cannot lift a pause"
        );
        assert!(record_lift(&root, "epic-risk", "  ", Utc::now()).is_err());
        let lifted_at = Utc::now() + Duration::seconds(1);
        record_lift(
            &root,
            "epic-risk",
            "Reviewed plan; replace the tax calculation design",
            lifted_at,
        )
        .unwrap();
        core.cas_task_start_with_options(Parameters(start("next-start")))
            .await
            .unwrap();
        assert!(
            tasks
                .get("epic-risk")
                .unwrap()
                .notes
                .contains("Reviewed plan; replace the tax calculation design")
        );

        // Fake time proves a reasoned lift acknowledges old reports, then a
        // fresh cycle and even an expired window cannot silently clear a latch.
        let mut config = crate::config::Config::default();
        config
            .set("factory.design_review_bug_threshold", "2")
            .unwrap();
        config
            .set("factory.design_review_window_hours", "24")
            .unwrap();
        config.save(&root).unwrap();
        for (index, hours) in [(1, -25), (2, -24), (3, 0)] {
            let mut report =
                Task::new(format!("new-report-{index}"), format!("New report {index}"));
            report.task_type = TaskType::Bug;
            report.priority = Priority(1);
            report.labels.push("stakeholder-reported".into());
            report.created_at = lifted_at + Duration::hours(30 + hours);
            tasks.add(&report).unwrap();
            tasks
                .add_dependency(&Dependency::new(
                    report.id.clone(),
                    "epic-risk".into(),
                    DependencyType::ParentChild,
                ))
                .unwrap();
            let pause = refresh_at(&root, "epic-risk", lifted_at + Duration::hours(30)).unwrap();
            if index < 3 {
                assert!(pause.is_none(), "outside-window reports must not count");
            } else {
                assert!(pause.is_some(), "the inclusive window boundary must count");
            }
        }
        assert!(
            refresh_at(&root, "epic-risk", lifted_at + Duration::hours(100))
                .unwrap()
                .is_some(),
            "only the operator's reasoned lift clears a latched pause"
        );
    }
}
