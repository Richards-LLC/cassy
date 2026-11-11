//! One renderer per close-gate refusal family (cas-90e8, audit M73).
//!
//! Before this module each family was a hand-copied `format!` at every site
//! that could refuse, and the copies drifted: different steps, missing
//! arguments, commands the recipient could not run. Every renderer here takes
//! the tool prefixes explicitly, so the text is a pure function of its inputs
//! and caller data and safety boundaries are exercised below.
//!
//! Rules every renderer follows:
//! - every suggested call is complete and carries its harness prefix;
//! - every suggested call is one the recipient can make;
//! - free text from the caller (a close reason) is never re-fenced.

use sha2::Digest as _;

/// Persist a full routine receipt under its content hash and return a compact
/// reference. The create-only path makes the detail immutable.
pub(crate) fn compact_receipt(
    cas_root: &std::path::Path,
    configured_artifacts_root: Option<&str>,
    task_id: &str,
    kind: &str,
    summary: &str,
    detail: &str,
) -> Result<String, String> {
    let digest = hex::encode(sha2::Sha256::digest(detail.as_bytes()));
    let paths = crate::config::resolved_factory_artifact_paths(cas_root, configured_artifacts_root);
    let dir = paths.task_dirs(task_id)[0].join("receipts");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("close receipt detail: could not create directory {}: {error}", dir.display()))?;
    let path = dir.join(format!("{kind}-{digest}.txt"));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            use std::io::Write as _;
            file.write_all(detail.as_bytes())
                .map_err(|error| format!("close receipt detail: could not write {}: {error}", path.display()))?;
            file.sync_all()
                .map_err(|error| format!("close receipt detail: could not sync {}: {error}", path.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = std::fs::read(&path)
                .map_err(|error| format!("close receipt detail: could not read {}: {error}", path.display()))?;
            if existing != detail.as_bytes() {
                return Err("content-addressed receipt detail did not match its path".into());
            }
        }
        Err(error) => return Err(format!("close receipt detail: could not create {}: {error}", path.display())),
    }
    Ok(format!(
        "{summary}\nDetails: {} (SHA-256 {digest})",
        path.display()
    ))
}

/// Keep advisory upkeep in the existing receipt channel. Failure to write the
/// file leaves the task's durable upkeep notes as the fallback, never a nag.
pub(crate) fn upkeep_receipt(
    cas_root: &std::path::Path,
    configured_artifacts_root: Option<&str>,
    task_id: &str,
    kind: &str,
    visible: String,
    notices: &[String],
) -> String {
    if notices.is_empty() { return visible; }
    let detail = format!("{visible}\n\n{}", notices.join("\n"));
    compact_receipt(cas_root, configured_artifacts_root, task_id, kind, &visible, &detail)
        .unwrap_or_else(|error| {
            tracing::warn!(task_id, %error, "upkeep receipt detail unavailable; retained in task notes");
            visible
        })
}

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
    verification_type: cas_types::VerificationType,
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
                 1. A registered supervisor reviews the exact dispatch and records the verdict \
                 directly: `{supervisor_prefix}verification action=add task_id={task_id} \
                 dispatch_id={dispatch_id} verification_type={verification_type} \
                 status=approved summary=\"...\"`\n\
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
             dispatch_id=<dispatch id named in that response> verification_type={verification_type} \
             status=approved summary=\"...\"`\n\
             3. Then retry {retry}."
        ),
    }
}

/// A shared stop for repeatable close refusals. This is advice only: the gate
/// does not count attempts or change its decision based on this paragraph.
pub(crate) fn repeated_close_refusal_stop(task_id: &str, caller_prefix: &str) -> String {
    format!(
        "If this gate has now refused you twice, stop fixing. Write down the premise you are \
         working from as a task note, take a census of what the gate actually checks versus what \
         you changed, then act. To record the premise, run `{caller_prefix}task action=notes \
         id={task_id} note_type=decision notes=\"<premise>\"`."
    )
}

/// G1–G6 share the same complete escape/retry contract, in the caller's harness.
pub(crate) fn upkeep_message(
    task_id: &str,
    input_path: &str,
    title: &str,
    detail: &str,
    escape_token: &str,
    warnings: &[String],
    caller_prefix: &str,
) -> String {
    // Routine documentation advisories remain in the note/receipt writer.
    // Keep analysis context that explains an actual blocking finding.
    let warnings = warnings.iter()
        .filter(|warning| !warning.starts_with("UPKEEP ADVISORY G5")
            && !warning.contains("attributed delivery base unavailable"))
        .map(|warning| format!("\n{warning}")).collect::<String>();
    format!(
        "⚠️ {title}\n\n{detail}{warnings}\n\nNext: Fix {input_path} in this branch, or record `{caller_prefix}task action=notes id={task_id} note_type=decision notes=\"{escape_token} <reason>\"` with a non-empty reason. Then retry `{caller_prefix}task action=close id={task_id}`."
    )
}

