//! Post-close / stale-instruction guards (cas-b269).
//!
//! Workers sometimes keep re-verifying and re-closing a task after it is
//! already `Closed` (stale transcript, replayed messages, or an urgent stop
//! that only interrupted the turn without invalidating tool-call paths).
//! These pure helpers enforce action-boundary revalidation:
//!
//! - close of an already-closed task is a no-op success (no re-verify)
//! - verification against a closed task is rejected
//! - urgent supervisor/director halt blocks further close/verify until a
//!   successful new start that does not race a newer halt generation
//!
//! Close-merge semantics and product code are out of scope.

use cas_types::TaskStatus;
use std::collections::HashMap;

/// Agent metadata key: when truthy, the worker must not run task close or
/// verification MCP until a successful `task start` clears it (urgent stop).
pub const HALT_TASK_WORK_META: &str = "halt_task_work";

/// Monotonic generation (unix millis) for the halt flag. A concurrent urgent
/// stop during `task start` writes a newer gen; start must not clear it.
pub const HALT_TASK_WORK_GEN_META: &str = "halt_task_work_gen";

/// Snapshot of one worker candidate for halt fan-out (pure tests + production).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HaltWorkerCandidate {
    pub name: String,
    pub factory_session: Option<String>,
}

/// Whether the task is in the terminal closed state for close/verify suppression.
pub fn is_terminal_closed(status: TaskStatus) -> bool {
    status == TaskStatus::Closed
}

/// Idempotent response when `task close` targets an already-closed task.
pub fn already_closed_close_message(task_id: &str) -> String {
    format!(
        "ALREADY CLOSED\n\n\
         Task {task_id} is already Closed. Do not re-verify or re-close it.\n\
         Await a new assignment (`task action=mine`) or an explicit new task start.\n\
         (cas-b269)"
    )
}

/// Error when recording verification against a closed task.
pub fn verification_on_closed_message(task_id: &str) -> String {
    format!(
        "Task {task_id} is already Closed — verification is not allowed. \
         Do not re-verify a closed task; await a new assignment (cas-b269)."
    )
}

/// Error when close/verify is attempted under an urgent halt flag.
pub fn halt_blocks_task_work_message(tool: &str) -> String {
    format!(
        "WORK HALTED: supervisor issued an urgent stop. \
         Refusing `{tool}` until a new task is started. \
         Call `task action=mine` and wait for assignment (cas-b269)."
    )
}

