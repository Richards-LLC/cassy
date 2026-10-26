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

/// The delivery a `worktree_merge` just landed: the repository, the source
/// branch it merged and the exact source commit it merged.
pub(crate) struct MergedLane<'a> {
    pub repo: &'a std::path::Path,
    pub branch: &'a str,
    pub tip: &'a str,
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
        return Err(format!("origin/{branch} does not exist"));
    };
    if pushed != tip && !super::close_ops::git_commit_is_ancestor(repo, tip, &pushed) {
        return Err(format!("{tip} is not pushed to origin/{branch}"));
    }
    let local = super::close_ops::resolve_branch_sha(repo, &format!("refs/heads/{branch}"));
    if !local.is_some_and(|local| local == tip || super::close_ops::git_commit_is_ancestor(repo, tip, &local)) {
        return Err(format!(
            "{tip} is on origin/{branch} but not on {worker}'s own {branch}; a commit pushed by someone else is not the worker's delivery"
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
    hold: Option<serde_json::Value>,
}

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

impl CasCore {
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
            "[{}] Cassy parked this task at its pushed delivery tip {} on {} after merge {merge_commit}; no worker re-close is needed.",
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
        let Ok(mut task) = store.get(task_id) else {
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
        let release = match verification_id {
            Some(verification_id) => {
                match self.release_hold_for_verdict(store.as_ref(), &task, &merge_commit, verification_id) {
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
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: task.deliverables.factory_branch_anchor.clone(),
        };
        let result = match as_cassy_merge_close(self.cas_task_close(Parameters(request))).await {
            Ok(result) => result,
            Err(error) => {
                let text = error.message.to_string();
                return self.held_after_verdict(store.as_ref(), task_id, release, text);
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
            return self.held_after_verdict(store.as_ref(), task_id, release, text);
        };
        if closed.status != TaskStatus::Closed {
            return self.held_after_verdict(store.as_ref(), task_id, release, text);
        }
        let unrecorded = release.as_ref()
            .map(|release| self.record_verdict_release(store.as_ref(), &closed, release))
            .unwrap_or_default();
        let mut notice = match super::supervisor_push::queue_cassy_merge_close_notice(
            &self.cas_root,
            &closed,
            &merge_commit,
        ) {
            Ok(Some(prompt_id)) => format!("worker notified (prompt {prompt_id})"),
            Ok(None) => "no live assigned worker to notify".to_string(),
            Err(error) => format!("worker notice failed: {error}"),
        };
        // cas-43d2: the supervisor reads this receipt; a record that failed
        // after the close committed belongs in it, not only in a warn log.
        for failure in unrecorded {
            notice.push_str(&format!("; NOT RECORDED: {failure}"));
        }
        CassyMergeClose::Closed { notice }
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
        if release.was_blocked {
            let restored = store.get(task_id).map_err(|error| error.to_string()).and_then(|current| {
                if current.status != TaskStatus::AwaitingMerge {
                    return Ok(());
                }
                let mut blocked = current.clone();
                blocked.status = TaskStatus::Blocked;
                blocked.updated_at = chrono::Utc::now();
                store.update_if_current(&blocked, &current).map(drop).map_err(|error| error.to_string())
            });
            if let Err(error) = restored {
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
        assert!(refused.contains("not on w's own factory/w"), "{refused}");
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
            note: "Cassy close authority".into(), foreign_hold: None, was_blocked: true, hold: None,
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
