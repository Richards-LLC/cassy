use cas_types::{Task, TaskStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::str::FromStr;

const MERGE_ENVELOPE_OPEN: &str = "<cas-merge-request>";
const MERGE_ENVELOPE_CLOSE: &str = "</cas-merge-request>";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MergeRequestEnvelope {
    pub task_id: String,
    pub branch_tip: String,
    pub target_branch: String,
    pub target_branch_tip: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeRequestDecision {
    Pending { target_tip: String },
    AlreadyIntegrated { target_tip: String },
    Unverifiable,
}

/// What the daemon should do with a worker→supervisor merge request at
/// transport time (cas-6eab, GH #61).
///
/// The request is an instruction ("please merge `<tip>`"), so it is only worth
/// delivering while its premise holds. Two things can kill that premise
/// between the worker composing it and the supervisor reading it, and in the
/// reported sessions both routinely did — the supervisor had already merged
/// and already replied before the request arrived, on ~12 of ~20 closes:
///
/// - the branch tip is already an ancestor of the target branch (merged), or
/// - the task is no longer parked awaiting a merge (re-closed, reopened,
///   request_changes'd), so nothing is waiting on the supervisor at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeRequestDelivery {
    /// Premise still holds — deliver, and tag the row so it can be retracted
    /// if the merge lands before the supervisor reads it.
    Deliver,
    /// The merge already landed. Suppress and tell the WORKER instead.
    SuppressLanded { target_tip: String },
    /// The task left `AwaitingMerge` — the request is moot whatever git says.
    SuppressResolved { status: TaskStatus },
}

/// Decide a merge request's fate from live state only (cas-6eab).
///
/// `task_status` is the task's status read fresh at transport time; `None`
/// means the task could not be read at all. `git` is the live reachability
/// check for the requested tip.
///
/// Fails open in exactly one direction: uncertainty (unreadable task,
/// unverifiable git) delivers. A suppression requires positive evidence that
/// the request is already satisfied, because the cost of wrongly suppressing
/// a genuine merge request is a stalled task, while the cost of delivering a
/// stale one is a supervisor round-trip.
pub(crate) fn merge_request_delivery_decision(
    task_status: Option<TaskStatus>,
    git: &MergeRequestDecision,
) -> MergeRequestDelivery {
    if let Some(status) = task_status
        && status != TaskStatus::AwaitingMerge
    {
        return MergeRequestDelivery::SuppressResolved { status };
    }
    match git {
        MergeRequestDecision::AlreadyIntegrated { target_tip } => {
            MergeRequestDelivery::SuppressLanded {
                target_tip: target_tip.clone(),
            }
        }
        MergeRequestDecision::Pending { .. } | MergeRequestDecision::Unverifiable => {
            MergeRequestDelivery::Deliver
        }
    }
}

/// Guidance sent to the worker when its merge request is suppressed because
/// the task is no longer parked (cas-6eab).
pub(crate) fn merge_request_moot_guidance(task_id: &str, status: TaskStatus) -> String {
    format!(
        "CAS suppressed your merge request for {task_id}: the task is no longer awaiting a \
         merge (current status: {status}). Nothing is queued for the supervisor. Re-read the \
         task with `task action=show id={task_id}` before sending anything further about it."
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LifecycleEnvelope {
    pub task_id: String,
    pub new_status: TaskStatus,
    pub occurrence: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LifecyclePromptDecision {
    Deliver,
    SuppressStale { task_id: String },
    Unstructured,
}

/// What the daemon should do with a lifecycle prompt whose task state has
/// moved on (cas-7787, GH #160).
///
/// [`revalidate_lifecycle_prompt`] answers only "is this payload still true?".
/// That question is necessary but not sufficient, and treating it as
/// sufficient is the reported defect: in session cas-src-fast-pelican-83 the
/// `task_awaiting_merge` relays for cas-fe23 (18:51), cas-d897 (19:00),
/// cas-b69a (19:17) and cas-edee (19:26) were each enqueued correctly,
/// written to the supervisor's inbox, never transported, and then stamped
/// `suppressed_idle` at the exact second their task went
/// awaiting_merge → closed. Suppression was the right call for the PAYLOAD —
/// it had genuinely expired — and the wrong call for the FACT that a relay
/// the factory depends on had failed to arrive. Nothing recorded the failure,
/// so a human became the delivery mechanism for three finished lanes.
///
/// So staleness alone no longer decides. A stale row that was delivered is a
/// benign suppression; a stale row that was never transported is a delivery
/// FAILURE, and a wake-eligible one is a failure the factory cannot function
/// without knowing about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LifecycleStaleOutcome {
    /// Payload still true — hand it to transport.
    Deliver,
    /// Payload expired and the recipient already got it. Quiet suppression.
    SuppressDelivered { task_id: String },
    /// Payload expired and the recipient NEVER got it. Terminate the row (a
    /// live re-write would storm an expired premise) but record it loudly.
    UndeliveredRelayFailure { task_id: String },
}

/// Decide a stale lifecycle row's fate from the payload verdict plus the two
/// facts the payload cannot know: did it ever reach the recipient, and was it
/// load-bearing (cas-7787, GH #160).
///
/// Pure so the honesty contract is testable without a daemon or a harness.
///
/// Fails toward NOISE, deliberately and in one direction only: when a
/// wake-eligible relay is stale and undelivered we report a failure rather
/// than assume it was harmless. A false alarm costs the supervisor one glance
/// at `worker_status`; a missed one costs a parked lane an unbounded stall,
/// which is exactly what happened.
pub(crate) fn lifecycle_stale_outcome(
    decision: &LifecyclePromptDecision,
    wake_eligible: bool,
    transport_delivered: bool,
) -> LifecycleStaleOutcome {
    let LifecyclePromptDecision::SuppressStale { task_id } = decision else {
        return LifecycleStaleOutcome::Deliver;
    };
    // A non-wake lifecycle row (task_started / task_ready / task_closed) is
    // progress FYI. Losing one costs nothing the factory needs, so it keeps
    // the quiet path and does not train anyone to ignore the banner.
    if wake_eligible && !transport_delivered {
        return LifecycleStaleOutcome::UndeliveredRelayFailure {
            task_id: task_id.clone(),
        };
    }
    LifecycleStaleOutcome::SuppressDelivered {
        task_id: task_id.clone(),
    }
}

/// Operator-facing description of one undelivered lifecycle relay
/// (cas-7787, GH #160).
///
/// States what did not happen and what to do, in that order, without
/// mentioning a queue stage or a prompt row — the reader is a supervisor
/// deciding whether a lane is waiting on them, not someone debugging CAS.
pub(crate) fn undelivered_relay_notice(task_id: &str, summary: Option<&str>) -> String {
    let what = summary.unwrap_or("a task lifecycle transition");
    format!(
        "UNDELIVERED: the factory notified the supervisor that {task_id} needed them \
         ({what}) and that message never arrived. The task moved on before it could be \
         delivered, so it has been withdrawn rather than re-sent with an expired premise. \
         Check {task_id} directly — do not assume it was handled."
    )
}

pub(crate) fn select_unambiguous_merge_task<'a>(
    parked_tasks: &'a [Task],
    worker: &str,
    explicit_task_id: Option<&str>,
) -> Option<&'a Task> {
    let mut matches = parked_tasks.iter().filter(|task| {
        task.status == TaskStatus::AwaitingMerge
            && task
                .assignee
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(worker))
            && explicit_task_id.is_none_or(|id| task.id == id)
    });
    let only = matches.next()?;
    matches.next().is_none().then_some(only)
}