/// How to find, check and present a `commit_receipt` — shared by every
/// refusal that asks for one.
pub(crate) fn commit_receipt_recovery_steps(
    task_id: &str,
    parent_branch: &str,
    caller_prefix: &str,
) -> String {
    let [log, show, ancestry] = match crate::worktree::GitOperations::git_remedies([
        &["log", "--oneline", "--all"],
        &["show", "--stat", "<sha>"],
        &["merge-base", "--is-ancestor", "<sha>", parent_branch],
    ]) {
        Ok(commands) => commands,
        Err(refusal) => return refusal,
    };
    format!(
        "find the SHA of the worker task commit OR the merge commit that carried this \
         task's work (never an unrelated historical commit; `{log}` \
         lists candidates), verify it with `{show}` and \
         `{ancestry}`, then retry \
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
         negative_result_artifact_path=<absolute-path-under-artifacts_root/<project-key>/{task_id}> \
         negative_result_reference=<closed-PR-URL-or-branch> reason=\"...\"`. For successful \
         docs/artifacts-only reports intentionally not integrated, a registered supervisor \
         may use `{supervisor_prefix}task action=close id={task_id} evidence_only=true \
         evidence_only_artifact_path=<absolute-path-under-artifacts_root/<project-key>/{task_id}> \
         evidence_only_reference=<PR-URL-or-branch> reason=\"...\"`; CAS measures and logs \
         the immutable delivery's net base..tip diff. A non-evidence path that was \
         touched and fully reverted does not disqualify the report; no history rewrite \
         is required. If non-evidence changes remain in that net diff and the \
         supervisor declines the delivery for rework, it runs `{supervisor_prefix}task \
         action=request_changes id={task_id} reason=\"...\"`; only after that verdict \
         may the assigned worker start a fresh cycle."
    )
}

/// cas-5597: the one answer to "who closes a worker's task", shared by every
/// worker-facing refusal; the parity test pins it against the skill, launch
/// contract and assignment text.
pub(crate) const WORKER_CLOSE_OWNERSHIP: &str = "Your close parks the delivery; the merge verdict closes it. Never re-close, spawn a verifier or ask anyone to close for you.";

pub(crate) const SUPERVISOR_CLOSE_RECOVERY: &str = "Cassy closes eligible merged deliveries on the verdict; the supervisor owns any remaining close recovery, including no-code deliveries, and the worker never re-closes.";
pub(crate) const BATCH_CLOSE_RULE: &str = "In qa.mode=batch, Cassy closes merged deliveries covered by the release verdict through ordinary gates; a batch QA receipt alone does not replace that verdict, and the worker does nothing.";

/// cas-5597: a successful park answers the worker as done. It keeps the
/// `MERGE REQUIRED` head that runtime detectors read, drops the merge/retry
/// remediation meant for genuine refusals, and keeps any conflict finding.
pub(crate) fn parked_reply(merge_required: &str, tip: Option<&str>, branch: Option<&str>, target: &str) -> String {
    // Other parked outcomes (e.g. DELIVERY CONTENT DROPPED) keep their own text.
    if !super::gate_preflight::is_merge_handoff(merge_required) {
        return merge_required.to_string();
    }
    let conflict = ["\n\n⚠️ This branch has a genuine", "\n\n⚠️ Cassy could not determine"]
        .iter()
        .find_map(|marker| merge_required.find(marker))
        .map_or("", |at| &merge_required[at..]);
    format!(
        "⚠️ MERGE REQUIRED: PARKED {} on {} for {target}; awaiting merge verdict; nothing more to do.\n\n\
         {WORKER_CLOSE_OWNERSHIP} {PARK_SIGNAL}{conflict}",
        tip.unwrap_or("the pushed tip"),
        branch.unwrap_or("the factory branch"),
    )
}

/// A factory worker's close parked on a verification dispatch (cas-8725).
pub(crate) struct WorkerDispatchGate<'a> {
    pub task_id: &'a str,
    pub dispatch_id: &'a str,
    pub owner: &'a str,
    pub deadline: &'a str,
    /// Bound head and superseded-verdict sentences, already formatted.
    pub context: &'a str,
    /// Whether Cassy queued the supervisor handoff itself.
    pub handoff_queued: bool,
    pub coord: &'a str,
    pub close_reason_hint: &'a str,
}

pub(crate) fn worker_dispatch_gate(gate: &WorkerDispatchGate<'_>) -> String {
    let WorkerDispatchGate { task_id: id, dispatch_id, owner, deadline, context, handoff_queued, coord, close_reason_hint } = gate;
    // cas-8725: when CAS queued the handoff itself, say so — a worker told to
    // "forward this" sends a second, free-text copy that cannot wake anyone.
    // cas-abb0: remaining close recovery belongs to the supervisor.
    let handoff = if *handoff_queued {
        format!("Cassy has already delivered this dispatch to the supervisor; send no {coord} message for it.")
    } else {
        format!(
            "Cassy could not deliver this dispatch; send one blocker: {coord} action=message \
             target=supervisor task_id={id} blocker=true summary=\"Verification dispatch {dispatch_id}\" \
             message=\"Task {id} waits on verification dispatch {dispatch_id}.{close_reason_hint}\""
        )
    };
    format!(
        "Factory worker verification gate: task {id} close is pending \
         dispatch {dispatch_id}, owned by {owner}, deadline {deadline}. \
         {context}{handoff}\n\n{SUPERVISOR_CLOSE_RECOVERY} {WORKER_CLOSE_OWNERSHIP}"
    )
}

