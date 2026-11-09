//! Ownership evidence for the supervisor's assign-ready stall action (cas-7806).
//! Reads run on the external lane; the loop only gates the snapshot in memory.
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;

use crate::cloud::peer_claims::{PeerBudget, PeerClaims};
use crate::ui::factory::director::DirectorData;

#[derive(Debug, Clone)]
pub(super) struct AssignmentRequest {
    cas_dir: PathBuf,
    task_ids: HashSet<String>,
    blocked_tasks: Vec<(String, String)>,
    session: String,
}

#[derive(Debug, Clone, Default)]
pub(super) struct AssignmentEvidence {
    checked: HashSet<String>,
    peer_owned: HashSet<String>,
    blocked_workers: HashSet<String>,
}

pub(super) fn has_live_operator_ask(
    data: &DirectorData,
    session: &str,
    supervisor_name: &str,
    now: chrono::DateTime<Utc>,
) -> bool {
    let supervisor_ids: HashSet<_> = data
        .agents
        .iter()
        .filter(|agent| agent.name == supervisor_name)
        .map(|agent| agent.id.as_str())
        .collect();
    data.reminders.iter().any(|reminder| {
        reminder.status == cas_store::ReminderStatus::Pending
            && reminder.trigger_type == cas_store::ReminderTriggerType::Time
            && reminder.trigger_at.is_some_and(|deadline| deadline > now)
            && reminder
                .trigger_filter
                .as_ref()
                .is_some_and(|filter| filter.get("operator_ask").is_some())
            && reminder
                .session_id
                .as_deref()
                .is_none_or(|id| id == session)
            && (reminder.target_id == supervisor_name
                || supervisor_ids.contains(reminder.target_id.as_str()))
    })
}

impl AssignmentRequest {
    pub(super) fn new(
        cas_dir: PathBuf,
        data: &DirectorData,
        session: &str,
        supervisor_name: &str,
    ) -> Option<Self> {
        let task_ids: HashSet<_> = data
            .ready_tasks
            .iter()
            .filter(|task| {
                task.status == cas_types::TaskStatus::Open
                    && task.assignee.is_none()
                    && !data.start_gated_task_ids.contains(&task.id)
            })
            .map(|task| task.id.clone())
            .collect();
        let now = Utc::now();
        let has_live_operator_ask = has_live_operator_ask(data, session, supervisor_name, now);
        let blocked_tasks = if has_live_operator_ask {
            data.ready_tasks
                .iter()
                .filter(|task| task.status == cas_types::TaskStatus::Blocked)
                .filter_map(|task| {
                    task.assignee
                        .as_ref()
                        .map(|assignee| (task.id.clone(), assignee.clone()))
                })
                .collect()
        } else {
            Vec::new()
        };
        (!task_ids.is_empty()).then_some(Self {
            cas_dir,
            task_ids,
            blocked_tasks,
            session: session.to_string(),
        })
    }

    /// No mutations or claims. Cloud checks share one bounded budget; a Cloud
    /// outage keeps start's existing advisory/fail-open policy. Unreadable local
    /// ownership evidence remains pending rather than inventing assignments.
    pub(super) fn read(&self) -> AssignmentEvidence {
        let (Ok(splits), Ok(agents), Ok(tasks)) = (
            crate::cloud::epic_split::current_epic_splits(&self.cas_dir),
            crate::store::open_agent_store(&self.cas_dir),
            crate::store::open_task_store_local(&self.cas_dir),
        ) else {
            return AssignmentEvidence::default();
        };
        let Ok(local) = agents.list(None) else {
            return AssignmentEvidence::default();
        };
        let local_ids: HashSet<String> = local.iter().map(|agent| agent.id.clone()).collect();
        let now = Utc::now();
        let mut peer_owned: HashSet<_> = splits
            .into_iter()
            .filter(|split| split.is_binding(now))
            .flat_map(|split| split.theirs)
            .collect();
        if let Some(claims) = PeerClaims::for_project(&self.cas_dir) {
            let claims = claims.with_budget(PeerBudget::new(Duration::from_secs(2)));
            // Stable order avoids HashSet iteration deciding which reads fit in the budget.
            let mut tasks: Vec<_> = self
                .task_ids
                .iter()
                .filter(|id| !peer_owned.contains(*id))
                .collect();
            tasks.sort();
            for id in tasks {
                if claims
                    .peer_hold(id, &|holder| local_ids.contains(holder))
                    .is_ok_and(|hold| hold.is_some())
                {
                    peer_owned.insert(id.clone());
                }
            }
        }
        let checked = self
            .task_ids
            .iter()
            .filter(|id| {
                tasks.get(id).is_ok_and(|task| {
                    task.status == cas_types::TaskStatus::Open
                        && task.assignee.is_none()
                        && task.task_type != cas_types::TaskType::Gate
                })
            })
            .cloned()
            .collect();
        let mut blocked_workers = HashSet::new();
        for (task_id, assignee) in &self.blocked_tasks {
            let Ok(task) = tasks.get(task_id) else { continue };
            if task.status != cas_types::TaskStatus::Blocked {
                continue;
            }
            let reason = crate::mcp::tools::core::task::block_hold::latest_blocker_reason(
                &task.notes,
            )
            .or_else(|| {
                crate::mcp::tools::core::task::block_hold::read(tasks.as_ref(), task_id)
                    .ok()
                    .flatten()
                    .map(|hold| hold.reason)
            });
            if !reason.as_deref().is_some_and(|reason| {
                crate::mcp::tools::core::task::block_hold::reason_has_named_owner(reason)
            }) {
                continue;
            }
            blocked_workers.insert(assignee.clone());
            let Some(agent) = local.iter().find(|agent| {
                agent.role == cas_types::AgentRole::Worker
                    && agent.factory_session.as_deref() == Some(self.session.as_str())
                    && (agent.name == *assignee || agent.id == *assignee)
            }) else {
                continue;
            };
            blocked_workers.insert(agent.name.clone());
            blocked_workers.insert(agent.id.clone());
        }
        AssignmentEvidence {
            checked,
            peer_owned,
            blocked_workers,
        }
    }
}

impl AssignmentEvidence {
    pub(super) fn allows(&self, task_id: &str) -> bool {
        // Missing observations remain pending. Peer children stay in the
        // snapshot, so they still prevent a premature assemble action.
        self.checked.contains(task_id) && !self.peer_owned.contains(task_id)
    }

    pub(super) fn worker_waiting_on_other(&self, worker: &str) -> bool {
        self.blocked_workers.contains(worker)
    }
}