pub(crate) fn revalidate_merge_request(
    repo_path: &Path,
    branch_tip: &str,
    target_branch: &str,
) -> MergeRequestDecision {
    let Some(target_tip) = crate::mcp::tools::core::task::lifecycle::close_ops::resolve_branch_sha(
        repo_path,
        target_branch,
    ) else {
        return MergeRequestDecision::Unverifiable;
    };
    if crate::mcp::tools::core::task::lifecycle::close_ops::git_commit_is_ancestor(
        repo_path,
        branch_tip,
        &target_tip,
    ) {
        MergeRequestDecision::AlreadyIntegrated { target_tip }
    } else {
        MergeRequestDecision::Pending { target_tip }
    }
}

pub(crate) fn attach_merge_request_envelope(
    message: &str,
    envelope: &MergeRequestEnvelope,
) -> String {
    let encoded = serde_json::to_string(envelope)
        .expect("MergeRequestEnvelope contains only JSON-serializable strings");
    format!("{message}\n\n{MERGE_ENVELOPE_OPEN}{encoded}{MERGE_ENVELOPE_CLOSE}")
}

pub(crate) fn merge_landed_guidance(
    task_id: &str,
    branch_tip: &str,
    target_branch: &str,
    target_tip: &str,
) -> String {
    format!(
        "Merge already landed; CAS suppressed your stale merge request.\n\n\
         Task: {task_id}\nBranch tip: {branch_tip}\nTarget: {target_branch} at {target_tip}\n\n\
         Re-run task close for {task_id} now."
    )
}

