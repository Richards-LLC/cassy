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

/// Read-only admission shared by receipt close and release backfill preview.
/// `Some` retains the task's own ordinary review path; `None` lets the release
/// verdict authorize it. Neither result bypasses the remaining close gates.
pub(crate) fn release_receipt_close_admission(
    root: &std::path::Path,
    task: &Task,
    repo: &std::path::Path,
) -> Result<Option<String>, String> {
    if task.deliverables.effective_merge_ci_admission() == cas_types::MergeCiAdmission::Override {
        return Err("CI_OVERRIDE: merged under a CI override; it needs its own close".into());
    }
    if task.deliverables.factory_branch_anchor.is_none()
        && !task.deliverables.historical_factory_branch_anchors.is_empty()
    {
        return Err("REQUEST_CHANGES: request_changes moved the task out of its park; close the retired delivery with task close commit_receipt=<merged sha>".into());
    }
    if task.status != TaskStatus::AwaitingMerge && task.deliverables.green_merge_commit.is_none() {
        return Err("NOT_PARKED: not parked; it needs its own close".into());
    }
    Ok(super::review_tier::own_review(root, task, repo))
}

/// The delivery a `worktree_merge` just landed: the repository, the source
/// branch it merged and the exact source commit it merged.
pub(crate) struct MergedLane<'a> {
    pub repo: &'a std::path::Path,
    pub branch: &'a str,
    pub tip: &'a str,
}

/// A reused worker lane is not task identity. The recorded start excludes
/// preceding work, including a prior task's tip at exactly that boundary.
/// Legacy tasks need a durable task anchor or an explicitly named commit.
pub(crate) fn delivery_tip_belongs_to_task(
    repo: &std::path::Path,
    task: &Task,
    branch: &str,
    tip: &str,
) -> bool {
    let branch = branch.strip_prefix("origin/").unwrap_or(branch);
    // Older release paths retained the preceding holder's start. Like close
    // attribution, ignore an unusable binding and require explicit task commit
    // identity below; a stale record cannot veto the new holder's own work.
    if let Some(start) = task.deliverables.factory_branch_start.as_ref().filter(|start| {
        task.assignee.as_deref() == Some(start.assignee.as_str())
            && task.deliverables.work_target.as_ref() == Some(&start.work_target)
            && branch == start.branch
    }) {
        return tip != start.base_sha
            && super::close_ops::git_commit_is_ancestor(repo, &start.base_sha, tip);
    }
    let identity = super::close_ops::task_commit_identity(task, None);
    super::close_ops::commit_is_task_attributable(repo, tip, &identity)
        || identity.known_commits.iter().any(|anchor| {
            super::close_ops::git_commit_is_ancestor(repo, anchor, tip)
        })
}

/// cas-46ea: the one definition of a pushed delivery tip, shared with
/// merge-on-green (cas-20d3). `tip` qualifies when it is on the assignee's own
/// delivery branch for this task and origin already holds it, so the merge
/// carries the worker's pushed work rather than a supervisor-authored commit.
///
/// cas-e9d6: factory agents share one Git author identity, so authorship
/// cannot tell a worker's commit from a supervisor's. The worker's own local
/// branch can: only the worker commits to `refs/heads/<branch>`, while a
/// commit pushed straight to origin by anyone else exists only on the
/// remote-tracking ref. The tip must be on both.
pub(crate) fn pushed_delivery_tip(
    repo: &std::path::Path,
    task: &Task,
    branch: &str,
    tip: &str,
) -> Result<(), String> {
    if task.task_type == TaskType::Epic {
        return Err("an epic closes through epic verification".to_string());
    }
    let Some(worker) = task.assignee.as_deref() else {
        return Err("the task has no assignee".to_string());
    };
    let branch = branch.strip_prefix("origin/").unwrap_or(branch);
    if !crate::mcp::tools::core::workflow::task_delivery_branch_matches(
        branch, worker, &task.id,
    ) {
        return Err(format!("{branch} is not {worker}'s delivery branch for {}", task.id));
    }
    let Some(pushed) = super::close_ops::resolve_branch_sha(repo, &format!("refs/remotes/origin/{branch}")) else {
        return Err(format!("cached origin/{branch} does not resolve; live publication is unknown"));
    };
    if pushed != tip && !super::close_ops::git_commit_is_ancestor(repo, tip, &pushed) {
        return Err(format!("{tip} is absent from cached origin/{branch}; live publication is unknown"));
    }
    let local = super::close_ops::resolve_branch_sha(repo, &format!("refs/heads/{branch}"));
    if !local.is_some_and(|local| local == tip || super::close_ops::git_commit_is_ancestor(repo, tip, &local)) {
        return Err(format!(
            "{tip} is on cached origin/{branch} but not on {worker}'s local {branch}; this does not establish the worker's own delivery"
        ));
    }
    Ok(())
}

/// What a verdict close lifted before closing, to record after the close
/// commits or to restore when it is held (cas-e9d6).
struct VerdictRelease {
    note: String,
    foreign_hold: Option<String>,
    was_blocked: bool,
    resumed_status: TaskStatus,
    hold: Option<serde_json::Value>,
}

/// What Cassy's own post-merge close did.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
            // cas-46ea: the verification gate is Cassy's own next hop, not a
            // worker turn; say so where the supervisor reads the merge.
            Self::Held(text) if text.contains("VERIFICATION REQUIRED") => format!(
                "\nCassy close of {task_id} is waiting on its verification dispatch. Run the verifier on it; \
                 a PASS closes {task_id} and a FAIL returns it to the worker. Do not message the worker.\n{text}"
            ),
            Self::Held(text) => format!("\nCassy close of {task_id} is waiting on a gate:\n{text}"),
        }
    }
}

/// One line of a gate's text for a report naming many deliveries.
fn summarize_gate(text: &str) -> String {
    const LIMIT: usize = 240;
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= LIMIT {
        return line;
    }
    format!("{}…", line.chars().take(LIMIT).collect::<String>())
}