/// A worker's close that found another requester's dispatch already pending.
pub(crate) fn worker_pending_dispatch(task_id: &str, dispatch_id: &str, owner: &str, deadline: &str, conflict: &str) -> String {
    format!(
        "Task {task_id} close is pending existing dispatch {dispatch_id}, owned by {owner}, deadline {deadline}. {conflict} {SUPERVISOR_CLOSE_RECOVERY} {WORKER_CLOSE_OWNERSHIP}"
    )
}

/// `task action=start` on a clean parked delivery.
pub(crate) fn awaiting_merge_start_refusal(task_id: &str, tool_prefix: &str) -> String {
    format!(
        "Cannot start a task that is awaiting merge. The worker work is \
         already complete; wait for the supervisor to merge the factory branch; \
         the merge verdict closes the task. If review fails — the supervisor \
         declines the delivery, or requires an amendment after merging — \
         they must first run \
         `{tool_prefix}task action=request_changes id={task_id} reason=\"state what remains and what must be corrected or reverted\"`; \
         a worker cannot self-reject or start a clean parked delivery. When \
         the parked tip genuinely conflicts with the current target (a \
         merge-queue conflict dequeue or a moved target), its assigned worker \
         can start task {task_id} to resolve it; Cassy measures the local target and \
         origin/<target>, so `git fetch origin <target>` first if that ref is stale."
    )
}

/// cas-5597: the one completion signal a worker sends (or does not).
pub(crate) const PARK_SIGNAL: &str = "The park relay is the completion signal; send merge_request=true only for local_merge.";

/// A Tier 1 delivery in release QA mode, waiting on the one verdict that names it.
pub(crate) fn waiting_for_release_verdict(task_id: &str, tip: &str, supervisor_prefix: &str) -> String {
    format!(
        "⏳ WAITING FOR RELEASE VERDICT\n\nTask {task_id} is Tier 1 in qa.mode=batch at {tip}, so it opens no verifier of its own. The supervisor runs one review of the assembled tip, publishes a report naming every delivery, then records {supervisor_prefix}verification action=batch_receipt verification_type=release task_ids=[...] staging_sha=<tip> ledger_artifact_id=<id> ledger_path=<path> ledger_author=<reviewer>. {BATCH_CLOSE_RULE}"
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
    use tempfile::tempdir;


    #[test]
    fn close_receipt_is_compact_and_references_immutable_details() {
        let root = tempdir().unwrap();
        let detail = "Closed task cas-x; verification passed; full diagnostic payload.";
        let short = compact_receipt(
            root.path(),
            Some(root.path().to_str().unwrap()),
            "cas-x",
            "close",
            "Closed task cas-x; delivery abc123; target epic/x; next: none.",
            detail,
        )
        .unwrap();
        assert!(short.len() < 800, "{short}");
        let paths = crate::config::resolved_factory_artifact_paths(root.path(), Some(root.path().to_str().unwrap()));
        let receipt_dir = paths.task_dirs("cas-x")[0].join("receipts");
        let files: Vec<_> = std::fs::read_dir(receipt_dir).unwrap().map(|entry| entry.unwrap().path()).collect();
        assert_eq!(files.len(), 1);
        let path = &files[0];
        assert_eq!(std::fs::read_to_string(path).unwrap(), detail);
        let digest = hex::encode(sha2::Sha256::digest(detail.as_bytes()));
        assert!(short.contains(path.to_str().unwrap()) && short.contains(&digest));
        assert!(compact_receipt(
            root.path(),
            Some(root.path().to_str().unwrap()),
            "cas-x",
            "close",
            "retry",
            detail
        )
        .is_ok());
        assert_eq!(std::fs::read_to_string(path).unwrap(), detail);
    }

    /// Every suggested `task`/`verification`/`coordination` call in close-gate
    /// runtime text carries a harness prefix (literal or `{…}` placeholder).

    /// A string literal line ending in `\\` renders a literal backslash plus
    /// the next line's indentation instead of joining the lines.


    #[test]
    fn commit_receipt_steps_find_verify_and_retry_with_a_complete_command() {
        let special = commit_receipt_recovery_steps("cas-r1", "epic/{a,b}!e", "mcp__cs__");
        // pin: ancestry instructions are copyable argv, including the SHA placeholder.
        assert!(special.contains("git merge-base --is-ancestor '<sha>' 'epic/{a,b}!e'"), "{special}");
        let unsafe_ref = commit_receipt_recovery_steps("cas-r1", "epic/control\u{85}label", "mcp__cs__");
        assert!(unsafe_ref.contains("\\u{85}") && !unsafe_ref.contains("git ")
            && !unsafe_ref.contains("mcp__cs__task"), "{unsafe_ref:?}");

    }



    #[test]
    fn worker_verification_required_is_the_handoff_only() {
        let text = worker_verification_required("cas-v1", "GATE LINE");
        assert!(text.contains("cas-v1") && text.contains("GATE LINE"));
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

    }
}