pub(crate) fn parse_merge_request_envelope(prompt: &str) -> Option<MergeRequestEnvelope> {
    let start = prompt.rfind(MERGE_ENVELOPE_OPEN)? + MERGE_ENVELOPE_OPEN.len();
    let end = prompt[start..].find(MERGE_ENVELOPE_CLOSE)? + start;
    serde_json::from_str(&prompt[start..end]).ok()
}

fn xml_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

pub(crate) fn parse_lifecycle_envelope(prompt: &str) -> Option<LifecycleEnvelope> {
    let tag_end = prompt.find('>')?;
    let tag = &prompt[..tag_end];
    if !tag.starts_with("<task-lifecycle ") {
        return None;
    }
    let task_id = xml_attribute(tag, "task_id")?.to_string();
    let new_status = TaskStatus::from_str(xml_attribute(tag, "new")?).ok()?;
    let occurrence = DateTime::parse_from_rfc3339(xml_attribute(tag, "occurrence")?)
        .ok()?
        .with_timezone(&Utc);
    Some(LifecycleEnvelope {
        task_id,
        new_status,
        occurrence,
    })
}

pub(crate) fn revalidate_lifecycle_prompt(
    prompt: &str,
    current_status: TaskStatus,
    current_updated_at: DateTime<Utc>,
) -> LifecyclePromptDecision {
    let Some(envelope) = parse_lifecycle_envelope(prompt) else {
        return LifecyclePromptDecision::Unstructured;
    };
    if current_status != envelope.new_status {
        return LifecyclePromptDecision::SuppressStale {
            task_id: envelope.task_id,
        };
    }
    // cas-f02b (GH #101): occurrence equality is the right staleness test for a
    // transient transition — it distinguishes one Open→InProgress cycle from
    // the next. It is the WRONG test for a state the task is still sitting in.
    // A parked task keeps accruing writes while it waits (e.g.
    // `mark_awaiting_merge_conflicted` on a worker's close retry bumps
    // `updated_at`), and every one of those would have made the park's own
    // notification look stale and dropped it before transport — silently
    // reproducing the stall this notification exists to prevent. While the task
    // is still IN the state the envelope describes, the signal is still true.
    if current_status.is_parked_awaiting_supervisor() {
        return LifecyclePromptDecision::Deliver;
    }
    if current_updated_at == envelope.occurrence {
        LifecyclePromptDecision::Deliver
    } else {
        LifecyclePromptDecision::SuppressStale {
            task_id: envelope.task_id,
        }
    }
}

#[cfg(test)]
mod cas_7787_relay_honesty_tests {
    use super::*;
    use chrono::TimeZone;

    fn awaiting_merge_prompt(task_id: &str, occurrence: DateTime<Utc>) -> String {
        format!(
            "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"{task_id}\" \
             old=\"in_progress\" new=\"awaiting_merge\" actor=\"smooth-octopus-84\" \
             notification_id=\"3386\" occurrence=\"{}\">\nparked\n</task-lifecycle>",
            occurrence.to_rfc3339()
        )
    }