fn record_batch_close_outcome(
    cas_root: &std::path::Path,
    store: &dyn cas_store::TaskStore,
    task_id: &str,
    receipt_id: &str,
    recorded_at: &str,
    outcome: &str,
) -> Result<String, String> {
    let summary = summarize_gate(outcome);
    let body = if summary == outcome {
        summary
    } else {
        // Preserve complete commands and original whitespace in the existing
        // immutable receipt channel; never truncate its detail pointer.
        let config = crate::config::Config::load(cas_root).map_err(|error| error.to_string())?;
        let brief: String = summary.chars().take(80).collect();
        super::close_ops::gate_text::compact_receipt(
            cas_root, config.factory().artifacts_root.as_deref(), task_id,
            "batch-close", &brief, outcome,
        )?.replace('\n', " ")
    };
    // The immutable receipt's timestamp makes a replay byte-identical. The
    // store's trusted append deduplicates atomically; neutralize gate text as
    // the ordinary append did before giving it that internal authority.
    let note = format!("[{recorded_at}] 📝 PROGRESS BATCH_CLOSE_OUTCOME {receipt_id}: {body}");
    let note = cas_store::task_note_markers::neutralize_new_markers(&note, None);
    store.append_trusted_note(task_id, &note).map_err(|error| error.to_string())?;
    Ok(note.into_owned())
}

/// The shared, delivery-bound return path for synchronous close and durable QA decisions.
pub(crate) fn return_merged_delivery_for_source_fix(
    cas_root: &std::path::Path, actor: &str, task: &Task,
    repo: &std::path::Path, target: &str, gate: &str,
) -> Result<String, String> {
    let (Some(anchor), Some(merge), Some(branch)) = (
        task.deliverables.factory_branch_anchor.as_deref(),
        task.deliverables.green_merge_commit.as_deref(),
        task.deliverables.parked_branch.as_deref(),
    ) else { return Ok(String::new()); };
    if task.status != TaskStatus::AwaitingMerge || task.assignee.is_none() {
        return Ok(String::new());
    }
    let target = target.strip_prefix("origin/").unwrap_or(target);
    let target_tip = super::close_ops::resolve_branch_sha(repo, &format!("origin/{target}"))
        .or_else(|| super::close_ops::resolve_branch_sha(repo, target));
    let Some(target_tip) = target_tip.filter(|tip| {
        super::close_ops::git_commit_is_ancestor(repo, anchor, tip)
            && super::close_ops::git_commit_is_ancestor(repo, merge, tip)
    }) else { return Ok(String::new()); };
    let reason = format!("post_merge_close_gate: delivery {anchor} landed at {merge} (target {target_tip}); source correction required.\n{gate}");
    (|| -> Result<String, String> {
        let store = crate::store::open_task_store(cas_root).map_err(|error| error.to_string())?;
        let current = store.get(&task.id).map_err(|error| error.to_string())?;
        if current.status != TaskStatus::AwaitingMerge
            || current.assignee != task.assignee
            || current.deliverables.factory_branch_anchor != task.deliverables.factory_branch_anchor
            || current.deliverables.green_merge_commit != task.deliverables.green_merge_commit
            || current.deliverables.parked_branch != task.deliverables.parked_branch
            || current.deliverables.work_target != task.deliverables.work_target
        {
            return Ok("delivery changed during close; no task transition attempted".into());
        }
        // Retire the old auto-close intent before the worker may start a
        // correction, so recovery cannot re-park the already declined tip.
        cas_store::clear_pending_observed_delivery_merge(cas_root, &task.id, branch, target, anchor)
            .map_err(|error| error.to_string())?;
        cas_store::request_changes_for_parked_delivery(cas_root, &task.id, actor, &reason)
            .map_err(|error| error.to_string())?;
        let returned = store.get(&task.id).map_err(|error| error.to_string())?;
        let notice = match super::supervisor_push::queue_source_fix_reopen_notice(
            cas_root, &returned, anchor, merge, &reason,
        ) {
            Ok(Some(id)) => format!("worker notified (prompt {id})"),
            Ok(None) => "no live assigned worker to notify".into(),
            Err(error) => format!("worker notice failed: {error}"),
        };
        Ok(format!("Cassy returned task {} Open to its assignee for the source fix; {notice}. Start task {} and re-deliver after fixing the gate; no supervisor request_changes is needed.", task.id, task.id))
    })()
}

impl CasCore {
    /// Source-upkeep refusals opt in here at their typed call sites. Only a
    /// published, still-current parked delivery may be returned for a fix;
    /// verification/QA waits and unreadable evidence keep their own recovery.
    pub(super) fn return_merged_delivery_for_source_fix(
        &self, task: &Task, repo: &std::path::Path, target: &str, gate: &str,
    ) -> String {
        let actor = self.get_agent_id().unwrap_or_else(|_| "cassy".into());
        match return_merged_delivery_for_source_fix(&self.cas_root, &actor, task, repo, target, gate) {
            Ok(detail) if detail.is_empty() => String::new(),
            Ok(detail) => format!("\n\n{detail}"),
            Err(error) => format!("\n\nCassy could not return task {} for its source fix ({error}); run task action=request_changes id={}.", task.id, task.id),
        }
    }

