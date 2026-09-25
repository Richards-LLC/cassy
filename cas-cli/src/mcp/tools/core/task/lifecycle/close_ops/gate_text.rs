//! One renderer per close-gate refusal family (cas-90e8, audit M73).
//!
//! Before this module each family was a hand-copied `format!` at every site
//! that could refuse, and the copies drifted: different steps, missing
//! arguments, commands the recipient could not run. Every renderer here takes
//! the tool prefixes explicitly, so the text is a pure function of its inputs
//! and each family is pinned by the tests at the bottom of this file.
//!
//! Rules every renderer follows:
//! - every suggested call is complete and carries its harness prefix;
//! - every suggested call is one the recipient can make;
//! - free text from the caller (a close reason) is never re-fenced.

/// What the timed-out verification cycle was bound to.
#[derive(Debug, Clone, Copy)]
pub(crate) enum VerificationTimeout<'a> {
    /// A typed dispatch exists and has been marked `timed_out`.
    Dispatch {
        dispatch_id: &'a str,
        waited_mins: Option<i64>,
        recovery_action: Option<&'a str>,
    },
    /// A legacy untyped dispatch row timed out; no dispatch id exists yet.
    Legacy { waited_mins: i64 },
}

/// VERIFICATION TIMED OUT, for every timeout path in the close gate.
pub(crate) fn verification_timeout_message(
    task_id: &str,
    timeout: VerificationTimeout<'_>,
    supervisor_prefix: &str,
    caller_prefix: &str,
) -> String {
    let retry = format!("`{caller_prefix}task action=close id={task_id}`");
    match timeout {
        VerificationTimeout::Dispatch {
            dispatch_id,
            waited_mins,
            recovery_action,
        } => {
            let waited = waited_mins
                .map(|mins| format!(" after {mins} minutes"))
                .unwrap_or_default();
            let recovery = recovery_action
                .map(|action| format!(" ({action})"))
                .unwrap_or_default();
            format!(
                "⚠️ VERIFICATION TIMED OUT\n\n\
                 Task {task_id} exact dispatch {dispatch_id} timed out{waited} without a \
                 verdict. Its lease was released; a registered supervisor recovers it.\n\n\
                 Recovery{recovery}:\n\
                 1. Re-dispatch the task-verifier for task {task_id}, or record the verdict \
                 directly: `{supervisor_prefix}verification action=add task_id={task_id} \
                 dispatch_id={dispatch_id} status=approved summary=\"...\"`\n\
                 2. Then retry {retry}."
            )
        }
        VerificationTimeout::Legacy { waited_mins } => format!(
            "⚠️ VERIFICATION TIMED OUT\n\n\
             Task {task_id} waited {waited_mins} minutes for a verdict that never came, \
             usually because the verifier was never spawned or crashed. Its pending \
             verification was cleared and its lease released.\n\n\
             Recovery:\n\
             1. Retry {retry} to mint a verification dispatch.\n\
             2. Re-dispatch the task-verifier for task {task_id}, or record the verdict \
             directly: `{supervisor_prefix}verification action=add task_id={task_id} \
             dispatch_id=<dispatch id named in that response> status=approved summary=\"...\"`\n\
             3. Then retry {retry}."
        ),
    }
}

/// How to find, check and present a `commit_receipt` — shared by every
/// refusal that asks for one.
pub(crate) fn commit_receipt_recovery_steps(
    task_id: &str,
    parent_branch: &str,
    caller_prefix: &str,
) -> String {
    format!(
        "find the SHA of the worker task commit OR the merge commit that carried this \
         task's work (never an unrelated historical commit; `git log --oneline --all` \
         lists candidates), verify it with `git show --stat <sha>` and \
         `git merge-base --is-ancestor <sha> {parent_branch}`, then retry \
         `{caller_prefix}task action=close id={task_id} commit_receipt=<sha>` \
         (full SHA or an unambiguous abbreviation)."
    )
}