/// True when agent metadata marks task work as halted after urgent stop.
pub fn agent_task_work_halted(metadata: &HashMap<String, String>) -> bool {
    metadata
        .get(HALT_TASK_WORK_META)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Parse halt generation from metadata (unix millis). Missing/invalid → 0.
pub fn halt_generation(metadata: &HashMap<String, String>) -> u64 {
    metadata
        .get(HALT_TASK_WORK_GEN_META)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

/// Whether `task start` may clear halt given the generation present at clear
/// time and the ceiling captured after a successful start (unix millis).
///
/// Clears only when `stored_gen <= clear_ceiling`. A concurrent urgent halt
/// that wrote a *newer* gen must be preserved.
pub fn should_clear_halt_at_generation(stored_gen: u64, clear_ceiling: u64) -> bool {
    stored_gen <= clear_ceiling
}

/// Apply a new halt generation to metadata (sets flag + gen).
pub fn apply_halt_metadata(metadata: &mut HashMap<String, String>, generation: u64) {
    metadata.insert(HALT_TASK_WORK_META.to_string(), "1".to_string());
    metadata.insert(
        HALT_TASK_WORK_GEN_META.to_string(),
        generation.to_string(),
    );
}

/// Clear halt metadata keys.
pub fn clear_halt_metadata(metadata: &mut HashMap<String, String>) {
    metadata.remove(HALT_TASK_WORK_META);
    metadata.remove(HALT_TASK_WORK_GEN_META);
}

/// Whether the message **source** is authorized to set `halt_task_work`.
///
/// Authorize by role string (`supervisor` / `director` via `AgentRole`) and
/// by known display names (`supervisor`, `director`). Workers must not halt.
pub fn may_source_set_halt(source_display: &str, source_role: &str) -> bool {
    let role = source_role.eq_ignore_ascii_case("supervisor")
        || source_role.eq_ignore_ascii_case("director");
    let name = source_display.eq_ignore_ascii_case("supervisor")
        || source_display.eq_ignore_ascii_case("director");
    role || name
}

/// Same as [`may_source_set_halt`] but with typed `AgentRole` when known.
pub fn may_source_role_set_halt(role: cas_types::AgentRole) -> bool {
    matches!(
        role,
        cas_types::AgentRole::Supervisor | cas_types::AgentRole::Director
    )
}

/// Session-scope worker names visible to `factory_session` (strict match when
/// `Some`; unfiltered when `None` for pure tests / non-factory).
pub fn session_scoped_worker_names(
    workers: &[HaltWorkerCandidate],
    factory_session: Option<&str>,
) -> Vec<String> {
    workers
        .iter()
        .filter(|w| match factory_session {
            Some(session) => w.factory_session.as_deref() == Some(session),
            None => true,
        })
        .map(|w| w.name.clone())
        .collect()
}

/// Resolve which agent **names** should receive a durable halt for an urgent
/// message, scoped to the provided session-filtered worker name list.
///
/// - Never includes `supervisor`.
/// - `all_workers` expands to every provided (session-scoped) worker name.
/// - Single worker name only when present in the session-scoped list.
pub fn halt_targets_for_urgent(
    resolved_target: &str,
    session_worker_names: &[String],
) -> Vec<String> {
    if resolved_target.eq_ignore_ascii_case("supervisor") {
        return Vec::new();
    }
    if resolved_target.eq_ignore_ascii_case("all_workers") {
        return session_worker_names.to_vec();
    }
    session_worker_names
        .iter()
        .filter(|n| n.eq_ignore_ascii_case(resolved_target))
        .cloned()
        .collect()
}

/// Whether an urgent send should attempt durable halt for this source/target.
pub fn should_persist_urgent_halt(
    urgent: bool,
    source_display: &str,
    source_role: &str,
    resolved_target: &str,
    session_worker_names: &[String],
) -> bool {
    urgent
        && may_source_set_halt(source_display, source_role)
        && !halt_targets_for_urgent(resolved_target, session_worker_names).is_empty()
}

/// Heuristic: does this delivered prompt instruct close / re-verify work?
pub fn looks_like_close_or_verify_guidance(text: &str) -> bool {
    let lower = text.to_lowercase();
    const MARKERS: &[&str] = &[
        "task action=close",
        "action=close",
        "re-close",
        "reclose",
        "re-verif",
        "reverify",
        "verification required",
        "verify and close",
        "close the task",
        "close id=",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

/// If guidance still tells a worker to close/verify tasks that are already
/// closed, rewrite to an idle instruction. Returns `None` when no rewrite.
pub fn rewrite_stale_close_guidance(
    text: &str,
    closed_task_ids: &[&str],
) -> Option<String> {
    if closed_task_ids.is_empty() || !looks_like_close_or_verify_guidance(text) {
        return None;
    }
    let mentions_closed = closed_task_ids.iter().any(|id| text.contains(id));
    if !mentions_closed {
        return None;
    }
    let ids = closed_task_ids.join(", ");
    Some(format!(
        "STALE GUIDANCE SUPPRESSED (cas-b269)\n\n\
         Task(s) {ids} are already Closed. Do not re-verify or re-close.\n\
         Idle and await a new assignment (`task action=mine`).\n\n\
         --- original message (for audit) ---\n{text}"
    ))
}

/// Whether a byte can be part of a CAS task-id run (`cas-<hex>`): ASCII
/// alphanumeric or `-`. Used to reject prefix/suffix near-matches so that
/// `cas-5c02x` (trailing alnum) or `xcas-5c02` (leading alnum) do NOT count as
/// a mention of `cas-5c02`.
fn is_task_id_boundary_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

/// Whether `text` mentions `task_id` as a **bounded** token — i.e. the id is not
/// a prefix or suffix of a longer id-like run. Guards the exemption predicate
/// against near-misses (`cas-5c02x`, `cas-5c02-b`, `xcas-5c02`).
pub fn text_mentions_task_id_bounded(text: &str, task_id: &str) -> bool {
    if task_id.is_empty() {
        return false;
    }
    let bytes = text.as_bytes();
    let id_len = task_id.len();
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find(task_id) {
        let start = search_from + rel;
        let end = start + id_len;
        let before_ok = start == 0 || !is_task_id_boundary_char(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_task_id_boundary_char(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        // Advance one byte past this occurrence and keep scanning for a
        // bounded match later in the string.
        search_from = start + 1;
    }
    false
}

/// cas-126b: Whether an urgent message is a merge-complete **re-close**
/// hand-off that must NOT arm `halt_task_work`.
///
/// The urgent "MERGE DONE → re-close now" notification wakes a worker parked in
/// [`TaskStatus::AwaitingMerge`] and instructs it to `task close` the very task
/// the message names. Arming halt on *that* send deadlocks the worker: close is
/// refused (`WORK HALTED`) yet the parked task cannot be `start`ed to clear the
/// halt (starting an `AwaitingMerge` task is illegal by design), so the only
/// escape is starting an *unrelated* Open task — the exact multi-step recovery
/// that made factory throughput die at the merge/re-close handoff.
///
/// We exempt exactly this class — close/verify guidance that references, as a
/// **bounded** token, at least one task the caller has already resolved to be
/// (a) currently in `AwaitingMerge` **and** (b) assigned to the urgent's target
/// worker. Binding to the target's own parked task is essential: an urgent to
/// worker B must not skip halt merely because its text names worker A's parked
/// task. Halt is preserved for ordinary urgent stop / re-scope messages (which
/// either don't look like close guidance or don't name the target's
/// `AwaitingMerge` task). The exemption only skips the halt flag; it does not
/// touch the factory-branch merge gate, so a re-close sent before the merge is
/// actually visible still rejects with `MERGE REQUIRED` (never a false
/// success).
///
/// Callers pass the ids of the **target worker's** tasks currently in
/// `AwaitingMerge`; this keeps the predicate pure and harness-agnostic (Grok /
/// Codex / Claude all funnel through the same `message_send` fan-out).
pub fn is_merge_reclose_exempt_urgent(text: &str, target_awaiting_merge_task_ids: &[String]) -> bool {
    if !looks_like_close_or_verify_guidance(text) {
        return false;
    }
    target_awaiting_merge_task_ids
        .iter()
        .any(|id| text_mentions_task_id_bounded(text, id))
}

/// cas-60393 (G-M1/X-M1 deadlock); widened by cas-3894 to `InProgress`.
///
/// Whether the `task close` / `verification action=add` halt-work gate
/// should be **exempted** for this specific call.
///
/// cas-126b already stops the merge-done *notification itself* from arming
/// halt. But a halt armed **earlier**, by an unrelated urgent stop, survives
/// that fix untouched: the worker is parked in [`TaskStatus::AwaitingMerge`],
/// the supervisor merges the branch, and the re-close is refused anyway
/// because `halt_task_work` is still `1` from the older, unrelated event.
/// There is no in-band recovery — starting the parked task is illegal (you
/// cannot `start` an `AwaitingMerge` task), so only an operator can clear the
/// halt out of band. cas-60393 closed that deadlock for `AwaitingMerge`.
///
/// cas-3894: the same trap recurs one state earlier, for `InProgress`, and
/// is worse there because it can be a genuine mutual deadlock rather than
/// merely an unreachable escape. Two recorded incidents: a worker holding a
/// finished, gate-green `InProgress` task was halted by an entirely
/// unrelated, informational urgent message (a checkpoint nudge in one case,
/// a task-assignment briefing in the other — neither was a redirect about
/// the task being closed). The documented escape, "start a new task", was
/// either (a) unreachable — no unstarted assignment existed — or (b) reachable
/// but itself refused by the verification jail, because starting a *new* task
/// requires the caller's current `InProgress` task to be verified/closed
/// first — which is exactly the call halt was blocking. Two independent gates
/// each demand the other complete first.
///
/// The exemption is narrow by construction, identical in shape to the
/// `AwaitingMerge` case:
/// - It fires **only** when the task being closed is `AwaitingMerge` OR
///   `InProgress` and its `assignee` is exactly the calling agent's own
///   display name — someone else's task, or this same task in any other
///   status (`Open`, `Blocked`, `Closed`, `PendingSupervisorReview`), still
///   halts.
/// - It does **not** clear `halt_task_work` — every other close/verify call
///   remains blocked until a real new `task start` clears the flag. A worker
///   cannot use this to bootstrap its way into unhalted work on anything
///   other than the exact task it already owns.
/// - It does **not** touch any other gate. Skipping the halt check only lets
///   execution reach the merge-integrity / verification / code-review gates
///   that already run after it — an `AwaitingMerge` task whose commit is not
///   actually merged yet still bounces `MERGE REQUIRED`, and an `InProgress`
///   task with no verification row still bounces `VERIFICATION REQUIRED`.
///   This can never manufacture a false close success; it only removes the
///   halt gate's veto over gates that are already sufficient on their own.
/// - A genuine redirect ("stop, this approach is wrong") does not lose its
///   force here: the redirect is about the *content* of the task, and the
///   gates downstream of this check — verification, code review, the merge
///   gate — are what enforce content correctness, not this flag. What this
///   predicate removes is only the blanket "you may not close ANY task you
///   own, even already-finished ones" side effect of a halt that was never
///   about this task in the first place.
pub fn halt_exempt_for_owned_task(
    task_status: TaskStatus,
    task_assignee: Option<&str>,
    caller_agent_name: Option<&str>,
) -> bool {
    if !matches!(
        task_status,
        TaskStatus::AwaitingMerge | TaskStatus::InProgress
    ) {
        return false;
    }
    match (task_assignee, caller_agent_name) {
        (Some(assignee), Some(caller)) if !assignee.is_empty() && !caller.is_empty() => {
            assignee == caller
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::{AgentRole, TaskStatus};

    #[test]
    fn test_b269_terminal_closed_only_for_closed() {
        assert!(is_terminal_closed(TaskStatus::Closed));
        assert!(!is_terminal_closed(TaskStatus::InProgress));
        assert!(!is_terminal_closed(TaskStatus::Open));
        assert!(!is_terminal_closed(TaskStatus::AwaitingMerge));
        assert!(!is_terminal_closed(TaskStatus::PendingSupervisorReview));
    }

    #[test]
    fn test_b269_already_closed_message_forbids_reclose() {
        let msg = already_closed_close_message("cas-a651");
        assert!(msg.contains("ALREADY CLOSED"));
        assert!(msg.contains("cas-a651"));
        assert!(msg.to_lowercase().contains("do not re-verify"));
        assert!(msg.contains("mine"));
    }

    #[test]
    fn test_b269_verification_on_closed_rejected_message() {
        let msg = verification_on_closed_message("cas-a651");
        assert!(msg.contains("Closed"));
        assert!(msg.contains("cas-a651"));
        assert!(msg.to_lowercase().contains("not allowed") || msg.contains("await"));
    }

    #[test]
    fn test_b269_halt_metadata_detection() {
        let mut meta = HashMap::new();
        assert!(!agent_task_work_halted(&meta));
        meta.insert(HALT_TASK_WORK_META.to_string(), "1".to_string());
        assert!(agent_task_work_halted(&meta));
        meta.insert(HALT_TASK_WORK_META.to_string(), "true".to_string());
        assert!(agent_task_work_halted(&meta));
        meta.insert(HALT_TASK_WORK_META.to_string(), "0".to_string());
        assert!(!agent_task_work_halted(&meta));
    }

    #[test]
    fn test_b269_rewrite_stale_close_guidance_when_task_closed() {
        let original = "Please re-verify and task action=close id=cas-a651 reason=\"done\"";
        let rewritten = rewrite_stale_close_guidance(original, &["cas-a651"])
            .expect("must rewrite stale close guidance");
        assert!(rewritten.contains("STALE GUIDANCE SUPPRESSED"));
        assert!(rewritten.contains("cas-a651"));
        assert!(rewritten.to_lowercase().contains("already closed"));
        assert!(rewritten.contains("original message"));
    }

    #[test]
    fn test_b269_no_rewrite_when_task_still_open_guidance() {
        let original = "task action=close id=cas-open reason=x";
        assert!(rewrite_stale_close_guidance(original, &[]).is_none());
        assert!(rewrite_stale_close_guidance("hello idle", &["cas-a651"]).is_none());
        assert!(rewrite_stale_close_guidance(
            "task action=close id=cas-other",
            &["cas-a651"]
        )
        .is_none());
    }

    #[test]
    fn test_b269_looks_like_close_or_verify_guidance() {
        assert!(looks_like_close_or_verify_guidance(
            "run task action=close id=cas-x"
        ));
        assert!(looks_like_close_or_verify_guidance("please re-verify the tip"));
        assert!(!looks_like_close_or_verify_guidance("standing by for work"));
    }

    /// Review 2: authorize AgentRole::Director explicitly (role string), not
    /// display-name-only.
    #[test]
    fn test_b269_director_role_authorized_explicitly() {
        assert!(may_source_set_halt("any-name", "director"));
        assert!(may_source_role_set_halt(AgentRole::Director));
        assert!(may_source_role_set_halt(AgentRole::Supervisor));
        assert!(!may_source_role_set_halt(AgentRole::Worker));
        assert!(!may_source_role_set_halt(AgentRole::Standard));
        assert!(may_source_set_halt("eager-marten-46", "supervisor"));
        assert!(may_source_set_halt("director", "primary")); // display fallback
        assert!(!may_source_set_halt("staging-sync", "worker"));
    }

    /// Review 2: session scope — same worker name in another factory session
    /// must not receive halt.
    #[test]
    fn test_b269_halt_scoped_to_factory_session_same_name_cross_session() {
        let workers = vec![
            HaltWorkerCandidate {
                name: "staging-sync".into(),
                factory_session: Some("session-a".into()),
            },
            HaltWorkerCandidate {
                name: "staging-sync".into(), // same name, other session
                factory_session: Some("session-b".into()),
            },
            HaltWorkerCandidate {
                name: "other-worker".into(),
                factory_session: Some("session-a".into()),
            },
        ];
        let scoped = session_scoped_worker_names(&workers, Some("session-a"));
        assert_eq!(
            scoped,
            vec!["staging-sync".to_string(), "other-worker".to_string()]
        );
        // all_workers only session-a workers
        let targets = halt_targets_for_urgent("all_workers", &scoped);
        assert_eq!(targets.len(), 2);
        assert!(targets.contains(&"staging-sync".to_string()));
        assert!(targets.contains(&"other-worker".to_string()));
        // direct target: only session-scoped list (one staging-sync)
        let direct = halt_targets_for_urgent("staging-sync", &scoped);
        assert_eq!(direct, vec!["staging-sync".to_string()]);
        // session-b scoped list would not include session-a other-worker
        let scoped_b = session_scoped_worker_names(&workers, Some("session-b"));
        assert_eq!(scoped_b, vec!["staging-sync".to_string()]);
    }

    #[test]
    fn test_b269_urgent_all_workers_halts_every_session_worker_not_supervisor() {
        let workers = vec!["w1".into(), "w2".into(), "w3".into()];
        let targets = halt_targets_for_urgent("all_workers", &workers);
        assert_eq!(targets, workers);
        assert!(halt_targets_for_urgent("supervisor", &workers).is_empty());
        assert_eq!(
            halt_targets_for_urgent("w2", &workers),
            vec!["w2".to_string()]
        );
        assert!(halt_targets_for_urgent("not-a-worker", &workers).is_empty());
    }

    #[test]
    fn test_b269_worker_cannot_halt_supervisor() {
        let workers = vec!["staging-sync".into()];
        assert!(!should_persist_urgent_halt(
            true,
            "staging-sync",
            "worker",
            "supervisor",
            &workers
        ));
        assert!(should_persist_urgent_halt(
            true,
            "eager-marten-46",
            "supervisor",
            "staging-sync",
            &workers
        ));
        assert!(should_persist_urgent_halt(
            true,
            "factory-director",
            "director",
            "staging-sync",
            &workers
        ));
        assert!(!should_persist_urgent_halt(
            false,
            "eager-marten-46",
            "supervisor",
            "staging-sync",
            &workers
        ));
    }

    /// Review 2: start must not clear a newer concurrent urgent halt gen.
    #[test]
    fn test_b269_start_does_not_clear_newer_halt_generation() {
        let clear_ceiling = 1_000u64;
        assert!(should_clear_halt_at_generation(500, clear_ceiling));
        assert!(should_clear_halt_at_generation(1_000, clear_ceiling));
        assert!(!should_clear_halt_at_generation(1_001, clear_ceiling));
        // Legacy halt with no gen (0) is clearable.
        assert!(should_clear_halt_at_generation(0, clear_ceiling));
    }

    #[test]
    fn test_b269_apply_and_clear_halt_metadata() {
        let mut meta = HashMap::new();
        apply_halt_metadata(&mut meta, 42);
        assert!(agent_task_work_halted(&meta));
        assert_eq!(halt_generation(&meta), 42);
        clear_halt_metadata(&mut meta);
        assert!(!agent_task_work_halted(&meta));
        assert_eq!(halt_generation(&meta), 0);
    }

    /// cas-126b: the urgent MERGE DONE re-close hand-off (close guidance that
    /// names a task currently AwaitingMerge) is halt-exempt so the worker can
    /// re-close without starting a second task.
    #[test]
    fn test_126b_merge_reclose_urgent_is_halt_exempt() {
        let awaiting = vec!["cas-5c02".to_string()];
        let merge_done = "MERGE DONE: factory/comm-grok merged to epic. \
             Re-close now: task action=close id=cas-5c02 reason=\"merged\"";
        assert!(
            is_merge_reclose_exempt_urgent(merge_done, &awaiting),
            "close guidance naming an AwaitingMerge task must be halt-exempt"
        );
    }

    /// cas-126b: an ordinary urgent stop / re-scope must STILL arm halt — it is
    /// neither close guidance nor references an AwaitingMerge task.
    #[test]
    fn test_126b_ordinary_urgent_stop_still_halts() {
        let awaiting = vec!["cas-5c02".to_string()];
        let stop = "STOP — you are off the rails. Abandon the current approach \
             and stand by for a re-scope.";
        assert!(
            !is_merge_reclose_exempt_urgent(stop, &awaiting),
            "ordinary stop/redirect must not be halt-exempt (halt must still fire)"
        );
    }

    /// cas-126b: close guidance that does NOT name an AwaitingMerge task is not
    /// exempt — e.g. a re-close nudge for a task that is not parked, or when no
    /// task is AwaitingMerge at all (the no-merge / error case). Halt still fires
    /// and the merge gate remains the sole authority on close success.
    #[test]
    fn test_126b_close_guidance_without_awaiting_merge_task_not_exempt() {
        let close_text =
            "task action=close id=cas-9999 reason=\"done\""; // cas-9999 not AwaitingMerge
        // Referenced task is not among the AwaitingMerge ids.
        assert!(!is_merge_reclose_exempt_urgent(
            close_text,
            &["cas-5c02".to_string()]
        ));
        // No task is AwaitingMerge at all → never exempt.
        assert!(!is_merge_reclose_exempt_urgent(close_text, &[]));
        // Empty id string must never match via substring.
        assert!(!is_merge_reclose_exempt_urgent(
            close_text,
            &[String::new()]
        ));
    }

    /// cas-126b review-2 gate: exact bounded id match — a near-miss id that is a
    /// prefix/suffix of a longer id-like run must NOT be exempt.
    #[test]
    fn test_126b_task_id_match_is_bounded_not_substring() {
        // Whole-token mentions in realistic phrasings match.
        assert!(text_mentions_task_id_bounded(
            "task action=close id=cas-5c02 reason=merged",
            "cas-5c02"
        ));
        assert!(text_mentions_task_id_bounded("re-close cas-5c02.", "cas-5c02"));
        assert!(text_mentions_task_id_bounded(
            "close cas-5c02, then continue",
            "cas-5c02"
        ));
        // Trailing / leading alnum or '-' near-misses must NOT match.
        assert!(!text_mentions_task_id_bounded("close cas-5c02x now", "cas-5c02"));
        assert!(!text_mentions_task_id_bounded("close cas-5c02-b now", "cas-5c02"));
        assert!(!text_mentions_task_id_bounded("close xcas-5c02 now", "cas-5c02"));
        // Empty id never matches.
        assert!(!text_mentions_task_id_bounded("anything", ""));
        // A later bounded occurrence is found even if an earlier one is a near-miss.
        assert!(text_mentions_task_id_bounded(
            "cas-5c02x is wrong; use cas-5c02 instead",
            "cas-5c02"
        ));

        // Predicate level: near-miss id in close guidance is not exempt.
        assert!(!is_merge_reclose_exempt_urgent(
            "MERGE DONE — task action=close id=cas-5c02x reason=merged",
            &["cas-5c02".to_string()]
        ));
    }

    /// cas-126b composition: halt persists for an authorized urgent EXCEPT when
    /// the send is a merge re-close hand-off. Ties `should_persist_urgent_halt`
    /// (authorization/targeting) together with the new exemption to encode the
    /// net decision made in `message_send`.
    #[test]
    fn test_126b_net_halt_decision_skips_only_merge_reclose() {
        let workers = vec!["comm-grok".to_string()];
        let awaiting = vec!["cas-5c02".to_string()];

        let net_halt = |urgent: bool, text: &str| -> bool {
            should_persist_urgent_halt(urgent, "supervisor", "supervisor", "comm-grok", &workers)
                && !is_merge_reclose_exempt_urgent(text, &awaiting)
        };

        // Merge re-close urgent from supervisor → do NOT halt.
        assert!(!net_halt(
            true,
            "MERGE DONE — task action=close id=cas-5c02 reason=merged"
        ));
        // Ordinary urgent stop from supervisor → DO halt.
        assert!(net_halt(true, "STOP and stand by for re-scope"));
        // Non-urgent never halts regardless of text.
        assert!(!net_halt(false, "STOP and stand by for re-scope"));
    }

    #[test]
    fn test_b269_failed_start_statuses_do_not_clear_halt_policy() {
        let no_clear = [
            TaskStatus::Closed,
            TaskStatus::PendingSupervisorReview,
            TaskStatus::AwaitingMerge,
        ];
        for status in no_clear {
            assert!(
                !matches!(
                    status,
                    TaskStatus::Open | TaskStatus::InProgress | TaskStatus::Blocked
                ),
                "status {status:?} is a failed-start path that must preserve halt"
            );
        }
    }

    /// cas-60393 happy path: the caller's own AwaitingMerge task is exempt
    /// from an unrelated, pre-existing halt.
    #[test]
    fn test_60393_owned_awaiting_merge_is_halt_exempt() {
        assert!(halt_exempt_for_owned_task(
            TaskStatus::AwaitingMerge,
            Some("swift-fox-12"),
            Some("swift-fox-12"),
        ));
    }

    /// cas-60393: only the assigned AwaitingMerge task is exempt — a
    /// different worker's parked task must not skip the halt gate for this
    /// caller.
    #[test]
    fn test_60393_other_workers_awaiting_merge_task_not_exempt() {
        assert!(!halt_exempt_for_owned_task(
            TaskStatus::AwaitingMerge,
            Some("other-worker"),
            Some("swift-fox-12"),
        ));
    }

    /// cas-3894: widened from the original cas-60393 assertion. `InProgress`
    /// moved from the "must still halt" list to its own exempt test below —
    /// this is the ticket's whole point, not a regression. What remains true,
    /// and is the actual safety property (AC2): every status that is neither
    /// the caller's own `AwaitingMerge` nor `InProgress` task still halts.
    #[test]
    fn test_3894_non_exempt_statuses_still_halt() {
        for status in [
            TaskStatus::Open,
            TaskStatus::Blocked,
            TaskStatus::Closed,
            TaskStatus::PendingSupervisorReview,
        ] {
            assert!(
                !halt_exempt_for_owned_task(status, Some("swift-fox-12"), Some("swift-fox-12"),),
                "status {status:?} must not be halt-exempt — only AwaitingMerge/InProgress are"
            );
        }
    }

    /// cas-60393: missing assignee or unknown caller identity must fail
    /// closed (never exempt) rather than assume ownership.
    #[test]
    fn test_60393_missing_identity_fails_closed_to_halt() {
        assert!(!halt_exempt_for_owned_task(
            TaskStatus::AwaitingMerge,
            None,
            Some("swift-fox-12"),
        ));
        assert!(!halt_exempt_for_owned_task(
            TaskStatus::AwaitingMerge,
            Some("swift-fox-12"),
            None,
        ));
        assert!(!halt_exempt_for_owned_task(
            TaskStatus::AwaitingMerge,
            Some(""),
            Some(""),
        ));
    }

    // --- cas-3894: InProgress carve-out --------------------------------

    /// AC1: the recorded deadlock shape — a worker's own `InProgress` task,
    /// halted by an unrelated urgent (checkpoint nudge / task briefing, not a
    /// redirect about this task), must be closable without a new assignment.
    #[test]
    fn test_3894_owned_in_progress_is_halt_exempt() {
        assert!(halt_exempt_for_owned_task(
            TaskStatus::InProgress,
            Some("patient-cobra-45"),
            Some("patient-cobra-45"),
        ));
    }

    /// AC2 (safety half 1): a different worker's InProgress task must not be
    /// exempted for this caller — the exemption is ownership-bound, not
    /// status-only. Halt still fires for anything the caller does not own.
    #[test]
    fn test_3894_other_workers_in_progress_task_not_exempt() {
        assert!(!halt_exempt_for_owned_task(
            TaskStatus::InProgress,
            Some("other-worker"),
            Some("patient-cobra-45"),
        ));
    }

    /// AC2 (safety half 2): missing assignee/caller identity fails closed for
    /// InProgress exactly as it already does for AwaitingMerge — an unknown
    /// owner is never assumed to be "mine".
    #[test]
    fn test_3894_in_progress_missing_identity_fails_closed_to_halt() {
        assert!(!halt_exempt_for_owned_task(
            TaskStatus::InProgress,
            None,
            Some("patient-cobra-45"),
        ));
        assert!(!halt_exempt_for_owned_task(
            TaskStatus::InProgress,
            Some("patient-cobra-45"),
            None,
        ));
        assert!(!halt_exempt_for_owned_task(
            TaskStatus::InProgress,
            Some(""),
            Some(""),
        ));
    }
}
