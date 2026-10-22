//! Cassy closes a parked delivery that it merged and published.
//!
//! Before this, a merged delivery waited for its worker to re-close, and a
//! verdict that approved it waited for a second worker re-close. Both turns
//! restated facts Cassy already held: the parked close reason and the merge.
//! This removes the handoff, not a gate: the close below is the ordinary
//! close, with every QA, verification, proof and merge gate; only the
//! supervisor's verification skip is withheld (see `as_cassy_merge_close`).

use crate::mcp::tools::core::imports::*;

use super::close_ops::{ALREADY_CLOSED_PREFIX, as_cassy_merge_close};

/// What Cassy's own post-merge close did.
pub(crate) enum CassyMergeClose {
    /// The task does not carry a published merge Cassy may close for.
    NotEligible,
    /// This call closed the task; `notice` reports the worker notification.
    Closed { notice: String },
    /// Someone else (normally the worker) closed it first.
    AlreadyClosed,
    /// A gate still holds the task open; its text is the next action.
    Held(String),
}

impl CassyMergeClose {
    /// One receipt line for the caller's response.
    /// cas-3fc8: what the post-commit note says this close concluded.
    pub(crate) fn followup(&self, task_id: &str) -> String {
        match self {
            Self::NotEligible => "not eligible for Cassy's close".to_string(),
            other => other.receipt(task_id),
        }
    }

    pub(crate) fn receipt(&self, task_id: &str) -> String {
        match self {
            Self::NotEligible => String::new(),
            Self::Closed { notice } => {
                format!("\nCassy closed task {task_id} after the published merge; {notice}.")
            }
            Self::AlreadyClosed => format!("\nTask {task_id} was already closed."),
            Self::Held(text) => format!("\nCassy close of {task_id} is waiting on a gate:\n{text}"),
        }
    }
}

impl CasCore {
    /// Record that `worktree_merge` published this parked delivery, then close
    /// it as Cassy. The legacy green_merge_commit field records the merge tip
    /// for both confirmed and advisory CI; the task note preserves that distinction.
    pub(crate) async fn close_after_published_merge(
        &self,
        task_id: &str,
        merge_commit: &str,
        ci_confirmed: bool,
    ) -> CassyMergeClose {
        let Ok(store) = self.open_task_store() else {
            return CassyMergeClose::NotEligible;
        };
        // One conditional field write, never a read-modify-write of the row:
        // a worker close landing first must stay closed.
        match store.record_green_merge(task_id, merge_commit) {
            Ok(true) => {
                if !ci_confirmed {
                    let note = format!(
                        "CI unconfirmed at merge {merge_commit}; worktree_merge proceeded under advisory CI policy."
                    );
                    if let Err(error) = store.append_trusted_note(task_id, &note) {
                        return CassyMergeClose::Held(format!(
                            "the advisory CI close note could not be recorded ({error}); retry task close."
                        ));
                    }
                }
                self.close_green_merged_delivery(task_id).await
            }
            Ok(false) => match store.get(task_id) {
                Ok(task) if task.status == TaskStatus::Closed => CassyMergeClose::AlreadyClosed,
                _ => CassyMergeClose::NotEligible,
            },
            Err(error) => CassyMergeClose::Held(format!(
                "the published merge could not be recorded ({error}); retry task close."
            )),
        }
    }

    /// Close a delivery that carries a recorded green merge, with the parked
    /// close reason and the task's own delivered commit as receipt.
    pub(crate) async fn close_green_merged_delivery(&self, task_id: &str) -> CassyMergeClose {
        let Ok(store) = self.open_task_store() else {
            return CassyMergeClose::NotEligible;
        };
        let Ok(task) = store.get(task_id) else {
            return CassyMergeClose::NotEligible;
        };
        if task.status == TaskStatus::Closed {
            return CassyMergeClose::AlreadyClosed;
        }
        let Some(merge_commit) = task.deliverables.green_merge_commit.clone() else {
            return CassyMergeClose::NotEligible;
        };
        let reason = task.deliverables.parked_close_reason.clone().unwrap_or_else(|| {
            format!("Merged at {merge_commit}; closed by Cassy after the merge.")
        });
        let request = TaskCloseRequest {
            no_delivery: None,
            stranded_branch_override: None,
            id: task_id.to_string(),
            reason: Some(reason),
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: task.deliverables.factory_branch_anchor.clone(),
        };
        let result = match as_cassy_merge_close(self.cas_task_close(Parameters(request))).await {
            Ok(result) => result,
            Err(error) => return CassyMergeClose::Held(error.message.to_string()),
        };
        let text = result
            .content
            .iter()
            .filter_map(|content| match &content.raw {
                rmcp::model::RawContent::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if text.starts_with(ALREADY_CLOSED_PREFIX) {
            return CassyMergeClose::AlreadyClosed;
        }
        let Ok(closed) = store.get(task_id) else {
            return CassyMergeClose::Held(text);
        };
        if closed.status != TaskStatus::Closed {
            return CassyMergeClose::Held(text);
        }
        let notice = match super::supervisor_push::queue_cassy_merge_close_notice(
            &self.cas_root,
            &closed,
            &merge_commit,
        ) {
            Ok(Some(prompt_id)) => format!("worker notified (prompt {prompt_id})"),
            Ok(None) => "no live assigned worker to notify".to_string(),
            Err(error) => format!("worker notice failed: {error}"),
        };
        CassyMergeClose::Closed { notice }
    }
}