/// Freshness step for merge-gate remediations: drain the inbox before
/// asking for a merge that may already have happened.
pub(crate) fn inbox_drain_step(caller_prefix: &str) -> String {
    format!(
        "Run `{caller_prefix}coordination action=inbox_poll` until it returns \
         `No unread messages`. If a reply says this branch was merged or asks for \
         changes, follow it instead of the steps below."
    )
}

/// The merge-request call a worker sends to have its branch merged. It names
/// the branch tip; the wording of `message` is the worker's own.
pub(crate) fn merge_request_call(
    caller_prefix: &str,
    task_id: &str,
    factory_branch: &str,
    branch_tip: &str,
) -> String {
    format!(
        "`{caller_prefix}coordination action=message target=supervisor task_id={task_id} \
         merge_request=true summary=\"...\" message=\"...\"` naming {factory_branch} tip \
         {branch_tip}"
    )
}

/// The closing paragraph shared by merge-gate refusals: the two sanctioned
/// supervisor exits when the delivery is not going to land.
pub(crate) fn merge_gate_exits_paragraph(task_id: &str, supervisor_prefix: &str) -> String {
    format!(
        "If this is a completed, measured negative result whose delivery must not \
         land, a registered supervisor may close with `{supervisor_prefix}task \
         action=close id={task_id} negative_result=true \
         negative_result_artifact_path=<absolute-path-under-artifacts_root/{task_id}> \
         negative_result_reference=<closed-PR-URL-or-branch> reason=\"...\"`. If the \
         supervisor declines the delivery for rework, it runs `{supervisor_prefix}task \
         action=request_changes id={task_id} reason=\"...\"`; only after that verdict \
         may the assigned worker start a fresh cycle."
    )
}

/// VERIFICATION REQUIRED as a factory worker sees it: the gate line with its
/// handoff, and nothing addressed to the verifier or the supervisor.
pub(crate) fn worker_verification_required(task_id: &str, gate_line: &str) -> String {
    format!("⚠️ VERIFICATION REQUIRED\n\nTask {task_id} requires verification before closing.\n\n{gate_line}")
}

/// The proposed close reason as the verifier sees it. The reason is caller
/// free text, so it is quoted inline on one line and never fenced: a fenced
/// echo can nest fences, which crashes the Claude Code renderer.
pub(crate) fn proposed_close_reason_line(reason: &str, verifier_agent: &str) -> String {
    let one_line = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "Proposed close reason: \"{}\". The {verifier_agent} must reject it if it admits \
         incomplete work (for example 'remaining items', 'beyond scope', 'will need to').",
        one_line.replace('`', "'")
    )
}

/// Why a task-verifier spawn was refused, as the spawning agent sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerifierSpawnDenial {
    RegistryUnavailable,
    UnregisteredParent,
    InactiveParent,
    MissingPrompt,
    NoUniqueTask,
    DispatchOwnedElsewhere,
    DispatchDeadlineElapsed,
    NoActiveDispatch,
    DispatchUnreadable,
    MissingToolUseId,
    SpawnAlreadyPending,
    HandoffFailed,
}

