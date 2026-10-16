//! Merge idle prompts require settled CI and live target evidence.
use super::{DirectorData, DirectorEvent};
use crate::ui::factory::director::{
    MergeAlertEvidence, MergeAlertFreshness, check_merge_alert_freshness,
};
use std::path::Path;

pub(super) struct Ready {
    pub evidence: MergeAlertEvidence,
    pub tip: String,
    pub ci: &'static str,
}

pub(super) fn task_id(event: &DirectorEvent) -> Option<&str> {
    match event {
        DirectorEvent::WorkerIdle {
            active_task: Some(task),
            ..
        } if task.task_status == cas_types::TaskStatus::AwaitingMerge => Some(&task.task_id),
        _ => None,
    }
}

/// Runs off the loop. No result means defer; it never consumes the dedupe key.
pub(super) fn read(event: &DirectorEvent, data: &DirectorData, repo: &Path) -> Option<Ready> {
    let MergeAlertFreshness::Fresh(evidence) = check_merge_alert_freshness(event, data, repo)
    else {
        return None;
    };
    let tip = evidence.factory_tip.clone();
    let ci = crate::mcp::tools::core::workflow::delivery_tip_ci_conclusion(&tip, repo)?;
    // Never attach another tip's CI if the worker pushed during the probe.
    let current = crate::git_evidence::resolve_ref_commit_sha(repo, &evidence.factory_branch)
        .or_else(|| {
            crate::git_evidence::resolve_ref_commit_sha(
                repo,
                &format!("origin/{}", evidence.factory_branch),
            )
        })?;
    (tip == current).then_some(Ready { evidence, tip, ci })
}