    /// Record that `worktree_merge` published this parked delivery, then close
    /// it as Cassy. The legacy green_merge_commit field records the merge tip
    /// with a separate CI admission state, including explicit override merges.
    pub(crate) async fn close_after_published_merge(
        &self,
        task_id: &str,
        merge_commit: &str,
        admission: cas_types::MergeCiAdmission,
        lane: Option<MergedLane<'_>>,
    ) -> CassyMergeClose {
        let Ok(store) = self.open_task_store() else {
            return CassyMergeClose::NotEligible;
        };
        if admission != cas_types::MergeCiAdmission::Override
            && let Some(lane) = lane
            && let Err(error) = self.park_merged_lane(store.as_ref(), task_id, merge_commit, &lane)
        {
            return CassyMergeClose::Held(error);
        }
        // One conditional field write, never a read-modify-write of the row:
        // a worker close landing first must stay closed.
        match store.record_merge_admission(task_id, merge_commit, admission) {
            Ok(true) => {
                if admission == cas_types::MergeCiAdmission::Override {
                    // An override remains a manual close, including after a
                    // verdict arrives. Recording evidence grants no close authority.
                    return CassyMergeClose::NotEligible;
                }
                if admission == cas_types::MergeCiAdmission::Advisory {
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

    /// cas-46ea: a supervisor may merge a pushed lane tip before its worker
    /// parks (merge on green). Record the park the worker would have made, at
    /// the exact merged tip, so the post-merge close below applies unchanged.
    /// A task that is not InProgress/Blocked, or whose merged commit is not
    /// its assignee's pushed delivery tip, is left as it was.
    fn park_merged_lane(
        &self,
        store: &dyn cas_store::TaskStore,
        task_id: &str,
        merge_commit: &str,
        lane: &MergedLane<'_>,
    ) -> Result<(), String> {
        let Ok(task) = store.get(task_id) else {
            return Ok(());
        };
        if !matches!(task.status, TaskStatus::InProgress | TaskStatus::Blocked)
            || task.deliverables.merge_conflicted
            || pushed_delivery_tip(lane.repo, &task, lane.branch, lane.tip).is_err()
            || !super::close_ops::git_commit_is_ancestor(lane.repo, lane.tip, merge_commit)
        {
            return Ok(());
        }
        let mut parked = task.clone();
        parked.status = TaskStatus::AwaitingMerge;
        if parked.deliverables.factory_branch_anchor.as_deref() != Some(lane.tip) {
            parked.deliverables.retain_factory_branch_anchor_as_history();
            parked.deliverables.factory_branch_anchor = Some(lane.tip.to_string());
        }
        parked.deliverables.parked_branch =
            Some(lane.branch.strip_prefix("origin/").unwrap_or(lane.branch).to_string());
        parked.pending_verification = false;
        parked.pending_worktree_merge = false;
        let note = format!(
            "[{}] Cassy parked this task at its delivery tip {} on {} (checked against cached origin refs) after merge {merge_commit}; no worker re-close is needed.",
            chrono::Utc::now().format("%Y-%m-%d %H:%M"),
            lane.tip,
            lane.branch,
        );
        parked.notes = if parked.notes.is_empty() { note } else { format!("{}\n\n{note}", parked.notes) };
        parked.updated_at = chrono::Utc::now();
        store
            .update_if_current(&parked, &task)
            .map_err(|error| format!("the merged lane could not be parked ({error}); retry task close."))?;
        if let Ok(agents) = self.open_agent_store()
            && let Err(error) = agents.release_lease_for_task(task_id, "Merged by supervisor: parked by Cassy")
        {
            tracing::warn!(%task_id, %error, "lease release after Cassy lane park failed");
        }
        // cas-e9d6: the worker just lost its lease; tell it why, once.
        if let Err(error) = super::supervisor_push::queue_merged_lane_parked_notice(
            &self.cas_root,
            &parked,
            lane.tip,
            merge_commit,
        ) {
            tracing::warn!(%task_id, %error, "merged-lane park notice not queued");
        }
        Ok(())
    }

    /// Resume only the explicit close saved with this proof cycle. The ordinary
    /// close remeasures absence of delivery and enforces every existing gate.
    pub(crate) async fn close_no_delivery_after_verdict(
        &self, task_id: &str, dispatch_id: &str,
    ) -> Result<Option<String>, McpError> {
        let store = self.open_task_store()?;
        let task = store.get(task_id).map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        let Some(intent) = task.deliverables.no_delivery_close_intent.as_ref() else { return Ok(None); };
        // A retired attempt cannot change the behavior of a newer ordinary
        // close. Only this dispatch owns the no-delivery declaration.
        if intent.dispatch_id != dispatch_id { return Ok(None); }
        // A saved intent never falls back to a forbidden worker retry, even
        // when a successor or independent hold now prevents its completion.
        let held = |reason: &str| Some(format!("\nCassy no-delivery close of {task_id} is held: {reason}. No worker close retry was queued."));
        let latest = cas_store::get_latest_verification_dispatch(&self.cas_root, task_id)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        if intent.assignee != task.assignee
            || latest.as_ref().is_none_or(|dispatch| dispatch.id != dispatch_id
                || dispatch.state != cas_types::VerificationDispatchState::Resolved)
        {
            return Ok(held("saved close intent no longer matches the current assignment and exact verdict"));
        }
        if task.status == TaskStatus::Blocked
            && !super::super::block_hold::blocked_only_by_verification_wait(&self.cas_root, task_id, dispatch_id)
        {
            return Ok(held("an independent blocker remains"));
        }
        if !matches!(task.status, TaskStatus::Blocked | TaskStatus::InProgress | TaskStatus::Open) {
            return Ok(held("task left its no-delivery close cycle"));
        }
        let hold = super::super::block_hold::read(store.as_ref(), task_id)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error))?;
        if task.status != TaskStatus::Blocked && hold.is_some() {
            return Ok(held("an independent hold remains"));
        }
        let release = VerdictRelease {
            note: format!("[{}] Cassy close authority: explicit no_delivery intent + verdict dispatch {dispatch_id}.", chrono::Utc::now().format("%Y-%m-%d %H:%M")),
            foreign_hold: None,
            was_blocked: task.status == TaskStatus::Blocked,
            resumed_status: TaskStatus::InProgress,
            hold: hold.as_ref().and_then(|hold| serde_json::to_value(hold).ok()),
        };
        if release.was_blocked {
            // Reuse the existing restoration path for a failed verdict close.
            let mut released = task.clone();
            released.status = release.resumed_status;
            store.update_if_current(&released, &task)
                .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        }
        if release.hold.is_some() {
            if let Err(error) = store.patch_execution_state(task_id, &serde_json::json!({"blocked_hold": null})) {
                return Ok(Some(self.held_after_verdict(store.as_ref(), task_id, Some(release), error.to_string()).receipt(task_id)));
            }
        }
        let request = TaskCloseRequest {
            id: task_id.to_string(), reason: Some(intent.reason.clone()), no_delivery: Some(true),
            stranded_branch_override: None, supervisor_override: None, search_manifest: None, commit_receipt: None,
        };
        let close = crate::async_runtime::heap_future(|| self.cas_task_close(Parameters(request)));
        let result = as_cassy_merge_close(close).await;
        let text = match result {
            Ok(result) => result.content.iter().filter_map(|content| match &content.raw {
                rmcp::model::RawContent::Text(text) => Some(text.text.as_str()), _ => None,
            }).collect::<Vec<_>>().join("\n"),
            Err(error) => error.message.to_string(),
        };
        let current = store.get(task_id).map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        if current.status != TaskStatus::Closed {
            let outcome = self.held_after_verdict(store.as_ref(), task_id, Some(release), text);
            return Ok(Some(format!("{}\nNo worker close retry was queued.", outcome.receipt(task_id))));
        }
        let unrecorded = self.record_verdict_release(store.as_ref(), &current, &release);
        let notice = super::supervisor_push::queue_no_delivery_close_notice(&self.cas_root, &current, dispatch_id)
            .map(|id| id.map(|id| format!("worker notified (prompt {id})")).unwrap_or_else(|| "no live assigned worker to notify".into()))
            .unwrap_or_else(|error| format!("worker notice failed: {error}"));
        Ok(Some(format!("\nCassy closed task {task_id} on its approved no-delivery intent; {notice}.{}",
            if unrecorded.is_empty() { String::new() } else { format!(" NOT RECORDED: {}", unrecorded.join("; ")) })))
    }

    /// Close a delivery that carries a recorded green merge, with the parked
    /// close reason and the task's own delivered commit as receipt.
    pub(crate) async fn close_green_merged_delivery(&self, task_id: &str) -> CassyMergeClose {
        self.close_green_merged_delivery_after_verdict(task_id, None).await
    }

    /// cas-46ea: as above, after `verification_id` approved the merged
    /// delivery. A worker hold no longer stops the close: a hold set while
    /// the verification ran was waiting on this verdict; any other hold is
    /// recorded on the task and reported to the supervisor once.
    pub(crate) async fn close_green_merged_delivery_after_verdict(
        &self,
        task_id: &str,
        verification_id: Option<&str>,
    ) -> CassyMergeClose {
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
        if task.deliverables.effective_merge_ci_admission() == cas_types::MergeCiAdmission::Override {
            return CassyMergeClose::NotEligible;
        }
        self.close_merged_delivery(store.as_ref(), task, &merge_commit, verification_id, None).await
    }

    /// The ordinary close of a merged delivery, with `merge_commit` as its
    /// merge evidence and `verification_id` as the verdict that authorizes it.
    async fn close_merged_delivery(
        &self,
        store: &dyn cas_store::TaskStore,
        mut task: Task,
        merge_commit: &str,
        verification_id: Option<&str>,
        release_receipt: Option<&cas_store::QaBatchReceipt>,
    ) -> CassyMergeClose {
        let task_id = task.id.clone();
        let task_id = task_id.as_str();
        let release = match verification_id {
            Some(verification_id) => {
                match self.release_hold_for_verdict(store, &task, merge_commit, verification_id) {
                    Ok((released, release)) => {
                        task = released;
                        Some(release)
                    }
                    Err(error) => return CassyMergeClose::Held(error),
                }
            }
            None => None,
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
            search_manifest: None,
            commit_receipt: task.deliverables.factory_branch_anchor.clone().or_else(|| {
                // GH #1641: a PR merge recorded no anchor; close on the resolved delivery.
                let (repo, target) = crate::qa_batch::task_context(&self.cas_root, &task).ok()?;
                crate::qa_batch::resolved_delivery_anchor(&repo, &task, &target)
            }),
        };
        let fixed_release = if crate::qa_batch::release_close::target(&self.cas_root, task_id).is_none() {
            release_receipt.and_then(|receipt| {
                let (repo, _) = crate::qa_batch::task_context(&self.cas_root, &task).ok()?;
                Some(crate::qa_batch::release_close::ReleaseClose::for_receipt(&self.cas_root, &repo, &task, receipt))
            })
        } else { None };
        let close = crate::async_runtime::heap_future(|| self.cas_task_close(Parameters(request)));
        let close = async {
            match fixed_release {
                Some(context) => context.scope(close).await,
                None => close.await,
            }
        };
        let result = match as_cassy_merge_close(close).await {
            Ok(result) => result,
            Err(error) => {
                let text = error.message.to_string();
                return self.held_after_verdict(store, task_id, release, text);
            }
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
            return self.held_after_verdict(store, task_id, release, text);
        };
        if closed.status != TaskStatus::Closed {
            return self.held_after_verdict(store, task_id, release, text);
        }
        let unrecorded = release.as_ref()
            .map(|release| self.record_verdict_release(store, &closed, release))
            .unwrap_or_default();
        let mut notice = if crate::release_reconcile::active() {
            "completion included in release receipt".to_string()
        } else { match super::supervisor_push::queue_cassy_merge_close_notice(
            &self.cas_root,
            &closed,
            merge_commit,
        ) {
            Ok(Some(prompt_id)) => format!("worker notified (prompt {prompt_id})"),
            Ok(None) => "no live assigned worker to notify".to_string(),
            Err(error) => format!("worker notice failed: {error}"),
        } };
        // cas-43d2: the supervisor reads this receipt; a record that failed
        // after the close committed belongs in it, not only in a warn log.
        for failure in unrecorded {
            notice.push_str(&format!("; NOT RECORDED: {failure}"));
        }
        CassyMergeClose::Closed { notice }
    }

    /// Close one receipt-covered delivery, retaining optional review authority.
    /// The receipt binds its anchor to staging on the target, which is the merge
    /// evidence for a delivery merged outside `worktree_merge`. An override merge
    /// stays a manual close, and work that has left its park is not closed.
    async fn close_after_batch_receipt(&self, task_id: &str, receipt: &cas_store::QaBatchReceipt, verification_id: Option<&str>) -> CassyMergeClose {
        let Ok(store) = self.open_task_store() else {
            return CassyMergeClose::NotEligible;
        };
        let Ok(task) = store.get(task_id) else {
            return CassyMergeClose::NotEligible;
        };
        let outcome = if task.deliverables.effective_merge_ci_admission() == cas_types::MergeCiAdmission::Override {
            CassyMergeClose::NotEligible
        } else if task.status == TaskStatus::AwaitingMerge || task.deliverables.green_merge_commit.is_some() {
            let merged = task.deliverables.green_merge_commit.clone().unwrap_or_else(|| receipt.staging_sha.clone());
            self.close_merged_delivery(store.as_ref(), task, &merged, verification_id, Some(receipt)).await
        } else {
            CassyMergeClose::NotEligible
        };
        let CassyMergeClose::Closed { mut notice } = outcome else {
            return outcome;
        };
        // A transactional delivery held at CloseReady completes with this close,
        // as `worktree_merge` completes it when its own close passes.
        if let Ok(Some((_, transaction))) = cas_store::get_latest_worker_delivery(&self.cas_root, task_id)
            && transaction.state == cas_types::WorkerDeliveryState::CloseReady
        {
            let actor = self.get_agent_id().unwrap_or_else(|_| "cassy".into());
            if let Err(error) = cas_store::transition_worker_delivery(
                &self.cas_root,
                &transaction.id,
                &[cas_types::WorkerDeliveryState::CloseReady],
                cas_types::WorkerDeliveryState::Delivered,
                &actor,
                Some(&actor),
                None,
                Some(&receipt.staging_sha),
                None,
            ) {
                notice.push_str(&format!("; NOT RECORDED: delivery completion ({error})"));
            }
        }
        CassyMergeClose::Closed { notice }
    }

    /// Trusted publish/promotion adapters pass the shared immutable inventory.
    /// This reuses receipt finalization and every ordinary close gate; a stale
    /// member is held while independent members continue.
    pub(crate) async fn close_release_inventory_outcomes(
        &self, inventory: &crate::qa_batch::ReleaseInventory,
    ) -> std::collections::BTreeMap<String, CassyMergeClose> {
        self.close_release_inventory_with_checkpoint(inventory, &mut |_, _| Ok(())).await
            .unwrap_or_else(|error| inventory.deliveries.iter().map(|d|
                (d.task_id.clone(), CassyMergeClose::Held(error.clone()))).collect())
    }

    /// The durable release controller commits each result before this loop
    /// proceeds. A failed checkpoint stops effects; the ordinary close remains
    /// replayable if it committed just before the process died.
    pub(crate) async fn close_release_inventory_with_checkpoint(
        &self,
        inventory: &crate::qa_batch::ReleaseInventory,
        checkpoint: &mut impl FnMut(&str, CassyMergeClose) -> Result<(), String>,
    ) -> Result<std::collections::BTreeMap<String, CassyMergeClose>, String> {
        use crate::qa_batch::{self, release_close::ReleaseClose};
        let mut outcomes: std::collections::BTreeMap<_, _> = inventory.exceptions.iter().map(|e|
            (e.task_id.clone(), CassyMergeClose::Held(format!("RELEASE_MEMBER_HELD: {:?}", e.code)))).collect();
        for id in inventory.scope.included_task_ids.iter().chain(inventory.scope.no_code_members.keys()) {
            if !inventory.deliveries.iter().any(|d| &d.task_id == id) {
                outcomes.entry(id.clone()).or_insert_with(|| CassyMergeClose::Held("RELEASE_MEMBERSHIP_MISSING".into()));
            }
        }
        for (id, outcome) in &outcomes { checkpoint(id, outcome.clone())?; }
        let repo = self.cas_root.parent().unwrap_or(&self.cas_root);
        let context = match ReleaseClose::prepare(&self.cas_root, repo, inventory) {
            Ok(context) => context,
            Err(error) => {
                for delivery in &inventory.deliveries {
                    let outcome = CassyMergeClose::Held(error.clone());
                    checkpoint(&delivery.task_id, outcome.clone())?;
                    outcomes.insert(delivery.task_id.clone(), outcome);
                }
                return Ok(outcomes);
            }
        };
        context.clone().scope(async {
            let store = self.open_task_store().map_err(|e| e.to_string())?;
            for delivery in &inventory.deliveries {
                let id = &delivery.task_id;
                let outcome = if store.get(id).is_ok_and(|task| task.status == TaskStatus::Closed) {
                    CassyMergeClose::AlreadyClosed
                } else if let Some(error) = context.member_error(id) {
                    CassyMergeClose::Held(error.into())
                } else {
                    let result = (|| {
                        let task = store.get(id).map_err(|e| e.to_string())?;
                        qa_batch::release_close::validate_task(&self.cas_root, &task)?;
                        let (repo, target) = qa_batch::task_context(&self.cas_root, &task)?;
                        let anchor = delivery.anchor.as_deref().ok_or("RELEASE_NO_CODE_REQUIRES_OWN_CLOSE")?;
                        qa_batch::satisfying_release_verdict(&self.cas_root, &task, &repo, &target, anchor)?
                            .ok_or_else(|| "RELEASE_REVIEW_MISSING: no bound independent release review".to_string())
                    })();
                    match result {
                        Ok(mut receipt) => {
                            receipt.task_heads.retain(|task, _| task == id);
                            self.close_release_covered_delivery_outcomes(&receipt).await.remove(id)
                                .unwrap_or(CassyMergeClose::NotEligible)
                        }
                        Err(error) => CassyMergeClose::Held(error),
                    }
                };
                checkpoint(id, outcome.clone())?;
                outcomes.insert(id.clone(), outcome);
            }
            Ok(outcomes)
        }).await
    }

    /// cas-f497: a recorded release verdict closes the deliveries it owns. A QA
    /// receipt retries ordinary close too: its existing review gates either
    /// finish the delivery or name the remaining verdict. QA never replaces
    /// release review or a dedicated task review. Returns the report for the receipt's response.
    pub(crate) async fn close_release_covered_delivery_outcomes(
        &self, receipt: &cas_store::QaBatchReceipt,
    ) -> std::collections::BTreeMap<String, CassyMergeClose> {
        let recording_verdict = matches!(
            receipt.schema_version,
            cas_store::RELEASE_VERDICT_SCHEMA | cas_store::RELEASE_TAG_VERDICT_SCHEMA
        );
        let mut outcomes = std::collections::BTreeMap::new();
        let Ok(store) = self.open_task_store() else {
            return receipt.task_heads.keys().map(|id| (id.clone(), CassyMergeClose::Held(
                "TASK_STORE_UNREADABLE: the task store is unreadable; retry task close.".into(),
            ))).collect();
        };
        for task_id in receipt.task_heads.keys() {
            // A close can outlive the batch response's post-commit grace.
            // Persist each result as it happens, including a selector refusal.
            let record = |outcome: &str| {
                if let Err(error) = record_batch_close_outcome(&self.cas_root, store.as_ref(), task_id, &receipt.id, &receipt.recorded_at, outcome) {
                    tracing::warn!(%task_id, %error, "batch close outcome not recorded");
                }
            };
            let mut hold = |reason: &str| {
                use sha2::Digest;
                let detail = format!(
                    "Receipt {} covers {task_id} at {}. Ordinary finalization is held:\n{reason}\n\n\
                     Supervisor action: resolve the gate above, then retry ordinary close with \
                     task action=close id={task_id} commit_receipt={} reason=\"<gate proof>\". \
                     No worker re-close is required.",
                    receipt.id, receipt.staging_sha, receipt.task_heads[task_id],
                );
                record(&detail);
                // A retired implementer cannot relay this hold. Address the
                // recorder (or its live successor/operator) directly, using
                // the existing idempotent supervisor outbox and wake class.
                let notice = (|| -> Result<(), String> {
                    if crate::release_reconcile::active() { return Ok(()); }
                    let agents = crate::store::open_agent_store(&self.cas_root)
                        .map_err(|error| error.to_string())?.list(None).map_err(|error| error.to_string())?;
                    let recipient = crate::supervisor_notices::resolve(
                        &agents, Some(&receipt.recorder_agent_id), None,
                    );
                    let occurrence = format!("{task_id}:{}:{:x}", receipt.task_heads[task_id], sha2::Sha256::digest(reason.as_bytes()));
                    crate::supervisor_notices::enqueue(
                        &self.cas_root, &recipient, "batch_close_hold", &detail, &occurrence,
                    ).map_err(|error| error.to_string())
                })();
                let mut reason = reason.to_string();
                if let Err(error) = notice {
                    reason.push_str(&format!("; supervisor notice not queued: {error}"));
                }
                outcomes.insert(task_id.clone(), CassyMergeClose::Held(reason));
            };
            let task = match store.get(task_id) {
                Ok(task) => task,
                Err(error) => {
                    hold(&error.to_string());
                    continue;
                }
            };
            if task.status == TaskStatus::Closed {
                outcomes.insert(task_id.clone(), CassyMergeClose::AlreadyClosed);
                continue;
            }
            let (repo, _) = match crate::qa_batch::task_context(&self.cas_root, &task) {
                Ok(context) => context,
                Err(error) => {
                    hold(&error);
                    continue;
                }
            };
            let target = crate::qa_batch::release_close::target(&self.cas_root, &task.id)
                .unwrap_or_else(|| receipt.staging_sha.clone());
            if task.status == TaskStatus::Blocked
            {
                hold("RELEASE_MEMBER_HELD: independent task hold remains active");
                continue;
            }
            if receipt.schema_version == cas_store::RELEASE_TAG_VERDICT_SCHEMA {
                let Some(anchor) = receipt.task_heads.get(task_id) else {
                    hold("the release verdict has no delivery anchor for this task");
                    continue;
                };
                if !crate::qa_batch::release_tag_ships_delivery(&repo, anchor, &receipt.staging_sha, &task.id) {
                    hold("the release tag no longer contains this delivery");
                    continue;
                }
            }
            match super::super::block_hold::read(store.as_ref(), &task.id) {
                Ok(Some(_)) => { hold("RELEASE_MEMBER_HELD: independent task hold remains active"); continue; }
                Err(error) => { hold(&format!("RELEASE_HOLD_UNREADABLE: {error}")); continue; }
                Ok(None) => {}
            }
            let own_review = match release_receipt_close_admission(&self.cas_root, &task, &repo) {
                Ok(own_review) => own_review,
                Err(reason) => {
                    hold(&reason);
                    continue;
                }
            };
            // cas-bc15: after request_changes the retired anchor is the delivery.
            let anchor = crate::qa_batch::resolved_delivery_anchor(&repo, &task, &target).unwrap_or_default();
            let verdict = match crate::qa_batch::satisfying_release_verdict(&self.cas_root, &task, &repo, &target, &anchor) {
                Ok(Some(verdict)) => Some(verdict),
                Ok(None) if recording_verdict => {
                    hold("this verdict does not cover the delivery's current anchor; review it again");
                    continue;
                }
                Ok(None) => None,
                Err(error) => {
                    hold(&error);
                    continue;
                }
            };
            let outcome = if let Some(verdict) = verdict.as_ref().filter(|_| own_review.is_none()) {
                self.close_after_batch_receipt(task_id, verdict, Some(&verdict.id)).await
            } else if task.status == TaskStatus::AwaitingMerge
                && task.deliverables.effective_merge_ci_admission() != cas_types::MergeCiAdmission::Override
            {
                // QA attests the walk, never the review. Ordinary close may
                // finish a dedicated review or report the missing release verdict.
                match crate::qa_batch::satisfying_receipt(&self.cas_root, &task, &repo, &target, &anchor)
                    .map(|qa| qa.or(verdict)) {
                    Ok(Some(qa)) => self.close_after_batch_receipt(task_id, &qa, None).await,
                    Ok(None) => CassyMergeClose::Held("Batch QA receipt no longer covers the current delivery anchor; record current QA evidence before close.".into()),
                    Err(error) => CassyMergeClose::Held(error),
                }
            } else {
                CassyMergeClose::NotEligible
            };
            match outcome {
                CassyMergeClose::Closed { notice } => {
                    record(&format!("closed; {notice}"));
                    outcomes.insert(task_id.clone(), CassyMergeClose::Closed { notice });
                }
                CassyMergeClose::AlreadyClosed => {
                    outcomes.insert(task_id.clone(), CassyMergeClose::AlreadyClosed);
                }
                CassyMergeClose::Held(text) => hold(&text),
                CassyMergeClose::NotEligible if task.deliverables.factory_branch_anchor.is_none() => hold(&format!(
                    "this receipt covers its merged delivery, but request_changes moved the task out of its park; close it with task close commit_receipt={}",
                    receipt.staging_sha
                )),
                CassyMergeClose::NotEligible => {
                    hold("not parked, or merged under a CI override; it needs its own close")
                }
            }
        }
        outcomes
    }

    pub(crate) async fn close_release_covered_deliveries(&self, receipt: &cas_store::QaBatchReceipt) -> String {
        let outcomes = self.close_release_covered_delivery_outcomes(receipt).await;
        let (mut closed, mut already, mut held) = (Vec::new(), Vec::new(), Vec::new());
        for (id, outcome) in &outcomes {
            match outcome {
                CassyMergeClose::Closed { .. } => closed.push(id.as_str()),
                CassyMergeClose::AlreadyClosed => already.push(id.as_str()),
                CassyMergeClose::Held(reason) => {
                    let summary = if let Some((gate, warning)) = reason.rsplit_once("; supervisor notice not queued:") {
                        format!("{}; supervisor notice not queued:{warning}", summarize_gate(gate))
                    } else { summarize_gate(reason) };
                    held.push(format!("- {id}: {summary}"));
                },
                CassyMergeClose::NotEligible => {},
            }
        }
        let mut report = String::new();
        if !closed.is_empty() {
            report.push_str(&format!("\nClosed {}: {}.", closed.len(), closed.join(", ")));
        }
        if !already.is_empty() {
            report.push_str(&format!("\nAlready closed: {}.", already.join(", ")));
        }
        if !held.is_empty() {
            report.push_str(&format!("\nStill held ({}):\n{}", held.len(), held.join("\n")));
        }
        report
    }

    /// Return a Blocked merged delivery to AwaitingMerge for its verdict
    /// close. Nothing is recorded yet: the caller writes the authority note
    /// after the close commits, or restores the hold when the close is held.
    fn release_hold_for_verdict(
        &self,
        store: &dyn cas_store::TaskStore,
        task: &Task,
        merge_commit: &str,
        verification_id: &str,
    ) -> Result<(Task, VerdictRelease), String> {
        let hold = super::super::block_hold::read(store, &task.id).unwrap_or(None);
        let dispatch_requested = cas_store::get_latest_verification_dispatch(&self.cas_root, &task.id)
            .ok()
            .flatten()
            .map(|dispatch| dispatch.requested_at);
        let foreign_hold = hold.as_ref().filter(|hold| {
            dispatch_requested.is_none_or(|requested| hold.when < requested)
        });
        let stamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
        let mut note = format!(
            "[{stamp}] Cassy close authority: merge {merge_commit} + verdict {verification_id}."
        );
        if let Some(hold) = foreign_hold {
            note.push_str(&format!(
                " Closed over a worker hold set by {} at {}: {}",
                hold.who,
                hold.when.format("%Y-%m-%d %H:%M"),
                hold.reason
            ));
        }
        let release = VerdictRelease {
            note,
            foreign_hold: foreign_hold.map(|hold| hold.reason.clone()),
            was_blocked: task.status == TaskStatus::Blocked,
            resumed_status: TaskStatus::AwaitingMerge,
            hold: hold
                .as_ref()
                .and_then(|hold| serde_json::to_value(hold).ok()),
        };
        if release.was_blocked {
            let mut parked = task.clone();
            parked.status = TaskStatus::AwaitingMerge;
            parked.updated_at = chrono::Utc::now();
            store
                .update_if_current(&parked, task)
                .map_err(|error| format!("the blocked delivery could not be released for close ({error}); retry task close."))?;
        }
        if release.hold.is_some() {
            store
                .patch_execution_state(&task.id, &serde_json::json!({"blocked_hold": null}))
                .map_err(|error| format!("the worker hold could not be cleared ({error}); retry task close."))?;
        }
        let released = store.get(&task.id).map_err(|error| error.to_string())?;
        Ok((released, release))
    }

    /// cas-e9d6: the authority note is written only once the close has
    /// committed, and a foreign hold is reported only then. Returns what
    /// could not be recorded, for the supervisor-visible receipt.
    fn record_verdict_release(
        &self,
        store: &dyn cas_store::TaskStore,
        closed: &Task,
        release: &VerdictRelease,
    ) -> Vec<String> {
        let task_id = closed.id.as_str();
        let mut unrecorded = Vec::new();
        if let Err(error) = store.append_trusted_note(task_id, &release.note) {
            tracing::warn!(%task_id, %error, "close authority note not recorded");
            unrecorded.push(format!("the close-authority note ({error})"));
        }
        if let Some(reason) = release.foreign_hold.as_deref()
            && let Err(error) = super::supervisor_push::queue_cassy_close_over_hold_receipt(
                &self.cas_root,
                closed,
                reason,
            )
        {
            tracing::warn!(%task_id, %error, "close-over-hold receipt not queued");
            unrecorded.push(format!("the close-over-hold receipt ({error})"));
        }
        unrecorded
    }

    /// cas-e9d6: a held verdict close puts back the worker hold it lifted.
    /// cas-43d2: what it could not put back is part of the held result.
    fn held_after_verdict(
        &self,
        store: &dyn cas_store::TaskStore,
        task_id: &str,
        release: Option<VerdictRelease>,
        text: String,
    ) -> CassyMergeClose {
        let Some(release) = release else {
            return CassyMergeClose::Held(text);
        };
        let mut unrestored = Vec::new();
        // A source gate may already have returned this delivery to the
        // worker. Never restore the old verdict hold over that fresh cycle.
        let current = match store.get(task_id) {
            Ok(current) if current.status != release.resumed_status => {
                return CassyMergeClose::Held(text);
            }
            Ok(current) => Some(current),
            Err(error) => {
                tracing::warn!(%task_id, %error, "held verdict close could not read the task to restore it");
                if release.was_blocked {
                    unrestored.push(format!("Blocked status ({error})"));
                }
                if release.hold.is_some() {
                    unrestored.push(format!("the worker hold ({error})"));
                }
                None
            }
        };
        if let Some(current) = current {
            if release.was_blocked {
                let mut blocked = current.clone();
                blocked.status = TaskStatus::Blocked;
                blocked.updated_at = chrono::Utc::now();
                if let Err(error) = store.update_if_current(&blocked, &current) {
                    tracing::warn!(%task_id, %error, "held verdict close could not restore Blocked");
                    unrestored.push(format!("Blocked status ({error})"));
                }
            }
            if let Some(hold) = release.hold
                && let Err(error) = store.patch_execution_state(task_id, &serde_json::json!({"blocked_hold": hold}))
            {
                tracing::warn!(%task_id, %error, "held verdict close could not restore the worker hold");
                unrestored.push(format!("the worker hold ({error})"));
            }
        }
        if unrestored.is_empty() {
            return CassyMergeClose::Held(text);
        }
        CassyMergeClose::Held(format!(
            "{text}\n\nCassy lifted these for the verdict close and could not put them back: {}. Restore them before the worker resumes.",
            unrestored.join("; ")
        ))
    }

    /// cas-46ea: a rejected verdict on a merged delivery returns the task to
    /// its worker with the findings, exactly as `request_changes` would.
    pub(crate) fn return_rejected_merged_delivery(
        &self,
        task_id: &str,
        verification: &Verification,
    ) -> String {
        let mut findings = verification.summary.trim().to_string();
        for issue in verification.issues.iter().filter(|issue| issue.is_blocking()) {
            let location = issue
                .line
                .map(|line| format!("{}:{line}", issue.file))
                .unwrap_or_else(|| issue.file.clone());
            findings.push_str(&format!("; {location}: {}", issue.problem.trim()));
        }
        let findings: String = findings.chars().take(900).collect();
        let reason = format!(
            "Verifier rejected the merged delivery ({}): {findings}",
            verification.id
        );
        let actor = self.get_agent_id().unwrap_or_else(|_| "cassy".to_string());
        if let Err(error) =
            cas_store::request_changes_for_parked_delivery(&self.cas_root, task_id, &actor, &reason)
        {
            return format!(
                "\nCassy could not return task {task_id} to its worker ({error}); run task action=request_changes id={task_id}."
            );
        }
        let notice = self
            .open_task_store()
            .ok()
            .and_then(|store| store.get(task_id).ok())
            .map(|task| {
                super::supervisor_push::queue_rejected_merge_reopen_notice(
                    &self.cas_root,
                    &task,
                    &verification.id,
                    &findings,
                )
            });
        let notice = match notice {
            Some(Ok(Some(prompt_id))) => format!("worker notified (prompt {prompt_id})"),
            Some(Ok(None)) => "no live assigned worker to notify".to_string(),
            Some(Err(error)) => format!("worker notice failed: {error}"),
            None => "worker notice skipped: task unreadable".to_string(),
        };
        format!("\nCassy returned task {task_id} to its worker with the findings; {notice}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "CAS Test")
            .env("GIT_AUTHOR_EMAIL", "cas@example.test")
            .env("GIT_COMMITTER_NAME", "CAS Test")
            .env("GIT_COMMITTER_EMAIL", "cas@example.test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn batch_hold_note_is_bounded_and_not_a_bundle_citation_cas_dd05() {
        // Condensed cas-6faf refusal and command hint with a fixture artifact root.
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        let store = crate::store::open_task_store(&root).unwrap();
        store.add(&Task::new("cas-note".into(), "outcome note".into())).unwrap();
        let hold = "PARK REFUSED: more than one close gate failed.\n\nTASK CLOSE REJECTED: cas-6faf is user-facing and its QA evidence bundle is not cited. Next: produce the bundle, then `task action=notes id=cas-6faf notes=\"qa-bundle: /proof/cas-6faf/qa/bundle.json\"`.";
        for outcome in [hold.to_string(), "Unreadable qa-bundle: /suggested/bundle.json".into(), format!("{hold}{}", " long detail".repeat(100))] {
            let note = record_batch_close_outcome(&root, store.as_ref(), "cas-note", "qab-fixture", "2026-11-09T03:00:00Z", &outcome).unwrap();
            assert_eq!(crate::qa_evidence::cited_bundle_path(&note), None, "{note}");
            assert_eq!(note.lines().count(), 1);
            assert!(note.chars().count() <= 800, "outcome note is too long");
        }
    }

    #[test]
    fn batch_outcome_keeps_a_complete_remedy_pointer_cas_d4310() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        let store = crate::store::open_task_store(&root).unwrap();
        store.add(&Task::new("cas-dd05".into(), "scoped proof hold".into())).unwrap();
        let command = "SCOPED_PROOF_BASE=eee347de689bf0b842a14b8146d42118e1d455fd scripts/run-scoped-tests.sh --proof";
        let outcome = format!("scoped proof receipt has base None; must be proven against SCOPED_PROOF_BASE=eee347de689bf0b842a14b8146d42118e1d455fd. Re-run the delivered tip in a detached supervisor checkout with: {command}");
        record_batch_close_outcome(&root, store.as_ref(), "cas-dd05", "qab-remedy", "2026-11-09T03:00:00Z", &outcome).unwrap();
        let note = store.get("cas-dd05").unwrap().notes;
        let path = note.split_once("Details: ").expect("the bounded note must point to its complete remedy")
            .1.split_once(" (SHA-256 ").unwrap().0;
        let detail = std::fs::read_to_string(path).unwrap();
        assert_eq!(detail, outcome);
        assert!(detail.contains(command));
        assert_eq!(note.lines().count(), 1);
        assert!(note.chars().count() <= 800, "{note}");
    }

    /// cas-e9d6: agents share one Git identity, so a tip someone else pushed
    /// to the worker's origin branch is told apart by the worker's own local
    /// branch, which never received it.
    #[test]
    fn a_tip_pushed_by_someone_else_is_not_the_workers_delivery_cas_e9d6() {
        let origin = tempfile::tempdir().unwrap();
        git(origin.path(), &["init", "-q", "--bare"]);
        let repo = tempfile::tempdir().unwrap();
        let repo = repo.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["commit", "-q", "--allow-empty", "-m", "seed"]);
        git(repo, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
        git(repo, &["checkout", "-q", "-b", "factory/w"]);
        git(repo, &["commit", "-q", "--allow-empty", "-m", "worker delivery"]);
        git(repo, &["push", "-q", "origin", "factory/w"]);
        let mut task = Task::new("cas-e9d6".into(), "lane".into());
        task.assignee = Some("w".into());
        let worker_tip = git(repo, &["rev-parse", "HEAD"]);
        assert_eq!(pushed_delivery_tip(repo, &task, "factory/w", &worker_tip), Ok(()));

        let other = tempfile::tempdir().unwrap();
        git(other.path(), &["clone", "-q", "-b", "factory/w", origin.path().to_str().unwrap(), "."]);
        git(other.path(), &["commit", "-q", "--allow-empty", "-m", "supervisor fixup"]);
        git(other.path(), &["push", "-q", "origin", "factory/w"]);
        let foreign_tip = git(other.path(), &["rev-parse", "HEAD"]);
        git(repo, &["fetch", "-q", "origin"]);
        let refused = pushed_delivery_tip(repo, &task, "factory/w", &foreign_tip).unwrap_err();
        assert!(refused.contains("not on w's local factory/w"), "{refused}");
    }

    /// cas-43d2: what the one-hop verdict close could not restore or record
    /// reaches the supervisor-visible result, not only a warn log.
    #[test]
    fn unrestored_and_unrecorded_close_state_reaches_the_result_cas_43d2() {
        let cas = tempfile::tempdir().unwrap();
        let core = CasCore::with_daemon(cas.path().join(".cas"), None, None);
        let mut task = Task::new("cas-43d2".into(), "held verdict close".into());
        task.status = TaskStatus::AwaitingMerge;
        let store = crate::store::mock::MockTaskStore::with_tasks(vec![task.clone()]);
        let release = || VerdictRelease {
            note: "Cassy close authority".into(), foreign_hold: None, was_blocked: true,
            resumed_status: TaskStatus::AwaitingMerge, hold: None,
        };
        store.inject_error(cas_store::StoreError::Other("disk full".into()));
        let held = core.held_after_verdict(&store, &task.id, Some(release()), "gate text".into());
        let CassyMergeClose::Held(text) = held else { panic!("a held close stays held") };
        assert!(text.contains("could not put them back: Blocked status") && text.contains("disk full"), "{text}");
        // The mock store, like a failing one, cannot append a trusted note.
        let unrecorded = core.record_verdict_release(&store, &task, &release());
        assert!(unrecorded.iter().any(|failure| failure.contains("close-authority note")), "{unrecorded:?}");
    }
}