/// Verifier-authority denial with the next call the spawning agent can make.
pub(crate) fn verifier_spawn_denial(
    denial: VerifierSpawnDenial,
    task_id: Option<&str>,
    prefix: &str,
) -> String {
    let task = task_id.unwrap_or("<task-id>");
    let close = format!("`{prefix}task action=close id={task}`");
    let (problem, next) = match denial {
        VerifierSpawnDenial::RegistryUnavailable => (
            "Cannot establish verifier authority: the agent registry is unavailable.".to_string(),
            "run `cas doctor`, then retry the spawn.".to_string(),
        ),
        VerifierSpawnDenial::UnregisteredParent => (
            "Cannot establish verifier authority for an anonymous or orphan session.".to_string(),
            format!(
                "run `{prefix}coordination action=whoami`; only a registered session that owns \
                 the task's dispatch can spawn its verifier."
            ),
        ),
        VerifierSpawnDenial::InactiveParent => (
            "Cannot establish verifier authority for an inactive parent session.".to_string(),
            format!("run `{prefix}coordination action=heartbeat`, then retry the spawn."),
        ),
        VerifierSpawnDenial::MissingPrompt | VerifierSpawnDenial::NoUniqueTask => (
            "The task-verifier prompt must name exactly one existing Cassy task ID.".to_string(),
            "retry the spawn with prompt=\"Verify task <task-id>\".".to_string(),
        ),
        VerifierSpawnDenial::DispatchOwnedElsewhere => (
            "This task's verification dispatch is owned by another registered session.".to_string(),
            format!(
                "leave the verdict to that owner (`{prefix}task action=show id={task}` names \
                 it); do not spawn a second verifier."
            ),
        ),
        VerifierSpawnDenial::DispatchDeadlineElapsed => (
            "This task's verification dispatch deadline has elapsed.".to_string(),
            format!(
                "retry {close}; it marks the dispatch timed_out and prints the named-dispatch \
                 recovery verdict."
            ),
        ),
        VerifierSpawnDenial::NoActiveDispatch => (
            "No active verification dispatch owned by this session exists for this task."
                .to_string(),
            format!("retry {close} to mint the dispatch, then spawn the verifier."),
        ),
        VerifierSpawnDenial::DispatchUnreadable => (
            "Could not validate task-scoped verification dispatch authority.".to_string(),
            "retry the spawn; if it fails again, run `cas doctor`.".to_string(),
        ),
        VerifierSpawnDenial::MissingToolUseId => (
            "Cannot establish verifier authority: PreToolUse did not provide tool_use_id \
             correlation."
                .to_string(),
            "retry the spawn from the Task/Agent tool of the session that owns the dispatch."
                .to_string(),
        ),
        VerifierSpawnDenial::SpawnAlreadyPending => (
            "Another task-verifier spawn is already awaiting SubagentStart for this parent."
                .to_string(),
            "wait for it to bind, or retry after the failed spawn is cleaned up or expires."
                .to_string(),
        ),
        VerifierSpawnDenial::HandoffFailed => (
            "Could not establish server-side task-verifier authority for the exact dispatch."
                .to_string(),
            format!("retry {close} to refresh the dispatch, then spawn the verifier again."),
        ),
    };
    format!("{problem} Next: {next}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIXES: [&str; 4] = ["mcp__cas__", "mcp__cs__", "cas__", "cas_"];

    #[test]
    fn verification_timeout_always_names_a_complete_recovery_verdict() {
        for prefix in PREFIXES {
            let typed = verification_timeout_message(
                "cas-t1",
                VerificationTimeout::Dispatch {
                    dispatch_id: "vdispatch-1",
                    waited_mins: Some(31),
                    recovery_action: Some("redispatch_or_direct_verdict"),
                },
                prefix,
                "mcp__cas__",
            );
            assert!(typed.starts_with("⚠️ VERIFICATION TIMED OUT"));
            assert!(typed.contains("after 31 minutes"), "{typed}");
            assert!(typed.contains("(redispatch_or_direct_verdict)"), "{typed}");
            assert!(
                typed.contains(&format!(
                    "`{prefix}verification action=add task_id=cas-t1 dispatch_id=vdispatch-1 status=approved summary=\"...\"`"
                )),
                "{typed}"
            );
            assert!(typed.contains("`mcp__cas__task action=close id=cas-t1`"), "{typed}");
            assert!(!typed.contains("Task(subagent_type"), "{typed}");

            let bare = verification_timeout_message(
                "cas-t1",
                VerificationTimeout::Dispatch {
                    dispatch_id: "vdispatch-1",
                    waited_mins: None,
                    recovery_action: None,
                },
                prefix,
                prefix,
            );
            assert!(bare.contains("dispatch_id=vdispatch-1"), "{bare}");
            assert!(!bare.contains("Recovery ("), "{bare}");

            let legacy = verification_timeout_message(
                "cas-t1",
                VerificationTimeout::Legacy { waited_mins: 12 },
                prefix,
                prefix,
            );
            assert!(legacy.contains("waited 12 minutes"), "{legacy}");
            assert!(
                legacy.contains(&format!("{prefix}task action=close id=cas-t1` to mint")),
                "{legacy}"
            );
            assert!(legacy.contains("dispatch_id=<dispatch id named in that response>"));
        }
    }

    #[test]
    fn commit_receipt_steps_find_verify_and_retry_with_a_complete_command() {
        let steps = commit_receipt_recovery_steps("cas-r1", "epic/x", "mcp__cs__");
        for required in [
            "worker task commit OR the merge commit",
            "never an unrelated historical commit",
            "git log --oneline --all",
            "git show --stat <sha>",
            "git merge-base --is-ancestor <sha> epic/x",
            "`mcp__cs__task action=close id=cas-r1 commit_receipt=<sha>`",
            "unambiguous abbreviation",
        ] {
            assert!(steps.contains(required), "missing {required:?}: {steps}");
        }
    }

    #[test]
    fn merge_gate_pieces_are_short_and_complete() {
        let drain = inbox_drain_step("cas__");
        assert!(drain.contains("`cas__coordination action=inbox_poll`"));
        assert!(drain.contains("`No unread messages`"));
        for internal in ["at most 10 rows", "at-most-once", "daemon transport"] {
            assert!(!drain.contains(internal), "polling internals leaked: {drain}");
        }

        let request = merge_request_call("mcp__cas__", "cas-m1", "factory/w", "abc123");
        assert!(request.contains(
            "`mcp__cas__coordination action=message target=supervisor task_id=cas-m1 merge_request=true summary=\"...\" message=\"...\"`"
        ));
        assert!(request.contains("factory/w tip abc123"));

        let exits = merge_gate_exits_paragraph("cas-m1", "mcp__cs__");
        assert!(exits.contains("`mcp__cs__task action=close id=cas-m1 negative_result=true"));
        assert!(exits.contains("negative_result_artifact_path=<absolute-path-under-artifacts_root/cas-m1>"));
        assert!(exits.contains("negative_result_reference="));
        assert!(exits.contains("`mcp__cs__task action=request_changes id=cas-m1 reason=\"...\"`"));
    }

    #[test]
    fn worker_verification_required_is_the_handoff_only() {
        let text = worker_verification_required("cas-v1", "GATE LINE");
        assert_eq!(
            text,
            "⚠️ VERIFICATION REQUIRED\n\nTask cas-v1 requires verification before closing.\n\nGATE LINE"
        );
    }

    #[test]
    fn close_reason_is_never_fenced() {
        let line = proposed_close_reason_line(
            "done\n```rust\nfn x() {}\n```\nremaining items",
            "task-verifier",
        );
        assert!(!line.contains("```"), "{line}");
        assert!(!line.contains('\n'), "{line}");
        assert!(line.contains("remaining items"), "{line}");
        assert!(line.contains("task-verifier must reject"), "{line}");
    }

    #[test]
    fn every_verifier_spawn_denial_names_a_next_step() {
        use VerifierSpawnDenial::*;
        for denial in [
            RegistryUnavailable,
            UnregisteredParent,
            InactiveParent,
            MissingPrompt,
            NoUniqueTask,
            DispatchOwnedElsewhere,
            DispatchDeadlineElapsed,
            NoActiveDispatch,
            DispatchUnreadable,
            MissingToolUseId,
            SpawnAlreadyPending,
            HandoffFailed,
        ] {
            let text = verifier_spawn_denial(denial, Some("cas-d1"), "mcp__cs__");
            let (_, next) = text
                .split_once(" Next: ")
                .unwrap_or_else(|| panic!("{denial:?} names no next step: {text}"));
            assert!(!next.trim().is_empty(), "{denial:?}");
            assert!(!text.contains("recorded recovery path"), "{denial:?}: {text}");
            assert!(!text.contains("mcp__cas__"), "{denial:?}: {text}");
        }
        assert!(
            verifier_spawn_denial(NoActiveDispatch, Some("cas-d1"), "mcp__cs__")
                .contains("`mcp__cs__task action=close id=cas-d1` to mint the dispatch")
        );
        assert!(
            verifier_spawn_denial(DispatchDeadlineElapsed, None, "cas__")
                .contains("`cas__task action=close id=<task-id>`")
        );
    }
}