    /// The reported incident, reduced to its decision.
    ///
    /// Replays the exact cas-fe23 shape from session cas-src-fast-pelican-83:
    /// a `task_awaiting_merge` relay enqueued at 18:51:51, never transported,
    /// and revalidated at 18:53:58 after the task had gone
    /// awaiting_merge → closed. The old code called that a plain stale
    /// suppression and the supervisor was never told anything had been lost.
    #[test]
    fn an_awaiting_merge_relay_that_expires_undelivered_is_reported_as_a_failure() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 7, 18, 51, 51).unwrap();
        let closed_at = Utc.with_ymd_and_hms(2026, 8, 7, 18, 53, 58).unwrap();
        let prompt = awaiting_merge_prompt("cas-fe23", occurrence);

        let decision = revalidate_lifecycle_prompt(&prompt, TaskStatus::Closed, closed_at);
        assert!(
            matches!(decision, LifecyclePromptDecision::SuppressStale { .. }),
            "the payload really is stale once the task closed — that part was never wrong"
        );

        assert_eq!(
            lifecycle_stale_outcome(&decision, true, false),
            LifecycleStaleOutcome::UndeliveredRelayFailure {
                task_id: "cas-fe23".to_string()
            },
            "a wake-eligible relay that expired without transport is a delivery FAILURE, \
             not a benign suppression — this is the silence GH #160 reports"
        );
    }

    /// The honesty rule must not become a noise machine: a relay the recipient
    /// demonstrably received (explicit `message_ack`) is a quiet suppression,
    /// exactly as before.
    #[test]
    fn a_relay_the_supervisor_acknowledged_stays_a_quiet_suppression() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 7, 18, 51, 51).unwrap();
        let prompt = awaiting_merge_prompt("cas-fe23", occurrence);
        let decision = revalidate_lifecycle_prompt(
            &prompt,
            TaskStatus::Closed,
            occurrence + chrono::Duration::seconds(127),
        );

        assert_eq!(
            lifecycle_stale_outcome(&decision, true, true),
            LifecycleStaleOutcome::SuppressDelivered {
                task_id: "cas-fe23".to_string()
            },
            "delivered-then-expired must stay silent or the banner trains people to skip it"
        );
    }

    /// Progress FYI (`task_started` / `task_closed`) is not wake-eligible.
    /// Losing one costs the factory nothing, so it must not raise an alarm.
    #[test]
    fn a_non_wake_lifecycle_row_never_raises_the_alarm() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 7, 18, 46, 30).unwrap();
        let prompt = format!(
            "<task-lifecycle transition=\"task_started\" task_id=\"cas-fe23\" \
             old=\"open\" new=\"in_progress\" actor=\"worker\" notification_id=\"3382\" \
             occurrence=\"{}\">\nstarted\n</task-lifecycle>",
            occurrence.to_rfc3339()
        );
        let decision = revalidate_lifecycle_prompt(
            &prompt,
            TaskStatus::Closed,
            occurrence + chrono::Duration::seconds(60),
        );

        assert_eq!(
            lifecycle_stale_outcome(&decision, false, false),
            LifecycleStaleOutcome::SuppressDelivered {
                task_id: "cas-fe23".to_string()
            }
        );
    }

    /// A still-true payload is untouched by the new decision layer — the
    /// delivery path must not change shape for the healthy case.
    #[test]
    fn a_live_relay_is_still_delivered() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 7, 18, 51, 51).unwrap();
        let prompt = awaiting_merge_prompt("cas-fe23", occurrence);
        let decision = revalidate_lifecycle_prompt(
            &prompt,
            TaskStatus::AwaitingMerge,
            occurrence + chrono::Duration::seconds(90),
        );

        assert_eq!(decision, LifecyclePromptDecision::Deliver);
        assert_eq!(
            lifecycle_stale_outcome(&decision, true, false),
            LifecycleStaleOutcome::Deliver
        );
    }

    /// The notice a supervisor actually reads must name the task and refuse to
    /// imply the work was handled.
    #[test]
    fn the_undelivered_notice_names_the_task_and_refuses_to_imply_success() {
        let notice =
            undelivered_relay_notice("cas-edee", Some("task_awaiting_merge: cas-edee (19:26)"));
        assert!(notice.contains("cas-edee"), "must name the task");
        assert!(
            notice.contains("UNDELIVERED"),
            "must state the failure, not describe a queue stage"
        );
        assert!(
            notice.contains("do not assume it was handled"),
            "silence must not read as success"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// cas-f02b (GH #101): a parked task keeps accruing writes while it waits
    /// (a close retry setting `merge_conflicted`, a note, a dependency edit).
    /// Occurrence equality would call the park's own notification stale and
    /// drop it before transport — silently reproducing the stall the
    /// notification exists to prevent.
    #[test]
    fn awaiting_merge_notice_survives_writes_while_the_task_stays_parked() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 6, 2, 10, 0).unwrap();
        let prompt = format!(
            "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-f02b\" \
             old=\"in_progress\" new=\"awaiting_merge\" actor=\"swift-fox\" \
             notification_id=\"41\" occurrence=\"{}\">\nparked\n</task-lifecycle>",
            occurrence.to_rfc3339()
        );

        // Same occurrence: delivered, as before.
        assert!(matches!(
            revalidate_lifecycle_prompt(&prompt, TaskStatus::AwaitingMerge, occurrence),
            LifecyclePromptDecision::Deliver
        ));

        // Later write while STILL parked: still true, still delivered.
        let later = occurrence + chrono::Duration::seconds(90);
        assert!(
            matches!(
                revalidate_lifecycle_prompt(&prompt, TaskStatus::AwaitingMerge, later),
                LifecyclePromptDecision::Deliver
            ),
            "a write while the task stays parked must not silence the merge signal"
        );

        // Left the state: genuinely stale, suppressed.
        assert!(matches!(
            revalidate_lifecycle_prompt(&prompt, TaskStatus::Closed, later),
            LifecyclePromptDecision::SuppressStale { .. }
        ));
    }

    /// Transient transitions keep the strict occurrence test — one
    /// Open→InProgress cycle must not be confirmed by the next one's write.
    #[test]
    fn transient_transitions_still_require_matching_occurrence() {
        let occurrence = Utc.with_ymd_and_hms(2026, 8, 6, 2, 10, 0).unwrap();
        let prompt = format!(
            "<task-lifecycle transition=\"task_started\" task_id=\"cas-x\" old=\"open\" \
             new=\"in_progress\" actor=\"w\" notification_id=\"1\" occurrence=\"{}\">\n\
             </task-lifecycle>",
            occurrence.to_rfc3339()
        );
        assert!(matches!(
            revalidate_lifecycle_prompt(
                &prompt,
                TaskStatus::InProgress,
                occurrence + chrono::Duration::seconds(5)
            ),
            LifecyclePromptDecision::SuppressStale { .. }
        ));
    }

    use chrono::{TimeZone, Utc};
    use std::process::Command;

    fn git(repo: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn merge_then_stale_request_is_suppressed_with_current_target_tip() {
        let repo = tempfile::tempdir().expect("temp repo");
        git(repo.path(), &["init", "-b", "main"]);
        git(
            repo.path(),
            &["config", "user.email", "cas-test@example.invalid"],
        );
        git(repo.path(), &["config", "user.name", "CAS Test"]);
        std::fs::write(repo.path().join("base"), "base\n").expect("base file");
        git(repo.path(), &["add", "base"]);
        git(repo.path(), &["commit", "-m", "base"]);
        git(repo.path(), &["checkout", "-b", "factory/test-worker"]);
        std::fs::write(repo.path().join("work"), "work\n").expect("work file");
        git(repo.path(), &["add", "work"]);
        git(repo.path(), &["commit", "-m", "work"]);
        let worker_tip = git(repo.path(), &["rev-parse", "factory/test-worker"]);

        // Reproduce the production ordering: the supervisor merges first, but
        // the worker's already-composed merge request reaches delivery later.
        git(repo.path(), &["checkout", "main"]);
        git(
            repo.path(),
            &["merge", "--no-ff", "factory/test-worker", "-m", "merge"],
        );
        let target_tip = git(repo.path(), &["rev-parse", "main"]);

        assert_eq!(
            revalidate_merge_request(repo.path(), &worker_tip, "main"),
            MergeRequestDecision::AlreadyIntegrated {
                target_tip: target_tip.clone(),
            }
        );

        let envelope = MergeRequestEnvelope {
            task_id: "cas-test".to_string(),
            branch_tip: worker_tip,
            target_branch: "main".to_string(),
            target_branch_tip: target_tip,
        };
        assert_eq!(
            parse_merge_request_envelope(&attach_merge_request_envelope("please merge", &envelope)),
            Some(envelope)
        );
        let guidance = merge_landed_guidance(
            "cas-test",
            &git(repo.path(), &["rev-parse", "factory/test-worker"]),
            "main",
            &git(repo.path(), &["rev-parse", "main"]),
        );
        assert!(guidance.contains("Merge already landed"));
        assert!(guidance.contains("Re-run task close for cas-test now"));
    }

    #[test]
    fn genuinely_unmerged_request_remains_pending() {
        let repo = tempfile::tempdir().expect("temp repo");
        git(repo.path(), &["init", "-b", "main"]);
        git(
            repo.path(),
            &["config", "user.email", "cas-test@example.invalid"],
        );
        git(repo.path(), &["config", "user.name", "CAS Test"]);
        std::fs::write(repo.path().join("base"), "base\n").expect("base file");
        git(repo.path(), &["add", "base"]);
        git(repo.path(), &["commit", "-m", "base"]);
        let target_tip = git(repo.path(), &["rev-parse", "main"]);
        git(repo.path(), &["checkout", "-b", "factory/test-worker"]);
        std::fs::write(repo.path().join("work"), "work\n").expect("work file");
        git(repo.path(), &["add", "work"]);
        git(repo.path(), &["commit", "-m", "work"]);
        let worker_tip = git(repo.path(), &["rev-parse", "factory/test-worker"]);

        assert_eq!(
            revalidate_merge_request(repo.path(), &worker_tip, "main"),
            MergeRequestDecision::Pending { target_tip }
        );
    }

    #[test]
    fn stale_blocked_occurrence_is_suppressed_after_task_resumes() {
        let occurrence = Utc
            .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
            .single()
            .expect("timestamp");
        let prompt = format!(
            "<task-lifecycle transition=\"task_blocked\" task_id=\"cas-test\" old=\"in_progress\" new=\"blocked\" actor=\"worker\" notification_id=\"1\" occurrence=\"{}\">\nTask blocked\n</task-lifecycle>",
            occurrence.to_rfc3339()
        );
        let current_updated_at = occurrence + chrono::Duration::seconds(1);

        assert_eq!(
            revalidate_lifecycle_prompt(&prompt, TaskStatus::InProgress, current_updated_at),
            LifecyclePromptDecision::SuppressStale {
                task_id: "cas-test".to_string(),
            }
        );
    }

    #[test]
    fn current_blocked_occurrence_is_delivered() {
        let occurrence = Utc
            .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
            .single()
            .expect("timestamp");
        let prompt = format!(
            "<task-lifecycle transition=\"task_blocked\" task_id=\"cas-test\" old=\"in_progress\" new=\"blocked\" actor=\"worker\" notification_id=\"1\" occurrence=\"{}\">\nTask blocked\n</task-lifecycle>",
            occurrence.to_rfc3339()
        );
        assert_eq!(
            revalidate_lifecycle_prompt(&prompt, TaskStatus::Blocked, occurrence),
            LifecyclePromptDecision::Deliver
        );
    }

    #[test]
    fn unstructured_messages_are_not_revalidated() {
        assert_eq!(
            parse_merge_request_envelope("ordinary free-form message"),
            None
        );
        assert_eq!(parse_lifecycle_envelope("ordinary free-form message"), None);
    }

    /// cas-6eab / GH #61: the reported sequence — supervisor merges and
    /// replies, THEN the worker's already-composed request reaches transport.
    /// It must be suppressed rather than delivered as an actionable ask.
    #[test]
    fn merge_request_delivered_after_the_merge_is_suppressed() {
        assert_eq!(
            merge_request_delivery_decision(
                Some(TaskStatus::AwaitingMerge),
                &MergeRequestDecision::AlreadyIntegrated {
                    target_tip: "abc123".to_string(),
                },
            ),
            MergeRequestDelivery::SuppressLanded {
                target_tip: "abc123".to_string(),
            }
        );
    }

    /// The same class one step further along: the merge landed AND the task
    /// was already re-closed. Status alone settles it without trusting git.
    #[test]
    fn merge_request_for_a_task_that_left_awaiting_merge_is_moot() {
        for status in [
            TaskStatus::Closed,
            TaskStatus::InProgress,
            TaskStatus::Open,
            TaskStatus::PendingSupervisorReview,
        ] {
            assert_eq!(
                merge_request_delivery_decision(
                    Some(status),
                    &MergeRequestDecision::Pending {
                        target_tip: "abc123".to_string(),
                    },
                ),
                MergeRequestDelivery::SuppressResolved { status },
                "a task at {status} has nothing queued for the supervisor"
            );
        }
        let guidance = merge_request_moot_guidance("cas-test", TaskStatus::Closed);
        assert!(guidance.contains("cas-test"));
        assert!(guidance.contains("no longer awaiting a merge"));
    }

    /// A genuinely outstanding merge must still reach the supervisor — and so
    /// must anything CAS cannot verify. Suppression requires positive evidence.
    #[test]
    fn outstanding_and_unverifiable_merge_requests_are_delivered() {
        assert_eq!(
            merge_request_delivery_decision(
                Some(TaskStatus::AwaitingMerge),
                &MergeRequestDecision::Pending {
                    target_tip: "abc123".to_string(),
                },
            ),
            MergeRequestDelivery::Deliver
        );
        assert_eq!(
            merge_request_delivery_decision(
                Some(TaskStatus::AwaitingMerge),
                &MergeRequestDecision::Unverifiable,
            ),
            MergeRequestDelivery::Deliver,
            "unverifiable git state must never suppress a merge request"
        );
        assert_eq!(
            merge_request_delivery_decision(None, &MergeRequestDecision::Unverifiable),
            MergeRequestDelivery::Deliver,
            "an unreadable task is uncertainty, not evidence of staleness"
        );
        assert_eq!(
            merge_request_delivery_decision(
                None,
                &MergeRequestDecision::AlreadyIntegrated {
                    target_tip: "abc123".to_string(),
                },
            ),
            MergeRequestDelivery::SuppressLanded {
                target_tip: "abc123".to_string(),
            },
            "git reachability is positive evidence even when the task is unreadable"
        );
    }

    #[test]
    fn exactly_one_parked_task_is_inferred_but_ambiguity_is_left_unstructured() {
        let mut first = cas_types::Task::new("cas-one".to_string(), "one".to_string());
        first.status = TaskStatus::AwaitingMerge;
        first.assignee = Some("worker-a".to_string());
        let mut second = cas_types::Task::new("cas-two".to_string(), "two".to_string());
        second.status = TaskStatus::AwaitingMerge;
        second.assignee = Some("worker-a".to_string());

        assert_eq!(
            select_unambiguous_merge_task(&[first.clone()], "worker-a", None)
                .map(|task| task.id.as_str()),
            Some("cas-one")
        );
        assert!(
            select_unambiguous_merge_task(&[first.clone(), second], "worker-a", None).is_none(),
            "ambiguous implicit merge context must preserve free-form delivery"
        );
        assert_eq!(
            select_unambiguous_merge_task(&[first], "worker-a", Some("cas-one"))
                .map(|task| task.id.as_str()),
            Some("cas-one")
        );
    }
}
