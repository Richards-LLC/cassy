//! cas-5ea7: the hotfix fast lane at close. At park an eligible delivery
//! defers QA evidence; after a lane merge the post-merge receipt
//! (receipt checks, receipt statuses and the journey walk) replaces the QA
//! evidence gate, and an attributable receipt failure opens a revert PR.
use super::*;
use crate::fast_lane::{self as lane, Eligibility, FastLaneConfig, Receipt, Walk};
use std::path::Path;

/// Where this close stands on the lane.
pub(super) enum LaneClose {
    Off,
    /// Parked and eligible: merges on the lane's merge checks.
    Park,
    /// Merged on the lane at `merge`, at `at`.
    Merged { lane: FastLaneConfig, merge: String, at: chrono::DateTime<chrono::Utc> },
}

impl LaneClose {
    pub(super) fn on_lane(&self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// A lane merge recorded for this delivery's merge, else park eligibility,
/// recorded as a decision note naming the measured values or the refusal.
pub(super) fn classify(
    notes: &mut CloseNoteTracker<'_>,
    task: &mut Task,
    cas_root: &Path,
    repo: &Path,
    target: &str,
    receipt: Option<&str>,
) -> LaneClose {
    let Some(config) = lane::configured(cas_root) else {
        return LaneClose::Off;
    };
    if let Some(merge) = task.deliverables.green_merge_commit.clone() {
        return match lane::merged_delivery(task) {
            Some(record) => {
                LaneClose::Merged { lane: config, merge, at: record.at }
            }
            _ => LaneClose::Off,
        };
    }
    let Some(head) = super::super::qa_evidence_gate::delivered_head(task, repo, target, receipt) else {
        return LaneClose::Off;
    };
    let eligibility = lane::evaluate_delivery(&config, task, repo, target, &head);
    notes.record(task, &format!("✅ DECISION {}", lane::eligibility_note(&config, &head, &eligibility)));
    match eligibility {
        Eligibility::Eligible { .. } => LaneClose::Park,
        Eligibility::Refused(_) => LaneClose::Off,
    }
}

/// One follow-up task per `key`, recorded on the hotfix so retries reuse it.
fn follow_up(notes: &mut CloseNoteTracker<'_>, task: &mut Task, key: &str, title: &str, description: &str) -> String {
    if lane::followed_up(&task.notes, key) {
        return "already filed".into();
    }
    match lane::file_follow_up(notes.store, task, title.to_string(), description.to_string()) {
        Ok(id) => {
            record_proof(notes, task, &lane::follow_up_note(key, &id));
            id
        }
        Err(error) => {
            notes.record(task, &format!("FAST LANE WARNING: the {key} follow-up task could not be filed ({error})."));
            format!("not filed: {error}")
        }
    }
}

/// Lane state goes through the store's trusted append, the only writer that
/// keeps a reserved `FAST_LANE_PROOF:` line; the task is then reread so the
/// close snapshot carries it.
fn record_proof(notes: &mut CloseNoteTracker<'_>, task: &mut Task, note: &str) {
    let formatted = format!("[{}] {note}", chrono::Utc::now().format("%Y-%m-%d %H:%M"));
    let reread = notes
        .store
        .append_trusted_note(&task.id, &formatted)
        .and_then(|_| notes.store.get(&task.id));
    match reread {
        Ok(current) => {
            task.notes = current.notes;
            task.updated_at = current.updated_at;
            notes.persisted_bytes = task.notes.len();
        }
        Err(error) => notes.record(task, &format!("FAST LANE WARNING: lane state was not recorded ({error}).")),
    }
}

/// What the receipt gate decided.
pub(super) enum ReceiptGate {
    Passed,
    Hold(String),
    /// A failed receipt that returns the merged delivery to its worker.
    Failed(String),
}

impl CasCore {
    pub(super) fn fast_lane_receipt_gate(
        &self,
        notes: &mut CloseNoteTracker<'_>,
        task: &mut Task,
        stage: &LaneClose,
        repo: &Path,
        target: &str,
        ledger: &Path,
    ) -> ReceiptGate {
        let LaneClose::Merged { lane: config, merge, at } = stage else {
            return ReceiptGate::Passed;
        };
        match crate::fast_lane_walk::pending(&self.cas_root, task) {
            Ok(Some(message)) => return ReceiptGate::Hold(message),
            Ok(None) => {},
            Err(error) => return ReceiptGate::Hold(format!("FAST LANE QA DECISION UNREADABLE: {error}")),
        }
        let gh = crate::github_repo::gh_binary();
        let elapsed = (chrono::Utc::now() - *at).num_minutes().max(0);
        let window = i64::from(config.receipt_window_mins);
        let due = if elapsed > window {
            format!("OVERDUE: the receipt was due {window} minutes after merge, {} minutes ago", elapsed - window)
        } else {
            format!("due within {} minutes", window - elapsed)
        };
        let observed = match lane::observe(repo, merge, &gh) {
            Ok(observed) => observed,
            Err(error) => {
                return ReceiptGate::Hold(format!(
                    "FAST LANE RECEIPT UNREADABLE for merge {merge}: {error}. Retry the close when GitHub is reachable ({due})."
                ));
            }
        };
        let failed = config.receipt_checks.iter().any(|name| {
            observed.checks.iter().any(|check| &check.name == name && check.state == lane::State::Failure)
        });
        let parent = failed
            .then(|| lane::resolve(repo, &format!("{merge}^1")))
            .flatten()
            .and_then(|parent| lane::observe(repo, &parent, &gh).ok());
        match lane::receipt(config, &observed, parent.as_ref(), |name| lane::reran(&task.notes, merge, name)) {
            Receipt::Pending(waiting) => {
                self.arm_fast_lane_wake(&task.id, merge);
                ReceiptGate::Hold(format!(
                    "FAST LANE RECEIPT PENDING for merge {merge}: {waiting} ({due}). Cassy re-checks when CI on the merge finishes; the task stays open until the receipt passes."
                ))
            }
            Receipt::Rerun(check) => match lane::rerun(repo, &check, &gh) {
                Ok(()) => {
                    record_proof(notes, task, &lane::rerun_note(merge, &check));
                    self.arm_fast_lane_wake(&task.id, merge);
                    ReceiptGate::Hold(format!(
                        "FAST LANE RECEIPT: {} failed on merge {merge}; its failed jobs are rerunning once ({}).",
                        check.name,
                        check.url.as_deref().unwrap_or("run URL unavailable")
                    ))
                }
                Err(error) => ReceiptGate::Hold(format!(
                    "FAST LANE RECEIPT: {} failed on merge {merge} and could not be rerun ({error}). Rerun it, then retry the close.",
                    check.name
                )),
            },
            Receipt::Revert(check, parent_green) => {
                let why = if parent_green {
                    "it passed on the merge's first parent, so the hotfix caused it"
                } else {
                    "its result on the merge's first parent is unknown or unreadable, so it counts as caused by the hotfix"
                };
                let opened = match lane::reverted(&task.notes, merge) {
                    Some(url) => Ok(url),
                    None => lane::open_revert_pr(repo, target, merge, &task.id, &check, why, &gh).inspect(|url| {
                        record_proof(notes, task, &lane::revert_note(merge, &check.name, url));
                    }),
                };
                match opened {
                    Ok(url) => ReceiptGate::Failed(format!(
                        "FAST LANE REVERT: {} failed on merge {merge} after one rerun; {why}. Revert PR: {url}. Merge it once its merge checks pass and tell the operator; the hotfix returns to its worker to fix forward.",
                        check.name
                    )),
                    Err(error) => ReceiptGate::Hold(format!(
                        "FAST LANE REVERT NEEDED: {} failed on merge {merge} after one rerun; {why}, but the revert PR could not be opened ({error}). Revert {merge} by hand, then retry the close.",
                        check.name
                    )),
                }
            }
            Receipt::Return(status) => ReceiptGate::Failed(format!(
                "FAST LANE RECEIPT FAILED: {} failed on merge {merge} ({}); production keeps its previous deployment. Fix the build and re-deliver.",
                status.name,
                status.url.as_deref().unwrap_or("no target URL")
            )),
            Receipt::Passed { excused } => {
                if !excused.is_empty() {
                    let names: Vec<_> = excused.iter().map(|check| check.name.as_str()).collect();
                    follow_up(notes, task, &format!("pre-existing {merge}"), "Fix the receipt check that was already red before a fast-lane hotfix", &format!(
                        "{} failed on hotfix merge {merge} and on its first parent too, so the failure predates hotfix {}. Fix it on the target branch.",
                        names.join(", "),
                        task.id
                    ));
                }
                match lane::journey_walk(ledger, &task.notes) {
                    Walk::Done => {
                        notes.record(task, &format!(
                            "FAST LANE RECEIPT PASSED on merge {merge} after {elapsed} minutes: {}.",
                            config.receipt_checks.iter().chain(&config.receipt_statuses).cloned().collect::<Vec<_>>().join(", ")
                        ));
                        ReceiptGate::Passed
                    }
                    Walk::Missing => ReceiptGate::Hold(format!(
                        "FAST LANE RECEIPT: CI passed on merge {merge}; the journey walk is owed ({due}). Walk each touched journey on production and record a row per journey in {}, or record a decision `{}` <reason> when the diff touches none, then retry the close.",
                        ledger.display(),
                        lane::NO_JOURNEY_IMPACT
                    )),
                    Walk::Failed(row) => {
                        let decision = self.get_agent_id().map_err(|error| format!("{error:?}"))
                            .and_then(|owner| crate::fast_lane_walk::ask(
                                &self.cas_root, task, repo, target, &owner, &row,
                            ).map_err(|error| error.to_string()));
                        ReceiptGate::Hold(decision.unwrap_or_else(|error| format!(
                            "FAST LANE QA WALK FAILED on merge {merge}: {row}. Commander decision could not be recorded ({error}); retry the close."
                        )))
                    },
                }
            }
        }
    }

    /// One `ci_completed` wake per merge, so the close retries when CI settles.
    fn arm_fast_lane_wake(&self, task_id: &str, merge: &str) {
        const MESSAGE: &str = "FAST LANE RECEIPT";
        let armed = (|| -> anyhow::Result<()> {
            let owner = self.get_agent_id().map_err(|error| anyhow::anyhow!("{error:?}"))?;
            let reminders = crate::store::open_reminder_store(&self.cas_root)?;
            let filter = serde_json::json!({ "sha": merge });
            if reminders.list_all_pending()?.iter().any(|reminder| {
                reminder.task_id.as_deref() == Some(task_id)
                    && reminder.message.starts_with(MESSAGE)
                    && reminder.trigger_filter.as_ref() == Some(&filter)
            }) {
                return Ok(());
            }
            reminders.create_with_scope(
                &owner,
                None,
                &format!("{MESSAGE}: CI finished on hotfix merge {merge}. Run task action=close id={task_id} to record its post-merge receipt."),
                cas_store::ReminderTriggerType::Event,
                None,
                Some("ci_completed"),
                Some(&filter),
                24 * 3600,
                None,
                None,
                true,
                Some(task_id),
            )?;
            Ok(())
        })();
        if let Err(error) = armed {
            tracing::warn!(%task_id, %merge, %error, "fast-lane receipt wake not armed");
        }
    }
}
