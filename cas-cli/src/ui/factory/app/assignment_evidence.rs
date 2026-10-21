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
}

#[derive(Debug, Clone, Default)]
pub(super) struct AssignmentEvidence {
    checked: HashSet<String>,
    peer_owned: HashSet<String>,
}

impl AssignmentRequest {
    pub(super) fn new(cas_dir: PathBuf, data: &DirectorData) -> Option<Self> {
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
        (!task_ids.is_empty()).then_some(Self { cas_dir, task_ids })
    }

    /// No mutations or claims. Cloud checks share one bounded budget; a Cloud
    /// outage keeps start's existing advisory/fail-open policy. Unreadable local
    /// ownership evidence remains pending rather than inventing assignments.
    pub(super) fn read(&self) -> AssignmentEvidence {
        let (Ok(splits), Ok(agents)) = (
            crate::cloud::epic_split::current_epic_splits(&self.cas_dir),
            crate::store::open_agent_store(&self.cas_dir),
        ) else {
            return AssignmentEvidence::default();
        };
        let Ok(local) = agents.list(None) else {
            return AssignmentEvidence::default();
        };
        let local: HashSet<_> = local.into_iter().map(|agent| agent.id).collect();
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
                    .peer_hold(id, &|holder| local.contains(holder))
                    .is_ok_and(|hold| hold.is_some())
                {
                    peer_owned.insert(id.clone());
                }
            }
        }
        AssignmentEvidence {
            checked: self.task_ids.clone(),
            peer_owned,
        }
    }
}

impl AssignmentEvidence {
    pub(super) fn allows(&self, task_id: &str) -> bool {
        // Missing observations remain pending. Peer children stay in the
        // snapshot, so they still prevent a premature assemble action.
        self.checked.contains(task_id) && !self.peer_owned.contains(task_id)
    }
}
