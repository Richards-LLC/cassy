use crate::harness_policy::{
    is_supervisor_from_env, is_worker_without_subagents_from_env, supervisor_harness_from_env,
    supervisor_verification_tool, verification_policy, worker_coordination_tool,
    worker_harness_from_env,
};
use crate::mcp::tools::core::imports::*;

/// cas-9fff: gate epic close when `epic_verification_owner` is set.
///
/// Fail closed: the caller must present an identity that matches `owner`.
/// An unknown / absent identity is a rejection (not a silent fall-through),
/// so a mis-routed completion prompt cannot close an owned epic without
/// credentials.
pub(crate) fn epic_close_owner_gate(
    epic_id: &str,
    owner: &str,
    caller_id: Option<&str>,
    caller_name: Option<&str>,
    caller_session: Option<&str>,
) -> Result<(), String> {
    // cas-cc74: trim owner + identity facets so write-boundary normalize and
    // close compare stay consistent (exact match after trim).
    let owner = owner.trim();
    let matches_owner = [caller_id, caller_name, caller_session]
        .into_iter()
        .flatten()
        .any(|id| id.trim() == owner);
    if matches_owner {
        return Ok(());
    }
    let has_identity = [caller_id, caller_name, caller_session]
        .into_iter()
        .flatten()
        .any(|s| !s.trim().is_empty());
    if !has_identity {
        return Err(format!(
            "Epic {epic_id} is owned by epic_verification_owner={owner}; \
             caller identity is unknown — refusing close (fail closed, cas-9fff). \
             Present CAS agent id / CAS_AGENT_NAME / CAS_SESSION_ID matching the owner, \
             or transfer epic_verification_owner first."
        ));
    }
    Err(format!(
        "Epic {epic_id} is owned by epic_verification_owner={owner}; \
         this session cannot close it. Update epic_verification_owner \
         if ownership has transferred (cas-9fff)."
    ))
}

/// Deadline for one explicit task-scoped verification dispatch.
const VERIFICATION_DISPATCH_TIMEOUT_SECS: i64 = 600;

/// Heartbeat staleness threshold (seconds) for deciding whether an assignee
/// is still considered active for verification-skip purposes. Aligned with
/// the same 5-minute window used by task-claim reclaim.
const ASSIGNEE_STALE_SECS: i64 = 300;

/// Marker prefix used on the dispatch-request verification row (see
/// lines ~255-272 below). Used to distinguish a stale dispatch from a real
/// verifier-written Error verdict during auto-escalation.
const DISPATCH_SUMMARY_PREFIX: &str = "Dispatch requested";

pub(crate) fn required_verification_type(task_type: TaskType) -> VerificationType {
    if task_type == TaskType::Epic {
        VerificationType::Epic
    } else {
        VerificationType::Task
    }
}

fn delivery_audit_text_is_portable(value: &str) -> bool {
    if value.chars().any(char::is_control) {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    if [
        "-----begin private key",
        "-----begin rsa private key",
        "token=",
        "password=",
        "secret=",
        "authorization:",
        "bearer ",
        "sk-",
        "ghp_",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return false;
    }
    !value.split_whitespace().any(|token| {
        let token = token.trim_matches(|ch: char| "(),[]{}<>\"'".contains(ch));
        token.starts_with('/')
            || (token.len() >= 3
                && token.as_bytes()[1] == b':'
                && matches!(token.as_bytes()[2], b'/' | b'\\'))
    })
}

fn request_changes_role_gate(is_supervisor: bool, task_id: &str) -> Result<(), String> {
    if is_supervisor {
        Ok(())
    } else {
        Err(format!(
            "task request_changes rejected: only supervisors may decline an AwaitingMerge delivery. Task {task_id} stays parked; a worker cannot reject its own work to escape the delivery proof boundary."
        ))
    }
}

#[cfg(test)]
mod delivery_audit_text_tests {
    use super::{delivery_audit_text_is_portable, request_changes_role_gate};

    #[test]
    fn receipt_audit_text_rejects_paths_secrets_and_payload_controls() {
        assert!(delivery_audit_text_is_portable(
            "proof:serialized-workspace-1"
        ));
        assert!(delivery_audit_text_is_portable(
            "bounded delivery scope without local paths"
        ));
        assert!(!delivery_audit_text_is_portable(
            "proof stored at /home/alice/proof.json"
        ));
        assert!(!delivery_audit_text_is_portable(
            "proof stored at C:\\Users\\alice\\proof.json"
        ));
        assert!(!delivery_audit_text_is_portable("token=secret-value"));
        assert!(!delivery_audit_text_is_portable("ghp_secret-shaped"));
        assert!(!delivery_audit_text_is_portable("payload\nsecond line"));
    }

    #[test]
    fn only_a_supervisor_can_request_changes_on_awaiting_merge_work() {
        let worker_error = request_changes_role_gate(false, "cas-7484")
            .expect_err("a worker must not escape awaiting_merge by rejecting itself");
        assert!(worker_error.contains("only supervisors"));
        assert!(worker_error.contains("cas-7484"));
        request_changes_role_gate(true, "cas-7484").unwrap();
    }
}

/// Why the close path decided to skip (or not skip) the task-verifier step
/// for a given close attempt.
///
/// Carried through to the response message so the audit trail cites the
/// real reason instead of the catch-all "assignee inactive" phrase that
/// surfaced cas-3bd4.
///
/// The pre-cas-3bd4 implementation represented this as a single
/// `assignee_inactive: bool`. Every lookup failure — including the
/// very-common name-vs-id mismatch described below — defaulted to `true`
/// and the success message confidently lied that the assignee was inactive.
/// This enum preserves the same skip *behavior* (supervisor still closes
/// orphaned or genuinely-stale tasks without a verifier hop) but forces
/// every skip reason to be named.
///
/// ## Why the old `agent_store.get(task.assignee)` kept returning "inactive"
///
/// `task.assignee` is set by `task_claiming.rs:89` to
/// `Some(agent_name.clone())` — the human-readable display name, e.g.
/// `"mighty-viper-52"`. But `AgentStore::get(id)` runs `WHERE id = ?` in
/// `ops_agent.rs:79`, and `id` is the session-id (a UUID-like
/// identifier), not the name. The lookup never found the row, so
/// `unwrap_or(true)` treated the worker as inactive even though it was
/// actively holding a fresh lease. `compute_verification_skip_reason`
/// fixes this by consulting the task's active lease first — `TaskLease`
/// stores the real `agent_id`, not the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VerificationSkipReason {
    /// The assignee is alive and no bypass flag was set. Verification
    /// runs normally; this is *not* a skip.
    None,
    /// The caller passed the epic ownership gate. Epics do not run the
    /// per-task verifier because their child tasks are verified individually;
    /// closing one as its verification owner is therefore not an orphan
    /// recovery action even though epics normally have no task assignee.
    EpicOwnerClosed,
    /// The task has no assignee at all. Treated as orphaned; legacy
    /// callers reached this via the same skip path.
    NoAssignee,
    /// The assignee exists and is registered, but their heartbeat or
    /// lease is stale. `minutes_stale` is the observed staleness if we
    /// could measure it.
    AssigneeInactive { minutes_stale: Option<i64> },
    /// `task.assignee` is set but we cannot resolve it through the
    /// lease *or* a direct id lookup. The agent may have been GC'd or
    /// the assignee row holds an old display name no longer in the
    /// agent store. Skip verification and cite the real reason.
    AssigneeUnknown,
    /// Supervisor is closing a task whose assignee is still alive and
    /// has explicitly requested a verification skip via
    /// `bypass_code_review=true`. Separate from `AssigneeInactive` so
    /// the audit note reflects supervisor intent, not worker state.
    SupervisorBypass,
    /// cas-1932 (GH #62, minor): an assignee-lookup failure would have
    /// reported "verification skipped", but a current-cycle APPROVED
    /// verification for this task already exists. The close is authorized
    /// by that verdict, so name it instead of claiming nothing verified
    /// the work.
    ExistingApprovedVerification { verification_id: String },
}

impl VerificationSkipReason {
    /// Whether this reason short-circuits the verification gate.
    pub(crate) fn is_skip(&self) -> bool {
        !matches!(self, VerificationSkipReason::None)
    }

    /// Short human-readable suffix appended to the `Closed task:` line.
    /// Must start with a leading space so it slots cleanly into the
    /// format string.
    pub(crate) fn response_suffix(&self, verification_enabled: bool) -> String {
        match self {
            VerificationSkipReason::None => {
                if verification_enabled {
                    " (verified)".to_string()
                } else {
                    String::new()
                }
            }
            VerificationSkipReason::EpicOwnerClosed => {
                " (epic verification: owner-closed; child tasks individually verified)".to_string()
            }
            VerificationSkipReason::NoAssignee => {
                " (verification skipped — orphaned task, no assignee)".to_string()
            }
            VerificationSkipReason::AssigneeInactive {
                minutes_stale: Some(m),
            } => {
                format!(" (verification skipped — assignee inactive for {m}m)")
            }
            VerificationSkipReason::AssigneeInactive {
                minutes_stale: None,
            } => " (verification skipped — assignee lease expired)".to_string(),
            VerificationSkipReason::AssigneeUnknown => {
                " (verification skipped — assignee unknown)".to_string()
            }
            VerificationSkipReason::SupervisorBypass => {
                " (verification skipped — supervisor bypass via bypass_code_review=true)"
                    .to_string()
            }
            VerificationSkipReason::ExistingApprovedVerification { verification_id } => {
                format!(" (verified — approved verification {verification_id} on record)")
            }
        }
    }

    /// Reason text written to the `Skipped` verification row so the
    /// audit trail records the accurate reason alongside the row, not
    /// just in the response text.
    pub(crate) fn audit_reason(&self) -> String {
        match self {
            VerificationSkipReason::None => String::new(),
            VerificationSkipReason::EpicOwnerClosed => {
                "Epic closed by its verification owner; child tasks were individually verified."
                    .to_string()
            }
            VerificationSkipReason::NoAssignee => {
                "Closed via supervisor bypass — task had no assignee (orphaned).".to_string()
            }
            VerificationSkipReason::AssigneeInactive {
                minutes_stale: Some(m),
            } => format!(
                "Closed via supervisor bypass — assignee inactive for {m} minute(s) at close time."
            ),
            VerificationSkipReason::AssigneeInactive {
                minutes_stale: None,
            } => "Closed via supervisor bypass — assignee lease had expired at close time."
                .to_string(),
            VerificationSkipReason::AssigneeUnknown => {
                "Closed via supervisor bypass — assignee row not found in agent store (likely \
                 a stale or renamed agent)."
                    .to_string()
            }
            VerificationSkipReason::SupervisorBypass => {
                "Closed via supervisor bypass — bypass_code_review=true explicitly set by \
                 supervisor while assignee was still active."
                    .to_string()
            }
            VerificationSkipReason::ExistingApprovedVerification { verification_id } => {
                format!(
                    "Closed on the approved verification {verification_id} already recorded for \
                     this task's current work cycle; the assignee lookup could not resolve a live \
                     verifier, but the verdict exists and authorizes the close."
                )
            }
        }
    }
}

/// cas-1932 (GH #62 symptom 1): does an existing verification row authorize
/// the close that would otherwise be re-queued for supervisor review?
///
/// Accepted only when the verdict is `Approved`, matches the verification
/// type the task requires, and was recorded inside the task's current work
/// cycle (same `TaskCommitReceiptWindow` used to attribute commits, with the
/// same clock-skew allowance). A verdict from an earlier cycle — for example
/// one that predates a reopen and its rework — can never authorize a fresh
/// close.
pub(crate) fn approved_verification_satisfies_review_queue(
    verification: &Verification,
    window: Option<&TaskCommitReceiptWindow>,
    required_type: VerificationType,
) -> bool {
    if verification.status != VerificationStatus::Approved {
        return false;
    }
    if verification.verification_type != required_type {
        return false;
    }
    match window {
        Some(window) => {
            verification.created_at.timestamp()
                >= window.not_before.timestamp() - COMMIT_RECEIPT_CLOCK_SKEW_SECS
        }
        None => true,
    }
}

/// cas-1932 (GH #62, minor): the close path reported
/// "verification skipped — assignee unknown" while verification
/// `ver-fd59de6ef422` existed for the task. An assignee-resolution failure is
/// not evidence that nothing verified the work — when a current-cycle
/// approved verdict is on record, cite it instead.
///
/// Only *lookup-failure* reasons are replaced. `SupervisorBypass` and
/// `EpicOwnerClosed` are deliberate decisions and keep their own audit text;
/// `None` is not a skip at all.
pub(crate) fn skip_reason_with_existing_verification(
    reason: VerificationSkipReason,
    approved: Option<&Verification>,
) -> VerificationSkipReason {
    let is_lookup_failure = matches!(
        reason,
        VerificationSkipReason::NoAssignee
            | VerificationSkipReason::AssigneeUnknown
            | VerificationSkipReason::AssigneeInactive { .. }
    );
    match approved {
        Some(verification) if is_lookup_failure => {
            VerificationSkipReason::ExistingApprovedVerification {
                verification_id: verification.id.clone(),
            }
        }
        _ => reason,
    }
}

/// cas-1932 (GH #62 symptom 2): inputs for the shared-checkout
/// reviewable-change routing decision.
///
/// `attributable_reviewable_changes` is `Some(true|false)` when git could
/// answer whether this task's own work cycle produced reviewable commits, and
/// `None` when it could not.
pub(crate) struct SharedCheckoutReviewScope<'a> {
    pub task_type: TaskType,
    pub execution_note: Option<&'a str>,
    pub attributable_reviewable_changes: Option<bool>,
    pub checkout_has_reviewable_changes: bool,
}

/// Decide whether a non-isolated (shared-checkout) close has reviewable
/// changes *of its own*.
///
/// A shared worker's checkout is not a task diff: in the GH #62 incident it
/// carried ~64 files of prior-factory WIP, which the gate read as the spike's
/// output and answered with `CODE_REVIEW_REQUIRED` on a task that produced
/// nothing. Commits this task made in its work cycle always count. When it
/// made none AND the task's own spec declares no-code work (Spike/Chore, or
/// any `execution_note`), the checkout's dirty state is not attributed to it.
/// Every other shape keeps the previous signal, so no code task silently
/// escapes review, and unknowable git state falls back to the old behavior.
pub(crate) fn shared_checkout_has_reviewable_changes(scope: SharedCheckoutReviewScope<'_>) -> bool {
    match scope.attributable_reviewable_changes {
        Some(true) => true,
        Some(false) => {
            let declares_no_code_work = matches!(scope.task_type, TaskType::Spike | TaskType::Chore)
                || scope.execution_note.is_some_and(|note| !note.trim().is_empty());
            if declares_no_code_work {
                false
            } else {
                scope.checkout_has_reviewable_changes
            }
        }
        None => scope.checkout_has_reviewable_changes,
    }
}

impl CasCore {
    async fn submit_worker_completion_receipt(
        &self,
        raw_receipt: &str,
        task: &mut cas_types::Task,
        task_store: &dyn cas_store::TaskStore,
    ) -> Result<CallToolResult, McpError> {
        // Authority is server-derived before parsing or consulting any
        // caller-supplied receipt identity. A new receipt requires the exact
        // live task lease; an already persisted same-session receipt may be
        // retried idempotently after the handoff released that lease.
        let caller_id = match self.get_registered_agent_id_read_only() {
            Ok(id) => id,
            Err(_) => {
                return Ok(Self::tool_error(
                    "DELIVERY RECEIPT REJECTED: an already registered authenticated CAS worker holding the exact active task lease is required.",
                ));
            }
        };
        let agent_store = self.open_agent_store()?;
        let caller = match agent_store.get(&caller_id) {
            Ok(caller) => caller,
            Err(_) => {
                return Ok(Self::tool_error(
                    "DELIVERY RECEIPT REJECTED: authenticated session is not an active registered CAS worker holding the exact active task lease.",
                ));
            }
        };
        if caller.role != cas_types::AgentRole::Worker || !caller.is_alive() {
            return Ok(Self::tool_error(
                "DELIVERY RECEIPT REJECTED: only a live authenticated worker session holding the exact active task lease may submit a new receipt.",
            ));
        }
        let lease = agent_store.get_lease(&task.id).map_err(|error| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!(
                "Failed to inspect exact active task lease before receipt acceptance: {error}"
            )),
            data: None,
        })?;
        let lease = lease.filter(|lease| lease.expires_at > chrono::Utc::now());
        let latest_delivery = cas_store::get_latest_worker_delivery(&self.cas_root, &task.id)
            .map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Failed to inspect existing worker delivery boundary: {error}"
                )),
                data: None,
            })?;
        let (lease_epoch, retry_receipt_id) = match lease {
            Some(lease) if lease.agent_id == caller.id => (Some(lease.epoch), None),
            Some(_) => {
                return Ok(Self::tool_error(
                    "DELIVERY RECEIPT REJECTED: authenticated caller does not hold the exact active task lease. Assignment/display-name and request identity fields are non-authoritative; use the lease-owning worker session.",
                ));
            }
            None => match latest_delivery.as_ref() {
                Some((receipt, _)) if receipt.worker_agent_id == caller.id => {
                    (None, Some(receipt.id.clone()))
                }
                _ => {
                    return Ok(Self::tool_error(
                        "DELIVERY RECEIPT REJECTED: no exact active task lease belongs to the authenticated caller. A new receipt requires the lease-owning worker session; only that same session may retry its already-persisted exact receipt.",
                    ));
                }
            },
        };

        let input: cas_types::WorkerCompletionReceiptInput = serde_json::from_str(raw_receipt)
            .map_err(|error| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "completion_receipt must be valid WorkerCompletionReceiptInput JSON: {error}"
                )),
                data: None,
            })?;
        if input.task_id != task.id {
            return Ok(Self::tool_error(format!(
                "DELIVERY RECEIPT REJECTED: receipt task {} does not match close task {}.",
                input.task_id, task.id
            )));
        }
        if input.proof_reference.is_empty()
            || input.proof_reference.len() > 256
            || !input
                .proof_reference
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":._/-".contains(&byte))
            || !delivery_audit_text_is_portable(&input.proof_reference)
        {
            return Ok(Self::tool_error(
                "DELIVERY RECEIPT REJECTED: proof_reference must be 1-256 opaque ASCII characters from [A-Za-z0-9:._/-]. It is audit linkage only and never grants authority.",
            ));
        }
        if input.scope_summary.trim().is_empty()
            || input.scope_summary.len() > 1000
            || !delivery_audit_text_is_portable(&input.scope_summary)
        {
            return Ok(Self::tool_error(
                "DELIVERY RECEIPT REJECTED: scope_summary must be non-empty, at most 1000 bytes, portable, and contain no absolute path or secret-shaped payload.",
            ));
        }
        let full_sha =
            |value: &str| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !full_sha(&input.commit_sha)
            || !full_sha(&input.merge_base_sha)
            || !full_sha(&input.target_sha)
        {
            return Ok(Self::tool_error(
                "DELIVERY RECEIPT REJECTED: commit_sha, merge_base_sha, and target_sha must be full 40-character hexadecimal commit IDs.",
            ));
        }

        if input.worker_agent_id != caller.id {
            return Ok(Self::tool_error(
                "DELIVERY RECEIPT REJECTED: receipt worker_agent_id must match the authenticated lease-owning worker session; request identity claims never grant authority.",
            ));
        }
        let expected_source = format!("factory/{}", caller.name);
        if input.source_branch != expected_source {
            return Ok(Self::tool_error(format!(
                "DELIVERY RECEIPT REJECTED: source branch must be the registered worker branch `{expected_source}`."
            )));
        }
        let receipt =
            cas_store::build_worker_completion_receipt(&input, &caller.name, chrono::Utc::now());
        if let Some((latest_receipt, latest_transaction)) = latest_delivery.as_ref()
            && latest_receipt.id != receipt.id
            && super::proof_scope::delivery_state_locks_scope(latest_transaction.state)
        {
            return Ok(Self::tool_error(format!(
                "DELIVERY RECEIPT REJECTED: task {} already has active delivery proof {} in state {}. The proposed receipt is a distinct proof boundary and cannot replace it. Retry the exact persisted receipt or complete/recover the active delivery cycle first; task, deliverables, lease, receipts, transactions, events, dispatch, verdict, and outbox are unchanged.",
                task.id, latest_receipt.id, latest_transaction.state
            )));
        }
        if retry_receipt_id
            .as_ref()
            .is_some_and(|expected| expected != &receipt.id)
        {
            return Ok(Self::tool_error(
                "DELIVERY RECEIPT REJECTED: authenticated caller has no active task lease and the proposed receipt is not its already-persisted exact proof boundary. Reclaim/start the task before submitting new proof.",
            ));
        }
        let existing_delivery =
            cas_store::get_worker_delivery_by_receipt(&self.cas_root, &receipt.id).map_err(
                |error| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "Failed to inspect worker delivery receipt: {error}"
                    )),
                    data: None,
                },
            )?;
        if existing_delivery
            .as_ref()
            .is_some_and(|(_, transaction)| {
                transaction.state == cas_types::WorkerDeliveryState::Delivered
                    && task.status != TaskStatus::Closed
            })
        {
            return Ok(Self::tool_error(format!(
                "DELIVERY RECEIPT REJECTED: receipt {} belongs to a terminal Delivered proof cycle, but task {} has been reopened. Submit a fresh cycle-bound receipt after completing and proving the new work. Task, deliverables, lease, delivery, dispatch, verdict, and events are unchanged.",
                receipt.id, task.id
            )));
        }
        let target = match task.deliverables.work_target.as_ref() {
            Some(target) => target,
            None => {
                return Ok(Self::tool_error(
                    "DELIVERY RECEIPT REJECTED: transactional delivery requires a declared WorkTarget/RepoContext; legacy close remains available without completion_receipt.",
                ));
            }
        };
        let context = match crate::mcp::tools::core::task::repo_context::resolve_repo_context(
            &self.cas_root,
            target,
        ) {
            Ok(context) => context,
            Err(message) => return Ok(Self::tool_error(message)),
        };
        if input.repo_selector != context.repo_selector
            || input.target_branch != context.target_branch
        {
            return Ok(Self::tool_error(
                "DELIVERY RECEIPT REJECTED: repository selector or target branch does not match the task's server-resolved RepoContext.",
            ));
        }
        let source_sha = resolve_branch_sha(&context.repo_root, &input.source_branch);
        let target_sha = resolve_branch_sha(&context.repo_root, &input.target_branch);
        let merge_base = git_merge_base(
            &context.repo_root,
            &input.source_branch,
            &input.target_branch,
        );
        if source_sha.as_deref() != Some(input.commit_sha.as_str()) {
            return Ok(Self::tool_error(
                "DELIVERY RECEIPT REJECTED: changed worker tip; commit_sha is not the current source-branch tip.",
            ));
        }
        if target_sha.as_deref() != Some(input.target_sha.as_str()) {
            return Ok(Self::tool_error(
                "DELIVERY RECEIPT REJECTED: target tip changed before receipt acceptance; refresh target_sha and merge_base.",
            ));
        }
        if merge_base.as_deref() != Some(input.merge_base_sha.as_str()) {
            return Ok(Self::tool_error(
                "DELIVERY RECEIPT REJECTED: merge_base_sha does not match the live source/target merge base.",
            ));
        }
        let worker_path = match self.resolve_worker_worktree_path(task, Some(&context)) {
            Ok(Some(path)) => path,
            Ok(None) => {
                return Ok(Self::tool_error(
                    "DELIVERY RECEIPT REJECTED: no task-owned worker worktree resolves for the declared repository.",
                ));
            }
            Err(message) => return Ok(Self::tool_error(message)),
        };
        let hook_evidence = match run_declared_pre_close_hook(
            task,
            &context,
            Some(&worker_path),
            Some(&input.commit_sha),
        ) {
            Ok(evidence) => evidence,
            Err(message) => {
                return Ok(Self::tool_error(format!(
                    "DELIVERY RECEIPT REJECTED: task-owned pre-close proof failed.\n\n{message}"
                )));
            }
        };

        // A receipt creates a fresh proof boundary. A legacy or earlier-cycle
        // verdict must never authorize this immutable commit by accident.
        // Exact retries return the existing transaction (which may already
        // have advanced); genuinely new receipts always await a new verdict.
        let initial_state = cas_types::WorkerDeliveryState::AwaitingVerification;
        let delivery_boundary = cas_types::VerificationProofBoundary::delivery(
            receipt.id.clone(),
            cas_store::worker_delivery_transaction_id(&receipt.id),
        );
        let owner_id = self.verification_dispatch_owner(&caller.id)?;
        let was_existing = existing_delivery.is_some();
        let transaction = if let Some((_, transaction)) = existing_delivery {
            transaction
        } else {
            // The immutable receipt, delivery transaction, and exact dispatch
            // are one SQLite intent transaction. Receipt B therefore cannot
            // persist if active dispatch A rejects the new boundary.
            let Some(lease_epoch) = lease_epoch else {
                return Ok(Self::tool_error(
                    "DELIVERY RECEIPT REJECTED: a new immutable receipt requires the authenticated caller's exact active task lease.",
                ));
            };
            cas_store::create_worker_delivery_with_dispatch_for_lease(
                &self.cas_root,
                &receipt,
                initial_state,
                &caller.id,
                lease_epoch,
                &owner_id,
                chrono::Utc::now() + chrono::Duration::seconds(VERIFICATION_DISPATCH_TIMEOUT_SECS),
            )
            .map(|(transaction, _)| transaction)
            .map_err(|error| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "DELIVERY RECEIPT REJECTED: exact verification proof boundary conflict: {error}"
                )),
                data: None,
            })?
        };

        let (projected_status, projected_pending_verification) = if transaction.state
            == cas_types::WorkerDeliveryState::AwaitingVerification
        {
            // Exact retries validate and recover only their own boundary.
            cas_store::create_verification_dispatch_bound(
                &self.cas_root,
                &task.id,
                &caller.id,
                &owner_id,
                &delivery_boundary,
                chrono::Utc::now() + chrono::Duration::seconds(VERIFICATION_DISPATCH_TIMEOUT_SECS),
                false,
            )
            .map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Failed to persist task-scoped verification dispatch: {error}"
                )),
                data: None,
            })?;
            (TaskStatus::PendingSupervisorReview, true)
        } else if transaction.state == cas_types::WorkerDeliveryState::AwaitingMerge {
            (TaskStatus::AwaitingMerge, false)
        } else {
            (task.status, task.pending_verification)
        };

        // Receipt/delivery/dispatch persistence is the immutable recovery
        // boundary, but it is not a completed worker-to-supervisor handoff
        // until the exact lease generation is gone. Keep task projection
        // behind this gate so a failed cleanup cannot advertise review-ready
        // state while the worker still visibly owns the task. An exact retry
        // reuses the boundary above and reconciles this step idempotently.
        if let Some(lease_epoch) = lease_epoch {
            let cleanup_complete = match agent_store.release_lease_if_owner_epoch(
                &task.id,
                &caller.id,
                lease_epoch,
                "Immutable worker completion receipt accepted for supervisor delivery",
            ) {
                Ok(true) => true,
                // Another concurrent exact submission may have completed the
                // same release first. Only absence of an active lease makes
                // that conditional miss a successful reconciliation.
                Ok(false) => matches!(agent_store.get_lease(&task.id), Ok(None)),
                Err(_) => false,
            };
            if !cleanup_complete {
                return Ok(Self::tool_error(format!(
                    "DELIVERY RECEIPT HANDOFF INCOMPLETE\n\nReceipt: {}\nTransaction: {}\nState: {}\n\nThe immutable delivery boundary is safely persisted, but exact task-lease cleanup did not complete, so CAS did not report a clean handoff or advance this invocation's task projection. The lease remains active or could not be verified as released.\n\nRemediation: resolve the lease cleanup failure, then retry the exact same completion_receipt from this worker session. Do not create a replacement receipt. If lease ownership changed, a supervisor must reconcile the task lease before delivery continues.",
                    receipt.id, transaction.id, transaction.state
                )));
            }
        }

        let projection_coherent = was_existing
            && task.status == projected_status
            && task.pending_verification == projected_pending_verification
            && task.deliverables.pre_close_hook.as_ref() == Some(&hook_evidence)
            && task.deliverables.factory_branch_anchor.as_deref()
                == Some(input.commit_sha.as_str())
            && task.deliverables.parked_branch.as_deref() == Some(input.source_branch.as_str());
        if !projection_coherent {
            task.status = projected_status;
            task.pending_verification = projected_pending_verification;
            task.deliverables.pre_close_hook = Some(hook_evidence);
            task.deliverables.factory_branch_anchor = Some(input.commit_sha.clone());
            task.deliverables.parked_branch = Some(input.source_branch.clone());
            task.updated_at = chrono::Utc::now();
            task_store.update(task).map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Failed to project worker delivery state onto task: {error}"
                )),
                data: None,
            })?;
        }

        let next = match transaction.state {
            cas_types::WorkerDeliveryState::AwaitingVerification => {
                "A capability-bound task-verifier or registered supervisor must record the exact-task verdict."
            }
            cas_types::WorkerDeliveryState::AwaitingMerge
            | cas_types::WorkerDeliveryState::MergeAuthorized
            | cas_types::WorkerDeliveryState::Merged
            | cas_types::WorkerDeliveryState::CloseReady => {
                "A registered supervisor may call worktree_merge with this task_id; CAS will revalidate and resume the delivery."
            }
            cas_types::WorkerDeliveryState::Delivered => {
                "No action; the exact immutable delivery is already complete."
            }
            cas_types::WorkerDeliveryState::VerificationFailed
            | cas_types::WorkerDeliveryState::ChangesRequested
            | cas_types::WorkerDeliveryState::Conflict
            | cas_types::WorkerDeliveryState::Stale
            | cas_types::WorkerDeliveryState::RepoMismatch
            | cas_types::WorkerDeliveryState::TipChanged => {
                "Correct the recorded failure, produce fresh proof, and submit a new immutable receipt."
            }
        };
        Ok(Self::success(format!(
            "Worker delivery receipt accepted idempotently.\nReceipt: {}\nTransaction: {}\nState: {}\nNext action: {}",
            receipt.id, transaction.id, transaction.state, next
        )))
    }

    fn verification_dispatch_owner(&self, requester_id: &str) -> Result<String, McpError> {
        let agent_store = self.open_agent_store()?;
        let requester = agent_store.get(requester_id).map_err(|_| McpError {
            code: ErrorCode::INVALID_REQUEST,
            message: Cow::from(
                "Verification dispatch requires an authenticated registered CAS session.",
            ),
            data: None,
        })?;
        if requester.role != cas_types::AgentRole::Worker || requester.factory_session.is_none() {
            return Ok(requester.id);
        }
        let owner_id = super::supervisor_push::resolve_owning_supervisor(
            agent_store.as_ref(),
            requester.factory_session.as_deref(),
        )
        .map(|supervisor| supervisor.agent_id)
        // A registered worker may own issuance when no live supervisor is
        // registered. This does not grant verdict authority: the worker must
        // still mint, bind, and present the one-time capability to a distinct
        // registered task-verifier child.
        .unwrap_or(requester.id);
        Ok(owner_id)
    }

    fn record_close_rejection_activity(&self, task_id: &str, reason: &str, message: &str) {
        let Ok(agent_id) = self.get_agent_id() else {
            return;
        };
        let Ok(event_store) = cas_store::SqliteEventStore::open(&self.cas_root) else {
            return;
        };
        use cas_store::EventStore;
        use cas_types::{Event, EventEntityType, EventType};

        let event = Event::new(
            EventType::WorkerVerificationBlocked,
            EventEntityType::Agent,
            &agent_id,
            format!("task close rejected: {task_id} {reason}"),
        )
        .with_session(agent_id)
        .with_metadata(serde_json::json!({
            "task_id": task_id,
            "close_rejected": true,
            "reason": reason,
            "message": message,
        }));
        if let Err(e) = event_store.record(&event) {
            tracing::warn!(task_id = %task_id, error = %e, "failed to record close rejection activity");
        }
    }

    fn park_task_awaiting_merge(
        &self,
        task_store: &dyn cas_store::TaskStore,
        task: &Task,
        reason: &str,
        message: &str,
        factory_branch_anchor: Option<String>,
        merge_conflicted: bool,
    ) {
        let mut parked = task.clone();
        let now = chrono::Utc::now();
        parked.status = TaskStatus::AwaitingMerge;
        // cas-a844/cas-7308a: snapshot whether THIS park needs worker merge
        // rework: either a genuine conflict, or a preflight error that means
        // CAS cannot prove it clean. This only fires once per task (same
        // guard as the anchor below); a later preflight re-check on retry
        // (see call site) can still flip it true if the situation changes.
        parked.deliverables.merge_conflicted = merge_conflicted;
        // cas-4b3f/cas-3d37: retain the commit-time task anchor when present;
        // otherwise snapshot the factory tip the FIRST time this task parks.
        // Anchors `run_factory_branch_merge_gate`'s later retries to THIS
        // task's own work instead of a reused branch's live HEAD.
        if parked.deliverables.factory_branch_anchor.is_none() {
            parked.deliverables.factory_branch_anchor = factory_branch_anchor;
        }
        // cas-a844: record the branch NAME (not just its tip sha) so a lost
        // worker's commits stay linked to this task even after the assignee
        // field is reassigned or cleared. Never overwrite an existing value —
        // this only fires once per task, same as the anchor above.
        if parked.deliverables.parked_branch.is_none() {
            parked.deliverables.parked_branch = task
                .assignee
                .as_deref()
                .map(|assignee| format!("factory/{assignee}"));
        }
        // Parking precedes verification dispatch. Clear only this task's
        // pending flag so the next close attempt can create a fresh typed
        // dispatch after the merge gate succeeds.
        parked.pending_verification = false;
        parked.pending_worktree_merge = false;
        parked.updated_at = now;

        let timestamp = now.format("%Y-%m-%d %H:%M");
        let audit = format!(
            "[{timestamp}] Close rejected: {reason}. Task parked as awaiting_merge; worker lease released until supervisor merge completes."
        );
        parked.notes = if parked.notes.is_empty() {
            audit
        } else {
            format!("{}\n\n{}", parked.notes, audit)
        };

        if let Err(e) = task_store.update(&parked) {
            tracing::warn!(
                task_id = %task.id,
                error = %e,
                "failed to park task awaiting merge after close rejection"
            );
        } else {
            // cas-062d / cas-17e4: durable outbox for AwaitingMerge + close-rejected.
            let actor = self.get_agent_id().unwrap_or_else(|_| "unknown".into());
            let actor_name = self
                .open_agent_store()
                .ok()
                .and_then(|s| s.get(&actor).ok())
                .map(|a| a.name)
                .unwrap_or_else(|| actor.clone());
            let occurrence = super::supervisor_push::occurrence_from_updated_at(parked.updated_at);
            if let Err(e) = self.push_task_lifecycle(
                &task.id,
                &task.title,
                task.status,
                TaskStatus::AwaitingMerge,
                &actor_name,
                Some(reason),
                super::supervisor_push::LifecycleTransition::AwaitingMerge,
                &occurrence,
            ) {
                tracing::error!(
                    task_id = %task.id,
                    error = %e,
                    "supervisor lifecycle push failed after AwaitingMerge park (task remains AwaitingMerge; replay outbox)"
                );
            }
            if let Err(e) = self.push_task_lifecycle(
                &task.id,
                &task.title,
                task.status,
                TaskStatus::AwaitingMerge,
                &actor_name,
                Some(reason),
                super::supervisor_push::LifecycleTransition::CloseRejected,
                &occurrence,
            ) {
                tracing::error!(
                    task_id = %task.id,
                    error = %e,
                    "supervisor lifecycle push failed after close rejection (task remains AwaitingMerge; replay outbox)"
                );
            }
        }

        if let Ok(agent_store) = self.open_agent_store() {
            if let Err(e) = agent_store
                .release_lease_for_task(&task.id, "MERGE REQUIRED: parked awaiting_merge")
            {
                tracing::warn!(
                    task_id = %task.id,
                    error = %e,
                    "failed to release task lease after merge-gated close rejection"
                );
            }
        }

        self.record_close_rejection_activity(&task.id, reason, message);
    }

    /// cas-a844: refresh `merge_conflicted` on an already-parked task when a
    /// retried close now shows a genuine conflict that wasn't flagged (or
    /// didn't exist) at first park. Deliberately does NOT touch `notes` —
    /// the audit note is written once, at park time; this only keeps the
    /// status-output flag truthful on later retries. No-op if the task
    /// isn't (or is no longer) `AwaitingMerge`, or is already flagged.
    fn mark_awaiting_merge_conflicted(&self, task_store: &dyn cas_store::TaskStore, task_id: &str) {
        let Ok(mut task) = task_store.get(task_id) else {
            return;
        };
        if task.status != TaskStatus::AwaitingMerge || task.deliverables.merge_conflicted {
            return;
        }
        task.deliverables.merge_conflicted = true;
        task.updated_at = chrono::Utc::now();
        if let Err(e) = task_store.update(&task) {
            tracing::warn!(
                task_id = %task_id,
                error = %e,
                "failed to refresh merge_conflicted flag on retried awaiting_merge close"
            );
        }
    }

    /// Reject direct `task update status=closed` while an immutable delivery
    /// transaction still owns the task's close lifecycle.
    ///
    /// This is deliberately read-only: direct update may recognize an already
    /// Delivered transaction only while the task retains that exact cycle's
    /// commit anchor. Reopen clears the anchor, so terminal evidence cannot
    /// authorize a later cycle. This guard must not advance, repair, or fail a
    /// transaction; those transitions belong to the delivery path.
    pub(crate) fn guard_direct_update_close_delivery_state(
        &self,
        task: &Task,
    ) -> Result<(), String> {
        let Some((receipt, transaction)) =
            cas_store::get_latest_worker_delivery(&self.cas_root, &task.id).map_err(|error| {
                format!(
                    "DELIVERY CLOSE CHECK FAILED: could not inspect the task's immutable delivery state: {error}"
                )
            })?
        else {
            return Ok(());
        };

        if transaction.state == cas_types::WorkerDeliveryState::Delivered {
            if task.deliverables.factory_branch_anchor.as_deref()
                == Some(receipt.commit_sha.as_str())
            {
                return Ok(());
            }
            return Err(format!(
                "DELIVERY CLOSE BLOCKED\n\nTask {} was reopened after immutable delivery transaction {} reached Delivered. The terminal receipt belongs to the prior proof cycle and cannot authorize this close.\n\nRemediation: complete the reopened work and submit a fresh immutable completion receipt for the new cycle.",
                task.id, transaction.id
            ));
        }

        let remediation = match transaction.state {
            cas_types::WorkerDeliveryState::AwaitingVerification => {
                "Record the exact-task delivery verdict through verification, then resume delivery through supervisor worktree_merge."
            }
            cas_types::WorkerDeliveryState::AwaitingMerge
            | cas_types::WorkerDeliveryState::MergeAuthorized
            | cas_types::WorkerDeliveryState::Merged
            | cas_types::WorkerDeliveryState::CloseReady => {
                "A registered supervisor must resume this immutable delivery with worktree_merge and its task_id."
            }
            cas_types::WorkerDeliveryState::VerificationFailed
            | cas_types::WorkerDeliveryState::ChangesRequested
            | cas_types::WorkerDeliveryState::Conflict
            | cas_types::WorkerDeliveryState::Stale
            | cas_types::WorkerDeliveryState::RepoMismatch
            | cas_types::WorkerDeliveryState::TipChanged => {
                "Correct the recorded delivery failure, produce fresh proof, and submit a new immutable completion receipt."
            }
            cas_types::WorkerDeliveryState::Delivered => unreachable!(),
        };
        Err(format!(
            "DELIVERY CLOSE BLOCKED\n\nTask {} has immutable delivery transaction {} in state {}. Direct task update cannot advance or bypass the delivery state machine.\n\nRemediation: {}",
            task.id, transaction.id, transaction.state, remediation
        ))
    }

    /// Apply the normal task-close merge predicate to direct
    /// `task update status=closed` without performing close's parking,
    /// lifecycle-push, event, anchor, or lease side effects.
    ///
    /// The update handler resolves a declared RepoContext once and passes it
    /// here for both this gate and the later task-owned pre-close hook.
    pub(crate) fn guard_direct_update_close_merge_state(
        &self,
        task_store: &dyn cas_store::TaskStore,
        task: &Task,
        declared_repo_context: Option<&super::super::repo_context::RepoContext>,
    ) -> Result<(), String> {
        if task.task_type == TaskType::Epic || task.assignee.is_none() {
            return Ok(());
        }

        let has_recorded_merge_evidence = task.worktree_id.is_some()
            || task.deliverables.factory_branch_anchor.is_some()
            || task.deliverables.parked_branch.is_some()
            || task
                .assignee
                .as_deref()
                .and_then(|assignee| resolve_system_b_worktree_path(&self.cas_root, assignee))
                .is_some();
        let factory_merge_enforcement =
            std::env::var_os("CAS_FACTORY_MODE").is_some() && has_recorded_merge_evidence;
        let resolved_repo = declared_repo_context
            .map(|context| Ok(context.repo_root.clone()))
            .unwrap_or_else(|| resolve_close_gate_repo_root(&self.cas_root));
        let close_repo_verified = resolved_repo.is_ok();
        let close_project_root = match resolved_repo {
            Ok(repo_root) => repo_root,
            Err(message) if factory_merge_enforcement => return Err(message),
            Err(_) => self
                .cas_root
                .parent()
                .unwrap_or(&self.cas_root)
                .to_path_buf(),
        };

        let worktree_store_parent_branch = task.worktree_id.as_deref().and_then(|wt_id| {
            self.open_worktree_store()
                .ok()
                .and_then(|store| store.get(wt_id).ok())
                .map(|wt| wt.parent_branch.clone())
        });
        let epic_parent_branch = task_store
            .get_parent_epic(&task.id)
            .ok()
            .flatten()
            .and_then(|parent| parent.branch);
        let parent_branch_resolution = if let Some(context) = declared_repo_context {
            Ok(context.target_branch.clone())
        } else if close_repo_verified {
            resolve_close_parent_branch(
                worktree_store_parent_branch,
                epic_parent_branch,
                &close_project_root,
            )
        } else {
            Ok(worktree_store_parent_branch
                .or(epic_parent_branch)
                .unwrap_or_else(|| "main".to_string()))
        };
        let resolved_parent_branch = match parent_branch_resolution {
            Ok(branch) => branch,
            Err(message) if factory_merge_enforcement => return Err(message),
            Err(_) => "main".to_string(),
        };
        let req = TaskCloseRequest {
            id: task.id.clone(),
            reason: None,
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: None,
            commit_receipt: None,
        };
        match run_factory_branch_merge_gate(
            task,
            &req,
            &resolved_parent_branch,
            &close_project_root,
        ) {
            MergeStateGateOutcome::Proceed | MergeStateGateOutcome::ProceedWithNote(_) => Ok(()),
            MergeStateGateOutcome::Reject(message) => Err(message),
        }
    }

    pub async fn cas_task_close(
        &self,
        params: Parameters<TaskCloseRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.cas_task_close_with_completion(params, None).await
    }

    pub async fn cas_task_close_with_completion(
        &self,
        Parameters(req): Parameters<TaskCloseRequest>,
        completion_receipt: Option<String>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let mut task = task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        if let Some(raw_receipt) = completion_receipt.as_deref() {
            if let Err(message) = super::proof_scope::guard_task_proof_scope(
                &self.cas_root,
                &task,
                super::proof_scope::ProofScopeOperation::CompletionReceipt,
            ) {
                return Ok(Self::tool_error(message));
            }
            return self
                .submit_worker_completion_receipt(raw_receipt, &mut task, task_store.as_ref())
                .await;
        }

        // cas-6d0b / cas-b269: short-circuit already-Closed before
        // merge/review/verification gates. Do not re-success, overwrite
        // closed_at, or demand CODE_REVIEW_REQUIRED.
        if super::stale_close_guard::is_terminal_closed(task.status)
            || task.status == TaskStatus::Closed
        {
            let closed_at_msg = task
                .closed_at
                .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "unknown time".to_string());
            return Ok(Self::success(format!(
                "Already closed: {} - {} (closed at {}). This call did not re-close \
                 the task; closed_at and notes were left unchanged.",
                req.id, task.title, closed_at_msg
            )));
        }

        // An explicitly persisted proof cycle is stronger than configuration,
        // depth, orphan, and review convenience paths. Once one exists, no
        // close projection may proceed until that exact dispatch resolves.
        // This guard also protects internal post-merge re-close calls.
        match cas_store::get_latest_verification_dispatch(&self.cas_root, &req.id) {
            Err(error) => {
                return Ok(Self::tool_error(format!(
                    "⚠️ VERIFICATION DISPATCH INVALID\n\nTask {} has unreadable exact dispatch state: {}. CAS refuses to infer close authority.",
                    req.id, error
                )));
            }
            Ok(Some(dispatch))
                if matches!(
                    dispatch.state,
                    cas_types::VerificationDispatchState::Pending
                        | cas_types::VerificationDispatchState::Claimed
                ) =>
            {
                if dispatch.deadline_at <= chrono::Utc::now() {
                    let timed_out = cas_store::timeout_verification_dispatch(
                        &self.cas_root,
                        &req.id,
                        chrono::Utc::now(),
                    )
                    .map_err(|error| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Failed to persist exact verification timeout: {error}"
                        )),
                        data: None,
                    })?
                    .ok_or_else(|| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(
                            "Exact verification timeout changed before persistence; retry close.",
                        ),
                        data: None,
                    })?;
                    let mut timed_out_task = task.clone();
                    timed_out_task.pending_verification = false;
                    timed_out_task.updated_at = chrono::Utc::now();
                    task_store
                        .update(&timed_out_task)
                        .map_err(|error| McpError {
                            code: ErrorCode::INTERNAL_ERROR,
                            message: Cow::from(format!(
                                "Failed to project exact verification timeout: {error}"
                            )),
                            data: None,
                        })?;
                    let sup_ver = supervisor_verification_tool();
                    return Ok(Self::tool_error(format!(
                        "⚠️ VERIFICATION TIMED OUT\n\nTask {} exact dispatch {} requires named registered-supervisor recovery before close.\n\nRecord the direct recovery verdict with {sup_ver} action=add task_id={} dispatch_id={} status=approved summary=\"...\", then retry close.",
                        req.id, timed_out.id, req.id, timed_out.id
                    )));
                }
                return Ok(Self::tool_error(format!(
                    "⚠️ VERIFICATION REQUIRED\n\nTask {} cannot close until exact pending dispatch {} records its capability-bound verifier or registered supervisor-direct verdict.",
                    req.id, dispatch.id
                )));
            }
            Ok(Some(dispatch))
                if dispatch.state == cas_types::VerificationDispatchState::TimedOut =>
            {
                return Ok(Self::tool_error(format!(
                    "⚠️ VERIFICATION TIMED OUT\n\nTask {} exact dispatch {} requires named registered-supervisor recovery before close.",
                    req.id, dispatch.id
                )));
            }
            Ok(_) => {}
        }

        // cas-b269: urgent stop sets halt_task_work; block close until new start.
        //
        // cas-60393 (AwaitingMerge) + cas-3894 (widened to InProgress): a
        // pre-existing halt armed by an EARLIER, unrelated urgent stop must
        // not obstruct re-close of the caller's OWN task. cas-a844 now lets
        // AwaitingMerge restart and clear the halt, but forcing that redundant
        // state transition complicates the merge hand-off. InProgress can hit
        // a genuine *mutual* deadlock, because the documented escape
        // ("start a new task") is itself refused by the verification jail
        // until this very task is closed. The exemption only skips *this*
        // check; the merge/verification/review gates below remain fully
        // authoritative, so a task whose work is not actually done yet still
        // bounces on those, exactly as before. Halt continues to block close
        // for every task/status/assignee the caller does not own as
        // AwaitingMerge or InProgress.
        if let Ok(agent_id) = self.get_agent_id() {
            if let Ok(agent_store) = self.open_agent_store() {
                if let Ok(agent) = agent_store.get(&agent_id) {
                    let halt_exempt = super::stale_close_guard::halt_exempt_for_owned_task(
                        task.status,
                        task.assignee.as_deref(),
                        Some(agent.name.as_str()),
                    );
                    if super::stale_close_guard::agent_task_work_halted(&agent.metadata)
                        && !halt_exempt
                    {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(
                                super::stale_close_guard::halt_blocks_task_work_message(
                                    "task action=close",
                                ),
                            ),
                            data: None,
                        });
                    }
                }
            }
        }

        // cas-9fff: refuse silent cross-session epic close when
        // epic_verification_owner is set. Fail closed: unknown caller
        // identity is a rejection (not a fall-through). Callers who need
        // to take over must update epic_verification_owner first (or act
        // as the owner). Prevents a mis-routed director completion prompt
        // from racing the owning supervisor on `task close`.
        if task.task_type == TaskType::Epic {
            if let Some(ref owner) = task.epic_verification_owner {
                let caller_id = self.get_agent_id().ok();
                let caller_name = std::env::var("CAS_AGENT_NAME").ok();
                let caller_session = std::env::var("CAS_SESSION_ID").ok();
                if let Err(msg) = epic_close_owner_gate(
                    &req.id,
                    owner,
                    caller_id.as_deref(),
                    caller_name.as_deref(),
                    caller_session.as_deref(),
                ) {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(msg),
                        data: None,
                    });
                }
            }
        }

        // cas-6538 (EPIC cas-1255 — per-task depth speed mode): a `depth=light`
        // task is the feel-driven, fast-iteration path. The close gate skips
        // the two *rigor* gates — the verification jail (no `task-verifier`
        // dispatch / `pending_verification` arming) and the P0 code-review gate
        // (treated as satisfied, including the supervisor-review queue hop that
        // *is* the P0 gate under `owner = "supervisor"`). The skip is recorded
        // as a decision note on the task (see `light_skip_decision_note`) so
        // the bypass is auditable.
        //
        // REGRESSION GUARD: this flag is the *only* condition that diverges
        // light from today's behavior. `Deep`/unset rows read back as `Deep`
        // (NULL→Deep, see cas-0344), so every existing close path is byte-for-
        // byte unchanged unless the task was explicitly created `depth=light`.
        //
        // SCOPE: the light skip deliberately does NOT touch the data-state
        // guards (merge-state cas-95ce/cas-762e, uncommitted-work cas-895d,
        // additive-only cas-bc1b, commit-claim). Those verify the work
        // physically exists / is merged — orthogonal to review rigor — and a
        // light task must still satisfy them. It also does not interact with
        // the supervisor `bypass_code_review` override, which stays exactly as
        // before for `Deep` and is simply redundant for `Light`.
        let depth_light = task.depth == crate::types::TaskDepth::Light;

        // cas-ede8: every factory merge-enforcement gate must bind to the same
        // verified repository. `cas_root.parent()` is only correct for the
        // conventional `<repo>/.cas` layout; nested/custom CAS roots otherwise
        // make the gates query a non-repository and silently proceed.
        //
        // CAS also supports non-git stores (including verification-only MCP
        // usage) that never enter factory merge enforcement. Preserve that
        // behavior outside factory mode; once factory enforcement is active,
        // an unresolved repository is a hard rejection.
        let has_recorded_merge_evidence = if task.task_type == TaskType::Epic {
            task.branch.is_some()
        } else {
            task.worktree_id.is_some()
                || task.deliverables.factory_branch_anchor.is_some()
                || task.deliverables.parked_branch.is_some()
                || task
                    .assignee
                    .as_deref()
                    .and_then(|assignee| resolve_system_b_worktree_path(&self.cas_root, assignee))
                    .is_some()
        };
        let factory_merge_enforcement =
            std::env::var_os("CAS_FACTORY_MODE").is_some() && has_recorded_merge_evidence;
        // An explicit task work target overrides the factory spawn repo.
        // Resolve once before any merge/reachability query and reuse it.
        let declared_repo_context = match task.deliverables.work_target.as_ref() {
            Some(target) => {
                match crate::mcp::tools::core::task::repo_context::resolve_repo_context(
                    &self.cas_root,
                    target,
                ) {
                    Ok(context) => Some(context),
                    Err(message) => return Ok(Self::tool_error(message)),
                }
            }
            None => None,
        };
        let resolved_close_repo = declared_repo_context
            .as_ref()
            .map(|context| Ok(context.repo_root.clone()))
            .unwrap_or_else(|| resolve_close_gate_repo_root(&self.cas_root));
        let close_repo_verified = resolved_close_repo.is_ok();
        let close_project_root = match resolved_close_repo {
            Ok(repo_root) => repo_root,
            Err(message) if factory_merge_enforcement => {
                return Ok(Self::tool_error(message));
            }
            Err(_) => self
                .cas_root
                .parent()
                .unwrap_or(&self.cas_root)
                .to_path_buf(),
        };

        // For Epics: Check that all worker branches are merged before verification
        // This ensures epic-level verification runs on the complete merged code
        if task.task_type == TaskType::Epic {
            let target_branch = match declared_repo_context
                .as_ref()
                .map(|context| context.target_branch.clone())
                .or_else(|| task.branch.clone())
            {
                Some(branch) => branch,
                None if close_repo_verified => {
                    match resolve_close_gate_default_branch(&close_project_root) {
                        Ok(branch) => branch,
                        Err(message) if factory_merge_enforcement => {
                            return Ok(Self::tool_error(message));
                        }
                        Err(_) => "master".to_string(),
                    }
                }
                None => "master".to_string(),
            };
            let unmerged =
                check_unmerged_epic_branches(&close_project_root, &req.id, &target_branch);
            if !unmerged.is_empty() {
                let branch_list = unmerged.join("\n  - ");
                return Ok(Self::tool_error(format!(
                    "⚠️ MERGE REQUIRED\n\n\
                    Epic {} has {} unmerged worker branch(es):\n  - {}\n\n\
                    Worker branches must be merged to {} before closing the epic.\n\n\
                    Use /factory-merge-epic to:\n\
                    1. Fetch all worker branches from remote\n\
                    2. Merge each branch to {}\n\
                    3. Run tests on the merged code\n\n\
                    After merging, call mcp__cas__task action=close id={} again.",
                    req.id,
                    unmerged.len(),
                    branch_list,
                    target_branch,
                    target_branch,
                    req.id
                )));
            }

            // cas-8f8f: per-child factory-branch merge-state guard for
            // epic close. The check above (`check_unmerged_epic_branches`)
            // operates on the epic's own branch namespace; this gate
            // walks every child task's `factory/<assignee>` branch and
            // rejects when any has stranded commits relative to the
            // epic branch. Bypass-immune (data-state guard, not a
            // review gate). Diagnostic surface for in-flight queries
            // is `mcp__cas__coordination action=epic_status id=<epic>`.
            //
            // Errors from `get_subtasks` MUST surface as a hard error,
            // never a silent empty-list pass. Round-1 cas-code-review
            // (correctness P1) caught the `unwrap_or_default()` failure
            // mode: a transient SQLite error would map to "no children"
            // and the gate would Proceed — defeating the entire
            // enforcement that this task adds. Mirror the conservative
            // pattern at line ~869 (`epic_subtask_receipts_cover`)
            // where a store error is treated as gate-blocking.
            let subtasks = task_store.get_subtasks(&req.id).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "epic-close merge gate: failed to read subtasks of {epic_id}: {e}",
                    epic_id = req.id
                )),
                data: None,
            })?;
            match run_epic_close_merge_gate(
                &task,
                &req,
                &target_branch,
                &close_project_root,
                &subtasks,
            ) {
                EpicCloseGateOutcome::Proceed => {}
                EpicCloseGateOutcome::ProceedWithNote(note) => {
                    append_close_decision_note(task_store.as_ref(), &mut task, &note);
                }
                EpicCloseGateOutcome::Reject(msg) => {
                    return Ok(Self::tool_error(msg));
                }
            }
        }

        // cas-95ce / cas-4b3f / cas-cf64: per-task close-time merge-state
        // guard. Mirrors the shape of the epic check above, but at the
        // worker scope: when a non-epic task with an assignee is being
        // closed, reject if `factory/<assignee>` carries commits that
        // haven't landed on the real integration target. Runs BEFORE the
        // verification policy and the cas-code-review bypass —
        // `bypass_code_review=true` cannot skip this guard because it is a
        // data-state check, not a review gate. See
        // `run_factory_branch_merge_gate` for the full skip matrix and EPIC
        // cas-754b for context.
        //
        // cas-cf64 fix (standalone-task backstop gap): cas-4b3f's fix for
        // BUG-close-guard-nonepic-task-targets-main over-corrected in two
        // ways: (a) it exempted Chore/Spike task types from this gate
        // outright, and (b) it skipped the gate entirely whenever no
        // parent-epic branch resolved. Both left a real hole: a standalone
        // Bug/Chore/Spike task that commits real code to
        // `factory/<assignee>` and never merges it closed cleanly — this
        // gate never ran, and the B2 "merge-reality" gate
        // (`check_factory_branch_merge_reality`) only refuses the
        // zero-commit case, so N>0 committed-unmerged commits sailed
        // through untouched.
        //
        // Fix: run this DATA-STATE check unconditionally for every
        // non-epic task with an assignee — dropping the type exemption.
        // Genuine review/docs/zero-commit tasks of ANY type are still
        // unaffected: `count_unmerged_factory_commits` naturally returns 0
        // when `factory/<assignee>` has nothing to strand, so
        // `run_factory_branch_merge_gate` still Proceeds for them exactly
        // as before — "review/docs tasks close on notes alone" holds
        // without needing a type-based carve-out. When no parent epic
        // branch resolves, resolve the REAL integration target
        // (`resolve_standalone_merge_target`: configured
        // `epic_base_branch`, falling back to git's detected default
        // branch) instead of skipping the gate or guessing `"main"`.
        // cas-7efe: single, authoritative parent-branch resolution for
        // every close-time gate below (merge gate, commit-claim gate,
        // additive-only gate, zero-commit gate, diff stat). Resolved once
        // and reused — previously four of these five sites independently
        // resolved via `task.worktree_id -> worktree_store.get(..).parent_branch`
        // with a silent `.unwrap_or_else(|| "main".to_string())` fallback,
        // which on an epic based on a non-`main` branch (e.g. `staging`)
        // evaluated every downstream gate against the wrong branch — the
        // root cause of the ZERO-COMMIT catch-22
        // (BUG-zero-commit-close-gate-catch22.md) and the 110KB diff-stat
        // overflow (BUG-task-close-returns-110kb-diffstat-overflowing-token-limit.md).
        // See `resolve_close_parent_branch` for the resolution order; it
        // never falls back to a bare `"main"` literal.
        let worktree_store_parent_branch = task.worktree_id.as_deref().and_then(|wt_id| {
            self.open_worktree_store()
                .ok()
                .and_then(|store| store.get(wt_id).ok())
                .map(|wt| wt.parent_branch.clone())
        });
        let epic_parent_branch = task_store
            .get_parent_epic(&req.id)
            .ok()
            .flatten()
            .and_then(|p| p.branch);
        let parent_branch_resolution = if let Some(context) = declared_repo_context.as_ref() {
            Ok(context.target_branch.clone())
        } else if close_repo_verified {
            resolve_close_parent_branch(
                worktree_store_parent_branch,
                epic_parent_branch,
                &close_project_root,
            )
        } else {
            Ok(worktree_store_parent_branch
                .or(epic_parent_branch)
                .unwrap_or_else(|| "main".to_string()))
        };
        let resolved_parent_branch = match parent_branch_resolution {
            Ok(branch) => branch,
            Err(message) if factory_merge_enforcement => {
                return Ok(Self::tool_error(message));
            }
            Err(_) => "main".to_string(),
        };
        // cas-5626: a worker-supplied receipt is attributable only to the
        // current task work cycle. The latest claim/transfer survives the
        // AwaitingMerge park path, while a reopened task gets a newer claim.
        // Fall back to task creation when lease history is unavailable.
        // cas-e74c: resolved unconditionally (it used to be computed only
        // when a receipt was supplied). The merge-state guard now needs the
        // same work-cycle window to tell this task's commits apart from a
        // reused lane's prior-task residue, receipt or no receipt.
        // cas-9596: attribution evidence for commits this task produced in ANY
        // cycle, under any assignee — the parked factory anchor, the durable
        // worker delivery receipt, and the task id itself.
        let task_commit_identity = task_commit_identity(
            &task,
            cas_store::get_latest_worker_delivery(&self.cas_root, &task.id)
                .ok()
                .flatten()
                .map(|(receipt, _)| receipt.commit_sha),
        );
        let commit_receipt_window = {
            let lease_history = self
                .open_agent_store()
                .ok()
                .and_then(|store| store.get_lease_history(&req.id, None).ok())
                .unwrap_or_default();
            Some(resolve_task_commit_receipt_window(
                task.created_at,
                &lease_history,
                task_commit_identity.clone(),
            ))
        };
        // cas-fdc9 (GH #56): a receipt is only evidence if it exists in the
        // repository this close is bound to. The cross-repo delivery in the
        // report supplied a SHA that lived solely in the repo where the work
        // landed, and no gate on that path happened to need the receipt — so
        // an unverifiable commit id was recorded as proof. Check it up front,
        // for every caller and every bypass level, because the failure this
        // prevents is false assurance in the audit trail rather than a
        // premature close. Ancestry, diff and work-cycle checks stay with the
        // gates below; this asks only whether the receipt is ours to verify.
        if let Some(receipt) = req.commit_receipt.as_deref()
            && close_repo_verified
            && let Some(message) = commit_receipt_repo_binding_error(&close_project_root, receipt)
        {
            return Ok(Self::tool_error(message));
        }

        if task.task_type != TaskType::Epic && task.assignee.is_some() {
            match run_factory_branch_merge_gate_with_attribution(
                &task,
                &req,
                &resolved_parent_branch,
                &close_project_root,
                TaskCommitAttribution {
                    receipt: req.commit_receipt.as_deref(),
                    window: commit_receipt_window.as_ref(),
                },
            ) {
                MergeStateGateOutcome::Proceed => {}
                // cas-e74c: the delivery is proven integrated (or nothing on
                // the lane belongs to this task) — record the residue note
                // on the task and let the close continue.
                MergeStateGateOutcome::ProceedWithNote(note) => {
                    append_close_decision_note(task_store.as_ref(), &mut task, &note);
                }
                MergeStateGateOutcome::Reject(msg) => {
                    // cas-a844: "MERGE REQUIRED" alone doesn't say whether the
                    // supervisor's merge will actually succeed — it fires for
                    // ANY stranded commits, whether or not they'd conflict.
                    // Distinguish the two with a read-only preflight (bright-
                    // gopher-20's cas-e18f `preflight_merge_conflicts`, which
                    // uses `git merge-tree --write-tree` and touches neither
                    // the working tree nor the index) so the parked state and
                    // this refusal both name the real situation instead of
                    // reading as "done, pending a formality" either way.
                    let conflict_check = task
                        .assignee
                        .as_deref()
                        .map(|assignee| {
                            factory_branch_merge_conflict_paths(
                                &close_project_root,
                                &resolved_parent_branch,
                                &format!("factory/{assignee}"),
                            )
                        })
                        .unwrap_or_else(|| Ok(Vec::new()));
                    let (conflict_paths, conflict_check_error) =
                        classify_merge_conflict_preflight(conflict_check);
                    // cas-7308a: an unavailable preflight is not evidence that
                    // the branch is clean. Mark the park reopen-eligible so a
                    // transient git error cannot reinstate the awaiting_merge
                    // dead end that cas-5054 removed.
                    let merge_conflicted =
                        !conflict_paths.is_empty() || conflict_check_error.is_some();

                    // cas-a844 AC4: name the alternative in the refusal
                    // itself when the merge genuinely can't succeed — not
                    // just in the parked task's notes. Computed before
                    // parking so the enriched text is what both the parked
                    // task's activity log AND the returned refusal carry.
                    let msg = enrich_merge_required_with_conflict_check(
                        msg,
                        &resolved_parent_branch,
                        &task.id,
                        &conflict_paths,
                        conflict_check_error.as_deref(),
                    );

                    // cas-627f: a worker looping `close` before the
                    // supervisor merges (the documented #1 worker failure
                    // mode) used to re-run `park_task_awaiting_merge` on
                    // EVERY retry — appending a duplicate audit note to
                    // `task.notes` and emitting a duplicate
                    // `WorkerVerificationBlocked` close-rejection activity
                    // event each time, unboundedly. Park (and record the
                    // rejection activity) only the first time a task
                    // transitions into `AwaitingMerge`; once it's already
                    // parked, a retry gets the same rejection message with
                    // no further state mutation.
                    if task.status != TaskStatus::AwaitingMerge {
                        // cas-4b3f: snapshot the factory branch's current
                        // tip so later retries anchor to THIS task's own
                        // commit range, not whatever HEAD drifts to if a
                        // second task starts on the same branch.
                        let anchor = task.assignee.as_deref().and_then(|assignee| {
                            resolve_branch_sha(&close_project_root, &format!("factory/{assignee}"))
                        });
                        self.park_task_awaiting_merge(
                            task_store.as_ref(),
                            &task,
                            "MERGE REQUIRED",
                            &msg,
                            anchor,
                            merge_conflicted,
                        );
                    } else if merge_conflicted && !task.deliverables.merge_conflicted {
                        // Already parked (a retry), but a fresh preflight now
                        // shows a genuine conflict or cannot be evaluated.
                        // Refresh the flag so the worker exit remains open
                        // without duplicating the park audit note.
                        self.mark_awaiting_merge_conflicted(task_store.as_ref(), &task.id);
                    }

                    return Ok(Self::tool_error(msg));
                }
            }
        }

        // Check verification status if enabled
        let config = self.load_config();
        let policy = verification_policy(supervisor_harness_from_env(), worker_harness_from_env());
        let is_factory_worker = std::env::var("CAS_AGENT_ROLE")
            .map(|r| r.eq_ignore_ascii_case("worker"))
            .unwrap_or(false)
            && std::env::var("CAS_FACTORY_MODE").is_ok();
        let verification_enabled = config.verification_enabled()
            && if task.task_type == TaskType::Epic {
                if is_supervisor_from_env() {
                    policy.epic_required()
                } else {
                    true
                }
            } else {
                policy.task_required()
            };

        // cas-8edb: under `[code_review] owner = "supervisor"` (default
        // since cas-865b / v2.13.0), a worker close is a pure transition
        // operation — for reviewable diffs the `supervisor_review_mode`
        // block further down transitions the task to
        // `PendingSupervisorReview`; for additive-only / docs-only /
        // zero-diff shapes the rest of the close pipeline handles the
        // close normally. Either way, the verification-jail path (which
        // arms `pending_verification=true` and dispatches `task-verifier`)
        // is the legacy `owner=worker` mechanism and must not fire for a
        // worker under supervisor-owned review — workers don't submit a
        // `ReviewOutcome` envelope in this mode, so the self-cert
        // short-circuit cannot fire either, leaving every clean close
        // deadlocked. Skip the gate here; the supervisor review queue
        // replaces it.
        //
        // Supervisor-driven close paths are unaffected: `is_factory_worker`
        // is false for supervisors, so `worker_under_supervisor_review`
        // is false and the existing gate runs (with supervisor exemptions
        // already in place).
        let worker_under_supervisor_review = is_factory_worker
            && task.task_type != TaskType::Epic
            && config
                .code_review
                .as_ref()
                .map(|cr| cr.supervisor_owned())
                .unwrap_or_else(|| crate::config::CodeReviewConfig::default().supervisor_owned());

        // Skip verification for orphaned tasks: if caller is supervisor and the
        // task's assignee is inactive (heartbeat expired or lease gone), allow
        // close without verification. cas-3bd4: compute the reason as a typed
        // enum so the response message cites the actual state instead of
        // defaulting to "assignee inactive" for every lookup failure.
        let skip_reason = if verification_enabled && is_supervisor_from_env() {
            if task.task_type == TaskType::Epic && task.epic_verification_owner.is_some() {
                // The ownership gate above already proved that this caller is
                // the configured epic verification owner. Epics normally have
                // no task assignee, so feeding them through the generic helper
                // would mislabel a healthy owner-close as orphan recovery.
                VerificationSkipReason::EpicOwnerClosed
            } else {
                // cas-1932 (GH #62, minor): an assignee that cannot be
                // resolved is not evidence that nothing verified the work.
                // The incident close reported "verification skipped —
                // assignee unknown" while verification ver-fd59de6ef422 was
                // on record for the task, losing the audit linkage. When a
                // current-cycle approved verdict exists, cite it instead.
                skip_reason_with_existing_verification(
                    self.compute_verification_skip_reason(&task, &req),
                    self.current_cycle_approved_verification(
                        &req.id,
                        required_verification_type(task.task_type),
                        commit_receipt_window.as_ref(),
                    )
                    .as_ref(),
                )
            }
        } else {
            VerificationSkipReason::None
        };
        // Orphan/supervisor convenience may not erase an explicit current
        // proof cycle. Once a typed dispatch exists, only its exact verdict or
        // named supervisor-direct recovery can authorize close.
        let exact_dispatch_allows_skip = matches!(
            cas_store::get_latest_verification_dispatch(&self.cas_root, &req.id),
            Ok(None)
                | Ok(Some(cas_types::VerificationDispatch {
                    state: cas_types::VerificationDispatchState::Resolved
                        | cas_types::VerificationDispatchState::Invalidated,
                    ..
                }))
        );
        let skip_verification = skip_reason.is_skip() && exact_dispatch_allows_skip;

        // Also allow supervisor to skip verification jail when they are the
        // task assignee for a non-epic task (fixes supervisor self-close deadlock).
        let supervisor_is_assignee = is_supervisor_from_env()
            && task.task_type != TaskType::Epic
            && self
                .get_agent_id()
                .ok()
                .map(|aid| task.assignee.as_deref() == Some(aid.as_str()))
                .unwrap_or(false);

        // cas-6538: `depth_light` short-circuits the verification jail. The
        // jail arms `pending_verification=true` and demands a `task-verifier`
        // verdict before close; light tasks skip it entirely. For `Deep`/unset
        // (`!depth_light`) the condition is unchanged, so the jail arms exactly
        // as today.
        if verification_enabled
            && !skip_verification
            && !worker_under_supervisor_review
            && !depth_light
        {
            let is_worker_without_subagents = is_worker_without_subagents_from_env();

            // Check for approved verification
            if let Ok(verification_store) = self.open_verification_store() {
                // Determine verification type and agent based on task type
                let is_epic = task.task_type == TaskType::Epic;
                let verification_type = required_verification_type(task.task_type);
                let verifier_agent = "task-verifier";

                // Get the appropriate verification (by type for epics, any for tasks)
                let task_wide_latest = if is_epic {
                    verification_store.get_latest_for_task_by_type(&req.id, verification_type)
                } else {
                    verification_store.get_latest_for_task(&req.id)
                };
                let typed_dispatch =
                    match cas_store::get_latest_verification_dispatch(&self.cas_root, &req.id) {
                        Ok(dispatch) => dispatch,
                        Err(error) => {
                            return Ok(Self::tool_error(format!(
                                "⚠️ VERIFICATION DISPATCH INVALID\n\n\
                                 Task {} has unreadable durable verification-dispatch state: {}. \
                                 CAS refuses to infer authority or recovery from corrupt metadata.",
                                req.id, error
                            )));
                        }
                    };

                // Whether a prior verification row (of any status) already
                // exists. Used below to decide whether to persist a fresh
                // dispatch-request marker so the close attempt is durably
                // observable instead of fire-and-forget.
                let had_prior_verification = matches!(&task_wide_latest, Ok(Some(_)));

                // A verdict authorizes only the exact durable proof cycle it
                // resolved. Task-wide rows remain readable for legacy timeout
                // diagnostics but can never authorize a current close.
                let latest = match typed_dispatch.as_ref() {
                    Some(dispatch)
                        if dispatch.state == cas_types::VerificationDispatchState::Resolved =>
                    {
                        cas_store::get_verification_for_dispatch(&self.cas_root, &dispatch.id)
                    }
                    // Pre-m213 tasks have no typed dispatch boundary. Preserve
                    // their legacy close behavior, but only for rows that also
                    // lack dispatch provenance. As soon as any typed dispatch
                    // exists, this fallback is unreachable and can never
                    // authorize that current cycle.
                    None => match task_wide_latest.as_ref() {
                        Ok(Some(row)) if row.dispatch_id.is_none() => Ok(Some(row.clone())),
                        Ok(_) => Ok(None),
                        Err(_) => Ok(None),
                    },
                    _ => Ok(None),
                };

                // Typed state is authoritative for new dispatches. The legacy
                // Error row remains a readable fallback for pre-m211 databases,
                // but cannot grant authority.
                let now = chrono::Utc::now();
                let in_flight_dispatch = typed_dispatch.as_ref().is_some_and(|dispatch| {
                    matches!(
                        dispatch.state,
                        cas_types::VerificationDispatchState::Pending
                            | cas_types::VerificationDispatchState::Claimed
                    ) && dispatch.deadline_at > now
                }) || (typed_dispatch.is_none()
                    && matches!(&task_wide_latest, Ok(Some(v))
                        if v.status == VerificationStatus::Error
                            && v.summary.starts_with(DISPATCH_SUMMARY_PREFIX)
                            && (now - v.created_at).num_seconds()
                                <= VERIFICATION_DISPATCH_TIMEOUT_SECS));

                if let Some(dispatch) = typed_dispatch.as_ref()
                    && matches!(
                        dispatch.state,
                        cas_types::VerificationDispatchState::Pending
                            | cas_types::VerificationDispatchState::Claimed
                    )
                    && dispatch.deadline_at <= now
                {
                    let timed_out = cas_store::timeout_verification_dispatch(
                        &self.cas_root,
                        &req.id,
                        now,
                    )
                    .map_err(|error| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Failed to persist verification timeout: {error}"
                        )),
                        data: None,
                    })?
                    .ok_or_else(|| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(
                            "Exact verification timeout changed before persistence; retry close.",
                        ),
                        data: None,
                    })?;
                    let mut task_to_update = task.clone();
                    task_to_update.pending_verification = false;
                    task_to_update.updated_at = now;
                    task_store
                        .update(&task_to_update)
                        .map_err(|error| McpError {
                            code: ErrorCode::INTERNAL_ERROR,
                            message: Cow::from(format!(
                                "Failed to recover timed-out task transition: {error}"
                            )),
                            data: None,
                        })?;
                    if let Ok(agent_store) = self.open_agent_store() {
                        let _ = agent_store.release_lease_for_task(
                            &req.id,
                            "Verification dispatch timed out: supervisor recovery required",
                        );
                    }
                    let elapsed_mins = (now - timed_out.requested_at).num_seconds() / 60;
                    let sup_ver = supervisor_verification_tool();
                    return Ok(Self::tool_error(format!(
                        "⚠️ VERIFICATION TIMED OUT\n\n\
                         Task {} waited {} minutes for dispatch {} without a verdict. \
                         Only this task's dispatch was marked timed_out and its lease released.\n\n\
                         Recovery ({}): a registered supervisor must re-dispatch a task-verifier \
                         or record a direct verdict with {sup_ver}, then retry this task's close.",
                        req.id, elapsed_mins, timed_out.id, timed_out.recovery_action
                    )));
                }

                match latest {
                    Ok(Some(v))
                        if (v.status == VerificationStatus::Approved
                            || v.status == VerificationStatus::Skipped)
                            && v.verification_type == verification_type =>
                    {
                        // Verification approved or explicitly skipped
                        // (supervisor bypass row from a prior orphaned close) —
                        // proceed with close. See cas-82d6.
                    }
                    Ok(Some(v)) if v.status == VerificationStatus::Rejected => {
                        // Verification rejected, block close
                        // Only auto-claim if the closing agent is the task's assignee.
                        // If a supervisor closes a worker's task, skip the lease to avoid
                        // locking the task to the supervisor.
                        let is_assignee = self
                            .get_agent_id()
                            .ok()
                            .map(|aid| task.assignee.as_deref() == Some(aid.as_str()))
                            .unwrap_or(false);
                        if is_assignee {
                            self.auto_claim_for_verification(&req.id, task_store.as_ref())?;
                        }

                        let issue_count = v.issues.len();
                        let blocking = v
                            .issues
                            .iter()
                            .filter(|i| i.severity == crate::types::IssueSeverity::Blocking)
                            .count();

                        // Include new close reason if provided (may have been fixed)
                        let close_reason_note = if let Some(ref reason) = req.reason {
                            format!(
                                "\n\n## New Close Reason Provided\n\
                                ```\n{reason}\n```\n\n\
                                If resubmitting, ensure the close reason describes COMPLETED work only.\n\
                                Do not use language like 'remaining', 'beyond scope', 'will need to', etc."
                            )
                        } else {
                            String::new()
                        };

                        return Ok(Self::tool_error(format!(
                            "⚠️ VERIFICATION FAILED\n\n\
                            Task {} has a rejected verification with {} issue(s) ({} blocking).\n\n\
                            Summary: {}\n\n\
                            {}{}\n\n\
                            {}",
                            req.id,
                            issue_count,
                            blocking,
                            v.summary,
                            if is_worker_without_subagents {
                                // cas-8aaf: use harness-appropriate tool aliases.
                                let sup_ver = supervisor_verification_tool();
                                format!(
                                    "To fix: Address the issues in this worker.\n\
                                     Then ask supervisor to run verification \
                                     (task-verifier or direct {sup_ver}) \
                                     and close the task on your behalf."
                                )
                            } else {
                                format!(
                                    "To fix: Address the issues and run the {verifier_agent} agent again."
                                )
                            },
                            close_reason_note,
                            if is_worker_without_subagents {
                                // cas-8aaf: use harness-appropriate coordination tool.
                                let coord = worker_coordination_tool();
                                let sup_ver = supervisor_verification_tool();
                                format!(
                                    "Suggested message: {coord} action=message target=supervisor \
                                     message=\"Task {id} is ready for re-verification. \
                                     Please verify (task-verifier or direct {sup_ver}) \
                                     and close if approved.\"",
                                    id = req.id
                                )
                            } else {
                                format!(
                                    "To verify: Task(subagent_type=\"{}\", prompt=\"Verify task {}\")",
                                    verifier_agent, req.id
                                )
                            }
                        )));
                    }
                    Ok(Some(ref v))
                        if v.status == VerificationStatus::Error
                            && v.summary.starts_with(DISPATCH_SUMMARY_PREFIX)
                            && typed_dispatch.is_none()
                            && (chrono::Utc::now() - v.created_at).num_seconds()
                                > VERIFICATION_DISPATCH_TIMEOUT_SECS =>
                    {
                        // Stale dispatch-request row: the task-verifier subagent was
                        // supposed to write a verdict but never did. This is the
                        // within-task verification deadlock from cas-c29a. Auto-escalate
                        // so the supervisor sees a clean failure instead of an infinite
                        // VERIFICATION REQUIRED loop.
                        let elapsed_mins = (chrono::Utc::now() - v.created_at).num_seconds() / 60;

                        // Clear pending_verification so the jail releases.
                        let mut task_to_update = task.clone();
                        task_to_update.pending_verification = false;
                        task_to_update.updated_at = chrono::Utc::now();
                        if let Err(e) = task_store.update(&task_to_update) {
                            tracing::warn!(task_id = %req.id, error = %e, "failed to clear pending_verification on task");
                        }

                        // Release any lease so the supervisor can reclaim the task.
                        if let Ok(agent_store) = self.open_agent_store() {
                            let _ = agent_store.release_lease_for_task(
                                &req.id,
                                "Verification timed out: supervisor escalation",
                            );
                        }

                        // Replace the stale dispatch row with a timeout diagnostic so
                        // the audit trail shows escalation instead of a dangling
                        // "Dispatch requested" row.
                        let mut timeout_row = v.clone();
                        timeout_row.summary = format!(
                            "Verification timed out after {elapsed_mins} minutes — \
                             task-verifier subagent never recorded a verdict. \
                             Auto-escalated by cas_task_close: pending_verification cleared, \
                             lease released. Supervisor must re-dispatch verifier or record \
                             verdict manually."
                        );
                        timeout_row.created_at = chrono::Utc::now();
                        if let Err(e) =
                            cas_store::update_system_verification(&self.cas_root, &timeout_row)
                        {
                            tracing::warn!(task_id = %req.id, error = %e, "failed to update verification timeout row");
                        }

                        // Surface an activity event so the TUI shows the escalation.
                        if let Ok(agent_id) = self.get_agent_id() {
                            let event = crate::mcp::socket::DaemonEvent::WorkerActivity {
                                session_id: agent_id,
                                event_type: "verification_timeout_escalated".to_string(),
                                description: format!(
                                    "Verification timed out ({elapsed_mins}m): {}",
                                    req.id
                                ),
                                entity_id: Some(req.id.clone()),
                            };
                            let _ = crate::mcp::socket::send_event(&self.cas_root, &event);
                        }

                        // cas-7998: the manual-verdict fallback runs in the
                        // supervisor's harness (the supervisor is the one who
                        // re-dispatches or records the verdict), so the direct
                        // verification alias must track the supervisor CLI —
                        // hardcoding mcp__cas__verification hands a Codex
                        // supervisor an alias they cannot call.
                        let sup_ver = supervisor_verification_tool();
                        return Ok(Self::tool_error(format!(
                            "⚠️ VERIFICATION TIMED OUT\n\n\
                            Task {} was awaiting verification for {} minutes with no verdict \
                            from the task-verifier subagent. Auto-escalated: this task transition \
                            was released and its lease freed.\n\n\
                            This usually means the task-verifier subagent crashed, was never \
                            spawned, or failed silently.\n\n\
                            To proceed:\n\
                            1. Re-dispatch verifier: Task(subagent_type=\"task-verifier\", prompt=\"Verify task {}\")\n\
                            2. Or record verdict directly: {sup_ver} action=add task_id={} status=approved summary=\"...\"\n\
                            3. Then call cas_task_close again.",
                            req.id, elapsed_mins, req.id, req.id
                        )));
                    }
                    Ok(None) | Ok(Some(_)) => {
                        // No verification or pending/error status.
                        //
                        // cas-778a: factory-worker-owned verification short-circuit.
                        // If the factory worker provides a structurally valid
                        // ReviewOutcome envelope with no P0 in residual or
                        // pre_existing, the cas-code-review autofix pipeline IS the
                        // worker's verification step. Skip task-verifier dispatch —
                        // write a Skipped row for the audit trail and fall through
                        // to let the close proceed. Workers without a clean
                        // envelope (or without any envelope) continue to the
                        // existing jail-arming path below.
                        //
                        // Note: the downstream code_review_gate (below) also
                        // applies the full forgery defence (cas-4c64): Check A
                        // blocks any P0 in residual[] regardless of the
                        // per-finding pre_existing flag, and Check B blocks any
                        // P0 in pre_existing[]. Both this predicate and
                        // run_code_review_gate enforce the defence symmetrically.
                        // If you tighten either, tighten both.
                        let envelope_str = req
                            .code_review_findings
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty());
                        // cas-164c: suppress self-cert when a fresh task-verifier
                        // dispatch row is already in-flight.  The running subagent's
                        // verdict must be allowed to land before the close can
                        // short-circuit.  Stale dispatch rows (age >
                        // VERIFICATION_JAIL_TIMEOUT_SECS) are handled by the
                        // timeout-escalation arm above and do NOT set
                        // `in_flight_dispatch`, so self-cert still works once the
                        // timeout window expires.
                        let worker_owns_verification = is_factory_worker
                            && envelope_str.is_some_and(worker_review_envelope_is_clean)
                            && !in_flight_dispatch;

                        if worker_owns_verification {
                            // Eagerly persist the envelope and clear any stale
                            // pending_verification=true via a clone so the intermediate
                            // DB state is consistent even if the close fails later.
                            // The in-memory `task` variable is re-applied at line 1022
                            // (after `let mut task = task`) so the final close update
                            // also carries these fields. Both paths are needed: the
                            // intermediate persist catches early returns; the in-memory
                            // update ensures the final task_store.update(&task) wins.
                            if let Some(envelope) = envelope_str {
                                let mut task_to_persist = task.clone();
                                task_to_persist.deliverables.review_envelope =
                                    Some(envelope.to_string());
                                // Clear any stale pending_verification flag left by a
                                // prior jail-arming close attempt.
                                task_to_persist.pending_verification = false;
                                task_to_persist.updated_at = chrono::Utc::now();
                                if let Err(e) = task_store.update(&task_to_persist) {
                                    tracing::warn!(
                                        task_id = %req.id,
                                        error = %e,
                                        "failed to persist review envelope on worker self-verify"
                                    );
                                }
                            }
                            // Write a Skipped verification row for the audit trail
                            // so the bypass reason is permanently recorded and the
                            // exact-task close gate sees a satisfying row on retry.
                            //
                            // cas-c97e (Option B): if the write fails, fall through
                            // rather than abort the close — audit completeness is less
                            // critical than close-path correctness. But the failure must
                            // NOT be silent: emit a DaemonEvent::WorkerActivity with
                            // event_type="audit_trail_gap" so the supervisor TUI surfaces
                            // the missing record without halting the worker.
                            match verification_store.generate_id() {
                                Ok(ver_id) => {
                                    let mut skipped_row = Verification::skipped(
                                        ver_id,
                                        req.id.clone(),
                                        "Worker-owned verification: cas-code-review autofix \
                                             returned clean ReviewOutcome envelope"
                                            .to_string(),
                                    );
                                    skipped_row.verification_type = verification_type;
                                    skipped_row.provenance =
                                        cas_types::VerificationProvenance::System;
                                    // cas-eeab (Item 6): cache get_agent_id() once to avoid
                                    // the double-call that existed between the row assignment
                                    // and the gap-event emission on the add() failure path.
                                    let maybe_agent_id = self.get_agent_id().ok();
                                    if let Some(ref aid) = maybe_agent_id {
                                        skipped_row.agent_id = Some(aid.clone());
                                    }
                                    if let Err(e) = cas_store::add_system_verification(
                                        &self.cas_root,
                                        &skipped_row,
                                    ) {
                                        tracing::warn!(
                                            task_id = %req.id,
                                            error = %e,
                                            "failed to persist worker-owned verification Skipped row"
                                        );
                                        // cas-eeab (Item 4+5): delegate to helper; sentinel
                                        // fallback ensures the event fires even when the
                                        // agent ID is unavailable.
                                        self.emit_audit_gap_event(
                                            &req.id,
                                            format!(
                                                "Skipped verification row write failed \
                                                 for task {}: {e}",
                                                req.id
                                            ),
                                        );
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        task_id = %req.id,
                                        error = %e,
                                        "failed to generate ID for worker-owned verification Skipped row"
                                    );
                                    // cas-c97e: ID-generation failure also surfaces as an
                                    // audit-gap event — it is indistinguishable from a write
                                    // failure from the supervisor's perspective.
                                    // cas-eeab (Item 4+5): delegate to helper.
                                    self.emit_audit_gap_event(
                                        &req.id,
                                        format!(
                                            "Skipped verification row ID generation failed \
                                             for task {}: {e}",
                                            req.id
                                        ),
                                    );
                                }
                            }
                            // Emit activity event for supervisor visibility.
                            if let Ok(agent_id) = self.get_agent_id() {
                                let event = crate::mcp::socket::DaemonEvent::WorkerActivity {
                                    session_id: agent_id,
                                    event_type: "worker_verification_self_certified".to_string(),
                                    description: format!(
                                        "Worker-owned verification passed: {}",
                                        req.id
                                    ),
                                    entity_id: Some(req.id.clone()),
                                };
                                let _ = crate::mcp::socket::send_event(&self.cas_root, &event);
                            }
                            // Fall through — do NOT arm jail, do NOT return error.
                        } else {
                            // No clean envelope from a factory worker: proceed with
                            // the standard verification-jail path.

                            // Only auto-claim if the closing agent is the task's assignee.
                            // If a supervisor closes a worker's task, skip the lease to avoid
                            // locking the task to the supervisor.
                            let is_assignee = self
                                .get_agent_id()
                                .ok()
                                .map(|aid| task.assignee.as_deref() == Some(aid.as_str()))
                                .unwrap_or(false);
                            if is_assignee {
                                self.auto_claim_for_verification(&req.id, task_store.as_ref())?;
                            }

                            // Mark only this task's close transition pending.
                            let mut task_to_update = task.clone();
                            task_to_update.pending_verification = true;
                            if task_to_update.assignee.is_none() {
                                if let Ok(agent_id) = self.get_agent_id() {
                                    task_to_update.assignee = Some(agent_id);
                                }
                            }
                            // cas-3086: persist the worker's ReviewOutcome envelope on
                            // the task deliverables so a subsequent supervisor close
                            // (once verification approves) can forward the prior review
                            // receipt into the P0 gate instead of re-running the
                            // multi-persona reviewer or requiring `bypass_code_review`.
                            // We persist only non-empty envelopes; validation happens
                            // later in `run_code_review_gate`, which rejects malformed
                            // persisted envelopes so bad input cannot silently bypass
                            // the gate.
                            if let Some(envelope) = req
                                .code_review_findings
                                .as_deref()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                            {
                                task_to_update.deliverables.review_envelope =
                                    Some(envelope.to_string());
                            }
                            task_to_update.updated_at = chrono::Utc::now();
                            if let Err(e) = task_store.update(&task_to_update) {
                                tracing::warn!(task_id = %req.id, error = %e, "failed to set pending_verification on task");
                            }

                            // Include close reason in the message so verifier can check it
                            let close_reason_section = if let Some(ref reason) = req.reason {
                                format!(
                                    "\n\n## Proposed Close Reason\n\
                                    ```\n{reason}\n```\n\n\
                                    IMPORTANT: The {verifier_agent} MUST validate this close reason.\n\
                                    Reject if it admits incomplete work (e.g., 'remaining items', 'beyond scope', 'will need to')."
                                )
                            } else {
                                String::new()
                            };

                            let verification_desc = if is_epic {
                                "Epic verification runs on master to verify the complete merged implementation.\n\
                                The agent will check that all subtask implementations integrate correctly.\n\
                                The verifier MUST record verification_type=epic."
                            } else {
                                "The agent will check for TODO comments, stubs, incomplete implementations,\n\
                                AND validate the close reason doesn't admit incomplete work."
                            };

                            // Send verification blocked activity event (for supervisor visibility)
                            if let Ok(agent_id) = self.get_agent_id() {
                                let event = crate::mcp::socket::DaemonEvent::WorkerActivity {
                                    session_id: agent_id,
                                    event_type: "worker_verification_blocked".to_string(),
                                    description: format!("Awaiting verification: {}", req.id),
                                    entity_id: Some(req.id.clone()),
                                };
                                let _ = crate::mcp::socket::send_event(&self.cas_root, &event);
                            }

                            let requester_id = self.get_agent_id()?;
                            let owner_id = self.verification_dispatch_owner(&requester_id)?;
                            let supervisor_recovery = self
                                .open_agent_store()?
                                .get(&requester_id)
                                .is_ok_and(|agent| {
                                    agent.role == cas_types::AgentRole::Supervisor
                                        && agent.is_alive()
                                });
                            let dispatch = cas_store::create_verification_dispatch_bound(
                                &self.cas_root,
                                &req.id,
                                &requester_id,
                                &owner_id,
                                &cas_types::VerificationProofBoundary::task(),
                                chrono::Utc::now()
                                    + chrono::Duration::seconds(VERIFICATION_DISPATCH_TIMEOUT_SECS),
                                supervisor_recovery,
                            )
                            .map_err(|error| McpError {
                                code: ErrorCode::INTERNAL_ERROR,
                                message: Cow::from(format!(
                                    "Failed to persist verification dispatch: {error}"
                                )),
                                data: None,
                            })?;

                            // Keep the legacy Error row for old clients and
                            // audit views. It is descriptive only: typed dispatch
                            // state and server-derived authority control new adds.
                            if !had_prior_verification {
                                if let Ok(ver_id) = verification_store.generate_id() {
                                    let mut dispatch_row =
                                        Verification::new(ver_id, req.id.clone());
                                    dispatch_row.verification_type = verification_type;
                                    dispatch_row.status = VerificationStatus::Error;
                                    dispatch_row.agent_id = Some(owner_id.clone());
                                    dispatch_row.provenance =
                                        cas_types::VerificationProvenance::System;
                                    dispatch_row.issuer_agent_id = Some(requester_id);
                                    dispatch_row.summary = format!(
                                        "Dispatch requested ({}) — task-verifier subagent must be spawned via \
                                         Task(subagent_type=\"task-verifier\", prompt=\"Verify task {}\"). \
                                         This row will be superseded by the subagent's verdict.",
                                        dispatch.id, req.id
                                    );
                                    if let Err(e) = cas_store::add_system_verification(
                                        &self.cas_root,
                                        &dispatch_row,
                                    ) {
                                        tracing::warn!(task_id = %req.id, error = %e, "failed to persist verification dispatch row");
                                    }
                                }
                            }

                            let verification_gate = if is_factory_worker {
                                // cas-8aaf: use harness-appropriate coordination tool alias.
                                // Claude workers use mcp__cas__coordination, Codex workers
                                // use mcp__cs__coordination (CAS_FACTORY_WORKER_CLI drives
                                // the selection via worker_coordination_tool()).
                                let coord = worker_coordination_tool();
                                // cas-7998: escape the free-text reason so a
                                // quote/newline can't break the quoted
                                // `message="..."` argument below.
                                let close_reason_hint = req
                                    .reason
                                    .as_deref()
                                    .map(|r| {
                                        format!(
                                            " Close reason: {}.",
                                            escape_close_reason_for_quoted_command(r)
                                        )
                                    })
                                    .unwrap_or_default();
                                format!(
                                    "Factory worker verification gate: task {id} close is pending \
                                     dispatch {dispatch_id}, owned by {owner}, deadline {deadline}. \
                                     This close will only succeed after a legitimate verifier records a verdict.\n\n\
                                     Forward to supervisor (workers cannot spawn task-verifier directly):\n\n\
                                     {coord} action=message target=supervisor \
                                     summary=\"Ready to close {id}\" \
                                     message=\"Task {id} is ready to close.{close_reason_hint} \
                                     Please run task-verifier for task {id} and close on my behalf if approved.\"",
                                    id = req.id,
                                    dispatch_id = dispatch.id,
                                    owner = dispatch.owner_agent_id,
                                    deadline = dispatch.deadline_at,
                                )
                            } else if supervisor_is_assignee {
                                // cas-7998: the supervisor self-verifies in their
                                // own harness, so the direct verification alias
                                // must match the supervisor CLI (mcp__cs__ for a
                                // Codex supervisor, mcp__cas__ for Claude).
                                let sup_ver = supervisor_verification_tool();
                                format!(
                                    "You implemented this task yourself. Spawn a task-verifier to review your work:\n\n\
                                     Task(subagent_type=\"{}\", prompt=\"Verify task {}\")\n\n\
                                     Or record verification directly:\n\
                                     {sup_ver} action=add task_id={} \
                                     status=approved summary=\"Self-verified: <reason>\"",
                                    verifier_agent, req.id, req.id
                                )
                            } else {
                                format!(
                                    "Task {} close is pending verification dispatch {} owned by {} \
                                     until {}.\n\n\
                                     Use the Task tool to spawn a task-verifier subagent: \
                                     Task(subagent_type=\"{}\", prompt=\"Verify task {}\")",
                                    req.id,
                                    dispatch.id,
                                    dispatch.owner_agent_id,
                                    dispatch.deadline_at,
                                    verifier_agent,
                                    req.id
                                )
                            };

                            return Ok(Self::tool_error(format!(
                                "⚠️ VERIFICATION REQUIRED\n\n\
                                Task {} requires verification before closing.\n\n\
                                {}{}\n\n\
                                {}{}\n\n\
                                {}",
                                req.id,
                                verification_gate,
                                verification_desc,
                                close_reason_section.as_str(),
                                if is_worker_without_subagents {
                                    // cas-8aaf: harness-appropriate supervisor verification tool.
                                    let sup_ver = supervisor_verification_tool();
                                    format!(
                                        "Ask supervisor to run verification \
                                         (task-verifier or direct {sup_ver}) \
                                         and close task {} on your behalf.",
                                        req.id
                                    )
                                } else {
                                    String::new()
                                },
                                if is_worker_without_subagents {
                                    // cas-8aaf: harness-appropriate coordination tool.
                                    let coord = worker_coordination_tool();
                                    let sup_ver = supervisor_verification_tool();
                                    format!(
                                        "Suggested message: {coord} action=message \
                                         target=supervisor message=\"Please verify task {id} \
                                         (task-verifier or direct {sup_ver}) \
                                         and close it if approved.\"",
                                        id = req.id
                                    )
                                } else {
                                    "After verification passes, call cas_task_close again."
                                        .to_string()
                                }
                            )));
                        }
                    }
                    Err(_) => {
                        // Verification store error, proceed anyway
                    }
                }
            }
        }

        // Check for worktree that needs merging (only for epics or tasks with worktrees)
        // This check happens AFTER verification passes
        if let Some(worktree_id) = &task.worktree_id {
            let config = self.load_config();

            // Only trigger jail if worktrees are enabled and require_merge_on_epic_close is true
            let should_check_worktree = config
                .worktrees
                .as_ref()
                .map(|wc| wc.enabled && wc.require_merge_on_epic_close)
                .unwrap_or(false);

            if should_check_worktree {
                if let Ok(wt_store) = self.open_worktree_store() {
                    if let Ok(worktree) = wt_store.get(worktree_id) {
                        // Check if worktree still exists, is active, and hasn't been merged
                        // Skip jail if: removed, merged status, or has merged_at timestamp
                        let needs_merge = worktree.removed_at.is_none()
                            && worktree.status == WorktreeStatus::Active
                            && worktree.merged_at.is_none();

                        if needs_merge {
                            // Set pending_worktree_merge flag to enable worktree jail
                            let mut task_to_update = task.clone();
                            task_to_update.pending_worktree_merge = true;
                            if task_to_update.assignee.is_none() {
                                if let Ok(agent_id) = self.get_agent_id() {
                                    task_to_update.assignee = Some(agent_id);
                                }
                            }
                            task_to_update.updated_at = chrono::Utc::now();
                            if let Err(e) = task_store.update(&task_to_update) {
                                tracing::warn!(task_id = %req.id, error = %e, "failed to set pending_worktree_merge on task");
                            }

                            return Ok(Self::tool_error(format!(
                                "⚠️ WORKTREE MERGE REQUIRED\n\n\
                                Task {} has an associated worktree that needs to be merged before closing.\n\n\
                                📍 Worktree: {}\n\
                                🌿 Branch: {}\n\n\
                                🔒 WORKTREE JAIL ACTIVE: You cannot use other tools until you spawn the 'worktree-merger' agent.\n\n\
                                To merge: Spawn the 'worktree-merger' agent to:\n\
                                1. Check for uncommitted changes and commit them\n\
                                2. Push the branch to remote\n\
                                3. Merge the branch to the parent branch\n\
                                4. Clean up the worktree directory\n\n\
                                After the merge completes, call cas_task_close again.",
                                req.id,
                                worktree.path.display(),
                                worktree.branch
                            )));
                        }
                    }
                }
            }
        }

        // cas-895d + cas-bc1b (follow-up): close-gate checks that inspect
        // the worker's worktree are scoped *only* to tasks with an
        // isolated worker worktree (`task.worktree_id` set).
        //
        // Non-isolated tasks (`isolate=false` in spawn_workers) run
        // directly in the main cas-src worktree, which is routinely
        // dirty during an active session: supervisor edits in flight,
        // shared ops editing shared files, or simply unrelated drift.
        // Running either close gate against the main worktree would
        // reject every close in that mode and reintroduce the exact
        // wrong-worktree-scope bug cas-bc1b was filed to fix.
        //
        // `resolve_worker_worktree_path` returns `None` for non-isolated
        // tasks, and both gates below key off that Option to decide
        // whether to fire at all. For non-isolated tasks the close
        // path relies on cas-code-review (cas-b39f) + verification
        // (task-verifier) as the quality bar — those gates operate on
        // commits / review envelopes, not on working-tree state, so
        // they're safe to run in a shared worktree.
        let bypass_close_gates =
            req.bypass_code_review.unwrap_or(false) && is_supervisor_from_env();
        let worker_worktree_path =
            match self.resolve_worker_worktree_path(&task, declared_repo_context.as_ref()) {
                Ok(path) => path,
                Err(message) => return Ok(Self::tool_error(message)),
            };
        // Explicit work targets opt into a fail-closed executable gate on
        // every close path, independent of review owner/depth/bypass. This
        // keeps normal close aligned with direct update-to-closed: neither
        // may select a process-cwd or merely most-recent worker checkout.
        let declared_hook_evidence = if let Some(context) = declared_repo_context.as_ref() {
            match run_declared_pre_close_hook(
                &task,
                context,
                worker_worktree_path.as_deref(),
                req.commit_receipt.as_deref(),
            ) {
                Ok(evidence) => Some(evidence),
                Err(message) => {
                    return Ok(Self::tool_error(format!(
                        "⚠️ PRE-CLOSE HOOK FAILED\n\n{message}"
                    )));
                }
            }
        } else {
            None
        };

        // cas-895d: uncommitted work gate.
        //
        // The pre-cas-895d close path had no backstop checking that the
        // worker's claimed deliverables were actually committed. A
        // worker could complete a task, run tests, hit `task.close`, pass
        // verification, and successfully close — all while leaving the
        // actual edits **uncommitted** in the working tree. When the
        // worker's isolated worktree was later GC'd, the work was lost.
        //
        // The gate runs `git status --porcelain` scoped to the worker's
        // own worktree. Any non-`??` status line counts as uncommitted
        // tracked work — untracked files (`??`) are ignored because
        // they never belonged to the task in the first place.
        //
        // Scope: tasks with a resolved worker worktree only. Non-
        // isolated tasks skip this gate entirely per the comment above.
        //
        // Supervisors can bypass this gate with `bypass_code_review=true`,
        // matching the same "trust me" pattern used by the cas-b39f
        // code-review gate. Non-supervisors get a hard reject pointing
        // them at the dirty files.
        //
        // Graceful degradation: if the worktree path is not a git repo
        // or git fails, the check silently no-ops. The gate is advisory
        // when git state is unknowable.
        if !bypass_close_gates {
            if let Some(worker_wt) = worker_worktree_path.as_ref() {
                let uncommitted = check_uncommitted_work(worker_wt);
                if !uncommitted.is_empty() {
                    let file_list = uncommitted
                        .iter()
                        .map(|u| format!("  {}  {}", u.status, u.path))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Ok(Self::tool_error(format!(
                        "⚠️ UNCOMMITTED WORK\n\n\
                        task close rejected: the worker's tree has uncommitted tracked \
                        changes. Closing now would lose the work when the worktree is \
                        cleaned up.\n\n\
                        📂 Checked worktree: {}\n\n\
                        Dirty files:\n{file_list}\n\n\
                        To resolve:\n\
                        1. Review the diff: `git status`\n\
                        2. Stage and commit your changes with a meaningful message.\n\
                        3. Re-run `mcp__cas__task action=close id={}`.\n\n\
                        Supervisors may bypass this gate with bypass_code_review=true \
                        (logged as a decision note) when the worker is stuck and the \
                        work on disk is genuinely disposable.",
                        worker_wt.display(),
                        req.id
                    )));
                }
            }
        }

        // cas-490f: commit-claim integrity gate.
        //
        // The cas-ba91 incident: a factory worker fabricated a commit SHA
        // and code_review_findings against a branch with 0 actual commits.
        // The supervisor lost ~10 min before detecting the fabrication.
        //
        // Gate logic: when a worker provides non-empty `code_review_findings`
        // they are asserting "I wrote code and had it reviewed." This gate
        // verifies that assertion by counting commits on the worker branch
        // beyond its parent. 0 commits + findings = fabrication → hard reject.
        //
        // Firing conditions (all required):
        //   1. Task has a resolved worker worktree (non-isolated tasks skip).
        //   2. `code_review_findings` is non-empty (fabrication claim present).
        //   3. commit count on HEAD vs parent_branch == 0.
        //
        // Supervisors can bypass with `bypass_code_review=true` (same pattern
        // as other gates — logged, escape-hatched for genuine edge cases).
        if !bypass_close_gates {
            if let Some(worker_wt) = worker_worktree_path.as_ref() {
                let has_findings = req
                    .code_review_findings
                    .as_deref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false);
                if has_findings {
                    // cas-7efe: use the single close-time resolver instead
                    // of an independent worktree-only lookup that fell
                    // back to a bare "main".
                    match check_commit_claim_integrity(
                        worker_wt,
                        &resolved_parent_branch,
                        true,
                        task.deliverables.factory_branch_anchor.as_deref(),
                        req.commit_receipt.as_deref(),
                        commit_receipt_window.as_ref(),
                    ) {
                        CommitClaimGateOutcome::Reject(msg) => {
                            return Ok(Self::tool_error(msg));
                        }
                        CommitClaimGateOutcome::Proceed => {}
                        CommitClaimGateOutcome::ProceedWithReceipt(note) => {
                            append_close_decision_note(task_store.as_ref(), &mut task, &note);
                        }
                    }
                }
            }
        }

        // cas-e235 + cas-bc1b: additive-only execution_note backstop.
        //
        // If the worker declared `execution_note=additive-only`, reject
        // the close if git sees any modified, deleted, or renamed files
        // in the task's committed history.
        //
        // cas-bc1b: pre-fix, this check ran
        // `git diff --name-status HEAD` inside `self.cas_root.parent()`
        // (the *main* worktree) regardless of whether the task had an
        // attached worker worktree. Two cascading problems:
        //
        // 1. Factory workers commit their work on an isolated branch.
        //    The main worktree's `git status` has **no semantic
        //    relationship** to what the worker did — a stray dirty
        //    `Cargo.lock` in the main repo would fail an
        //    `additive-only` close on a pristine worker branch (the
        //    cas-4333 incident).
        // 2. Workers who do the right thing and commit everything on
        //    their branch produce an empty `git diff HEAD` inside
        //    their own worktree too, because the commits aren't
        //    "uncommitted diff". So the gate wouldn't see violations
        //    even in the correct worktree.
        //
        // Fix (option (a) from the task description): diff the worker
        // branch's committed history against its parent-branch merge
        // base (`git diff <parent>...HEAD` inside the worker's
        // worktree). Commits only — immune to CWD confusion.
        //
        // Non-isolated tasks skip this gate entirely: there's no
        // distinct worker branch to diff against `main`, so the check
        // has nothing to reason about. Earlier iterations fell through
        // to a legacy `git diff HEAD` path on the main worktree — that
        // path has been deleted in this commit because it reintroduced
        // the exact wrong-worktree-scope bug cas-bc1b was filed to fix.
        if task.execution_note.as_deref() == Some("additive-only") {
            if let Some(worker_wt) = worker_worktree_path.as_ref() {
                // cas-7efe: use the single close-time resolver instead of
                // an independent worktree-only lookup that fell back to a
                // bare "main".
                let violations = check_additive_only_branch_violations(
                    worker_wt,
                    &resolved_parent_branch,
                    task.deliverables.factory_branch_anchor.as_deref(),
                    &task_commit_identity,
                );
                if !violations.is_empty() {
                    let file_list = violations
                        .iter()
                        .map(|v| format!("  {} ({})", v.path, v.status))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Ok(Self::tool_error(format!(
                        "⚠️ ADDITIVE-ONLY VIOLATION\n\n\
                        task close rejected: execution_note=additive-only but diff contains \
                        modifications.\n\n\
                        Modified/deleted/renamed files:\n{file_list}\n\n\
                        Use execution_note=null or test-first to modify existing files."
                    )));
                }
            }
        }

        // cas-b39f: cas-code-review P0 close gate (Unit 9).
        //
        // This is the integration point for the multi-persona code review
        // pipeline. The *dispatch* of the review skill itself happens via
        // the worker's harness (the skill must be invoked through the
        // Task tool by an LLM, not from Rust), so the Phase 1 gate works
        // in three cooperating layers:
        //
        //   1. Skip conditions (here) — additive-only tasks, non-code
        //      diffs, and supervisor overrides bypass the gate before
        //      any review is attempted.
        //   2. The pure-Rust decision helper at
        //      `cas_store::code_review::close_gate::evaluate_gate` —
        //      given a residual finding set, returns Allow or
        //      BlockOnP0. Exhaustively unit-tested there.
        //   3. Graceful degradation — if the review pipeline is
        //      unavailable (skill not installed, orchestrator crash,
        //      no findings-cache entry), log a warning and allow the
        //      close. The task description is explicit: code review
        //      must not become a SPOF for closes.
        //
        // Supervisor override flow:
        //   * Caller sets `bypass_code_review=true` on the close
        //     request.
        //   * If `CAS_AGENT_ROLE=supervisor`, the gate is skipped and
        //     a decision note is appended to the task capturing who
        //     overrode and the close reason.
        //   * Any other caller setting the flag gets an explicit
        //     rejection — we do not silently ignore unauthorized
        //     overrides because that would mask a misconfigured
        //     harness.
        // cas-ee2b: resolve the effective "has reviewable changes" signal and
        // the parent branch for worker git operations.
        //
        // For isolated worker worktrees, checking the main repo's working-tree
        // state (`has_reviewable_changes(close_project_root)`) gives the wrong
        // answer: the main repo may have dirty files unrelated to the worker's
        // task (supervisor edits, other in-flight work). This caused false
        // CODE_REVIEW_REQUIRED on research/spike tasks with zero code commits
        // (the cas-cabc incident).
        //
        // Fix (cas-ee2b): for tasks with a resolved isolated worker worktree,
        // use `has_worker_committed_reviewable_changes` which inspects only
        // what the worker committed on their branch (merge-base..HEAD). For
        // non-isolated tasks, fall through to the existing main-repo check —
        // those workers share the main worktree so its state IS the task diff.
        //
        // cas-7efe: reuse the single close-time resolved parent branch
        // (computed once above) for lint / merge-reality / case-3
        // ambiguity, instead of an independent lookup that fell through to
        // a hard-coded `"main"` whenever `task.worktree_id` was unset — the
        // common System-B factory-isolation case (`spawn_workers
        // isolate=true` almost never sets `task.worktree_id`; see
        // `resolve_close_parent_branch`).
        //
        // cas-1932 (GH #62 symptom 2): the shared-checkout branch above read
        // "the checkout is dirty" as "the task wrote code". In the incident a
        // characterization-only spike closed in a main checkout carrying ~64
        // files of prior-factory WIP and was answered with
        // CODE_REVIEW_REQUIRED for changes it never made. Shared closes now
        // route on commits attributable to this task's work cycle; see
        // `shared_checkout_has_reviewable_changes` for the exact fallbacks
        // (code tasks with no no-code declaration, and unknowable git state,
        // keep the previous signal).
        let effective_has_reviewable = if let Some(worker_wt) = worker_worktree_path.as_ref() {
            has_worker_committed_reviewable_changes(worker_wt, &resolved_parent_branch)
        } else {
            shared_checkout_has_reviewable_changes(SharedCheckoutReviewScope {
                task_type: task.task_type,
                execution_note: task.execution_note.as_deref(),
                attributable_reviewable_changes: commit_receipt_window.as_ref().and_then(
                    |window| {
                        has_task_attributable_reviewable_changes(
                            &close_project_root,
                            &resolved_parent_branch,
                            window,
                        )
                    },
                ),
                checkout_has_reviewable_changes: has_reviewable_changes(&close_project_root),
            })
        };

        // cas-762e (B2): factory branch merge-reality gate.
        //
        // The cas-95ce gate (earlier in this function) already blocks when
        // factory/<assignee> has >0 stranded commits. But 0 unmerged commits
        // is ambiguous: either (a) the work was merged via PR — correct — or
        // (b) the worker never committed to their factory branch and the
        // commits landed elsewhere (the cas-073f bug). B2 distinguishes the
        // two cases: if the factory branch exists locally, has 0 commits
        // beyond the parent branch, AND was never pushed to origin (no remote
        // tracking ref), the close is refused.
        //
        // Bypass conditions mirror the supervisor-review block below:
        //   * Epic tasks — not a per-worker task.
        //   * `execution_note = "additive-only"` — no commit expected by spec.
        //   * `bypass_close_gates` — supervisor emergency bypass.
        //   * `!effective_has_reviewable` — zero diff; no commits needed.
        //   * `task.assignee.is_none()` — orphaned task; nothing to check.
        //   * Non-factory caller (`!is_factory_worker`) — supervisors are
        //     exempt because they may close on behalf of a worker.
        if is_factory_worker
            && task.task_type != TaskType::Epic
            && task.execution_note.as_deref() != Some("additive-only")
            && !bypass_close_gates
            && effective_has_reviewable
        {
            if let Some(assignee) = task.assignee.as_deref() {
                // cas-7efe: single close-time resolver, not a bare "main".
                match check_factory_branch_merge_reality(
                    &close_project_root,
                    assignee,
                    &resolved_parent_branch,
                ) {
                    MergeRealityOutcome::Proceed => {}
                    MergeRealityOutcome::Refuse(msg) => {
                        return Ok(Self::tool_error(msg));
                    }
                }
            }
        }

        // cas-b51a: supervisor-owned review mode.
        //
        // When `[code_review] owner = "supervisor"` AND the caller is a
        // factory worker AND the task has reviewable code changes, skip the
        // full 14-min multi-persona dispatch and instead:
        //   1. Run the lightweight structural lint (<1s).
        //   2. On lint pass, flip the task to `PendingSupervisorReview` and
        //      return success. The supervisor picks up the review queue at
        //      their own pace.
        //   3. On lint fail, return an error so the worker fixes the basics
        //      before the branch reaches the review queue.
        //
        // Bypass conditions (fall through to normal close):
        //   * `bypass_code_review=true` by a supervisor — they can always
        //     force-close regardless of mode.
        //   * Epic tasks — the subtask-receipts gate handles epics; the
        //     supervisor-owned path is for individual worker tasks.
        //   * Additive-only tasks — already skipped by the gate below.
        //   * `has_reviewable_changes` returns false — docs-only or empty
        //     diff; normal close path is appropriate.
        //   * `owner=worker` — legacy opt-out path, unchanged.
        //
        // When the `[code_review]` section is absent entirely, fall through to
        // `CodeReviewConfig::default().supervisor_owned()` so the runtime gate
        // tracks the same default as the config layer (cas-865b: default is
        // "supervisor").  The old `.unwrap_or(false)` hard-coded worker mode
        // for absent sections, making the config-layer default ineffective.
        let supervisor_review_mode = config
            .code_review
            .as_ref()
            .map(|cr| cr.supervisor_owned())
            .unwrap_or_else(|| crate::config::CodeReviewConfig::default().supervisor_owned());

        // cas-6538: under `owner = "supervisor"` the worker close normally
        // transitions to `PendingSupervisorReview` — that queue hop IS the P0
        // code-review gate for this mode (the supervisor runs cas-code-review
        // off the queue). For `depth_light` we treat the P0 gate as satisfied,
        // so skip the pend-transition and let the close complete immediately
        // (AC: "close succeeds", demo: "closes immediately"). `Deep`/unset is
        // unaffected — `!depth_light` keeps the transition firing as today.
        // cas-1932 (GH #62 symptom 1): the queue hop is the review gate for
        // this mode — but once the supervisor has recorded an APPROVED verdict
        // for this work cycle, that gate is satisfied. Before this fix the
        // worker's re-close re-queued the task to `PendingSupervisorReview`
        // forever: the approved verification on record was never consulted
        // here, so no close by the worker could ever complete and the
        // supervisor had to close on their behalf.
        //
        // Deliberately scoped to the supervisor-owned review path: there the
        // supervisor's verdict IS the code review. Under `owner = "worker"`
        // the verification jail and the cas-code-review envelope remain two
        // independent gates and neither may stand in for the other.
        let review_queue_verdict = if supervisor_review_mode
            && is_factory_worker
            && task.task_type != TaskType::Epic
            && !bypass_close_gates
        {
            self.current_cycle_approved_verification(
                &req.id,
                required_verification_type(task.task_type),
                commit_receipt_window.as_ref(),
            )
        } else {
            None
        };
        if supervisor_review_mode
            && is_factory_worker
            && task.task_type != TaskType::Epic
            && task.execution_note.as_deref() != Some("additive-only")
            && !bypass_close_gates
            && effective_has_reviewable
            && !depth_light
            && review_queue_verdict.is_none()
        {
            // cas-dc5d: scope lightweight lint to the closing worker's
            // worktree + committed task range (merge-base..HEAD), never
            // the shared main checkout's working-tree WIP. Sibling gates
            // (cas-ee2b / cas-bc1b) already use this authority; lint was
            // the remaining caller of bare `close_project_root`.
            let lint_outcome = if declared_hook_evidence.is_some() {
                LightweightLintOutcome::Pass
            } else if let Some(worker_wt) = worker_worktree_path.as_ref() {
                // cas-7efe: single close-time resolver, not a bare "main".
                run_lightweight_structural_lint_with_scope(
                    worker_wt,
                    Some(resolved_parent_branch.as_str()),
                )
            } else {
                run_lightweight_structural_lint(&close_project_root)
            };
            match lint_outcome {
                LightweightLintOutcome::Fail(msg) => {
                    return Ok(Self::tool_error(format!(
                        "⚠️ LIGHTWEIGHT LINT FAILED\n\n\
                        Worker close (supervisor-review mode) rejected by structural lint.\n\n\
                        {msg}\n\n\
                        Fix the violations above and retry close."
                    )));
                }
                LightweightLintOutcome::Pass => {
                    // Transition to PendingSupervisorReview.
                    let mut task_to_pend = task.clone();
                    let now = chrono::Utc::now();
                    task_to_pend.status = TaskStatus::PendingSupervisorReview;
                    task_to_pend.updated_at = now;
                    task_to_pend.deliverables.pre_close_hook = declared_hook_evidence.clone();
                    // Persist the close reason so the supervisor can see it.
                    if let Some(ref reason) = req.reason {
                        task_to_pend.close_reason = Some(reason.clone());
                        let timestamp = now.format("%Y-%m-%d %H:%M");
                        let note = format!(
                            "[{timestamp}] Pending supervisor review — close reason: {reason}"
                        );
                        if task_to_pend.notes.is_empty() {
                            task_to_pend.notes = note;
                        } else {
                            task_to_pend.notes = format!("{}\n\n{}", task_to_pend.notes, note);
                        }
                    }
                    if let Err(e) = task_store.update(&task_to_pend) {
                        tracing::warn!(
                            task_id = %req.id,
                            error = %e,
                            "failed to transition task to pending_supervisor_review"
                        );
                        return Ok(Self::tool_error(format!(
                            "Internal error: failed to update task status: {e}"
                        )));
                    }

                    // Emit activity event so the supervisor TUI shows the
                    // new review item immediately.
                    if let Ok(agent_id) = self.get_agent_id() {
                        let event = crate::mcp::socket::DaemonEvent::WorkerActivity {
                            session_id: agent_id,
                            event_type: "worker_pending_supervisor_review".to_string(),
                            description: format!("Task ready for supervisor review: {}", req.id),
                            entity_id: Some(req.id.clone()),
                        };
                        let _ = crate::mcp::socket::send_event(&self.cas_root, &event);
                    }

                    // cas-7fe9: release the worker's lease so the supervisor
                    // can claim the task immediately for review. Without this,
                    // the worker holds a phantom lease for ~10 min and
                    // `task action=claim` by the supervisor is blocked.
                    if let Ok(agent_store) = self.open_agent_store() {
                        let _ = agent_store
                            .release_lease_for_task(&req.id, "Queued for supervisor review");
                    }

                    return Ok(Self::success(format!(
                        "Task {} queued for supervisor review\n\n\
                        Lightweight structural lint passed (<1s). The full \
                        cas-code-review skill will be dispatched by the supervisor.\n\n\
                        Status: pending_supervisor_review\n\n\
                        You can now pick up the next task immediately. \
                        The supervisor will either:\n\
                        - Approve → closes + merges your branch\n\
                        - Reject → sends P0 findings back via coordination message",
                        req.id
                    )));
                }
            }
        }

        // cas-3086: Epic-close should not re-gate on the union diff
        // when every subtask already carries a valid ReviewOutcome
        // receipt (persisted on deliverables.review_envelope). The
        // subtasks were each individually reviewed before their own
        // close; running the multi-persona reviewer on the unioned
        // diff is redundant cost and wrong-shape signal.
        let epic_subtask_receipts_cover = if task.task_type == TaskType::Epic {
            match task_store.get_subtasks(&req.id) {
                Ok(subtasks) => epic_subtask_receipts_are_clean(&subtasks),
                Err(_) => false,
            }
        } else {
            false
        };

        // cas-ee2b: three-case routing for zero-reviewable-changes closes.
        // See `check_zero_commit_close` for the full decision tree.
        //
        // Case 2 (fabrication: findings provided + 0 commits) is already
        // handled by the cas-490f gate above; never reaches here.
        //
        // Cases 1/3/4 are handled here: docs-only, ambiguous zero-commit,
        // and deliberate no-code respectively.
        let gate_outcome = if depth_light {
            // cas-6538: light tasks treat the P0 code-review gate as satisfied.
            // This is the non-factory / solo close path (the factory worker
            // supervisor-review hop is already skipped above for light); a
            // direct close that reaches the gate must not be blocked by a
            // missing review envelope. `Deep`/unset falls through to the
            // existing routing, so the gate enforces exactly as today.
            CodeReviewGateOutcome::Proceed
        } else if review_queue_verdict.is_some() {
            // cas-1932: in supervisor-owned mode the recorded approval IS the
            // completed review, so it satisfies this gate too — otherwise the
            // close would clear the queue hop only to be refused for a missing
            // review envelope the supervisor already replaced. The audit note
            // naming the verdict is written further below, on the same task
            // the final store write uses.
            CodeReviewGateOutcome::Proceed
        } else if epic_subtask_receipts_cover {
            CodeReviewGateOutcome::Proceed
        } else if !effective_has_reviewable {
            let has_review_findings = req
                .code_review_findings
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            // Only run the case-3 gate for isolated-worker tasks that
            // are not supervisor-bypassed.
            if !bypass_close_gates {
                if let Some(worker_wt) = worker_worktree_path.as_ref() {
                    // cas-7efe: single close-time resolver, not the
                    // independently-derived `worker_review_parent_branch`
                    // that used to fall back to a bare "main".
                    match check_zero_commit_close(
                        worker_wt,
                        &resolved_parent_branch,
                        &req.id,
                        &task.task_type,
                        task.execution_note.as_deref(),
                        has_review_findings,
                        // cas-127f: parked tip from MERGE REQUIRED — proves
                        // real work even when merge-base..HEAD is now empty.
                        task.deliverables.factory_branch_anchor.as_deref(),
                        req.commit_receipt.as_deref(),
                        commit_receipt_window.as_ref(),
                    ) {
                        ZeroCommitCloseOutcome::AmbiguousCodeTask(msg) => {
                            return Ok(Self::tool_error(msg));
                        }
                        ZeroCommitCloseOutcome::Proceed => {}
                        ZeroCommitCloseOutcome::ProceedWithReceipt(note) => {
                            append_close_decision_note(task_store.as_ref(), &mut task, &note);
                        }
                    }
                }
            }
            // Cases 1 and 4: allow close — docs-only commits or deliberate
            // no-code (spike, chore, execution_note set, bypass).
            CodeReviewGateOutcome::Proceed
        } else {
            run_code_review_gate(&task, &req, &close_project_root, supervisor_review_mode)
        };
        match gate_outcome {
            CodeReviewGateOutcome::Proceed => {}
            CodeReviewGateOutcome::AppendDecisionNote(note) => {
                let mut t = task.clone();
                if t.notes.is_empty() {
                    t.notes = note;
                } else {
                    t.notes = format!("{}\n\n{}", t.notes, note);
                }
                t.updated_at = chrono::Utc::now();
                if let Err(e) = task_store.update(&t) {
                    tracing::warn!(task_id = %req.id, error = %e, "failed to append code review decision note");
                }
            }
            CodeReviewGateOutcome::Reject(msg) => {
                return Ok(Self::tool_error(msg));
            }
        }

        // cas-49f1: zero-hit search-manifest guardrail for investigation
        // (Spike) tasks. Never blocks close — worst case is a loud warning
        // note on the task's audit trail. Computed here (read-only) but
        // applied to `task.notes` after the mutable shadow below, alongside
        // the `depth_light` decision note — the eager clone-persist pattern
        // used by the code-review gate above gets silently overwritten by
        // the final `task_store.update(&task)` write, so a note that must
        // survive close has to land on the same in-memory `task` that write
        // uses.
        let search_manifest_warning = match run_search_manifest_gate(&task, &req) {
            SearchManifestGateOutcome::Proceed => None,
            SearchManifestGateOutcome::AppendWarningNote(note) => Some(note),
        };

        // Proceed with close
        let now = chrono::Utc::now();
        // cas-062d: capture pre-close status for durable lifecycle push identity.
        let old_status_for_lifecycle = task.status;
        task.status = TaskStatus::Closed;
        task.closed_at = Some(now);
        task.updated_at = now;
        task.deliverables.pre_close_hook = declared_hook_evidence;
        // cas-eaf8: preserve the task-specific factory anchor after close.
        // The epic close guard needs this durable receipt to distinguish
        // this task's merged work from later, unrelated commits added when
        // the same worker branch is reused for another epic. The task-store
        // update boundary clears the anchor on every Closed -> non-Closed
        // transition before rework starts, preserving cas-cf64's stale-anchor
        // protection even when callers bypass the dedicated reopen action.

        // cas-778a: apply worker-owned verification fields to the now-mutable
        // `task` so the final task_store.update(&task) below carries them.
        // The intermediate clone-persist above writes the DB eagerly; this
        // ensures the in-memory value used for `deliverables` capture below
        // and the final write are consistent with the intermediate state.
        if let Some(envelope) = req
            .code_review_findings
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if is_factory_worker && worker_review_envelope_is_clean(envelope) {
                task.deliverables.review_envelope = Some(envelope.to_string());
                task.pending_verification = false;
            }
        }

        // Capture deliverables on close
        let mut deliverables = task.deliverables.clone();
        if let Some(worktree_id) = &task.worktree_id {
            if let Ok(wt_store) = self.open_worktree_store() {
                if let Ok(worktree) = wt_store.get(worktree_id) {
                    if let Some(commit) = worktree.merge_commit.clone() {
                        deliverables.merge_commit = Some(commit);
                    }
                }
            }
        }
        task.deliverables = deliverables;

        // When closing via the supervisor bypass (assignee inactive / orphaned /
        // supervisor-forced), we skip the verification gate but MUST still
        // write a durable `Skipped` verification row. Without this row, the
        // exact-task close gate treats the task as unverified on retry.
        //
        // cas-3bd4: the Skipped row now records the *actual* skip reason
        // (from `VerificationSkipReason::audit_reason`) instead of the
        // catch-all "assignee inactive or orphaned task" string.
        if skip_verification && verification_enabled {
            if let Ok(verification_store) = self.open_verification_store() {
                let needs_row = verification_store
                    .get_latest_for_task(&req.id)
                    .map(|v| {
                        !matches!(
                            v,
                            Some(ref r) if r.status == VerificationStatus::Approved
                                || r.status == VerificationStatus::Skipped
                        )
                    })
                    .unwrap_or(true);
                if needs_row {
                    if let Ok(ver_id) = verification_store.generate_id() {
                        let mut row = Verification::skipped(
                            ver_id,
                            req.id.clone(),
                            skip_reason.audit_reason(),
                        );
                        row.verification_type = if task.task_type == TaskType::Epic {
                            VerificationType::Epic
                        } else {
                            VerificationType::Task
                        };
                        row.provenance = cas_types::VerificationProvenance::System;
                        if let Ok(agent_id) = self.get_agent_id() {
                            row.agent_id = Some(agent_id);
                        }
                        if let Err(e) = cas_store::add_system_verification(&self.cas_root, &row) {
                            tracing::warn!(task_id = %req.id, error = %e, "failed to persist verification skip row");
                        }
                    }
                }
            }
        }

        // cas-6538: record an auditable decision note for the light-depth skip
        // BEFORE the close-reason note, so the bypass is permanently visible in
        // the task timeline. Persisted via the single `task_store.update(&task)`
        // below (the final write), which guarantees it survives — unlike the
        // earlier gate clone-persist paths that the final write overwrites.
        if depth_light {
            let timestamp = now.format("%Y-%m-%d %H:%M");
            let decision_note = format!("[{timestamp}] {}", light_skip_decision_note());
            if task.notes.is_empty() {
                task.notes = decision_note;
            } else {
                task.notes = format!("{}\n\n{}", task.notes, decision_note);
            }
        }

        // cas-1932: record which supervisor verdict authorized this close on
        // the same in-memory task the final write uses. The code-review gate's
        // eager clone-persist above is overwritten by that write, and this
        // audit linkage — the thing GH #62's "verification skipped" minor was
        // about — has to survive the close.
        if let Some(verdict) = review_queue_verdict.as_ref() {
            let timestamp = now.format("%Y-%m-%d %H:%M");
            let note = format!(
                "[{timestamp}] DECISION: close authorized by approved verification {} \
                 recorded {} — supervisor review already complete, task not re-queued.",
                verdict.id,
                verdict.created_at.to_rfc3339(),
            );
            if task.notes.is_empty() {
                task.notes = note;
            } else {
                task.notes = format!("{}\n\n{}", task.notes, note);
            }
        }

        // cas-49f1: apply the zero-hit search-manifest warning (computed
        // above, before the mutable shadow) directly to the in-memory task
        // so it survives the final `task_store.update(&task)` write below.
        if let Some(note) = search_manifest_warning {
            if task.notes.is_empty() {
                task.notes = note;
            } else {
                task.notes = format!("{}\n\n{}", task.notes, note);
            }
        }

        if let Some(reason) = &req.reason {
            task.close_reason = Some(reason.clone());
            let timestamp = now.format("%Y-%m-%d %H:%M");
            let close_note = format!("[{timestamp}] Closed: {reason}");
            if task.notes.is_empty() {
                task.notes = close_note;
            } else {
                task.notes = format!("{}\n\n{}", task.notes, close_note);
            }
        }

        task_store.update(&task).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        // cas-062d / cas-17e4: durable Closed outbox push after successful close.
        {
            let actor = self
                .get_agent_id()
                .ok()
                .and_then(|id| {
                    self.open_agent_store()
                        .ok()
                        .and_then(|s| s.get(&id).ok())
                        .map(|a| a.name)
                })
                .unwrap_or_else(|| "unknown".into());
            let occurrence = super::supervisor_push::occurrence_from_updated_at(task.updated_at);
            if let Err(e) = self.push_task_lifecycle(
                &req.id,
                &task.title,
                old_status_for_lifecycle,
                TaskStatus::Closed,
                &actor,
                req.reason.as_deref(),
                super::supervisor_push::LifecycleTransition::Closed,
                &occurrence,
            ) {
                let key = super::supervisor_push::transition_key(
                    &req.id,
                    old_status_for_lifecycle,
                    TaskStatus::Closed,
                    std::env::var("CAS_FACTORY_SESSION").ok().as_deref(),
                    super::supervisor_push::LifecycleTransition::Closed,
                    &occurrence,
                );
                return Err(Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    super::supervisor_push::lifecycle_push_failure_message(
                        &req.id,
                        TaskStatus::Closed,
                        super::supervisor_push::LifecycleTransition::Closed,
                        &key,
                        &e,
                    ),
                ));
            }

            let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
                &self.cas_root,
                "task_closed",
                &[
                    ("task_id", &req.id),
                    ("title", &task.title),
                    ("actor", &actor),
                    ("reason", req.reason.as_deref().unwrap_or("")),
                ],
            );
        }

        // Auto-unblock tasks that were blocked solely by this task.
        // This keeps dependency state and task status synchronized.
        let mut auto_unblocked_tasks: Vec<String> = Vec::new();
        if let Ok(dependents) = task_store.get_dependents(&req.id) {
            for dep in dependents
                .iter()
                .filter(|dep| dep.dep_type == DependencyType::Blocks)
            {
                if let Ok(mut dependent_task) = task_store.get(&dep.from_id) {
                    if dependent_task.status != TaskStatus::Blocked {
                        continue;
                    }
                    let is_unblocked = task_store
                        .get_blockers(&dependent_task.id)
                        .map(|blockers| blockers.is_empty())
                        .unwrap_or(false);
                    if !is_unblocked {
                        continue;
                    }
                    dependent_task.status = TaskStatus::Open;
                    dependent_task.updated_at = chrono::Utc::now();
                    let timestamp = dependent_task.updated_at.format("%Y-%m-%d %H:%M");
                    let unblock_note = format!(
                        "[{}] Auto-unblocked: all blockers closed (latest: {}).",
                        timestamp, req.id
                    );
                    if dependent_task.notes.is_empty() {
                        dependent_task.notes = unblock_note;
                    } else {
                        dependent_task.notes =
                            format!("{}\n\n{}", dependent_task.notes, unblock_note);
                    }
                    if task_store.update(&dependent_task).is_ok() {
                        // cas-062d / cas-17e4: ready/reopened outbox for auto-unblocked dependents.
                        let dep_id = dependent_task.id.clone();
                        let dep_title = dependent_task.title.clone();
                        let occurrence = super::supervisor_push::occurrence_from_updated_at(
                            dependent_task.updated_at,
                        );
                        let actor = self
                            .get_agent_id()
                            .ok()
                            .and_then(|id| {
                                self.open_agent_store()
                                    .ok()
                                    .and_then(|s| s.get(&id).ok())
                                    .map(|a| a.name)
                            })
                            .unwrap_or_else(|| "unknown".into());
                        if let Err(e) = self.push_task_lifecycle(
                            &dep_id,
                            &dep_title,
                            TaskStatus::Blocked,
                            TaskStatus::Open,
                            &actor,
                            Some("auto-unblocked"),
                            super::supervisor_push::LifecycleTransition::ReadyReopened,
                            &occurrence,
                        ) {
                            tracing::error!(
                                task_id = %dep_id,
                                error = %e,
                                "supervisor lifecycle push failed after auto-unblock (task remains Open; replay outbox)"
                            );
                        }
                        auto_unblocked_tasks.push(dep_id);
                    }
                }
            }
        }

        // Track epic completion with subtask count and duration
        if task.task_type == TaskType::Epic {
            let subtasks = task_store.get_subtasks(&req.id).unwrap_or_default();
            let duration_mins = task
                .closed_at
                .zip(Some(task.created_at))
                .map(|(closed, created)| (closed - created).num_minutes().max(0) as u64)
                .unwrap_or(0);
            crate::telemetry::track_epic_completed(subtasks.len(), duration_mins);
        }

        // Release any lease on this task (regardless of who owns it)
        let lease_msg = if let Ok(agent_store) = self.open_agent_store() {
            match agent_store.release_lease_for_task(&req.id, "Task closed") {
                Ok(true) => " (lease released)",
                Ok(false) => "",
                Err(_) => "",
            }
        } else {
            ""
        };

        // cas-3bd4: use the typed skip reason so the audit suffix cites
        // the real reason (e.g. "assignee unknown" for name/id mismatches,
        // "supervisor bypass" for explicit overrides) instead of always
        // saying "assignee inactive".
        let verification_note = skip_reason.response_suffix(verification_enabled);

        // Note about worktree status (merge already handled by worktree-merger agent)
        let worktree_msg = if let Some(worktree_id) = &task.worktree_id {
            if let Ok(wt_store) = self.open_worktree_store() {
                if let Ok(worktree) = wt_store.get(worktree_id) {
                    if worktree.removed_at.is_some() {
                        // Worktree was merged and cleaned up by worktree-merger
                        format!("\n🌳 Worktree merged (branch: {})", worktree.branch)
                    } else {
                        // Worktree still exists - this shouldn't happen if jail worked correctly
                        format!("\n⚠️ Worktree still exists at {}", worktree.path.display())
                    }
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        // Check if this task is a subtask of an epic, and if all siblings are now closed
        let epic_close_msg = {
            // Get dependencies of this task to find its parent
            let deps = task_store.get_dependencies(&req.id).unwrap_or_default();
            let parent_dep = deps
                .iter()
                .find(|d| d.dep_type == cas_types::DependencyType::ParentChild);

            if let Some(dep) = parent_dep {
                // Get the parent task
                if let Ok(parent) = task_store.get(&dep.to_id) {
                    // Check if parent is an Epic
                    if parent.task_type == cas_types::TaskType::Epic
                        && parent.status != TaskStatus::Closed
                    {
                        // Get all subtasks of this epic
                        let subtasks = task_store.get_subtasks(&parent.id).unwrap_or_default();

                        // Check if all subtasks are now closed
                        let all_closed = subtasks.iter().all(|t| t.status == TaskStatus::Closed);

                        if all_closed && !subtasks.is_empty() {
                            // In factory mode, workers shouldn't close epics - supervisor handles that
                            let is_factory_worker = std::env::var("CAS_AGENT_ROLE")
                                .map(|r| r.to_lowercase() == "worker")
                                .unwrap_or(false);

                            if is_factory_worker {
                                // Send real notification to supervisor via daemon event
                                if let Ok(agent_id) = self.get_agent_id() {
                                    let event = crate::mcp::socket::DaemonEvent::WorkerActivity {
                                        session_id: agent_id,
                                        event_type: "epic_subtasks_complete".to_string(),
                                        description: format!(
                                            "All subtasks of epic '{}' ({}) are complete — ready to close",
                                            parent.title, parent.id
                                        ),
                                        entity_id: Some(parent.id.clone()),
                                    };
                                    let _ = crate::mcp::socket::send_event(&self.cas_root, &event);
                                }

                                format!(
                                    "\n\n🎉 All subtasks of epic '{}' ({}) are now complete!\n\
                                     → The supervisor has been notified to close the epic.",
                                    parent.title, parent.id
                                )
                            } else {
                                format!(
                                    "\n\n🎉 All subtasks of epic '{}' ({}) are now complete!\n\
                                     → Consider closing the epic with: mcp__cas__task action=close id={}",
                                    parent.title, parent.id, parent.id
                                )
                            }
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        };

        // Check if commit nudge is enabled
        let commit_nudge = config.tasks().commit_nudge_on_close;
        let commit_nudge_msg =
            if commit_nudge && worktree_msg.is_empty() && epic_close_msg.is_empty() {
                "\n\n💡 Consider committing your changes for this completed task."
            } else {
                ""
            };

        let auto_unblock_msg = if auto_unblocked_tasks.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n🔓 Auto-unblocked task(s): {}",
                auto_unblocked_tasks.join(", ")
            )
        };

        // cas-490f: surface the actual `git diff --stat` for the worker's
        // committed branch so supervisors see real deltas at close time,
        // without having to inspect the worktree manually. This is included
        // unconditionally when a resolved worker worktree is available —
        // the stat is an objective record of what was committed, independent
        // of whatever the worker's close reason claims.
        let diff_stat_msg = if let Some(worker_wt) = worker_worktree_path.as_ref() {
            // cas-7efe: single close-time resolver, not a bare "main" —
            // this used to diff against the wrong branch (e.g. the entire
            // staging/main divergence) whenever `task.worktree_id` was
            // unset, producing the 110KB diff-stat overflow.
            let stat = get_worker_diff_stat(worker_wt, &resolved_parent_branch);
            if stat.is_empty() {
                String::new()
            } else {
                format!("\n\n📊 Committed diff stat (vs {resolved_parent_branch}):\n{stat}")
            }
        } else {
            String::new()
        };

        Ok(Self::success(format_close_success_message(
            &req.id,
            &task.title,
            &verification_note,
            lease_msg,
            &worktree_msg,
            &diff_stat_msg,
            &epic_close_msg,
            commit_nudge_msg,
            &auto_unblock_msg,
        )))
    }

    /// Resolve the filesystem path for the **worker's isolated
    /// worktree**, if this task has one.
    ///
    /// Returns `Some(worktree_path)` for factory tasks spawned with
    /// `isolate=true` — there's a distinct git worktree per worker,
    /// resolved from `task.worktree_id` → `WorktreeStore`. This is the
    /// only surface where worktree-scoped close gates
    /// (cas-895d uncommitted-work, cas-bc1b additive-only) should fire.
    ///
    /// Returns `None` when NEITHER system below resolves — i.e. "this
    /// task does not have a worker-owned worktree to check":
    ///
    /// 1. `task.worktree_id` is absent, the worktree store can't be
    ///    opened, or the row exists but has been `removed_at` / its
    ///    on-disk path no longer exists (System A — see below) — AND
    /// 2. no `<cas_root>/worktrees/<assignee>` directory exists either
    ///    (System B — see below).
    ///
    /// A task with genuinely no worker worktree (non-isolated
    /// `spawn_workers isolate=false`, or a direct CLI flow) hits both
    /// misses and correctly returns `None`; checking the main worktree
    /// would reject every close because of unrelated in-flight work
    /// from the supervisor or other non-isolated workers. This was the
    /// pre-cas-895d/cas-bc1b follow-up bug.
    ///
    /// ## Two independent worktree systems (cas-4b3f)
    ///
    /// - **System A** — `WorktreeStore` rows keyed by `task.worktree_id`.
    ///   Set ONLY for epic-type tasks when `[worktrees] enabled = true`
    ///   (`cas_task_start`, `lifecycle.rs:563` — "Worktrees are scoped
    ///   to epics, not individual tasks"), and that config flag is
    ///   "experimental and disabled by default" per the `worktree_*`
    ///   MCP action gate. In practice this System is rarely populated
    ///   for ordinary worker tasks.
    /// - **System B** — factory worker isolation from
    ///   `spawn_workers isolate=true` (the actual day-to-day factory
    ///   path). Each worker gets a real git worktree at the fixed
    ///   convention `<cas_root>/worktrees/<assignee>` on branch
    ///   `factory/<assignee>` — but this is created directly by the
    ///   worktree manager and is **never written back onto the task
    ///   row's `worktree_id`** (confirmed: `worktree_id = Some(...)`
    ///   has exactly one call site in the whole crate, and it's the
    ///   epic-only System-A path above).
    ///
    /// Before this fix, `resolve_worker_worktree_path` only ever
    /// consulted System A. For the overwhelmingly common case — a
    /// single (non-epic) worker task closed by its System-B-isolated
    /// factory worker — `task.worktree_id` is always `None`, so this
    /// returned `None` unconditionally and EVERY gate gated on it
    /// (cas-895d uncommitted-work, cas-490f commit-claim, cas-762e/B2
    /// merge-reality, cas-ee2b zero-commit ambiguity, cas-bc1b
    /// additive-only) silently no-opped for real factory workers —
    /// exactly the "closed while uncommitted / never-pushed" data-loss
    /// near-miss from BUG-merge-gate-inconsistent-close-without-integration.
    /// This fix adds System B as a fallback so those gates actually run.
    pub(crate) fn resolve_worker_worktree_path(
        &self,
        task: &cas_types::Task,
        declared_repo_context: Option<&crate::mcp::tools::core::task::repo_context::RepoContext>,
    ) -> Result<Option<std::path::PathBuf>, String> {
        let system_a = task.worktree_id.as_deref().and_then(|worktree_id| {
            self.open_worktree_store()
                .ok()
                .and_then(|store| store.get(worktree_id).ok())
                .filter(|wt| wt.removed_at.is_none() && wt.path.exists())
        });
        // Preserve legacy System-A precedence when no explicit repository was
        // declared. Explicit targets instead reject multiple distinct
        // candidates rather than guessing which checkout owns the task.
        if declared_repo_context.is_none()
            && let Some(worktree) = system_a.as_ref()
        {
            return Ok(Some(worktree.path.clone()));
        }
        let system_b = task
            .assignee
            .as_deref()
            .and_then(|assignee| match declared_repo_context {
                Some(context) => resolve_system_b_worktree_path_for_repo(
                    &self.cas_root,
                    &context.repo_root,
                    assignee,
                ),
                None => resolve_system_b_worktree_path(&self.cas_root, assignee),
            });
        let Some(expected) = declared_repo_context else {
            return Ok(system_b);
        };
        if let (Some(worktree), Some(system_b_path)) = (system_a.as_ref(), system_b.as_ref())
            && worktree.path != *system_b_path
        {
            return Err(
                "PRE-CLOSE HOOK CONTEXT REJECTED: multiple distinct task worktrees match the \
                 declared target. No close-time executable gate was run."
                    .to_string(),
            );
        }
        if let Some(worktree) = system_a {
            validate_pre_close_worktree(&worktree.path, expected, Some(&worktree.branch))?;
            if let Some(assignee) = task.assignee.as_deref() {
                let expected_branch = format!("factory/{assignee}");
                validate_pre_close_worktree(&worktree.path, expected, Some(&expected_branch))?;
            }
            return Ok(Some(worktree.path));
        }
        if let Some(path) = system_b.as_ref() {
            let assignee = task
                .assignee
                .as_deref()
                .expect("System B requires assignee");
            let expected_branch = format!("factory/{assignee}");
            validate_pre_close_worktree(path, expected, Some(&expected_branch))?;
        }
        Ok(system_b)
    }

    /// cas-1932 (GH #62): the APPROVED verification for this task's current
    /// work cycle, if one is on record.
    ///
    /// Two close-path questions share this lookup: whether the supervisor's
    /// verdict already satisfies the review queue (so the worker's re-close
    /// completes instead of re-queuing), and whether a "verification skipped"
    /// message would be lying about a verdict that exists. Store failures are
    /// treated as "no verdict" — this only ever *grants* an exit, so an
    /// unreadable store must never manufacture one.
    pub(crate) fn current_cycle_approved_verification(
        &self,
        task_id: &str,
        required_type: VerificationType,
        window: Option<&TaskCommitReceiptWindow>,
    ) -> Option<Verification> {
        let store = self.open_verification_store().ok()?;
        let latest = store.get_latest_for_task(task_id).ok()??;
        approved_verification_satisfies_review_queue(&latest, window, required_type)
            .then_some(latest)
    }

    /// Compute why (if at all) the task-verifier step should be skipped
    /// for this close attempt.
    ///
    /// Only invoked after the caller has been identified as a supervisor
    /// and `verification_enabled` is true — the `VerificationSkipReason::None`
    /// cases here represent "supervisor is closing, but the assignee is
    /// still alive and no bypass flag was set, so run the verifier".
    ///
    /// Resolution order:
    ///
    /// 1. No assignee at all → `NoAssignee`.
    /// 2. Consult the task's active lease via `agent_store.get_lease`.
    ///    `TaskLease.agent_id` is the real session-id even when
    ///    `task.assignee` stores a display name, so this is the most
    ///    reliable liveness source. If the lease is valid and the
    ///    referenced agent is alive+fresh → not a skip (unless the
    ///    supervisor passed `bypass_code_review=true`, in which case
    ///    we honor it as `SupervisorBypass`). If the lease is stale or
    ///    the referenced agent is dead → `AssigneeInactive`.
    /// 3. No lease — try a direct `agent_store.get(task.assignee)` for
    ///    legacy tasks whose assignee field may hold an agent_id. Same
    ///    liveness logic as above.
    /// 4. Everything failed → `AssigneeUnknown` (never falsely reported
    ///    as "assignee inactive" — the agent row is simply missing).
    pub(crate) fn compute_verification_skip_reason(
        &self,
        task: &cas_types::Task,
        req: &TaskCloseRequest,
    ) -> VerificationSkipReason {
        let Some(assignee) = task.assignee.as_deref() else {
            return VerificationSkipReason::NoAssignee;
        };

        let Ok(agent_store) = self.open_agent_store() else {
            // Can't reach the agent store at all — be conservative and
            // let verification run (None is the safe default).
            return VerificationSkipReason::None;
        };

        let bypass_requested = req.bypass_code_review.unwrap_or(false);
        let alive_result = |agent: &cas_types::Agent| {
            agent.is_alive() && !agent.is_heartbeat_expired(ASSIGNEE_STALE_SECS)
        };
        let stale_minutes = |agent: &cas_types::Agent| {
            chrono::Utc::now()
                .signed_duration_since(agent.last_heartbeat)
                .num_minutes()
        };

        // 1) Lease-based path. TaskLease.agent_id always holds the real
        //    session id, so this survives the name-vs-id mismatch that
        //    broke the pre-cas-3bd4 path.
        if let Ok(Some(lease)) = agent_store.get_lease(&task.id) {
            if lease.is_valid() {
                if let Ok(agent) = agent_store.get(&lease.agent_id) {
                    return if alive_result(&agent) {
                        if bypass_requested {
                            VerificationSkipReason::SupervisorBypass
                        } else {
                            VerificationSkipReason::None
                        }
                    } else {
                        VerificationSkipReason::AssigneeInactive {
                            minutes_stale: Some(stale_minutes(&agent)),
                        }
                    };
                }
                // Lease is valid but the referenced agent row is gone —
                // agent was unregistered but the lease wasn't cleaned up.
                return VerificationSkipReason::AssigneeUnknown;
            }
            // Lease exists but expired.
            return VerificationSkipReason::AssigneeInactive {
                minutes_stale: None,
            };
        }

        // 2) No lease — try the legacy direct-id lookup. Works only when
        //    task.assignee holds an agent_id, not a display name.
        if let Ok(agent) = agent_store.get(assignee) {
            return if alive_result(&agent) {
                if bypass_requested {
                    VerificationSkipReason::SupervisorBypass
                } else {
                    VerificationSkipReason::None
                }
            } else {
                VerificationSkipReason::AssigneeInactive {
                    minutes_stale: Some(stale_minutes(&agent)),
                }
            };
        }

        // 3) No lease, no matching agent row. The assignee is unknown
        //    to the store — do not falsely report "inactive".
        VerificationSkipReason::AssigneeUnknown
    }

    /// Reopen a closed/blocked task, or reset one exact approved task-only scope.
    ///
    /// cas-3c23: reopening a Closed/merged task is a supervisor-only action.
    /// A factory worker told (by a stale director re-dispatch or coordination
    /// message) to work an already-Closed ticket must NOT be able to reopen
    /// it unilaterally — that's exactly the thrash loop cas-a7c8 diagnosed
    /// (reopen → re-verify already-shipped code → re-close, stomping main).
    /// The same supervisor-only gate applies to unblocking (cas-cd24): a
    /// blocked task is typically waiting on a supervisor decision (e.g. an
    /// acceptance criterion the worker correctly flagged as wrong), so
    /// lifting the block is kept a supervisor action too, for the same
    /// "don't let a stale signal cause unilateral state thrash" reason.
    ///
    /// cas-cd24: `reopen` previously only accepted `Closed` tasks —
    /// `Blocked` tasks had no documented path back to `Open` via this verb,
    /// forcing `update status=open` as an undiscoverable workaround that
    /// also silently dropped the `reason` (see
    /// `BUG-blocked-tasks-cannot-be-reopened.md`). Closed→Open behavior
    /// (status flip, `closed_at`/`factory_branch_anchor` reset) is
    /// unchanged; Blocked→Open is new and does not touch those
    /// closed-specific fields.
    ///
    /// cas-e1b5: a nonterminal task with the exact latest nonlegacy
    /// Approved/Skipped Resolved task-only dispatch may also be reopened to
    /// start a fresh review scope. The named dispatch is invalidated before
    /// moving the task to Open. Delivery-bound, rejected/error, superseded,
    /// and ordinary task states retain the existing rejection behavior.
    pub async fn cas_task_reopen(
        &self,
        Parameters(req): Parameters<TaskReopenRequest>,
    ) -> Result<CallToolResult, McpError> {
        if !is_supervisor_from_env() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "task reopen rejected: only supervisors may reopen a closed or \
                     blocked task (CAS_AGENT_ROLE=supervisor). Task {} stays as-is. \
                     Message your supervisor if you believe this task needs rework \
                     or its blocker should be lifted — do not reopen it yourself.",
                    req.id
                ),
            ));
        }

        let task_store = self.open_task_store()?;

        let mut task = task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;
        let original_updated_at = task.updated_at;

        let fresh_scope_dispatch =
            super::proof_scope::close_authoritative_task_proof_dispatch(&self.cas_root, &task.id)
                .map_err(|reason| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "task fresh-scope recovery rejected: exact proof state for {} is unreadable ({reason})",
                            task.id
                        ),
                    )
                })?;

        if task.status != TaskStatus::Closed
            && task.status != TaskStatus::Blocked
            && fresh_scope_dispatch.is_none()
        {
            let already_reopened = task.status == TaskStatus::Open
                && cas_store::get_latest_verification_dispatch(&self.cas_root, &task.id)
                    .ok()
                    .flatten()
                    .is_some_and(|dispatch| {
                        dispatch.state == cas_types::VerificationDispatchState::Invalidated
                    });
            if already_reopened {
                return Ok(Self::success(format!(
                    "Reopened task idempotently: {} - {}",
                    req.id, task.title
                )));
            }
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "Task is already {} (only closed or blocked tasks can be \
                     reopened, unless an exact Resolved task-only proof must be \
                     invalidated before fresh review scope). To change status \
                     directly, use: `task action=update id={} status=open`.",
                    task.status, req.id
                ),
            ));
        }

        let old_status = task.status;
        task.status = TaskStatus::Open;
        if old_status == TaskStatus::Closed {
            // cas-cd24: closed-specific resets stay gated to the closed
            // path so Closed→Open behavior is byte-for-byte unchanged
            // (AC2) — a Blocked task was never closed, so `closed_at` is
            // already `None` and clearing `factory_branch_anchor` here
            // would be a no-op at best, dead code at worst.
            task.closed_at = None;
            // cas-cf64 (P2, anchor freshness — Scenario B): a stale
            // `factory_branch_anchor` from a PRIOR close/park cycle must not
            // survive a reopen. Without this, `run_factory_branch_merge_gate`
            // would keep trusting the OLD anchor sha (already merged, from
            // before the reopen) forever — `park_task_awaiting_merge`'s
            // `is_none()` guard never overwrites an existing anchor, so any
            // NEW commits made after rework would be invisible to the gate and
            // the task would false-Proceed on reworked-but-unmerged code.
            task.deliverables.factory_branch_anchor = None;
        }
        task.updated_at = chrono::Utc::now();

        // cas-cd24: capture the reopen/unblock reason on the audit trail —
        // previously silently dropped (the dispatcher discarded `reason`
        // for this action entirely; see `TaskRequest` -> `IdRequest` in
        // `service/core.rs` before this fix). Mirrors the close-path
        // `close_reason`/note pattern above in `cas_task_close`.
        if let Some(reason) = &req.reason {
            let timestamp = task.updated_at.format("%Y-%m-%d %H:%M");
            let verb = if fresh_scope_dispatch.is_some() && old_status != TaskStatus::Closed {
                "Review scope reset"
            } else if old_status == TaskStatus::Blocked {
                "Unblocked"
            } else {
                "Reopened"
            };
            let reopen_note = format!("[{timestamp}] {verb}: {reason}");
            if task.notes.is_empty() {
                task.notes = reopen_note;
            } else {
                task.notes = format!("{}\n\n{}", task.notes, reopen_note);
            }
        }

        let actor = self
            .get_agent_id()
            .ok()
            .and_then(|id| {
                self.open_agent_store()
                    .ok()
                    .and_then(|s| s.get(&id).ok())
                    .map(|a| a.name)
            })
            .unwrap_or_else(|| "unknown".into());
        let occurrence = super::supervisor_push::occurrence_from_updated_at(task.updated_at);
        let lifecycle_outbox = if old_status == TaskStatus::Closed {
            let agent_store = self.open_agent_store().map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to prepare reopen lifecycle: {error}")),
                data: None,
            })?;
            let outbox = super::supervisor_push::prepare_task_lifecycle_outbox(
                agent_store.as_ref(),
                &task.id,
                &task.title,
                old_status,
                TaskStatus::Open,
                &actor,
                req.reason.as_deref(),
                super::supervisor_push::LifecycleTransition::ReadyReopened,
                &occurrence,
            );
            if outbox.is_some() {
                crate::store::open_supervisor_queue_store(&self.cas_root).map_err(|error| {
                    McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Failed to initialize reopen lifecycle outbox: {error}"
                        )),
                        data: None,
                    }
                })?;
            }
            outbox
        } else {
            None
        };

        if let Some(dispatch) = fresh_scope_dispatch
            .as_ref()
            .filter(|_| old_status != TaskStatus::Closed)
        {
            cas_store::invalidate_verification_dispatch_and_reopen_task_exact(
                &self.cas_root,
                &dispatch.id,
                &task,
                old_status,
            )
            .map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Failed to atomically invalidate exact reviewed scope {} and reopen task {}: {error}",
                    dispatch.id, task.id
                )),
                data: None,
            })?;
        } else if old_status == TaskStatus::Closed {
            cas_store::reopen_closed_task_atomic(
                &self.cas_root,
                &task,
                original_updated_at,
                cas_store::ParentDependencyUpdate::Unchanged,
                lifecycle_outbox.as_ref(),
            )
            .map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Failed to atomically invalidate proof cycle and reopen task {}: {error}",
                    task.id
                )),
                data: None,
            })?;
        } else {
            task_store.update(&task).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to update: {e}")),
                data: None,
            })?;
        }

        // cas-062d / cas-17e4: ready/reopened outbox after successful reopen.
        if let Err(e) = self.push_task_lifecycle(
            &req.id,
            &task.title,
            old_status,
            TaskStatus::Open,
            &actor,
            Some("reopen"),
            super::supervisor_push::LifecycleTransition::ReadyReopened,
            &occurrence,
        ) {
            let key = super::supervisor_push::transition_key(
                &req.id,
                old_status,
                TaskStatus::Open,
                std::env::var("CAS_FACTORY_SESSION").ok().as_deref(),
                super::supervisor_push::LifecycleTransition::ReadyReopened,
                &occurrence,
            );
            return Err(Self::error(
                ErrorCode::INTERNAL_ERROR,
                super::supervisor_push::lifecycle_push_failure_message(
                    &req.id,
                    TaskStatus::Open,
                    super::supervisor_push::LifecycleTransition::ReadyReopened,
                    &key,
                    &e,
                ),
            ));
        }

        let suffix = if fresh_scope_dispatch.is_some() && old_status != TaskStatus::Closed {
            " (invalidated the exact approved proof and opened a fresh verification scope)"
        } else {
            ""
        };
        Ok(Self::success(format!(
            "Reopened task: {} - {}{}",
            req.id, task.title, suffix
        )))
    }

    /// Record a supervisor's negative review of a parked AwaitingMerge task.
    ///
    /// This is the one sanctioned exception to the delivery-proof scope lock:
    /// a negative verdict invalidates the proof it rejects, reopens the task
    /// without changing its assignee, and clears the parked anchor. It applies
    /// to every parked shape — declined-before-merge, amendment-after-merge
    /// (GH #55), and deliveries whose proof boundary is unbound or absent
    /// (GH #82) — so a failed review always has an exit. It stays distinct
    /// from `reopen`, which handles closed/blocked work, and from `reset`,
    /// which remains orphan recovery and clears ownership.
    pub async fn cas_task_request_changes(
        &self,
        Parameters(req): Parameters<TaskRequestChangesRequest>,
    ) -> Result<CallToolResult, McpError> {
        request_changes_role_gate(is_supervisor_from_env(), &req.id)
            .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))?;
        if req.reason.trim().is_empty() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "task request_changes rejected: reason must explain what must change and whether prior commits should stand or be reverted",
            ));
        }

        let task_store = self.open_task_store()?;
        let task = task_store.get(&req.id).map_err(|error| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("Task not found: {error}"),
            )
        })?;
        if task.status != TaskStatus::AwaitingMerge {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "task request_changes rejected: {} is {} rather than awaiting_merge. This action declines a parked delivery; a task that is not parked is already actionable.",
                    task.id, task.status
                ),
            ));
        }
        let supervisor_id = self.get_agent_id().map_err(|error| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("task request_changes requires registered supervisor identity: {error}"),
            )
        })?;

        // Deliberately no delivery/dispatch precondition: GH #55 and #82 both
        // deadlocked because the recovery action refused parked tasks whose
        // proof boundary was merged, unbound, or legacy — exactly the states a
        // failed review produces. Boundary shape is reported, never gating.
        let outcome = cas_store::request_changes_for_parked_delivery(
            &self.cas_root,
            &task.id,
            &supervisor_id,
            &req.reason,
        )
        .map_err(|error| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("task request_changes rejected: {error}"),
            )
        })?;

        Ok(Self::success(format!(
            "Changes requested for task: {} - {}\n\nThe task is Open with assignee {} preserved. Branch handling: {} The declined delivery anchor and proof were invalidated, so re-close requires a fresh cycle.\n\nDecision: {}",
            task.id,
            task.title,
            task.assignee.as_deref().unwrap_or("unassigned"),
            outcome.branch_handling,
            req.reason.trim(),
        )))
    }

    /// Emit a `DaemonEvent::WorkerActivity { event_type = "audit_trail_gap" }` to
    /// the supervisor TUI, fire-and-forget.
    ///
    /// Uses `get_agent_id()` to identify the sender; falls back to the sentinel
    /// `"unknown-session"` when the agent ID is unavailable (e.g., no SessionStart
    /// hook ran, `CAS_SESSION_ID` unset in CI). This ensures the gap event always
    /// reaches the TUI even when agent metadata is missing — losing fidelity on the
    /// sender is better than silently dropping the event.
    ///
    /// cas-eeab (Item 4+5): extracted from the two duplicated inline blocks in the
    /// `match verification_store.generate_id()` arm.
    fn emit_audit_gap_event(&self, task_id: &str, description: String) {
        let session_id = self
            .get_agent_id()
            .unwrap_or_else(|_| "unknown-session".to_string());
        let gap_event = crate::mcp::socket::DaemonEvent::WorkerActivity {
            session_id,
            event_type: "audit_trail_gap".to_string(),
            description,
            entity_id: Some(task_id.to_string()),
        };
        if let Err(e) = crate::mcp::socket::send_event(&self.cas_root, &gap_event) {
            // Fire-and-forget, but log the failure so that a double-silent failure
            // (store write fails AND socket send fails) leaves at least a trace.
            // cas-eeab: safe_auto autofix for silently discarded send_event result.
            tracing::warn!(
                task_id = %task_id,
                error = %e,
                "failed to emit audit_trail_gap event to daemon socket"
            );
        }
    }
}

/// cas-6538: audit text recorded on a `depth=light` task at close time,
/// stating exactly which rigor gates were skipped and why. Kept as a
/// self-contained `pub(crate)` fn (not an inline literal) so the wording
/// is unit-testable and stays in one place. The caller prepends a
/// `[timestamp]` and appends it to the task notes timeline.
pub(crate) fn light_skip_decision_note() -> String {
    "decision: depth=light close (EPIC cas-1255 speed mode) — skipped the \
     verification jail (no task-verifier dispatch; pending_verification left \
     false) and the P0 code-review gate (treated as satisfied; supervisor \
     review queue hop bypassed). Reason: task depth=light. Data-state guards \
     (merge-state, uncommitted-work, additive-only, commit-claim) still ran."
        .to_string()
}

/// cas-7998: sanitize a free-text close reason for safe embedding inside a
/// double-quoted `message="..."` argument of a suggested coordination command.
///
/// The verification-jail guidance prints a ready-to-run coordination call such
/// as `mcp__cas__coordination action=message ... message="Task X is ready to
/// close. Close reason: <reason>. ..."`. A raw reason containing a double quote
/// would prematurely terminate that quoted argument, and an embedded newline
/// would split the single-line command — in both cases the worker copies a
/// broken instruction. Collapse every run of whitespace (spaces, tabs,
/// newlines, CRs) into a single space and backslash-escape `\` then `"` so the
/// resulting text always stays inside one balanced double-quoted argument.
pub(crate) fn escape_close_reason_for_quoted_command(reason: &str) -> String {
    let collapsed = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    // Escape backslashes first so a pre-existing `\` before a quote can't
    // combine with the quote-escape we add and break the escaping.
    collapsed.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A single additive-only violation: a file whose git status indicates it
/// was modified, deleted, or renamed relative to HEAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdditiveOnlyViolation {
    pub status: String,
    pub path: String,
}

/// cas-9596 (GH #82): evidence that a commit belongs to one task, regardless
/// of which assignee produced it.
///
/// A task can span several workers and work cycles — worker 1 pushes WIP and
/// dies, the supervisor preserves it, worker 2 finishes. Commits from those
/// earlier cycles are still *this task's* work, so gates that ask "was this
/// pre-existing / foreign?" must be able to recognize them.
///
/// Empty (`default()`) means "attribution unavailable", which reproduces the
/// pre-cas-9596 behavior: nothing is recognized as the task's own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskCommitIdentity {
    /// The task id, matched as a whole token against commit messages.
    pub task_id: Option<String>,
    /// Commit ids CAS durably recorded for this task (parked factory anchor,
    /// worker delivery receipts). Exact evidence that needs no convention.
    pub known_commits: Vec<String>,
}

impl TaskCommitIdentity {
    fn is_empty(&self) -> bool {
        self.task_id.is_none() && self.known_commits.is_empty()
    }

    /// True when `commit` is one of the durably recorded task commits.
    ///
    /// Both sides are git object ids, so a prefix match in either direction is
    /// the same commit (CAS records full ids; git may hand back an
    /// abbreviation).
    fn matches_known_commit(&self, commit: &str) -> bool {
        let commit = commit.trim().to_ascii_lowercase();
        if commit.is_empty() {
            return false;
        }
        self.known_commits.iter().any(|known| {
            let known = known.trim().to_ascii_lowercase();
            !known.is_empty() && (known.starts_with(&commit) || commit.starts_with(&known))
        })
    }
}

/// Collect every durable commit id CAS recorded for this task, plus the task
/// id itself, into one attribution record.
///
/// `latest_delivery_commit` is the commit sha from the task's most recent
/// worker delivery receipt (the caller reads it from the delivery store).
pub(crate) fn task_commit_identity(
    task: &Task,
    latest_delivery_commit: Option<String>,
) -> TaskCommitIdentity {
    let mut known_commits: Vec<String> = task
        .deliverables
        .factory_branch_anchor
        .iter()
        .cloned()
        .chain(latest_delivery_commit)
        .map(|commit| commit.trim().to_string())
        .filter(|commit| !commit.is_empty())
        .collect();
    known_commits.dedup();
    TaskCommitIdentity {
        task_id: Some(task.id.clone()),
        known_commits,
    }
}

/// True when `message` references `task_id` as a whole token.
///
/// Guards against prefix collisions: `cas-f1b12` must not be read as a
/// reference to `cas-f1b1`.
pub(crate) fn message_references_task(message: &str, task_id: &str) -> bool {
    if task_id.is_empty() {
        return false;
    }
    let haystack = message.to_ascii_lowercase();
    let needle = task_id.to_ascii_lowercase();
    let boundary = |character: Option<char>| {
        character.is_none_or(|character| !character.is_ascii_alphanumeric() && character != '-')
    };
    let mut offset = 0;
    while let Some(found) = haystack[offset..].find(&needle) {
        let start = offset + found;
        let end = start + needle.len();
        let before = haystack[..start].chars().next_back();
        let after = haystack[end..].chars().next();
        if boundary(before) && boundary(after) {
            return true;
        }
        offset = start + 1;
    }
    false
}

/// True when `commit` is attributable to the task described by `identity`.
///
/// Two independent signals, either sufficient: a durably recorded task commit
/// id, or a commit message that names the task. Returns false when attribution
/// evidence is unavailable — callers use this to *relax* a gate, so an unknown
/// commit must stay unattributed.
pub(crate) fn commit_is_task_attributable(
    repo_path: &std::path::Path,
    commit: &str,
    identity: &TaskCommitIdentity,
) -> bool {
    use std::process::Command;

    if commit.is_empty() || commit.starts_with('-') || identity.is_empty() {
        return false;
    }
    if identity.matches_known_commit(commit) {
        return true;
    }
    let Some(task_id) = identity.task_id.as_deref() else {
        return false;
    };
    let message = Command::new("git")
        .args(["log", "-1", "--format=%B", commit, "--"])
        .current_dir(repo_path)
        .output();
    match message {
        Ok(output) if output.status.success() => {
            message_references_task(&String::from_utf8_lossy(&output.stdout), task_id)
        }
        _ => false,
    }
}

/// True when every commit that ever touched `path` up to `base_rev` belongs to
/// this task — i.e. the "pre-existing" version the diff compares against is the
/// task's own earlier work (GH #82 step 3), not foreign code.
///
/// Fails closed: unknowable git state, no history for the path, or a single
/// unattributable commit all mean "genuinely pre-existing".
fn pre_image_is_task_owned(
    repo_path: &std::path::Path,
    base_rev: &str,
    path: &str,
    identity: &TaskCommitIdentity,
) -> bool {
    use std::process::Command;

    /// A path touched by more commits than this was not produced by one task's
    /// WIP; scanning further would only burn git calls. Fail closed instead.
    const MAX_PRE_IMAGE_COMMITS: usize = 500;

    if identity.is_empty() || base_rev.is_empty() || base_rev.starts_with('-') {
        return false;
    }
    // One git call for the whole path history: `%H<US>%B<RS>` records, so
    // attribution never costs a subprocess per commit.
    let history = Command::new("git")
        .args(["log", "--format=%H%x1f%B%x1e", base_rev, "--", path])
        .current_dir(repo_path)
        .output();
    let Ok(history) = history else {
        return false;
    };
    if !history.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&history.stdout);
    let records: Vec<&str> = stdout
        .split('\u{1e}')
        .map(str::trim)
        .filter(|record| !record.is_empty())
        .collect();
    if records.is_empty() || records.len() > MAX_PRE_IMAGE_COMMITS {
        return false;
    }
    records.iter().all(|record| {
        let (commit, message) = record.split_once('\u{1f}').unwrap_or((record, ""));
        let commit = commit.trim();
        identity.matches_known_commit(commit)
            || identity
                .task_id
                .as_deref()
                .is_some_and(|task_id| message_references_task(message, task_id))
    })
}

/// Drop violations whose pre-existing version was produced by this same task.
fn retain_foreign_violations(
    repo_path: &std::path::Path,
    base_rev: &str,
    identity: &TaskCommitIdentity,
    violations: Vec<AdditiveOnlyViolation>,
) -> Vec<AdditiveOnlyViolation> {
    if identity.is_empty() || violations.is_empty() {
        return violations;
    }
    violations
        .into_iter()
        .filter(|violation| {
            !pre_image_is_task_owned(repo_path, base_rev, &violation.path, identity)
        })
        .collect()
}

/// A single uncommitted-work entry: a tracked file that `git status` reports
/// as modified, deleted, added-but-not-committed, renamed, or copied.
///
/// `status` is the raw two-char porcelain field (e.g. ` M`, `M `, `A `,
/// `D `, `R `) and `path` is the workspace-relative path git reported.
/// Untracked (`??`) entries are excluded by [`check_uncommitted_work`];
/// they never belonged to the task in the first place so they cannot
/// represent lost work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UncommittedEntry {
    pub status: String,
    pub path: String,
}

/// Return tracked files that are modified, staged, or otherwise in a
/// non-committed state relative to HEAD in the git repo at
/// `project_root`. Returns an empty vec for non-git directories or if
/// the `git` subprocess fails — the gate is advisory and must not
/// block closes it cannot reason about.
///
/// The check is deliberately scoped to **tracked** files. Untracked
/// files (`??`) are allowed through because:
///   * They're safe to delete if the task is disposable.
///   * They're often scratch output (`*.log`, `target/`) that the
///     worker had no intention of committing.
///   * If the worker *did* intend to commit them, they would have run
///     `git add` first, which promotes them to the `A ` status and the
///     gate catches them.
pub(crate) fn check_uncommitted_work(project_root: &std::path::Path) -> Vec<UncommittedEntry> {
    use std::process::Command;

    let Ok(output) = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(project_root)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    let mut entries = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // Porcelain format: "XY path" where XY is a 2-char status.
        // Short lines, empty lines → skip.
        if line.len() < 4 {
            continue;
        }
        let (status, rest) = line.split_at(2);
        // Skip untracked entries (`??`). They're additive by nature
        // and never represent a lost commit.
        if status == "??" {
            continue;
        }
        // Rename format: "R  old -> new". Record the new path.
        let path = if let Some((_, new)) = rest.trim_start().split_once(" -> ") {
            new.to_string()
        } else {
            rest.trim_start().to_string()
        };
        entries.push(UncommittedEntry {
            status: status.to_string(),
            path,
        });
    }
    entries
}

/// cas-bc1b: check additive-only violations by comparing the worker
/// branch's committed history against its parent branch. This is the
/// path used for factory worker tasks — it inspects only what the
/// worker committed on their isolated branch, immune to the
/// main-worktree dirty-state confusion that tripped cas-4333.
///
/// Before the factory branch is merged, runs
/// `git diff --name-status <merge-base>..HEAD`. After MERGE REQUIRED parked a
/// `factory_branch_anchor` and that anchor is merged into the parent, scopes
/// the diff to the supervisor merge's first-parent delta instead. This avoids
/// attributing the epic's pre-existing baseline to the additive-only task.
/// Untracked files don't exist in committed history, so `??` handling isn't
/// needed here.
///
/// Graceful degradation: if the worktree isn't a git repo, git can't
/// find `parent_branch`, or the merge-base computation fails, returns
/// an empty vec. The gate is advisory when git state is unknowable.
/// cas-9596 (GH #82): whichever window is used, a file whose pre-existing
/// version came from this same task's earlier commits (a dead worker's WIP
/// that the supervisor preserved) is not "pre-existing code" — `identity`
/// filters those out. Pass [`TaskCommitIdentity::default`] when attribution is
/// unavailable; the gate then behaves exactly as it did before.
pub(crate) fn check_additive_only_branch_violations(
    worker_worktree_path: &std::path::Path,
    parent_branch: &str,
    factory_branch_anchor: Option<&str>,
    identity: &TaskCommitIdentity,
) -> Vec<AdditiveOnlyViolation> {
    use std::process::Command;

    // cas-3f7f: once the parked worker tip is integrated, HEAD and the
    // parent branch may both point at (or beyond) an epic merge. Re-scanning
    // merge-base..HEAD then includes unrelated epic history. Find the
    // first-parent merge whose non-first parent is the parked worker tip and
    // inspect only that merge delta. This preserves every commit introduced
    // by the worker side, including real M/D/R changes.
    if let Some(anchor) = factory_branch_anchor {
        if commit_is_merged_into_parent(worker_worktree_path, anchor, parent_branch) {
            let parent_ref = if git_ref_exists(worker_worktree_path, parent_branch) {
                parent_branch.to_string()
            } else {
                format!("origin/{parent_branch}")
            };
            let history = Command::new("git")
                .args(["rev-list", "--first-parent", "--parents", &parent_ref])
                .current_dir(worker_worktree_path)
                .output();
            if let Ok(history) = history {
                if history.status.success() {
                    for line in String::from_utf8_lossy(&history.stdout).lines() {
                        let commits = line.split_whitespace().collect::<Vec<_>>();
                        if commits.len() >= 3 && commits[2..].contains(&anchor) {
                            let diff_out = Command::new("git")
                                .args(["diff", "--name-status", commits[1], commits[0]])
                                .current_dir(worker_worktree_path)
                                .output();
                            return match diff_out {
                                Ok(o) if o.status.success() => retain_foreign_violations(
                                    worker_worktree_path,
                                    commits[1],
                                    identity,
                                    parse_name_status(&String::from_utf8_lossy(&o.stdout)),
                                ),
                                _ => Vec::new(),
                            };
                        }
                    }
                }
            }

            // Fast-forward or squash-less integration without a merge commit:
            // the parked tip itself is still the narrowest reliable task
            // window available (and is the documented fallback for cas-3f7f).
            let anchor_parent = format!("{anchor}^");
            let diff_out = Command::new("git")
                .args(["diff", "--name-status", &anchor_parent, anchor])
                .current_dir(worker_worktree_path)
                .output();
            return match diff_out {
                Ok(o) if o.status.success() => retain_foreign_violations(
                    worker_worktree_path,
                    &anchor_parent,
                    identity,
                    parse_name_status(&String::from_utf8_lossy(&o.stdout)),
                ),
                _ => Vec::new(),
            };
        }
    }

    // Resolve the merge base first. Using `git merge-base` explicitly
    // (rather than the `a..b` revspec shorthand) means we get a clear
    // failure signal if the parent branch ref can't be resolved — we
    // don't silently compare against the wrong thing.
    let merge_base_out = Command::new("git")
        .args(["merge-base", "HEAD", parent_branch])
        .current_dir(worker_worktree_path)
        .output();
    let merge_base = match merge_base_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return Vec::new(),
    };
    if merge_base.is_empty() {
        return Vec::new();
    }

    let diff_out = Command::new("git")
        .args(["diff", "--name-status", &format!("{merge_base}..HEAD")])
        .current_dir(worker_worktree_path)
        .output();
    match diff_out {
        Ok(o) if o.status.success() => retain_foreign_violations(
            worker_worktree_path,
            &merge_base,
            identity,
            parse_name_status(&String::from_utf8_lossy(&o.stdout)),
        ),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// cas-95ce: per-task close-time merge-state guard
// ---------------------------------------------------------------------------

/// Outcome of the cas-95ce factory-branch merge-state close gate.
///
/// Mirrors [`CodeReviewGateOutcome`] in shape so the call site is a
/// uniform pattern-match. The gate exposes only `Proceed` / `Reject`
/// because, unlike the cas-code-review gate, this one has no
/// supervisor override path — bypass cannot skip a data-state guard.
#[derive(Debug)]
pub(crate) enum MergeStateGateOutcome {
    /// Close may proceed — factory branch is merged into the parent
    /// epic branch (or the guard is structurally skipped: epic-type
    /// task, no assignee, branch missing locally, or git history
    /// unknowable).
    Proceed,
    /// cas-e74c: close may proceed because the *delivery* is proven
    /// integrated (validated `commit_receipt`) or because no commit on
    /// the lane is attributable to this task's work cycle — while the
    /// lane itself still carries unmerged residue belonging to other
    /// tasks. Carries the audit note the caller records on the task, so
    /// the residue is logged rather than fatal.
    ProceedWithNote(String),
    /// Close must be rejected with this user-facing error message.
    Reject(String),
}

/// cas-e74c: evidence that scopes the merge-state guard to the closing
/// task's own delivery instead of the whole registered lane branch.
///
/// - `receipt`: the worker-supplied `commit_receipt` (if any). When it
///   validates against the parent branch it IS the delivery evidence.
/// - `window`: the task's current work-cycle lower bound (latest lease
///   claim/transfer, falling back to task creation). Commits on the lane
///   that predate it belong to other tasks and are not this task's to
///   merge.
///
/// Both `None` reproduces the pre-cas-e74c whole-branch behavior, which
/// is what the non-close callers (and the 4-arg
/// [`run_factory_branch_merge_gate`] shim) use.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct TaskCommitAttribution<'a> {
    pub receipt: Option<&'a str>,
    pub window: Option<&'a TaskCommitReceiptWindow>,
}

/// Per-task close-time guard: reject `task.close` when the worker's
/// `factory/<assignee>` branch carries commits not present on the
/// parent epic branch.
///
/// Runs BEFORE [`run_code_review_gate`]; consequently
/// `bypass_code_review=true` cannot skip it. This is a data-state
/// guard, not a review gate — a reviewed-but-unmerged branch is
/// still stranded work, and the entire point of the cas-95ce/cas-754b
/// scoping decision is that there is no escape hatch (an escape
/// hatch would let the same workaround pattern persist that motivated
/// the EPIC).
///
/// Skipped (Proceed) when:
/// - `task.task_type == Epic` — epic close is already covered by
///   [`check_unmerged_epic_branches`] at the epic-id branch namespace.
/// - `task.assignee.is_none()` — orphaned task; nothing to check.
/// - `factory/<assignee>` does not exist locally and merge-base
///   computation fails — graceful pass. We do not false-reject when
///   the worktree predates the convention or the branch was already
///   pruned post-merge.
///
/// Rejects (Reject) when the factory branch has > 0 commits not on
/// `parent_branch`. The error message includes the stranded count,
/// the factory branch name, the parent branch name, and explicit
/// remediation steps.
///
/// `_req` is intentionally unused — the bypass flag does not affect
/// this guard. It is carried through so the call signature mirrors
/// [`run_code_review_gate`] and the structural placement (this gate
/// sits upstream of any bypass evaluation) is self-documenting.
///
/// ## cas-4b3f: anchored to the task's own commits, not branch HEAD
///
/// `count_unmerged_factory_commits` is always evaluated against a
/// *commit-ish*, not necessarily the branch name. When
/// `task.deliverables.factory_branch_anchor` is set (recorded by the
/// caller the FIRST time this gate rejected this specific task — see
/// `park_task_awaiting_merge`) and that sha still resolves in this
/// repo, it is used instead of the live `factory/<assignee>` HEAD.
///
/// Without this, a worker who starts a second task on the same
/// `factory/<assignee>` branch before the first task's commits are
/// merged re-strands the first task on every retry: the second task's
/// unmerged commits ride along on branch HEAD and the gate can't tell
/// them apart from the first task's own (already-merged) work. See
/// BUG-close-guard-branch-head-not-task-commits.md.
///
/// ## cas-cf64: the anchor is trusted ONLY while `status == AwaitingMerge`
///
/// An anchor is written exactly once, by `park_task_awaiting_merge`, which
/// sets `status = AwaitingMerge` in the SAME update — so in the intended
/// lifecycle an anchor is never present without that status. This is a
/// defense-in-depth guard against a data anomaly (for example, a legacy
/// stale anchor that predates the task-store invariant): if `status` isn't
/// `AwaitingMerge`, an existing anchor value is ignored and the gate falls
/// back to the live branch name, matching first-attempt behavior.
///
/// ## cas-2938: squash-equivalent convergence (content + live-ref)
///
/// The parked anchor is a *historical* commit-ish. GitHub squash-merge
/// rewrites the factory tip A into a new integration tip B that does not
/// have A as an ancestor, so raw ancestry against A never clears.
///
/// When the gate is evaluating a trusted AwaitingMerge anchor and that
/// anchor still looks stranded by rev-list ancestry, two secondary
/// signals may clear the gate without deleting the historical anchor:
///
/// 1. **Content / tip-tree reachability** — the anchor tip's tree object
///    is reachable from the parent (or `origin/<parent>`). Clean squash
///    of one or many commits produces a tip tree identical to the
///    pre-squash factory tip; this also preserves cas-4b3f serial-task
///    protection after squash (task A can close while task B's later
///    unmerged commits ride on the live factory HEAD).
/// 2. **Live-ref convergence** — the live `factory/<assignee>` tip is
///    **known** to have zero commits not on the parent, via
///    [`known_unmerged_factory_commits`] → [`KnownUnmergedCount::KnownZero`]
///    (never the fail-open `count_unmerged_factory_commits == 0`). Covers
///    conflict-resolved squashes whose tip tree differs from A after the
///    worker force-aligns the factory ref to the integration tip. Missing
///    refs / merge-base / rev-list failure are `Unknown` and do not clear
///    the gate. Cannot mask later unmerged serial work (KnownPositive).
///
/// ## cas-5485: stale pre-rebase factory SHA
///
/// A normal `git rebase` rewrites parked tip A → A' (new SHA). The first
/// park still stores A. After A' is integrated, ancestry against A never
/// clears. Secondary acceptance (fail-closed):
///
/// 1. **Tip-tree reachability** of A on parent (identical tip tree).
/// 2. **Live tip KnownZero** — factory HEAD fully integrated (no later
///    unmerged work). Does not help when serial task B advanced HEAD.
/// 3. **Cherry-equivalent patches of A on parent** — `git cherry` reports
///    every commit unique to A as equivalent (`-`) on parent/origin. This
///    is the task-specific proof for the rebased form A': it does **not**
///    require live HEAD to be zero-ahead, so serial B cannot mask or
///    satisfy task A. Unknown/failed cherry → not integrated (fail closed).
/// cas-a844: does the worker's factory branch have a genuine git merge
/// conflict against `parent_branch` — as opposed to simply carrying commits
/// not yet merged? Delegates to `GitOperations::preflight_merge_conflicts`
/// (bright-gopher-20's cas-e18f helper), which runs `git merge-tree
/// --write-tree` — it computes the merge purely in-memory and never touches
/// the working tree or index, so it's safe to call from this read-only
/// close-time gate.
///
/// Returns the conflicting file paths when the check succeeds (empty =
/// cleanly mergeable). Evaluation failures remain errors so the caller can
/// distinguish "clean" from "unknown" and keep the task's rework exit open.
/// Named paths beat a bare boolean: they're exactly what a
/// worker needs to go resolve the conflict, and what a supervisor needs to
/// judge severity, without either party re-running the check by hand.
pub(crate) fn factory_branch_merge_conflict_paths(
    repo_path: &std::path::Path,
    parent_branch: &str,
    factory_branch: &str,
) -> Result<Vec<String>, crate::worktree::GitError> {
    crate::worktree::GitOperations::new(repo_path.to_path_buf())
        .preflight_merge_conflicts(parent_branch, factory_branch)
}

fn classify_merge_conflict_preflight(
    check: Result<Vec<String>, crate::worktree::GitError>,
) -> (Vec<String>, Option<String>) {
    match check {
        Ok(paths) => (paths, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    }
}

fn enrich_merge_required_with_conflict_check(
    message: String,
    parent_branch: &str,
    task_id: &str,
    conflict_paths: &[String],
    check_error: Option<&str>,
) -> String {
    if !conflict_paths.is_empty() {
        format!(
            "{message}\n\n\
             ⚠️ This branch has a genuine git merge conflict against \
             {parent_branch} (not just unmerged commits) — a supervisor \
             merge attempt will fail here. Conflicting file(s): {}.\n\n\
             Alternative: the assigned worker can \
             `mcp__cas__task action=start id={task_id}` (now permitted from \
             `awaiting_merge`) to resolve the conflict directly on their \
             factory branch and re-close.",
            conflict_paths.join(", ")
        )
    } else if let Some(error) = check_error {
        format!(
            "{message}\n\n\
             ⚠️ CAS could not determine whether this branch merges cleanly \
             into {parent_branch}. Git conflict preflight failed: {error}.\n\n\
             To avoid stranding the task in `awaiting_merge`, CAS marks this \
             park as reopen-eligible. The assigned worker can \
             `mcp__cas__task action=start id={task_id}` to inspect or resolve \
             the branch, then re-close."
        )
    } else {
        message
    }
}

/// Backwards-compatible shim: evaluate the gate with no delivery-scoping
/// evidence (whole-branch semantics). Used by callers that have no close
/// request in hand (e.g. the `AwaitingMerge` delivery precheck) and by the
/// pre-cas-e74c unit tests.
pub(crate) fn run_factory_branch_merge_gate(
    task: &Task,
    req: &TaskCloseRequest,
    parent_branch: &str,
    repo_path: &std::path::Path,
) -> MergeStateGateOutcome {
    run_factory_branch_merge_gate_with_attribution(
        task,
        req,
        parent_branch,
        repo_path,
        TaskCommitAttribution::default(),
    )
}

/// cas-e74c: count commits on `commit_ish` that are not on `parent_branch`
/// AND fall inside this task's work cycle (committer date at or after
/// `window.not_before`, with the same clock-skew allowance the commit
/// receipt uses).
///
/// Returns `None` — "unknowable" — when the merge-base or the rev-list
/// cannot be computed, so the caller falls back to the whole-branch count
/// rather than treating unknown Git state as "nothing attributable".
pub(crate) fn count_task_attributable_unmerged_commits(
    repo_path: &std::path::Path,
    commit_ish: &str,
    parent_branch: &str,
    window: &TaskCommitReceiptWindow,
) -> Option<u32> {
    use std::process::Command;

    if !is_safe_git_refname(commit_ish) || !is_safe_git_refname(parent_branch) {
        return None;
    }

    let merge_base_out = Command::new("git")
        .args(["merge-base", parent_branch, commit_ish])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !merge_base_out.status.success() {
        return None;
    }
    let merge_base = String::from_utf8_lossy(&merge_base_out.stdout)
        .trim()
        .to_string();
    if merge_base.is_empty() {
        return None;
    }

    // Git parses `@<epoch>` as an absolute timestamp, so no locale- or
    // timezone-dependent formatting is involved.
    let since = format!(
        "@{}",
        window.not_before.timestamp() - COMMIT_RECEIPT_CLOCK_SKEW_SECS
    );
    // cas-fdc9 (GH #66): exclude the remote-tracking target too. Measuring
    // this task's commits against a stale local ref alone counts work that
    // already landed on `origin/<parent>` as stranded.
    let since_arg = format!("--since={since}");
    let range = format!("{merge_base}..{commit_ish}");
    let origin_parent = format!("origin/{parent_branch}");
    let mut args = vec!["rev-list", "--count", since_arg.as_str(), range.as_str()];
    if git_ref_exists(repo_path, &origin_parent) {
        args.push("--not");
        args.push(origin_parent.as_str());
    }
    let count_out = Command::new("git")
        .args(&args)
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !count_out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&count_out.stdout)
        .trim()
        .parse()
        .ok()
}

pub(crate) fn run_factory_branch_merge_gate_with_attribution(
    task: &Task,
    _req: &TaskCloseRequest,
    parent_branch: &str,
    repo_path: &std::path::Path,
    attribution: TaskCommitAttribution<'_>,
) -> MergeStateGateOutcome {
    if task.task_type == TaskType::Epic {
        return MergeStateGateOutcome::Proceed;
    }
    let Some(assignee) = task.assignee.as_deref() else {
        return MergeStateGateOutcome::Proceed;
    };
    // cas-cf64 (P3, option-injection hardening): reject leading-`-`
    // assignee/parent-branch names before they ever reach a git shell-out.
    // `count_unmerged_factory_commits`/`fetch_parent_branch_best_effort`
    // also fail closed on this independently (defense in depth for other
    // callers), but doing it here first gives a clear, actionable error
    // instead of a confusing "u32::MAX commits not merged" message.
    if !is_safe_git_refname(assignee) || !is_safe_git_refname(parent_branch) {
        return MergeStateGateOutcome::Reject(format!(
            "⚠️ INVALID BRANCH NAME\n\n\
             task close rejected: the assignee ({assignee:?}) or resolved parent \
             branch ({parent_branch:?}) is not a safe git ref name (empty, or \
             starts with '-'). This looks like corrupted task data or a malformed \
             call — fix the task's assignee/epic-branch fields and retry."
        ));
    }
    let factory_branch = format!("factory/{assignee}");
    let commit_ish = match task.deliverables.factory_branch_anchor.as_deref() {
        Some(tip) if task.status == TaskStatus::AwaitingMerge && git_ref_exists(repo_path, tip) => {
            tip
        }
        _ => factory_branch.as_str(),
    };
    let stranded = count_unmerged_factory_commits(repo_path, commit_ish, parent_branch);
    if stranded == 0 {
        return MergeStateGateOutcome::Proceed;
    }

    // cas-38e2: before rejecting, re-check reachability against
    // `origin/<parent_branch>` — the closing worker's LOCAL view of
    // `parent_branch` can be behind (a merge landed via a different
    // checkout/session and was pushed to origin, but this repo's local
    // ref for it hasn't caught up). A commit already reachable from
    // `origin/<parent_branch>` is genuinely integrated and must not
    // bounce off a stale local ref. See
    // BUG (live repro): worker committed+pushed, supervisor merged into
    // the epic branch and pushed it to origin, worker's close still
    // bounced MERGE REQUIRED because ITS local `parent_branch` ref
    // hadn't observed the merge.
    //
    // Best-effort `git fetch` first (refreshes `origin/<parent_branch>`
    // if a remote is configured and reachable); any failure — no
    // `origin` remote, offline, auth prompt suppressed via
    // `GIT_TERMINAL_PROMPT=0` — is swallowed and we fall back to
    // whatever `origin/<parent_branch>` already resolves to locally (or
    // nothing, if it never existed). This mirrors the graceful-
    // degradation posture of every other gate in this file: an
    // unreachable/unconfigured origin is not treated as evidence of
    // non-integration, just as "nothing extra to check."
    //
    // cas-f522: origin-parent *acceptance* must use the success-bearing
    // [`known_unmerged_factory_commits`] helper — only
    // [`KnownUnmergedCount::KnownZero`] authorizes close. The legacy
    // `count_unmerged_factory_commits == 0` fail-open treats missing
    // merge-base / failed rev-list as "merged" and would let unknowable
    // origin Git state masquerade as integration. Missing origin ref
    // simply skips this block (no rescue); KnownPositive and Unknown
    // fall through to Reject.
    fetch_parent_branch_best_effort(repo_path, parent_branch);
    let origin_parent_branch = format!("origin/{parent_branch}");
    if git_ref_exists(repo_path, &origin_parent_branch)
        && matches!(
            known_unmerged_factory_commits(repo_path, commit_ish, &origin_parent_branch),
            KnownUnmergedCount::KnownZero
        )
    {
        return MergeStateGateOutcome::Proceed;
    }

    // cas-fdc9 (GH #66): re-measure against BOTH target refs now that the
    // fetch has refreshed `origin/<parent_branch>`. The count above came from
    // the local ref alone, which a factory worktree never advances — that is
    // how a worker with one unmerged commit was told nine were stranded. A
    // commit reachable from either ref is merged; only what is on neither is
    // this branch's stranded work. Unknowable git state keeps the local
    // measurement (fail closed), and a partially-merged branch now reports the
    // real remainder instead of stale-base arithmetic.
    let remote_aware_stranded = count_unmerged_against_targets(repo_path, commit_ish, parent_branch);
    if remote_aware_stranded == Some(0) {
        return MergeStateGateOutcome::Proceed;
    }
    let stranded = remote_aware_stranded.unwrap_or(stranded);

    // cas-2938 / cas-5485: when a trusted historical anchor still looks
    // stranded by ancestry (squash A→B, or rebase A→A'), accept close if
    // tip-tree, live KnownZero, or cherry-equivalent patches of the
    // *parked anchor* are on parent. Live KnownZero alone cannot cover
    // serial task B on the same factory branch; cherry-equivalence is
    // task-specific to A and cannot be satisfied by B's unmerged commits.
    // Unknown Git state never authorizes close.
    if task.status == TaskStatus::AwaitingMerge
        && task.deliverables.factory_branch_anchor.is_some()
        && commit_ish != factory_branch.as_str()
    {
        if commit_tip_tree_reachable_from(repo_path, commit_ish, parent_branch) {
            return MergeStateGateOutcome::Proceed;
        }
        if git_ref_exists(repo_path, &origin_parent_branch)
            && commit_tip_tree_reachable_from(repo_path, commit_ish, &origin_parent_branch)
        {
            return MergeStateGateOutcome::Proceed;
        }

        // cas-2938 P0 / cas-5485: live factory tip KnownZero after rewrite.
        if live_factory_tip_known_fully_merged(
            repo_path,
            factory_branch.as_str(),
            parent_branch,
            &origin_parent_branch,
        ) {
            return MergeStateGateOutcome::Proceed;
        }

        // cas-5485 P2: rebased A' integrated while live HEAD carries later
        // unmerged B — prove parked anchor A via patch-id equivalence on
        // parent (not live HEAD zero-ahead). B cannot satisfy this check.
        if commit_patches_cherry_equivalent_on_parent(repo_path, commit_ish, parent_branch) {
            return MergeStateGateOutcome::Proceed;
        }
        if git_ref_exists(repo_path, &origin_parent_branch)
            && commit_patches_cherry_equivalent_on_parent(
                repo_path,
                commit_ish,
                &origin_parent_branch,
            )
        {
            return MergeStateGateOutcome::Proceed;
        }
    }

    // cas-e74c (GH #80): a valid `commit_receipt` IS the delivery evidence.
    // When it resolves to a commit of this work cycle that carries a
    // non-empty diff and is already reachable from `parent_branch` (or
    // `origin/<parent_branch>`), the task's work has landed — regardless of
    // what else the reused lane branch is still carrying. That residue
    // belongs to other tasks (or to a supervisor decision not to merge the
    // lane at all); it is logged on the closing task, not made fatal.
    // Without this, a cherry-pick delivery from a dirty lane could never
    // close: merging the lane would land unrelated commits on the target,
    // and the receipt path — documented for exactly this case — did not
    // exempt the close.
    let mut receipt_rejection_reason: Option<String> = None;
    if let Some(receipt) = attribution.receipt {
        match attribution.window {
            Some(window) => {
                match validate_task_commit_receipt(repo_path, receipt, parent_branch, window) {
                    Ok(note) => {
                        return MergeStateGateOutcome::ProceedWithNote(format!(
                            "{note} merge-state guard: cleared by delivery receipt; \
                             {factory_branch} still carries {stranded} commit(s) not on \
                             {parent_branch}, which are not attributable to this task's \
                             delivery and remain the lane's own residue."
                        ));
                    }
                    Err(reason) => receipt_rejection_reason = Some(reason),
                }
            }
            None => {
                receipt_rejection_reason =
                    Some("task attribution window is unavailable".to_string());
            }
        }
    }

    // cas-e74c (GH #62 symptoms 3-4): scope the guard to commits made
    // inside this task's own work cycle. A reused lane branch routinely
    // carries commits from prior (already closed, already merged-elsewhere)
    // tasks; demanding that the closing task merge them to its own target
    // is both wrong and, for a zero-commit task, impossible to satisfy.
    // Unknowable Git state falls back to the whole-branch count (fail
    // closed), and commits this task actually made still reject below.
    let attributable = attribution.window.and_then(|window| {
        count_task_attributable_unmerged_commits(repo_path, commit_ish, parent_branch, window)
    });
    if attributable == Some(0) {
        return MergeStateGateOutcome::ProceedWithNote(format!(
            "decision: merge-state guard cleared — no commit on {factory_branch} is \
             attributable to this task's work cycle (basis: {}). The branch still \
             carries {stranded} commit(s) not on {parent_branch}; that residue \
             belongs to other tasks and is recorded here rather than blocking this \
             close.",
            attribution
                .window
                .map(|window| window.basis)
                .unwrap_or("task work cycle"),
        ));
    }
    let stranded = attributable.unwrap_or(stranded);

    // cas-c631: `epic/<slug>` branches are created locally by the supervisor
    // (see cas-supervisor EPIC workflow) and are, by convention, never pushed
    // to origin — the epic ships to `main` as a single PR once complete, not
    // per-worker. Telling a worker to "open a PR targeting {parent_branch}"
    // when `parent_branch` is one of these local-only epic branches sends
    // them straight at a `gh pr create --base epic/<slug>` call that fails
    // (no such ref on origin), which is exactly the recurring close-time
    // friction this task exists to fix. Detect that case by the naming
    // convention (cheap, deterministic, matches the same `starts_with(
    // "epic/")` check used elsewhere for epic branches, e.g.
    // `mcp/tools/mod.rs` and `worktree/manager/epic_ops.rs`) and hand the
    // worker a supervisor-merge-request handoff instead of PR instructions.
    let parent_is_local_epic_branch = parent_branch.starts_with("epic/");
    let branch_tip = resolve_branch_sha(repo_path, &factory_branch)
        .unwrap_or_else(|| "unresolved at close rejection".to_string());
    let coord = worker_coordination_tool();

    let remediation = if parent_is_local_epic_branch {
        format!(
            "Remediation:\n\
             1. Before escalating, repeatedly run `{coord} action=inbox_poll` \
             until it returns `No unread messages`. A default poll returns at most \
             10 rows, so one poll is not a complete freshness check. Polling marks \
             messages seen without consuming daemon transport delivery. The polling \
             claim is at-most-once: if its MCP response is lost, those rows are not \
             replayed by another poll, so also re-read any just-delivered supervisor \
             messages in your conversation. If one says this branch was merged or \
             requests more changes, follow it and do not send a stale merge \
             request.\n\
             2. {parent_branch} is a local-only epic branch (not pushed to origin) \
             — do NOT run `gh pr create --base {parent_branch}`, it has no \
             matching ref on origin and the PR will fail.\n\
             3. Push {factory_branch} to origin so your commit is durable: \
             `git push origin {factory_branch}`\n\
             4. If a merge is still needed, message your supervisor to merge \
             {factory_branch} into {parent_branch}, including the current tip \
             and freshness qualifier (e.g. \
             `{coord} action=message \
             target=supervisor task_id={} summary=\"ready to merge\" message=\"Fresh after \
             draining unread inbox messages until No unread messages: \
             {factory_branch} tip {branch_tip}; please re-check reachability, then \
             merge into {parent_branch} if still needed\"`). \
             They merge with \
             `git merge --no-ff {factory_branch}` on the epic branch.\n\
             5. Once merged, retry mcp__cas__task action=close. If the supervisor \
             declines the unmerged delivery instead, the supervisor runs \
             `mcp__cas__task action=request_changes id={} reason=\"state what prior work remains and what must be corrected or reverted\"`; \
             only after that verdict may the assigned worker start a fresh cycle.",
            task.id,
            task.id,
        )
    } else {
        format!(
            "Remediation:\n\
             1. Repeatedly run `{coord} action=inbox_poll` until it returns \
             `No unread messages` before continuing. A default poll returns at \
             most 10 rows, so one poll is not a complete freshness check. Follow \
             every unread merge or review instruction it returns. The polling \
             claim is at-most-once, so also re-read just-delivered supervisor \
             messages in case the MCP response was lost after claiming rows.\n\
             2. Push {factory_branch} to its remote\n\
             3. Open a PR targeting {parent_branch}\n\
             4. Merge the PR. CAS already fetched and measured this branch \
             against BOTH {parent_branch} and origin/{parent_branch}, so a \
             merge that has landed on either one is already counted — running \
             `git fetch` again will not change this number, and a stale local \
             {parent_branch} ref cannot be the cause (fetch never moves a local \
             branch ref). If you believe the work is merged, check it directly: \
             `git merge-base --is-ancestor {factory_branch} origin/{parent_branch}`.\n\
             5. Retry mcp__cas__task action=close. If the supervisor declines \
             the unmerged delivery instead, the supervisor runs \
             `mcp__cas__task action=request_changes id={} reason=\"state what prior work remains and what must be corrected or reverted\"`; \
             only after that verdict may the assigned worker start a fresh cycle.",
            task.id,
        )
    };

    // cas-e74c: when a receipt was supplied but did not validate, say why —
    // otherwise the worker sees a bare MERGE REQUIRED and cannot tell that
    // their receipt was even considered.
    let receipt_note = match receipt_rejection_reason {
        Some(reason) => format!(
            "\nThe supplied commit_receipt was not accepted as merge evidence: \
             {reason}.\n"
        ),
        None => String::new(),
    };

    MergeStateGateOutcome::Reject(format!(
        "⚠️ MERGE REQUIRED\n\n\
         task close rejected: {factory_branch} has {stranded} commit(s) from this task \
         not on {parent_branch}.\n{receipt_note}\n\
         The branch must be merged into {parent_branch} before closing. This \
         guard cannot be bypassed (use of bypass_code_review=true does not \
         skip merge-state checks — it is a data-state guard, not a review \
         gate).\n\n\
         {remediation}",
    ))
}

/// Count commits reachable from `factory_branch` but not from
/// `parent_branch`, within the git repository rooted at `repo_path`.
///
/// Returns 0 (treated as "merged" by [`run_factory_branch_merge_gate`])
/// when:
/// - The factory branch ref does not resolve locally (worker may
///   have pushed-and-pruned, or never pushed in this checkout).
/// - The merge-base between the two refs cannot be computed.
/// - `git rev-list --count` fails or returns an unparseable value.
///
/// Mirrors the shell-out style of
/// [`check_additive_only_branch_violations`] — no external git crate.
///
/// cas-cf64 (P3, option-injection hardening): returns `u32::MAX`
/// ("maximally stranded", forcing the caller to Reject) instead of the
/// usual graceful-degradation `0` when either ref fails
/// [`is_safe_git_refname`] — a ref starting with `-` reaching
/// `git merge-base`/`git rev-list` would be parsed as an option, not a ref
/// name. This is a deliberate departure from this function's normal
/// fail-open-to-0 posture: an invalid ref here means corrupted task data
/// or a malformed call, and the safe direction for a close-integrity gate
/// is to refuse, not to silently treat unparseable input as "merged".
pub(crate) fn count_unmerged_factory_commits(
    repo_path: &std::path::Path,
    factory_branch: &str,
    parent_branch: &str,
) -> u32 {
    use std::process::Command;

    if !is_safe_git_refname(factory_branch) || !is_safe_git_refname(parent_branch) {
        return u32::MAX;
    }

    // Resolve merge-base explicitly so we get a clean failure signal
    // when either ref can't be resolved (vs. silently comparing
    // against the wrong base via the `a..b` revspec).
    let merge_base_out = Command::new("git")
        .args(["merge-base", parent_branch, factory_branch])
        .current_dir(repo_path)
        .output();
    let merge_base = match merge_base_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return 0,
    };
    if merge_base.is_empty() {
        return 0;
    }

    let count_out = Command::new("git")
        .args([
            "rev-list",
            "--count",
            &format!("{merge_base}..{factory_branch}"),
        ])
        .current_dir(repo_path)
        .output();
    match count_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<u32>()
            // Saturate on overflow so an implausibly large count
            // still maps to "stranded" (Reject), not 0 (Proceed).
            // Unreachable in practice but the semantically correct
            // direction for an unparseable count.
            .unwrap_or(u32::MAX),
        _ => 0,
    }
}

/// Success-bearing unmerged-count for close-integrity **acceptance** paths
/// (cas-2938 live-ref convergence / cas-5485 pre-rebase SHA refresh).
///
/// Unlike [`count_unmerged_factory_commits`], which deliberately fail-opens
/// to `0` when Git history is unknowable (legacy Proceed-friendly posture
/// for the primary ancestry path), this tri-state never maps unknown state
/// to zero. Callers that authorize close must match on [`KnownZero`] only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KnownUnmergedCount {
    /// Refs resolved, merge-base computed, rev-list succeeded with count 0.
    KnownZero,
    /// Refs resolved and rev-list reported a positive stranded count.
    KnownPositive(u32),
    /// Missing ref, failed merge-base, failed/unparseable rev-list, or
    /// unsafe refname — Git state is not evidence of convergence.
    Unknown,
}

/// cas-5485 / cas-2938: true when the live factory tip is **known** fully
/// merged into `parent_branch` or `origin/<parent>` (KnownZero only).
///
/// This is the auditable "refresh to current factory tip" acceptance proof
/// used when a parked pre-rebase (or pre-squash) anchor still looks
/// stranded by ancestry. Does not treat Unknown as merged.
fn live_factory_tip_known_fully_merged(
    repo_path: &std::path::Path,
    factory_branch: &str,
    parent_branch: &str,
    origin_parent_branch: &str,
) -> bool {
    if matches!(
        known_unmerged_factory_commits(repo_path, factory_branch, parent_branch),
        KnownUnmergedCount::KnownZero
    ) {
        return true;
    }
    git_ref_exists(repo_path, origin_parent_branch)
        && matches!(
            known_unmerged_factory_commits(repo_path, factory_branch, origin_parent_branch),
            KnownUnmergedCount::KnownZero
        )
}

/// cas-5485 P2: true when every commit unique to `commit_ish` (vs
/// `parent_ref`) has a patch-id-equivalent commit on `parent_ref`
/// (`git cherry` marks them `-`).
///
/// Used when a parked pre-rebase tip A was rewritten to A' and A' is
/// integrated, but the live factory HEAD carries later unmerged work B
/// (so live KnownZero cannot clear). Equivalence is evaluated against
/// the **parked task anchor**, never against live HEAD — so B cannot
/// satisfy task A.
///
/// Fail-closed: missing refs, unsafe names, failed `git cherry`, empty
/// output (no positive evidence), or any `+` (non-equivalent) line →
/// false.
fn commit_patches_cherry_equivalent_on_parent(
    repo_path: &std::path::Path,
    commit_ish: &str,
    parent_ref: &str,
) -> bool {
    use std::process::Command;

    if !is_safe_git_refname(commit_ish) || !is_safe_git_refname(parent_ref) {
        return false;
    }
    if !git_ref_exists(repo_path, commit_ish) || !git_ref_exists(repo_path, parent_ref) {
        return false;
    }

    // `git cherry <upstream> <head>` lists commits reachable from head but
    // not upstream; prefix `-` = equivalent patch on upstream, `+` = not.
    let cherry_out = Command::new("git")
        .args(["cherry", parent_ref, commit_ish])
        .current_dir(repo_path)
        .output();
    let Ok(o) = cherry_out else {
        return false;
    };
    if !o.status.success() {
        return false;
    }

    let stdout = String::from_utf8_lossy(&o.stdout);
    let mut saw_equivalent = false;
    for line in stdout.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // Lines look like "+ <sha>" or "- <sha>".
        if t.starts_with('+') {
            return false;
        }
        if t.starts_with('-') {
            saw_equivalent = true;
            continue;
        }
        // Unrecognized cherry output is not evidence of equivalence.
        return false;
    }
    // Empty output is not positive proof (already-ancestor cases are
    // handled by the primary ancestry path; fail closed here).
    saw_equivalent
}

/// cas-fdc9 (GH #66): count commits on `commit_ish` that are on NEITHER the
/// local `parent_branch` ref NOR its remote-tracking `origin/<parent_branch>`.
///
/// A factory worktree is cut with whatever the local target ref pointed at and
/// never advances it. On a repository whose target moves often, that ref is
/// stale within minutes, so a count measured against it is arithmetic about a
/// base nobody merges into — the reported "9 commits not on staging" when
/// exactly one was unmerged. Both refs are consulted because either one can be
/// the current truth: origin is ahead when merges land elsewhere, and the local
/// ref is ahead for a local-only epic branch that is never pushed.
///
/// Returns `None` when git cannot answer (missing ref, failed rev-list), so
/// callers keep their existing fail-closed local measurement instead of
/// treating "couldn't tell" as "nothing stranded".
pub(crate) fn count_unmerged_against_targets(
    repo_path: &std::path::Path,
    commit_ish: &str,
    parent_branch: &str,
) -> Option<u32> {
    use std::process::Command;

    if !is_safe_git_refname(commit_ish) || !is_safe_git_refname(parent_branch) {
        return None;
    }
    if !git_ref_exists(repo_path, commit_ish) || !git_ref_exists(repo_path, parent_branch) {
        return None;
    }

    let origin_parent = format!("origin/{parent_branch}");
    let mut args = vec!["rev-list", "--count", commit_ish, "--not", parent_branch];
    if git_ref_exists(repo_path, &origin_parent) {
        args.push(origin_parent.as_str());
    }

    let out = Command::new("git")
        .args(&args)
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// cas-fdc9 (GH #56): is a supplied `commit_receipt` even present in the
/// repository this close is bound to?
///
/// A cross-repo delivery reported a receipt for a commit that existed only in
/// the repo where the work actually landed. Nothing refused it — no gate
/// happened to need the receipt on that path — so an unverifiable SHA entered
/// the audit trail as if it had been checked. A receipt is evidence; when it
/// cannot be resolved here, say so and point at the declared-target-repo
/// mechanism instead of recording false assurance.
///
/// Returns `Some(message)` when the receipt does not resolve in `repo_path`,
/// `None` when it does. Ancestry, diff and work-cycle semantics stay with
/// [`validate_task_commit_receipt`]; this is only the repo-binding question.
pub(crate) fn commit_receipt_repo_binding_error(
    repo_path: &std::path::Path,
    receipt: &str,
) -> Option<String> {
    let reason = resolve_task_commit_receipt_sha(repo_path, receipt).err()?;
    Some(format!(
        "⚠️ RECEIPT NOT FOUND IN THIS REPOSITORY\n\n\
         task close rejected: commit_receipt `{receipt}` does not resolve in the \
         repository this close is bound to ({}): {reason}.\n\n\
         A receipt is merge evidence, so CAS refuses to record one it cannot \
         verify here — a receipt from another repository would read as proof \
         while proving nothing.\n\n\
         To resolve:\n\
         1. If the work landed in a DIFFERENT repository, declare it on the task \
            so every close gate runs there: \
            `mcp__cas__task action=update id=<task> target_repo=<path-or-selector> \
            target_branch=<branch>`, then retry close.\n\
         2. If the work is in this repository, re-copy the SHA \
            (`git log --oneline --all`) — full or an unambiguous abbreviation \
            both work.",
        repo_path.display(),
    ))
}

/// Explicit success-bearing counterpart to [`count_unmerged_factory_commits`].
///
/// Returns:
/// - [`KnownUnmergedCount::KnownZero`] only when both refs resolve, merge-base
///   succeeds, and `rev-list --count` parses as `0`.
/// - [`KnownUnmergedCount::KnownPositive`] when the count is a known `> 0`
///   (or the cas-cf64 unsafe-refname fail-closed `u32::MAX` case).
/// - [`KnownUnmergedCount::Unknown`] on any resolution/computation failure —
///   never treats "couldn't tell" as "zero ahead".
pub(crate) fn known_unmerged_factory_commits(
    repo_path: &std::path::Path,
    factory_branch: &str,
    parent_branch: &str,
) -> KnownUnmergedCount {
    use std::process::Command;

    if !is_safe_git_refname(factory_branch) || !is_safe_git_refname(parent_branch) {
        // Corrupted/injection input: not KnownZero. Surface as positive so
        // any caller that only checks `== KnownZero` still refuses, and
        // callers that inspect magnitude still see "stranded".
        return KnownUnmergedCount::KnownPositive(u32::MAX);
    }

    // Both tips must resolve — missing factory or parent is Unknown, not zero.
    if !git_ref_exists(repo_path, factory_branch) || !git_ref_exists(repo_path, parent_branch) {
        return KnownUnmergedCount::Unknown;
    }

    let merge_base_out = Command::new("git")
        .args(["merge-base", parent_branch, factory_branch])
        .current_dir(repo_path)
        .output();
    let merge_base = match merge_base_out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                return KnownUnmergedCount::Unknown;
            }
            s
        }
        _ => return KnownUnmergedCount::Unknown,
    };

    let count_out = Command::new("git")
        .args([
            "rev-list",
            "--count",
            &format!("{merge_base}..{factory_branch}"),
        ])
        .current_dir(repo_path)
        .output();
    match count_out {
        Ok(o) if o.status.success() => {
            match String::from_utf8_lossy(&o.stdout).trim().parse::<u32>() {
                Ok(0) => KnownUnmergedCount::KnownZero,
                Ok(n) => KnownUnmergedCount::KnownPositive(n),
                // Unparseable count is not evidence of zero.
                Err(_) => KnownUnmergedCount::Unknown,
            }
        }
        _ => KnownUnmergedCount::Unknown,
    }
}

/// cas-2938: true when the tip tree of `commit_ish` appears on `parent_ref`.
///
/// Clean GitHub squash-merges rewrite the commit SHA (so ancestry fails)
/// but preserve the factory tip tree as the squash commit's tree. Scanning
/// parent history for that tree object is the content-level equivalence
/// check that lets an AwaitingMerge task clear after squash without
/// requiring the pre-squash SHA to be an ancestor.
///
/// Returns false on any resolution failure (missing ref, unsafe name,
/// empty tree sha) — fail closed toward "not integrated" so the live-ref
/// path / Reject can still decide.
fn commit_tip_tree_reachable_from(
    repo_path: &std::path::Path,
    commit_ish: &str,
    parent_ref: &str,
) -> bool {
    use std::process::Command;

    if !is_safe_git_refname(commit_ish) || !is_safe_git_refname(parent_ref) {
        return false;
    }
    if !git_ref_exists(repo_path, commit_ish) || !git_ref_exists(repo_path, parent_ref) {
        return false;
    }

    let tree_out = Command::new("git")
        .args(["rev-parse", &format!("{commit_ish}^{{tree}}")])
        .current_dir(repo_path)
        .output();
    let tree = match tree_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return false,
    };
    if tree.is_empty() || !tree.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }

    // Walk parent history for any commit whose tree matches the anchor tip.
    // `--pretty=%T` emits one tree SHA per commit; exact-line match avoids
    // substring false positives from longer hashes (full SHA is fixed length).
    let log_out = Command::new("git")
        .args(["log", "--pretty=%T", parent_ref])
        .current_dir(repo_path)
        .output();
    match log_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .any(|line| line.trim() == tree),
        _ => false,
    }
}

/// cas-38e2 / cas-cf64 (P3, bounded + validated): best-effort
/// `git fetch origin <parent_branch>` inside `repo_path`, refreshing the
/// `origin/<parent_branch>` remote-tracking ref before
/// [`run_factory_branch_merge_gate`] consults it as a fallback.
///
/// Deliberately fire-and-forget:
/// - No `origin` remote configured (common for local-only dev repos, or the
///   `epic/<slug>` local-only-branch convention) → the command fails fast
///   and is ignored; the caller falls back to whatever `origin/<parent_branch>`
///   already resolves to (nothing, if it never existed).
/// - Offline / unreachable remote → same graceful ignore.
/// - `GIT_TERMINAL_PROMPT=0` prevents a credential prompt from hanging the
///   close call indefinitely on a private remote with no cached credentials.
///
/// cas-cf64 (P3) hardening on top of the original cas-38e2 version:
/// - **Bounded**: this fires on the reject path on EVERY retry (there is no
///   fetch-once-per-park cache), so a worker looping `close` on a parked
///   task against a slow/blackholed `origin` would otherwise re-hang the
///   synchronous MCP handler each attempt. The child process is killed if
///   it hasn't finished within [`FETCH_TIMEOUT`], bounding worst-case
///   added latency per close attempt regardless of transport (SSH/HTTP/
///   filesystem) or how the remote fails.
/// - **Validated**: `parent_branch` is checked with [`is_safe_git_refname`]
///   before ever reaching the shell-out. `git fetch <remote> <refspec>` has
///   no safe `--` end-of-options marker, so a `parent_branch` value
///   starting with `-` would otherwise be parsed as a git option instead of
///   a ref name.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) fn fetch_parent_branch_best_effort(repo_path: &std::path::Path, parent_branch: &str) {
    use std::process::{Command, Stdio};
    use std::time::Instant;

    if !is_safe_git_refname(parent_branch) {
        return;
    }

    // Force-update the exact remote-tracking ref. The leading `+` is
    // intentional: a remote epic may have been rebased/force-pushed, and the
    // caller needs the authoritative remote state rather than a fetch rejected
    // as non-fast-forward.
    let refspec = format!("+refs/heads/{parent_branch}:refs/remotes/origin/{parent_branch}");
    let mut child = match Command::new("git")
        .args(["fetch", "--quiet", "origin", &refspec])
        .current_dir(repo_path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return,
    };

    let deadline = Instant::now() + FETCH_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return, // finished (success or failure — don't care)
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return,
        }
    }
}

/// cas-cf64 (P3, option-injection hardening): `true` when `name` is safe to
/// pass as a git ref/branch-name argument to a git subcommand.
///
/// None of the git subcommands this module shells out to
/// (`fetch`, `merge-base`, `rev-list`, `rev-parse --verify`) offer a safe
/// `--` end-of-options marker at every position a branch name is passed —
/// `git fetch <remote> <refspec>` in particular has none — so callers
/// validate at the source instead of trying to escape per call site. A
/// name starting with `-` would otherwise be parsed as a command-line
/// option (by git itself, or by ssh/git's transport helpers) rather than a
/// ref name.
fn is_safe_git_refname(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-')
}

// ---------------------------------------------------------------------------
// cas-762e (B2): factory branch merge-reality gate
// ---------------------------------------------------------------------------

/// Outcome of the B2 factory-branch merge-reality close gate.
///
/// Mirrors [`MergeStateGateOutcome`] in shape. The gate refuses close when
/// a factory branch exists locally with 0 commits beyond the parent branch
/// AND the branch was never pushed to `origin` — the combination that
/// distinguishes "committed to wrong place" from "merged and pruned".
#[derive(Debug)]
pub(crate) enum MergeRealityOutcome {
    /// Close may proceed — the factory branch is absent, has commits, or
    /// shows evidence of a prior push (remote tracking ref exists).
    Proceed,
    /// Close must be refused with this user-facing error message.
    Refuse(String),
}

/// B2 gate: refuse close when the worker's factory branch exists locally,
/// has **zero** commits beyond `parent_branch`, and was **never pushed** to
/// `origin`.
///
/// ## Why this gate is needed
///
/// `run_factory_branch_merge_gate` (cas-95ce) returns `Proceed` when
/// `count_unmerged_factory_commits == 0`.  Zero is correct for the normal
/// merged path (all commits landed on parent), but also true when the worker
/// never committed to `factory/<name>` — e.g., the commits leaked into the
/// supervisor's main checkout instead of the isolated worktree (cas-073f).
///
/// B2 distinguishes the two cases by checking the remote tracking ref:
/// - `origin/factory/<name>` present → branch was pushed → PR likely
///   opened/merged → 0 commits = correct post-merge state → **Proceed**
/// - `origin/factory/<name>` absent AND 0 commits AND branch exists →
///   work was never delivered through the normal push/review path →
///   **Refuse**
///
/// ## Safe-fail behaviour
///
/// - If `factory/<name>` does not exist locally (push+merge+prune),
///   returns `Proceed` — matching the graceful-pass documented in
///   [`count_unmerged_factory_commits`].
/// - If `count_unmerged_factory_commits > 0`, returns `Proceed` so the
///   existing cas-95ce gate (which runs earlier) handles that rejection;
///   B2 must not double-reject.
/// - Any git failure inside the helpers returns the safe-default
///   `Proceed` (never a false `Refuse`).
///
/// ## Call-site bypass conditions
///
/// The caller in `cas_task_close` gates this function behind:
/// - `is_factory_worker` — supervisor closes are not affected
/// - `task.task_type != Epic` — epic close is handled elsewhere
/// - `execution_note != "additive-only"` — no commit expected
/// - `!bypass_close_gates` — supervisor emergency bypass
/// - `effective_has_reviewable` — zero-diff tasks don't need commits
pub(crate) fn check_factory_branch_merge_reality(
    repo_path: &std::path::Path,
    assignee: &str,
    parent_branch: &str,
) -> MergeRealityOutcome {
    let factory_branch = format!("factory/{assignee}");

    // Branch absent locally → push+merge+prune path; treat as merged.
    if !git_ref_exists(repo_path, &factory_branch) {
        return MergeRealityOutcome::Proceed;
    }

    // ≥1 unmerged commits: cas-95ce (run earlier in the pipeline) already
    // blocks this case. Return Proceed so B2 doesn't double-reject.
    if count_unmerged_factory_commits(repo_path, &factory_branch, parent_branch) > 0 {
        return MergeRealityOutcome::Proceed;
    }

    // 0 unmerged commits. Check whether the branch was ever pushed: if
    // `origin/<factory_branch>` exists, the work went through the normal
    // push/PR path and 0 commits = post-merge clean state.
    let remote_ref = format!("origin/{factory_branch}");
    if git_ref_exists(repo_path, &remote_ref) {
        return MergeRealityOutcome::Proceed;
    }

    // Branch exists + 0 commits beyond parent + no remote push evidence.
    // The worker's commits almost certainly landed on the wrong branch.
    MergeRealityOutcome::Refuse(format!(
        "⚠️ MERGE REALITY CHECK FAILED\n\n\
         task close rejected: {factory_branch} has no commits beyond \
         {parent_branch} and has never been pushed to origin.\n\n\
         This typically means commits landed on the wrong branch (e.g., the \
         supervisor's main checkout instead of the factory worktree), or the \
         branch was never committed to at all.\n\n\
         Remediation:\n\
         1. Check where your commits actually landed:\n\
            `git log --oneline {parent_branch} -5` and \
            `git log --oneline {factory_branch} -5`\n\
         2. If commits are on the wrong branch, move them onto \
         {factory_branch}:\n\
            `git cherry-pick <sha>` or `git rebase --onto {factory_branch}`\n\
         3. Push {factory_branch} to origin:\n\
            `git push origin {factory_branch}`\n\
         4. Open a PR targeting {parent_branch} and merge it.\n\
         5. Retry: `mcp__cas__task action=close`\n\n\
         If this task intentionally has no code commits, the Supervisor can \
         bypass with `bypass_code_review=true`.",
    ))
}

/// Return `true` if `refname` resolves to an existing commit object in the
/// git repository at `repo_path`.
///
/// `rev-parse --verify` alone accepts a syntactically valid full object ID even
/// when that object is absent. Every caller feeds the result to a commit-history
/// operation, so verify both object existence and commit shape with
/// `git cat-file -e <refname>^{commit}`.
fn git_ref_exists(repo_path: &std::path::Path, refname: &str) -> bool {
    use std::process::Command;
    Command::new("git")
        .args(["cat-file", "-e", "--", &format!("{refname}^{{commit}}")])
        .current_dir(repo_path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Resolve `refname` to its full commit sha in the git repository at
/// `repo_path` (equivalent to `git rev-parse --verify <refname>`, returning
/// the trimmed stdout instead of just a boolean). Returns `None` on any
/// failure — used by the cas-4b3f factory-branch-anchor snapshot, where an
/// unresolvable ref simply means "nothing to anchor yet", not an error.
pub(crate) fn resolve_branch_sha(repo_path: &std::path::Path, refname: &str) -> Option<String> {
    use std::process::Command;
    let out = Command::new("git")
        .args(["rev-parse", "--verify", refname])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

pub(crate) fn git_merge_base(
    repo_path: &std::path::Path,
    left: &str,
    right: &str,
) -> Option<String> {
    use std::process::Command;
    let out = Command::new("git")
        .args(["merge-base", left, right])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(sha)
    } else {
        None
    }
}

/// Resolve the git repository that owns `cas_root` for close-time enforcement.
///
/// A CAS root is conventionally `<repo>/.cas`, but custom state layouts may
/// place it more deeply inside the checkout. Walking ancestors keeps those
/// layouts bound to the owning repository while refusing roots that are not
/// contained in a git checkout at all. A `.git` file is accepted as well as a
/// directory so linked worktrees use the same path.
fn resolve_close_gate_repo_root(cas_root: &std::path::Path) -> Result<std::path::PathBuf, String> {
    use std::process::Command;

    for ancestor in cas_root.ancestors() {
        if !ancestor.join(".git").exists() {
            continue;
        }
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(ancestor)
            .output();
        match output {
            Ok(output) if output.status.success() => {
                let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !root.is_empty() {
                    return Ok(std::path::PathBuf::from(root));
                }
            }
            _ => continue,
        }
    }

    Err(format!(
        "⚠️ CLOSE GATE GIT REPOSITORY ERROR\n\n\
         Cannot resolve a git repository containing CAS root {}. Close-time \
         merge enforcement refuses to proceed because git state would be \
         unknowable. Start CAS from a project checkout or configure CAS_ROOT \
         beneath the intended repository, then retry.",
        cas_root.display()
    ))
}

/// Resolve the repository's default branch for close-time merge enforcement.
///
/// `origin/HEAD` is authoritative when configured. Local-only repositories
/// then use the conventional branch refs in deterministic order (`main`,
/// followed by `master`). No current-HEAD or hardcoded fallback is allowed:
/// close gates must not bind to an arbitrary feature branch or silently guess
/// when HEAD is detached and neither conventional default exists.
fn resolve_close_gate_default_branch(repo_path: &std::path::Path) -> Result<String, String> {
    use std::process::Command;

    let remote_head = Command::new("git")
        .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
        .current_dir(repo_path)
        .output();
    if let Ok(output) = remote_head {
        if output.status.success() {
            let reference = String::from_utf8_lossy(&output.stdout);
            if let Some(branch) = reference
                .trim()
                .strip_prefix("refs/remotes/origin/")
                .filter(|branch| is_safe_git_refname(branch))
            {
                return Ok(branch.to_string());
            }
        }
    }

    for candidate in ["main", "master"] {
        let reference = format!("refs/heads/{candidate}");
        let exists = Command::new("git")
            .args(["show-ref", "--verify", "--quiet", &reference])
            .current_dir(repo_path)
            .status();
        if matches!(exists, Ok(status) if status.success()) {
            return Ok(candidate.to_string());
        }
    }

    Err(format!(
        "⚠️ CLOSE GATE DEFAULT BRANCH ERROR\n\n\
         Cannot resolve the default branch for git repository {}. \
         refs/remotes/origin/HEAD is unset and neither local `main` nor \
         `master` exists. Close-time merge enforcement refuses to guess; \
         configure origin/HEAD or set an explicit task/epic base branch, \
         then retry.",
        repo_path.display()
    ))
}

/// cas-cf64: resolve the real integration target for a non-epic task with
/// no parent-epic branch recorded.
///
/// cas-4b3f's original fix for BUG-close-guard-nonepic-task-targets-main
/// skipped the merge-state gate ENTIRELY in this case (treating the target
/// as unset) rather than guess `"main"`. That over-corrected: a standalone
/// task that commits real code to `factory/<assignee>` and never merges it
/// now closed cleanly with no backstop at all. The right target isn't
/// "skip" or "main" — it's whatever this repo's actual configured/detected
/// trunk is, exactly what epic-branch creation and worker-spawn base
/// resolution already agree on (cas-b082):
///
/// 1. `[factory] epic_base_branch` from `.cas/config.toml`, if configured.
/// 2. Otherwise the shared close-gate default resolver (remote HEAD →
///    existing `main` → existing `master`), which fails closed rather than
///    guessing from current HEAD.
///
/// Genuine review/docs/zero-commit standalone tasks are unaffected: when
/// `factory/<assignee>` has no commits beyond whatever this resolves to,
/// `count_unmerged_factory_commits` still naturally returns 0 → Proceed.
fn resolve_standalone_merge_target(repo_path: &std::path::Path) -> Result<String, String> {
    match crate::config::Config::configured_epic_base_branch(repo_path) {
        Some(branch) => Ok(branch),
        None => resolve_close_gate_default_branch(repo_path),
    }
}

/// cas-7efe: the single, authoritative parent-branch resolution policy for
/// every close-time gate in `cas_task_close` (merge gate, commit-claim
/// gate, additive-only gate, zero-commit gate, diff stat).
///
/// Before this fix, four of the five gates resolved the parent branch via
/// `task.worktree_id -> worktree_store.get(..).parent_branch`, falling back
/// to a hardcoded `.unwrap_or_else(|| "main".to_string())` whenever
/// `worktree_id` was unset (the common System-B factory-isolation case —
/// see `resolve_worker_worktree_path`) or the worktree-store lookup failed.
/// On an epic based on a non-`main` branch (e.g. `staging`), that silently
/// evaluated every downstream gate against the wrong branch:
///
/// - `check_zero_commit_close`'s cas-127f merge-satisfied path calls
///   `commit_is_merged_into_parent(anchor, "main")`. The anchor was merged
///   into the epic, not `main` -> `false` -> an ambiguous ZERO-COMMIT
///   rejection immediately after the supervisor did exactly what the prior
///   MERGE REQUIRED rejection demanded (the catch-22 documented in
///   BUG-zero-commit-close-gate-catch22.md).
/// - `get_worker_diff_stat(wt, "main")` diffs across the *entire*
///   staging/main divergence instead of the task's own contribution,
///   producing a ~110KB result that overflows the MCP tool-result token
///   limit (BUG-task-close-returns-110kb-diffstat-overflowing-token-limit.md).
///
/// Resolution order (first `Some` wins):
///
/// 1. `worktree_parent_branch` — the `WorktreeStore` row's recorded parent
///    for `task.worktree_id` (System A). Most specific when present: it's
///    the actual recorded parent of this task's own worktree.
/// 2. `epic_branch` — `task_store.get_parent_epic(task_id).branch`. Covers
///    System-B isolated workers (`spawn_workers isolate=true`), which are
///    the day-to-day factory path and almost never set `worktree_id`.
/// 3. [`resolve_standalone_merge_target`] — configured `epic_base_branch`,
///    falling back to git's own detected default branch. Used only when
///    neither tier above resolves (a standalone task with no parent epic).
///
/// Never a bare `"main"` literal: tier 3 is a real resolution (configured
/// value or git-detected default), not a guess, so this function always
/// returns a genuine answer rather than silently guessing.
fn resolve_close_parent_branch(
    worktree_parent_branch: Option<String>,
    epic_branch: Option<String>,
    repo_path: &std::path::Path,
) -> Result<String, String> {
    match worktree_parent_branch.or(epic_branch) {
        Some(branch) => Ok(branch),
        None => resolve_standalone_merge_target(repo_path),
    }
}

/// cas-e093: build the `task.close` success message with the confirmation
/// line always first (`"Closed task: <id> - <title>"`), so it survives
/// truncation/spilling if the rest of the payload (e.g. a wide diff stat)
/// turns out to be large. See BUG-task-close-returns-110kb-diffstat-
/// overflowing-token-limit.md: a successful close whose confirmation is
/// buried inside a spilled file presents as a failure to the caller.
#[allow(clippy::too_many_arguments)]
fn format_close_success_message(
    task_id: &str,
    task_title: &str,
    verification_note: &str,
    lease_msg: &str,
    worktree_msg: &str,
    diff_stat_msg: &str,
    epic_close_msg: &str,
    commit_nudge_msg: &str,
    auto_unblock_msg: &str,
) -> String {
    format!(
        "Closed task: {task_id} - {task_title}{verification_note}{lease_msg}{worktree_msg}\
         {diff_stat_msg}{epic_close_msg}{commit_nudge_msg}{auto_unblock_msg}"
    )
}

#[cfg(test)]
mod parent_branch_resolver_tests {
    //! cas-7efe: unit tests for the single close-time parent-branch
    //! resolution policy, and cas-e093 tests for the success-message
    //! ordering that makes the "Closed task" confirmation survive
    //! truncation/spilling.
    use super::*;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_committed_repo(branch: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", branch]);
        std::fs::write(dir.path().join("seed"), "seed").unwrap();
        git(dir.path(), &["add", "seed"]);
        git(dir.path(), &["commit", "-q", "-m", "seed"]);
        dir
    }

    #[test]
    fn worktree_store_parent_wins_over_epic_branch() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = resolve_close_parent_branch(
            Some("staging".to_string()),
            Some("epic/other".to_string()),
            dir.path(),
        )
        .expect("explicit worktree branch resolves without git fallback");
        assert_eq!(
            resolved, "staging",
            "the most specific source (worktree store) must win"
        );
    }

    #[test]
    fn epic_branch_wins_over_standalone_fallback_when_worktree_unset() {
        // Reproduces the ZERO-COMMIT catch-22 shape at the policy level:
        // System-B factory workers never set `task.worktree_id`, so the
        // worktree-store tier is always `None` for them. The resolver
        // must still prefer the real epic branch over guessing "main".
        let dir = tempfile::tempdir().unwrap();
        let resolved =
            resolve_close_parent_branch(None, Some("epic/staging-thing".to_string()), dir.path())
                .expect("explicit epic branch resolves without git fallback");
        assert_eq!(
            resolved, "epic/staging-thing",
            "must never fall through to a bare 'main' literal when the \
             epic branch is known"
        );
    }

    #[test]
    fn falls_back_to_standalone_merge_target_when_both_unset() {
        // Neither the worktree store nor a parent epic resolved (a
        // standalone task with no epic) — falls back to
        // `resolve_standalone_merge_target`, which is a real git-detected
        // answer, not a blind guess. A repo whose default is the legacy
        // `master` name proves both supported conventions work.
        let dir = init_committed_repo("master");
        let resolved = resolve_close_parent_branch(None, None, dir.path())
            .expect("master must be detected as the default branch");
        assert_eq!(
            resolved, "master",
            "final tier must reflect the repo's real detected default, \
             never a hardcoded 'main'"
        );
    }

    #[test]
    fn repo_root_resolves_when_cas_root_is_nested_below_repo() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        let cas_root = dir.path().join("state/runtime/.cas");
        std::fs::create_dir_all(&cas_root).unwrap();

        let resolved = resolve_close_gate_repo_root(&cas_root).expect("ancestor repo must resolve");
        assert_eq!(resolved, dir.path().canonicalize().unwrap());
    }

    #[test]
    fn repo_root_resolution_fails_loud_outside_git() {
        let dir = tempfile::tempdir().unwrap();
        let cas_root = dir.path().join("state/.cas");
        std::fs::create_dir_all(&cas_root).unwrap();

        let error = resolve_close_gate_repo_root(&cas_root).expect_err("missing repo must reject");
        assert!(error.contains("CLOSE GATE GIT REPOSITORY ERROR"));
        assert!(error.contains(&cas_root.display().to_string()));
    }

    #[test]
    fn detached_head_still_resolves_existing_main() {
        let dir = init_committed_repo("main");
        git(dir.path(), &["checkout", "-q", "--detach"]);

        assert_eq!(
            resolve_close_gate_default_branch(dir.path())
                .expect("existing main must resolve while HEAD is detached"),
            "main"
        );
    }

    #[test]
    fn detached_head_without_known_default_fails_closed() {
        let dir = init_committed_repo("topic");
        git(dir.path(), &["checkout", "-q", "--detach"]);

        let error = resolve_close_gate_default_branch(dir.path())
            .expect_err("ambiguous detached HEAD must reject");
        assert!(error.contains("CLOSE GATE DEFAULT BRANCH ERROR"));
        assert!(error.contains("neither local `main` nor `master` exists"));
    }

    // ── cas-e093: success message ordering ──────────────────────────────────

    #[test]
    fn success_message_puts_closed_task_line_first() {
        // Even with every optional suffix populated (the worst case that
        // used to spill ~110KB via the diff stat), the confirmation must
        // be the literal first text of the result.
        let msg = format_close_success_message(
            "cas-e093",
            "Bound the close-time diff stat",
            " (verified)",
            " (lease released)",
            "\n🌳 Worktree merged (branch: factory/worker)",
            "\n\n📊 Committed diff stat (vs epic/foo):\n a.txt | 1 +",
            "\n\n🎉 All subtasks complete!",
            "\n\n💡 Consider committing your changes.",
            "\n\n🔓 Auto-unblocked task(s): cas-xyz",
        );
        assert!(
            msg.starts_with("Closed task: cas-e093 - Bound the close-time diff stat"),
            "success confirmation must be first, unconditionally; got: {msg}"
        );
        // Sanity: every suffix is still present, just not first.
        for fragment in [
            "(verified)",
            "lease released",
            "Worktree merged",
            "Committed diff stat",
            "subtasks complete",
            "Consider committing",
            "Auto-unblocked",
        ] {
            assert!(msg.contains(fragment), "must retain {fragment}: {msg}");
        }
    }

    #[test]
    fn success_message_with_all_suffixes_empty_is_just_the_confirmation() {
        let msg = format_close_success_message("cas-1234", "Some task", "", "", "", "", "", "", "");
        assert_eq!(msg, "Closed task: cas-1234 - Some task");
    }
}

// ---------------------------------------------------------------------------
// cas-4b3f: System B (factory-isolation) worktree resolution
// ---------------------------------------------------------------------------

/// Resolve the filesystem path of a "System B" factory-isolation worktree
/// for `assignee`, if one exists on disk.
///
/// `spawn_workers isolate=true` places every worker at the fixed
/// convention `<cas_root>/worktrees/<assignee>` on branch
/// `factory/<assignee>` (see `cas-cli/src/store/detect.rs`,
/// `cas-cli/src/mcp/tools/core/workflow/worktree_ops.rs:154-156`). That
/// directory is never registered in the `WorktreeStore` (System A) —
/// it's created directly by the worktree manager and nothing writes
/// `task.worktree_id` for non-epic tasks. This helper is the pure,
/// directly-testable core of that lookup; [`CasCore::resolve_worker_worktree_path`]
/// wires it in as a fallback when System A doesn't resolve.
///
/// Returns `None` when the path doesn't exist or isn't a git worktree
/// (no `.git` entry) — the same graceful-pass posture as every other
/// git-backed check in this module: an unknowable worktree is not
/// treated as a false positive.
///
/// cas-cf64 (P3, path-traversal hardening): `assignee` is validated as a
/// single, safe path COMPONENT before ever being joined onto a filesystem
/// path. Without this, an `assignee` like `"../.."` would let
/// `cas_root.join("worktrees").join(assignee)` escape `worktrees/` entirely
/// — e.g. resolving all the way back up to the MAIN repo checkout (which
/// has its own `.git`), reintroducing the exact cas-895d "reject every
/// close on unrelated dirty state" bug this whole System-B resolution path
/// exists to fix.
///
/// cas-cf64 (P3, configurable layout): the base directory is resolved via
/// [`system_b_worktree_base`], which honors a configured
/// `[worktrees] base_path` override instead of always hardcoding
/// `<cas_root>/worktrees`.
pub(crate) fn resolve_system_b_worktree_path(
    cas_root: &std::path::Path,
    assignee: &str,
) -> Option<std::path::PathBuf> {
    if !is_safe_path_component(assignee) {
        return None;
    }
    let path = system_b_worktree_base(cas_root).join(assignee);
    if path.join(".git").exists() {
        Some(path)
    } else {
        None
    }
}

fn resolve_system_b_worktree_path_for_repo(
    cas_root: &std::path::Path,
    repo_root: &std::path::Path,
    assignee: &str,
) -> Option<std::path::PathBuf> {
    if !is_safe_path_component(assignee) {
        return None;
    }
    let path = system_b_worktree_base_for_repo(cas_root, repo_root).join(assignee);
    path.join(".git").exists().then_some(path)
}

fn validate_pre_close_worktree(
    path: &std::path::Path,
    expected: &crate::mcp::tools::core::task::repo_context::RepoContext,
    expected_branch: Option<&str>,
) -> Result<(), String> {
    let actual = crate::mcp::tools::core::task::repo_context::resolve_path_context(
        path,
        &expected.target_branch,
    )
    .map_err(|reason| {
        format!("PRE-CLOSE HOOK CONTEXT REJECTED: cannot resolve the task-owned worktree: {reason}")
    })?;
    if actual.repo_selector != expected.repo_selector
        || actual.git_common_dir != expected.git_common_dir
    {
        return Err(format!(
            "PRE-CLOSE HOOK CONTEXT REJECTED: task targets repository `{}`, but its recorded \
             worktree resolves to `{}`. No close-time executable gate was run.",
            expected.repo_selector, actual.repo_selector
        ));
    }
    if let Some(expected_branch) = expected_branch {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["branch", "--show-current"])
            .output()
            .map_err(|error| {
                format!(
                    "PRE-CLOSE HOOK CONTEXT REJECTED: cannot inspect task worktree branch: {error}"
                )
            })?;
        let actual_branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() || actual_branch != expected_branch {
            return Err(format!(
                "PRE-CLOSE HOOK CONTEXT REJECTED: expected task worktree branch \
                 `{expected_branch}`, found `{actual_branch}`. No close-time executable gate was run."
            ));
        }
    }
    Ok(())
}

/// cas-cf64 (P3, path-traversal hardening): `true` when `name` is safe to
/// use as a single filesystem path COMPONENT (not a full path) — i.e. it
/// cannot escape the directory it's joined onto. Rejects empty strings,
/// `.`/`..`, and any path separator (`/` or `\`, so this is safe on both
/// Unix and Windows layouts regardless of which platform actually runs
/// the check).
fn is_safe_path_component(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')
}

/// Default `[worktrees] base_path` template, mirrored from
/// `WorktreeConfig`/`WorktreesConfig`'s own default
/// (`cas-cli/src/worktree/manager/mod.rs`, `cas-cli/src/config/runtime.rs`).
/// Used to detect "no override configured" so the common case resolves via
/// the simple, unchanged `<cas_root>/worktrees` convention rather than
/// re-deriving an equivalent (but not test-fixture-friendly) path through
/// the full `{project}` + repo-root-parent formula.
const DEFAULT_WORKTREE_BASE_PATH_TEMPLATE: &str = "{project}/.cas/worktrees";

/// cas-cf64 (P3, configurable worktree layout): resolve the System-B
/// worktree base directory, honoring a configured `[worktrees] base_path`
/// override.
///
/// `resolve_system_b_worktree_path` previously hardcoded
/// `<cas_root>/worktrees` unconditionally — but that's only ONE of the two
/// places `base_path` is allowed to point. `WorktreeManager::worktree_root()`
/// (the code that actually CREATES these directories for
/// `spawn_workers isolate=true`) resolves the same config-driven
/// `[worktrees] base_path` (`{project}` placeholder, absolute-or-relative-
/// to-the-repo-root's-parent) — a customized base path (e.g. a sibling
/// directory outside `.cas` entirely) would silently no-op every close
/// gate that depends on finding the worker's real worktree, exactly like
/// the original System-A/System-B gap cas-4b3f fixed, just triggered by a
/// config choice instead of a code path.
///
/// When no override is configured (the overwhelmingly common case —
/// `[worktrees]` absent, or `base_path` left at its default), this
/// resolves to the SAME `<cas_root>/worktrees` path as before — zero
/// behavior change and no dependency on `cas_root` looking like a real
/// `<repo>/.cas` directory (existing tests construct `cas_root` as an
/// arbitrary tempdir with no real repo structure above it).
fn system_b_worktree_base(cas_root: &std::path::Path) -> std::path::PathBuf {
    let configured_base_path = crate::config::Config::load(cas_root)
        .ok()
        .map(|c| c.worktrees().base_path);
    match configured_base_path {
        Some(base_path_template) if base_path_template != DEFAULT_WORKTREE_BASE_PATH_TEMPLATE => {
            let repo_root = cas_root.parent().unwrap_or(cas_root);
            let project_name = repo_root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project");
            let base = base_path_template.replace("{project}", project_name);
            if base.starts_with('/') {
                std::path::PathBuf::from(base)
            } else {
                repo_root.parent().unwrap_or(repo_root).join(base)
            }
        }
        _ => cas_root.join("worktrees"),
    }
}

fn system_b_worktree_base_for_repo(
    cas_root: &std::path::Path,
    repo_root: &std::path::Path,
) -> std::path::PathBuf {
    let configured_base_path = crate::config::Config::load(cas_root)
        .ok()
        .map(|c| c.worktrees().base_path);
    match configured_base_path {
        Some(base_path_template) if base_path_template != DEFAULT_WORKTREE_BASE_PATH_TEMPLATE => {
            let project_name = repo_root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project");
            let base = base_path_template.replace("{project}", project_name);
            if base.starts_with('/') {
                std::path::PathBuf::from(base)
            } else {
                repo_root.parent().unwrap_or(repo_root).join(base)
            }
        }
        _ => repo_root.join(".cas").join("worktrees"),
    }
}

// ---------------------------------------------------------------------------
// cas-490f: commit-claim integrity helpers
// ---------------------------------------------------------------------------

/// Count commits reachable from `HEAD` but not from `parent_branch`,
/// running `git` inside `worker_worktree_path`.
///
/// Used by [`check_commit_claim_integrity`] to detect workers that submit
/// `code_review_findings` (claiming code was written) when their branch
/// carries no commits beyond the parent. The key difference from
/// [`count_unmerged_factory_commits`] is that this function operates on
/// `HEAD` — it is meant to be called from inside the worker's own
/// worktree, where `HEAD` IS the worker branch.
///
/// Returns 0 on any git failure (graceful degradation — an unknowable
/// history is not treated as fabrication evidence).
pub(crate) fn count_worker_branch_commits(
    worker_worktree_path: &std::path::Path,
    parent_branch: &str,
) -> u32 {
    use std::process::Command;

    let merge_base_out = Command::new("git")
        .args(["merge-base", "HEAD", parent_branch])
        .current_dir(worker_worktree_path)
        .output();
    let merge_base = match merge_base_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return 0,
    };
    if merge_base.is_empty() {
        return 0;
    }

    let count_out = Command::new("git")
        .args(["rev-list", "--count", &format!("{merge_base}..HEAD")])
        .current_dir(worker_worktree_path)
        .output();
    match count_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<u32>()
            .unwrap_or(u32::MAX),
        _ => 0,
    }
}

/// cas-e093: cap on the number of files listed in the close-time diff
/// stat. Even a legitimately huge task must not overflow the MCP
/// tool-result token limit — six closes in one session each spilled
/// ~110KB (BUG-task-close-returns-110kb-diffstat-overflowing-token-limit.md),
/// forcing an extra shell call every time just to confirm the close
/// (whose success line was buried at the top of the spill file) actually
/// landed.
const DIFF_STAT_MAX_FILES: usize = 40;

/// Return a `git diff --stat` summary for commits on `HEAD` beyond
/// `parent_branch`, running inside `worker_worktree_path`.
///
/// Included verbatim in the `task.close` success message so supervisors
/// see the actual code delta without inspecting the worktree manually.
///
/// Returns an empty string on any git failure or when there are no commits
/// beyond the parent (an empty stat is the correct representation there).
///
/// cas-e093: bounded to [`DIFF_STAT_MAX_FILES`] files via git's own
/// `--stat-count`, so git does the truncation work rather than this
/// function reading an unbounded stat into memory first. When the diff is
/// wider than the cap, the truncated file list gets an explicit
/// "… and M more files" line (derived from git's own trailing "N files
/// changed" summary) in place of git's bare "..." marker.
pub(crate) fn get_worker_diff_stat(
    worker_worktree_path: &std::path::Path,
    parent_branch: &str,
) -> String {
    use std::process::Command;

    let merge_base_out = Command::new("git")
        .args(["merge-base", "HEAD", parent_branch])
        .current_dir(worker_worktree_path)
        .output();
    let merge_base = match merge_base_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return String::new(),
    };
    if merge_base.is_empty() {
        return String::new();
    }

    let stat_out = Command::new("git")
        .args([
            "diff",
            "--stat",
            &format!("--stat-count={DIFF_STAT_MAX_FILES}"),
            &format!("{merge_base}..HEAD"),
        ])
        .current_dir(worker_worktree_path)
        .output();
    let raw = match stat_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return String::new(),
    };
    cap_diff_stat_output(&raw, DIFF_STAT_MAX_FILES)
}

/// Post-process a `git diff --stat --stat-count=<max_files>` result.
///
/// When the true file count (parsed from git's own trailing "N files
/// changed" summary line) exceeds `max_files`, git has already truncated
/// the file listing and left a bare "..." marker line — replace that with
/// an explicit "… and M more files" count. Below the cap, or if the
/// summary line can't be parsed (unexpected git output shape), returns
/// `raw` unchanged.
fn cap_diff_stat_output(raw: &str, max_files: usize) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let Some((summary_line, body_lines)) = lines.split_last() else {
        return raw.to_string();
    };
    let Some(total_files) = parse_files_changed(summary_line) else {
        return raw.to_string();
    };
    if total_files <= max_files {
        return raw.to_string();
    }
    let more = total_files - max_files;
    let body: Vec<&str> = body_lines
        .iter()
        .copied()
        .filter(|line| line.trim() != "...")
        .collect();
    format!(
        "{}\n … and {more} more files\n{summary_line}",
        body.join("\n")
    )
}

/// Parse the leading file count from a `git diff --stat` summary line,
/// e.g. " 1700 files changed, 42 insertions(+)" -> `Some(1700)`.
fn parse_files_changed(summary_line: &str) -> Option<usize> {
    summary_line.trim().split_whitespace().next()?.parse().ok()
}

/// Outcome of the cas-490f commit-claim integrity gate.
#[derive(Debug)]
pub(crate) enum CommitClaimGateOutcome {
    /// Close may proceed — either no `code_review_findings` was provided,
    /// or the worker branch has at least one commit to back up the claim.
    Proceed,
    /// Close may proceed because a worker-supplied receipt was validated
    /// against the current task work cycle. Carries the audit-note body.
    ProceedWithReceipt(String),
    /// Close must be rejected — worker provided `code_review_findings`
    /// (claiming code was written and reviewed) but the branch has 0
    /// commits beyond the parent (fabrication signal).
    Reject(String),
}

/// cas-490f: verify that a worker who claims code changes actually produced
/// commits.
///
/// When `has_review_findings` is true, calls
/// [`count_worker_branch_commits`] inside `worker_worktree_path`. If the
/// count is 0, an integrated automatic anchor or validated task commit
/// receipt satisfies the claim; otherwise returns `Reject` with an explicit
/// "FABRICATION DETECTED" or invalid-receipt message. A positive branch count
/// returns `Proceed`.
///
/// Graceful degradation: `count_worker_branch_commits` returns 0 on git
/// failures. Callers must only invoke this gate when a resolved worktree
/// path is available (`resolve_worker_worktree_path` returned `Some`), so
/// the path is always a real git worktree in production. Test helpers
/// supply a minimal in-memory repo.
pub(crate) fn check_commit_claim_integrity(
    worker_worktree_path: &std::path::Path,
    parent_branch: &str,
    has_review_findings: bool,
    factory_branch_anchor: Option<&str>,
    commit_receipt: Option<&str>,
    commit_receipt_window: Option<&TaskCommitReceiptWindow>,
) -> CommitClaimGateOutcome {
    if !has_review_findings {
        return CommitClaimGateOutcome::Proceed;
    }
    let commit_count = count_worker_branch_commits(worker_worktree_path, parent_branch);
    if commit_count == 0 {
        // cas-127f: after MERGE REQUIRED + supervisor merge, merge-base..HEAD
        // is empty even though real work (the parked anchor) landed on parent.
        // That is not fabrication — it is merge-satisfied.
        if let Some(anchor) = factory_branch_anchor {
            if commit_is_merged_into_parent(worker_worktree_path, anchor, parent_branch) {
                return CommitClaimGateOutcome::Proceed;
            }
        }
        if let Some(receipt) = commit_receipt {
            let Some(window) = commit_receipt_window else {
                return CommitClaimGateOutcome::Reject(commit_receipt_rejection(
                    receipt,
                    parent_branch,
                    "task attribution window is unavailable; ask the supervisor for an audited bypass",
                ));
            };
            return match validate_task_commit_receipt(
                worker_worktree_path,
                receipt,
                parent_branch,
                window,
            ) {
                Ok(note) => CommitClaimGateOutcome::ProceedWithReceipt(note),
                Err(reason) => CommitClaimGateOutcome::Reject(commit_receipt_rejection(
                    receipt,
                    parent_branch,
                    &reason,
                )),
            };
        }
        CommitClaimGateOutcome::Reject(format!(
            "⚠️ FABRICATION DETECTED\n\n\
            task close rejected: code_review_findings was provided (indicating \
            code was written and reviewed) but the worker branch has 0 commits \
            beyond {parent_branch}.\n\n\
            📂 Worker worktree: {}\n\
            🌿 Parent branch: {parent_branch}\n\
            📊 Commits beyond base: 0 commits\n\n\
            Do not submit fabricated code review findings. If no code was written, \
            close without code_review_findings.\n\n\
            To resolve:\n\
            1. If you wrote code but forgot to commit: stage and commit your \
               changes, then retry close.\n\
            2. If no code was needed for this task: retry close without the \
               code_review_findings field (documentation/spike tasks don't \
               need a review envelope).",
            worker_worktree_path.display()
        ))
    } else {
        CommitClaimGateOutcome::Proceed
    }
}

// ---------------------------------------------------------------------------
// cas-8f8f: epic-close per-child merge-state gate + diagnostic
// ---------------------------------------------------------------------------

/// One row in the epic_status diagnostic / epic-close gate report.
///
// ---------------------------------------------------------------------------
// cas-ee2b: zero-commit close gate (case 3)
// ---------------------------------------------------------------------------

/// Outcome of the cas-ee2b zero-commit ambiguity gate (case 3).
#[derive(Debug)]
pub(crate) enum ZeroCommitCloseOutcome {
    /// Close may proceed — either the task is not a code-expecting type,
    /// has an execution_note signalling intentional no-code work, has
    /// committed docs-only changes (count > 0), or the review findings
    /// claim is present (handled by the cas-490f gate instead).
    Proceed,
    /// Close may proceed because a worker-supplied receipt was validated
    /// against the current task work cycle. Carries the audit-note body.
    ProceedWithReceipt(String),
    /// Close rejected — ambiguous zero-commit close on a code task with
    /// no execution_note. Carries the user-facing rejection message.
    AmbiguousCodeTask(String),
}

const COMMIT_RECEIPT_CLOCK_SKEW_SECS: i64 = 5;

/// Durable lower bound used to attribute a receipt to one task work cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskCommitReceiptWindow {
    pub not_before: chrono::DateTime<chrono::Utc>,
    pub basis: &'static str,
    /// cas-9596 (GH #82): lower bound of the task's ENTIRE life, not just the
    /// current cycle. A restart moves `not_before` forward without producing
    /// any commits; work from an earlier cycle of the same task still sits
    /// above this floor and must remain valid evidence.
    pub task_floor: chrono::DateTime<chrono::Utc>,
    /// Evidence used to recognize an earlier cycle's commit as this task's own.
    pub identity: TaskCommitIdentity,
}

/// Prefer the most recent claim/transfer (the current work cycle), falling
/// back to task creation when lease history is unavailable.
pub(crate) fn resolve_task_commit_receipt_window(
    task_created_at: chrono::DateTime<chrono::Utc>,
    lease_history: &[cas_store::LeaseHistoryEntry],
    identity: TaskCommitIdentity,
) -> TaskCommitReceiptWindow {
    let cycle_start = lease_history
        .iter()
        .filter(|entry| matches!(entry.event_type.as_str(), "claimed" | "transferred"))
        .map(|entry| entry.timestamp)
        .max();
    match cycle_start {
        Some(timestamp) if timestamp > task_created_at => TaskCommitReceiptWindow {
            not_before: timestamp,
            basis: "latest task lease claim/transfer",
            task_floor: task_created_at,
            identity,
        },
        _ => TaskCommitReceiptWindow {
            not_before: task_created_at,
            basis: "task creation time (lease-history fallback)",
            task_floor: task_created_at,
            identity,
        },
    }
}

/// cas-127f: true when `commit_ish` is an ancestor of `parent_branch`
/// (or `origin/<parent_branch>` when that ref exists) inside `repo_path`.
///
/// Used to distinguish "never committed" from "committed, MERGE REQUIRED
/// parked an anchor, supervisor already integrated those commits" — after
/// a successful merge, `count_worker_branch_commits` is 0 even though real
/// work landed on the parent. Fail closed on any git error.
pub(crate) fn commit_is_merged_into_parent(
    repo_path: &std::path::Path,
    commit_ish: &str,
    parent_branch: &str,
) -> bool {
    use std::process::Command;
    if commit_ish.is_empty() || !is_safe_git_refname(parent_branch) {
        return false;
    }
    // Reject option-injection on the commit-ish too (full SHAs are fine;
    // leading `-` is not a valid commit ref for our purposes).
    if commit_ish.starts_with('-') {
        return false;
    }
    let check = |parent: &str| -> bool {
        Command::new("git")
            .args(["merge-base", "--is-ancestor", commit_ish, parent])
            .current_dir(repo_path)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if check(parent_branch) {
        return true;
    }
    // Mirror cas-38e2: local parent ref can lag origin after a remote merge.
    let origin_parent = format!("origin/{parent_branch}");
    if git_ref_exists(repo_path, &origin_parent) {
        return check(&origin_parent);
    }
    false
}

/// Validate a worker-supplied task commit receipt.
///
/// The receipt accepts the hexadecimal abbreviations Git users normally copy,
/// but normalizes them to one full immutable commit ID before validation. The
/// object must be a commit with a non-empty merge-aware file diff, the
/// committer timestamp must fall inside the current task work cycle, and the
/// commit must already be reachable from the resolved parent branch (local or
/// origin). This is evidence for the merge-before-close case only; it does not
/// mutate the task's durable commit-time anchor.
fn resolve_task_commit_receipt_sha(
    repo_path: &std::path::Path,
    receipt: &str,
) -> Result<String, String> {
    use std::process::Command;

    let receipt = receipt.trim();
    if !(4..=64).contains(&receipt.len()) || !receipt.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "receipt format invalid: expected 4 to 64 hexadecimal characters; received {} characters",
            receipt.len()
        ));
    }

    // Caller input is restricted to ASCII hex before it reaches Git. Resolve
    // the submitted object first so type errors remain distinguishable from
    // unknown/ambiguous abbreviations.
    let resolved = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", receipt])
        .current_dir(repo_path)
        .output()
        .map_err(|error| format!("failed to resolve the commit receipt: {error}"))?;
    if !resolved.status.success() {
        return Err(format!(
            "receipt resolution failed: hexadecimal value `{receipt}` does not uniquely resolve to a commit (it is unknown or ambiguous)"
        ));
    }
    let full_object = String::from_utf8_lossy(&resolved.stdout).trim().to_string();
    if !matches!(full_object.len(), 40 | 64)
        || !full_object
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err("git returned an invalid full commit object ID".to_string());
    }
    if !full_object
        .to_ascii_lowercase()
        .starts_with(&receipt.to_ascii_lowercase())
    {
        return Err(format!(
            "receipt resolution failed: hexadecimal value `{receipt}` resolved as a ref name rather than an object-ID prefix"
        ));
    }

    // rev-parse's peel intentionally follows annotated tags. A receipt is
    // stricter: its submitted object itself must be a commit, never a tag or
    // tree whose meaning depends on an extra dereference step.
    let object_type = Command::new("git")
        .args(["cat-file", "-t", &full_object])
        .current_dir(repo_path)
        .output()
        .map_err(|error| format!("failed to inspect the receipt object type: {error}"))?;
    if !object_type.status.success() {
        return Err(format!(
            "receipt resolution failed: hexadecimal value `{receipt}` does not uniquely resolve to a commit (it is unknown or ambiguous)"
        ));
    }
    let object_type = String::from_utf8_lossy(&object_type.stdout)
        .trim()
        .to_string();
    if object_type != "commit" {
        return Err(format!(
            "receipt object type invalid: `{receipt}` resolves to a {object_type} object, not a commit"
        ));
    }

    let commit_object = format!("{full_object}^{{commit}}");
    let commit = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", &commit_object])
        .current_dir(repo_path)
        .output()
        .map_err(|error| format!("failed to peel the receipt commit: {error}"))?;
    if !commit.status.success() {
        return Err(format!(
            "receipt resolution failed: `{receipt}` could not be peeled to a commit"
        ));
    }
    let full_commit = String::from_utf8_lossy(&commit.stdout).trim().to_string();
    if full_commit != full_object {
        return Err("git resolved the receipt to a different commit object".to_string());
    }

    Ok(full_commit)
}

pub(crate) fn validate_task_commit_receipt(
    repo_path: &std::path::Path,
    receipt: &str,
    parent_branch: &str,
    window: &TaskCommitReceiptWindow,
) -> Result<String, String> {
    use std::process::Command;

    let receipt = receipt.trim();
    let full_receipt = resolve_task_commit_receipt_sha(repo_path, receipt)?;

    if !commit_is_merged_into_parent(repo_path, &full_receipt, parent_branch) {
        return Err(format!(
            "the commit is not an ancestor of {parent_branch} or origin/{parent_branch}"
        ));
    }

    let commit_epoch_output = Command::new("git")
        .args(["show", "-s", "--format=%ct", &full_receipt, "--"])
        .current_dir(repo_path)
        .output()
        .map_err(|error| format!("failed to inspect the commit timestamp: {error}"))?;
    if !commit_epoch_output.status.success() {
        return Err("git could not inspect the commit timestamp".to_string());
    }
    let commit_epoch = String::from_utf8_lossy(&commit_epoch_output.stdout)
        .trim()
        .parse::<i64>()
        .map_err(|_| "git returned an invalid commit timestamp".to_string())?;
    // cas-9596 (GH #82 step 6): an administrative restart — supervisor clears a
    // note, worker re-`start`s, no new commits — moves the work-cycle bound past
    // a delivery that is already merged and ancestry-verified above. That
    // receipt is still this task's own work, so the cycle bound yields when the
    // commit is attributable to the task and postdates the task itself. A
    // foreign commit, or one older than the task, is still refused.
    let earliest_allowed = window.not_before.timestamp() - COMMIT_RECEIPT_CLOCK_SKEW_SECS;
    let mut prior_cycle_basis = None;
    if commit_epoch < earliest_allowed {
        let task_floor = window.task_floor.timestamp() - COMMIT_RECEIPT_CLOCK_SKEW_SECS;
        if commit_epoch >= task_floor
            && commit_is_task_attributable(repo_path, &full_receipt, &window.identity)
        {
            prior_cycle_basis = Some(format!(
                " The commit predates the current work cycle beginning {} ({}), but it is \
                 attributable to an earlier work cycle of this task, postdates the task itself \
                 ({}), and is already merged — an administrative restart does not invalidate a \
                 delivery that already landed.",
                window.not_before.to_rfc3339(),
                window.basis,
                window.task_floor.to_rfc3339(),
            ));
        } else {
            return Err(format!(
                "the commit predates this task work cycle (commit epoch {commit_epoch}; \
                 earliest accepted epoch {earliest_allowed}, based on {})",
                window.basis
            ));
        }
    }

    let diff = Command::new("git")
        .args([
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-m",
            &full_receipt,
            "--",
        ])
        .current_dir(repo_path)
        .output()
        .map_err(|error| format!("failed to inspect the commit diff: {error}"))?;
    if !diff.status.success() {
        return Err("git could not inspect the commit diff".to_string());
    }
    if diff.stdout.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Err("the commit carries an empty file diff".to_string());
    }

    Ok(format!(
        "decision: accepted commit_receipt `{receipt}` resolved to full commit `{full_receipt}` \
         as task-attributed merge evidence; \
         commit epoch {commit_epoch} is within the current task work cycle beginning {} \
         (basis: {}; {}s clock-skew allowance), the commit is merged into \
         {parent_branch}/origin/{parent_branch}, and its merge-aware file diff is non-empty.{}",
        window.not_before.to_rfc3339(),
        window.basis,
        COMMIT_RECEIPT_CLOCK_SKEW_SECS,
        prior_cycle_basis.unwrap_or_default()
    ))
}

fn append_close_decision_note(task_store: &dyn cas_store::TaskStore, task: &mut Task, note: &str) {
    if task.notes.contains(note) {
        return;
    }
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
    let formatted = format!("[{timestamp}] {note}");
    task.notes = if task.notes.is_empty() {
        formatted
    } else {
        format!("{}\n\n{formatted}", task.notes)
    };
    task.updated_at = chrono::Utc::now();
    if let Err(error) = task_store.update(task) {
        tracing::warn!(
            task_id = %task.id,
            error = %error,
            "failed to persist close decision note"
        );
    }
}

fn commit_receipt_rejection(receipt: &str, parent_branch: &str, reason: &str) -> String {
    format!(
        "⚠️ INVALID TASK COMMIT RECEIPT\n\n\
         task close rejected: commit_receipt `{receipt}` is not valid merge \
         evidence: {reason}.\n\n\
         A close receipt must be the full SHA or an unambiguous hexadecimal \
         abbreviation of a commit produced by this \
         task, carry a non-empty file diff, and already be an ancestor of \
         {parent_branch} (or origin/{parent_branch}).\n\n\
         To resolve:\n\
         1. Find the task commit with `git log --oneline --all`.\n\
         2. Verify it with `git show --stat <sha>` and \
            `git merge-base --is-ancestor <sha> {parent_branch}`.\n\
         3. Retry close with `commit_receipt=<sha>` (full or an unambiguous abbreviation).\n\
         4. If no commit from this task's current work cycle is available, \
            ask the supervisor to audit the merge and close with \
            `bypass_code_review=true`."
    )
}

/// Resolve a close against the merge evidence the caller supplied, if any.
///
/// Returns `Some(outcome)` when evidence was offered — the anchor proves
/// integration (cas-127f), or the receipt is adjudicated by
/// [`validate_task_commit_receipt`] and either carries the close or is
/// rejected on its own terms. Returns `None` only when NO evidence was
/// offered, leaving the caller's heuristic to decide.
///
/// cas-cab3 (GH #128): this used to be inlined on the zero-commit path only,
/// so the no-diff path rejected merged work that came with a valid receipt —
/// the state a branch is in after the supervisor merges it and the worker
/// syncs with the epic tip. Evidence outranks the heuristic on BOTH paths;
/// one function so they cannot drift apart again.
fn resolve_merge_evidence(
    worker_worktree_path: &std::path::Path,
    parent_branch: &str,
    factory_branch_anchor: Option<&str>,
    commit_receipt: Option<&str>,
    commit_receipt_window: Option<&TaskCommitReceiptWindow>,
) -> Option<ZeroCommitCloseOutcome> {
    // cas-127f: merge-satisfied — parked tip is now on the parent.
    if let Some(anchor) = factory_branch_anchor {
        if commit_is_merged_into_parent(worker_worktree_path, anchor, parent_branch) {
            return Some(ZeroCommitCloseOutcome::Proceed);
        }
    }
    let receipt = commit_receipt?;
    let Some(window) = commit_receipt_window else {
        return Some(ZeroCommitCloseOutcome::AmbiguousCodeTask(
            commit_receipt_rejection(
                receipt,
                parent_branch,
                "task attribution window is unavailable; ask the supervisor for an audited bypass",
            ),
        ));
    };
    Some(
        match validate_task_commit_receipt(worker_worktree_path, receipt, parent_branch, window) {
            Ok(note) => ZeroCommitCloseOutcome::ProceedWithReceipt(note),
            Err(reason) => ZeroCommitCloseOutcome::AmbiguousCodeTask(commit_receipt_rejection(
                receipt,
                parent_branch,
                &reason,
            )),
        },
    )
}

/// cas-ee2b: check whether a zero-commit close is ambiguous and should be
/// rejected.
///
/// This function is the testable core of the cas-ee2b case-3 routing. The
/// caller (`cas_task_close`) invokes it only when `!effective_has_reviewable`,
/// i.e. after the main `has_worker_committed_reviewable_changes` check
/// returned false.
///
/// Decision tree:
///
/// 1. **Docs-only commits** (`count > 0` but no reviewable files): worker
///    committed work; the reviewable-files check correctly returned false.
///    → `Proceed`. This function sees `count > 0` and returns `Proceed`.
///
/// 2. **Deliberate no-code** (`execution_note` is set, OR task type is
///    Spike/Chore/Epic, OR `has_review_findings` is true — the cas-490f
///    gate handles that case):
///    → `Proceed`. No ambiguity.
///
/// 3. **Merge-satisfied** (cas-127f/cas-3d37): `factory_branch_anchor` is set
///    and that SHA is an ancestor of `parent_branch` (work was committed, the
///    PostToolUse hook captured the tip, and the supervisor merged into the
///    epic either before or after the first close), or a `commit_receipt`
///    passes [`validate_task_commit_receipt`]. → `Proceed` /
///    `ProceedWithReceipt`. Without this path, post-merge close
///    false-rejects because the worker tip is no longer *ahead of* parent
///    even though the work landed.
///
///    cas-cab3 (GH #128): this applies whether the branch has 0 commits or
///    only zero-diff sync merges. A supervisor merge followed by a sync with
///    the epic tip produces the latter, and it is what SUCCESS looks like —
///    evidence is consulted before the no-diff heuristic on both shapes.
///
/// 4. **Ambiguous zero-commit** (`count == 0`, no anchor / anchor not
///    integrated, no `execution_note`, task type is Bug/Feature/Task, no
///    review findings):
///    → `AmbiguousCodeTask(msg)`. Ask worker to commit, set `execution_note`,
///    or have the supervisor bypass.
///
/// `has_review_findings`: true when `code_review_findings` was non-empty.
/// When true, the cas-490f gate fires upstream; this function returns `Proceed`
/// so the two gates don't double-reject.
///
/// `factory_branch_anchor`: optional full tip SHA recorded after a successful
/// worker commit, with `park_task_awaiting_merge` as a legacy/fallback capture.
/// Genuine zero-commit tasks never receive an anchor.
///
/// `commit_receipt`: optional full SHA or unambiguous hexadecimal abbreviation
/// supplied on close when no automatic anchor was captured. It is accepted only after
/// [`validate_task_commit_receipt`] proves existence, current-cycle
/// attribution, non-empty merge-aware diff, and ancestry from the parent.
///
/// Returns `Proceed` on any git failure for the count path (graceful
/// degradation — not ambiguous when history is unknowable). Ancestor
/// checks fail closed (unknown integration ≠ merge-satisfied).
pub(crate) fn check_zero_commit_close(
    worker_worktree_path: &std::path::Path,
    parent_branch: &str,
    task_id: &str,
    task_type: &TaskType,
    execution_note: Option<&str>,
    has_review_findings: bool,
    factory_branch_anchor: Option<&str>,
    commit_receipt: Option<&str>,
    commit_receipt_window: Option<&TaskCommitReceiptWindow>,
) -> ZeroCommitCloseOutcome {
    // Not a code-expecting task type → no ambiguity.
    if !matches!(
        task_type,
        TaskType::Bug | TaskType::Feature | TaskType::Task
    ) {
        return ZeroCommitCloseOutcome::Proceed;
    }
    // execution_note is set → worker explicitly signalled the no-code intent.
    if execution_note.is_some() {
        return ZeroCommitCloseOutcome::Proceed;
    }
    // Findings present → cas-490f gate handles; don't double-reject here.
    if has_review_findings {
        return ZeroCommitCloseOutcome::Proceed;
    }
    // Count commits: if > 0, this MAY be case 1 (docs-only) — but it can
    // also be a sync-only merge/fast-forward-forced commit (cas-9eae
    // "sync ≠ work"): `git merge --no-ff <parent>` on a branch with no
    // unique work produces a commit that advances the count while
    // contributing an empty diff. "Did HEAD move / is there a commit" is
    // not sufficient; require an actual non-empty diff vs `parent_branch`.
    let commit_count = count_worker_branch_commits(worker_worktree_path, parent_branch);
    if commit_count > 0 {
        let has_diff = !get_worker_diff_stat(worker_worktree_path, parent_branch)
            .trim()
            .is_empty();
        if has_diff {
            return ZeroCommitCloseOutcome::Proceed;
        }
        // Case 3b: commit(s) exist but the diff vs parent is empty — a
        // sync/merge-only close.
        //
        // cas-cab3 (GH #128): "sync-only" is ALSO the shape of a successful
        // post-merge close. Once the supervisor merges the factory branch and
        // the worker syncs with the epic tip, the branch legitimately holds
        // nothing but a zero-diff merge commit — the work is in the parent,
        // which is where it belongs. Consult the merge evidence (anchor /
        // receipt) FIRST, exactly as the zero-commit path below does; the
        // heuristic only decides the cases evidence cannot.
        if let Some(outcome) = resolve_merge_evidence(
            worker_worktree_path,
            parent_branch,
            factory_branch_anchor,
            commit_receipt,
            commit_receipt_window,
        ) {
            return outcome;
        }
        let task_type_str = format!("{task_type:?}").to_lowercase();
        let wt_display = worker_worktree_path.display();
        return ZeroCommitCloseOutcome::AmbiguousCodeTask(format!(
            "⚠️ NO-DIFF CLOSE ON CODE TASK\n\n\
            task close rejected: this is a {task_type_str} task with no \
            code_review_findings, no execution_note, no merge evidence, and \
            {commit_count} commit(s) on the worker branch that produce an \
            EMPTY diff vs {parent_branch} (a sync/merge-only commit, e.g. \
            `git merge --no-ff` with no unique work, not task work). That \
            combination is ambiguous — either the work wasn't committed yet, \
            or this task was resolved without code.\n\n\
            📂 Worker worktree: {wt_display}\n\
            📊 Commits on branch: {commit_count} (zero-diff vs {parent_branch})\n\n\
            To resolve:\n\
            1. If you wrote code but forgot to commit: stage and commit your \
               changes, then retry close.\n\
            2. If the supervisor already merged this task's work and you then \
               synced this branch with {parent_branch}, that is exactly what \
               a finished task looks like — do NOT reset or force-push the \
               branch. Find the SHA of the worker task commit OR the merge \
               commit that carried this task's work (never an unrelated \
               historical commit), verify it with `git show --stat <sha>` and \
               `git merge-base --is-ancestor <sha> {parent_branch}`, then \
               retry close with `commit_receipt=<sha>` (full or an \
               unambiguous abbreviation).\n\
            3. If this task was resolved without code (fixed by a sibling task, \
               docs-only, characterization-only): update the task with an \
               execution_note to signal intentional no-code work:\n\
               `mcp__cas__task action=update id={task_id} execution_note=additive-only`\n\
            4. Supervisors may bypass this gate with bypass_code_review=true \
               (logged as a decision note)."
        ));
    }
    if let Some(outcome) = resolve_merge_evidence(
        worker_worktree_path,
        parent_branch,
        factory_branch_anchor,
        commit_receipt,
        commit_receipt_window,
    ) {
        return outcome;
    }
    // Case 3: ambiguous zero-commit close.
    let task_type_str = format!("{task_type:?}").to_lowercase();
    let wt_display = worker_worktree_path.display();
    ZeroCommitCloseOutcome::AmbiguousCodeTask(format!(
        "⚠️ ZERO-COMMIT CLOSE ON CODE TASK\n\n\
        task close rejected: this is a {task_type_str} task with no \
        code_review_findings, no execution_note, and 0 commits on the \
        worker branch. That combination is ambiguous — either the work \
        wasn't committed yet, or this task was resolved without code.\n\n\
        📂 Worker worktree: {wt_display}\n\
        📊 Commits on branch: 0\n\n\
        To resolve:\n\
        1. If you wrote code but forgot to commit: stage and commit your \
           changes, then retry close.\n\
        2. If this task was resolved without code (fixed by a sibling task, \
           docs-only, characterization-only): update the task with an \
           execution_note to signal intentional no-code work:\n\
           `mcp__cas__task action=update id={task_id} execution_note=additive-only`\n\
        3. If the supervisor already merged this task's work — including an \
           out-of-band merge after conflict rework cleared the old anchor — \
           find the SHA of the worker task commit OR the merge commit \
           that actually carried this task's work (never an unrelated \
           historical commit), verify it is an ancestor of \
           {parent_branch}, then retry close with \
           `commit_receipt=<sha>` (full or an unambiguous abbreviation).\n\
        4. If no task commit receipt is available, ask the supervisor to \
           audit the merge and close with `bypass_code_review=true`. Only a \
           supervisor can perform that bypass."
    ))
}

/// Captures everything the supervisor needs to see at a glance: which
/// child task this is, who owns it, whether that task's recorded work has
/// stranded commits relative to the parent epic, and (for unmerged rows)
/// when that recorded work was last touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EpicChildBranchStatus {
    pub task_id: String,
    pub task_status: TaskStatus,
    pub assignee: Option<String>,
    /// Task-specific commit receipt recorded when the child was parked/closed.
    pub recorded_anchor: Option<String>,
    /// Primary branch checked when the task-specific anchor is unavailable.
    /// This is the historical parked branch when present, otherwise the
    /// current assignee-derived branch.
    pub factory_branch: Option<String>,
    /// Other branches checked for the same task. A reassigned task can retain
    /// a historical `parked_branch` while its current assignee has live work
    /// on a different factory branch; both must remain visible to the gate.
    pub additional_factory_branches: Vec<String>,
    pub unmerged_count: u32,
    /// Unix epoch seconds of the most recent commit across the checked
    /// commit-ish values. `None` when none resolve or `git log` fails.
    pub last_commit_unix: Option<i64>,
    /// Audit note emitted when a stranded recorded anchor is superseded by
    /// live branch tips that are all known merged into the parent.
    pub merge_evidence_note: Option<String>,
}

impl EpicChildBranchStatus {
    fn factory_branches_label(&self) -> String {
        let branches = self
            .factory_branch
            .iter()
            .chain(self.additional_factory_branches.iter())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        if branches.is_empty() {
            "—".to_string()
        } else {
            branches
        }
    }
}

/// Resolve every distinct live tip for `branch` (local and `origin/`) and
/// classify its merge state without mapping Git failures to zero.
///
/// `None` means neither ref exists. When both refs resolve to the same commit,
/// evaluate that tip once but retain both ref names in the audit summary.
fn live_branch_merge_evidence(
    repo_path: &std::path::Path,
    branch: &str,
    parent_branch: &str,
) -> Option<(KnownUnmergedCount, Vec<String>)> {
    let refnames = [branch.to_string(), format!("origin/{branch}")];
    let mut tips: Vec<(String, String)> = Vec::new();
    for refname in refnames {
        let Some(tip) = resolve_branch_sha(repo_path, &refname) else {
            continue;
        };
        if let Some((names, _)) = tips.iter_mut().find(|(_, known_tip)| *known_tip == tip) {
            names.push_str(&format!(", {refname}"));
        } else {
            tips.push((refname, tip));
        }
    }
    if tips.is_empty() {
        return None;
    }

    let mut max_positive = 0;
    let mut summaries = Vec::new();
    for (refnames, tip) in tips {
        match known_unmerged_factory_commits(repo_path, &tip, parent_branch) {
            KnownUnmergedCount::KnownZero => {}
            KnownUnmergedCount::KnownPositive(count) => {
                max_positive = max_positive.max(count);
            }
            KnownUnmergedCount::Unknown => {
                return Some((KnownUnmergedCount::Unknown, summaries));
            }
        }
        summaries.push(format!("{refnames} tip {tip}"));
    }
    let state = if max_positive == 0 {
        KnownUnmergedCount::KnownZero
    } else {
        KnownUnmergedCount::KnownPositive(max_positive)
    };
    Some((state, summaries))
}

/// Walk an epic's children and report whether each child task's own recorded
/// work is merged into `parent_branch`.
///
/// A parked `factory_branch_anchor` is the authoritative commit-ish. It is
/// task-specific and remains stable when a worker later reuses the same
/// factory branch for another epic. When that anchor is absent or no longer
/// resolves, both the historical `parked_branch` and a distinct current
/// assignee-derived live branch are checked. Taking the maximum unmerged count
/// avoids double-counting shared history while ensuring reassignment cannot
/// hide either worker's stranded commits.
///
/// A resolvable anchor that is not an ancestor of `parent_branch` may be
/// reconciled as superseded only when every recorded/current live branch is
/// present and known fully merged *and* the anchor has task-specific content
/// proof on the parent (an identical tip tree or cherry-equivalent patches).
/// A zero-ahead branch alone is never proof: the branch name may have been
/// recycled or reset after discarding the recorded task's work.
///
/// Used by both:
/// - `factory_epic_status` (read-only diagnostic — renders all rows)
/// - `run_epic_close_merge_gate` (close gate — filters to rows with
///   `unmerged_count > 0`)
///
/// Children without any recorded branch or assignee are still represented in
/// the output so the report is complete; the gate filters only on
/// `unmerged_count > 0` so a valid anchor still blocks even if its branch-name
/// receipt is missing.
pub(crate) fn collect_epic_branch_statuses(
    subtasks: &[Task],
    parent_branch: &str,
    repo_path: &std::path::Path,
) -> Vec<EpicChildBranchStatus> {
    subtasks
        .iter()
        .map(|t| {
            let parked_branch = t.deliverables.parked_branch.clone();
            let live_factory_branch = t
                .assignee
                .as_ref()
                .map(|assignee| format!("factory/{assignee}"));
            let recorded_anchor = t.deliverables.factory_branch_anchor.as_deref();
            let resolved_anchor =
                recorded_anchor.filter(|anchor| git_ref_exists(repo_path, anchor));

            let mut fallback_branches = Vec::new();
            if let Some(branch) = parked_branch.as_ref() {
                fallback_branches.push(branch.clone());
            }
            if let Some(branch) = live_factory_branch.as_ref()
                && !fallback_branches.contains(branch)
            {
                fallback_branches.push(branch.clone());
            }
            let factory_branch = fallback_branches.first().cloned().or(parked_branch);
            let additional_factory_branches: Vec<String> =
                fallback_branches.into_iter().skip(1).collect();

            let fallback_branches = factory_branch
                .iter()
                .chain(additional_factory_branches.iter())
                .cloned()
                .collect::<Vec<_>>();
            let checked_refs = if let Some(anchor) = resolved_anchor {
                vec![anchor]
            } else {
                fallback_branches.iter().map(String::as_str).collect()
            };
            let mut unmerged_count = 0;
            let mut latest_commit_unix = None;
            for commit in checked_refs {
                unmerged_count = unmerged_count.max(count_unmerged_factory_commits(
                    repo_path,
                    commit,
                    parent_branch,
                ));
                latest_commit_unix = latest_commit_unix.max(last_commit_unix(repo_path, commit));
            }
            let mut merge_evidence_note = None;
            if let Some(anchor) = resolved_anchor
                && unmerged_count > 0
            {
                let required_live_branch = live_factory_branch.as_ref().or(factory_branch.as_ref());
                let mut required_branch_is_known = false;
                let mut live_state_is_known = true;
                let mut live_unmerged_count = 0;
                let mut live_summaries = Vec::new();
                for branch in &fallback_branches {
                    match live_branch_merge_evidence(repo_path, branch, parent_branch) {
                        Some((KnownUnmergedCount::KnownZero, summaries)) => {
                            required_branch_is_known |= required_live_branch == Some(branch);
                            live_summaries.extend(summaries);
                        }
                        Some((KnownUnmergedCount::KnownPositive(count), summaries)) => {
                            required_branch_is_known |= required_live_branch == Some(branch);
                            live_unmerged_count = live_unmerged_count.max(count);
                            live_summaries.extend(summaries);
                        }
                        Some((KnownUnmergedCount::Unknown, _)) => {
                            live_state_is_known = false;
                        }
                        // Reassignment must not let a clean current branch
                        // hide a vanished historical parked branch.
                        None => live_state_is_known = false,
                    }
                }
                let anchor_has_task_specific_proof =
                    commit_tip_tree_reachable_from(repo_path, anchor, parent_branch)
                        || commit_patches_cherry_equivalent_on_parent(
                            repo_path,
                            anchor,
                            parent_branch,
                        );
                if required_branch_is_known
                    && live_state_is_known
                    && live_unmerged_count == 0
                    && anchor_has_task_specific_proof
                {
                    unmerged_count = 0;
                    merge_evidence_note = Some(format!(
                        "decision: recorded factory_branch_anchor `{anchor}` for child task `{}` \
                         is not an ancestor of `{parent_branch}`, but its task-specific content \
                         is proven on the parent and the current live branch evidence is fully \
                         merged ({}). Treated the recorded anchor as superseded rather than \
                         requiring history pollution.",
                        t.id,
                        live_summaries.join("; "),
                    ));
                }
            }
            EpicChildBranchStatus {
                task_id: t.id.clone(),
                task_status: t.status,
                assignee: t.assignee.clone(),
                recorded_anchor: recorded_anchor.map(str::to_string),
                factory_branch,
                additional_factory_branches,
                unmerged_count,
                last_commit_unix: latest_commit_unix,
                merge_evidence_note,
            }
        })
        .collect()
}

/// Render the per-child branch statuses as a Markdown report for the
/// supervisor-facing `factory_epic_status` action. Stable shape; the
/// snapshot test in `epic_status_gate_tests` pins the exact layout.
pub(crate) fn render_epic_status_report(
    epic_id: &str,
    parent_branch: &str,
    statuses: &[EpicChildBranchStatus],
) -> String {
    render_epic_status_report_with_stack(epic_id, parent_branch, statuses, &[])
}

/// Render an epic-status report, naming the unlanded epic branches this epic
/// is stacked on (cas-aae6 / GH #110).
///
/// `stacked_on` is trunk-first, so the landing order is that list followed by
/// this epic's own branch. An empty slice renders exactly the pre-existing
/// report, which is why the plain [`render_epic_status_report`] delegates here.
pub(crate) fn render_epic_status_report_with_stack(
    epic_id: &str,
    parent_branch: &str,
    statuses: &[EpicChildBranchStatus],
    stacked_on: &[String],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Epic {epic_id} — factory branch status\n\
         Parent branch: {parent_branch}\n",
    ));
    if !stacked_on.is_empty() {
        let quoted: Vec<String> = stacked_on.iter().map(|b| format!("'{b}'")).collect();
        let mut order = quoted.clone();
        order.push(format!("'{parent_branch}'"));
        // Deliberately does NOT claim each entry contains the previous one:
        // when an epic branch merges two independent unlanded epics, both are
        // ancestors of it while neither contains the other. What is always
        // true — and what the supervisor needs — is that this epic contains
        // all of them, so none can be left behind.
        out.push_str(&format!(
            "Stacked on: {} unlanded epic branch(es) — {}\n\
             Landing order: {} (this epic contains all of them; merging it merges them)\n",
            stacked_on.len(),
            quoted.join(" → "),
            order.join(" → "),
        ));
    }
    out.push('\n');
    if statuses.is_empty() {
        out.push_str("(no child tasks)\n");
        return out;
    }
    out.push_str("| Task | Status | Assignee | Factory branch | Unmerged | Last commit |\n");
    out.push_str("|------|--------|----------|----------------|----------|-------------|\n");
    for s in statuses {
        // Use Display (snake_case: in_progress, closed) rather than
        // Debug (PascalCase: InProgress, Closed) so the supervisor-
        // facing column matches the rest of the CLI's status rendering
        // (e.g., `task list`). Round-1 cas-code-review fix.
        let status_str = s.task_status.to_string();
        let assignee = s.assignee.as_deref().unwrap_or("—");
        let branch = s.factory_branches_label();
        let unmerged = if s.factory_branch.is_some() {
            s.unmerged_count.to_string()
        } else {
            "—".to_string()
        };
        let last_commit = match s.last_commit_unix {
            Some(ts) => format_unix_timestamp(ts),
            None => "—".to_string(),
        };
        out.push_str(&format!(
            "| {task} | {status} | {assignee} | {branch} | {unmerged} | {last} |\n",
            task = s.task_id,
            status = status_str,
            assignee = assignee,
            branch = branch,
            unmerged = unmerged,
            last = last_commit,
        ));
    }
    let reconciled = statuses
        .iter()
        .filter_map(|s| s.merge_evidence_note.as_deref())
        .collect::<Vec<_>>();
    if !reconciled.is_empty() {
        out.push_str("\nℹ️  Recorded/live merge-evidence reconciliation:\n");
        for note in reconciled {
            out.push_str(&format!("- {note}\n"));
        }
    }
    let stranded = statuses.iter().filter(|s| s.unmerged_count > 0).count();
    if stranded > 0 {
        out.push_str(&format!(
            "\n⚠️  {stranded} child task(s) carry stranded factory commits. \
             Epic close will be hard-blocked until they are merged.\n",
        ));
    } else {
        out.push_str("\n✓ All child factory branches are merged into the parent epic branch.\n");
    }
    out
}

/// Format a Unix epoch second as ISO-8601 UTC. Pure helper used only
/// by [`render_epic_status_report`] in this module; tests in the
/// same file can call private helpers directly. (Round-1 cas-code-review
/// fix: was previously `pub(crate)` for no good reason.)
fn format_unix_timestamp(ts: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| format!("ts={ts}"))
}

/// Last-commit timestamp on `branch` (Unix epoch seconds), or `None`
/// when the branch ref doesn't resolve or `git log` fails. Mirrors
/// the shell-out style of [`count_unmerged_factory_commits`].
pub(crate) fn last_commit_unix(repo_path: &std::path::Path, branch: &str) -> Option<i64> {
    use std::process::Command;
    let out = Command::new("git")
        .args(["log", "-1", "--format=%ct", branch])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<i64>()
        .ok()
}

/// Immutable commit ID currently named by `reference`, or `None` when the ref
/// is unsafe, missing, or does not resolve to a commit.
///
/// Director merge-alert classification resolves all movable refs through this
/// helper once, then performs every merge-base/count operation against these
/// immutable IDs so concurrent ref updates cannot produce a mixed snapshot.
pub(crate) fn resolve_ref_commit_sha(
    repo_path: &std::path::Path,
    reference: &str,
) -> Option<String> {
    use std::process::Command;

    if !is_safe_git_refname(reference) {
        return None;
    }

    let commit = format!("{reference}^{{commit}}");
    let out = Command::new("git")
        .args(["rev-parse", "--verify", &commit])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

/// Outcome of the cas-8f8f epic-close per-child merge-state gate.
///
/// Symmetric to [`MergeStateGateOutcome`] but at the epic scope.
/// Bypass-immune by construction (no bypass parameter on the gate).
#[derive(Debug)]
pub(crate) enum EpicCloseGateOutcome {
    Proceed,
    ProceedWithNote(String),
    Reject(String),
}

/// Per-epic close-time guard: reject Epic-task close when ANY child
/// task's own recorded factory anchor is not merged into the epic branch.
///
/// The sole non-ancestry exception is a rewritten/superseded anchor whose
/// task-specific content is proven on the parent by an identical tip tree or
/// cherry-equivalent patches, while all recorded/current live branch tips are
/// present and known fully merged. A recycled or reset-to-parent branch being
/// zero-ahead is not sufficient evidence.
///
/// Runs IN ADDITION to (and AFTER) the existing
/// [`check_unmerged_epic_branches`] which only validates the epic's
/// own branch namespace. cas-8f8f extends the principle from "epic
/// branch" to "every child task's factory work".
///
/// Bypass-immune for the same reasons as cas-95ce
/// [`run_factory_branch_merge_gate`]: this is a data-state guard,
/// not a review gate. `_req` is intentionally unused.
pub(crate) fn run_epic_close_merge_gate(
    task: &Task,
    _req: &TaskCloseRequest,
    parent_branch: &str,
    repo_path: &std::path::Path,
    subtasks: &[Task],
) -> EpicCloseGateOutcome {
    if task.task_type != TaskType::Epic {
        return EpicCloseGateOutcome::Proceed;
    }
    let statuses = collect_epic_branch_statuses(subtasks, parent_branch, repo_path);
    let stranded: Vec<&EpicChildBranchStatus> =
        statuses.iter().filter(|s| s.unmerged_count > 0).collect();
    if stranded.is_empty() {
        let notes = statuses
            .iter()
            .filter_map(|s| s.merge_evidence_note.as_deref())
            .collect::<Vec<_>>();
        if !notes.is_empty() {
            return EpicCloseGateOutcome::ProceedWithNote(notes.join("\n"));
        }
        return EpicCloseGateOutcome::Proceed;
    }
    let mut detail = String::new();
    let mut cleaned_worktree_guidance = String::new();
    for s in &stranded {
        // Round-1 cas-code-review autofix: use idiomatic `writeln!`
        // rather than the explicit `Write::write_fmt(format_args!(...))`
        // desugaring. `writeln!` returns Result; the surrounding
        // String backing cannot fail, so the discard is intentional.
        use std::fmt::Write as _;
        let recorded_anchor = s
            .recorded_anchor
            .as_deref()
            .map(|anchor| format!("; recorded anchor {anchor}"))
            .unwrap_or_default();
        let _ = writeln!(
            detail,
            "  - {task} ({branch}): {n} commit(s) not on {parent}{recorded_anchor}",
            task = s.task_id,
            branch = s.factory_branches_label(),
            n = s.unmerged_count,
            parent = parent_branch,
            recorded_anchor = recorded_anchor,
        );
        for branch in s
            .factory_branch
            .iter()
            .chain(s.additional_factory_branches.iter())
        {
            let _ = writeln!(
                cleaned_worktree_guidance,
                "  - {task}: `git fetch origin {branch}:refs/remotes/origin/{branch}` then \
                 `git merge --no-ff {branch}` (or `origin/{branch}` when only the \
                 remote-tracking ref remains)",
                task = s.task_id,
            );
        }
    }
    EpicCloseGateOutcome::Reject(format!(
        "⚠️ MERGE REQUIRED\n\n\
         Epic {epic_id} cannot close — {n} child task(s) have stranded factory \
         branches:\n{detail}\n\
         Each child's factory branch must be merged into {parent} before the \
         epic can close. This guard cannot be bypassed (use of \
         bypass_code_review=true does not skip merge-state checks — it is a \
         data-state guard, not a review gate).\n\n\
         Remediation when the worker worktree still exists:\n\
         - `mcp__cas__coordination action=worktree_merge id=<factory-branch> \
           task_id=<child-task-id>`\n\n\
         If cleanup already removed the worktree, merge the surviving branch \
         directly from the epic checkout (the branch, not the stale recorded SHA):\n\
         {cleaned_worktree_guidance}\n\
         Diagnostic: run `mcp__cas__coordination action=epic_status id={epic_id}` \
         for a per-child report.",
        epic_id = task.id,
        n = stranded.len(),
        detail = detail,
        cleaned_worktree_guidance = cleaned_worktree_guidance,
        parent = parent_branch,
    ))
}

// ---------------------------------------------------------------------------
// cas-778a + cas-3086 + cas-fef4: clean-envelope predicate and epic bypass
// ---------------------------------------------------------------------------

/// Returns `true` iff `envelope` is a structurally valid, semantically-
/// validated [`cas_types::ReviewOutcome`] with:
///   * no PR-introduced P0 in `residual` (cas-3086), and
///   * no P0 reclassified into `pre_existing` (cas-fef4 forgery defence).
///
/// Used in two call sites:
///   1. The factory-worker-owned verification short-circuit in
///      `cas_task_close` (cas-778a): before arming the verification jail,
///      check whether the worker supplied a clean envelope; if so, write a
///      `Skipped` row and let the close proceed without task-verifier
///      dispatch.
///   2. [`epic_subtask_receipts_are_clean`]: the epic-level bypass that
///      skips the union-diff multi-persona gate when every subtask already
///      holds a clean per-task receipt.
///
/// Keeping both call sites on the same predicate ensures they evolve
/// together — a tightening here (e.g., adding a SHA-anchor staleness
/// check) automatically applies to both.
pub(crate) fn worker_review_envelope_is_clean(envelope: &str) -> bool {
    use cas_store::code_review::close_gate::{GateDecision, evaluate_gate};
    use cas_types::FindingSeverity;

    let Ok(outcome) = serde_json::from_str::<cas_types::ReviewOutcome>(envelope) else {
        return false;
    };
    if outcome.validate().is_err() {
        return false;
    }
    // cas-3086: no PR-introduced P0 in residual.
    // ALSO explicitly reject any P0 in residual regardless of per-finding
    // `pre_existing` flag: `evaluate_gate` skips findings with
    // `pre_existing: true` (they are treated as baseline noise, not
    // PR-introduced), so a forged envelope with a P0 in `residual` that
    // carries `pre_existing: true` would otherwise pass both the gate call
    // and the `pre_existing`-array check below. Genuine pre-existing P0s
    // belong in `outcome.pre_existing[]`, not in `residual[]`.
    // cas-acf83 (GH #108): self-certification requires evidence the review
    // actually ran. Symmetric with run_code_review_gate — a zero-persona
    // envelope must not buy a verification bypass either.
    if !outcome.execution_status().is_executed() {
        return false;
    }
    let residual_clean = matches!(evaluate_gate(&outcome.residual), GateDecision::Allow)
        && !outcome
            .residual
            .iter()
            .any(|f| f.severity == FindingSeverity::P0);
    // cas-fef4: no P0 smuggled through the top-level pre_existing array.
    let pre_existing_clean = outcome
        .pre_existing
        .iter()
        .all(|f| f.severity != FindingSeverity::P0);
    residual_clean && pre_existing_clean
}

/// Decide whether an epic's subtasks collectively carry clean review
/// receipts that justify skipping the multi-persona close gate on the
/// union diff.
///
/// Returns `true` iff every subtask:
///   * has a non-empty `deliverables.review_envelope`,
///   * that passes [`worker_review_envelope_is_clean`] (deserialises,
///     validates, no P0 in residual, no P0 in pre_existing).
///
/// Returns `false` when the subtask list is empty — there is nothing to
/// "cover" the union diff, so fall through to the normal gate.
///
/// ## Why both residual- and pre_existing-P0 disqualify the bypass
///
/// The bypass treats "every subtask has a clean receipt" as a proof
/// stand-in that the union diff was already reviewed piece-by-piece. A
/// worker supplying an envelope of shape `{ residual: [], pre_existing:
/// [<real_p0>] }` would satisfy the old `evaluate_gate(residual) ==
/// Allow` check but smuggle a real P0 past the epic-close gate — the
/// `pre_existing` channel was designed to classify *findings that
/// predate the change*, not as a free downgrade slot for workers to
/// drop P0s into. Per cas-fef4, we tighten the clean-receipt semantics
/// to reject any receipt where a P0 appears anywhere — residual OR
/// pre_existing. Legitimate pre-existing P0s on a change's diff are
/// extraordinarily rare; if one genuinely appears post-hoc, re-running
/// the gate is cheap insurance compared with a silent bypass.
///
/// ## Staleness note
///
/// This helper still treats the persisted envelopes structurally — it
/// cannot detect whether the epic branch has commits *not* covered by
/// any subtask's reviewed diff (supervisor fixups, merge-resolution
/// commits). That is tracked separately (cas-cc1d staleness follow-up)
/// and needs a diff-SHA anchor in the envelope schema to close cleanly.
pub(crate) fn epic_subtask_receipts_are_clean(subtasks: &[Task]) -> bool {
    if subtasks.is_empty() {
        return false;
    }

    subtasks.iter().all(|t| {
        t.deliverables
            .review_envelope
            .as_deref()
            .map(worker_review_envelope_is_clean)
            .unwrap_or(false)
    })
}

// ---------------------------------------------------------------------------
// cas-49f1: zero-hit search-manifest guardrail for investigation (Spike)
// task closes
// ---------------------------------------------------------------------------

/// Outcome of the investigation-task (`Spike`) search-manifest gate
/// (cas-49f1). Unlike [`CodeReviewGateOutcome`] this gate never rejects a
/// close — it is a guardrail, not a framework: the loudest it gets is a
/// warning note appended to the task's audit trail.
#[derive(Debug)]
pub(crate) enum SearchManifestGateOutcome {
    /// Close may proceed. No note to write.
    Proceed,
    /// Close may proceed, but the caller should append this warning note
    /// to the task first — either a malformed manifest, or one or more
    /// search steps that returned zero hits across all inputs.
    AppendWarningNote(String),
}

/// Run the cas-49f1 zero-hit search-manifest guardrail.
///
/// Only fires for `TaskType::Spike` (investigation/research) tasks that
/// supply a non-empty `search_manifest`. Ordinary code tasks, and Spike
/// tasks that omit the field entirely, are untouched — this is
/// deliberately opt-in, not a close-time requirement, per the cas-49f1
/// scope discipline ("do not build a general 'prove your investigation'
/// framework").
///
/// When a manifest is present: any entry reporting `hits == 0` is surfaced
/// as a loud warning note rather than allowed to blend into a "nothing
/// found" close narrative silently. A manifest that fails to parse is
/// itself treated as warning-worthy (the worker claimed to report
/// coverage and the claim is unreadable).
pub(crate) fn run_search_manifest_gate(
    task: &Task,
    req: &TaskCloseRequest,
) -> SearchManifestGateOutcome {
    if task.task_type != TaskType::Spike {
        return SearchManifestGateOutcome::Proceed;
    }
    let raw = match req
        .search_manifest
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(r) => r,
        None => return SearchManifestGateOutcome::Proceed,
    };
    let manifest = match cas_types::parse_search_manifest(raw) {
        Ok(m) => m,
        Err(e) => {
            let note = format!(
                "[{}] WARNING: search_manifest could not be parsed and was \
                 skipped for zero-hit checking: {e}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M"),
            );
            return SearchManifestGateOutcome::AppendWarningNote(note);
        }
    };
    let zero_hits = cas_types::zero_hit_entries(&manifest);
    if zero_hits.is_empty() {
        return SearchManifestGateOutcome::Proceed;
    }
    let commands = zero_hits
        .iter()
        .map(|e| format!("  - `{}` -> 0 hits", e.command))
        .collect::<Vec<_>>()
        .join("\n");
    let note = format!(
        "[{}] ⚠️ ZERO_HIT_SEARCH_WARNING: {} of {} search step(s) in this \
         investigation's search_manifest returned 0 hits across all inputs:\n\
         {commands}\n\n\
         A search that matches nothing anywhere is far more often a broken \
         pattern than a clean corpus (cas-49f1). Verify these patterns \
         against a known-positive input before trusting this close's \
         conclusion.",
        chrono::Utc::now().format("%Y-%m-%d %H:%M"),
        zero_hits.len(),
        manifest.len(),
    );
    SearchManifestGateOutcome::AppendWarningNote(note)
}

// ---------------------------------------------------------------------------
// cas-b39f (Unit 9): cas-code-review P0 close gate
// ---------------------------------------------------------------------------

/// Outcome of the cas-code-review close gate, as seen by `cas_task_close`.
///
/// This enum is deliberately tiny: the hard work (P0 residual evaluation)
/// lives in `cas_store::code_review::close_gate::evaluate_gate`, and the
/// soft conditions (supervisor override, additive-only skip, non-code
/// diff, graceful degradation) are resolved by [`run_code_review_gate`]
/// below. The call site in `cas_task_close` just pattern-matches on the
/// three outcomes.
#[derive(Debug)]
pub(crate) enum CodeReviewGateOutcome {
    /// Close may proceed. No note to write, no error to return.
    Proceed,
    /// Close may proceed, but the caller should append this decision
    /// note to the task before the main close transaction. Used for
    /// the supervisor override path so the audit trail captures who
    /// downgraded a P0 block and why.
    AppendDecisionNote(String),
    /// Close must be rejected with this user-facing error message.
    /// Used for (a) P0 residual blocks, and (b) unauthorized override
    /// attempts.
    Reject(String),
}

/// Decide whether the cas-code-review P0 close gate fires for this
/// close request.
///
/// Per brainstorm Outstanding Question #1 option (a): the worker runs
/// the cas-code-review skill *before* calling `task.close` and passes
/// the structured findings envelope in via
/// [`TaskCloseRequest::code_review_findings`]. This Rust helper only
/// enforces the gate on what the worker sends — it does not (and
/// cannot) invoke the skill itself.
///
/// Contract:
///
/// - `execution_note == "additive-only"` → [`Proceed`]. Pure-addition
///   closes are new-files-only by definition and already covered by
///   the cas-e235 gate above.
/// - `bypass_code_review == Some(true)` and caller is a supervisor →
///   [`AppendDecisionNote`] with the override reason. Gate skipped.
/// - `bypass_code_review == Some(true)` and caller is **not** a
///   supervisor → [`Reject`] with an unauthorized-override message.
///   Silently ignoring the flag would mask a misconfigured harness.
/// - `has_reviewable_changes(project_root) == false` → [`Proceed`].
///   Pure docs-only diffs (`*.md` / `docs/**`) and pure test-only
///   diffs do not require a code review pass.
/// - `code_review_findings == None` at this point → [`Reject`] with
///   `CODE_REVIEW_REQUIRED`, pointing the worker at the skill.
/// - `code_review_findings == Some(envelope)` that fails
///   [`ReviewOutcome::validate`] → [`Reject`] as a malformed envelope.
/// - Otherwise → run the full forgery defence (cas-4c64): Check A
///   rejects any P0 in `residual[]` regardless of the per-finding
///   `pre_existing` flag; Check B rejects any P0 in `pre_existing[]`.
///   Then [`evaluate_gate`] is called as a safety net. Any rejection
///   returns a formatted block message; all checks pass → [`Proceed`].
/// Build the `CODE_REVIEW_REQUIRED` rejection message with mode guidance
/// that matches the configured `[code_review] owner` (cas-297e).
///
/// - `supervisor_owned = true` (default since v2.13.0): recommend
///   `mode=interactive` / `mode=headless`.
/// - `supervisor_owned = false` (`owner = "worker"`): recommend the
///   legacy `mode=autofix` path.
/// cas-acf83 (GH #108): the review reported that it did not run.
///
/// Names the reason (and any persona launch failures the producer recorded)
/// so the worker can tell "the transport is down" from "the diff was empty"
/// without digging through a workflow transcript.
fn format_review_did_not_execute(task_id: &str, reason: &str, supervisor_owned: bool) -> String {
    let rerun = if supervisor_owned {
        "mode=interactive (or mode=headless for skill-to-skill)"
    } else {
        "mode=autofix"
    };
    format!(
        "⚠️ REVIEW DID NOT EXECUTE\n\n         task close rejected for {task_id}: the code_review_findings envelope \
         reports that no persona produced a verdict, so its empty residual[] is \
         an ABSENT verdict, not a passing one.\n\n         Reported reason: {reason}\n\n         To resolve:\n\
         1. Fix what stopped the personas from running (a down or \
            out-of-credit review transport is the usual cause; the reason \
            above names it when the producer knew).\n\
         2. Re-run cas-code-review with {rerun} and confirm the returned \
            envelope reports execution.personas_run > 0.\n\
         3. Re-call task.close with that envelope.\n\n         If the review transport is genuinely unavailable, review the diff by \
         another means and have a supervisor issue bypass_code_review=true — \
         that is a recorded decision, which a silently-empty review is not."
    )
}

/// cas-acf83 (GH #108): personas ran, but a mandatory lane did not.
///
/// The all-or-nothing check is not enough on its own: every always-on persona
/// runs on the same transport, so the outage that motivated this task takes all
/// four out at once while leaving the one Claude-hosted persona to report a
/// "successful" run.
fn format_review_incomplete(
    task_id: &str,
    required_missing: &[String],
    personas_failed: &[String],
    supervisor_owned: bool,
) -> String {
    let rerun = if supervisor_owned {
        "mode=interactive (or mode=headless for skill-to-skill)"
    } else {
        "mode=autofix"
    };
    let failures = if personas_failed.is_empty() {
        "(the producer recorded no per-persona reason)".to_string()
    } else {
        personas_failed.join("\n  - ")
    };
    format!(
        "⚠️ REVIEW INCOMPLETE\n\n         task close rejected for {task_id}: the review ran, but these mandatory \
         reviewers produced no verdict, so whole classes of defect went \
         unexamined:\n  - {}\n\n         Recorded failures:\n  - {failures}\n\n         An empty residual[] from a partial review is not a clean bill of \
         health — it is silence from the reviewers that did not run.\n\n         To resolve: fix the transport (a shared outage takes out every \
         same-transport persona at once), re-run cas-code-review with {rerun}, \
         and confirm execution.required_personas_missing is empty. If the \
         transport cannot be restored, review those lanes by another means and \
         have a supervisor record bypass_code_review=true.",
        required_missing.join("\n  - "),
    )
}

/// cas-acf83 (GH #108): the envelope says nothing about whether it ran.
fn format_review_execution_unreported(task_id: &str, supervisor_owned: bool) -> String {
    let rerun = if supervisor_owned {
        "mode=interactive (or mode=headless for skill-to-skill)"
    } else {
        "mode=autofix"
    };
    format!(
        "⚠️ REVIEW EXECUTION UNREPORTED\n\n         task close rejected for {task_id}: the code_review_findings envelope \
         carries no `execution` block, so there is no evidence a review ran. \
         An envelope without it is indistinguishable from hand-written JSON, \
         and an empty residual[] then proves nothing.\n\n         To resolve: re-run cas-code-review with {rerun} and pass the envelope \
         it returns verbatim — it now reports execution.personas_run, \
         execution.personas_failed, and execution.skipped_reason.\n\n         {}\n\n         Supervisors may bypass with bypass_code_review=true (logged).",
        cas_types::review_outcome_shape_hint(),
    )
}

fn format_code_review_required(supervisor_owned: bool) -> String {
    let step1 = if supervisor_owned {
        "1. Invoke the cas-code-review skill via the Skill or Task tool with \
         mode=interactive (or mode=headless for skill-to-skill) and the \
         current diff.\n\
         Note: mode=autofix is the legacy path for projects that pin \
         [code_review] owner = \"worker\"."
    } else {
        "1. Invoke the cas-code-review skill via the Skill or Task tool with \
         mode=autofix and the current diff."
    };
    format!(
        "⚠️ CODE_REVIEW_REQUIRED\n\n\
         task close rejected: this task has reviewable code changes \
         and no code_review_findings envelope was provided.\n\n\
         To resolve:\n\
         {step1}\n\
         2. Collect the returned ReviewOutcome envelope (residual, \
            pre_existing, mode).\n\
         3. Re-call task.close with the envelope JSON-stringified \
            in code_review_findings.\n\n\
         {}\n\n\
         Supervisors may bypass this gate with \
         bypass_code_review=true (logged as a decision note).",
        cas_types::review_outcome_shape_hint(),
    )
}

/// Run the cas-code-review P0 close gate.
///
/// `supervisor_owned` selects owner-aware mode guidance in
/// `CODE_REVIEW_REQUIRED` (interactive/headless vs legacy autofix).
pub(crate) fn run_code_review_gate(
    task: &Task,
    req: &TaskCloseRequest,
    project_root: &std::path::Path,
    supervisor_owned: bool,
) -> CodeReviewGateOutcome {
    // Skip 1: additive-only tasks bypass the gate entirely.
    if task.execution_note.as_deref() == Some("additive-only") {
        return CodeReviewGateOutcome::Proceed;
    }

    // Skip 2: supervisor override.
    if req.bypass_code_review.unwrap_or(false) {
        if is_supervisor_from_env() {
            let reason = req
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("(no reason provided)");
            let note = format!(
                "[{}] DECISION: cas-code-review P0 gate overridden by supervisor. \
                 Reason: {}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                reason
            );
            return CodeReviewGateOutcome::AppendDecisionNote(note);
        } else {
            return CodeReviewGateOutcome::Reject(
                "⚠️ UNAUTHORIZED OVERRIDE\n\n\
                 task close rejected: bypass_code_review=true is only honored \
                 when the caller runs as a supervisor (CAS_AGENT_ROLE=supervisor). \
                 Non-supervisor callers must either fix the P0 findings and retry \
                 close, or ask a supervisor to issue the override."
                    .to_string(),
            );
        }
    }

    // Skip 3: docs-only / test-only / empty diffs. The gate is not a
    // SPOF for changes it cannot meaningfully review.
    if !has_reviewable_changes(project_root) {
        return CodeReviewGateOutcome::Proceed;
    }

    // From here on, we require a findings envelope. The request's
    // `code_review_findings` always wins; if it is absent or empty we
    // fall back to any envelope persisted on the task deliverables
    // from a prior jailed close (cas-3086). The persisted fallback is
    // *not* a merge — an explicit request envelope wholly replaces
    // what the gate sees.
    let persisted_envelope = task
        .deliverables
        .review_envelope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let envelope_json = match req.code_review_findings.as_deref() {
        Some(s) if !s.trim().is_empty() => s,
        _ => match persisted_envelope {
            Some(s) => s,
            None => {
                return CodeReviewGateOutcome::Reject(format_code_review_required(
                    supervisor_owned,
                ));
            }
        },
    };

    // cas-297e: multi-field parse via parse_review_outcome so a single
    // bad envelope lists every missing Finding field (and documents the
    // full Finding schema in the reject text).
    let envelope: cas_types::ReviewOutcome = match cas_types::parse_review_outcome(envelope_json) {
        Ok(e) => e,
        Err(e) => {
            return CodeReviewGateOutcome::Reject(format!(
                "⚠️ MALFORMED REVIEW ENVELOPE\n\n\
                     task close rejected: code_review_findings failed to parse \
                     as ReviewOutcome JSON:\n{}\n\n\
                     {}\n\n\
                     Fix every listed field (or re-run cas-code-review) and retry close.",
                e.message,
                cas_types::review_outcome_shape_hint(),
            ));
        }
    };

    // cas-acf83 (GH #108): a review that never ran is not a clean review.
    //
    // The checks below only look for P0 findings, so `residual: []` passes
    // them trivially — and that is exactly what the workflow returns when
    // every persona fails to launch. When the Codex transport ran out of
    // credits, envelopes with `personas_run: 0` sailed through this gate; the
    // voluntary re-reviews that caught it found a workspace-build break and a
    // P0 regression. An absent verdict must never read as a passing verdict.
    match envelope.execution_status() {
        cas_types::ReviewExecutionStatus::Executed { .. } => {}
        cas_types::ReviewExecutionStatus::Incomplete {
            required_missing,
            personas_failed,
            ..
        } => {
            return CodeReviewGateOutcome::Reject(format_review_incomplete(
                &task.id,
                &required_missing,
                &personas_failed,
                supervisor_owned,
            ));
        }
        cas_types::ReviewExecutionStatus::DidNotExecute { reason } => {
            return CodeReviewGateOutcome::Reject(format_review_did_not_execute(
                &task.id,
                &reason,
                supervisor_owned,
            ));
        }
        cas_types::ReviewExecutionStatus::Unreported => {
            return CodeReviewGateOutcome::Reject(format_review_execution_unreported(
                &task.id,
                supervisor_owned,
            ));
        }
    }

    use cas_store::code_review::close_gate::{GateDecision, evaluate_gate, format_block_message};
    use cas_types::FindingSeverity;

    // cas-4c64: apply the full forgery defence — symmetric with
    // worker_review_envelope_is_clean — so the gate is equally strict
    // whether the envelope came from `req.code_review_findings` or was
    // read back from the persisted `task.deliverables.review_envelope`
    // (the cas-3086 retry-after-jail path). Before this fix, only
    // `evaluate_gate` was called here; that function filters on
    // `!f.pre_existing && f.severity == P0`, so a forged envelope with a
    // P0 carrying `pre_existing: true` in `residual[]` would pass the
    // gate even though `worker_review_envelope_is_clean` had rejected it
    // on the original close attempt and the envelope was persisted by the
    // jail-arming branch.

    // Check A: no P0 in residual[], regardless of per-finding pre_existing
    // flag. This catches the forgery vector where a P0 is marked
    // pre_existing=true to evade evaluate_gate's filter. Genuine
    // pre-existing P0s belong in `outcome.pre_existing[]`, not residual[].
    let residual_p0s: Vec<_> = envelope
        .residual
        .iter()
        .filter(|f| f.severity == FindingSeverity::P0)
        .cloned()
        .collect();
    if !residual_p0s.is_empty() {
        return CodeReviewGateOutcome::Reject(format_block_message(&task.id, &residual_p0s));
    }

    // Check B: no P0 in pre_existing[] bucket (cas-fef4 forgery defence).
    // Workers cannot reclassify a P0 as pre-existing to bypass the gate.
    if envelope
        .pre_existing
        .iter()
        .any(|f| f.severity == FindingSeverity::P0)
    {
        return CodeReviewGateOutcome::Reject(
            "⚠️ BLOCKED: P0 in pre_existing[]\n\n\
             task close rejected: code_review_findings pre_existing[] contains \
             a P0-severity finding. Pre-existing P0s are not a downgrade slot — \
             they block the close gate regardless of classification. \
             Fix the P0, re-run cas-code-review, and retry close. \
             (cas-fef4 + cas-4c64 forgery defence)"
                .to_string(),
        );
    }

    // Final check: evaluate_gate for PR-introduced P0s (pre_existing=false).
    // After Checks A and B above, this is redundant for P0 detection, but
    // retained for the format_block_message formatting it provides and as a
    // safety net in case evaluate_gate's semantics are extended in future.
    match evaluate_gate(&envelope.residual) {
        GateDecision::Allow => CodeReviewGateOutcome::Proceed,
        GateDecision::BlockOnP0(blocking) => {
            CodeReviewGateOutcome::Reject(format_block_message(&task.id, &blocking))
        }
    }
}

/// Return `true` if `project_root` has any staged, unstaged, or
/// committed-since-base changes in files that are worth asking the
/// multi-persona reviewer about. Returns `false` for docs-only
/// (`*.md`, anything under `docs/`) and test-only diffs, and for
/// non-git directories where we cannot reason about the diff.
///
/// The classification is deliberately *loose*: when we cannot tell
/// whether a change is reviewable, we assume it is, and the worker
/// runs the review. False positives waste latency; false negatives
/// silently skip the gate.
pub(crate) fn has_reviewable_changes(project_root: &std::path::Path) -> bool {
    use std::process::Command;

    // Collect changed paths from both the index/working-tree diff and
    // the HEAD diff. Union handles in-flight edits on top of the
    // already-committed task work.
    let mut changed: Vec<String> = Vec::new();

    for args in [
        &["diff", "--name-only", "HEAD"][..],
        &["diff", "--name-only", "--cached"][..],
    ] {
        if let Ok(output) = Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
        {
            if !output.status.success() {
                // Not a git repo, or HEAD doesn't exist — we cannot
                // reason about the diff, so the gate should not block.
                // Per the "not a SPOF" rule, treat as no-reviewable.
                return false;
            }
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    changed.push(trimmed.to_string());
                }
            }
        } else {
            return false;
        }
    }

    changed.sort();
    changed.dedup();

    changed.iter().any(|path| is_reviewable_path(path))
}

/// Classify a single path as "worth running the multi-persona
/// reviewer on". Docs (`*.md`, anything under `docs/`) and tests
/// (anything under `tests/`, `test/`, or a file ending in
/// `_test.rs` / `.test.ts`) are excluded.
pub(crate) fn is_reviewable_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();

    // Docs
    if lower.ends_with(".md") {
        return false;
    }
    if lower.starts_with("docs/") || lower.contains("/docs/") {
        return false;
    }

    // Tests
    if lower.starts_with("tests/") || lower.contains("/tests/") {
        return false;
    }
    if lower.starts_with("test/") || lower.contains("/test/") {
        return false;
    }
    if lower.ends_with("_test.rs")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.tsx")
        || lower.ends_with(".spec.ts")
        || lower.ends_with(".spec.tsx")
        || lower.ends_with("_test.py")
        || lower.ends_with("_test.go")
    {
        return false;
    }

    true
}

/// cas-ee2b: return `true` if the worker's committed history
/// (`merge-base..HEAD` inside `worker_worktree_path`) contains any files
/// that qualify as reviewable by [`is_reviewable_path`].
///
/// This is the isolated-worker-aware replacement for the
/// `has_reviewable_changes(close_project_root)` call that was checking
/// the **main repo's working tree** state. For a worker with an isolated
/// worktree, the main repo's dirty files are irrelevant — what matters is
/// what the worker actually committed on their branch.
///
/// Examples of cases where the old check gave a wrong answer:
/// - Researcher closes a spike task (zero code commits). Main repo has
///   a dirty `Cargo.lock` or in-flight edit. Old: true (wrong) → CODE_REVIEW_REQUIRED.
///   New: false (correct) → gate skipped.
/// - Worker committed only `*.md` docs. Old: depends on main repo state.
///   New: false (correct) → gate skipped.
///
/// Returns `false` on any git failure (graceful degradation — avoids
/// false-requiring review when history is unknowable, consistent with
/// `has_reviewable_changes` returning false for non-git dirs).
pub(crate) fn has_worker_committed_reviewable_changes(
    worker_worktree_path: &std::path::Path,
    parent_branch: &str,
) -> bool {
    use std::process::Command;

    let merge_base_out = Command::new("git")
        .args(["merge-base", "HEAD", parent_branch])
        .current_dir(worker_worktree_path)
        .output();
    let merge_base = match merge_base_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return false,
    };
    if merge_base.is_empty() {
        return false;
    }

    let diff_out = Command::new("git")
        .args(["diff", "--name-only", &format!("{merge_base}..HEAD")])
        .current_dir(worker_worktree_path)
        .output();
    match diff_out {
        Ok(o) if o.status.success() => {
            let output = String::from_utf8_lossy(&o.stdout);
            output.lines().any(|line| {
                let trimmed = line.trim();
                !trimmed.is_empty() && is_reviewable_path(trimmed)
            })
        }
        _ => false,
    }
}

/// cas-1932 (GH #62 symptom 2): do the commits this task made during its own
/// work cycle touch reviewable code?
///
/// Scoped exactly like the cas-e74c merge guard: commits reachable from HEAD
/// but not from `parent_branch`, whose committer date falls at or after the
/// work-cycle start (same clock-skew allowance). Uncommitted working-tree
/// state is deliberately ignored — in a shared checkout it belongs to whoever
/// left it there, not to the closing task.
///
/// `None` means git could not answer (no repo, no merge-base, failed log);
/// callers fall back to the unscoped checkout signal rather than treating an
/// unknown as "nothing to review".
pub(crate) fn has_task_attributable_reviewable_changes(
    repo_path: &std::path::Path,
    parent_branch: &str,
    window: &TaskCommitReceiptWindow,
) -> Option<bool> {
    use std::process::Command;

    if !is_safe_git_refname(parent_branch) {
        return None;
    }

    let merge_base_out = Command::new("git")
        .args(["merge-base", "HEAD", parent_branch])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !merge_base_out.status.success() {
        return None;
    }
    let merge_base = String::from_utf8_lossy(&merge_base_out.stdout)
        .trim()
        .to_string();
    if merge_base.is_empty() {
        return None;
    }

    let since = format!(
        "@{}",
        window.not_before.timestamp() - COMMIT_RECEIPT_CLOCK_SKEW_SECS
    );
    let log_out = Command::new("git")
        .args([
            "log",
            &format!("--since={since}"),
            "--name-only",
            "--pretty=format:",
            &format!("{merge_base}..HEAD"),
        ])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !log_out.status.success() {
        return None;
    }
    let output = String::from_utf8_lossy(&log_out.stdout);
    Some(output.lines().any(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && is_reviewable_path(trimmed)
    }))
}

/// Parse the output of `git diff --name-status` into violations. Only rows
/// whose status starts with M, D, or R are returned. A, C, T, U, and ?? are
/// considered additive or uninteresting.
fn parse_name_status(output: &str) -> Vec<AdditiveOnlyViolation> {
    let mut violations = Vec::new();
    for line in output.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        // Format: "<STATUS>\t<PATH>" or for renames "R100\t<OLD>\t<NEW>"
        let mut parts = line.splitn(3, '\t');
        let Some(status) = parts.next() else {
            continue;
        };
        let Some(first_path) = parts.next() else {
            continue;
        };
        let second_path = parts.next();
        let first_char = status.chars().next().unwrap_or(' ');
        match first_char {
            'M' | 'D' => violations.push(AdditiveOnlyViolation {
                status: status.to_string(),
                path: first_path.to_string(),
            }),
            'R' => {
                let path = second_path.unwrap_or(first_path).to_string();
                violations.push(AdditiveOnlyViolation {
                    status: status.to_string(),
                    path,
                });
            }
            _ => {}
        }
    }
    violations
}

#[cfg(test)]
mod additive_only_tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn git_output(dir: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("existing.txt"), "original\n").unwrap();
        git(p, &["add", "existing.txt"]);
        git(p, &["commit", "-q", "-m", "initial"]);
        dir
    }

    // Legacy `check_additive_only_violations` unit tests (non_git_dir,
    // clean_repo, new_file, modified_file, deleted_file, renamed_file)
    // were removed alongside the function itself. The `branch_check_*`
    // tests below cover the replacement path — see cas-bc1b follow-up.

    #[test]
    fn parse_name_status_mixed() {
        let out = "A\tadded.txt\nM\tmodified.txt\nD\tdeleted.txt\nR100\told.txt\tnew.txt\n";
        let v = parse_name_status(out);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].path, "modified.txt");
        assert_eq!(v[1].path, "deleted.txt");
        assert_eq!(v[2].path, "new.txt");
        assert!(v[2].status.starts_with('R'));
    }

    // --- cas-895d: check_uncommitted_work ---------------------------------

    #[test]
    fn uncommitted_non_git_dir_is_empty() {
        let dir = tempdir().unwrap();
        assert!(check_uncommitted_work(dir.path()).is_empty());
    }

    #[test]
    fn uncommitted_clean_repo_is_empty() {
        let dir = init_repo();
        assert!(check_uncommitted_work(dir.path()).is_empty());
    }

    #[test]
    fn uncommitted_untracked_file_is_ignored() {
        let dir = init_repo();
        std::fs::write(dir.path().join("scratch.log"), "noise\n").unwrap();
        let v = check_uncommitted_work(dir.path());
        assert!(
            v.is_empty(),
            "untracked files must not count as lost work, got: {v:?}"
        );
    }

    #[test]
    fn uncommitted_staged_new_file_is_caught() {
        // cas-895d core scenario: the worker wrote a new file and staged
        // it, but never committed. This is EXACTLY the cas-953d miss —
        // the work exists on disk but would be GC'd with the worktree.
        let dir = init_repo();
        std::fs::write(dir.path().join("new.rs"), "fn main() {}\n").unwrap();
        git(dir.path(), &["add", "new.rs"]);
        let v = check_uncommitted_work(dir.path());
        assert_eq!(v.len(), 1, "staged-but-uncommitted must block: {v:?}");
        assert_eq!(v[0].path, "new.rs");
        assert!(
            v[0].status.starts_with('A'),
            "staged-new status should start with A, got {}",
            v[0].status
        );
    }

    #[test]
    fn uncommitted_unstaged_modification_is_caught() {
        let dir = init_repo();
        std::fs::write(dir.path().join("existing.txt"), "changed\n").unwrap();
        let v = check_uncommitted_work(dir.path());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "existing.txt");
        assert!(
            v[0].status.contains('M'),
            "modified status should contain M, got {}",
            v[0].status
        );
    }

    #[test]
    fn uncommitted_staged_modification_is_caught() {
        let dir = init_repo();
        std::fs::write(dir.path().join("existing.txt"), "changed\n").unwrap();
        git(dir.path(), &["add", "existing.txt"]);
        let v = check_uncommitted_work(dir.path());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "existing.txt");
        assert!(v[0].status.contains('M'));
    }

    #[test]
    fn uncommitted_deleted_tracked_file_is_caught() {
        let dir = init_repo();
        std::fs::remove_file(dir.path().join("existing.txt")).unwrap();
        let v = check_uncommitted_work(dir.path());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "existing.txt");
        assert!(v[0].status.contains('D'));
    }

    #[test]
    fn uncommitted_renamed_tracked_file_is_caught() {
        let dir = init_repo();
        git(dir.path(), &["mv", "existing.txt", "renamed.txt"]);
        let v = check_uncommitted_work(dir.path());
        assert_eq!(v.len(), 1);
        assert!(
            v[0].status.contains('R'),
            "renamed status should contain R, got {}",
            v[0].status
        );
        // Porcelain prints "R  old -> new"; check_uncommitted_work
        // records the new path.
        assert_eq!(v[0].path, "renamed.txt");
    }

    // --- cas-bc1b: check_additive_only_branch_violations ------------------

    /// Helper: initialize a repo, create a `main` commit, branch off into
    /// `factory/worker`, and return the tempdir. The caller can then commit
    /// whatever it wants on `factory/worker` before running the check.
    fn init_branched_repo() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("existing.txt"), "original\n").unwrap();
        git(p, &["add", "existing.txt"]);
        git(p, &["commit", "-q", "-m", "main: initial"]);
        git(p, &["checkout", "-q", "-b", "factory/worker"]);
        dir
    }

    #[test]
    fn branch_check_non_git_returns_empty() {
        let dir = tempdir().unwrap();
        assert!(
            check_additive_only_branch_violations(
                dir.path(),
                "main",
                None,
                &TaskCommitIdentity::default()
            )
            .is_empty()
        );
    }

    #[test]
    fn branch_check_missing_parent_branch_returns_empty() {
        // New repo with `main` but no such branch `nope` — merge-base
        // fails → empty. The gate must not fire when it can't reason
        // about history.
        let dir = init_branched_repo();
        let v = check_additive_only_branch_violations(
            dir.path(),
            "nope",
            None,
            &TaskCommitIdentity::default(),
        );
        assert!(v.is_empty(), "unknown parent must no-op, got: {v:?}");
    }

    #[test]
    fn branch_check_clean_branch_is_empty() {
        // factory/worker has the same HEAD as main → no commits → no
        // violations.
        let dir = init_branched_repo();
        let v = check_additive_only_branch_violations(
            dir.path(),
            "main",
            None,
            &TaskCommitIdentity::default(),
        );
        assert!(v.is_empty(), "branch with no commits must be clean: {v:?}");
    }

    #[test]
    fn branch_check_additive_commit_passes() {
        // cas-bc1b happy path: the worker committed one new file on
        // their branch. The branch-diff must be empty of M/D/R entries.
        let dir = init_branched_repo();
        std::fs::write(dir.path().join("new.rs"), "fn main() {}\n").unwrap();
        git(dir.path(), &["add", "new.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: new.rs"]);
        let v = check_additive_only_branch_violations(
            dir.path(),
            "main",
            None,
            &TaskCommitIdentity::default(),
        );
        assert!(
            v.is_empty(),
            "purely additive branch commit must pass: {v:?}"
        );
    }

    #[test]
    fn branch_check_modifying_commit_fails() {
        // The worker modified an existing file on their branch. Must
        // be rejected.
        let dir = init_branched_repo();
        std::fs::write(dir.path().join("existing.txt"), "worker edit\n").unwrap();
        git(dir.path(), &["add", "existing.txt"]);
        git(
            dir.path(),
            &["commit", "-q", "-m", "fix: edit existing.txt"],
        );
        let v = check_additive_only_branch_violations(
            dir.path(),
            "main",
            None,
            &TaskCommitIdentity::default(),
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "existing.txt");
        assert!(v[0].status.starts_with('M'));
    }

    #[test]
    fn branch_check_merge_satisfied_scopes_to_worker_merge_delta() {
        // cas-0a2d shape: the epic already modifies a baseline file, while
        // the additive-only worker contributes one new file. After the
        // supervisor merge, the epic-vs-main diff contains M+A, but the
        // task's merge delta contains only A and must pass.
        let dir = init_branched_repo();
        git(dir.path(), &["checkout", "-q", "main"]);
        git(dir.path(), &["checkout", "-q", "-b", "epic"]);
        std::fs::write(dir.path().join("existing.txt"), "pre-existing epic edit\n").unwrap();
        git(dir.path(), &["add", "existing.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "epic: baseline drift"]);
        git(dir.path(), &["checkout", "-q", "-b", "factory/task"]);
        std::fs::write(dir.path().join("task-doc.md"), "task docs\n").unwrap();
        git(dir.path(), &["add", "task-doc.md"]);
        git(dir.path(), &["commit", "-q", "-m", "docs: additive task"]);
        let anchor = git_output(dir.path(), &["rev-parse", "HEAD"]);
        git(dir.path(), &["checkout", "-q", "epic"]);
        git(
            dir.path(),
            &["merge", "-q", "--no-ff", "factory/task", "-m", "merge task"],
        );

        let v = check_additive_only_branch_violations(
            dir.path(),
            "epic",
            Some(anchor.as_str()),
            &TaskCommitIdentity::default(),
        );
        assert!(
            v.is_empty(),
            "pre-existing epic modifications must not count against the task: {v:?}"
        );
    }

    /// GH #82 steps 1-3: worker 1 commits a WIP file for THIS task and dies;
    /// the supervisor merges the WIP into the epic to preserve it. Worker 2
    /// finishes the same task, superseding that file. The epic diff then shows
    /// the file as Modified — but its only prior version is the task's own WIP,
    /// so an additive-only close must not be rejected for it.
    ///
    /// Fixture shape: WIP merged into the epic BEFORE worker 2 branches, then
    /// worker 2's branch is merged and the anchor path scopes the diff.
    fn init_same_task_wip_repo(wip_message: &str) -> (tempfile::TempDir, String, String) {
        let dir = init_branched_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "main"]);
        git(p, &["checkout", "-q", "-b", "epic"]);
        // Worker 1's preserved WIP for the same task.
        std::fs::write(p.join("feature.rs"), "// wip\n").unwrap();
        git(p, &["add", "feature.rs"]);
        git(p, &["commit", "-q", "-m", wip_message]);
        let wip_sha = git_output(p, &["rev-parse", "HEAD"]);
        // Worker 2 branches from the epic that already carries the WIP.
        git(p, &["checkout", "-q", "-b", "factory/worker-two"]);
        std::fs::write(p.join("feature.rs"), "// finished\n").unwrap();
        git(p, &["add", "feature.rs"]);
        git(
            p,
            &["commit", "-q", "-m", "feat(cas-f1b1): finish the feature"],
        );
        let anchor = git_output(p, &["rev-parse", "HEAD"]);
        git(p, &["checkout", "-q", "epic"]);
        git(
            p,
            &[
                "merge",
                "-q",
                "--no-ff",
                "factory/worker-two",
                "-m",
                "merge worker two",
            ],
        );
        (dir, anchor, wip_sha)
    }

    #[test]
    fn branch_check_same_task_wip_is_not_a_pre_existing_modification() {
        let (dir, anchor, _) = init_same_task_wip_repo("wip(cas-f1b1): partial feature");

        // Without attribution this is the GH #82 false positive.
        let blind = check_additive_only_branch_violations(
            dir.path(),
            "epic",
            Some(anchor.as_str()),
            &TaskCommitIdentity::default(),
        );
        assert_eq!(
            blind.len(),
            1,
            "fixture must reproduce the false positive when attribution is unavailable: {blind:?}"
        );

        let attributed = check_additive_only_branch_violations(
            dir.path(),
            "epic",
            Some(anchor.as_str()),
            &TaskCommitIdentity {
                task_id: Some("cas-f1b1".to_string()),
                known_commits: Vec::new(),
            },
        );
        assert!(
            attributed.is_empty(),
            "a file whose only prior version is this task's own WIP is not pre-existing: {attributed:?}"
        );
    }

    #[test]
    fn branch_check_same_task_wip_is_attributable_by_recorded_commit_id() {
        // The WIP commit message never names the task (a dying worker's
        // scratch commit), but CAS durably recorded its commit id for this
        // task — that is attribution evidence too.
        let (dir, anchor, wip_sha) = init_same_task_wip_repo("wip: partial feature");
        let attributed = check_additive_only_branch_violations(
            dir.path(),
            "epic",
            Some(anchor.as_str()),
            &TaskCommitIdentity {
                task_id: Some("cas-f1b1".to_string()),
                known_commits: vec![wip_sha],
            },
        );
        assert!(
            attributed.is_empty(),
            "a recorded task commit id must attribute the prior version: {attributed:?}"
        );
    }

    #[test]
    fn branch_check_foreign_pre_existing_file_still_violates_under_attribution() {
        // Same shape, but the prior version came from unrelated work. The
        // gate must still reject: attribution relaxes only the task's own WIP.
        let (dir, anchor, _) = init_same_task_wip_repo("chore: unrelated baseline file");
        let violations = check_additive_only_branch_violations(
            dir.path(),
            "epic",
            Some(anchor.as_str()),
            &TaskCommitIdentity {
                task_id: Some("cas-f1b1".to_string()),
                known_commits: Vec::new(),
            },
        );
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert_eq!(violations[0].path, "feature.rs");
    }

    #[test]
    fn branch_check_same_task_wip_is_not_pre_existing_before_the_merge() {
        // Pre-merge path (merge-base..HEAD): worker 2's branch is not merged
        // yet, and the merge base already carries the task's own WIP.
        let dir = init_branched_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "main"]);
        git(p, &["checkout", "-q", "-b", "epic"]);
        std::fs::write(p.join("feature.rs"), "// wip\n").unwrap();
        git(p, &["add", "feature.rs"]);
        git(p, &["commit", "-q", "-m", "wip(cas-f1b1): partial feature"]);
        git(p, &["checkout", "-q", "-b", "factory/worker-two"]);
        std::fs::write(p.join("feature.rs"), "// finished\n").unwrap();
        git(p, &["add", "feature.rs"]);
        git(p, &["commit", "-q", "-m", "feat(cas-f1b1): finish"]);

        let attributed = check_additive_only_branch_violations(
            p,
            "epic",
            None,
            &TaskCommitIdentity {
                task_id: Some("cas-f1b1".to_string()),
                known_commits: Vec::new(),
            },
        );
        assert!(
            attributed.is_empty(),
            "the unmerged path must attribute the task's own WIP too: {attributed:?}"
        );
    }

    #[test]
    fn branch_check_file_touched_by_foreign_history_still_violates() {
        // The file was created by unrelated work and only LATER touched by
        // this task. Its pre-image is genuinely foreign — fail closed.
        let dir = init_branched_repo();
        let p = dir.path();
        git(p, &["checkout", "-q", "main"]);
        git(p, &["checkout", "-q", "-b", "epic"]);
        std::fs::write(p.join("feature.rs"), "// baseline\n").unwrap();
        git(p, &["add", "feature.rs"]);
        git(p, &["commit", "-q", "-m", "chore: baseline feature"]);
        std::fs::write(p.join("feature.rs"), "// task wip\n").unwrap();
        git(p, &["add", "feature.rs"]);
        git(p, &["commit", "-q", "-m", "wip(cas-f1b1): touch feature"]);
        git(p, &["checkout", "-q", "-b", "factory/worker-two"]);
        std::fs::write(p.join("feature.rs"), "// finished\n").unwrap();
        git(p, &["add", "feature.rs"]);
        git(p, &["commit", "-q", "-m", "feat(cas-f1b1): finish"]);

        let violations = check_additive_only_branch_violations(
            p,
            "epic",
            None,
            &TaskCommitIdentity {
                task_id: Some("cas-f1b1".to_string()),
                known_commits: Vec::new(),
            },
        );
        assert_eq!(
            violations.len(),
            1,
            "a file with foreign history in its pre-image must still violate: {violations:?}"
        );
    }

    #[test]
    fn task_commit_identity_collects_every_durable_task_commit() {
        let mut task = Task::new("cas-f1b1".to_string(), "same-task WIP".to_string());
        task.deliverables.factory_branch_anchor = Some("a".repeat(40));
        let identity = task_commit_identity(&task, Some("b".repeat(40)));
        assert_eq!(identity.task_id.as_deref(), Some("cas-f1b1"));
        assert_eq!(
            identity.known_commits,
            vec!["a".repeat(40), "b".repeat(40)],
            "anchor and delivery receipt are both task-owned commit evidence"
        );

        let bare = Task::new("cas-f1b1".to_string(), "no durable commits".to_string());
        let identity = task_commit_identity(&bare, None);
        assert!(identity.known_commits.is_empty());
        assert!(
            !identity.is_empty(),
            "the task id alone still supports message attribution"
        );
    }

    #[test]
    fn task_id_attribution_requires_a_whole_token_match() {
        assert!(message_references_task(
            "feat(cas-f1b1): finish the feature",
            "cas-f1b1"
        ));
        assert!(message_references_task(
            "body mentions cas-f1b1.\n",
            "cas-f1b1"
        ));
        assert!(
            !message_references_task("feat(cas-f1b12): different task", "cas-f1b1"),
            "a longer id must not be attributed to its prefix"
        );
        assert!(!message_references_task(
            "no task reference here",
            "cas-f1b1"
        ));
    }

    #[test]
    fn branch_check_merge_satisfied_still_rejects_task_modification() {
        let dir = init_branched_repo();
        git(dir.path(), &["checkout", "-q", "main"]);
        git(dir.path(), &["checkout", "-q", "-b", "epic"]);
        git(dir.path(), &["checkout", "-q", "-b", "factory/task"]);
        std::fs::write(dir.path().join("existing.txt"), "task edit\n").unwrap();
        git(dir.path(), &["add", "existing.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "fix: modify existing"]);
        let anchor = git_output(dir.path(), &["rev-parse", "HEAD"]);
        git(dir.path(), &["checkout", "-q", "epic"]);
        git(
            dir.path(),
            &["merge", "-q", "--no-ff", "factory/task", "-m", "merge task"],
        );

        let v = check_additive_only_branch_violations(
            dir.path(),
            "epic",
            Some(anchor.as_str()),
            &TaskCommitIdentity::default(),
        );
        assert_eq!(v.len(), 1, "task modification must still be rejected");
        assert_eq!(v[0].path, "existing.txt");
        assert!(v[0].status.starts_with('M'));
    }

    #[test]
    fn branch_check_deleting_commit_fails() {
        let dir = init_branched_repo();
        git(dir.path(), &["rm", "-q", "existing.txt"]);
        git(
            dir.path(),
            &["commit", "-q", "-m", "chore: drop existing.txt"],
        );
        let v = check_additive_only_branch_violations(
            dir.path(),
            "main",
            None,
            &TaskCommitIdentity::default(),
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "existing.txt");
        assert!(v[0].status.starts_with('D'));
    }

    #[test]
    fn branch_check_ignores_main_worktree_drift() {
        // The core cas-4333 repro: main has a dirty uncommitted file
        // after the worker's branch forked. The branch-diff view must
        // not attribute that dirt to the worker. We achieve this by
        // comparing `main..HEAD` from inside the worker's worktree,
        // which only sees the worker's own commits.
        //
        // This test runs everything in a single tempdir because the
        // production fix uses `git -C <worker_worktree_path>` which
        // already gives us the CWD isolation we need; a separate
        // physical worktree is not necessary for the unit. The main-
        // drift scenario is that the worker's branch is additive *and*
        // there's uncommitted dirt in the tree that isn't on the
        // branch. The branch-diff must not report it.
        let dir = init_branched_repo();
        // Additive commit on the worker branch.
        std::fs::write(dir.path().join("new.rs"), "fn main() {}\n").unwrap();
        git(dir.path(), &["add", "new.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: new.rs"]);
        // Now simulate drift: modify an existing tracked file but
        // leave it unstaged. The legacy `git diff HEAD` path would see
        // this and reject. The branch-diff path must not.
        std::fs::write(dir.path().join("existing.txt"), "drift\n").unwrap();
        let v = check_additive_only_branch_violations(
            dir.path(),
            "main",
            None,
            &TaskCommitIdentity::default(),
        );
        assert!(
            v.is_empty(),
            "uncommitted drift must not count against the branch: {v:?}"
        );
    }

    #[test]
    fn uncommitted_after_commit_is_empty() {
        // Complement scenario: the worker commits their work before
        // calling close. The gate must not fire.
        let dir = init_repo();
        std::fs::write(dir.path().join("new.rs"), "fn main() {}\n").unwrap();
        git(dir.path(), &["add", "new.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: add new.rs"]);
        let v = check_uncommitted_work(dir.path());
        assert!(v.is_empty(), "committed work must pass the gate: {v:?}");
    }
}

// ---------------------------------------------------------------------------
// cas-b51a: Lightweight structural lint (supervisor-owned review mode)
// ---------------------------------------------------------------------------

/// Outcome of the lightweight structural lint run at worker close-time when
/// `[code_review] owner = "supervisor"`.
///
/// The full multi-persona `cas-code-review` skill is deferred to the
/// supervisor; this gate only catches the most egregious anti-patterns
/// (leftover debug statements, `unimplemented!`, large commented-out
/// blocks) that should never leave a worker branch regardless of who
/// reviews.
#[derive(Debug)]
pub(crate) enum LightweightLintOutcome {
    /// Lint passed — proceed to `PendingSupervisorReview` transition.
    Pass,
    /// Lint found violations — worker must fix before close.
    Fail(String),
}

pub(crate) fn run_declared_pre_close_hook(
    task: &cas_types::Task,
    repo_context: &crate::mcp::tools::core::task::repo_context::RepoContext,
    worker_worktree_path: Option<&std::path::Path>,
    commit_receipt: Option<&str>,
) -> Result<cas_types::PreCloseHookEvidence, String> {
    let receipt_repo = worker_worktree_path.unwrap_or(&repo_context.repo_root);
    let normalized_receipt = commit_receipt
        .map(|receipt| resolve_task_commit_receipt_sha(receipt_repo, receipt))
        .transpose()
        .map_err(|error| {
            format!("PRE-CLOSE HOOK CONTEXT REJECTED: commit_receipt {error}")
        })?;
    let (execution_root, worktree_branch, task_tip) = match worker_worktree_path {
        Some(path) => {
            let branch = git_branch_name(path).ok_or_else(|| {
                "PRE-CLOSE HOOK CONTEXT REJECTED: task worktree has detached or unreadable HEAD"
                    .to_string()
            })?;
            let tip = normalized_receipt
                .as_deref()
                .or(task.deliverables.factory_branch_anchor.as_deref())
                .map(str::to_string)
                .or_else(|| resolve_branch_sha(path, "HEAD"))
                .ok_or_else(|| {
                    "PRE-CLOSE HOOK CONTEXT REJECTED: cannot resolve task-owned commit tip"
                        .to_string()
                })?;
            if !git_ref_exists(path, &tip) {
                return Err(
                    "PRE-CLOSE HOOK CONTEXT REJECTED: task commit evidence does not resolve in \
                     its validated worktree repository."
                        .to_string(),
                );
            }
            if !git_commit_is_ancestor(path, &tip, "HEAD") {
                return Err(
                    "PRE-CLOSE HOOK CONTEXT REJECTED: task commit evidence is not reachable from \
                     the validated task worktree branch. No close-time executable gate was run."
                        .to_string(),
                );
            }
            (path, Some(branch), tip)
        }
        None => {
            let tip = normalized_receipt
                .as_deref()
                .or(task.deliverables.factory_branch_anchor.as_deref())
                .ok_or_else(|| {
                    "PRE-CLOSE HOOK CONTEXT REJECTED: declared task repository resolved, but no \
                     task-owned worktree, commit receipt, or factory anchor identifies the code \
                     to check. No close-time executable gate was run."
                        .to_string()
                })?;
            if !git_ref_exists(&repo_context.repo_root, tip) {
                return Err(
                    "PRE-CLOSE HOOK CONTEXT REJECTED: task commit evidence does not resolve in \
                     the declared repository. No close-time executable gate was run."
                        .to_string(),
                );
            }
            if !commit_is_merged_into_parent(
                &repo_context.repo_root,
                tip,
                &repo_context.target_branch,
            ) {
                return Err(
                    "PRE-CLOSE HOOK CONTEXT REJECTED: task commit evidence is not reachable from \
                     the declared target branch. No close-time executable gate was run."
                        .to_string(),
                );
            }
            (repo_context.repo_root.as_path(), None, tip.to_string())
        }
    };
    match run_lightweight_structural_lint_at_tip(
        execution_root,
        Some(&repo_context.target_branch),
        &task_tip,
    ) {
        LightweightLintOutcome::Pass => Ok(cas_types::PreCloseHookEvidence {
            repo_selector: repo_context.repo_selector.clone(),
            target_branch: repo_context.target_branch.clone(),
            worktree_branch,
            task_tip: Some(task_tip),
        }),
        LightweightLintOutcome::Fail(message) => Err(message),
    }
}

pub(crate) fn git_commit_is_ancestor(
    repo_path: &std::path::Path,
    commit: &str,
    descendant: &str,
) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["merge-base", "--is-ancestor", commit, descendant])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn git_branch_name(repo_path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["branch", "--show-current"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!branch.is_empty()).then_some(branch)
}

/// Run the lightweight structural lint used in supervisor-owned review mode
/// against the working tree at `project_root` (`git diff HEAD` / `--cached`).
///
/// Prefer [`run_lightweight_structural_lint_with_scope`] for isolated factory
/// workers (cas-dc5d) so lint evaluates the committed task range inside the
/// worker worktree rather than shared main-checkout WIP.
pub(crate) fn run_lightweight_structural_lint(
    project_root: &std::path::Path,
) -> LightweightLintOutcome {
    run_lightweight_structural_lint_with_scope(project_root, None)
}

/// Lightweight structural lint with optional committed-range scoping.
///
/// Scans the git diff for patterns that must never reach a review queue:
///
/// - `unimplemented!()` / `todo!()` macros (incomplete stubs)
/// - `dbg!` macro calls (leftover debug instrumentation)
/// - Commented-out code blocks larger than 5 consecutive lines
///
/// This lint is intentionally cheap — it runs on raw diff text without
/// language-aware parsing. False positives in multi-line string literals
/// are accepted in exchange for zero external dependencies and <1s latency.
///
/// ## Scope (cas-dc5d)
///
/// - `committed_range_parent = Some(parent)` — diff
///   `merge-base(HEAD, parent)..HEAD` inside `project_root`. Used for
///   isolated worker worktrees so only task commits are linted; unrelated
///   dirty files in the main checkout are never visible. **Fail-closed**
///   (cas-dc5d P2): unsafe/missing parent, failed merge-base, or failed
///   `git diff` returns [`LightweightLintOutcome::Fail`] with an actionable
///   message — never silent Pass.
/// - `committed_range_parent = None` — legacy working-tree `git diff HEAD`
///   (then `--cached`) in `project_root`, for non-isolated closes. Empty
///   diff still Passes (graceful degradation).
pub(crate) fn run_lightweight_structural_lint_with_scope(
    project_root: &std::path::Path,
    committed_range_parent: Option<&str>,
) -> LightweightLintOutcome {
    run_lightweight_structural_lint_at_tip(project_root, committed_range_parent, "HEAD")
}

fn run_lightweight_structural_lint_at_tip(
    project_root: &std::path::Path,
    committed_range_parent: Option<&str>,
    task_tip: &str,
) -> LightweightLintOutcome {
    use std::process::Command;

    // Collect the diff text.
    //
    // Isolated workers (cas-dc5d): committed range only — merge-base..HEAD
    // inside the worker worktree. Never `git diff HEAD` against a shared
    // main checkout that may carry unrelated WIP.
    //
    // Non-isolated: try `HEAD` (working tree + staged vs last commit) then
    // `--cached`. `--unified=0` keeps the consecutive-comment heuristic
    // free of context lines. Only one diff source is used.
    let diff_text = if let Some(parent) = committed_range_parent {
        // cas-dc5d P2: committed-range proof must fail closed. Passing on
        // unsafe/missing parent or failed merge-base/diff would silently
        // skip lint for isolated workers (worse than scanning wrong root).
        if !is_safe_git_refname(parent) {
            return LightweightLintOutcome::Fail(format!(
                "Cannot scope structural lint: parent branch name {parent:?} is not a safe git \
                 ref (empty or starts with '-'). Fix the task's parent epic branch and retry."
            ));
        }
        if !git_ref_exists(project_root, parent) {
            return LightweightLintOutcome::Fail(format!(
                "Cannot scope structural lint: parent branch `{parent}` does not resolve in the \
                 worker worktree. Ensure the epic/integration branch is available locally \
                 (fetch or merge base) and retry close."
            ));
        }
        if !git_ref_exists(project_root, task_tip) {
            return LightweightLintOutcome::Fail(format!(
                "Cannot scope structural lint: task tip `{task_tip}` does not resolve in the \
                 selected task repository."
            ));
        }
        let merge_base_out = Command::new("git")
            .args(["merge-base", task_tip, parent])
            .current_dir(project_root)
            .output();
        let merge_base = match merge_base_out {
            Ok(o) if o.status.success() => {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if s.is_empty() {
                    return LightweightLintOutcome::Fail(format!(
                        "Cannot scope structural lint: empty merge-base between task tip and \
                         `{parent}`. Check that the worker branch shares history with the \
                         integration branch."
                    ));
                }
                s
            }
            _ => {
                return LightweightLintOutcome::Fail(format!(
                    "Cannot scope structural lint: failed to compute merge-base(task tip, `{parent}`). \
                     Ensure both refs exist in the worker worktree and share history."
                ));
            }
        };
        match Command::new("git")
            .args(["diff", "--unified=0", &format!("{merge_base}..{task_tip}")])
            .current_dir(project_root)
            .output()
        {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                return LightweightLintOutcome::Fail(format!(
                    "Cannot scope structural lint: `git diff {merge_base}..{task_tip}` failed \
                     against target branch `{parent}`.{maybe_stderr}",
                    maybe_stderr = if stderr.trim().is_empty() {
                        String::new()
                    } else {
                        format!(" stderr: {}", stderr.trim())
                    }
                ));
            }
            Err(e) => {
                return LightweightLintOutcome::Fail(format!(
                    "Cannot scope structural lint: failed to spawn git diff ({e})."
                ));
            }
        }
    } else {
        let mut text = String::new();
        for args in [
            &["diff", "--unified=0", "HEAD"][..],
            &["diff", "--unified=0", "--cached"][..],
        ] {
            if let Ok(output) = Command::new("git")
                .args(args)
                .current_dir(project_root)
                .output()
            {
                if output.status.success() && !output.stdout.is_empty() {
                    text = String::from_utf8_lossy(&output.stdout).into_owned();
                    break; // use the first non-empty diff; don't append both
                }
            }
        }
        text
    };

    if diff_text.is_empty() {
        return LightweightLintOutcome::Pass;
    }

    let mut violations: Vec<String> = Vec::new();

    #[derive(Debug)]
    struct AddedLine<'a> {
        content: &'a str,
        path: String,
        file_line: usize,
        is_rust_file: bool,
    }

    #[derive(Debug)]
    enum DiffEntry<'a> {
        Added(AddedLine<'a>),
        HunkBoundary,
    }

    // Scan added lines only, tracking which file each line belongs to so that
    // Rust-specific macro checks can be scoped to `*.rs` files and findings can
    // point workers at the file they need to fix.
    let diff_entries: Vec<DiffEntry<'_>> = {
        let mut result = Vec::new();
        let mut current_path = String::new();
        let mut current_is_rust = false;
        let mut current_file_line = 0usize;
        for line in diff_text.lines() {
            if line.starts_with("+++ ") {
                // "+++ b/path/to/file.ext" in a unified git diff.
                // Strip the diff prefix ("b/") to obtain the bare path.
                let path = line[4..].trim_start_matches("b/");
                current_path = path.to_string();
                current_is_rust = path.ends_with(".rs");
                current_file_line = 0;
            } else if line.starts_with("@@ ") {
                result.push(DiffEntry::HunkBoundary);
            } else if line.starts_with('+') && !line.starts_with("+++") {
                current_file_line += 1;
                result.push(DiffEntry::Added(AddedLine {
                    content: &line[1..],
                    path: current_path.clone(),
                    file_line: current_file_line,
                    is_rust_file: current_is_rust,
                }));
            }
        }
        result
    };

    // Check 1: unimplemented!() or todo!() macro calls — Rust-only.
    // Use `contains("macro!(")` to catch both `macro!()` (no args) and
    // `macro!("msg")` forms, regardless of where they appear on the line.
    // Scoped to `*.rs` files; TypeScript/JS/Python etc. are not checked.
    for entry in &diff_entries {
        let DiffEntry::Added(added) = entry else {
            continue;
        };
        if !added.is_rust_file {
            continue;
        }
        let trimmed = added.content.trim();
        if trimmed.contains("unimplemented!(") {
            violations.push(format!(
                "{} line +{}: `unimplemented!()` — replace with a real implementation",
                added.path, added.file_line,
            ));
        }
        if trimmed.contains("todo!(") {
            violations.push(format!(
                "{} line +{}: `todo!()` — replace with a real implementation",
                added.path, added.file_line,
            ));
        }
    }

    // Check 2: dbg! macro calls — Rust-only.
    // Use `contains("dbg!(")` to catch all forms regardless of preceding
    // whitespace: bare `dbg!(...)`, `=dbg!(...)`, `let x=dbg!(...)`, etc.
    // Scoped to `*.rs` files; other languages do not use the dbg! macro.
    for entry in &diff_entries {
        let DiffEntry::Added(added) = entry else {
            continue;
        };
        if !added.is_rust_file {
            continue;
        }
        let trimmed = added.content.trim();
        if trimmed.contains("dbg!(") {
            violations.push(format!(
                "{} line +{}: `dbg!()` call — remove debug instrumentation before review",
                added.path, added.file_line,
            ));
        }
    }

    // Check 3: commented-out code blocks > 5 consecutive comment lines.
    // Heuristic: 6 or more consecutive lines that (a) start with '//'
    // and (b) are not doc comments ('///') or copyright headers.
    // Applied across all languages (// comments exist in Rust, TS, JS, etc.).
    {
        fn flush_comment_run(
            violations: &mut Vec<String>,
            run_path: &str,
            run_start: usize,
            run_end: usize,
            run: usize,
        ) {
            if run > 5 {
                violations.push(format!(
                    "{} lines +{}–+{}: {} consecutive commented-out lines — \
                     remove or restore the code before review (>5-line threshold)",
                    run_path, run_start, run_end, run
                ));
            }
        }

        let mut run = 0usize;
        let mut run_start = 0usize;
        let mut run_end = 0usize;
        let mut run_path = String::new();
        for entry in &diff_entries {
            let DiffEntry::Added(added) = entry else {
                flush_comment_run(&mut violations, &run_path, run_start, run_end, run);
                run = 0;
                run_path.clear();
                continue;
            };
            let trimmed = added.content.trim();
            let is_code_comment = trimmed.starts_with("//")
                && !trimmed.starts_with("///")
                && !trimmed.starts_with("//!");
            if is_code_comment {
                if run == 0 || run_path != added.path {
                    flush_comment_run(&mut violations, &run_path, run_start, run_end, run);
                    run_start = added.file_line;
                    run_path = added.path.clone();
                    run = 0;
                }
                run += 1;
                run_end = added.file_line;
            } else {
                flush_comment_run(&mut violations, &run_path, run_start, run_end, run);
                run = 0;
                run_path.clear();
            }
        }
        flush_comment_run(&mut violations, &run_path, run_start, run_end, run);
    }

    if violations.is_empty() {
        LightweightLintOutcome::Pass
    } else {
        let vlist = violations
            .iter()
            .enumerate()
            .map(|(i, v)| format!("  {}. {}", i + 1, v))
            .collect::<Vec<_>>()
            .join("\n");
        LightweightLintOutcome::Fail(format!(
            "Lightweight structural lint found {} violation(s):\n\n{}\n\n\
             Fix these before retrying close. The full cas-code-review skill \
             will be run by the supervisor once these basics are clean.",
            violations.len(),
            vlist
        ))
    }
}

#[cfg(test)]
mod lightweight_lint_tests {
    //! Unit tests for the cas-b51a lightweight structural lint gate.
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn init_repo_with_diff(added_lines: &str) -> TempDir {
        init_repo_with_diff_for_file("changed.rs", added_lines)
    }

    /// Like `init_repo_with_diff` but lets the caller choose the filename
    /// (and therefore the extension) of the file being changed. This is used
    /// to exercise the language-aware lint checks (e.g. Rust macros must not
    /// fire for `.ts` / `.js` / `.py` changes).
    fn init_repo_with_diff_for_file(filename: &str, added_lines: &str) -> TempDir {
        init_repo_with_diffs(&[(filename, added_lines)])
    }

    fn init_repo_with_diffs(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        // init git
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // initial commit so HEAD exists
        std::fs::write(dir.path().join("base.rs"), "fn main() {}\n").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // write changed files using the requested filename/extension
        for (filename, added_lines) in files {
            let changed_path = dir.path().join(filename);
            if let Some(parent) = changed_path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&changed_path, added_lines).unwrap();
        }
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    fn init_repo_with_multihunk_comment_diff() -> TempDir {
        let dir = TempDir::new().unwrap();
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        let original = (1..=40)
            .map(|i| format!("pub fn line_{i}() -> u32 {{ {i} }}\n"))
            .collect::<String>();
        std::fs::write(dir.path().join("changed.rs"), original).unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        let mut modified = String::new();
        for i in 1..=40 {
            modified.push_str(&format!("pub fn line_{i}() -> u32 {{ {i} }}\n"));
            if i == 2 || i == 30 {
                modified.push_str("// disabled line 1\n");
                modified.push_str("// disabled line 2\n");
                modified.push_str("// disabled line 3\n");
            }
        }
        std::fs::write(dir.path().join("changed.rs"), modified).unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn lint_passes_for_clean_code() {
        let dir = init_repo_with_diff("fn foo() { 42 }\n");
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Pass),
            "clean code should pass lint"
        );
    }

    #[test]
    fn lint_catches_unimplemented() {
        let dir = init_repo_with_diff("fn foo() { unimplemented!() }\n");
        let outcome = run_lightweight_structural_lint(dir.path());
        match outcome {
            LightweightLintOutcome::Fail(msg) => {
                assert!(
                    msg.contains("changed.rs line +1"),
                    "unimplemented!() finding must identify the file-local line: {msg}"
                );
            }
            other => panic!("unimplemented!() should fail lint, got {other:?}"),
        }
    }

    #[test]
    fn lint_catches_todo() {
        let dir = init_repo_with_diff("fn bar() { todo!(\"later\") }\n");
        let outcome = run_lightweight_structural_lint(dir.path());
        match outcome {
            LightweightLintOutcome::Fail(msg) => {
                assert!(
                    msg.contains("changed.rs line +1"),
                    "todo!() finding must identify the file-local line: {msg}"
                );
            }
            other => panic!("todo!() should fail lint, got {other:?}"),
        }
    }

    #[test]
    fn lint_catches_dbg_with_space() {
        // "let x = dbg!(foo)" — space before dbg
        let dir = init_repo_with_diff("let x = dbg!(foo);\n");
        let outcome = run_lightweight_structural_lint(dir.path());
        match outcome {
            LightweightLintOutcome::Fail(msg) => {
                assert!(
                    msg.contains("changed.rs line +1"),
                    "dbg!() finding must identify the file-local line: {msg}"
                );
            }
            other => panic!("dbg!() with space before it should fail lint, got {other:?}"),
        }
    }

    #[test]
    fn lint_catches_dbg_bare() {
        // "dbg!(foo)" — bare at start of expression
        let dir = init_repo_with_diff("dbg!(foo);\n");
        let outcome = run_lightweight_structural_lint(dir.path());
        match outcome {
            LightweightLintOutcome::Fail(msg) => {
                assert!(
                    msg.contains("changed.rs line +1"),
                    "bare dbg!() finding must identify the file-local line: {msg}"
                );
            }
            other => panic!("bare dbg!() should fail lint, got {other:?}"),
        }
    }

    #[test]
    fn lint_catches_dbg_no_space_after_equals() {
        // "let x=dbg!(foo)" — no space between = and dbg
        let dir = init_repo_with_diff("let x=dbg!(foo);\n");
        let outcome = run_lightweight_structural_lint(dir.path());
        match outcome {
            LightweightLintOutcome::Fail(msg) => {
                assert!(
                    msg.contains("changed.rs line +1"),
                    "dbg!() finding must identify the file-local line: {msg}"
                );
            }
            other => panic!("dbg!() with no space after '=' should fail lint, got {other:?}"),
        }
    }

    #[test]
    fn lint_catches_dbg_embedded_in_expression() {
        // "return self.compute(=dbg!(val))" — dbg! embedded in a call-site expression
        let dir = init_repo_with_diff("return self.compute(=dbg!(val));\n");
        let outcome = run_lightweight_structural_lint(dir.path());
        match outcome {
            LightweightLintOutcome::Fail(msg) => {
                assert!(
                    msg.contains("changed.rs line +1"),
                    "dbg!() finding must identify the file-local line: {msg}"
                );
            }
            other => panic!("dbg!() embedded in expression should fail lint, got {other:?}"),
        }
    }

    #[test]
    fn lint_catches_large_commented_block() {
        let code = "// line 1\n// line 2\n// line 3\n// line 4\n// line 5\n// line 6\n";
        let dir = init_repo_with_diff(code);
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Fail(_)),
            "6-line comment block should fail lint"
        );
    }

    #[test]
    fn lint_comment_block_finding_names_file() {
        let code = "// line 1\n// line 2\n// line 3\n// line 4\n// line 5\n// line 6\n";
        let dir = init_repo_with_diff_for_file("src/legacy.rs", code);
        let outcome = run_lightweight_structural_lint(dir.path());
        match outcome {
            LightweightLintOutcome::Fail(msg) => {
                assert!(
                    msg.contains("src/legacy.rs lines +1–+6"),
                    "comment-block finding must identify the file-local line range: {msg}"
                );
            }
            other => panic!("6-line comment block should fail lint, got {other:?}"),
        }
    }

    #[test]
    fn lint_does_not_merge_comment_runs_across_files() {
        let a = "// a1\n// a2\n// a3\n";
        let b = "// b1\n// b2\n// b3\n";
        let dir = init_repo_with_diffs(&[("src/a.rs", a), ("src/b.rs", b)]);
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Pass),
            "separate 3-line runs in separate files must not merge: {outcome:?}"
        );
    }

    #[test]
    fn lint_reports_real_multifile_comment_run_with_file_local_lines() {
        let a = "// a1\n// a2\n// a3\n";
        let b = "pub fn before() {}\n// b1\n// b2\n// b3\n// b4\n// b5\n// b6\n";
        let dir = init_repo_with_diffs(&[("src/a.rs", a), ("src/b.rs", b)]);
        let outcome = run_lightweight_structural_lint(dir.path());
        match outcome {
            LightweightLintOutcome::Fail(msg) => {
                assert!(
                    msg.contains("src/b.rs lines +2–+7"),
                    "finding must name the violating file and B-local lines: {msg}"
                );
                assert!(
                    !msg.contains("src/a.rs"),
                    "non-violating file must not appear in the finding: {msg}"
                );
            }
            other => panic!("real 6-line block in file B should fail lint, got {other:?}"),
        }
    }

    #[test]
    fn lint_does_not_merge_comment_runs_across_hunks() {
        let dir = init_repo_with_multihunk_comment_diff();
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Pass),
            "separate same-file 3-line runs in separate hunks must not merge: {outcome:?}"
        );
    }

    #[test]
    fn lint_allows_xml_block_doc_header() {
        let xml = "\
<!--
  Android colors for the feature.
  Kept verbose on purpose so this reproduces the ozer-style
  resource header that used to be confused with commented-out code.
  XML has block comments only.
  This is documentation, not disabled code.
-->
<resources>
  <color name=\"brand\">#123456</color>
</resources>
";
        let dir = init_repo_with_diff_for_file("res/values/colors.xml", xml);
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Pass),
            "XML block doc headers must not trip // commented-out-code lint: {outcome:?}"
        );
    }

    #[test]
    fn lint_allows_five_line_comment_block() {
        let code = "// line 1\n// line 2\n// line 3\n// line 4\n// line 5\n";
        let dir = init_repo_with_diff(code);
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Pass),
            "5-line comment block should pass lint (threshold is >5)"
        );
    }

    #[test]
    fn lint_passes_on_empty_repo() {
        let dir = TempDir::new().unwrap();
        // no git init at all — graceful degradation
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Pass),
            "no-git directory should pass (graceful degradation)"
        );
    }

    // --- cas-b829: language-aware Rust-macro gate ---
    // Rust macros (todo!, unimplemented!, dbg!) must only fire for *.rs files.
    // TypeScript / JS / Python diffs must never trigger these checks.
    //
    // NOTE: test strings below are built via format!() so that the Rust macro
    // patterns (todo!, unimplemented!, dbg!) do NOT appear as bare literals in
    // this source file and do not self-trip the very lint they test.

    #[test]
    fn lint_ignores_todo_macro_in_ts_file() {
        // A TypeScript file may contain `todo!()` as a literal string in a
        // comment or error message without it being an incomplete Rust stub.
        let code = format!("// NOTE: {}(\"not done\") later\n", "todo!");
        let dir = init_repo_with_diff_for_file("component.ts", &code);
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Pass),
            "todo!() pattern in a .ts file must not trigger Rust-macro lint"
        );
    }

    #[test]
    fn lint_ignores_unimplemented_macro_in_ts_file() {
        let code = format!("throw new Error('{}');\n", "unimplemented!()");
        let dir = init_repo_with_diff_for_file("service.ts", &code);
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Pass),
            "unimplemented!() pattern in a .ts file must not trigger Rust-macro lint"
        );
    }

    #[test]
    fn lint_ignores_dbg_macro_in_ts_file() {
        let code = format!("const x = {}value);\n", "dbg!(");
        let dir = init_repo_with_diff_for_file("utils.ts", &code);
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Pass),
            "dbg!() pattern in a .ts file must not trigger Rust-macro lint"
        );
    }

    #[test]
    fn lint_ignores_todo_macro_in_js_file() {
        let code = format!("// {}(\"later\")\n", "todo!");
        let dir = init_repo_with_diff_for_file("index.js", &code);
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Pass),
            "todo!() pattern in a .js file must not trigger Rust-macro lint"
        );
    }

    #[test]
    fn lint_ignores_todo_macro_in_python_file() {
        let code = format!("# {}(\"later\")\n", "todo!");
        let dir = init_repo_with_diff_for_file("app.py", &code);
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Pass),
            "todo!() pattern in a .py file must not trigger Rust-macro lint"
        );
    }

    #[test]
    fn lint_still_catches_todo_in_rs_file() {
        // Confirm existing Rust behaviour is preserved after the language-aware change.
        let code = format!("fn stub() {{ {}(\"implement\") }}\n", "todo!");
        let dir = init_repo_with_diff_for_file("lib.rs", &code);
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Fail(_)),
            "todo!() in a .rs file must still fail lint"
        );
    }

    #[test]
    fn lint_still_catches_unimplemented_in_rs_file() {
        let code = format!("fn stub() {{ {}() }}\n", "unimplemented!");
        let dir = init_repo_with_diff_for_file("lib.rs", &code);
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Fail(_)),
            "unimplemented!() in a .rs file must still fail lint"
        );
    }

    #[test]
    fn lint_still_catches_dbg_in_rs_file() {
        let code = format!("let x = {}value);\n", "dbg!(");
        let dir = init_repo_with_diff_for_file("lib.rs", &code);
        let outcome = run_lightweight_structural_lint(dir.path());
        assert!(
            matches!(outcome, LightweightLintOutcome::Fail(_)),
            "dbg!() in a .rs file must still fail lint"
        );
    }

    // --- cas-dc5d: scope lint to worker committed range, not main WIP ------

    fn git_dc5d(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Reproduces BUG-factory-close-lightweight-lint-wrong-project-root:
    /// main checkout has a dirty tracked file with >5 consecutive `//`
    /// lines; worker worktree has a clean committed task diff. Scoped
    /// lint (worker + merge-base..HEAD) must Pass; unscoped lint on main
    /// would Fail (precondition).
    #[test]
    fn lint_scoped_to_worker_range_ignores_main_checkout_wip() {
        let main = TempDir::new().unwrap();
        let p = main.path();
        git_dc5d(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("base.rs"), "fn main() {}\n").unwrap();
        git_dc5d(p, &["add", "base.rs"]);
        git_dc5d(p, &["commit", "-q", "-m", "init"]);

        // Isolated worker worktree with clean committed feature (no lint hit).
        let worker = p.join("worktrees").join("worker");
        std::fs::create_dir_all(worker.parent().unwrap()).unwrap();
        git_dc5d(
            p,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "factory/worker",
                worker.to_str().unwrap(),
            ],
        );
        std::fs::write(
            worker.join("feature.rs"),
            "pub fn feature() -> u32 { 42 }\n",
        )
        .unwrap();
        git_dc5d(&worker, &["add", "feature.rs"]);
        git_dc5d(&worker, &["commit", "-q", "-m", "feat: clean worker task"]);

        // Unrelated dirty *tracked* main-checkout WIP with 7 consecutive //
        // lines (must be tracked so `git diff HEAD` sees it).
        std::fs::write(p.join("wip.rs"), "fn leftover() {}\n").unwrap();
        git_dc5d(p, &["add", "wip.rs"]);
        git_dc5d(p, &["commit", "-q", "-m", "track wip placeholder"]);
        let dirty_comments = "\
// line 1 of unrelated WIP comment block
// line 2 of unrelated WIP comment block
// line 3 of unrelated WIP comment block
// line 4 of unrelated WIP comment block
// line 5 of unrelated WIP comment block
// line 6 of unrelated WIP comment block
// line 7 of unrelated WIP comment block
fn leftover() {}\n";
        std::fs::write(p.join("wip.rs"), dirty_comments).unwrap();

        // Precondition: unscoped lint on main would see the dirty WIP.
        let main_unscoped = run_lightweight_structural_lint(p);
        assert!(
            matches!(main_unscoped, LightweightLintOutcome::Fail(_)),
            "precondition: unscoped lint on dirty main must Fail, got {main_unscoped:?}"
        );

        // Worker-scoped committed-range lint must ignore main WIP.
        let scoped = run_lightweight_structural_lint_with_scope(&worker, Some("main"));
        assert!(
            matches!(scoped, LightweightLintOutcome::Pass),
            "worker committed-range lint must Pass despite dirty main, got {scoped:?}"
        );
    }

    /// Real violation in the worker's committed task range still fails
    /// when lint is scoped to merge-base..HEAD (not only working-tree).
    #[test]
    fn lint_scoped_to_worker_range_fails_on_committed_todo() {
        let main = TempDir::new().unwrap();
        let p = main.path();
        git_dc5d(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("base.rs"), "fn main() {}\n").unwrap();
        git_dc5d(p, &["add", "base.rs"]);
        git_dc5d(p, &["commit", "-q", "-m", "init"]);

        let worker = p.join("worktrees").join("worker");
        std::fs::create_dir_all(worker.parent().unwrap()).unwrap();
        git_dc5d(
            p,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "factory/worker",
                worker.to_str().unwrap(),
            ],
        );
        std::fs::write(
            worker.join("bad.rs"),
            "pub fn bad() { todo!(\"not done\"); }\n",
        )
        .unwrap();
        git_dc5d(&worker, &["add", "bad.rs"]);
        git_dc5d(&worker, &["commit", "-q", "-m", "feat: incomplete"]);

        // Working tree clean — unscoped `git diff HEAD` would Pass wrongly.
        let unscoped = run_lightweight_structural_lint(&worker);
        assert!(
            matches!(unscoped, LightweightLintOutcome::Pass),
            "precondition: clean working tree makes unscoped HEAD-diff Pass"
        );

        let scoped = run_lightweight_structural_lint_with_scope(&worker, Some("main"));
        assert!(
            matches!(scoped, LightweightLintOutcome::Fail(_)),
            "committed todo!() in worker range must Fail scoped lint, got {scoped:?}"
        );
    }

    /// cas-6de2 / close-gate false positive addendum: lint must evaluate the
    /// cumulative worker range at the branch tip, not a single earlier task-
    /// tagged commit. A bad comment block added in commit 1 and removed in
    /// commit 2 must clear.
    #[test]
    fn lint_scoped_to_worker_range_followup_commit_clears_comment_finding() {
        let main = TempDir::new().unwrap();
        let p = main.path();
        git_dc5d(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("base.rs"), "fn main() {}\n").unwrap();
        git_dc5d(p, &["add", "base.rs"]);
        git_dc5d(p, &["commit", "-q", "-m", "init"]);

        let worker = p.join("worktrees").join("worker");
        std::fs::create_dir_all(worker.parent().unwrap()).unwrap();
        git_dc5d(
            p,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "factory/worker",
                worker.to_str().unwrap(),
            ],
        );
        let bad = "\
// disabled line 1
// disabled line 2
// disabled line 3
// disabled line 4
// disabled line 5
// disabled line 6
pub fn feature() -> u32 { 1 }
";
        std::fs::write(worker.join("feature.rs"), bad).unwrap();
        git_dc5d(&worker, &["add", "feature.rs"]);
        git_dc5d(&worker, &["commit", "-q", "-m", "feat: add bad comments"]);

        let fixed = "pub fn feature() -> u32 { 1 }\n";
        std::fs::write(worker.join("feature.rs"), fixed).unwrap();
        git_dc5d(&worker, &["add", "feature.rs"]);
        git_dc5d(&worker, &["commit", "-q", "-m", "fix: remove bad comments"]);

        let scoped = run_lightweight_structural_lint_with_scope(&worker, Some("main"));
        assert!(
            matches!(scoped, LightweightLintOutcome::Pass),
            "follow-up fix at branch tip must clear earlier comment finding; got {scoped:?}"
        );
    }

    /// cas-dc5d P1: when the real integration parent is `epic/x` (diverged
    /// from `main`), scoping against `main` falsely includes epic-branch
    /// comment blocks in the lint range; scoping against `epic/x` does not.
    #[test]
    fn lint_scoped_parent_must_be_epic_not_main_when_they_diverge() {
        let main = TempDir::new().unwrap();
        let p = main.path();
        git_dc5d(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("base.rs"), "fn main() {}\n").unwrap();
        git_dc5d(p, &["add", "base.rs"]);
        git_dc5d(p, &["commit", "-q", "-m", "init"]);

        // Epic diverges from main with a large // comment block (would Fail
        // lint if incorrectly included in the worker's range vs main).
        git_dc5d(p, &["checkout", "-q", "-b", "epic/x"]);
        let epic_comments = "\
// epic line 1
// epic line 2
// epic line 3
// epic line 4
// epic line 5
// epic line 6
// epic line 7
pub fn epic_only() {}\n";
        std::fs::write(p.join("epic.rs"), epic_comments).unwrap();
        git_dc5d(p, &["add", "epic.rs"]);
        git_dc5d(
            p,
            &["commit", "-q", "-m", "epic clean-ish but comment-heavy"],
        );

        // Worker branches from epic with a clean feature.
        let worker = p.join("worktrees").join("worker");
        std::fs::create_dir_all(worker.parent().unwrap()).unwrap();
        git_dc5d(
            p,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "factory/worker",
                worker.to_str().unwrap(),
            ],
        );
        std::fs::write(worker.join("feature.rs"), "pub fn feature() -> u32 { 1 }\n").unwrap();
        git_dc5d(&worker, &["add", "feature.rs"]);
        git_dc5d(&worker, &["commit", "-q", "-m", "feat: worker task"]);

        // Wrong parent (main): range includes epic comment block → Fail.
        let vs_main = run_lightweight_structural_lint_with_scope(&worker, Some("main"));
        assert!(
            matches!(vs_main, LightweightLintOutcome::Fail(_)),
            "wrong parent=main must include epic comments and Fail, got {vs_main:?}"
        );

        // Correct parent (epic/x): only clean feature → Pass.
        let vs_epic = run_lightweight_structural_lint_with_scope(&worker, Some("epic/x"));
        assert!(
            matches!(vs_epic, LightweightLintOutcome::Pass),
            "parent=epic/x must Pass for clean worker feature, got {vs_epic:?}"
        );
    }

    /// cas-dc5d P2: missing parent ref must Fail with actionable text.
    #[test]
    fn lint_scoped_fails_closed_on_missing_parent() {
        let dir = init_repo_with_diff("fn ok() {}\n");
        let out =
            run_lightweight_structural_lint_with_scope(dir.path(), Some("epic/does-not-exist"));
        match out {
            LightweightLintOutcome::Fail(msg) => {
                assert!(
                    msg.contains("does not resolve") || msg.contains("Cannot scope"),
                    "must be actionable, got: {msg}"
                );
            }
            other => panic!("missing parent must Fail closed, got {other:?}"),
        }
    }

    /// cas-dc5d P2: unsafe parent refname must Fail closed (not Pass).
    #[test]
    fn lint_scoped_fails_closed_on_unsafe_parent() {
        let dir = init_repo_with_diff("fn ok() {}\n");
        let out =
            run_lightweight_structural_lint_with_scope(dir.path(), Some("-oProxyCommand=evil"));
        match out {
            LightweightLintOutcome::Fail(msg) => {
                assert!(
                    msg.contains("not a safe git") || msg.contains("Cannot scope"),
                    "must mention unsafe ref, got: {msg}"
                );
            }
            other => panic!("unsafe parent must Fail closed, got {other:?}"),
        }
    }

    /// cas-dc5d P2: unrelated histories (merge-base fails) must Fail closed.
    #[test]
    fn lint_scoped_fails_closed_on_merge_base_failure() {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        git_dc5d(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git_dc5d(p, &["add", "seed.txt"]);
        git_dc5d(p, &["commit", "-q", "-m", "seed"]);

        // Orphan factory tip — no merge-base with main.
        git_dc5d(p, &["checkout", "-q", "--orphan", "factory/worker"]);
        let _ = Command::new("git")
            .args(["rm", "-rf", "--cached", "."])
            .current_dir(p)
            .output();
        std::fs::write(p.join("orphan.txt"), "orphan\n").unwrap();
        git_dc5d(p, &["add", "orphan.txt"]);
        git_dc5d(p, &["commit", "-q", "-m", "orphan"]);

        let out = run_lightweight_structural_lint_with_scope(p, Some("main"));
        match out {
            LightweightLintOutcome::Fail(msg) => {
                assert!(
                    msg.contains("merge-base") || msg.contains("Cannot scope"),
                    "must mention merge-base failure, got: {msg}"
                );
            }
            other => panic!("failed merge-base must Fail closed, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod code_review_gate_tests {
    //! Unit tests for the cas-b39f close gate helper. Covers the full
    //! decision matrix in [`run_code_review_gate`] under the option-(a)
    //! architecture where the worker passes findings in via
    //! `TaskCloseRequest.code_review_findings` before retrying close.
    //!
    //! The pure-Rust decision helper at
    //! `cas_store::code_review::close_gate::evaluate_gate` is already
    //! tested exhaustively in that module; these tests focus on the
    //! close-side glue — env role check, envelope plumbing, override
    //! path, docs-only skip, CODE_REVIEW_REQUIRED rejection.
    use super::*;

    /// cas-6538: the light-skip decision note must name both rigor gates it
    /// bypasses and the reason, so the bypass is auditable. The integration
    /// tests assert on substrings of this text; lock the wording here.
    #[test]
    fn light_skip_decision_note_names_both_gates_and_reason() {
        let note = light_skip_decision_note();
        assert!(note.contains("depth=light"), "must cite the reason: {note}");
        assert!(
            note.to_lowercase().contains("decision"),
            "must be a decision note: {note}"
        );
        assert!(
            note.contains("verification jail"),
            "must name the verification jail skip: {note}"
        );
        assert!(
            note.contains("code-review gate"),
            "must name the P0 code-review gate skip: {note}"
        );
    }

    /// cas-7998: a close reason containing a double quote must be escaped so it
    /// can't terminate the surrounding `message="..."` argument early. Embedding
    /// the escaped reason in a representative quoted command must leave the
    /// double quotes balanced (every `"` is either the argument delimiter or a
    /// backslash-escaped literal).
    #[test]
    fn escape_close_reason_neutralizes_embedded_quotes() {
        let raw = "fixed the \"flaky\" test";
        let escaped = escape_close_reason_for_quoted_command(raw);
        assert!(
            !escaped.contains('\u{0022}') || escaped.contains("\\\""),
            "raw double quotes must be backslash-escaped: {escaped}"
        );
        // No unescaped quote survives.
        assert!(
            !escaped.replace("\\\"", "").contains('"'),
            "every embedded quote must be escaped: {escaped}"
        );
        let command = format!("message=\"Task X is ready to close. Close reason: {escaped}.\"");
        // Count unescaped quotes: strip escaped ones first, then the remaining
        // quotes are only the two argument delimiters → even count.
        let unescaped_quotes = command.replace("\\\"", "").matches('"').count();
        assert_eq!(
            unescaped_quotes, 2,
            "exactly the two delimiter quotes may remain unescaped: {command}"
        );
    }

    /// cas-7998: newlines/tabs/CRs in a close reason must collapse to single
    /// spaces so the suggested single-line coordination command can't be split.
    #[test]
    fn escape_close_reason_collapses_newlines_and_whitespace() {
        let raw = "line one\nline two\r\n\tindented   spaced";
        let escaped = escape_close_reason_for_quoted_command(raw);
        assert!(
            !escaped.contains('\n') && !escaped.contains('\r') && !escaped.contains('\t'),
            "no raw line/tab control chars may survive: {escaped:?}"
        );
        assert_eq!(
            escaped, "line one line two indented spaced",
            "whitespace runs must collapse to single spaces: {escaped:?}"
        );
    }

    /// cas-7998: a backslash already present in the reason is escaped before the
    /// quote-escape, so a trailing `\` followed by a quote can't combine into a
    /// stray escape that re-opens the argument.
    #[test]
    fn escape_close_reason_escapes_backslash_before_quote() {
        let raw = "path C:\\dir then \"q\"";
        let escaped = escape_close_reason_for_quoted_command(raw);
        assert!(
            escaped.contains("C:\\\\dir"),
            "backslashes must be doubled: {escaped}"
        );
        // The quote after the escaped backslash is itself escaped, so stripping
        // escaped backslashes then escaped quotes leaves no bare quote.
        let no_esc_backslash = escaped.replace("\\\\", "");
        assert!(
            !no_esc_backslash.replace("\\\"", "").contains('"'),
            "quote must remain escaped even adjacent to a backslash: {escaped}"
        );
    }

    use cas_types::{AutofixClass, Finding, FindingSeverity, Owner, ReviewOutcome};
    use tempfile::TempDir;

    fn base_task() -> Task {
        Task {
            id: "cas-test1".to_string(),
            title: "test".to_string(),
            status: TaskStatus::InProgress,
            ..Default::default()
        }
    }

    fn base_req(id: &str) -> TaskCloseRequest {
        TaskCloseRequest {
            id: id.to_string(),
            reason: None,
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: None,
            commit_receipt: None,
        }
    }

    fn p0_finding() -> Finding {
        Finding {
            title: "SQL injection".to_string(),
            severity: FindingSeverity::P0,
            file: "src/auth.rs".to_string(),
            line: 42,
            why_it_matters: "allows login bypass".to_string(),
            autofix_class: AutofixClass::Manual,
            owner: Owner::Human,
            confidence: 0.95,
            evidence: vec!["format!(\"... {}\", user_input)".to_string()],
            pre_existing: false,
            suggested_fix: None,
            requires_verification: false,
        }
    }

    fn p2_finding() -> Finding {
        Finding {
            title: "dead import".to_string(),
            severity: FindingSeverity::P2,
            file: "src/lib.rs".to_string(),
            line: 3,
            why_it_matters: "minor".to_string(),
            autofix_class: AutofixClass::Manual,
            owner: Owner::ReviewFixer,
            confidence: 0.9,
            evidence: vec!["use foo::bar;".to_string()],
            pre_existing: false,
            suggested_fix: None,
            requires_verification: false,
        }
    }

    /// An envelope from a review that actually ran (cas-acf83): the default
    /// shape for gate tests, since a review that did not run is now rejected
    /// before any finding is inspected.
    fn autofix_envelope(residual: Vec<Finding>) -> String {
        let env = ReviewOutcome {
            residual,
            pre_existing: Vec::new(),
            mode: "autofix".to_string(),
            execution: Some(cas_types::ReviewExecution {
                personas_run: 4,
                personas_failed: Vec::new(),
                skipped_reason: None,
                required_personas_missing: Vec::new(),
            }),
        };
        serde_json::to_string(&env).expect("serialize ReviewOutcome")
    }

    /// cas-acf83: the envelope shape the workflow returns when every persona
    /// failed to launch — structurally identical to a clean review except for
    /// the execution block.
    fn envelope_that_did_not_execute(personas_failed: Vec<&str>, skipped_reason: &str) -> String {
        let env = ReviewOutcome {
            residual: Vec::new(),
            pre_existing: Vec::new(),
            mode: "headless".to_string(),
            execution: Some(cas_types::ReviewExecution {
                personas_run: 0,
                personas_failed: personas_failed.into_iter().map(String::from).collect(),
                skipped_reason: Some(skipped_reason.to_string()),
                required_personas_missing: Vec::new(),
            }),
        };
        serde_json::to_string(&env).expect("serialize ReviewOutcome")
    }

    /// cas-acf83: an envelope with no execution block at all — what a
    /// hand-written one looks like, and what producers emitted before #108.
    fn envelope_without_execution_block() -> String {
        let env = ReviewOutcome {
            residual: Vec::new(),
            pre_existing: Vec::new(),
            mode: "autofix".to_string(),
            execution: None,
        };
        serde_json::to_string(&env).expect("serialize ReviewOutcome")
    }

    /// Build a throwaway git repo with one committed file, then stage
    /// whatever paths the caller names so `git diff --cached` sees
    /// them. Returns the tempdir so the caller controls its lifetime.
    fn repo_with_staged(paths: &[(&str, &str)]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        use std::process::Command;
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(p)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .status()
                .expect("git")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(&["add", "seed.txt"]);
        git(&["commit", "-q", "-m", "seed"]);
        for (path, contents) in paths {
            let full = p.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&full, contents).unwrap();
            git(&["add", path]);
        }
        dir
    }

    /// Serialize env-mutating tests so `CAS_AGENT_ROLE` changes don't
    /// leak between them — delegated to the process-wide poison-tolerant lock
    /// in `crate::hooks` so all CAS_*-mutating test modules share ONE lock.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::hooks::test_env_lock()
    }

    // --- Path classification ------------------------------------------------

    #[test]
    fn docs_and_tests_are_not_reviewable() {
        assert!(!is_reviewable_path("README.md"));
        assert!(!is_reviewable_path("docs/foo.txt"));
        assert!(!is_reviewable_path("crates/cas-store/tests/foo.rs"));
        assert!(!is_reviewable_path("src/foo_test.rs"));
        assert!(!is_reviewable_path("app/bar.test.tsx"));
        assert!(!is_reviewable_path("tests/integration.py"));
    }

    #[test]
    fn code_files_are_reviewable() {
        assert!(is_reviewable_path("src/main.rs"));
        assert!(is_reviewable_path("app/login.ts"));
        assert!(is_reviewable_path("pkg/server/handler.go"));
    }

    // --- run_code_review_gate branches --------------------------------------

    #[test]
    fn additive_only_task_bypasses_gate() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/evil.rs", "bad\n")]);
        let mut t = base_task();
        t.execution_note = Some("additive-only".to_string());
        let mut req = base_req(&t.id);
        req.code_review_findings = Some(autofix_envelope(vec![p0_finding()]));
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        assert!(matches!(out, CodeReviewGateOutcome::Proceed));
    }

    /// cas-acf83 (GH #108): the reported incident. Every persona failed to
    /// launch (Codex out of credits), the workflow returned `residual: []`
    /// with `personas_run: 0`, and the gate — which only looks for P0s —
    /// accepted it as a clean review. The empty findings list is an ABSENT
    /// verdict, not a passing one.
    #[test]
    fn a_review_that_never_ran_cannot_pass_the_gate() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn shipped_unreviewed() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        req.code_review_findings = Some(envelope_that_did_not_execute(
            vec!["correctness: transport unavailable (402 insufficient credits)"],
            "all personas failed to launch",
        ));

        match run_code_review_gate(&t, &req, dir.path(), true) {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("REVIEW DID NOT EXECUTE"),
                    "must name the failure mode, not look like a findings block: {msg}"
                );
                assert!(
                    msg.contains("all personas failed to launch"),
                    "must quote the producer's reason: {msg}"
                );
                assert!(
                    msg.contains("insufficient credits"),
                    "must surface the named persona launch failure: {msg}"
                );
                assert!(
                    msg.contains("bypass_code_review"),
                    "must name the recorded escape hatch: {msg}"
                );
            }
            other => panic!("a zero-persona review must be rejected, got {other:?}"),
        }
    }

    /// cas-acf83: an envelope that says nothing about execution proves
    /// nothing. This is also what every hand-written envelope looks like.
    #[test]
    fn an_envelope_without_execution_evidence_cannot_pass_the_gate() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn f() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        req.code_review_findings = Some(envelope_without_execution_block());

        match run_code_review_gate(&t, &req, dir.path(), true) {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("REVIEW EXECUTION UNREPORTED"), "{msg}");
                assert!(msg.contains("execution.personas_run"), "{msg}");
            }
            other => panic!("an unreported-execution envelope must be rejected, got {other:?}"),
        }
    }

    /// cas-acf83 (GH #108): the reported outage, one lane short of total.
    /// Every always-on persona runs on the Codex transport; only `security` is
    /// Claude-hosted. So the same outage that produced `personas_run: 0` in the
    /// filed incident produces `personas_run: 1` whenever `security` is
    /// activated — and a check that only asked "did anything run" would wave
    /// that through with correctness, testing, maintainability and
    /// project-standards having never looked at the diff.
    #[test]
    fn a_partial_review_missing_mandatory_lanes_cannot_pass_the_gate() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn only_security_looked() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        let env = ReviewOutcome {
            residual: Vec::new(),
            pre_existing: Vec::new(),
            mode: "headless".to_string(),
            execution: Some(cas_types::ReviewExecution {
                personas_run: 1,
                personas_failed: vec![
                    "correctness: transport unavailable".to_string(),
                    "testing: transport unavailable".to_string(),
                    "maintainability: transport unavailable".to_string(),
                    "project-standards: transport unavailable".to_string(),
                ],
                skipped_reason: None,
                required_personas_missing: vec![
                    "correctness".to_string(),
                    "testing".to_string(),
                    "maintainability".to_string(),
                    "project-standards".to_string(),
                ],
            }),
        };
        req.code_review_findings = Some(serde_json::to_string(&env).unwrap());

        match run_code_review_gate(&t, &req, dir.path(), true) {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("REVIEW INCOMPLETE"), "{msg}");
                assert!(
                    msg.contains("correctness") && msg.contains("project-standards"),
                    "must name every mandatory lane that produced no verdict: {msg}"
                );
                assert!(
                    msg.contains("transport unavailable"),
                    "must surface the recorded failures: {msg}"
                );
            }
            other => panic!("a partial review must be rejected, got {other:?}"),
        }
    }

    /// cas-acf83: self-cert is symmetric for the partial case too.
    #[test]
    fn a_partial_review_cannot_self_certify_verification() {
        let env = ReviewOutcome {
            residual: Vec::new(),
            pre_existing: Vec::new(),
            mode: "headless".to_string(),
            execution: Some(cas_types::ReviewExecution {
                personas_run: 1,
                personas_failed: vec!["correctness: transport unavailable".to_string()],
                skipped_reason: None,
                required_personas_missing: vec!["correctness".to_string()],
            }),
        };
        assert!(!worker_review_envelope_is_clean(
            &serde_json::to_string(&env).unwrap()
        ));
    }

    /// cas-acf83: a review that DID run with no findings still passes — the
    /// gate must reject absent verdicts, not clean ones.
    #[test]
    fn a_review_that_ran_clean_still_passes_the_gate() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn f() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        req.code_review_findings = Some(autofix_envelope(vec![]));
        assert!(matches!(
            run_code_review_gate(&t, &req, dir.path(), true),
            CodeReviewGateOutcome::Proceed
        ));
    }

    /// cas-acf83: self-certification (the verification-jail bypass) must be
    /// symmetric with the gate — a zero-persona envelope cannot buy it either.
    #[test]
    fn a_review_that_never_ran_cannot_self_certify_verification() {
        assert!(
            !worker_review_envelope_is_clean(&envelope_that_did_not_execute(
                vec!["security: launch failed"],
                "all personas failed to launch",
            )),
            "a non-executed review must not satisfy worker-owned verification"
        );
        assert!(
            !worker_review_envelope_is_clean(&envelope_without_execution_block()),
            "an envelope with no execution evidence must not satisfy it either"
        );
        assert!(
            worker_review_envelope_is_clean(&autofix_envelope(vec![])),
            "a genuine clean review must still self-certify"
        );
    }

    #[test]
    fn docs_only_diff_skips_gate_without_findings() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("README.md", "new content\n"), ("docs/x.md", "x\n")]);
        let t = base_task();
        let req = base_req(&t.id); // no findings
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        assert!(
            matches!(out, CodeReviewGateOutcome::Proceed),
            "pure-docs diff must skip the review gate"
        );
    }

    #[test]
    fn code_change_without_findings_is_rejected_as_required() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let t = base_task();
        let req = base_req(&t.id);
        // supervisor_owned=true is the config default (cas-865b).
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("CODE_REVIEW_REQUIRED"));
                assert!(msg.contains("cas-code-review"));
                assert!(msg.contains("code_review_findings"));
                // cas-297e: supervisor-owned mode recommends interactive/headless.
                assert!(
                    msg.contains("mode=interactive"),
                    "supervisor-owned guidance must recommend interactive: {msg}"
                );
                assert!(
                    msg.contains("mode=headless"),
                    "supervisor-owned guidance must mention headless: {msg}"
                );
                assert!(
                    !msg.contains("mode=autofix and the current diff"),
                    "supervisor-owned guidance must not primary-recommend autofix: {msg}"
                );
                // Finding schema documented at the point of failure.
                assert!(
                    msg.contains("why_it_matters"),
                    "CODE_REVIEW_REQUIRED must document Finding fields: {msg}"
                );
            }
            other => panic!("expected CODE_REVIEW_REQUIRED reject, got {other:?}"),
        }
    }

    #[test]
    fn code_review_required_worker_owned_recommends_autofix() {
        // cas-297e AC3: owner=worker keeps the legacy autofix guidance.
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let t = base_task();
        let req = base_req(&t.id);
        let out = run_code_review_gate(&t, &req, dir.path(), false);
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("CODE_REVIEW_REQUIRED"));
                assert!(
                    msg.contains("mode=autofix"),
                    "worker-owned guidance must recommend autofix: {msg}"
                );
                assert!(
                    !msg.contains("mode=interactive"),
                    "worker-owned guidance must not recommend interactive: {msg}"
                );
            }
            other => panic!("expected CODE_REVIEW_REQUIRED reject, got {other:?}"),
        }
    }

    #[test]
    fn partial_finding_envelope_lists_all_missing_fields_once() {
        // cas-297e AC1+AC2: one response lists every missing Finding field
        // and documents the Finding schema.
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        req.code_review_findings = Some(
            r#"{
                "mode": "interactive",
                "residual": [{
                    "title": "partial finding",
                    "severity": "P2",
                    "file": "src/foo.rs",
                    "line": 1
                }],
                "pre_existing": []
            }"#
            .to_string(),
        );
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("MALFORMED REVIEW ENVELOPE"), "{msg}");
                for field in [
                    "why_it_matters",
                    "autofix_class",
                    "owner",
                    "confidence",
                    "evidence",
                    "pre_existing",
                ] {
                    assert!(
                        msg.contains(&format!("missing field `{field}`")),
                        "expected all-fields list to include `{field}`:\n{msg}"
                    );
                }
                // Schema hint documents required Finding keys.
                assert!(
                    msg.contains("Each Finding requires:"),
                    "error must document Finding required fields:\n{msg}"
                );
                assert!(msg.contains("residual[0]"), "{msg}");
            }
            other => panic!("expected MALFORMED reject, got {other:?}"),
        }
    }

    #[test]
    fn p0_residual_blocks_close() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        req.code_review_findings = Some(autofix_envelope(vec![p0_finding()]));
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("P0 BLOCK"));
                assert!(msg.contains("SQL injection"));
                assert!(msg.contains("bypass_code_review=true"));
            }
            other => panic!("expected P0 block, got {other:?}"),
        }
    }

    #[test]
    fn p2_residual_does_not_block_close() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        req.code_review_findings = Some(autofix_envelope(vec![p2_finding()]));
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        assert!(
            matches!(out, CodeReviewGateOutcome::Proceed),
            "P2 residual must route to Unit 8, not block close"
        );
    }

    #[test]
    fn empty_residual_with_envelope_allows_close() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn ok() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        req.code_review_findings = Some(autofix_envelope(Vec::new()));
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        assert!(matches!(out, CodeReviewGateOutcome::Proceed));
    }

    #[test]
    fn malformed_envelope_validation_failure_is_rejected() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn ok() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        // Whitespace-only mode passes serde but fails validate().
        req.code_review_findings =
            Some(r#"{"residual":[],"pre_existing":[],"mode":"   "}"#.to_string());
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("MALFORMED REVIEW ENVELOPE"));
            }
            other => panic!("expected malformed-envelope reject, got {other:?}"),
        }
    }

    #[test]
    fn unparseable_envelope_json_is_rejected() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn ok() {}\n")]);
        let t = base_task();
        let mut req = base_req(&t.id);
        req.code_review_findings = Some("not json at all".to_string());
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("MALFORMED REVIEW ENVELOPE"));
                assert!(msg.contains("failed to parse"));
                // cas-297e AC2: even parse failures document the Finding schema.
                assert!(
                    msg.contains("Each Finding requires:"),
                    "parse error must document Finding schema:\n{msg}"
                );
            }
            other => panic!("expected parse reject, got {other:?}"),
        }
    }

    #[test]
    fn supervisor_override_appends_decision_note() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let prev = std::env::var("CAS_AGENT_ROLE").ok();
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "supervisor");
        }

        let t = base_task();
        let mut req = base_req(&t.id);
        req.bypass_code_review = Some(true);
        req.reason = Some("P0 is a false positive, tracked in cas-xyz".to_string());

        let out = run_code_review_gate(&t, &req, dir.path(), true);

        unsafe {
            match prev {
                Some(v) => std::env::set_var("CAS_AGENT_ROLE", v),
                None => std::env::remove_var("CAS_AGENT_ROLE"),
            }
        }

        match out {
            CodeReviewGateOutcome::AppendDecisionNote(note) => {
                assert!(note.contains("DECISION"));
                assert!(note.contains("supervisor"));
                assert!(note.contains("false positive"));
            }
            other => panic!("expected AppendDecisionNote, got {other:?}"),
        }
    }

    #[test]
    fn non_supervisor_override_is_rejected() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let prev = std::env::var("CAS_AGENT_ROLE").ok();
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "worker");
        }

        let t = base_task();
        let mut req = base_req(&t.id);
        req.bypass_code_review = Some(true);

        let out = run_code_review_gate(&t, &req, dir.path(), true);

        unsafe {
            match prev {
                Some(v) => std::env::set_var("CAS_AGENT_ROLE", v),
                None => std::env::remove_var("CAS_AGENT_ROLE"),
            }
        }

        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("UNAUTHORIZED OVERRIDE"));
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn additive_only_plus_missing_findings_still_proceeds() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/evil.rs", "bad\n")]);
        let mut t = base_task();
        t.execution_note = Some("additive-only".to_string());
        let req = base_req(&t.id); // no findings, no override
        // additive-only short-circuits before the findings check.
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        assert!(matches!(out, CodeReviewGateOutcome::Proceed));
    }

    #[test]
    fn non_git_project_root_skips_gate() {
        let _g = env_lock();
        let dir = tempfile::tempdir().unwrap();
        let t = base_task();
        let req = base_req(&t.id);
        // Non-git dir → has_reviewable_changes returns false → skip.
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        assert!(matches!(out, CodeReviewGateOutcome::Proceed));
    }

    // --- cas-3086: persisted-envelope fallback ------------------------------

    #[test]
    fn persisted_envelope_satisfies_gate_when_req_missing() {
        // Simulates supervisor-close: the worker persisted a clean
        // envelope on a prior (jailed) close attempt; supervisor
        // calls close without re-running review and without
        // bypass_code_review=true.
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let mut t = base_task();
        t.deliverables.review_envelope = Some(autofix_envelope(Vec::new()));
        let req = base_req(&t.id); // no findings in request
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        assert!(
            matches!(out, CodeReviewGateOutcome::Proceed),
            "persisted clean envelope must let supervisor-close proceed without bypass"
        );
    }

    #[test]
    fn persisted_envelope_with_p0_still_blocks() {
        // Forwarding a receipt does not weaken the P0 gate.
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let mut t = base_task();
        t.deliverables.review_envelope = Some(autofix_envelope(vec![p0_finding()]));
        let req = base_req(&t.id);
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(msg.contains("P0 BLOCK"), "P0 must still block: {msg}");
            }
            other => panic!("expected P0 block on persisted envelope, got {other:?}"),
        }
    }

    #[test]
    fn request_envelope_takes_precedence_over_persisted() {
        // If the caller sends a fresh envelope, that's what the gate
        // sees — the persisted one is a fallback, not a merge.
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let mut t = base_task();
        // Persisted envelope has a P0 — would block if chosen.
        t.deliverables.review_envelope = Some(autofix_envelope(vec![p0_finding()]));
        let mut req = base_req(&t.id);
        // Request envelope is clean — should let the close proceed.
        req.code_review_findings = Some(autofix_envelope(Vec::new()));
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        assert!(
            matches!(out, CodeReviewGateOutcome::Proceed),
            "explicit request envelope must win over persisted fallback"
        );
    }

    #[test]
    fn persisted_malformed_envelope_is_rejected() {
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let mut t = base_task();
        t.deliverables.review_envelope = Some("not-json".to_string());
        let req = base_req(&t.id);
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        assert!(
            matches!(out, CodeReviewGateOutcome::Reject(_)),
            "malformed persisted envelope must be rejected, not silently bypassed"
        );
    }

    // --- cas-fef4 + cas-3086: epic_subtask_receipts_are_clean ----------------

    /// Build a subtask carrying a specific review envelope (JSON string).
    fn subtask_with_envelope(id: &str, envelope: Option<String>) -> Task {
        let mut t = Task {
            id: id.to_string(),
            title: format!("subtask {id}"),
            status: TaskStatus::Closed,
            ..Default::default()
        };
        t.deliverables.review_envelope = envelope;
        t
    }

    /// cas-acf83: like [`autofix_envelope`], these fixtures represent reviews
    /// that ran — the forgery under test is the finding classification, not a
    /// missing execution claim, so the P0 defences are proven on their own
    /// merits rather than being masked by the execution check.
    fn envelope_with_pre_existing(residual: Vec<Finding>, pre_existing: Vec<Finding>) -> String {
        let env = ReviewOutcome {
            residual,
            pre_existing,
            mode: "autofix".to_string(),
            execution: Some(cas_types::ReviewExecution {
                personas_run: 4,
                personas_failed: Vec::new(),
                skipped_reason: None,
                required_personas_missing: Vec::new(),
            }),
        };
        serde_json::to_string(&env).expect("serialize ReviewOutcome")
    }

    #[test]
    fn epic_receipts_clean_when_all_subtasks_have_empty_envelopes() {
        let subtasks = vec![
            subtask_with_envelope("s1", Some(autofix_envelope(Vec::new()))),
            subtask_with_envelope("s2", Some(autofix_envelope(Vec::new()))),
        ];
        assert!(
            epic_subtask_receipts_are_clean(&subtasks),
            "two clean subtask envelopes must cover the epic"
        );
    }

    #[test]
    fn epic_receipts_not_clean_when_no_subtasks() {
        // cas-3086: `_ => false` arm — an epic with zero subtasks has
        // nothing "covering" the union diff, so fall through to the
        // normal gate.
        assert!(!epic_subtask_receipts_are_clean(&[]));
    }

    #[test]
    fn epic_receipts_not_clean_when_subtask_has_residual_p0() {
        // cas-3086 defense-in-depth: a subtask envelope that somehow
        // leaked a residual P0 past its own close must NOT let the
        // epic bypass the gate.
        let subtasks = vec![
            subtask_with_envelope("s1", Some(autofix_envelope(Vec::new()))),
            subtask_with_envelope("s2", Some(autofix_envelope(vec![p0_finding()]))),
        ];
        assert!(
            !epic_subtask_receipts_are_clean(&subtasks),
            "residual-P0 on any subtask must disqualify the bypass"
        );
    }

    #[test]
    fn epic_receipts_not_clean_when_subtask_has_pre_existing_p0() {
        // cas-fef4 (this task): a worker supplying an envelope of shape
        // `{ residual: [], pre_existing: [<real_p0>] }` satisfies the
        // old cas-3086 check (residual is clean) but smuggles a real
        // P0 past the epic-close gate by reclassifying it as
        // "pre-existing". The tightened clean-receipt semantics must
        // reject this forgery and fall through to run_code_review_gate
        // on the union diff.
        let forged = envelope_with_pre_existing(Vec::new(), vec![p0_finding()]);
        let subtasks = vec![
            subtask_with_envelope("s1", Some(autofix_envelope(Vec::new()))),
            subtask_with_envelope("s2", Some(forged)),
        ];
        assert!(
            !epic_subtask_receipts_are_clean(&subtasks),
            "pre_existing-P0 smuggling must disqualify the bypass"
        );
    }

    #[test]
    fn epic_receipts_clean_when_pre_existing_is_only_subp0() {
        // Sanity check on the tightened check: non-P0 severities in
        // pre_existing (the normal case — legitimate low-severity
        // debt classified by the reviewer) must not block the bypass.
        let clean_with_low_pre = envelope_with_pre_existing(Vec::new(), vec![p2_finding()]);
        let subtasks = vec![subtask_with_envelope("s1", Some(clean_with_low_pre))];
        assert!(
            epic_subtask_receipts_are_clean(&subtasks),
            "pre_existing with only sub-P0 severities is legitimate and must not block bypass"
        );
    }

    #[test]
    fn epic_receipts_not_clean_when_subtask_envelope_missing_or_malformed() {
        // Missing envelope on any subtask → no structural proof → bypass declined.
        let subtasks = vec![
            subtask_with_envelope("s1", Some(autofix_envelope(Vec::new()))),
            subtask_with_envelope("s2", None),
        ];
        assert!(
            !epic_subtask_receipts_are_clean(&subtasks),
            "missing envelope on any subtask must disqualify the bypass"
        );

        let subtasks = vec![
            subtask_with_envelope("s1", Some(autofix_envelope(Vec::new()))),
            subtask_with_envelope("s2", Some("not-json".to_string())),
        ];
        assert!(
            !epic_subtask_receipts_are_clean(&subtasks),
            "malformed envelope on any subtask must disqualify the bypass"
        );
    }

    // --- cas-4c64: run_code_review_gate forgery defence (persisted path) ------
    //
    // These tests cover the two-step attack surface fixed in cas-4c64:
    //
    //   Step 1: Worker submits a forged envelope (P0 marked pre_existing=true in
    //           residual[], or P0 in pre_existing[]). `worker_review_envelope_is_clean`
    //           rejects it, so the short-circuit does NOT fire. The jail-arming
    //           `else` branch persists the envelope unconditionally to
    //           `task.deliverables.review_envelope`.
    //
    //   Step 2: Supervisor's task-verifier clears the jail.
    //
    //   Step 3: Worker retries close WITHOUT code_review_findings. The gate reads
    //           the persisted forged envelope. Before cas-4c64, `evaluate_gate`
    //           filtered `!f.pre_existing`, so the P0 was silently skipped →
    //           Allow → bypass. After cas-4c64, Check A/B fire unconditionally →
    //           Reject.
    //
    // Step 1 integration is covered by `test_worker_close_with_p0_residual_pre_existing_true_still_blocked`
    // in verification_flow.rs. Step 3 (the close-gate layer) is here.

    /// P0 finding with `pre_existing: true` — the per-finding flag that
    /// `evaluate_gate` filters on (`!f.pre_existing && f.severity == P0`),
    /// making it the attack vector before cas-4c64 Check A.
    fn p0_finding_pre_existing_true() -> Finding {
        Finding {
            pre_existing: true,
            ..p0_finding()
        }
    }

    #[test]
    fn persisted_envelope_with_p0_pre_existing_true_in_residual_is_blocked() {
        // Regression for cas-4c64 Check A: a persisted envelope whose
        // residual[] contains a P0 marked `pre_existing:true` must be
        // rejected even though `evaluate_gate` would have allowed it
        // (it filters `!f.pre_existing`). Check A catches all P0s in
        // residual[], regardless of the per-finding flag.
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let mut t = base_task();
        // Forged envelope: P0 in residual[] with pre_existing=true.
        let forged = envelope_with_pre_existing(vec![p0_finding_pre_existing_true()], Vec::new());
        t.deliverables.review_envelope = Some(forged);
        let req = base_req(&t.id); // no findings in request — reads persisted
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("P0 BLOCK"),
                    "must block on P0 even with pre_existing=true in residual[]: {msg}"
                );
            }
            other => panic!(
                "expected Reject for persisted forged P0-in-residual envelope, got {other:?}"
            ),
        }
    }

    #[test]
    fn persisted_envelope_with_p0_in_pre_existing_array_is_blocked() {
        // Regression for cas-4c64 Check B: a persisted envelope whose
        // pre_existing[] bucket contains a P0 must be rejected. Before
        // cas-4c64, `evaluate_gate` did not inspect this bucket at all —
        // only `residual[]` was checked. Check B closes that gap.
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/foo.rs", "fn new() {}\n")]);
        let mut t = base_task();
        // Envelope with P0 reclassified as pre-existing (the fef4 forgery).
        let forged = envelope_with_pre_existing(Vec::new(), vec![p0_finding()]);
        t.deliverables.review_envelope = Some(forged);
        let req = base_req(&t.id);
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("P0 in pre_existing"),
                    "must block on P0 in pre_existing[]: {msg}"
                );
            }
            other => {
                panic!("expected Reject for persisted P0-in-pre_existing[] envelope, got {other:?}")
            }
        }
    }

    #[test]
    fn two_step_forged_envelope_rejected_on_retry() {
        // Full two-step attack regression (cas-4c64 supervisor spec):
        //
        //   (1) Worker sends {residual:[{P0, pre_existing:true}]}.
        //       worker_review_envelope_is_clean → false (no short-circuit).
        //       Jail-arming branch persists the envelope unconditionally.
        //   (2) Supervisor verifies + clears jail (simulated here by
        //       setting t.deliverables.review_envelope directly).
        //   (3) Worker retries close without code_review_findings.
        //       Before cas-4c64: evaluate_gate skips pre_existing=true → Allow
        //       (bypass succeeds — the bug).
        //       After cas-4c64: Check A blocks on any P0 in residual[] → Reject
        //       (the fix).
        //
        // This test exercises step 3 only (the close-gate function boundary).
        // Step 1 integration is in verification_flow.rs
        // `test_worker_close_with_p0_residual_pre_existing_true_still_blocked`.
        let _g = env_lock();
        let dir = repo_with_staged(&[("src/auth.rs", "fn login() {}\n")]);
        let mut t = base_task();
        // Simulate the persisted forged envelope from step 1.
        let forged = serde_json::to_string(&ReviewOutcome {
            residual: vec![p0_finding_pre_existing_true()],
            pre_existing: Vec::new(),
            mode: "autofix".to_string(),
            // cas-acf83: a real review ran and produced this P0 — the forgery
            // is the pre_existing reclassification, not the execution claim.
            // Keeping it executed proves the P0 defence still fires on its own
            // merits rather than being masked by the new execution check.
            execution: Some(cas_types::ReviewExecution {
                personas_run: 4,
                personas_failed: Vec::new(),
                skipped_reason: None,
                required_personas_missing: Vec::new(),
            }),
        })
        .expect("serialize forged envelope");
        t.deliverables.review_envelope = Some(forged);
        // Step 3: worker retries without providing code_review_findings.
        let req = base_req(&t.id);
        let out = run_code_review_gate(&t, &req, dir.path(), true);
        match out {
            CodeReviewGateOutcome::Reject(msg) => {
                // Must block with P0 — not CODE_REVIEW_REQUIRED (which would
                // mislead the worker into thinking they just forgot to attach
                // findings).
                assert!(
                    msg.contains("P0 BLOCK"),
                    "two-step forged close must produce P0 BLOCK, not CODE_REVIEW_REQUIRED: {msg}"
                );
            }
            other => panic!("two-step forged close must be rejected, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod search_manifest_gate_tests {
    //! Unit tests for the cas-49f1 zero-hit search-manifest guardrail
    //! ([`run_search_manifest_gate`]). Covers the cas-94a3 regression
    //! scenario directly: a manifest where one search step (the
    //! operator-turn grep pattern) returned 0 hits while sibling greps
    //! against the same corpus returned hundreds.
    use super::*;

    fn spike_task() -> Task {
        Task {
            id: "cas-test-spike".to_string(),
            title: "investigation task".to_string(),
            status: TaskStatus::InProgress,
            task_type: TaskType::Spike,
            ..Default::default()
        }
    }

    fn code_task() -> Task {
        Task {
            id: "cas-test-code".to_string(),
            title: "code task".to_string(),
            status: TaskStatus::InProgress,
            task_type: TaskType::Task,
            ..Default::default()
        }
    }

    fn req_with_manifest(id: &str, manifest_json: Option<&str>) -> TaskCloseRequest {
        TaskCloseRequest {
            id: id.to_string(),
            reason: None,
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: manifest_json.map(str::to_string),
            commit_receipt: None,
        }
    }

    /// cas-94a3 regression: the `"type":"human"` grep returned 0 hits in
    /// every one of 4 project sweeps while the sibling `"type":"user"`
    /// grep returned 207. A manifest reporting that exact shape must
    /// produce a warning note, not a silent Proceed.
    #[test]
    fn cas_94a3_zero_hit_step_amid_nonzero_siblings_is_flagged() {
        let t = spike_task();
        let manifest = serde_json::json!([
            {"command": "grep -o '\"message\":{\"type\":\"human\"[^}]*}' project-a/*.jsonl", "hits": 0},
            {"command": "grep -c '\"type\":\"user\"' project-a/*.jsonl", "hits": 207},
        ])
        .to_string();
        let req = req_with_manifest(&t.id, Some(&manifest));
        match run_search_manifest_gate(&t, &req) {
            SearchManifestGateOutcome::AppendWarningNote(note) => {
                assert!(
                    note.contains("ZERO_HIT_SEARCH_WARNING"),
                    "note must be loud: {note}"
                );
                assert!(
                    note.contains("type\":\"human"),
                    "note must name the offending command: {note}"
                );
            }
            SearchManifestGateOutcome::Proceed => {
                panic!("a zero-hit search step must not pass silently")
            }
        }
    }

    #[test]
    fn manifest_with_all_nonzero_hits_proceeds_silently() {
        let t = spike_task();
        let manifest = serde_json::json!([
            {"command": "grep -c foo file", "hits": 3},
            {"command": "grep -c bar file", "hits": 12},
        ])
        .to_string();
        let req = req_with_manifest(&t.id, Some(&manifest));
        assert!(matches!(
            run_search_manifest_gate(&t, &req),
            SearchManifestGateOutcome::Proceed
        ));
    }

    #[test]
    fn missing_manifest_proceeds_silently_even_for_spike() {
        let t = spike_task();
        let req = req_with_manifest(&t.id, None);
        assert!(matches!(
            run_search_manifest_gate(&t, &req),
            SearchManifestGateOutcome::Proceed
        ));
    }

    /// AC3: ordinary code tasks must be entirely unaffected — the gate
    /// doesn't even look at the manifest for a non-Spike task, so a
    /// zero-hit entry there is never surfaced.
    #[test]
    fn ordinary_code_task_is_never_gated_even_with_zero_hit_manifest() {
        let t = code_task();
        let manifest = serde_json::json!([{"command": "grep -c foo file", "hits": 0}]).to_string();
        let req = req_with_manifest(&t.id, Some(&manifest));
        assert!(matches!(
            run_search_manifest_gate(&t, &req),
            SearchManifestGateOutcome::Proceed
        ));
    }

    #[test]
    fn malformed_manifest_json_is_flagged_not_silently_ignored() {
        let t = spike_task();
        let req = req_with_manifest(&t.id, Some("not json at all"));
        match run_search_manifest_gate(&t, &req) {
            SearchManifestGateOutcome::AppendWarningNote(note) => {
                assert!(note.contains("WARNING"), "note must warn: {note}");
            }
            SearchManifestGateOutcome::Proceed => {
                panic!("malformed manifest must not proceed silently")
            }
        }
    }
}

#[cfg(test)]
mod merge_state_gate_tests {
    //! Unit tests for the cas-95ce factory-branch merge-state close
    //! gate ([`run_factory_branch_merge_gate`]). The gate sits at
    //! `cas_task_close` line ~183, immediately after the existing
    //! [`check_unmerged_epic_branches`] guard for epic-type tasks, and
    //! BEFORE the cas-code-review gate / `bypass_code_review` plumbing.
    //!
    //! Why these tests are pure-helper instead of end-to-end
    //! `cas_task_close` calls:
    //!
    //! - The integration call site is mechanical (one
    //!   `pattern-match { Proceed => {} | Reject(msg) => return tool_error(msg) }`
    //!   block, mirroring the cas-code-review gate at line ~815).
    //! - Bypass-immunity is enforced **structurally**: the gate
    //!   function does not consume the bypass flag, and it runs at
    //!   the merge-state insertion (currently `cas_task_close`
    //!   ~line 184) — strictly upstream of the `bypass_code_review`
    //!   evaluation inside `run_code_review_gate`. The test sets
    //!   `req.bypass_code_review = Some(true)` and confirms the gate
    //!   still rejects, demonstrating bypass cannot reach this layer.
    //!
    //! Test layout mirrors `code_review_gate_tests` above.
    use super::*;
    use crate::test_support::TestEnvGuard;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Build a repo with `main` carrying one seed commit, branch off
    /// into `factory/<worker>`, and return the tempdir on the worker
    /// branch. Caller adds whatever commits it wants on top.
    fn init_factory_repo(worker: &str) -> TempDir {
        init_factory_repo_with_parent(worker, "main")
    }

    /// Same as [`init_factory_repo`] but the initial (parent) branch is
    /// named `parent_branch` instead of the hardcoded `main` — used by
    /// cas-c631's regression coverage to simulate a local-only
    /// `epic/<slug>` parent branch.
    fn init_factory_repo_with_parent(worker: &str, parent_branch: &str) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", parent_branch]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        git(p, &["checkout", "-q", "-b", &format!("factory/{worker}")]);
        dir
    }

    fn worker_task(assignee: &str) -> Task {
        Task {
            id: "cas-test1".to_string(),
            title: "worker task".to_string(),
            status: TaskStatus::InProgress,
            assignee: Some(assignee.to_string()),
            ..Default::default()
        }
    }

    fn epic_task(assignee: Option<&str>) -> Task {
        Task {
            id: "cas-epic".to_string(),
            title: "the epic".to_string(),
            status: TaskStatus::InProgress,
            task_type: TaskType::Epic,
            assignee: assignee.map(str::to_string),
            ..Default::default()
        }
    }

    fn base_req(id: &str) -> TaskCloseRequest {
        TaskCloseRequest {
            id: id.to_string(),
            reason: None,
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: None,
            commit_receipt: None,
        }
    }

    // --- The 6 named tests (per cas-95ce design / acceptance criteria) ----

    #[test]
    fn worker_task_close_rejects_when_factory_branch_unmerged() {
        // Worker committed two new files on factory/worker that never
        // landed on main. The gate must reject with stranded count + remediation.
        let dir = init_factory_repo("worker");
        std::fs::write(dir.path().join("a.rs"), "// a\n").unwrap();
        git(dir.path(), &["add", "a.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: a"]);
        std::fs::write(dir.path().join("b.rs"), "// b\n").unwrap();
        git(dir.path(), &["add", "b.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: b"]);

        let task = worker_task("worker");
        let req = base_req(&task.id);
        let mut env =
            TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_WORKER_CLI", Some("claude"))]);

        for (harness, coord) in [
            ("claude", "mcp__cas__coordination"),
            ("codex", "mcp__cs__coordination"),
            ("grok", "cas__coordination"),
        ] {
            env.set("CAS_FACTORY_WORKER_CLI", harness);
            let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());

            match out {
                MergeStateGateOutcome::Reject(msg) => {
                    assert!(msg.contains("MERGE REQUIRED"), "missing header: {msg}");
                    assert!(
                        msg.contains("factory/worker"),
                        "missing factory branch name: {msg}"
                    );
                    assert!(msg.contains("main"), "missing parent branch name: {msg}");
                    assert!(
                        msg.contains("2 commit"),
                        "expected stranded count of 2 in message (anchored to 'commit' \
                         to avoid weak digit-anywhere match): {msg}"
                    );
                    assert!(
                        msg.contains("bypass_code_review=true"),
                        "remediation must call out bypass-immunity: {msg}"
                    );
                    assert!(
                        msg.contains("Open a PR targeting main"),
                        "plain (non-epic) parent branch must keep the PR-based \
                         remediation unchanged: {msg}"
                    );
                    assert!(
                        msg.contains("task action=request_changes")
                            && msg.contains("reason="),
                        "declined-review remediation must name the supervisor verdict path: {msg}"
                    );
                    assert!(
                        msg.contains(&format!("`{coord} action=inbox_poll`"))
                            && msg.contains("`No unread messages`")
                            && msg.contains("at most 10 rows")
                            && msg.contains("polling claim is at-most-once"),
                        "{harness} remediation must use its harness-resolved inbox API, \
                         require drain-until-empty polling, and disclose at-most-once \
                         claim semantics: {msg}"
                    );
                    let poll = msg.find("action=inbox_poll").expect("poll step");
                    let push = msg.find("Push factory/worker").expect("push step");
                    let pr = msg.find("Open a PR targeting main").expect("PR step");
                    assert!(
                        poll < push && poll < pr,
                        "polling must precede push/escalation steps for {harness}: {msg}"
                    );
                }
                other => panic!("expected Reject for stranded factory branch, got {other:?}"),
            }
        }
    }

    #[test]
    fn worker_task_close_on_local_epic_branch_points_at_supervisor_merge_not_pr() {
        // cas-c631: when the parent branch is a local-only `epic/<slug>`
        // branch (the supervisor's EPIC workflow convention — never pushed
        // to origin), the remediation must NOT tell the worker to open a PR
        // against it (that `gh pr create --base epic/<slug>` call fails with
        // no matching origin ref — the exact recurring friction this task
        // fixes). It must instead hand the worker a push + supervisor-merge
        // handoff.
        let parent = "epic/epic-triage-fix-the-docs-requests-bug-backlog-veri-cas-fff9";
        let dir = init_factory_repo_with_parent("worker", parent);
        std::fs::write(dir.path().join("a.rs"), "// a\n").unwrap();
        git(dir.path(), &["add", "a.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: a"]);
        let expected_tip = resolve_branch_sha(dir.path(), "factory/worker")
            .expect("factory branch tip should resolve");

        let task = worker_task("worker");
        let req = base_req(&task.id);
        let mut env =
            TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_WORKER_CLI", Some("claude"))]);

        for (harness, coord) in [
            ("claude", "mcp__cas__coordination"),
            ("codex", "mcp__cs__coordination"),
            ("grok", "cas__coordination"),
        ] {
            env.set("CAS_FACTORY_WORKER_CLI", harness);
            let out = run_factory_branch_merge_gate(&task, &req, parent, dir.path());

            match out {
                MergeStateGateOutcome::Reject(msg) => {
                    assert!(msg.contains("MERGE REQUIRED"), "missing header: {msg}");
                    assert!(
                        msg.contains("factory/worker"),
                        "missing factory branch name: {msg}"
                    );
                    assert!(msg.contains(parent), "missing parent branch name: {msg}");
                    assert!(
                        msg.contains("bypass_code_review=true"),
                        "remediation must still call out bypass-immunity: {msg}"
                    );
                    assert!(
                        !msg.contains("Open a PR targeting"),
                        "must NOT tell the worker to open a PR against a local-only \
                         epic branch: {msg}"
                    );
                    assert!(
                        msg.contains("do NOT run `gh pr create"),
                        "must explicitly warn against gh pr create on the missing \
                         origin ref: {msg}"
                    );
                    assert!(
                        msg.contains("supervisor to merge"),
                        "must hand the worker a supervisor-merge-request handoff: {msg}"
                    );
                    assert!(
                        msg.contains(&format!("`{coord} action=inbox_poll`"))
                            && msg.contains(&format!("`{coord} action=message"))
                            && msg.contains("`No unread messages`")
                            && msg.contains("at most 10 rows")
                            && msg.contains("without consuming daemon transport delivery")
                            && msg.contains("polling claim is at-most-once"),
                        "{harness} remediation must use its harness-resolved inbox and \
                         message APIs, require drain-until-empty polling, and disclose \
                         at-most-once claim semantics: {msg}"
                    );
                    assert!(
                        msg.contains(&expected_tip),
                        "escalation template must include the current branch tip {expected_tip}: {msg}"
                    );
                    assert!(
                        msg.contains(&format!("task_id={}", task.id)),
                        "structured merge request must carry the parked task identity: {msg}"
                    );
                    assert!(
                        msg.contains(
                            "Fresh after draining unread inbox messages until No unread messages"
                        ) && msg.contains("re-check reachability"),
                        "escalation must identify its freshness window and ask the supervisor to re-check: {msg}"
                    );
                    let poll = msg.find("action=inbox_poll").expect("poll step");
                    let push = msg.find("Push factory/worker").expect("push step");
                    let escalation = msg.find("action=message").expect("escalation step");
                    assert!(
                        poll < push && poll < escalation,
                        "polling must precede push/escalation steps for {harness}: {msg}"
                    );
                }
                other => panic!("expected Reject for stranded factory branch, got {other:?}"),
            }
        }
    }

    #[test]
    fn worker_task_close_succeeds_when_factory_branch_merged() {
        // factory/worker has no commits beyond main → 0 stranded → Proceed.
        let dir = init_factory_repo("worker");
        let task = worker_task("worker");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "fully-merged factory branch must allow close, got {out:?}"
        );
    }

    #[test]
    fn worker_task_close_with_bypass_still_rejects_on_unmerged() {
        // Confirms `bypass_code_review=true` does NOT skip the
        // merge-state guard. Demonstrated at the type level — the
        // gate function does not consume the bypass flag — and at
        // the behavioral level by setting bypass=Some(true) on the
        // request and asserting the gate still rejects.
        let dir = init_factory_repo("worker");
        std::fs::write(dir.path().join("evil.rs"), "// stranded\n").unwrap();
        git(dir.path(), &["add", "evil.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: stranded"]);

        let task = worker_task("worker");
        let mut req = base_req(&task.id);
        req.bypass_code_review = Some(true);
        req.reason = Some("supervisor wants to skip review".to_string());

        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        match out {
            MergeStateGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("bypass_code_review=true"),
                    "rejection message must spell out bypass-immunity policy: {msg}"
                );
            }
            other => panic!("bypass_code_review must NOT skip merge-state guard, got {other:?}"),
        }
    }

    #[test]
    fn worker_task_close_skipped_for_epic_type() {
        // The epic-close path is owned by check_unmerged_epic_branches
        // (line 161-182) which works at the epic-id branch namespace.
        // This per-task guard MUST NOT fire on epic-type tasks even
        // if their `assignee` field happens to be set (e.g.,
        // supervisor self-assigned an epic).
        let dir = init_factory_repo("worker");
        std::fs::write(dir.path().join("c.rs"), "// c\n").unwrap();
        git(dir.path(), &["add", "c.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: c"]);

        let task = epic_task(Some("worker"));
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "epic-type task must skip the per-task guard, got {out:?}"
        );
    }

    #[test]
    fn worker_task_close_skipped_for_no_assignee() {
        // Orphan tasks have no factory branch convention to resolve.
        // The guard must Proceed (covered by NoAssignee verification
        // skip elsewhere; here we just need it not to false-reject).
        let dir = init_factory_repo("worker");
        // Stranded commits exist on factory/worker, but our task has no assignee.
        std::fs::write(dir.path().join("d.rs"), "// d\n").unwrap();
        git(dir.path(), &["add", "d.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: d"]);

        let mut task = worker_task("worker");
        task.assignee = None;
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "no-assignee task must skip the guard, got {out:?}"
        );
    }

    #[test]
    fn worker_task_close_handles_missing_factory_branch() {
        // Worker convention is `factory/<assignee>`, but for a fresh
        // repo where no such branch ref exists, the guard must
        // Proceed (treat-as-merged) instead of false-rejecting. This
        // mirrors check_additive_only_branch_violations' graceful
        // degradation when git can't reason about history.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        // Note: no `factory/ghost` branch is created.

        let task = worker_task("ghost");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "missing factory branch must be treated as merged (graceful pass), got {out:?}"
        );
    }

    // --- cas-e74c (GH #80 / #62 symptoms 3-4): delivery-scoped guard -------
    //
    // The guard used to evaluate the worker's ENTIRE registered lane
    // branch. Three real deadlocks followed: a cherry-pick delivery from a
    // reused lane could never close even with a valid, target-reachable
    // `commit_receipt`; a zero-commit task inherited the lane's unrelated
    // commits; and work done on a clean task-local branch, merged before
    // close, still bounced because the guard keyed on the lane name.

    /// Commit with an explicit committer/author date so a test can place
    /// commits before or after a task's attribution window.
    fn git_at(dir: &std::path::Path, args: &[&str], date: &str) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn commit_file_at(dir: &std::path::Path, name: &str, body: &str, date: &str) {
        std::fs::write(dir.join(name), body).unwrap();
        git_at(dir, &["add", name], date);
        git_at(dir, &["commit", "-q", "-m", &format!("feat: {name}")], date);
    }

    fn window_at(epoch: i64, basis: &'static str) -> TaskCommitReceiptWindow {
        TaskCommitReceiptWindow {
            not_before: chrono::DateTime::from_timestamp(epoch, 0).unwrap(),
            basis,
            task_floor: chrono::DateTime::from_timestamp(epoch, 0).unwrap(),
            identity: TaskCommitIdentity::default(),
        }
    }

    fn head_sha(dir: &std::path::Path) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .expect("git rev-parse");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// GH #80: the lane carries prior-session commits the supervisor
    /// deliberately refused to merge; the task's own work was cherry-picked
    /// onto the parent and handed back as a `commit_receipt`. The receipt IS
    /// the delivery evidence — close must pass, logging the lane residue.
    #[test]
    fn reused_lane_close_with_valid_receipt_proceeds_with_residue_note() {
        let dir = init_factory_repo("worker");
        let p = dir.path();
        // Two prior-session commits stranded on the reused lane.
        commit_file_at(p, "old-a.rs", "// a\n", "2020-01-01T00:00:00Z");
        commit_file_at(p, "old-b.rs", "// b\n", "2020-01-02T00:00:00Z");
        // This task's delivery, cherry-picked onto main as a new SHA.
        git(p, &["checkout", "-q", "main"]);
        commit_file_at(p, "delivery.rs", "// delivered\n", "2026-08-04T12:00:00Z");
        let receipt = head_sha(p);
        git(p, &["checkout", "-q", "factory/worker"]);

        let task = worker_task("worker");
        let mut req = base_req(&task.id);
        req.commit_receipt = Some(receipt.clone());
        let window = window_at(1_000_000_000, "latest task lease claim/transfer");

        let out = run_factory_branch_merge_gate_with_attribution(
            &task,
            &req,
            "main",
            p,
            TaskCommitAttribution {
                receipt: Some(&receipt),
                window: Some(&window),
            },
        );
        match out {
            MergeStateGateOutcome::ProceedWithNote(note) => {
                assert!(
                    note.contains(&receipt),
                    "note must record the accepted receipt: {note}"
                );
                assert!(
                    note.contains("factory/worker") && note.contains("2 commit"),
                    "note must log the unmerged lane residue: {note}"
                );
            }
            other => panic!("valid receipt must clear the lane guard, got {other:?}"),
        }
    }

    /// GH #62 symptom 4: the delivery lives on a clean task-local branch that
    /// was merged into the parent BEFORE close. The guard must resolve merge
    /// state from the receipt's ancestry, not the registered lane name.
    #[test]
    fn clean_task_local_branch_merged_before_close_proceeds() {
        let dir = init_factory_repo("worker");
        let p = dir.path();
        commit_file_at(p, "unrelated.rs", "// other task\n", "2020-03-01T00:00:00Z");
        // Task work on its own branch, cut from main and merged into main.
        git(p, &["checkout", "-q", "main"]);
        git(p, &["checkout", "-q", "-b", "factory/worker-cas-test1"]);
        commit_file_at(p, "scoped.rs", "// scoped\n", "2026-08-04T12:00:00Z");
        let receipt = head_sha(p);
        git(p, &["checkout", "-q", "main"]);
        git(
            p,
            &[
                "merge",
                "-q",
                "--no-ff",
                "factory/worker-cas-test1",
                "-m",
                "merge",
            ],
        );
        git(p, &["checkout", "-q", "factory/worker"]);

        let task = worker_task("worker");
        let mut req = base_req(&task.id);
        req.commit_receipt = Some(receipt.clone());
        let window = window_at(1_000_000_000, "latest task lease claim/transfer");

        let out = run_factory_branch_merge_gate_with_attribution(
            &task,
            &req,
            "main",
            p,
            TaskCommitAttribution {
                receipt: Some(&receipt),
                window: Some(&window),
            },
        );
        assert!(
            matches!(out, MergeStateGateOutcome::ProceedWithNote(_)),
            "receipt merged into parent before close must clear the guard, got {out:?}"
        );
    }

    /// GH #62 symptom 3: an epic-less, zero-commit task inherited the lane's
    /// 34 unrelated commits. No commit is attributable to this task's work
    /// cycle, so the guard must not fire.
    #[test]
    fn zero_task_attributable_commits_proceeds_despite_lane_residue() {
        let dir = init_factory_repo("worker");
        let p = dir.path();
        commit_file_at(p, "other-1.rs", "// 1\n", "2020-01-01T00:00:00Z");
        commit_file_at(p, "other-2.rs", "// 2\n", "2020-01-02T00:00:00Z");

        let task = worker_task("worker");
        let req = base_req(&task.id);
        // Work cycle started long after those commits were made.
        let window = window_at(1_700_000_000, "latest task lease claim/transfer");

        let out = run_factory_branch_merge_gate_with_attribution(
            &task,
            &req,
            "main",
            p,
            TaskCommitAttribution {
                receipt: None,
                window: Some(&window),
            },
        );
        match out {
            MergeStateGateOutcome::ProceedWithNote(note) => {
                assert!(
                    note.contains("2 commit"),
                    "residue must be logged, not fatal: {note}"
                );
            }
            other => {
                panic!("zero task-attributable commits must not trip the guard, got {other:?}")
            }
        }
    }

    /// The scoping must not become a bypass: commits made inside the task's
    /// own work cycle and left unmerged still reject, and the count reported
    /// is the task-attributable one (not the whole lane).
    #[test]
    fn task_attributable_unmerged_commits_still_reject() {
        let dir = init_factory_repo("worker");
        let p = dir.path();
        commit_file_at(p, "other-1.rs", "// 1\n", "2020-01-01T00:00:00Z");
        commit_file_at(p, "mine.rs", "// mine\n", "2026-08-04T12:00:00Z");

        let task = worker_task("worker");
        let req = base_req(&task.id);
        let window = window_at(1_700_000_000, "latest task lease claim/transfer");

        let out = run_factory_branch_merge_gate_with_attribution(
            &task,
            &req,
            "main",
            p,
            TaskCommitAttribution {
                receipt: None,
                window: Some(&window),
            },
        );
        match out {
            MergeStateGateOutcome::Reject(msg) => {
                assert!(msg.contains("MERGE REQUIRED"), "missing header: {msg}");
                assert!(
                    msg.contains("1 commit(s) from this task"),
                    "rejection must count only task-attributable commits: {msg}"
                );
            }
            other => panic!("unmerged task-own commits must still reject, got {other:?}"),
        }
    }

    /// A receipt that does not validate (here: not reachable from the parent)
    /// must NOT clear the guard.
    #[test]
    fn invalid_receipt_does_not_clear_the_guard() {
        let dir = init_factory_repo("worker");
        let p = dir.path();
        commit_file_at(p, "mine.rs", "// mine\n", "2026-08-04T12:00:00Z");
        let receipt = head_sha(p); // on the lane, never merged to main

        let task = worker_task("worker");
        let mut req = base_req(&task.id);
        req.commit_receipt = Some(receipt.clone());
        let window = window_at(1_000_000_000, "latest task lease claim/transfer");

        let out = run_factory_branch_merge_gate_with_attribution(
            &task,
            &req,
            "main",
            p,
            TaskCommitAttribution {
                receipt: Some(&receipt),
                window: Some(&window),
            },
        );
        match out {
            MergeStateGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("commit_receipt"),
                    "rejection should explain why the supplied receipt was not accepted: {msg}"
                );
            }
            other => panic!("unmerged receipt must not clear the guard, got {other:?}"),
        }
    }

    #[test]
    fn attributable_count_is_none_when_git_state_is_unknowable() {
        let dir = tempfile::tempdir().unwrap();
        let window = window_at(1_700_000_000, "test");
        assert!(
            count_task_attributable_unmerged_commits(dir.path(), "factory/x", "main", &window)
                .is_none(),
            "non-git dir must be Unknown (fall back to whole-branch count), not Some(0)"
        );
    }

    // --- cas-fdc9 (GH #66 / #56): target-ref resolution ---------------------
    //
    // #66: the guard measured "N commits not on staging" against the LOCAL
    // staging ref, which a factory worktree never advances. A worker was told
    // 9 commits were unmerged when exactly one was — and the printed
    // remediation said to `git fetch`, which updates `origin/staging` and
    // never moves the local branch the guard was reading. The advice could
    // not fix the measurement, so workers chased merges that had landed.
    //
    // #56: a receipt naming a commit that does not exist in the repository
    // the close is bound to was accepted as evidence.

    /// Build a repo with an `origin` remote so the guard can see both a local
    /// and a remote-tracking ref for `main`. Returns (worktree, origin).
    fn init_repo_with_origin(worker: &str) -> (TempDir, TempDir) {
        let origin = tempfile::tempdir().unwrap();
        git(origin.path(), &["init", "-q", "--bare", "-b", "main"]);
        let dir = init_factory_repo(worker);
        git(
            dir.path(),
            &["remote", "add", "origin", origin.path().to_str().unwrap()],
        );
        git(dir.path(), &["push", "-q", "origin", "main"]);
        git(dir.path(), &["fetch", "-q", "origin"]);
        (dir, origin)
    }

    /// Advance `origin/main` beyond the local `main` ref, optionally merging
    /// the worker's branch into it. The local `main` ref is deliberately left
    /// where it was — that staleness is the whole bug.
    fn advance_origin_main(dir: &std::path::Path, extra_commits: usize, merge_factory: Option<&str>) {
        git(dir, &["checkout", "-q", "-b", "origin-work", "main"]);
        for i in 0..extra_commits {
            let name = format!("other_{i}.rs");
            std::fs::write(dir.join(&name), format!("// other {i}\n")).unwrap();
            git(dir, &["add", &name]);
            git(dir, &["commit", "-q", "-m", &format!("feat: other {i}")]);
        }
        if let Some(branch) = merge_factory {
            git(dir, &["merge", "-q", "--no-ff", branch, "-m", "merge worker"]);
        }
        git(dir, &["push", "-q", "origin", "origin-work:main"]);
        git(dir, &["fetch", "-q", "origin"]);
        git(dir, &["checkout", "-q", branch_of_first_factory(dir)]);
    }

    /// The factory branch created by `init_factory_repo` — resolved back from
    /// the repo so the helper above can return to it.
    fn branch_of_first_factory(dir: &std::path::Path) -> &'static str {
        let _ = dir;
        "factory/worker"
    }

    #[test]
    fn merged_work_is_zero_ahead_once_origin_is_consulted_even_with_a_stale_local_ref() {
        let (dir, _origin) = init_repo_with_origin("worker");
        let p = dir.path();
        std::fs::write(p.join("mine.rs"), "// mine\n").unwrap();
        git(p, &["add", "mine.rs"]);
        git(p, &["commit", "-q", "-m", "feat: mine"]);
        // Someone else advanced origin/main 8 times and merged this branch.
        advance_origin_main(p, 8, Some("factory/worker"));

        assert!(
            count_unmerged_factory_commits(p, "factory/worker", "main") > 0,
            "precondition: measured against the stale LOCAL ref the work looks unmerged"
        );
        assert_eq!(
            count_unmerged_against_targets(p, "factory/worker", "main"),
            Some(0),
            "work merged into origin/main must read as zero ahead"
        );
    }

    #[test]
    fn reported_count_excludes_commits_already_on_the_remote_target() {
        // The #66 shape: local ref stale by 8 commits, exactly one commit of
        // this branch genuinely unmerged. The old measurement reported the
        // stale-base arithmetic; the fix must report 1.
        let (dir, _origin) = init_repo_with_origin("worker");
        let p = dir.path();
        advance_origin_main(p, 8, None);
        // The worker syncs onto the advanced remote target (fast-forward, so
        // no extra merge commit), then makes exactly one commit of their own.
        // The LOCAL `main` ref still points at the pre-advance tip.
        git(p, &["merge", "-q", "--ff-only", "origin/main"]);
        std::fs::write(p.join("mine.rs"), "// mine\n").unwrap();
        git(p, &["add", "mine.rs"]);
        git(p, &["commit", "-q", "-m", "feat: mine"]);

        let local = count_unmerged_factory_commits(p, "factory/worker", "main");
        assert!(
            local > 1,
            "precondition: the stale local ref inflates the count (got {local})"
        );
        assert_eq!(
            count_unmerged_against_targets(p, "factory/worker", "main"),
            Some(1),
            "only the genuinely unmerged commit may be counted"
        );
    }

    #[test]
    fn target_count_matches_local_when_no_remote_ref_exists() {
        let dir = init_factory_repo("worker");
        let p = dir.path();
        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: a"]);
        assert_eq!(
            count_unmerged_against_targets(p, "factory/worker", "main"),
            Some(1),
            "a local-only epic branch has no origin ref and must measure locally"
        );
    }

    #[test]
    fn target_count_is_unknown_when_git_cannot_answer() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            count_unmerged_against_targets(dir.path(), "factory/worker", "main"),
            None,
            "unknowable git state must stay Unknown, never a manufactured zero"
        );
    }

    #[test]
    fn merge_required_reports_the_remote_aware_count_and_no_fetch_advice() {
        let (dir, _origin) = init_repo_with_origin("worker");
        let p = dir.path();
        advance_origin_main(p, 8, None);
        git(p, &["merge", "-q", "--ff-only", "origin/main"]);
        std::fs::write(p.join("mine.rs"), "// mine\n").unwrap();
        git(p, &["add", "mine.rs"]);
        git(p, &["commit", "-q", "-m", "feat: mine"]);

        let task = worker_task("worker");
        let req = base_req(&task.id);
        match run_factory_branch_merge_gate(&task, &req, "main", p) {
            MergeStateGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("1 commit(s)"),
                    "count must be measured against the remote target, not the stale \
                     local ref: {msg}"
                );
                assert!(
                    !msg.contains("git fetch --prune` if it was already merged"),
                    "must not tell the worker a fetch fixes a stale LOCAL branch ref — \
                     fetch never moves it, and following that advice manufactures \
                     merge requests for work already landed: {msg}"
                );
                assert!(
                    msg.contains("origin/main"),
                    "the refusal must name the remote ref it measured against: {msg}"
                );
            }
            other => panic!("genuinely unmerged commit must still reject, got {other:?}"),
        }
    }

    // --- GH #56: a receipt must exist in the repository the close is bound to

    #[test]
    fn receipt_absent_from_the_bound_repo_is_refused_not_silently_accepted() {
        let (dir, _origin) = init_repo_with_origin("worker");
        let other_repo = init_factory_repo("elsewhere");
        std::fs::write(other_repo.path().join("cross.rs"), "// cross-repo\n").unwrap();
        git(other_repo.path(), &["add", "cross.rs"]);
        git(other_repo.path(), &["commit", "-q", "-m", "feat: cross"]);
        let foreign_sha = {
            let out = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(other_repo.path())
                .output()
                .expect("git rev-parse");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        let refusal = commit_receipt_repo_binding_error(dir.path(), &foreign_sha)
            .expect("a receipt that does not exist in the bound repo must be refused");
        assert!(
            refusal.contains(&foreign_sha[..12]),
            "refusal must name the receipt it could not find: {refusal}"
        );
        assert!(
            refusal.contains("target_repo"),
            "refusal must point cross-repo work at the declared target repo: {refusal}"
        );

        // A receipt that does resolve locally is not refused by this check —
        // ancestry/window semantics stay with the existing gates.
        let local_sha = {
            let out = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(dir.path())
                .output()
                .expect("git rev-parse");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        assert!(
            commit_receipt_repo_binding_error(dir.path(), &local_sha).is_none(),
            "a resolvable receipt must pass the repo-binding check"
        );
    }

    #[test]
    fn receipt_repo_binding_check_accepts_an_unambiguous_abbreviation() {
        // GH #57 is already fixed (abbreviations resolve); the binding check
        // must not regress that by demanding a full SHA.
        let (dir, _origin) = init_repo_with_origin("worker");
        let out = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(dir.path())
            .output()
            .expect("git rev-parse");
        let short = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert!(
            commit_receipt_repo_binding_error(dir.path(), &short).is_none(),
            "short receipt `{short}` must resolve in the bound repo"
        );
    }

    // --- Lower-level coverage on count_unmerged_factory_commits -------------

    #[test]
    fn count_returns_zero_for_non_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            count_unmerged_factory_commits(dir.path(), "factory/x", "main"),
            0,
            "non-git dir must degrade to 0"
        );
    }

    #[test]
    fn count_matches_committed_delta() {
        let dir = init_factory_repo("worker");
        // 3 commits on factory/worker beyond main.
        for (i, name) in ["x.rs", "y.rs", "z.rs"].iter().enumerate() {
            std::fs::write(dir.path().join(name), format!("// {i}\n")).unwrap();
            git(dir.path(), &["add", name]);
            git(
                dir.path(),
                &["commit", "-q", "-m", &format!("feat: {name}")],
            );
        }
        assert_eq!(
            count_unmerged_factory_commits(dir.path(), "factory/worker", "main"),
            3,
            "count must equal commits on factory/worker beyond main"
        );
    }

    // --- cas-cf64 (P3): option-injection hardening --------------------------

    #[test]
    fn is_safe_git_refname_rejects_leading_dash_and_empty() {
        assert!(!is_safe_git_refname("-oProxyCommand=evil"));
        assert!(!is_safe_git_refname("--upload-pack=evil"));
        assert!(!is_safe_git_refname("-"));
        assert!(!is_safe_git_refname(""));
        assert!(is_safe_git_refname("factory/worker"));
        assert!(is_safe_git_refname("epic/some-slug"));
        assert!(is_safe_git_refname("main"));
    }

    #[test]
    fn count_unmerged_factory_commits_fails_closed_on_unsafe_refname() {
        let dir = init_factory_repo("worker");
        // A leading-dash "branch name" must never reach the git shell-out —
        // fails CLOSED (u32::MAX, forcing Reject upstream), the opposite of
        // this function's normal graceful-degrade-to-0 posture, because an
        // invalid ref here signals corrupted data or an injection attempt,
        // not an ordinary unresolvable ref.
        assert_eq!(
            count_unmerged_factory_commits(dir.path(), "-oProxyCommand=evil", "main"),
            u32::MAX,
            "unsafe factory_branch must fail closed, not silently degrade to 0"
        );
        assert_eq!(
            count_unmerged_factory_commits(dir.path(), "factory/worker", "-oProxyCommand=evil"),
            u32::MAX,
            "unsafe parent_branch must fail closed, not silently degrade to 0"
        );
    }

    #[test]
    fn merge_gate_rejects_unsafe_assignee_or_parent_branch_with_clear_message() {
        let dir = init_factory_repo("worker");
        std::fs::write(dir.path().join("a.rs"), "// a\n").unwrap();
        git(dir.path(), &["add", "a.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: a"]);

        // Unsafe parent_branch (as would be produced by a corrupted epic
        // `branch` field or a malformed API call).
        let task = worker_task("worker");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "-oProxyCommand=evil", dir.path());
        match out {
            MergeStateGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("INVALID BRANCH NAME"),
                    "must give a clear, actionable error, not a confusing \
                     huge-stranded-count message: {msg}"
                );
            }
            other => panic!("unsafe parent_branch must Reject, got {other:?}"),
        }

        // Unsafe assignee.
        let mut task = worker_task("-oProxyCommand=evil");
        task.assignee = Some("-oProxyCommand=evil".to_string());
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        match out {
            MergeStateGateOutcome::Reject(ref msg) if msg.contains("INVALID BRANCH NAME") => {}
            other => panic!("unsafe assignee must Reject with a clear message, got {other:?}"),
        }
    }

    // --- cas-4b3f (AC b): anchor to the task's own commits, not HEAD -------

    /// Reproduces BUG-close-guard-branch-head-not-task-commits.md exactly:
    /// worker commits task A's work, the gate rejects (first attempt) and
    /// the caller snapshots the branch tip as `factory_branch_anchor`, the
    /// supervisor merges that tip into `main` via `--no-ff`, and THEN the
    /// worker starts task B serially on the *same* `factory/worker` branch
    /// before task A's close is retried. Without the anchor fix, task A's
    /// retry recomputes against the CURRENT branch HEAD (which now carries
    /// task B's unrelated, still-unmerged commit) and false-rejects even
    /// though task A's own commits are demonstrably in `main`.
    #[test]
    fn serial_second_task_on_same_branch_does_not_restrand_first_tasks_close() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        // Task A's commit.
        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task A"]);
        let task_a_tip = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(p)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Supervisor merges factory/worker (task A's tip) into main.
        git(p, &["checkout", "-q", "main"]);
        git(p, &["merge", "-q", "--no-ff", "factory/worker"]);
        git(p, &["checkout", "-q", "factory/worker"]);

        // Worker starts task B serially on the SAME branch before task A's
        // close is retried.
        std::fs::write(p.join("b.rs"), "// b (task B, unrelated)\n").unwrap();
        git(p, &["add", "b.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task B (unmerged)"]);

        // Sanity: without an anchor, the raw branch-name check DOES
        // (wrongly) see task B's commit as stranding the branch — this
        // pins the bug's precondition so the test can't silently pass
        // for the wrong reason.
        assert_eq!(
            count_unmerged_factory_commits(p, "factory/worker", "main"),
            1,
            "precondition: branch HEAD carries exactly task B's 1 unmerged commit"
        );

        // Task A's close, anchored to the tip captured at its first
        // rejection (simulating what `park_task_awaiting_merge` recorded).
        // Status must be AwaitingMerge too — cas-cf64 only trusts the
        // anchor in that status, matching what park_task_awaiting_merge
        // actually persists (both fields together, in the same update).
        let mut task_a = worker_task("worker");
        task_a.status = TaskStatus::AwaitingMerge;
        task_a.deliverables.factory_branch_anchor = Some(task_a_tip);
        let req = base_req(&task_a.id);
        let out = run_factory_branch_merge_gate(&task_a, &req, "main", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "task A's close must succeed once its OWN commits are merged, \
             regardless of task B's later unmerged work on the same branch, \
             got {out:?}"
        );
    }

    #[test]
    fn stale_anchor_that_no_longer_resolves_falls_back_to_branch_head() {
        // If the recorded anchor sha is garbage (e.g. from a rewritten
        // history), the gate must gracefully fall back to the live branch
        // name rather than erroring or silently Proceeding.
        let dir = init_factory_repo("worker");
        std::fs::write(dir.path().join("a.rs"), "// a\n").unwrap();
        git(dir.path(), &["add", "a.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: a"]);

        let mut task = worker_task("worker");
        task.deliverables.factory_branch_anchor =
            Some("0000000000000000000000000000000000dead".to_string());
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        match out {
            MergeStateGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("1 commit"),
                    "must fall back to counting the live branch's 1 stranded commit: {msg}"
                );
            }
            other => panic!("expected Reject via branch-name fallback, got {other:?}"),
        }
    }

    #[test]
    fn no_anchor_recorded_uses_branch_head_unchanged() {
        // First-attempt behavior (no anchor recorded yet) must be byte-for-
        // byte identical to before cas-4b3f: use the live branch HEAD.
        let dir = init_factory_repo("worker");
        std::fs::write(dir.path().join("a.rs"), "// a\n").unwrap();
        git(dir.path(), &["add", "a.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: a"]);

        let task = worker_task("worker"); // deliverables.factory_branch_anchor == None
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        assert!(
            matches!(out, MergeStateGateOutcome::Reject(_)),
            "no-anchor first attempt must still reject on the real stranded commit, got {out:?}"
        );
    }

    /// cas-cf64 (P2, anchor freshness): an anchor present on a task whose
    /// `status` is NOT `AwaitingMerge` must be ignored — this is a defense-
    /// in-depth guard against legacy/corrupt data bypassing the task-store
    /// transition invariant. The gate must fall back to the live branch name
    /// (first-attempt behavior) rather than trusting a sha that no longer
    /// corresponds to "this task is genuinely parked awaiting merge."
    #[test]
    fn anchor_present_but_status_not_awaiting_merge_is_ignored() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        // Commit + merge into main (this WOULD make the anchor-based check
        // Proceed if the anchor were trusted).
        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: a"]);
        let merged_tip = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(p)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        git(p, &["checkout", "-q", "main"]);
        git(p, &["merge", "-q", "--no-ff", "factory/worker"]);
        git(p, &["checkout", "-q", "factory/worker"]);

        // Now add a NEW, genuinely unmerged commit — simulating reworked
        // code after some state transition that left a stale anchor
        // pointing at the OLD (already-merged) tip.
        std::fs::write(p.join("b.rs"), "// b, reworked, unmerged\n").unwrap();
        git(p, &["add", "b.rs"]);
        git(p, &["commit", "-q", "-m", "feat: b (reworked, unmerged)"]);

        let mut task = worker_task("worker");
        task.status = TaskStatus::InProgress; // NOT AwaitingMerge
        task.deliverables.factory_branch_anchor = Some(merged_tip);
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Reject(_)),
            "a stale anchor on a non-AwaitingMerge task must be ignored, \
             falling back to the live branch (which has 1 genuinely \
             unmerged commit), got {out:?}"
        );
    }

    // --- cas-2938: squash-equivalent live-ref convergence -------------------

    fn rev_parse_local(dir: &std::path::Path, refname: &str) -> String {
        let out = std::process::Command::new("git")
            .args(["rev-parse", refname])
            .current_dir(dir)
            .output()
            .expect("git rev-parse");
        assert!(out.status.success(), "rev-parse {refname} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Reproduces BUG-awaitingmerge-anchor-squash-merge-2026-07-09.md:
    /// park with tip A, squash-merge into parent as B (A is not an ancestor
    /// of B), force-align factory tip to B. Historical anchor A still looks
    /// stranded; live factory has 0 commits ahead → must Proceed.
    #[test]
    fn squash_merged_awaiting_merge_with_live_ref_aligned_to_integration_proceeds() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        // Task work as commit A on factory/worker.
        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task A"]);
        let anchor_a = rev_parse_local(p, "HEAD");

        // GitHub-style squash: apply factory tip as a NEW commit B on main
        // that does not have A as an ancestor.
        git(p, &["checkout", "-q", "main"]);
        git(p, &["merge", "-q", "--squash", "factory/worker"]);
        git(p, &["commit", "-q", "-m", "feat: task A (#683)"]);
        let integration_b = rev_parse_local(p, "HEAD");
        assert_ne!(
            anchor_a, integration_b,
            "precondition: squash must rewrite SHA (A ≠ B)"
        );
        // A is not an ancestor of B after squash.
        let is_ancestor = std::process::Command::new("git")
            .args(["merge-base", "--is-ancestor", &anchor_a, &integration_b])
            .current_dir(p)
            .status()
            .expect("merge-base --is-ancestor");
        assert!(
            !is_ancestor.success(),
            "precondition: A must NOT be an ancestor of squash tip B"
        );

        // Worker force-aligns factory/<name> to the integration tip B.
        git(p, &["checkout", "-q", "factory/worker"]);
        git(p, &["reset", "-q", "--hard", &integration_b]);
        assert_eq!(
            rev_parse_local(p, "factory/worker"),
            integration_b,
            "precondition: live factory ref must equal integration tip B"
        );
        assert_eq!(
            count_unmerged_factory_commits(p, "factory/worker", "main"),
            0,
            "precondition: live factory must have 0 commits ahead of main"
        );
        assert_eq!(
            count_unmerged_factory_commits(p, &anchor_a, "main"),
            1,
            "precondition: historical anchor A still looks stranded vs main"
        );

        // Task still AwaitingMerge with park-time anchor A.
        let mut task = worker_task("worker");
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.factory_branch_anchor = Some(anchor_a);
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "squash-integrated + live-ref-aligned AwaitingMerge close must \
             Proceed (cas-2938), got {out:?}"
        );
    }

    /// Clean squash preserves tip tree: even without force-aligning the
    /// factory ref to B, content reachability of A's tree on main must
    /// clear the gate (better UX than requiring a manual reset).
    #[test]
    fn squash_merged_content_equivalent_without_live_ref_align_proceeds() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task A"]);
        let anchor_a = rev_parse_local(p, "HEAD");

        // Squash onto main as B, leave factory/worker at A.
        git(p, &["checkout", "-q", "main"]);
        git(p, &["merge", "-q", "--squash", "factory/worker"]);
        git(p, &["commit", "-q", "-m", "feat: task A (#683)"]);
        git(p, &["checkout", "-q", "factory/worker"]);
        assert_eq!(
            rev_parse_local(p, "factory/worker"),
            anchor_a,
            "precondition: factory tip still at historical A"
        );
        assert_eq!(
            count_unmerged_factory_commits(p, &anchor_a, "main"),
            1,
            "precondition: ancestry still counts A as stranded"
        );
        assert!(
            commit_tip_tree_reachable_from(p, &anchor_a, "main"),
            "precondition: tip tree of A must be on main after clean squash"
        );

        let mut task = worker_task("worker");
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.factory_branch_anchor = Some(anchor_a);
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "clean squash must clear via tip-tree reachability even without \
             live-ref alignment, got {out:?}"
        );
    }

    /// cas-2938 + cas-4b3f: after squash of A, a later unmerged task B on the
    /// same factory branch must NOT re-strand task A's close — content of A
    /// is on main even though live HEAD is ahead with B.
    #[test]
    fn squash_then_serial_second_task_does_not_restrand_first_close() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        // Task A tip.
        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task A"]);
        let anchor_a = rev_parse_local(p, "HEAD");

        // Squash A onto main as B (A not ancestor of main).
        git(p, &["checkout", "-q", "main"]);
        git(p, &["merge", "-q", "--squash", "factory/worker"]);
        git(p, &["commit", "-q", "-m", "feat: task A (#683)"]);

        // Align factory to B, then start task B with a new unmerged commit.
        git(p, &["checkout", "-q", "factory/worker"]);
        git(p, &["reset", "-q", "--hard", "main"]);
        std::fs::write(p.join("b.rs"), "// b (task B, unmerged)\n").unwrap();
        git(p, &["add", "b.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task B (unmerged)"]);

        assert_eq!(
            count_unmerged_factory_commits(p, "factory/worker", "main"),
            1,
            "precondition: live factory carries task B's unmerged commit"
        );

        let mut task_a = worker_task("worker");
        task_a.status = TaskStatus::AwaitingMerge;
        task_a.deliverables.factory_branch_anchor = Some(anchor_a);
        let req = base_req(&task_a.id);
        let out = run_factory_branch_merge_gate(&task_a, &req, "main", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "task A's squash-integrated content must clear close even while \
             task B's later unmerged commits ride on the live factory HEAD, \
             got {out:?}"
        );
    }

    /// Genuinely unmerged parked work still rejects: anchor A not on main
    /// by ancestry or tip tree, live factory still carries A.
    #[test]
    fn genuinely_unmerged_awaiting_merge_anchor_still_rejects() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        std::fs::write(p.join("a.rs"), "// a unmerged\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(
            p,
            &["commit", "-q", "-m", "feat: task A (never integrated)"],
        );
        let anchor_a = rev_parse_local(p, "HEAD");

        assert_eq!(
            count_unmerged_factory_commits(p, &anchor_a, "main"),
            1,
            "precondition: A is stranded vs main"
        );
        assert!(
            !commit_tip_tree_reachable_from(p, &anchor_a, "main"),
            "precondition: A's tree must not be on main"
        );

        let mut task = worker_task("worker");
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.factory_branch_anchor = Some(anchor_a);
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Reject(_)),
            "genuinely unmerged parked work must still Reject, got {out:?}"
        );
    }

    /// cas-2938 P0 integrity: missing live factory ref must not authorize
    /// close. Legacy `count_unmerged_factory_commits` returns 0 for a missing
    /// factory ref; the live-ref path must use KnownZero-only and Reject.
    #[test]
    fn live_ref_fallback_rejects_when_live_factory_ref_missing() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        std::fs::write(p.join("a.rs"), "// a unmerged\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(
            p,
            &["commit", "-q", "-m", "feat: task A (never integrated)"],
        );
        let anchor_a = rev_parse_local(p, "HEAD");

        // Drop the live factory branch while keeping the parked anchor sha
        // resolvable as a dangling commit object.
        git(p, &["checkout", "-q", "main"]);
        git(p, &["branch", "-D", "factory/worker"]);

        assert!(
            git_ref_exists(p, &anchor_a),
            "precondition: parked anchor commit object still resolves"
        );
        assert!(
            !git_ref_exists(p, "factory/worker"),
            "precondition: live factory ref is gone"
        );
        // Legacy helper fail-opens to 0 — pins the bug class under test.
        assert_eq!(
            count_unmerged_factory_commits(p, "factory/worker", "main"),
            0,
            "precondition: legacy count treats missing factory ref as 0"
        );
        assert!(
            matches!(
                known_unmerged_factory_commits(p, "factory/worker", "main"),
                KnownUnmergedCount::Unknown
            ),
            "success-bearing helper must report Unknown for missing factory ref"
        );
        assert!(
            !commit_tip_tree_reachable_from(p, &anchor_a, "main"),
            "precondition: content path must not clear (A never integrated)"
        );

        let mut task = worker_task("worker");
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.factory_branch_anchor = Some(anchor_a);
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Reject(_)),
            "missing live factory ref must not authorize close via live-ref \
             fallback (unknown Git state ≠ KnownZero), got {out:?}"
        );
    }

    /// cas-2938 P0 integrity: when live factory merge-base cannot be computed
    /// (unrelated histories), live-ref must not treat fail-open count==0 as
    /// convergence. Primary path still uses the historical anchor (known
    /// stranded); live-ref must not clear on Unknown.
    #[test]
    fn live_ref_fallback_rejects_when_merge_base_unknowable() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        // Parked anchor A: related history, genuinely unmerged (KnownPositive).
        std::fs::write(p.join("a.rs"), "// a unmerged\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(
            p,
            &["commit", "-q", "-m", "feat: task A (never integrated)"],
        );
        let anchor_a = rev_parse_local(p, "HEAD");
        assert_eq!(
            count_unmerged_factory_commits(p, &anchor_a, "main"),
            1,
            "precondition: primary anchor ancestry must still see 1 stranded"
        );

        // Rewrite live factory/worker onto an orphan history so merge-base
        // with main fails — legacy count fail-opens to 0; Known* is Unknown.
        git(p, &["checkout", "-q", "--orphan", "factory-orphan"]);
        let _ = std::process::Command::new("git")
            .args(["rm", "-rf", "--cached", "."])
            .current_dir(p)
            .output();
        std::fs::write(p.join("orphan.txt"), "orphan\n").unwrap();
        git(p, &["add", "orphan.txt"]);
        git(p, &["commit", "-q", "-m", "orphan factory tip"]);
        git(p, &["branch", "-f", "factory/worker", "HEAD"]);
        git(p, &["checkout", "-q", "factory/worker"]);

        let mb = std::process::Command::new("git")
            .args(["merge-base", "main", "factory/worker"])
            .current_dir(p)
            .status()
            .expect("merge-base");
        assert!(
            !mb.success(),
            "precondition: live factory must have no merge-base with main"
        );
        assert_eq!(
            count_unmerged_factory_commits(p, "factory/worker", "main"),
            0,
            "precondition: legacy count fail-opens to 0 on merge-base failure"
        );
        assert!(
            matches!(
                known_unmerged_factory_commits(p, "factory/worker", "main"),
                KnownUnmergedCount::Unknown
            ),
            "success-bearing helper must report Unknown when merge-base fails"
        );
        assert!(
            !commit_tip_tree_reachable_from(p, &anchor_a, "main"),
            "precondition: content path must not clear"
        );

        let mut task = worker_task("worker");
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.factory_branch_anchor = Some(anchor_a);
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Reject(_)),
            "unknowable live merge-base must not authorize close via live-ref \
             fallback, got {out:?}"
        );
    }

    /// Unit coverage: known_unmerged_factory_commits is KnownZero only for a
    /// real zero-ahead tip, not for missing refs.
    #[test]
    fn known_unmerged_count_distinguishes_known_zero_from_unknown() {
        let dir = init_factory_repo("worker");
        let p = dir.path();
        // factory tip == main tip after init (no extra commits) → KnownZero.
        assert!(
            matches!(
                known_unmerged_factory_commits(p, "factory/worker", "main"),
                KnownUnmergedCount::KnownZero
            ),
            "fully-merged live factory must be KnownZero"
        );

        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: a"]);
        assert!(
            matches!(
                known_unmerged_factory_commits(p, "factory/worker", "main"),
                KnownUnmergedCount::KnownPositive(1)
            ),
            "one stranded commit must be KnownPositive(1)"
        );

        assert!(
            matches!(
                known_unmerged_factory_commits(p, "factory/ghost", "main"),
                KnownUnmergedCount::Unknown
            ),
            "missing factory ref must be Unknown, not KnownZero"
        );
    }

    // --- cas-5485: stale pre-rebase factory SHA ----------------------------

    /// Reproduces BUG-factory-close-stale-pre-rebase-sha.md:
    /// park with tip A, parent advances, rebase A→A', integrate A'.
    /// Historical anchor A is no longer an ancestor of parent; close must
    /// still Proceed when the post-rebase tip is integrated.
    #[test]
    fn rebased_awaiting_merge_anchor_proceeds_after_post_rebase_tip_integrated() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        // Task work as commit A on factory/worker.
        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task A"]);
        let anchor_a = rev_parse_local(p, "HEAD");

        // Parent advances with an unrelated commit (other workers).
        git(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("other.rs"), "// other\n").unwrap();
        git(p, &["add", "other.rs"]);
        git(p, &["commit", "-q", "-m", "feat: other worker"]);

        // Rebase factory/worker onto advanced main → A' (new SHA).
        git(p, &["checkout", "-q", "factory/worker"]);
        git(p, &["rebase", "-q", "main"]);
        let tip_a_prime = rev_parse_local(p, "HEAD");
        assert_ne!(
            anchor_a, tip_a_prime,
            "precondition: rebase must rewrite SHA (A ≠ A')"
        );
        let a_still_ancestor = std::process::Command::new("git")
            .args(["merge-base", "--is-ancestor", &anchor_a, "main"])
            .current_dir(p)
            .status()
            .expect("is-ancestor A main");
        assert!(
            !a_still_ancestor.success(),
            "precondition: pre-rebase A must not be an ancestor of main yet"
        );

        // Integrate post-rebase tip A' into main.
        git(p, &["checkout", "-q", "main"]);
        git(
            p,
            &["merge", "-q", "--no-ff", "factory/worker", "-m", "merge A'"],
        );
        git(p, &["checkout", "-q", "factory/worker"]);

        assert_eq!(
            count_unmerged_factory_commits(p, &anchor_a, "main"),
            1,
            "precondition: stale pre-rebase A still looks stranded by ancestry"
        );
        assert!(
            matches!(
                known_unmerged_factory_commits(p, "factory/worker", "main"),
                KnownUnmergedCount::KnownZero
            ),
            "precondition: live post-rebase tip is fully integrated (KnownZero)"
        );

        let mut task = worker_task("worker");
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.factory_branch_anchor = Some(anchor_a);
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "integrated post-rebase tip must clear close despite stale \
             pre-rebase anchor A (cas-5485), got {out:?}"
        );
    }

    /// Characterization of the false reject: evaluating *only* the stale
    /// pre-rebase anchor (primary ancestry path) reports stranded after a
    /// successful rebase+integrate of A'. Pins the bug class so the full
    /// gate's Proceed cannot pass for the wrong reason.
    #[test]
    fn stale_pre_rebase_anchor_alone_is_stranded_after_rebase_integrate() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task A"]);
        let anchor_a = rev_parse_local(p, "HEAD");

        git(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("other.rs"), "// other\n").unwrap();
        git(p, &["add", "other.rs"]);
        git(p, &["commit", "-q", "-m", "feat: other"]);
        git(p, &["checkout", "-q", "factory/worker"]);
        git(p, &["rebase", "-q", "main"]);
        git(p, &["checkout", "-q", "main"]);
        git(
            p,
            &["merge", "-q", "--no-ff", "factory/worker", "-m", "merge A'"],
        );

        assert_eq!(
            count_unmerged_factory_commits(p, &anchor_a, "main"),
            1,
            "stale pre-rebase SHA alone must still look stranded (the bug \
             the refresh path must clear)"
        );
        // A is not an ancestor of live factory tip after rewrite either.
        let live = rev_parse_local(p, "factory/worker");
        let a_anc_of_live = std::process::Command::new("git")
            .args(["merge-base", "--is-ancestor", &anchor_a, &live])
            .current_dir(p)
            .status()
            .expect("is-ancestor");
        assert!(
            !a_anc_of_live.success(),
            "precondition: rebase rewrote history — A is not ancestor of live tip"
        );
    }

    /// cas-5485 safety: after rebase of A→A', if A' is NOT integrated and
    /// live tip is still ahead, close must Reject (absent work).
    #[test]
    fn rebased_but_unmerged_work_still_rejects() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        std::fs::write(p.join("a.rs"), "// a unmerged\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task A"]);
        let anchor_a = rev_parse_local(p, "HEAD");

        git(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("other.rs"), "// other\n").unwrap();
        git(p, &["add", "other.rs"]);
        git(p, &["commit", "-q", "-m", "feat: other"]);
        git(p, &["checkout", "-q", "factory/worker"]);
        git(p, &["rebase", "-q", "main"]);
        // Do NOT merge into main — work is absent from integration.

        assert!(
            matches!(
                known_unmerged_factory_commits(p, "factory/worker", "main"),
                KnownUnmergedCount::KnownPositive(_)
            ),
            "precondition: rebased tip still genuinely unmerged"
        );

        let mut task = worker_task("worker");
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.factory_branch_anchor = Some(anchor_a);
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Reject(_)),
            "rebased but unmerged work must still Reject, got {out:?}"
        );
    }

    /// cas-5485 P2: park A → rebase A' → integrate A' → unmerged serial B
    /// on the same factory branch. Live KnownZero fails (B ahead); tip-tree
    /// of pre-rebase A differs from A' after rebase onto parent files.
    /// Cherry-equivalence of parked A must still clear task A without
    /// letting B satisfy the gate.
    #[test]
    fn rebased_integrated_a_with_later_unmerged_b_still_closes_a() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task A"]);
        let anchor_a = rev_parse_local(p, "HEAD");

        // Parent advances; rebase A→A'; integrate A'.
        git(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("other.rs"), "// other\n").unwrap();
        git(p, &["add", "other.rs"]);
        git(p, &["commit", "-q", "-m", "feat: other worker"]);
        git(p, &["checkout", "-q", "factory/worker"]);
        git(p, &["rebase", "-q", "main"]);
        let tip_a_prime = rev_parse_local(p, "HEAD");
        assert_ne!(anchor_a, tip_a_prime, "precondition: rebase rewrites SHA");
        git(p, &["checkout", "-q", "main"]);
        git(
            p,
            &["merge", "-q", "--no-ff", "factory/worker", "-m", "merge A'"],
        );
        git(p, &["checkout", "-q", "factory/worker"]);

        // Serial task B unmerged on the same factory branch.
        std::fs::write(p.join("b.rs"), "// b task B unmerged\n").unwrap();
        git(p, &["add", "b.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task B (unmerged)"]);

        assert!(
            matches!(
                known_unmerged_factory_commits(p, "factory/worker", "main"),
                KnownUnmergedCount::KnownPositive(_)
            ),
            "precondition: live tip carries B — KnownZero path must not apply"
        );
        assert_eq!(
            count_unmerged_factory_commits(p, &anchor_a, "main"),
            1,
            "precondition: pre-rebase A still ancestry-stranded"
        );
        assert!(
            !commit_tip_tree_reachable_from(p, &anchor_a, "main"),
            "precondition: tip-tree of pre-rebase A is not on main after \
             rebase onto parent-with-other (trees differ)"
        );
        assert!(
            commit_patches_cherry_equivalent_on_parent(p, &anchor_a, "main"),
            "precondition: patch of A must be cherry-equivalent on main via A'"
        );

        let mut task_a = worker_task("worker");
        task_a.status = TaskStatus::AwaitingMerge;
        task_a.deliverables.factory_branch_anchor = Some(anchor_a);
        let req = base_req(&task_a.id);
        let out = run_factory_branch_merge_gate(&task_a, &req, "main", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "task A must close after A' integrated even with later unmerged \
             B on live factory HEAD (cas-5485 P2), got {out:?}"
        );
    }

    /// cas-5485 P2 safety: later unmerged B must not clear close for an
    /// anchor whose patches are still absent from the parent.
    #[test]
    fn later_unmerged_b_does_not_satisfy_unintegrated_anchor_a() {
        let dir = init_factory_repo("worker");
        let p = dir.path();

        // Anchor A — never integrated (no rebase, no merge).
        std::fs::write(p.join("a.rs"), "// a only on factory\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task A never integrated"]);
        let anchor_a = rev_parse_local(p, "HEAD");

        // Task B on same branch — also unmerged.
        std::fs::write(p.join("b.rs"), "// b\n").unwrap();
        git(p, &["add", "b.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task B"]);

        assert!(
            !commit_patches_cherry_equivalent_on_parent(p, &anchor_a, "main"),
            "precondition: A's patch is not on main"
        );

        let mut task_a = worker_task("worker");
        task_a.status = TaskStatus::AwaitingMerge;
        task_a.deliverables.factory_branch_anchor = Some(anchor_a);
        let req = base_req(&task_a.id);
        let out = run_factory_branch_merge_gate(&task_a, &req, "main", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Reject(_)),
            "unintegrated A must still Reject even when live HEAD has later \
             commits (B must not satisfy A), got {out:?}"
        );
    }

    #[test]
    fn cherry_equivalent_fails_closed_on_missing_ref() {
        let dir = init_factory_repo("worker");
        let p = dir.path();
        assert!(
            !commit_patches_cherry_equivalent_on_parent(p, "factory/ghost", "main"),
            "missing commit-ish must fail closed"
        );
        assert!(
            !commit_patches_cherry_equivalent_on_parent(p, "factory/worker", "no-such-parent"),
            "missing parent must fail closed"
        );
    }

    // --- cas-38e2: stale local parent-branch ref vs. origin ----------------

    fn rev_parse(dir: &std::path::Path, refname: &str) -> String {
        let out = std::process::Command::new("git")
            .args(["rev-parse", refname])
            .current_dir(dir)
            .output()
            .expect("git rev-parse");
        assert!(out.status.success(), "rev-parse {refname} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Reproduces the exact live incident (cas-38e2 AC): the worker's
    /// commit was merged into the epic branch and PUSHED to origin (so
    /// `origin/<parent_branch>` genuinely contains it), but this repo's
    /// LOCAL `<parent_branch>` ref is still at the pre-merge tip — the gate
    /// must not bounce off that stale local view when origin already has
    /// the work.
    #[test]
    fn stale_local_parent_branch_falls_back_to_origin_and_proceeds() {
        let bare = tempfile::tempdir().unwrap();
        let bare_status = std::process::Command::new("git")
            .args(["init", "-q", "--bare"])
            .current_dir(bare.path())
            .status()
            .expect("git init --bare");
        assert!(bare_status.success());

        let dir = init_factory_repo_with_parent("worker", "epic/x");
        let p = dir.path();
        git(
            p,
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );

        // Snapshot the epic branch's pre-merge tip before anything lands.
        let old_epic_tip = rev_parse(p, "epic/x");

        // Worker's commit on factory/worker.
        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: a"]);

        // Supervisor (simulated in the same repo): merge + push epic/x to
        // origin. This is what makes `origin/epic/x` genuinely contain the
        // work.
        git(p, &["checkout", "-q", "epic/x"]);
        git(
            p,
            &[
                "merge",
                "-q",
                "--no-ff",
                "factory/worker",
                "-m",
                "merge worker",
            ],
        );
        git(p, &["push", "-q", "origin", "epic/x"]);

        // Now force the LOCAL epic/x ref back to the pre-merge tip —
        // simulating the closing worker's own checkout not having observed
        // the merge, while `origin/epic/x` (already fetched via the push
        // above) still correctly reflects it.
        git(p, &["checkout", "-q", "factory/worker"]);
        git(p, &["branch", "-f", "epic/x", &old_epic_tip]);

        // Precondition: the LOCAL epic/x view alone really would strand
        // this close (proves the test exercises the origin-fallback path,
        // not a no-op).
        assert_eq!(
            count_unmerged_factory_commits(p, "factory/worker", "epic/x"),
            1,
            "precondition: local epic/x must look stranded before the origin fallback kicks in"
        );

        let task = worker_task("worker");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "epic/x", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "a commit already reachable from origin/epic/x must not bounce off \
             a stale local epic/x ref, got {out:?}"
        );
    }

    /// Negative control: `origin/<parent_branch>` exists but genuinely does
    /// NOT contain the worker's commit either — the fallback must not
    /// paper over a real integration gap.
    #[test]
    fn origin_parent_branch_without_the_commit_still_rejects() {
        let bare = tempfile::tempdir().unwrap();
        let bare_status = std::process::Command::new("git")
            .args(["init", "-q", "--bare"])
            .current_dir(bare.path())
            .status()
            .expect("git init --bare");
        assert!(bare_status.success());

        let dir = init_factory_repo_with_parent("worker", "epic/x");
        let p = dir.path();
        git(
            p,
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );
        // Push epic/x to origin BEFORE the worker's commit exists — origin
        // has a real ref for epic/x, but it's never seen this work.
        git(p, &["push", "-q", "origin", "epic/x"]);

        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: a"]);

        let task = worker_task("worker");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "epic/x", p);
        match out {
            MergeStateGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("1 commit"),
                    "expected stranded count of 1: {msg}"
                );
            }
            other => panic!(
                "commit reachable from neither local nor origin epic/x must still reject, got {other:?}"
            ),
        }
    }

    /// No `origin` remote configured at all (the common local-only dev/test
    /// case) — the best-effort fetch + origin lookup must no-op gracefully,
    /// with behavior identical to before cas-38e2.
    #[test]
    fn no_origin_remote_configured_degrades_to_existing_behavior() {
        let dir = init_factory_repo("worker");
        std::fs::write(dir.path().join("a.rs"), "// a\n").unwrap();
        git(dir.path(), &["add", "a.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: a"]);

        let task = worker_task("worker");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "main", dir.path());
        assert!(
            matches!(out, MergeStateGateOutcome::Reject(_)),
            "no-origin-remote case must still reject on the real stranded commit, got {out:?}"
        );
    }

    // --- cas-f522: origin-parent ancestry fallback fail-closed on Unknown ---

    /// AC1: origin/<parent> KnownZero is the only origin-ancestry signal that
    /// may authorize close when local parent still looks stranded. Pins the
    /// success-bearing helper (not fail-open count==0) for the happy path.
    #[test]
    fn origin_parent_known_zero_authorizes_close_cas_f522() {
        let bare = tempfile::tempdir().unwrap();
        let bare_status = std::process::Command::new("git")
            .args(["init", "-q", "--bare"])
            .current_dir(bare.path())
            .status()
            .expect("git init --bare");
        assert!(bare_status.success());

        let dir = init_factory_repo_with_parent("worker", "epic/x");
        let p = dir.path();
        git(
            p,
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );

        let old_epic_tip = rev_parse(p, "epic/x");

        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: a"]);

        git(p, &["checkout", "-q", "epic/x"]);
        git(
            p,
            &[
                "merge",
                "-q",
                "--no-ff",
                "factory/worker",
                "-m",
                "merge worker",
            ],
        );
        git(p, &["push", "-q", "origin", "epic/x"]);

        git(p, &["checkout", "-q", "factory/worker"]);
        git(p, &["branch", "-f", "epic/x", &old_epic_tip]);

        assert_eq!(
            count_unmerged_factory_commits(p, "factory/worker", "epic/x"),
            1,
            "precondition: local epic/x still looks stranded"
        );
        assert!(
            matches!(
                known_unmerged_factory_commits(p, "factory/worker", "origin/epic/x"),
                KnownUnmergedCount::KnownZero
            ),
            "precondition: origin/epic/x must be KnownZero vs factory tip"
        );

        let task = worker_task("worker");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "epic/x", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Proceed),
            "origin/<parent> KnownZero must authorize close despite stale local parent, got {out:?}"
        );
    }

    /// AC2: when origin/<parent> exists but merge-base is unknowable
    /// (unrelated histories), legacy count fail-opens to 0 — that must NOT
    /// authorize close. Unknown never masquerades as integration.
    #[test]
    fn origin_parent_unknown_merge_base_rejects_cas_f522() {
        let bare = tempfile::tempdir().unwrap();
        let bare_status = std::process::Command::new("git")
            .args(["init", "-q", "--bare"])
            .current_dir(bare.path())
            .status()
            .expect("git init --bare");
        assert!(bare_status.success());

        let dir = init_factory_repo_with_parent("worker", "epic/x");
        let p = dir.path();
        git(
            p,
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );

        // Unmerged factory work so local parent ancestry is KnownPositive.
        std::fs::write(p.join("a.rs"), "// a unmerged\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: a (never integrated)"]);

        // Push an UNRELATED orphan tip as origin/epic/x so the origin ref
        // exists but cannot compute merge-base with factory/worker.
        git(p, &["checkout", "-q", "--orphan", "origin-orphan"]);
        let _ = std::process::Command::new("git")
            .args(["rm", "-rf", "--cached", "."])
            .current_dir(p)
            .output();
        // Clear the worktree so the orphan commit is pure.
        for entry in std::fs::read_dir(p).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if name == ".git" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
        std::fs::write(p.join("orphan.txt"), "unrelated origin tip\n").unwrap();
        git(p, &["add", "orphan.txt"]);
        git(p, &["commit", "-q", "-m", "orphan origin epic tip"]);
        // Publish orphan as epic/x on origin (creates origin/epic/x tracking).
        git(p, &["push", "-q", "origin", "HEAD:epic/x"]);
        git(p, &["checkout", "-q", "factory/worker"]);
        // Ensure remote-tracking ref is present without re-fetching local epic.
        git(
            p,
            &["fetch", "-q", "origin", "epic/x:refs/remotes/origin/epic/x"],
        );

        assert!(
            git_ref_exists(p, "origin/epic/x"),
            "precondition: origin/epic/x must resolve"
        );
        assert_eq!(
            count_unmerged_factory_commits(p, "factory/worker", "epic/x"),
            1,
            "precondition: local epic/x must still strand the factory tip"
        );
        // Pin the fail-open hole under test.
        assert_eq!(
            count_unmerged_factory_commits(p, "factory/worker", "origin/epic/x"),
            0,
            "precondition: legacy count fail-opens to 0 on unknowable origin merge-base"
        );
        assert!(
            matches!(
                known_unmerged_factory_commits(p, "factory/worker", "origin/epic/x"),
                KnownUnmergedCount::Unknown
            ),
            "success-bearing helper must report Unknown for origin merge-base failure"
        );

        let task = worker_task("worker");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "epic/x", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Reject(_)),
            "origin-parent Unknown (failed merge-base) must not authorize close, got {out:?}"
        );
    }

    /// AC2 variant: origin/<parent> ref missing entirely — no origin rescue;
    /// gate must still Reject on the real local stranded work (not treat
    /// absence as KnownZero).
    #[test]
    fn origin_parent_missing_ref_does_not_authorize_close_cas_f522() {
        let dir = init_factory_repo_with_parent("worker", "epic/x");
        let p = dir.path();
        // Remote present but never pushed — origin/epic/x does not resolve.
        let bare = tempfile::tempdir().unwrap();
        let bare_status = std::process::Command::new("git")
            .args(["init", "-q", "--bare"])
            .current_dir(bare.path())
            .status()
            .expect("git init --bare");
        assert!(bare_status.success());
        git(
            p,
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );

        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: a"]);

        assert!(
            !git_ref_exists(p, "origin/epic/x"),
            "precondition: origin/epic/x must be missing"
        );
        assert!(
            matches!(
                known_unmerged_factory_commits(p, "factory/worker", "origin/epic/x"),
                KnownUnmergedCount::Unknown
            ),
            "missing origin ref must be Unknown, not zero"
        );

        let task = worker_task("worker");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "epic/x", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Reject(_)),
            "missing origin/<parent> must not authorize close, got {out:?}"
        );
    }

    /// AC3: origin/<parent> KnownPositive (genuinely unmerged vs origin) rejects.
    /// Strengthens the existing negative control with the success-bearing helper.
    #[test]
    fn origin_parent_known_positive_rejects_cas_f522() {
        let bare = tempfile::tempdir().unwrap();
        let bare_status = std::process::Command::new("git")
            .args(["init", "-q", "--bare"])
            .current_dir(bare.path())
            .status()
            .expect("git init --bare");
        assert!(bare_status.success());

        let dir = init_factory_repo_with_parent("worker", "epic/x");
        let p = dir.path();
        git(
            p,
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );
        git(p, &["push", "-q", "origin", "epic/x"]);

        std::fs::write(p.join("a.rs"), "// a\n").unwrap();
        git(p, &["add", "a.rs"]);
        git(p, &["commit", "-q", "-m", "feat: a"]);

        assert!(
            matches!(
                known_unmerged_factory_commits(p, "factory/worker", "origin/epic/x"),
                KnownUnmergedCount::KnownPositive(1)
            ),
            "precondition: origin must report KnownPositive(1)"
        );

        let task = worker_task("worker");
        let req = base_req(&task.id);
        let out = run_factory_branch_merge_gate(&task, &req, "epic/x", p);
        assert!(
            matches!(out, MergeStateGateOutcome::Reject(_)),
            "origin KnownPositive must reject, got {out:?}"
        );
    }
}

#[cfg(test)]
mod merge_conflict_detection_tests {
    //! cas-a844: `factory_branch_merge_conflict_paths` distinguishes a
    //! genuine git merge conflict from simply "not merged yet" — the
    //! distinction `awaiting_merge`'s status output and refusal message need
    //! so a conflicted park never reads identically to a clean one. It
    //! returns the conflicting paths themselves (empty = no conflict /
    //! unknowable) rather than a bare boolean, since the caller uses the
    //! paths to make the refusal message actionable. Uses the same `git()` /
    //! `init_factory_repo` fixtures as `merge_state_gate_tests`
    //! (pure-function coverage, same rationale: no full
    //! `CasCore`/`cas_task_close` harness needed to prove the detection
    //! logic itself).
    use super::*;
    use std::process::Command;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_factory_repo(worker: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        git(p, &["checkout", "-q", "-b", &format!("factory/{worker}")]);
        dir
    }

    #[test]
    fn clean_divergence_is_not_conflicted() {
        // Worker adds a brand-new file — never merged into main yet, but
        // trivially fast-forwardable/mergeable. This is the common,
        // supervisor-actionable "MERGE REQUIRED" case: unmerged, not
        // conflicted.
        let dir = init_factory_repo("worker");
        let p = dir.path();
        std::fs::write(p.join("worker.txt"), "worker change\n").unwrap();
        git(p, &["add", "worker.txt"]);
        git(p, &["commit", "-q", "-m", "worker change"]);

        assert!(
            factory_branch_merge_conflict_paths(p, "main", "factory/worker")
                .expect("preflight succeeds")
                .is_empty(),
            "a clean, non-overlapping divergence must not be reported as conflicted"
        );
    }

    #[test]
    fn overlapping_edits_to_same_file_is_conflicted() {
        // Both branches edit seed.txt differently after the fork point —
        // a real content conflict the supervisor's merge cannot resolve
        // automatically.
        let dir = init_factory_repo("worker");
        let p = dir.path();
        std::fs::write(p.join("seed.txt"), "worker's edit\n").unwrap();
        git(p, &["commit", "-aq", "-m", "worker edits seed"]);

        git(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("seed.txt"), "main's conflicting edit\n").unwrap();
        git(p, &["commit", "-aq", "-m", "main edits seed differently"]);

        let paths = factory_branch_merge_conflict_paths(p, "main", "factory/worker")
            .expect("preflight succeeds");
        assert_eq!(
            paths,
            vec!["seed.txt".to_string()],
            "must name the actual conflicting file, not just report a bare bool"
        );
    }

    #[test]
    fn missing_branch_preserves_preflight_error() {
        // cas-7308a: unknown is not clean. The close path uses this error
        // to park reopen-eligible instead of recreating the dead end.
        let dir = init_factory_repo("worker");
        let p = dir.path();
        let (paths, error) = classify_merge_conflict_preflight(
            factory_branch_merge_conflict_paths(p, "main", "factory/nobody"),
        );
        assert!(paths.is_empty());
        let error = error.expect("missing source branch must remain distinguishable from clean");
        assert!(
            error.contains("factory/nobody"),
            "error should name the missing source branch: {error}"
        );
        assert!(
            !paths.is_empty() || !error.is_empty(),
            "the production reopen predicate must treat preflight errors as eligible"
        );
        let message = enrich_merge_required_with_conflict_check(
            "MERGE REQUIRED".to_string(),
            "main",
            "cas-7308a",
            &paths,
            Some(&error),
        );
        assert!(
            message.contains("Git conflict preflight failed")
                && message.contains("reopen-eligible")
                && message.contains("action=start id=cas-7308a"),
            "error refusal must preserve the cause and the worker exit: {message}"
        );
    }

    #[test]
    fn same_branch_against_itself_is_not_conflicted() {
        let dir = init_factory_repo("worker");
        let p = dir.path();
        assert!(
            factory_branch_merge_conflict_paths(p, "factory/worker", "factory/worker")
                .expect("preflight succeeds")
                .is_empty()
        );
    }
}

#[cfg(test)]
mod system_b_worktree_resolution_tests {
    //! cas-4b3f (AC a, root cause): `resolve_worker_worktree_path` only ever
    //! consulted System A (`task.worktree_id`, epic-only, gated behind a
    //! config flag that's disabled by default). Every real single-task
    //! factory worker is isolated via System B
    //! (`spawn_workers isolate=true`, `<cas_root>/worktrees/<assignee>`),
    //! which was never checked — so cas-895d/cas-490f/cas-762e/cas-ee2b all
    //! silently no-opped for the overwhelmingly common case. These tests
    //! cover the pure resolution helper directly; the full close-path wiring
    //! is covered by the `mcp_tools_test` integration tests.
    use super::*;
    use std::process::Command;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn resolves_when_system_b_worktree_exists() {
        let cas_root = tempfile::tempdir().unwrap();
        let wt_path = cas_root.path().join("worktrees").join("worker-1");
        std::fs::create_dir_all(&wt_path).unwrap();
        git(&wt_path, &["init", "-q", "-b", "factory/worker-1"]);

        let resolved = resolve_system_b_worktree_path(cas_root.path(), "worker-1");
        assert_eq!(resolved, Some(wt_path));
    }

    #[test]
    fn returns_none_when_no_worktree_directory_exists() {
        let cas_root = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_system_b_worktree_path(cas_root.path(), "nobody"),
            None,
            "a genuinely non-isolated task (no directory at all) must not resolve"
        );
    }

    #[test]
    fn returns_none_for_directory_without_dot_git() {
        // Guards against treating an arbitrary stray directory under
        // `worktrees/` (not actually a git worktree) as resolvable.
        let cas_root = tempfile::tempdir().unwrap();
        let stray = cas_root.path().join("worktrees").join("not-a-worktree");
        std::fs::create_dir_all(&stray).unwrap();
        assert_eq!(
            resolve_system_b_worktree_path(cas_root.path(), "not-a-worktree"),
            None
        );
    }

    // --- cas-cf64 (P3): path-traversal hardening ----------------------------

    #[test]
    fn path_traversal_assignee_does_not_escape_worktrees_dir() {
        // Without sanitization, `cas_root.join("worktrees").join("../..")`
        // resolves to `cas_root`'s PARENT — in production that's the MAIN
        // repo checkout root (cas_root = <repo>/.cas), which has its own
        // `.git`. Reintroduces the cas-895d "reject every close on
        // unrelated dirty state" bug (`resolve_system_b_worktree_path`
        // would return the main worktree instead of `None`).
        let sandbox = tempfile::tempdir().unwrap();
        let fake_repo_root = sandbox.path().join("myrepo");
        let cas_root = fake_repo_root.join(".cas");
        // The intermediate `worktrees/` directory must actually exist on
        // disk for `..` traversal through it to resolve at all (the OS
        // resolves `..` against the real directory tree, not lexically) —
        // without this the malicious path would ENOENT regardless of
        // sanitization, silently defeating the test.
        std::fs::create_dir_all(cas_root.join("worktrees")).unwrap();
        // The "main repo checkout" decoy: `.git` at `cas_root`'s parent —
        // exactly where `worktrees/../..` would land.
        std::fs::create_dir_all(fake_repo_root.join(".git")).unwrap();

        // The realistic attack shape: exactly 2 levels up from
        // `<cas_root>/worktrees` lands on `<cas_root>`'s parent — the main
        // repo root in production. This is the one that would actually
        // reach the planted decoy `.git` above if unsanitized.
        assert_eq!(
            resolve_system_b_worktree_path(&cas_root, "../.."),
            None,
            "\"../..\" must not escape to the main repo checkout"
        );

        // Other traversal/separator shapes must also be rejected outright
        // (none of these coincidentally land on a `.git`-bearing directory
        // in this fixture, but they must still be refused at the
        // validation layer, not merely "happen to fail" the existence check).
        for malicious in ["..", "../victim", "a/../../escape"] {
            assert_eq!(
                resolve_system_b_worktree_path(&cas_root, malicious),
                None,
                "traversal-shaped assignee {malicious:?} must not resolve"
            );
        }
    }

    #[test]
    fn plain_assignee_names_with_hyphens_and_digits_still_resolve() {
        // Sanity: the sanitization must not be so strict it breaks normal
        // agent-name conventions (e.g. "adjective-noun-42").
        let cas_root = tempfile::tempdir().unwrap();
        let wt_path = cas_root.path().join("worktrees").join("quiet-tiger-24");
        std::fs::create_dir_all(&wt_path).unwrap();
        git(&wt_path, &["init", "-q", "-b", "factory/quiet-tiger-24"]);

        assert_eq!(
            resolve_system_b_worktree_path(cas_root.path(), "quiet-tiger-24"),
            Some(wt_path)
        );
    }

    // --- cas-cf64 (P3): configurable worktree_base_path ---------------------

    #[test]
    fn honors_configured_worktree_base_path_override() {
        // Use an ABSOLUTE base_path override (the code's `base.starts_with('/')`
        // branch) so the expected resolved location is unambiguous — the
        // relative form additionally resolves `{project}` against the repo
        // root's PARENT, which is exercised separately by
        // `WorktreeManager`'s own tests; here we're only proving that a
        // configured override is consulted at all instead of being
        // silently ignored.
        let sandbox = tempfile::tempdir().unwrap();
        let repo_root = sandbox.path().join("myrepo");
        std::fs::create_dir_all(&repo_root).unwrap();
        let cas_root = repo_root.join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();

        let override_base = sandbox.path().join("elsewhere-worktrees");
        std::fs::write(
            cas_root.join("config.toml"),
            format!(
                "[worktrees]\nbase_path = \"{}\"\n",
                override_base.to_str().unwrap().replace('\\', "\\\\")
            ),
        )
        .unwrap();

        // The configured location: NOT under `.cas/worktrees` at all.
        let configured_wt_path = override_base.join("worker-1");
        std::fs::create_dir_all(&configured_wt_path).unwrap();
        git(
            &configured_wt_path,
            &["init", "-q", "-b", "factory/worker-1"],
        );

        assert_eq!(
            resolve_system_b_worktree_path(&cas_root, "worker-1"),
            Some(configured_wt_path.clone()),
            "a configured base_path override must be honored, not silently \
             ignored in favor of the hardcoded <cas_root>/worktrees default"
        );

        // The OLD hardcoded default location must NOT be preferred, proving
        // the override actually took effect rather than the check passing
        // by coincidence.
        let default_wt_path = cas_root.join("worktrees").join("worker-1");
        std::fs::create_dir_all(&default_wt_path).unwrap();
        git(&default_wt_path, &["init", "-q", "-b", "factory/worker-1"]);
        assert_eq!(
            resolve_system_b_worktree_path(&cas_root, "worker-1"),
            Some(configured_wt_path),
            "with an override configured, the default path must be ignored \
             even when something happens to exist there too"
        );
    }

    #[test]
    fn no_config_falls_back_to_default_worktrees_path_unchanged() {
        // No `.cas/config.toml` at all — must behave exactly as before
        // cas-cf64 (simple `<cas_root>/worktrees/<assignee>`).
        let cas_root = tempfile::tempdir().unwrap();
        let wt_path = cas_root.path().join("worktrees").join("worker-1");
        std::fs::create_dir_all(&wt_path).unwrap();
        git(&wt_path, &["init", "-q", "-b", "factory/worker-1"]);

        assert_eq!(
            resolve_system_b_worktree_path(cas_root.path(), "worker-1"),
            Some(wt_path)
        );
    }

    // --- cas-cf64 follow-up: cross-validate against
    // WorktreeManager::worktree_path_for_worker() (hv-skill's cas-0938) ----
    //
    // cas-0938 (worktree_ops.rs, a different file/task) switched ITS
    // System-B path resolution to call
    // `WorktreeManager::worktree_path_for_worker()` directly rather than a
    // parallel formula, and flagged this module's
    // `resolve_system_b_worktree_path` as now-inconsistent. Reusing that
    // method here directly is architecturally awkward: `WorktreeManager::new`
    // requires a REAL git repository at the resolved root
    // (`GitOperations::detect_repo_root`) and does a live
    // `git --version`-style availability probe — both wrong for a "gate"
    // helper that must degrade gracefully on arbitrary/test paths, and it
    // would force every existing lightweight unit test in this module
    // (which use a bare tempdir as `cas_root`, not a real nested repo) to
    // grow a full fake repository. Per the supervisor's explicit fallback
    // ("if a shared helper is awkward... match its base_path resolution
    // exactly"), `system_b_worktree_base` re-derives the SAME formula
    // (`{project}` substitution, absolute-vs-relative-to-repo-root's-
    // parent) instead of calling the method. This test proves that
    // decision empirically: constructs a REAL `WorktreeManager` against
    // the same config and asserts both resolvers agree, for both the
    // default and a configured override — not just by code inspection.
    #[test]
    fn agrees_with_worktree_manager_worktree_path_for_worker_default() {
        let sandbox = tempfile::tempdir().unwrap();
        let repo_root = sandbox.path().canonicalize().unwrap().join("myrepo");
        std::fs::create_dir_all(&repo_root).unwrap();
        git(&repo_root, &["init", "-q", "-b", "main"]);
        let cas_root = repo_root.join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();

        let wt_config = crate::worktree::WorktreeConfig {
            enabled: true,
            base_path: DEFAULT_WORKTREE_BASE_PATH_TEMPLATE.to_string(),
            branch_prefix: "cas/".to_string(),
            auto_merge: false,
            cleanup_on_close: true,
            promote_entries_on_merge: true,
        };
        let manager = crate::worktree::WorktreeManager::new(&repo_root, wt_config)
            .expect("manager should construct against a real repo");

        assert_eq!(
            system_b_worktree_base(&cas_root),
            manager.worktree_root(),
            "default base_path must resolve to the SAME location as \
             WorktreeManager::worktree_root() (which worktree_path_for_worker \
             joins the assignee onto)"
        );
    }

    #[test]
    fn agrees_with_worktree_manager_worktree_path_for_worker_configured_override() {
        let sandbox = tempfile::tempdir().unwrap();
        let repo_root = sandbox.path().canonicalize().unwrap().join("myrepo");
        std::fs::create_dir_all(&repo_root).unwrap();
        git(&repo_root, &["init", "-q", "-b", "main"]);
        let cas_root = repo_root.join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();

        let override_template = "../{project}-worktrees".to_string();
        std::fs::write(
            cas_root.join("config.toml"),
            format!("[worktrees]\nbase_path = \"{override_template}\"\n"),
        )
        .unwrap();

        let wt_config = crate::worktree::WorktreeConfig {
            enabled: true,
            base_path: override_template,
            branch_prefix: "cas/".to_string(),
            auto_merge: false,
            cleanup_on_close: true,
            promote_entries_on_merge: true,
        };
        let manager = crate::worktree::WorktreeManager::new(&repo_root, wt_config)
            .expect("manager should construct against a real repo");

        assert_eq!(
            system_b_worktree_base(&cas_root),
            manager.worktree_root(),
            "a configured base_path override must resolve to the SAME \
             location via both resolvers — this is the exact drift \
             hv-skill's cas-0938 flagged"
        );
    }
}

#[cfg(test)]
mod epic_status_gate_tests {
    //! cas-8f8f: per-child branch merge-state report + epic-close gate.
    //!
    //! Layered on top of the cas-95ce per-task gate. The report
    //! rendering is a pure function of `Vec<EpicChildBranchStatus>`,
    //! and the gate is a thin filter on top of `collect_epic_branch_statuses`
    //! that rejects when any child has stranded factory commits.
    //! Bypass-immunity is structural (gate signature does not consume
    //! the bypass flag), and `run_epic_close_merge_gate` is also
    //! upstream of the cas-code-review bypass evaluation in
    //! [`run_code_review_gate`] — same shape as cas-95ce.
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn epic_git_stdout(dir: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8(output.stdout)
            .expect("git output must be utf-8")
            .trim()
            .to_string()
    }

    /// Set up a tempdir git repo where `main` is the seed and each
    /// of `workers` has a `factory/<name>` branch with `commits_per`
    /// additive commits beyond `main`. Returns the tempdir handle.
    fn init_epic_repo(workers_with_strands: &[(&str, usize)]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        for (worker, n) in workers_with_strands {
            git(p, &["checkout", "-q", "-b", &format!("factory/{worker}")]);
            for i in 0..*n {
                let fname = format!("{worker}-{i}.rs");
                std::fs::write(p.join(&fname), format!("// {worker} {i}\n")).unwrap();
                git(p, &["add", &fname]);
                git(p, &["commit", "-q", "-m", &format!("feat: {fname}")]);
            }
            git(p, &["checkout", "-q", "main"]);
        }
        dir
    }

    fn child(id: &str, status: TaskStatus, assignee: Option<&str>) -> Task {
        let parked_branch = assignee.map(|name| format!("factory/{name}"));
        let mut task = Task {
            id: id.to_string(),
            title: format!("child {id}"),
            status,
            assignee: assignee.map(str::to_string),
            ..Default::default()
        };
        task.deliverables.parked_branch = parked_branch;
        task
    }

    fn epic(id: &str) -> Task {
        Task {
            id: id.to_string(),
            title: format!("epic {id}"),
            status: TaskStatus::InProgress,
            task_type: TaskType::Epic,
            ..Default::default()
        }
    }

    fn base_req(id: &str) -> TaskCloseRequest {
        TaskCloseRequest {
            id: id.to_string(),
            reason: None,
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: None,
            commit_receipt: None,
        }
    }

    // --- collect_epic_branch_statuses ---------------------------------------

    #[test]
    fn factory_epic_status_returns_clean_report_when_all_merged() {
        // All workers fully merged → all children show 0 unmerged.
        let dir = init_epic_repo(&[("alpha", 0), ("bravo", 0)]);
        let subtasks = vec![
            child("cas-c1", TaskStatus::Closed, Some("alpha")),
            child("cas-c2", TaskStatus::Closed, Some("bravo")),
        ];
        let statuses = collect_epic_branch_statuses(&subtasks, "main", dir.path());
        assert_eq!(statuses.len(), 2);
        assert!(
            statuses.iter().all(|s| s.unmerged_count == 0),
            "all children should report 0 unmerged: {statuses:?}"
        );
        assert!(
            statuses.iter().all(|s| s.factory_branch.is_some()),
            "every child has an assignee → every row has a factory branch"
        );

        let report = render_epic_status_report("cas-epic", "main", &statuses);
        assert!(report.contains("Epic cas-epic"));
        assert!(report.contains("factory/alpha"));
        assert!(report.contains("factory/bravo"));
        assert!(
            report.contains("All child factory branches are merged"),
            "clean report must include the all-merged confirmation: {report}"
        );
    }

    #[test]
    fn factory_epic_status_reports_unmerged_per_worker() {
        // Two of three workers carry stranded commits; alpha is clean.
        let dir = init_epic_repo(&[("alpha", 0), ("bravo", 2), ("charlie", 5)]);
        let subtasks = vec![
            child("cas-c1", TaskStatus::Closed, Some("alpha")),
            child("cas-c2", TaskStatus::Closed, Some("bravo")),
            child("cas-c3", TaskStatus::Closed, Some("charlie")),
        ];
        let statuses = collect_epic_branch_statuses(&subtasks, "main", dir.path());
        assert_eq!(statuses.len(), 3);

        let by_id: std::collections::HashMap<_, _> =
            statuses.iter().map(|s| (s.task_id.as_str(), s)).collect();
        assert_eq!(by_id["cas-c1"].unmerged_count, 0, "alpha is clean");
        assert_eq!(by_id["cas-c2"].unmerged_count, 2, "bravo has 2 stranded");
        assert_eq!(by_id["cas-c3"].unmerged_count, 5, "charlie has 5 stranded");

        // Each row with stranded commits must carry a non-None last_commit_unix
        // (the branch exists locally and has at least one commit).
        assert!(by_id["cas-c2"].last_commit_unix.is_some());
        assert!(by_id["cas-c3"].last_commit_unix.is_some());

        let report = render_epic_status_report("cas-epic", "main", &statuses);
        assert!(
            report.contains("2 child task(s) carry stranded factory commits"),
            "report must summarize stranded count = 2 (bravo + charlie): {report}"
        );
    }

    // cas-aae6 (GH #110): a stacked epic cannot land alone, and epic_status is
    // where the supervisor decides merge order — so the chain belongs here, not
    // only in a creation message that scrolled away hours ago.

    #[test]
    fn epic_status_names_the_full_stack_and_landing_order() {
        let dir = init_epic_repo(&[("alpha", 1)]);
        let subtasks = vec![child("cas-c1", TaskStatus::Closed, Some("alpha"))];
        let statuses = collect_epic_branch_statuses(&subtasks, "epic/c", dir.path());

        let report = render_epic_status_report_with_stack(
            "cas-epic",
            "epic/c",
            &statuses,
            &["epic/a".to_string(), "epic/b".to_string()],
        );

        assert!(
            report.contains("Stacked on: 2 unlanded epic branch(es) — 'epic/a' → 'epic/b'"),
            "the ancestry must be named in full: {report}"
        );
        assert!(
            report.contains("Landing order: 'epic/a' → 'epic/b' → 'epic/c'")
                && report.contains("merging it merges them"),
            "the order the branches must land in is the actionable part: {report}"
        );
    }

    #[test]
    fn epic_status_without_a_stack_is_byte_identical_to_before() {
        let dir = init_epic_repo(&[("alpha", 1)]);
        let subtasks = vec![child("cas-c1", TaskStatus::Closed, Some("alpha"))];
        let statuses = collect_epic_branch_statuses(&subtasks, "main", dir.path());

        let plain = render_epic_status_report("cas-epic", "main", &statuses);
        let empty_stack =
            render_epic_status_report_with_stack("cas-epic", "main", &statuses, &[]);

        assert_eq!(
            plain, empty_stack,
            "an unstacked epic must render exactly as it did before this feature"
        );
        assert!(
            !plain.contains("Stacked on"),
            "no stack language when there is no stack: {plain}"
        );
    }

    #[test]
    fn epic_status_shows_the_stack_even_with_no_child_tasks() {
        let dir = init_epic_repo(&[]);
        let statuses = collect_epic_branch_statuses(&[], "epic/b", dir.path());

        let report = render_epic_status_report_with_stack(
            "cas-epic",
            "epic/b",
            &statuses,
            &["epic/a".to_string()],
        );

        assert!(
            report.contains("Stacked on: 1 unlanded epic branch(es) — 'epic/a'"),
            "an empty epic still has a landing constraint: {report}"
        );
        assert!(
            report.contains("(no child tasks)"),
            "the existing empty-epic body must survive: {report}"
        );
    }

    #[test]
    fn factory_epic_status_handles_no_subtasks() {
        // Epic with zero children produces a "no child tasks" report.
        let dir = init_epic_repo(&[]);
        let statuses = collect_epic_branch_statuses(&[], "main", dir.path());
        assert!(statuses.is_empty());

        let report = render_epic_status_report("cas-epic-empty", "main", &statuses);
        assert!(
            report.contains("(no child tasks)"),
            "empty-subtasks report must emit the explicit no-children marker: {report}"
        );
    }

    #[test]
    fn factory_epic_status_includes_assigneeless_children() {
        // Children without recorded task work are reported with em-dash
        // placeholders for branch / count so the report is complete; the
        // gate filters them out separately.
        let dir = init_epic_repo(&[]);
        let subtasks = vec![child("cas-orphan", TaskStatus::InProgress, None)];
        let statuses = collect_epic_branch_statuses(&subtasks, "main", dir.path());
        assert_eq!(statuses.len(), 1);
        assert!(statuses[0].factory_branch.is_none());
        assert_eq!(statuses[0].unmerged_count, 0);

        let report = render_epic_status_report("cas-epic", "main", &statuses);
        assert!(report.contains("cas-orphan"));
        assert!(
            report.contains("| — | — |"),
            "assigneeless rows must use em-dash for branch + unmerged columns: {report}"
        );
    }

    #[test]
    fn epic_close_derives_live_branch_for_legacy_child_without_evidence() {
        let dir = init_epic_repo(&[("worker", 1)]);
        let legacy_child = Task {
            id: "cas-legacy".to_string(),
            title: "legacy child without task receipt".to_string(),
            status: TaskStatus::Closed,
            assignee: Some("worker".to_string()),
            ..Default::default()
        };
        let statuses = collect_epic_branch_statuses(&[legacy_child], "main", dir.path());

        assert_eq!(statuses.len(), 1);
        assert_eq!(
            statuses[0].factory_branch.as_deref(),
            Some("factory/worker"),
            "without a recorded receipt, the live branch is the only available \
             merge-state evidence and must be checked rather than silently passed"
        );
        assert_eq!(
            statuses[0].unmerged_count, 1,
            "the legacy/no-receipt fallback must expose the stranded commit"
        );
    }

    #[test]
    fn dangling_anchor_falls_back_to_parked_branch_for_epic_close() {
        let dir = init_epic_repo(&[("worker", 1)]);
        let mut child = child("cas-dangling-anchor", TaskStatus::Closed, Some("worker"));
        child.deliverables.factory_branch_anchor = Some("0".repeat(40));
        assert!(git_ref_exists(dir.path(), "main"));
        assert!(
            !git_ref_exists(
                dir.path(),
                child.deliverables.factory_branch_anchor.as_deref().unwrap()
            ),
            "syntactically valid but absent object IDs must not count as existing refs"
        );

        let unmerged =
            collect_epic_branch_statuses(std::slice::from_ref(&child), "main", dir.path());
        assert_eq!(
            unmerged[0].unmerged_count, 1,
            "an unresolvable anchor must fall back to the parked branch and expose stranded work"
        );
        assert!(
            unmerged[0].last_commit_unix.is_some(),
            "the fallback must inspect the parked branch rather than treating the child as evidence-free"
        );

        git(
            dir.path(),
            &["merge", "--no-ff", "-m", "merge worker", "factory/worker"],
        );
        let merged = collect_epic_branch_statuses(std::slice::from_ref(&child), "main", dir.path());
        assert_eq!(merged[0].unmerged_count, 0);
        assert!(merged[0].last_commit_unix.is_some());

        let task = epic("cas-epic-dangling-anchor");
        let req = base_req(&task.id);
        assert!(
            matches!(
                run_epic_close_merge_gate(
                    &task,
                    &req,
                    "main",
                    dir.path(),
                    std::slice::from_ref(&child),
                ),
                EpicCloseGateOutcome::Proceed
            ),
            "a dangling anchor must not block close once its parked branch is legitimately merged"
        );
    }

    #[test]
    fn dangling_anchor_checks_stale_parked_and_current_assignee_branches() {
        let dir = init_epic_repo(&[("alice", 1), ("bob", 1)]);
        git(
            dir.path(),
            &["merge", "--no-ff", "-m", "merge alice", "factory/alice"],
        );

        let mut reassigned = child("cas-reassigned", TaskStatus::InProgress, Some("bob"));
        reassigned.deliverables.parked_branch = Some("factory/alice".to_string());
        reassigned.deliverables.factory_branch_anchor = Some("0".repeat(40));
        let task = epic("cas-epic-reassigned");
        let req = base_req(&task.id);

        match run_epic_close_merge_gate(
            &task,
            &req,
            "main",
            dir.path(),
            std::slice::from_ref(&reassigned),
        ) {
            EpicCloseGateOutcome::Reject(message) => {
                assert!(
                    message.contains("factory/alice"),
                    "the rejection must name the historical parked branch: {message}"
                );
                assert!(
                    message.contains("factory/bob"),
                    "the rejection must name the current assignee branch: {message}"
                );
                assert!(
                    message.contains("1 commit"),
                    "Bob's stranded commit must be visible: {message}"
                );
            }
            other => panic!("Bob's live stranded work must block epic close; got {other:?}"),
        }
    }

    /// cas-54ca: missing task-specific evidence must not turn UNKNOWN into
    /// VERIFIED-MERGED. Codex workers do not currently receive the commit-time
    /// PostToolUse hook, so this legacy/no-receipt shape occurs in practice.
    #[test]
    fn epic_close_rejects_unmerged_child_without_recorded_evidence() {
        let dir = init_epic_repo(&[("worker", 1)]);
        let child_without_receipt = Task {
            id: "cas-no-receipt".to_string(),
            title: "child with no commit-time receipt".to_string(),
            status: TaskStatus::Closed,
            assignee: Some("worker".to_string()),
            ..Default::default()
        };
        assert!(
            child_without_receipt
                .deliverables
                .factory_branch_anchor
                .is_none()
        );
        assert!(child_without_receipt.deliverables.parked_branch.is_none());
        let task = epic("cas-epic-no-receipt");
        let req = base_req(&task.id);

        let out =
            run_epic_close_merge_gate(&task, &req, "main", dir.path(), &[child_without_receipt]);
        assert!(
            matches!(out, EpicCloseGateOutcome::Reject(_)),
            "an unmerged assignee branch with no recorded anchor or parked branch \
             must fail closed; got {out:?}"
        );
    }

    // --- run_epic_close_merge_gate ------------------------------------------

    #[test]
    fn epic_close_rejects_when_any_child_factory_unmerged() {
        // 3 children, 1 has stranded commits → gate Rejects with detail.
        let dir = init_epic_repo(&[("alpha", 0), ("bravo", 3)]);
        let subtasks = vec![
            child("cas-c1", TaskStatus::Closed, Some("alpha")),
            child("cas-c2", TaskStatus::InProgress, Some("bravo")),
        ];
        let task = epic("cas-754b-test");
        let req = base_req(&task.id);

        let out = run_epic_close_merge_gate(&task, &req, "main", dir.path(), &subtasks);
        match out {
            EpicCloseGateOutcome::Reject(msg) => {
                assert!(msg.contains("MERGE REQUIRED"), "missing header: {msg}");
                assert!(msg.contains("cas-754b-test"), "missing epic id: {msg}");
                assert!(msg.contains("cas-c2"), "missing offending child id: {msg}");
                assert!(msg.contains("factory/bravo"), "missing branch: {msg}");
                assert!(msg.contains("3 commit"), "missing stranded count: {msg}");
                assert!(
                    !msg.contains("cas-c1"),
                    "must not list clean children in the rejection: {msg}"
                );
                assert!(
                    msg.contains("bypass_code_review=true"),
                    "rejection must call out bypass-immunity: {msg}"
                );
                assert!(
                    msg.contains("epic_status"),
                    "rejection must point at the diagnostic action: {msg}"
                );
                assert!(
                    msg.contains("If cleanup already removed the worktree")
                        && msg.contains("git merge --no-ff factory/bravo")
                        && msg.contains("origin/factory/bravo"),
                    "rejection must provide a branch-based remediation after worktree cleanup: {msg}"
                );
            }
            other => panic!("expected Reject for stranded child branch, got {other:?}"),
        }
    }

    #[test]
    fn epic_close_succeeds_when_all_children_merged() {
        let dir = init_epic_repo(&[("alpha", 0), ("bravo", 0), ("charlie", 0)]);
        let subtasks = vec![
            child("cas-c1", TaskStatus::Closed, Some("alpha")),
            child("cas-c2", TaskStatus::Closed, Some("bravo")),
            child("cas-c3", TaskStatus::Closed, Some("charlie")),
        ];
        let task = epic("cas-epic-clean");
        let req = base_req(&task.id);

        let out = run_epic_close_merge_gate(&task, &req, "main", dir.path(), &subtasks);
        assert!(
            matches!(out, EpicCloseGateOutcome::Proceed),
            "all-merged epic must allow close, got {out:?}"
        );
    }

    /// cas-eaf8: a worker branch is a long-lived execution lane, not a
    /// task receipt. Once task A's recorded anchor is merged into epic 1,
    /// later task B work for epic 2 on the same live branch must not
    /// re-strand epic 1.
    #[test]
    fn epic_close_uses_child_anchor_when_worker_branch_is_reused() {
        let dir = init_epic_repo(&[("worker", 1)]);
        let p = dir.path();
        let task_a_anchor = epic_git_stdout(p, &["rev-parse", "factory/worker"]);

        // Integrate task A into epic 1 (represented by main).
        git(p, &["merge", "-q", "--no-ff", "factory/worker"]);

        // Reuse the worker's one long-lived branch for task B on epic 2.
        git(p, &["checkout", "-q", "factory/worker"]);
        std::fs::write(p.join("task-b.rs"), "// unrelated epic 2 work\n").unwrap();
        git(p, &["add", "task-b.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task B on epic 2"]);

        assert_eq!(
            count_unmerged_factory_commits(p, "factory/worker", "main"),
            1,
            "precondition: the reused live branch carries epic 2 work"
        );
        assert!(
            commit_is_merged_into_parent(p, &task_a_anchor, "main"),
            "precondition: task A's own anchor is merged into epic 1"
        );

        let mut task_a = child("cas-task-a", TaskStatus::Closed, Some("worker"));
        task_a.deliverables.factory_branch_anchor = Some(task_a_anchor);
        task_a.deliverables.parked_branch = Some("factory/worker".to_string());
        let task = epic("cas-epic-1");
        let req = base_req(&task.id);

        let out = run_epic_close_merge_gate(&task, &req, "main", p, &[task_a]);
        assert!(
            matches!(out, EpicCloseGateOutcome::Proceed),
            "epic 1 must close based on task A's merged anchor, regardless of \
             task B's later commit on the reused worker branch; got {out:?}"
        );
    }

    #[test]
    fn epic_close_accepts_merged_live_tip_when_recorded_anchor_was_superseded() {
        let dir = init_epic_repo(&[("worker", 1)]);
        let p = dir.path();
        let recorded_anchor = epic_git_stdout(p, &["rev-parse", "factory/worker"]);

        git(p, &["checkout", "-q", "factory/worker"]);
        git(
            p,
            &[
                "commit",
                "-q",
                "--amend",
                "-m",
                "feat: worker patch with amended metadata",
            ],
        );
        let live_tip = epic_git_stdout(p, &["rev-parse", "HEAD"]);
        assert_ne!(recorded_anchor, live_tip);
        assert!(
            git_ref_exists(p, &recorded_anchor),
            "the superseded commit object must remain resolvable"
        );

        let remote = tempfile::tempdir().unwrap();
        git(remote.path(), &["init", "-q", "--bare"]);
        git(
            p,
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        git(p, &["push", "-q", "-u", "origin", "factory/worker"]);
        git(p, &["checkout", "-q", "main"]);
        git(
            p,
            &[
                "merge",
                "-q",
                "--no-ff",
                "-m",
                "merge amended tip",
                "factory/worker",
            ],
        );

        assert_eq!(
            count_unmerged_factory_commits(p, &recorded_anchor, "main"),
            1,
            "precondition: the superseded recorded commit is not an ancestor of main"
        );
        assert!(matches!(
            known_unmerged_factory_commits(p, "factory/worker", "main"),
            KnownUnmergedCount::KnownZero
        ));
        assert!(matches!(
            known_unmerged_factory_commits(p, "origin/factory/worker", "main"),
            KnownUnmergedCount::KnownZero
        ));

        let mut child = child("cas-superseded-anchor", TaskStatus::Closed, Some("worker"));
        child.deliverables.factory_branch_anchor = Some(recorded_anchor.clone());
        child.deliverables.parked_branch = Some("factory/worker".to_string());
        let task = epic("cas-epic-superseded-anchor");
        let req = base_req(&task.id);

        match run_epic_close_merge_gate(&task, &req, "main", p, &[child.clone()]) {
            EpicCloseGateOutcome::ProceedWithNote(note) => {
                assert!(note.contains(&recorded_anchor), "{note}");
                assert!(note.contains(&live_tip), "{note}");
                assert!(note.contains("factory/worker"), "{note}");
                assert!(note.contains("origin/factory/worker"), "{note}");
                assert!(note.contains("superseded"), "{note}");
            }
            other => panic!("merged live tip must clear a superseded anchor; got {other:?}"),
        }

        let statuses = collect_epic_branch_statuses(&[child], "main", p);
        let report = render_epic_status_report(&task.id, "main", &statuses);
        assert!(
            report.contains("Recorded/live merge-evidence reconciliation"),
            "{report}"
        );
        assert!(report.contains(&recorded_anchor), "{report}");
        assert!(report.contains(&live_tip), "{report}");
    }

    #[test]
    fn epic_close_rejects_recycled_worker_branch_that_does_not_prove_old_anchor() {
        let dir = init_epic_repo(&[("worker", 1)]);
        let p = dir.path();
        let stranded_anchor = epic_git_stdout(p, &["rev-parse", "factory/worker"]);

        git(p, &["branch", "-D", "factory/worker"]);
        git(p, &["checkout", "-q", "-b", "factory/worker", "main"]);
        std::fs::write(
            p.join("replacement-task.rs"),
            "// unrelated replacement work\n",
        )
        .unwrap();
        git(p, &["add", "replacement-task.rs"]);
        git(p, &["commit", "-q", "-m", "feat: replacement task"]);
        git(p, &["checkout", "-q", "main"]);
        git(
            p,
            &[
                "merge",
                "-q",
                "--no-ff",
                "-m",
                "merge replacement task",
                "factory/worker",
            ],
        );

        assert!(git_ref_exists(p, &stranded_anchor));
        assert_eq!(
            count_unmerged_factory_commits(p, &stranded_anchor, "main"),
            1
        );
        assert!(matches!(
            known_unmerged_factory_commits(p, "factory/worker", "main"),
            KnownUnmergedCount::KnownZero
        ));

        let mut child = child("cas-recycled-worker", TaskStatus::Closed, Some("worker"));
        child.deliverables.factory_branch_anchor = Some(stranded_anchor.clone());
        let task = epic("cas-epic-recycled-worker");
        match run_epic_close_merge_gate(&task, &base_req(&task.id), "main", p, &[child]) {
            EpicCloseGateOutcome::Reject(message) => {
                assert!(message.contains(&stranded_anchor), "{message}");
            }
            other => panic!("a recycled branch cannot prove the old task; got {other:?}"),
        }
    }

    #[test]
    fn epic_close_rejects_worker_branch_reset_to_parent_without_anchor_content() {
        let dir = init_epic_repo(&[("worker", 1)]);
        let p = dir.path();
        let stranded_anchor = epic_git_stdout(p, &["rev-parse", "factory/worker"]);
        git(p, &["branch", "-f", "factory/worker", "main"]);

        assert!(git_ref_exists(p, &stranded_anchor));
        assert_eq!(
            count_unmerged_factory_commits(p, &stranded_anchor, "main"),
            1
        );
        assert!(matches!(
            known_unmerged_factory_commits(p, "factory/worker", "main"),
            KnownUnmergedCount::KnownZero
        ));

        let mut child = child("cas-reset-worker", TaskStatus::Closed, Some("worker"));
        child.deliverables.factory_branch_anchor = Some(stranded_anchor.clone());
        let task = epic("cas-epic-reset-worker");
        match run_epic_close_merge_gate(&task, &base_req(&task.id), "main", p, &[child]) {
            EpicCloseGateOutcome::Reject(message) => {
                assert!(message.contains(&stranded_anchor), "{message}");
            }
            other => panic!("reset-to-parent cannot prove the task was merged; got {other:?}"),
        }
    }

    #[test]
    fn epic_close_surfaces_vanished_parked_branch_after_reassignment() {
        let dir = init_epic_repo(&[("alice", 1), ("bob", 0)]);
        let p = dir.path();
        let stranded_anchor = epic_git_stdout(p, &["rev-parse", "factory/alice"]);
        git(p, &["cherry-pick", "--no-commit", &stranded_anchor]);
        git(
            p,
            &[
                "commit",
                "-q",
                "-m",
                "integrate alice patch under rewritten commit",
            ],
        );
        git(p, &["branch", "-D", "factory/alice"]);

        assert_eq!(
            count_unmerged_factory_commits(p, &stranded_anchor, "main"),
            1,
            "the recorded SHA remains non-ancestral"
        );
        assert!(
            commit_patches_cherry_equivalent_on_parent(p, &stranded_anchor, "main"),
            "the anchor has task-specific content proof, so only the vanished \
             parked branch should prevent reconciliation"
        );
        assert!(matches!(
            known_unmerged_factory_commits(p, "factory/bob", "main"),
            KnownUnmergedCount::KnownZero
        ));

        let mut reassigned = child("cas-missing-parked", TaskStatus::Closed, Some("bob"));
        reassigned.deliverables.parked_branch = Some("factory/alice".to_string());
        reassigned.deliverables.factory_branch_anchor = Some(stranded_anchor.clone());
        let task = epic("cas-epic-missing-parked");
        match run_epic_close_merge_gate(&task, &base_req(&task.id), "main", p, &[reassigned]) {
            EpicCloseGateOutcome::Reject(message) => {
                assert!(message.contains(&stranded_anchor), "{message}");
                assert!(message.contains("factory/alice"), "{message}");
            }
            other => panic!("a vanished parked branch must be surfaced; got {other:?}"),
        }
    }

    /// cas-eaf8: task-specific anchoring must retain the guard's teeth.
    /// A child's own recorded anchor that is not on the epic branch still
    /// hard-blocks epic close.
    #[test]
    fn epic_close_rejects_genuinely_unmerged_child_anchor() {
        let dir = init_epic_repo(&[("worker", 1)]);
        let p = dir.path();
        let unmerged_anchor = epic_git_stdout(p, &["rev-parse", "factory/worker"]);
        let mut child = child("cas-unmerged", TaskStatus::Closed, Some("worker"));
        child.deliverables.factory_branch_anchor = Some(unmerged_anchor);
        child.deliverables.parked_branch = Some("factory/worker".to_string());
        let task = epic("cas-epic-blocked");
        let req = base_req(&task.id);

        match run_epic_close_merge_gate(&task, &req, "main", p, &[child]) {
            EpicCloseGateOutcome::Reject(msg) => {
                assert!(msg.contains("MERGE REQUIRED"), "missing hard block: {msg}");
                assert!(msg.contains("cas-unmerged"), "missing child id: {msg}");
                assert!(
                    msg.contains("factory/worker"),
                    "missing parked branch: {msg}"
                );
            }
            other => panic!("genuinely unmerged child anchor must Reject, got {other:?}"),
        }
    }

    #[test]
    fn epic_close_with_bypass_still_rejects_on_unmerged_child() {
        // Bypass-immunity at the structural level — gate has no
        // bypass parameter — and behavioral level — even with
        // bypass=Some(true) on the request, the gate rejects.
        let dir = init_epic_repo(&[("alpha", 1)]);
        let subtasks = vec![child("cas-c1", TaskStatus::InProgress, Some("alpha"))];
        let task = epic("cas-epic-bypass");
        let mut req = base_req(&task.id);
        req.bypass_code_review = Some(true);
        req.reason = Some("supervisor wants to skip review".to_string());

        let out = run_epic_close_merge_gate(&task, &req, "main", dir.path(), &subtasks);
        match out {
            EpicCloseGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("bypass_code_review=true"),
                    "rejection must spell out bypass-immunity policy: {msg}"
                );
            }
            other => panic!("bypass_code_review must NOT skip the epic merge gate, got {other:?}"),
        }
    }

    #[test]
    fn epic_close_gate_skips_non_epic_tasks() {
        // Symmetrical to cas-95ce's per-task gate: this one only fires
        // on Epic-type tasks.
        let dir = init_epic_repo(&[("alpha", 5)]);
        let subtasks = vec![child("cas-c1", TaskStatus::InProgress, Some("alpha"))];
        let task = child("cas-not-epic", TaskStatus::InProgress, None); // non-epic
        let req = base_req(&task.id);

        let out = run_epic_close_merge_gate(&task, &req, "main", dir.path(), &subtasks);
        assert!(
            matches!(out, EpicCloseGateOutcome::Proceed),
            "non-epic task must skip this gate, got {out:?}"
        );
    }

    // --- snapshot test on report shape --------------------------------------

    #[test]
    fn epic_status_report_snapshot_shape_is_stable() {
        // Pin the exact report layout. Future contributors changing
        // the markdown structure must update this assertion deliberately.
        let statuses = vec![
            EpicChildBranchStatus {
                task_id: "cas-aaaa".to_string(),
                task_status: TaskStatus::Closed,
                assignee: Some("alpha".to_string()),
                recorded_anchor: None,
                factory_branch: Some("factory/alpha".to_string()),
                additional_factory_branches: Vec::new(),
                unmerged_count: 0,
                last_commit_unix: Some(1735689600), // 2025-01-01 00:00 UTC
                merge_evidence_note: None,
            },
            EpicChildBranchStatus {
                task_id: "cas-bbbb".to_string(),
                task_status: TaskStatus::InProgress,
                assignee: Some("bravo".to_string()),
                recorded_anchor: None,
                factory_branch: Some("factory/bravo".to_string()),
                additional_factory_branches: Vec::new(),
                unmerged_count: 2,
                last_commit_unix: Some(1735776000), // 2025-01-02 00:00 UTC
                merge_evidence_note: None,
            },
            EpicChildBranchStatus {
                task_id: "cas-cccc".to_string(),
                task_status: TaskStatus::InProgress,
                assignee: None,
                recorded_anchor: None,
                factory_branch: None,
                additional_factory_branches: Vec::new(),
                unmerged_count: 0,
                last_commit_unix: None,
                merge_evidence_note: None,
            },
        ];
        let report = render_epic_status_report("cas-754b", "epic/foo", &statuses);

        // Status column uses TaskStatus's Display impl (snake_case:
        // closed, in_progress) per round-1 cas-code-review fix —
        // matches the rest of the CLI's status rendering.
        let expected = "\
Epic cas-754b — factory branch status\n\
Parent branch: epic/foo\n\
\n\
| Task | Status | Assignee | Factory branch | Unmerged | Last commit |\n\
|------|--------|----------|----------------|----------|-------------|\n\
| cas-aaaa | closed | alpha | factory/alpha | 0 | 2025-01-01 00:00 UTC |\n\
| cas-bbbb | in_progress | bravo | factory/bravo | 2 | 2025-01-02 00:00 UTC |\n\
| cas-cccc | in_progress | — | — | — | — |\n\
\n\
⚠️  1 child task(s) carry stranded factory commits. \
Epic close will be hard-blocked until they are merged.\n";

        assert_eq!(
            report, expected,
            "report shape regressed; review and update if intentional"
        );
    }

    // --- Lower-level helpers ------------------------------------------------

    #[test]
    fn format_unix_timestamp_is_iso_utc() {
        // 1735689600 = 2025-01-01T00:00:00Z
        assert_eq!(format_unix_timestamp(1735689600), "2025-01-01 00:00 UTC");
    }

    #[test]
    fn last_commit_unix_returns_none_for_missing_branch() {
        let dir = init_epic_repo(&[]);
        assert_eq!(last_commit_unix(dir.path(), "factory/ghost"), None);
    }

    #[test]
    fn last_commit_unix_returns_some_for_existing_branch() {
        let dir = init_epic_repo(&[("alpha", 1)]);
        let ts = last_commit_unix(dir.path(), "factory/alpha");
        assert!(ts.is_some(), "branch with commits must yield Some(ts)");
        assert!(ts.unwrap() > 0);
    }
}

#[cfg(test)]
mod commit_claim_integrity_tests {
    //! cas-490f: regression tests for the commit-claim integrity gate.
    //!
    //! The cas-ba91 incident: a factory worker fabricated a commit SHA and
    //! non-empty code_review_findings while their branch carried 0 commits
    //! beyond the base. The supervisor lost ~10 min before detection.
    //!
    //! This module tests:
    //!   - `count_worker_branch_commits` — counts HEAD commits vs parent
    //!   - `get_worker_diff_stat` — returns `git diff --stat` summary
    //!   - `check_commit_claim_integrity` — the gate helper that ties them
    //!     together (returns Proceed/Reject based on findings + commit count)
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Minimal worker repo: `main` with one seed commit, then branch off
    /// to `factory/test-worker`. Caller can add commits on top.
    fn init_worker_repo() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        git(p, &["checkout", "-q", "-b", "factory/test-worker"]);
        dir
    }

    // ── count_worker_branch_commits ──────────────────────────────────────────

    #[test]
    fn count_worker_returns_zero_with_no_commits_beyond_base() {
        // Worker branched off main but made no commits — this is the
        // fabrication scenario (0 commits on the branch).
        let dir = init_worker_repo();
        assert_eq!(
            count_worker_branch_commits(dir.path(), "main"),
            0,
            "fresh worker branch with no commits beyond base must count 0"
        );
    }

    #[test]
    fn count_worker_returns_correct_count_for_multiple_commits() {
        let dir = init_worker_repo();
        for name in ["a.rs", "b.rs", "c.rs"] {
            std::fs::write(dir.path().join(name), format!("// {name}\n")).unwrap();
            git(dir.path(), &["add", name]);
            git(dir.path(), &["commit", "-q", "-m", &format!("add {name}")]);
        }
        assert_eq!(
            count_worker_branch_commits(dir.path(), "main"),
            3,
            "3 commits on worker branch beyond base must count 3"
        );
    }

    #[test]
    fn count_worker_returns_zero_for_non_git_dir() {
        // Graceful degradation: non-git directory must not panic or error;
        // it returns 0 so the gate does not false-reject.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            count_worker_branch_commits(dir.path(), "main"),
            0,
            "non-git dir must degrade to 0"
        );
    }

    // ── get_worker_diff_stat ─────────────────────────────────────────────────

    #[test]
    fn get_diff_stat_returns_non_empty_for_committed_files() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("work.rs"), "fn foo() {}\n").unwrap();
        git(dir.path(), &["add", "work.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "add work"]);

        let stat = get_worker_diff_stat(dir.path(), "main");
        assert!(
            !stat.is_empty(),
            "diff stat must be non-empty when commits exist"
        );
        assert!(
            stat.contains("work.rs"),
            "diff stat must mention the committed file; got: {stat}"
        );
    }

    #[test]
    fn get_diff_stat_returns_empty_for_no_commits() {
        let dir = init_worker_repo();
        // No commits beyond main — diff stat must be empty.
        let stat = get_worker_diff_stat(dir.path(), "main");
        assert!(
            stat.is_empty(),
            "diff stat must be empty when no commits exist beyond base; got: {stat}"
        );
    }

    // ── cas-e093: bounded diff stat ──────────────────────────────────────────

    /// Pure-logic test for the truncation/annotation policy, independent of
    /// git: below-cap input passes through unchanged.
    #[test]
    fn cap_diff_stat_output_below_cap_is_unchanged() {
        let raw = " a.txt | 1 +\n 1 file changed, 1 insertion(+)";
        assert_eq!(cap_diff_stat_output(raw, 40), raw);
    }

    /// Pure-logic test: above-cap input gets an explicit "… and M more
    /// files" line derived from git's own trailing summary, and git's bare
    /// "..." marker (if present) is dropped rather than left in.
    #[test]
    fn cap_diff_stat_output_truncates_and_annotates() {
        let raw = " a.txt | 1 +\n b.txt | 1 +\n ...\n 3 files changed, 3 insertions(+)";
        let capped = cap_diff_stat_output(raw, 2);
        assert!(
            capped.contains("and 1 more files"),
            "must state exactly how many files were hidden; got: {capped}"
        );
        assert!(capped.contains("a.txt") && capped.contains("b.txt"));
        assert!(
            !capped.contains("..."),
            "git's bare truncation marker must be replaced, not left in: {capped}"
        );
        assert!(
            capped.ends_with("3 files changed, 3 insertions(+)"),
            "summary line must be preserved verbatim: {capped}"
        );
    }

    #[test]
    fn parse_files_changed_handles_singular_and_plural_and_junk() {
        assert_eq!(
            parse_files_changed(" 1 file changed, 1 insertion(+)"),
            Some(1)
        );
        assert_eq!(
            parse_files_changed(" 50 files changed, 50 insertions(+)"),
            Some(50)
        );
        assert_eq!(parse_files_changed("not a summary line"), None);
    }

    /// End-to-end (real git): a diff far wider than `DIFF_STAT_MAX_FILES`
    /// must produce a small, bounded result with an explicit remainder
    /// count — directly reproducing the bug doc's evidence shape (a
    /// long-lived branch differing across ~1700 files used to spill
    /// ~110KB and overflow the MCP tool-result token limit).
    #[test]
    fn get_diff_stat_synthetic_1700_file_diff_stays_small() {
        let dir = init_worker_repo();
        for i in 0..1700 {
            std::fs::write(dir.path().join(format!("wide{i}.txt")), "x\n").unwrap();
        }
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "wide diff"]);

        let stat = get_worker_diff_stat(dir.path(), "main");
        assert!(
            stat.len() < 4096,
            "a 1700-file diff must produce a small, bounded result \
             (bug doc evidence: unbounded == ~110,000 bytes); got {} bytes",
            stat.len()
        );
        assert!(
            stat.contains("more files"),
            "must indicate truncation for a diff this wide; got: {stat}"
        );
    }

    /// Sanity: a diff below the cap must NOT be annotated as truncated —
    /// the common small-task case is unaffected by the cas-e093 cap.
    #[test]
    fn get_diff_stat_below_cap_lists_every_file_untruncated() {
        let dir = init_worker_repo();
        for name in ["a.rs", "b.rs", "c.rs"] {
            std::fs::write(dir.path().join(name), format!("// {name}\n")).unwrap();
            git(dir.path(), &["add", name]);
        }
        git(dir.path(), &["commit", "-q", "-m", "add three files"]);

        let stat = get_worker_diff_stat(dir.path(), "main");
        assert!(
            !stat.contains("more files"),
            "small diff must not be flagged as truncated; got: {stat}"
        );
        for name in ["a.rs", "b.rs", "c.rs"] {
            assert!(stat.contains(name), "must list {name}; got: {stat}");
        }
    }

    /// cas-7efe (AC3): the close-time diff stat must be computed against
    /// the task's real parent (the epic branch), not a divergent trunk —
    /// otherwise it lists the trunk's entire unrelated history instead of
    /// the task's own contribution
    /// (BUG-task-close-returns-110kb-diffstat-overflowing-token-limit.md).
    #[test]
    fn get_diff_stat_against_epic_excludes_unrelated_trunk_divergence() {
        // `get_worker_diff_stat` diffs `merge-base(HEAD, parent_branch)..HEAD`
        // — so the bug only reproduces when the merge-base against the
        // WRONG branch ("main") is genuinely older/different than the
        // merge-base against the real parent (the epic). Mirror the bug
        // doc's actual shape: `staging` (the real trunk) has drifted far
        // from `main` with unrelated history, and the epic branches from
        // staging's current tip — so diffing the worker branch against
        // `main` walks all the way back through staging's entire
        // unrelated drift.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("root.txt"), "root\n").unwrap();
        git(p, &["add", "root.txt"]);
        git(p, &["commit", "-q", "-m", "root"]);

        // `staging` diverges from `main` with a lot of unrelated history —
        // `main` itself never advances past `root`.
        git(p, &["checkout", "-q", "-b", "staging"]);
        for i in 0..30 {
            std::fs::write(p.join(format!("unrelated{i}.txt")), "noise\n").unwrap();
        }
        git(p, &["add", "."]);
        git(p, &["commit", "-q", "-m", "staging drift"]);

        // The epic branches from staging's current (drifted) tip.
        git(p, &["checkout", "-q", "-b", "epic/foo"]);

        // Worker branches off the epic and touches exactly one file.
        git(p, &["checkout", "-q", "-b", "factory/worker"]);
        std::fs::write(p.join("task_file.rs"), "fn work() {}\n").unwrap();
        git(p, &["add", "task_file.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task work"]);

        let stat_vs_epic = get_worker_diff_stat(p, "epic/foo");
        assert!(
            stat_vs_epic.contains("task_file.rs"),
            "must list the task's own file; got: {stat_vs_epic}"
        );
        assert!(
            !stat_vs_epic.contains("unrelated0.txt"),
            "must NOT include trunk-only divergence when diffed against \
             the real epic parent; got: {stat_vs_epic}"
        );

        // Sanity: proves what the bug looked like when the wrong base
        // (main) was used instead of the epic branch — this pulls in
        // staging's entire unrelated drift, exactly the 110KB overflow.
        let stat_vs_main = get_worker_diff_stat(p, "main");
        assert!(
            stat_vs_main.contains("unrelated0.txt"),
            "sanity: diffing vs the wrong base pulls in the unrelated \
             trunk divergence — exactly the 110KB overflow bug; got: {stat_vs_main}"
        );
    }

    // ── check_commit_claim_integrity ─────────────────────────────────────────

    /// Reproduces the cas-ba91 incident: worker provides non-empty
    /// code_review_findings but the branch has 0 commits beyond the base.
    #[test]
    fn fabrication_detected_when_zero_commits_with_review_findings() {
        let dir = init_worker_repo();
        // No commits beyond base — fabrication scenario.
        let outcome = check_commit_claim_integrity(dir.path(), "main", true, None, None, None);
        match outcome {
            CommitClaimGateOutcome::Reject(msg) => {
                assert!(
                    msg.contains("FABRICATION DETECTED"),
                    "rejection must name the gate; got: {msg}"
                );
                assert!(
                    msg.contains("code_review_findings"),
                    "rejection must identify the fabrication signal; got: {msg}"
                );
                assert!(
                    msg.contains("0 commit"),
                    "rejection must show the 0-commit count; got: {msg}"
                );
            }
            CommitClaimGateOutcome::Proceed => {
                panic!("gate must reject zero-commit + findings = fabrication scenario (cas-ba91)");
            }
            CommitClaimGateOutcome::ProceedWithReceipt(_) => {
                panic!("gate must not accept an absent receipt")
            }
        }
    }

    #[test]
    fn zero_commits_without_review_findings_proceeds() {
        // Worker did documentation-only work and did not supply
        // code_review_findings. Empty branch is fine in that case.
        let dir = init_worker_repo();
        let outcome = check_commit_claim_integrity(dir.path(), "main", false, None, None, None);
        assert!(
            matches!(outcome, CommitClaimGateOutcome::Proceed),
            "no-findings close on empty branch must proceed (no fabrication claim)"
        );
    }

    #[test]
    fn commits_present_with_review_findings_proceeds() {
        // Worker did real work: commits on branch + findings provided.
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("real.rs"), "fn real() {}\n").unwrap();
        git(dir.path(), &["add", "real.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "real work"]);

        let outcome = check_commit_claim_integrity(dir.path(), "main", true, None, None, None);
        assert!(
            matches!(outcome, CommitClaimGateOutcome::Proceed),
            "commits + findings must proceed (worker did real work)"
        );
    }

    /// cas-127f: findings + empty merge-base..HEAD after supervisor merge is
    /// not fabrication when the parked factory tip is an ancestor of parent.
    #[test]
    fn findings_after_merge_satisfied_anchor_proceeds() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("real.rs"), "fn real() {}\n").unwrap();
        git(dir.path(), &["add", "real.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "real work"]);
        let anchor = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        // Supervisor merges factory tip into main (no-ff).
        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge worker",
                "factory/test-worker",
            ],
        );
        // Worker tip is now even with parent (or still at anchor which is
        // ancestor) — count beyond parent is 0 either way after checkout.
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["reset", "--hard", "main"]);

        assert_eq!(
            count_worker_branch_commits(dir.path(), "main"),
            0,
            "post-merge worker tip must not be ahead of parent"
        );
        let outcome =
            check_commit_claim_integrity(dir.path(), "main", true, Some(&anchor), None, None);
        assert!(
            matches!(outcome, CommitClaimGateOutcome::Proceed),
            "merge-satisfied anchor must not look like fabrication"
        );
    }
}

#[cfg(test)]
mod zero_change_close_tests {
    //! cas-ee2b: regression tests for the "no-diff close" fix.
    //!
    //! The cas-cabc incident: a researcher closed a spike task with zero code
    //! commits. The main repo had unrelated dirty files (normal during an active
    //! session). `has_reviewable_changes(close_project_root)` returned true
    //! (main repo dirty), triggering CODE_REVIEW_REQUIRED on a task that never
    //! touched code.
    //!
    //! Fix: `has_worker_committed_reviewable_changes(worker_wt, parent_branch)`
    //! checks the worker's committed diff instead of the main repo working tree.
    //!
    //! Tests cover:
    //! - Zero commits → false (the cas-cabc scenario)
    //! - Docs-only commits → false (*.md, docs/)
    //! - Code commits → true (Rust source, TypeScript, etc.)
    //! - Non-git dir → false (graceful degradation)
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_worker_repo() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        git(p, &["checkout", "-q", "-b", "factory/test-worker"]);
        dir
    }

    fn head_sha(dir: &Path) -> String {
        String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(dir)
                .output()
                .expect("rev-parse HEAD")
                .stdout,
        )
        .expect("utf8 SHA")
        .trim()
        .to_string()
    }

    fn test_receipt_window() -> TaskCommitReceiptWindow {
        TaskCommitReceiptWindow {
            not_before: chrono::Utc::now() - chrono::Duration::hours(1),
            basis: "test fixture",
            task_floor: chrono::Utc::now() - chrono::Duration::hours(2),
            identity: TaskCommitIdentity::default(),
        }
    }

    // ── has_worker_committed_reviewable_changes ──────────────────────────────

    /// Reproduces the cas-cabc scenario: researcher closes spike with zero
    /// code commits. Must return false so the review gate is skipped.
    #[test]
    fn zero_commits_returns_false() {
        let dir = init_worker_repo();
        // No commits beyond base.
        assert!(
            !has_worker_committed_reviewable_changes(dir.path(), "main"),
            "zero commits beyond base must not be reviewable (cas-cabc scenario)"
        );
    }

    #[test]
    fn docs_only_commits_return_false() {
        let dir = init_worker_repo();
        // Only markdown and docs/ files committed — not reviewable.
        std::fs::write(dir.path().join("NOTES.md"), "# notes\n").unwrap();
        git(dir.path(), &["add", "NOTES.md"]);
        git(dir.path(), &["commit", "-q", "-m", "docs: add notes"]);

        assert!(
            !has_worker_committed_reviewable_changes(dir.path(), "main"),
            "docs-only commits must not trigger the review gate"
        );
    }

    #[test]
    fn code_commits_return_true() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("lib.rs"), "pub fn foo() {}\n").unwrap();
        git(dir.path(), &["add", "lib.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: add lib"]);

        assert!(
            has_worker_committed_reviewable_changes(dir.path(), "main"),
            "code file commits must be reviewable"
        );
    }

    #[test]
    fn mixed_docs_and_code_commits_return_true() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("README.md"), "# readme\n").unwrap();
        std::fs::write(dir.path().join("main.ts"), "export function x() {}\n").unwrap();
        git(dir.path(), &["add", "README.md", "main.ts"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: mixed commit"]);

        assert!(
            has_worker_committed_reviewable_changes(dir.path(), "main"),
            "commit containing both docs and code must be reviewable"
        );
    }

    #[test]
    fn non_git_dir_returns_false() {
        // Graceful degradation: non-git directory must not panic or error.
        let dir = tempfile::tempdir().unwrap();
        assert!(
            !has_worker_committed_reviewable_changes(dir.path(), "main"),
            "non-git dir must degrade to false (no false-require-review)"
        );
    }

    // ── check_zero_commit_close (case 1/3/4) ────────────────────────────────

    /// Case 1: docs-only commits (commit_count > 0, no reviewable files).
    /// Worker committed something — the reviewable check correctly saw no code.
    /// This is NOT case 3; gate must proceed.
    #[test]
    fn case1_docs_only_commits_proceeds() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("NOTES.md"), "# notes\n").unwrap();
        git(dir.path(), &["add", "NOTES.md"]);
        git(dir.path(), &["commit", "-q", "-m", "docs: add notes"]);

        // count_worker_branch_commits == 1 → case 1, not case 3
        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-test1",
            &TaskType::Bug,
            None,  // no execution_note
            false, // no review findings
            None,  // factory_branch_anchor
            None,  // commit_receipt
            None,  // commit_receipt_window
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::Proceed),
            "docs-only commits must route to Proceed (case 1)"
        );
    }

    /// Case 3: ambiguous zero-commit close — reproduces cas-cabc scenario
    /// for a bug task with no hints that this is code-free work.
    #[test]
    fn case3_zero_commit_bug_task_no_hint_rejects() {
        let dir = init_worker_repo();
        // No commits beyond base — the ambiguous scenario.
        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-test1",
            &TaskType::Bug,
            None,  // no execution_note
            false, // no review findings
            None,  // factory_branch_anchor
            None,  // commit_receipt
            None,  // commit_receipt_window
        );
        match outcome {
            ZeroCommitCloseOutcome::AmbiguousCodeTask(msg) => {
                assert!(
                    msg.contains("ZERO-COMMIT CLOSE ON CODE TASK"),
                    "rejection must name the gate: {msg}"
                );
                assert!(
                    msg.contains("execution_note"),
                    "rejection must guide worker to set execution_note: {msg}"
                );
                assert!(
                    msg.contains("commit_receipt=<sha>")
                        && msg.contains("ask the supervisor")
                        && msg.contains("bypass_code_review=true")
                        && msg.contains("Only a supervisor"),
                    "rejection must name both the worker receipt path and the \
                     audited supervisor fallback: {msg}"
                );
                assert!(
                    msg.contains("out-of-band merge after conflict rework")
                        && msg.contains("worker task commit OR the merge commit")
                        && msg.contains("never an unrelated historical commit"),
                    "cleared-anchor guidance must identify an attributable task or \
                     merge commit, never an unrelated historical commit: {msg}"
                );
            }
            ZeroCommitCloseOutcome::Proceed => {
                panic!("case 3 must reject ambiguous zero-commit bug task");
            }
            ZeroCommitCloseOutcome::ProceedWithReceipt(_) => {
                panic!("gate must not accept an absent receipt")
            }
        }
    }

    /// cas-3d37: literal merge-before-first-close shape. Commit-time capture
    /// recorded `anchor` before any close attempt; the supervisor then merged
    /// the worker branch, leaving it zero-ahead. The first close gate check
    /// must recognize the task's merged work and proceed.
    #[test]
    fn merge_before_first_close_uses_commit_time_anchor() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("work.rs"), "fn work() {}\n").unwrap();
        git(dir.path(), &["add", "work.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "fix: task work"]);
        let anchor = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Supervisor merges without any prior close/MERGE REQUIRED park.
        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge worker before close",
                "factory/test-worker",
            ],
        );
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        assert_eq!(count_worker_branch_commits(dir.path(), "main"), 0);

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-merge-before-close",
            &TaskType::Bug,
            None,
            false,
            Some(&anchor),
            None,
            None,
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::Proceed),
            "first close after a supervisor merge must accept the commit-time \
             task anchor, got {outcome:?}"
        );
    }

    /// Case 4a: zero commits but execution_note set → deliberate no-code signal.
    #[test]
    fn case4_zero_commits_with_execution_note_proceeds() {
        let dir = init_worker_repo();
        // No commits, but worker explicitly set execution_note.
        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-test1",
            &TaskType::Bug,
            Some("additive-only"), // explicit no-code signal
            false,
            None, // factory_branch_anchor
            None, // commit_receipt
            None, // commit_receipt_window
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::Proceed),
            "execution_note set must allow close (case 4)"
        );
    }

    /// Case 4b: zero commits on a Chore/Spike task → non-code-expecting type.
    #[test]
    fn case4_zero_commits_on_chore_or_spike_proceeds() {
        let dir = init_worker_repo();
        for task_type in [TaskType::Chore, TaskType::Epic] {
            let outcome = check_zero_commit_close(
                dir.path(),
                "main",
                "cas-test1",
                &task_type,
                None,
                false,
                None,
                None,
                None,
            );
            assert!(
                matches!(outcome, ZeroCommitCloseOutcome::Proceed),
                "{task_type:?} task type must not be flagged as ambiguous"
            );
        }
    }

    /// Case 4c: review findings present → cas-490f handles that; this gate
    /// should Proceed and not double-reject.
    #[test]
    fn case4_review_findings_present_defers_to_490f_gate() {
        let dir = init_worker_repo();
        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-test1",
            &TaskType::Bug,
            None,
            true, // has_review_findings = true (cas-490f rejects, not this gate)
            None, // factory_branch_anchor
            None, // commit_receipt
            None, // commit_receipt_window
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::Proceed),
            "when review findings are present, the cas-490f gate owns the rejection"
        );
    }

    /// cas-9eae ("sync ≠ work"): a worker who merely syncs their branch to
    /// the parent tip via a non-fast-forward merge (`git merge --no-ff`)
    /// produces a commit `count_worker_branch_commits` > 0 with a
    /// completely empty diff — no task-relevant content whatsoever. The
    /// bug doc's confirmed repro (cas-0b7d, cli=claude worker
    /// vivid-octopus-81) is exactly this: "the worker did produce a HEAD
    /// change (a fast-forward/merge to the epic tip) but zero task-relevant
    /// diff, so any guard that only checks 'did HEAD move?' would be
    /// fooled." A pure fast-forward sync (no `--no-ff`) already yields 0
    /// commits and is caught by `case3_zero_commit_bug_task_no_hint_rejects`
    /// above; this test locks in the non-fast-forward variant, which the
    /// commit-count-only check does not catch.
    #[test]
    fn case3_sync_only_merge_commit_with_empty_diff_rejects() {
        let dir = init_worker_repo();
        // Advance "main" (the parent/epic branch) with an unrelated commit
        // so the worker's merge is not itself a no-op.
        git(dir.path(), &["checkout", "-q", "main"]);
        std::fs::write(dir.path().join("epic_progress.txt"), "epic moved on\n").unwrap();
        git(dir.path(), &["add", "epic_progress.txt"]);
        git(
            dir.path(),
            &["commit", "-q", "-m", "unrelated epic progress"],
        );
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        // Worker never touches any file — just syncs to the new parent tip
        // via a forced merge commit (not a fast-forward).
        git(
            dir.path(),
            &["merge", "--no-ff", "-m", "sync to epic tip", "main"],
        );

        // The merge commit means count_worker_branch_commits() > 0, so a
        // commit-count-only gate would wrongly Proceed here.
        assert!(
            count_worker_branch_commits(dir.path(), "main") > 0,
            "sanity: the sync merge must itself count as a commit beyond the old base"
        );

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-test1",
            &TaskType::Bug,
            None,  // no execution_note
            false, // no review findings
            None,  // factory_branch_anchor
            None,  // commit_receipt
            None,  // commit_receipt_window
        );
        match outcome {
            ZeroCommitCloseOutcome::AmbiguousCodeTask(msg) => {
                assert!(
                    msg.contains("ZERO-COMMIT CLOSE ON CODE TASK") || msg.contains("NO-DIFF CLOSE"),
                    "rejection must name a zero-work gate: {msg}"
                );
            }
            ZeroCommitCloseOutcome::Proceed => {
                panic!(
                    "sync-only merge commit with zero task-relevant diff must still \
                     be rejected as ambiguous — 'did HEAD move' is not sufficient, \
                     per cas-9eae"
                );
            }
            ZeroCommitCloseOutcome::ProceedWithReceipt(_) => {
                panic!("gate must not accept an absent receipt")
            }
        }
    }

    // ── cas-127f: post-MERGE-REQUIRED zero-commit false-positive ───────────

    /// Happy path: worker commits C, MERGE REQUIRED records C as anchor,
    /// supervisor merges C into parent, worker tip is no longer ahead →
    /// count==0 but anchor is ancestor → Proceed (not ZERO-COMMIT).
    #[test]
    fn cas127f_post_merge_ancestor_anchor_proceeds() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("fix.rs"), "pub fn work() {}\n").unwrap();
        git(dir.path(), &["add", "fix.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: task work"]);
        let anchor = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Sanity: before merge, zero-commit gate would Proceed via count>0.
        assert!(count_worker_branch_commits(dir.path(), "main") > 0);

        // Supervisor merges factory tip into main.
        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge factory/test-worker",
                "factory/test-worker",
            ],
        );
        // Worker factory branch reset to epic tip (post-merge sync).
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["reset", "--hard", "main"]);

        assert_eq!(
            count_worker_branch_commits(dir.path(), "main"),
            0,
            "post-merge: merge-base..HEAD must be empty (the false-positive fixture)"
        );
        assert!(
            commit_is_merged_into_parent(dir.path(), &anchor, "main"),
            "parked anchor must be ancestor of parent after merge"
        );

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-127f",
            &TaskType::Bug,
            None,
            false,
            Some(&anchor),
            None,
            None,
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::Proceed),
            "merge-satisfied anchor must Proceed, not ZERO-COMMIT; got {outcome:?}"
        );
    }

    /// Edge: genuine never-had-commits code task still rejects (no anchor).
    #[test]
    fn cas127f_genuine_zero_commit_without_anchor_still_rejects() {
        let dir = init_worker_repo();
        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-127f",
            &TaskType::Bug,
            None,
            false,
            None,
            None,
            None,
        );
        match outcome {
            ZeroCommitCloseOutcome::AmbiguousCodeTask(msg) => {
                assert!(
                    msg.contains("ZERO-COMMIT CLOSE ON CODE TASK"),
                    "genuine zero-commit must still name the gate: {msg}"
                );
            }
            ZeroCommitCloseOutcome::Proceed => {
                panic!("genuine zero-commit without anchor must not Proceed");
            }
            ZeroCommitCloseOutcome::ProceedWithReceipt(_) => {
                panic!("gate must not accept an absent receipt")
            }
        }
    }

    /// Edge: anchor set but not integrated (still unmerged) + count 0 —
    /// e.g. factory tip was force-reset without merging. Fail closed.
    #[test]
    fn cas127f_unmerged_anchor_with_empty_ahead_still_rejects() {
        let dir = init_worker_repo();
        // Commit work, record anchor, then hard-reset factory away so the
        // commit is orphaned from parent *and* HEAD is back at base.
        std::fs::write(dir.path().join("orphan.rs"), "fn orphan() {}\n").unwrap();
        git(dir.path(), &["add", "orphan.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "orphaned work"]);
        let anchor = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        git(dir.path(), &["reset", "--hard", "main"]);

        assert_eq!(count_worker_branch_commits(dir.path(), "main"), 0);
        assert!(
            !commit_is_merged_into_parent(dir.path(), &anchor, "main"),
            "orphan anchor must not be considered merged"
        );

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-127f",
            &TaskType::Feature,
            None,
            false,
            Some(&anchor),
            None,
            None,
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::AmbiguousCodeTask(_)),
            "unmerged anchor must not unlock zero-commit; got {outcome:?}"
        );
    }

    // ── cas-cab3 (GH #128): merge evidence outranks the no-diff heuristic ──

    /// Build the exact GH #128 shape and hand back the receipt SHA:
    /// worker commits real work → supervisor merges it into the parent →
    /// worker syncs the branch with the parent tip via `git merge --no-ff`.
    /// The branch now holds ONE commit (the sync merge) whose diff vs parent
    /// is empty, while the receipt commit is a merged ancestor carrying real
    /// files. That is what a finished task looks like, not a no-code close.
    fn build_gh128_post_merge_sync(dir: &Path) -> String {
        std::fs::write(dir.join("guard_fix.rs"), "pub fn guard() {}\n").unwrap();
        git(dir, &["add", "guard_fix.rs"]);
        git(dir, &["commit", "-q", "-m", "fix: real task work"]);
        let receipt = head_sha(dir);

        // Supervisor merges the factory branch into the parent.
        git(dir, &["checkout", "-q", "main"]);
        git(
            dir,
            &[
                "merge",
                "--no-ff",
                "-m",
                "Merge branch 'factory/test-worker'",
                "factory/test-worker",
            ],
        );
        // Parent moves on (a sibling lane lands), so the worker's later sync
        // is a genuine non-fast-forward merge, exactly as in the incident.
        std::fs::write(dir.join("sibling_lane.rs"), "pub fn sibling() {}\n").unwrap();
        git(dir, &["add", "sibling_lane.rs"]);
        git(dir, &["commit", "-q", "-m", "sibling lane work"]);

        // Worker syncs with the epic tip — the ONLY commit unique to the
        // branch is now a zero-diff merge.
        git(dir, &["checkout", "-q", "factory/test-worker"]);
        git(dir, &["merge", "--no-ff", "-m", "sync to epic tip", "main"]);
        receipt
    }

    /// THE GH #128 REGRESSION: receipt commit merged to the parent, branch is
    /// parent tip + sync-merge only → close must pass on the receipt.
    ///
    /// Before this fix the no-diff heuristic rejected first and never looked
    /// at the receipt, which pushed workers into `git reset --hard <epic tip>`
    /// + force-push as routine post-merge hygiene.
    #[test]
    fn cascab3_merged_receipt_beats_no_diff_after_post_merge_sync_gh128() {
        let dir = init_worker_repo();
        let receipt = build_gh128_post_merge_sync(dir.path());

        // Sanity: this really is the no-diff branch of the guard, not the
        // zero-commit one — the sync merge counts, its diff is empty.
        assert!(
            count_worker_branch_commits(dir.path(), "main") > 0,
            "sanity: the sync merge must count as a commit beyond the merge base"
        );
        assert!(
            get_worker_diff_stat(dir.path(), "main").trim().is_empty(),
            "sanity: a synced post-merge branch must have an empty diff vs parent"
        );

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-cab3",
            &TaskType::Bug,
            None,  // no execution_note
            false, // no review findings
            None,  // no anchor — receipt is the only evidence
            Some(&receipt),
            Some(&test_receipt_window()),
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::ProceedWithReceipt(_)),
            "a merged receipt with a real diff must carry the close after a \
             post-merge sync; got {outcome:?}"
        );
    }

    /// Same shape, evidence supplied as the anchor instead of the receipt.
    #[test]
    fn cascab3_merged_anchor_beats_no_diff_after_post_merge_sync_gh128() {
        let dir = init_worker_repo();
        let anchor = build_gh128_post_merge_sync(dir.path());

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-cab3",
            &TaskType::Bug,
            None,
            false,
            Some(&anchor),
            None,
            None,
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::Proceed),
            "a merged factory anchor must satisfy the no-diff path too; got {outcome:?}"
        );
    }

    /// STILL REJECTED #1: no receipt and no unique diff (the cas-9eae case
    /// the guard exists for). Its wording must point at the receipt remedy
    /// and must not read as an invitation to rewrite the branch.
    #[test]
    fn cascab3_no_diff_without_evidence_still_rejects_and_names_the_receipt_remedy() {
        let dir = init_worker_repo();
        // Parent moves; worker only syncs. No task work was ever committed.
        git(dir.path(), &["checkout", "-q", "main"]);
        std::fs::write(dir.path().join("epic_progress.txt"), "epic moved on\n").unwrap();
        git(dir.path(), &["add", "epic_progress.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "unrelated epic progress"]);
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["merge", "--no-ff", "-m", "sync only", "main"]);

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-cab3",
            &TaskType::Bug,
            None,
            false,
            None, // no anchor
            None, // no receipt
            None,
        );
        let ZeroCommitCloseOutcome::AmbiguousCodeTask(msg) = outcome else {
            panic!("a sync-only branch with no evidence at all must still be rejected");
        };
        assert!(
            msg.contains("NO-DIFF CLOSE ON CODE TASK"),
            "the guard must still name itself: {msg}"
        );
        assert!(
            msg.contains("commit_receipt=<sha>"),
            "the refusal must name the receipt remedy: {msg}"
        );
        assert!(
            msg.contains("do NOT reset or force-push"),
            "the refusal must steer away from branch surgery (GH #128): {msg}"
        );
    }

    /// STILL REJECTED #2: a receipt whose commit carries an empty diff is not
    /// evidence of work, no matter that it is an ancestor of the parent.
    #[test]
    fn cascab3_no_diff_with_empty_diff_receipt_still_rejects() {
        let dir = init_worker_repo();
        // An empty commit on the parent — merged, but contributes nothing.
        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &["commit", "-q", "--allow-empty", "-m", "empty parent commit"],
        );
        let empty_receipt = head_sha(dir.path());
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["merge", "--no-ff", "-m", "sync only", "main"]);

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-cab3",
            &TaskType::Bug,
            None,
            false,
            None,
            Some(&empty_receipt),
            Some(&test_receipt_window()),
        );
        let ZeroCommitCloseOutcome::AmbiguousCodeTask(msg) = outcome else {
            panic!("an empty-diff receipt must not carry a close");
        };
        assert!(
            msg.contains("INVALID TASK COMMIT RECEIPT"),
            "an offered-but-invalid receipt must be rejected on its own terms: {msg}"
        );
    }

    /// STILL REJECTED #3: a receipt that is NOT an ancestor of the parent —
    /// real work, but not integrated, so the close is premature.
    #[test]
    fn cascab3_no_diff_with_unmerged_receipt_still_rejects() {
        let dir = init_worker_repo();
        // Real work on a side branch that is never merged into main.
        git(dir.path(), &["checkout", "-q", "-b", "factory/side-lane"]);
        std::fs::write(dir.path().join("unmerged.rs"), "pub fn unmerged() {}\n").unwrap();
        git(dir.path(), &["add", "unmerged.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "fix: never merged"]);
        let unmerged_receipt = head_sha(dir.path());

        git(dir.path(), &["checkout", "-q", "main"]);
        std::fs::write(dir.path().join("epic_progress.txt"), "epic moved on\n").unwrap();
        git(dir.path(), &["add", "epic_progress.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "unrelated epic progress"]);
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["merge", "--no-ff", "-m", "sync only", "main"]);

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-cab3",
            &TaskType::Bug,
            None,
            false,
            None,
            Some(&unmerged_receipt),
            Some(&test_receipt_window()),
        );
        let ZeroCommitCloseOutcome::AmbiguousCodeTask(msg) = outcome else {
            panic!("an unmerged receipt must not carry a close");
        };
        assert!(
            msg.contains("INVALID TASK COMMIT RECEIPT"),
            "an unmerged receipt must be rejected on its own terms: {msg}"
        );
    }

    /// An unmerged ANCHOR must not silently unlock the no-diff path either —
    /// it falls through to the ordinary refusal (no receipt was offered).
    #[test]
    fn cascab3_no_diff_with_unmerged_anchor_still_rejects() {
        let dir = init_worker_repo();
        git(dir.path(), &["checkout", "-q", "-b", "factory/side-lane"]);
        std::fs::write(dir.path().join("unmerged.rs"), "pub fn unmerged() {}\n").unwrap();
        git(dir.path(), &["add", "unmerged.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "fix: never merged"]);
        let unmerged_anchor = head_sha(dir.path());

        git(dir.path(), &["checkout", "-q", "main"]);
        std::fs::write(dir.path().join("epic_progress.txt"), "epic moved on\n").unwrap();
        git(dir.path(), &["add", "epic_progress.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "unrelated epic progress"]);
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["merge", "--no-ff", "-m", "sync only", "main"]);

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-cab3",
            &TaskType::Bug,
            None,
            false,
            Some(&unmerged_anchor),
            None,
            None,
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::AmbiguousCodeTask(_)),
            "an unmerged anchor must not unlock the no-diff path; got {outcome:?}"
        );
    }

    /// The commit-count>0 WITH a real diff case is untouched: normal
    /// unmerged work still proceeds without needing any evidence.
    #[test]
    fn cascab3_branch_with_real_diff_is_unaffected() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("work.rs"), "pub fn work() {}\n").unwrap();
        git(dir.path(), &["add", "work.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "fix: unmerged work"]);

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-cab3",
            &TaskType::Bug,
            None,
            false,
            None,
            None,
            None,
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::Proceed),
            "a branch with a real diff must proceed without evidence; got {outcome:?}"
        );
    }

    /// cas-26bb: a worker whose commit hook did not capture an anchor can
    /// supply the exact task commit after the supervisor has merged it.
    #[test]
    fn cas26bb_valid_merged_receipt_without_anchor_proceeds() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("receipt.rs"), "pub fn receipt() {}\n").unwrap();
        git(dir.path(), &["add", "receipt.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "fix: receipted work"]);
        let receipt = head_sha(dir.path());

        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge receipted work",
                "factory/test-worker",
            ],
        );
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["reset", "--hard", "main"]);
        assert_eq!(count_worker_branch_commits(dir.path(), "main"), 0);
        let receipt_window = test_receipt_window();

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-26bb",
            &TaskType::Bug,
            None,
            false,
            None,
            Some(&receipt),
            Some(&receipt_window),
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::ProceedWithReceipt(_)),
            "validated task receipt must satisfy merged-before-close; got {outcome:?}"
        );
    }

    #[test]
    fn cas77af_valid_short_receipt_resolves_to_full_commit_and_proceeds() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("short.rs"), "pub fn short_receipt() {}\n").unwrap();
        git(dir.path(), &["add", "short.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "fix: short receipt work"]);
        let full_receipt = head_sha(dir.path());
        let short_receipt = &full_receipt[..8];

        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge short receipt work",
                "factory/test-worker",
            ],
        );
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["reset", "--hard", "main"]);

        let context = crate::mcp::tools::core::task::repo_context::RepoContext {
            repo_selector: "remote:example.invalid/cas77af".to_string(),
            repo_root: dir.path().to_path_buf(),
            git_common_dir: dir.path().join(".git"),
            target_branch: "main".to_string(),
        };
        let task = Task::new("cas-77af".to_string(), "short receipt".to_string());
        let evidence = run_declared_pre_close_hook(
            &task,
            &context,
            Some(dir.path()),
            Some(short_receipt),
        )
        .expect("short receipt must select a valid close-hook scope");
        assert_eq!(
            evidence.task_tip.as_deref(),
            Some(full_receipt.as_str()),
            "durable hook evidence must store the canonical full object ID"
        );

        let note = validate_task_commit_receipt(
            dir.path(),
            short_receipt,
            "main",
            &test_receipt_window(),
        )
        .expect("an unambiguous Git abbreviation must be valid receipt input");
        assert!(note.contains(short_receipt), "{note}");
        assert!(note.contains(&full_receipt), "{note}");
        assert!(
            note.contains("resolved to full commit"),
            "the audit note must preserve normalization evidence: {note}"
        );
    }

    #[test]
    fn cas77af_unmerged_short_receipt_reports_ancestry_not_format_or_merge_required() {
        let dir = init_worker_repo();
        std::fs::write(
            dir.path().join("unmerged-short.rs"),
            "pub fn unmerged_short_receipt() {}\n",
        )
        .unwrap();
        git(dir.path(), &["add", "unmerged-short.rs"]);
        git(
            dir.path(),
            &["commit", "-q", "-m", "fix: unmerged short receipt"],
        );
        let full_receipt = head_sha(dir.path());
        let short_receipt = &full_receipt[..8];

        let reason = validate_task_commit_receipt(
            dir.path(),
            short_receipt,
            "main",
            &test_receipt_window(),
        )
        .expect_err("a real but unmerged commit must remain invalid close evidence");
        assert!(reason.contains("not an ancestor of main"), "{reason}");
        assert!(!reason.contains("40- or 64-character"), "{reason}");

        let message = commit_receipt_rejection(short_receipt, "main", &reason);
        assert!(message.contains("INVALID TASK COMMIT RECEIPT"), "{message}");
        assert!(!message.contains("MERGE REQUIRED"), "{message}");
        assert!(message.contains("not an ancestor of main"), "{message}");
    }

    #[test]
    fn cas77af_receipt_must_name_a_commit_object_not_a_tree_or_tag_object() {
        let dir = init_worker_repo();
        let tree = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD^{tree}"])
                .current_dir(dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        let tree_error = validate_task_commit_receipt(
            dir.path(),
            &tree,
            "main",
            &test_receipt_window(),
        )
        .expect_err("a tree object is not immutable commit evidence");
        assert!(tree_error.contains("tree object"), "{tree_error}");
        assert!(tree_error.contains("not a commit"), "{tree_error}");

        git(dir.path(), &["tag", "-a", "receipt-tag", "-m", "receipt tag"]);
        let tag = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "receipt-tag"])
                .current_dir(dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        let tag_error = validate_task_commit_receipt(
            dir.path(),
            &tag,
            "main",
            &test_receipt_window(),
        )
        .expect_err("an annotated tag object is not immutable commit evidence");
        assert!(tag_error.contains("tag object"), "{tag_error}");
        assert!(tag_error.contains("not a commit"), "{tag_error}");
    }

    /// cas-7308a: conflict resume clears the parked anchor, then the
    /// supervisor resolves and merges out-of-band while the worker branch
    /// has no commits beyond the parent. This fixture exercises the worker
    /// task-commit receipt; cas-5626 separately covers a merge-commit receipt.
    #[test]
    fn cas7308a_conflict_resume_accepts_worker_commit_receipt_after_out_of_band_merge() {
        let dir = init_worker_repo();
        std::fs::write(
            dir.path().join("resolved.rs"),
            "pub fn resolved_conflict() {}\n",
        )
        .unwrap();
        git(dir.path(), &["add", "resolved.rs"]);
        git(
            dir.path(),
            &["commit", "-q", "-m", "fix: worker conflict resolution"],
        );
        let worker_task_receipt = head_sha(dir.path());

        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &[
                "merge",
                "--no-ff",
                "-m",
                "supervisor out-of-band merge",
                "factory/test-worker",
            ],
        );
        let supervisor_merge_commit = head_sha(dir.path());
        assert_ne!(
            worker_task_receipt, supervisor_merge_commit,
            "fixture must distinguish the attributable worker commit from the merge commit"
        );

        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["reset", "--hard", "main"]);
        assert_eq!(count_worker_branch_commits(dir.path(), "main"), 0);
        let receipt_window = test_receipt_window();

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-7308a",
            &TaskType::Bug,
            None,
            false,
            None, // conflict resume cleared the old anchor
            Some(&worker_task_receipt),
            Some(&receipt_window),
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::ProceedWithReceipt(_)),
            "the worker's merged task commit must close the cleared-anchor shape: {outcome:?}"
        );
    }

    /// cas-09f2 live shape: the hook captured C, a pre-push rebase rewrote
    /// it to C', and only C' was merged. The stale stored anchor must not
    /// prevent the worker from presenting the merged post-rebase receipt.
    #[test]
    fn cas26bb_post_rebase_receipt_supersedes_stale_anchor() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("rebased.rs"), "pub fn rebased() {}\n").unwrap();
        git(dir.path(), &["add", "rebased.rs"]);
        git(
            dir.path(),
            &["commit", "-q", "-m", "fix: pre-rebase task work"],
        );
        let stale_anchor = head_sha(dir.path());

        git(dir.path(), &["checkout", "-q", "main"]);
        std::fs::write(dir.path().join("epic.txt"), "new epic work\n").unwrap();
        git(dir.path(), &["add", "epic.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "feat: advance epic"]);

        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["rebase", "main"]);
        let post_rebase_receipt = head_sha(dir.path());
        assert_ne!(
            stale_anchor, post_rebase_receipt,
            "rebase fixture must rewrite the task commit"
        );

        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge post-rebase task work",
                "factory/test-worker",
            ],
        );
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["reset", "--hard", "main"]);

        assert!(
            !commit_is_merged_into_parent(dir.path(), &stale_anchor, "main"),
            "pre-rebase anchor must be stale"
        );
        assert!(
            commit_is_merged_into_parent(dir.path(), &post_rebase_receipt, "main"),
            "post-rebase receipt must be merged"
        );
        let receipt_window = test_receipt_window();

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-26bb",
            &TaskType::Bug,
            None,
            false,
            Some(&stale_anchor),
            Some(&post_rebase_receipt),
            Some(&receipt_window),
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::ProceedWithReceipt(_)),
            "validated post-rebase receipt must supersede a stale anchor; got {outcome:?}"
        );
    }

    #[test]
    fn cas26bb_unknown_receipt_rejects_with_actionable_reason() {
        let dir = init_worker_repo();
        let unknown = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let receipt_window = test_receipt_window();
        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-26bb",
            &TaskType::Bug,
            None,
            false,
            None,
            Some(unknown),
            Some(&receipt_window),
        );
        match outcome {
            ZeroCommitCloseOutcome::AmbiguousCodeTask(msg) => {
                assert!(msg.contains("INVALID TASK COMMIT RECEIPT"), "{msg}");
                assert!(msg.contains("does not uniquely resolve to a commit"), "{msg}");
                assert!(msg.contains("commit_receipt=<sha>"), "{msg}");
            }
            ZeroCommitCloseOutcome::Proceed => panic!("unknown receipt must not proceed"),
            ZeroCommitCloseOutcome::ProceedWithReceipt(_) => {
                panic!("unknown receipt must not proceed")
            }
        }
    }

    #[test]
    fn cas26bb_empty_diff_receipt_rejects_with_actionable_reason() {
        let dir = init_worker_repo();
        git(
            dir.path(),
            &["commit", "--allow-empty", "-q", "-m", "empty task receipt"],
        );
        let receipt = head_sha(dir.path());
        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge empty receipt",
                "factory/test-worker",
            ],
        );
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["reset", "--hard", "main"]);
        let receipt_window = test_receipt_window();

        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-26bb",
            &TaskType::Feature,
            None,
            false,
            None,
            Some(&receipt),
            Some(&receipt_window),
        );
        match outcome {
            ZeroCommitCloseOutcome::AmbiguousCodeTask(msg) => {
                assert!(msg.contains("INVALID TASK COMMIT RECEIPT"), "{msg}");
                assert!(msg.contains("empty file diff"), "{msg}");
            }
            ZeroCommitCloseOutcome::Proceed => panic!("empty receipt must not proceed"),
            ZeroCommitCloseOutcome::ProceedWithReceipt(_) => {
                panic!("empty receipt must not proceed")
            }
        }
    }

    #[test]
    fn cas26bb_valid_receipt_also_satisfies_commit_claim_gate() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("reviewed.rs"), "pub fn reviewed() {}\n").unwrap();
        git(dir.path(), &["add", "reviewed.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "fix: reviewed work"]);
        let receipt = head_sha(dir.path());
        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge reviewed work",
                "factory/test-worker",
            ],
        );
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["reset", "--hard", "main"]);
        let receipt_window = test_receipt_window();

        let outcome = check_commit_claim_integrity(
            dir.path(),
            "main",
            true,
            None,
            Some(&receipt),
            Some(&receipt_window),
        );
        assert!(
            matches!(outcome, CommitClaimGateOutcome::ProceedWithReceipt(_)),
            "a validated receipt must also prevent a false fabrication rejection"
        );
    }

    #[test]
    fn cas5626_historical_receipt_is_rejected_by_both_close_gates() {
        let dir = init_worker_repo();
        let historical = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "main"])
                .current_dir(dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        let window = TaskCommitReceiptWindow {
            // Put the task cycle definitively after the fixture commit. This
            // reproduces copying an arbitrary old merged SHA from git log.
            not_before: chrono::Utc::now() + chrono::Duration::hours(1),
            basis: "latest task lease claim/transfer",
            // cas-9596: the task itself is younger than the borrowed commit, so
            // the prior-cycle relaxation cannot rescue it either.
            task_floor: chrono::Utc::now() + chrono::Duration::hours(1),
            identity: TaskCommitIdentity::default(),
        };

        let zero_outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-5626",
            &TaskType::Bug,
            None,
            false,
            None,
            Some(&historical),
            Some(&window),
        );
        match zero_outcome {
            ZeroCommitCloseOutcome::AmbiguousCodeTask(message) => {
                assert!(
                    message.contains("predates this task work cycle"),
                    "{message}"
                );
                assert!(message.contains("ask the supervisor"), "{message}");
            }
            other => panic!("historical receipt must fail zero-commit gate: {other:?}"),
        }

        let claim_outcome = check_commit_claim_integrity(
            dir.path(),
            "main",
            true,
            None,
            Some(&historical),
            Some(&window),
        );
        match claim_outcome {
            CommitClaimGateOutcome::Reject(message) => {
                assert!(
                    message.contains("predates this task work cycle"),
                    "{message}"
                );
                assert!(message.contains("INVALID TASK COMMIT RECEIPT"), "{message}");
            }
            other => panic!("historical receipt must fail fabrication gate: {other:?}"),
        }
    }

    /// GH #82 step 6: an administrative restart (supervisor clears a note, the
    /// worker re-`start`s) moves the work-cycle window forward without any new
    /// commits. The receipt from before that restart is still this task's own
    /// merged work and must remain valid close evidence.
    #[test]
    fn gh82_receipt_from_a_prior_work_cycle_survives_an_administrative_restart() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("finished.rs"), "pub fn finished() {}\n").unwrap();
        git(dir.path(), &["add", "finished.rs"]);
        git(
            dir.path(),
            &["commit", "-q", "-m", "feat(cas-f1b1): finished delivery"],
        );
        let receipt = head_sha(dir.path());
        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge finished delivery",
                "factory/test-worker",
            ],
        );

        // The restart put the current cycle after the commit; the task itself
        // is older than it.
        let window = TaskCommitReceiptWindow {
            not_before: chrono::Utc::now() + chrono::Duration::hours(1),
            basis: "latest task lease claim/transfer",
            task_floor: chrono::Utc::now() - chrono::Duration::hours(2),
            identity: TaskCommitIdentity {
                task_id: Some("cas-f1b1".to_string()),
                known_commits: Vec::new(),
            },
        };

        let note = validate_task_commit_receipt(dir.path(), &receipt, "main", &window)
            .expect("an already-merged receipt for this task must survive a restart");
        assert!(note.contains(&receipt), "{note}");
        assert!(
            note.contains("earlier work cycle of this task"),
            "the audit note must record why the cycle bound was relaxed: {note}"
        );
    }

    /// The relaxation is bounded by attribution: a merged commit that neither
    /// names the task nor is a recorded task commit stays rejected.
    #[test]
    fn gh82_unattributable_pre_cycle_commit_is_still_rejected() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("foreign.rs"), "pub fn foreign() {}\n").unwrap();
        git(dir.path(), &["add", "foreign.rs"]);
        git(dir.path(), &["commit", "-q", "-m", "chore: someone else"]);
        let receipt = head_sha(dir.path());
        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge foreign work",
                "factory/test-worker",
            ],
        );

        let window = TaskCommitReceiptWindow {
            not_before: chrono::Utc::now() + chrono::Duration::hours(1),
            basis: "latest task lease claim/transfer",
            task_floor: chrono::Utc::now() - chrono::Duration::hours(2),
            identity: TaskCommitIdentity {
                task_id: Some("cas-f1b1".to_string()),
                known_commits: Vec::new(),
            },
        };
        let reason = validate_task_commit_receipt(dir.path(), &receipt, "main", &window)
            .expect_err("an unattributable pre-cycle commit must stay rejected");
        assert!(reason.contains("predates this task work cycle"), "{reason}");
    }

    /// A commit that predates the task itself is never this task's work, even
    /// when it happens to name the task id.
    #[test]
    fn gh82_commit_predating_the_task_is_still_rejected() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("old.rs"), "pub fn old() {}\n").unwrap();
        git(dir.path(), &["add", "old.rs"]);
        git(
            dir.path(),
            &["commit", "-q", "-m", "feat(cas-f1b1): borrowed reference"],
        );
        let receipt = head_sha(dir.path());
        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &["merge", "--no-ff", "-m", "merge old", "factory/test-worker"],
        );

        let window = TaskCommitReceiptWindow {
            not_before: chrono::Utc::now() + chrono::Duration::hours(2),
            basis: "latest task lease claim/transfer",
            task_floor: chrono::Utc::now() + chrono::Duration::hours(1),
            identity: TaskCommitIdentity {
                task_id: Some("cas-f1b1".to_string()),
                known_commits: Vec::new(),
            },
        };
        let reason = validate_task_commit_receipt(dir.path(), &receipt, "main", &window)
            .expect_err("a commit older than the task cannot be its delivery");
        assert!(reason.contains("predates this task work cycle"), "{reason}");
    }

    #[test]
    fn cas5626_merge_commit_receipt_is_valid_and_auditable() {
        let dir = init_worker_repo();
        std::fs::write(dir.path().join("merged.rs"), "pub fn merged() {}\n").unwrap();
        git(dir.path(), &["add", "merged.rs"]);
        git(
            dir.path(),
            &["commit", "-q", "-m", "fix: merge receipt work"],
        );

        git(dir.path(), &["checkout", "-q", "main"]);
        git(
            dir.path(),
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge task branch",
                "factory/test-worker",
            ],
        );
        let merge_receipt = head_sha(dir.path());
        git(dir.path(), &["checkout", "-q", "factory/test-worker"]);
        git(dir.path(), &["reset", "--hard", "main"]);

        let window = TaskCommitReceiptWindow {
            not_before: chrono::Utc::now() - chrono::Duration::hours(1),
            basis: "latest task lease claim/transfer",
            task_floor: chrono::Utc::now() - chrono::Duration::hours(2),
            identity: TaskCommitIdentity::default(),
        };
        let outcome = check_zero_commit_close(
            dir.path(),
            "main",
            "cas-5626",
            &TaskType::Bug,
            None,
            false,
            None,
            Some(&merge_receipt),
            Some(&window),
        );
        match outcome {
            ZeroCommitCloseOutcome::ProceedWithReceipt(note) => {
                assert!(note.contains("decision: accepted commit_receipt"), "{note}");
                assert!(note.contains(&merge_receipt), "{note}");
                assert!(note.contains("latest task lease claim/transfer"), "{note}");
                assert!(
                    note.contains("merge-aware file diff is non-empty"),
                    "{note}"
                );
            }
            other => panic!("legitimate merge receipt must validate: {other:?}"),
        }
    }

    #[test]
    fn cas5626_accepted_receipt_note_is_persisted_once() {
        let mut task = Task::new("cas-5626".to_string(), "receipt audit".to_string());
        let store = crate::store::mock::MockTaskStore::with_tasks(vec![task.clone()]);
        let note = "decision: accepted commit_receipt `abc` using latest task lease claim/transfer";

        append_close_decision_note(&store, &mut task, note);
        append_close_decision_note(&store, &mut task, note);

        let persisted = cas_store::TaskStore::get(&store, "cas-5626").unwrap();
        assert!(persisted.notes.contains(note), "{}", persisted.notes);
        assert_eq!(
            persisted.notes.matches(note).count(),
            1,
            "accepted receipt decision note must be idempotent"
        );
    }

    #[test]
    fn commit_is_merged_into_parent_false_for_unknown_sha() {
        let dir = init_worker_repo();
        assert!(!commit_is_merged_into_parent(
            dir.path(),
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            "main"
        ));
    }

    // ── cas-7efe: ZERO-COMMIT catch-22 on a non-`main`-based epic ──────────

    /// Regression test for BUG-zero-commit-close-gate-catch22.md.
    /// Reproduces the exact bug shape: an epic branched from a non-`main`
    /// base (here `staging`; this repo has no `main` branch at all — the
    /// old code's fallback would have nothing sane to land on), a
    /// worker's factory branch merged into the epic, and the worker
    /// branch subsequently synced to the epic tip (0 commits ahead — the
    /// ambiguous shape).
    ///
    /// Before cas-7efe, 4 of the 5 close-time gates resolved the parent
    /// branch via `task.worktree_id -> worktree_store.get(..).parent_branch`
    /// with `.unwrap_or_else(|| "main".to_string())`. Since
    /// `task.worktree_id` is unset for the common System-B factory path
    /// (`spawn_workers isolate=true`), they always fell straight through
    /// to that "main" literal, ignoring the real epic branch — so
    /// `check_zero_commit_close`'s cas-127f merge-satisfied path called
    /// `commit_is_merged_into_parent(anchor, "main")`, which is false
    /// (the anchor was merged into `epic/foo`, not `main`), producing an
    /// ambiguous ZERO-COMMIT rejection immediately after the supervisor
    /// did exactly what the prior MERGE REQUIRED rejection demanded.
    #[test]
    fn cas7efe_catch22_resolves_epic_not_main_and_proceeds() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "staging"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);

        // Epic branch, based on `staging` — never `main`.
        git(p, &["checkout", "-q", "-b", "epic/foo"]);

        // Worker branch off the epic.
        git(p, &["checkout", "-q", "-b", "factory/worker"]);
        std::fs::write(p.join("fix.rs"), "pub fn work() {}\n").unwrap();
        git(p, &["add", "fix.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task work"]);
        let anchor = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(p)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Supervisor merges the worker branch into the epic (what MERGE
        // REQUIRED demanded), then the worker's factory branch is synced
        // to the epic tip — the exact post-merge state that produced the
        // catch-22 (0 commits ahead of parent).
        git(p, &["checkout", "-q", "epic/foo"]);
        git(
            p,
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge factory/worker",
                "factory/worker",
            ],
        );
        git(p, &["checkout", "-q", "factory/worker"]);
        git(p, &["reset", "--hard", "epic/foo"]);

        assert_eq!(
            count_worker_branch_commits(p, "epic/foo"),
            0,
            "sanity: post-merge, worker branch must show 0 commits ahead \
             of the epic — the ambiguous shape"
        );
        assert!(
            !commit_is_merged_into_parent(p, &anchor, "main"),
            "sanity: this repo has no relevant 'main' branch — proves the \
             old hardcoded fallback would misfire if it were still used"
        );

        // The fix: resolve_close_parent_branch must select the epic
        // branch, never guess "main", when the worktree store has
        // nothing recorded (the common System-B factory-isolation shape).
        let resolved = resolve_close_parent_branch(None, Some("epic/foo".to_string()), p)
            .expect("explicit epic branch resolves");
        assert_eq!(
            resolved, "epic/foo",
            "must resolve the real epic branch, never a bare 'main'"
        );
        assert!(
            commit_is_merged_into_parent(p, &anchor, &resolved),
            "parked anchor must be recognized as merged into the \
             correctly-resolved epic branch"
        );

        // End to end: feeding the CORRECTLY resolved branch into the
        // zero-commit gate must Proceed — no ZERO-COMMIT rejection after
        // the supervisor did exactly what MERGE REQUIRED demanded. This
        // is the assertion that fails before the cas-7efe fix (when
        // exercised through the real close_ops.rs call sites, which used
        // to feed a bare "main" here instead of `resolved`).
        let outcome = check_zero_commit_close(
            p,
            &resolved,
            "cas-7efe",
            &TaskType::Bug,
            None,
            false,
            Some(&anchor),
            None,
            None,
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::Proceed),
            "post-merge close on a non-main-based epic must Proceed, not \
             ZERO-COMMIT; got {outcome:?}"
        );
    }

    /// Companion assertion: had the pre-fix code's "main" fallback still
    /// been wired into `check_zero_commit_close` for this same fixture,
    /// the close would have been wrongly rejected as ambiguous — pinning
    /// down exactly what the fix prevents.
    #[test]
    fn cas7efe_old_main_fallback_would_have_falsely_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "staging"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        git(p, &["checkout", "-q", "-b", "epic/foo"]);
        git(p, &["checkout", "-q", "-b", "factory/worker"]);
        std::fs::write(p.join("fix.rs"), "pub fn work() {}\n").unwrap();
        git(p, &["add", "fix.rs"]);
        git(p, &["commit", "-q", "-m", "feat: task work"]);
        let anchor = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(p)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        git(p, &["checkout", "-q", "epic/foo"]);
        git(
            p,
            &[
                "merge",
                "--no-ff",
                "-m",
                "merge factory/worker",
                "factory/worker",
            ],
        );
        git(p, &["checkout", "-q", "factory/worker"]);
        git(p, &["reset", "--hard", "epic/foo"]);

        // Simulating the OLD (pre-cas-7efe) resolution: no worktree_id,
        // so the old code's `.unwrap_or_else(|| "main".to_string())`
        // fired directly — this is that literal value, fed to the same
        // gate the real call site used.
        let old_hardcoded_fallback = "main";
        let outcome = check_zero_commit_close(
            p,
            old_hardcoded_fallback,
            "cas-7efe",
            &TaskType::Bug,
            None,
            false,
            Some(&anchor),
            None,
            None,
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::AmbiguousCodeTask(_)),
            "documents the bug: the old hardcoded 'main' fallback falsely \
             rejects this exact post-merge state as ZERO-COMMIT; got {outcome:?}"
        );
    }
}

#[cfg(test)]
mod merge_reality_tests {
    //! cas-762e / B2: regression tests for the factory branch merge reality gate.
    //!
    //! Root cause: `run_factory_branch_merge_gate` (cas-95ce) returns PROCEED
    //! whenever `count_unmerged_factory_commits == 0`. Zero commits is
    //! ambiguous: it can mean "merged via PR" (correct) or "the worker never
    //! committed to factory/<name> at all and put their work somewhere else"
    //! (the bug cas-073f tracks).
    //!
    //! `check_factory_branch_merge_reality` disambiguates the 0-commit case:
    //! if the factory branch exists locally, carries 0 commits beyond the
    //! parent branch, AND was never pushed to origin (no remote tracking ref),
    //! the close is refused.
    //!
    //! Tests cover:
    //! - AC1: branch exists, 0 commits, no remote → REFUSE
    //! - AC3a: branch has ≥1 unmerged commit → PROCEED (cas-95ce already
    //!         handles the stranded case; B2 must not double-reject)
    //! - AC3b: branch exists, 0 commits, remote tracking ref present → PROCEED
    //! - AC4: factory branch absent locally (push+merge+prune) → PROCEED
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn git_output(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Create a minimal git repo on `main` with a seed commit.
    /// Returns a repo where `factory/test-worker` was created but has
    /// **0 commits** beyond `main` (the branch was just checked out;
    /// no work committed to it).
    fn init_repo_worker_branch_empty() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        git(p, &["checkout", "-q", "-b", "factory/test-worker"]);
        // switch back to main so factory/test-worker has 0 commits beyond main
        git(p, &["checkout", "-q", "main"]);
        dir
    }

    /// Same as above but also adds one commit ON `factory/test-worker`.
    fn init_repo_worker_branch_with_commit() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        git(p, &["checkout", "-q", "-b", "factory/test-worker"]);
        std::fs::write(p.join("work.rs"), "fn work() {}\n").unwrap();
        git(p, &["add", "work.rs"]);
        git(p, &["commit", "-q", "-m", "cas-762e work"]);
        dir
    }

    // -------------------------------------------------------------------------
    // AC1: branch exists, 0 commits beyond parent, no remote → REFUSE
    // -------------------------------------------------------------------------

    /// The core bug scenario: worker ran in an isolated worktree but
    /// committed to the wrong place. `factory/test-worker` exists with
    /// 0 commits beyond `main` and was never pushed. Close must be refused.
    #[test]
    fn ac1_empty_branch_no_remote_is_refused() {
        let dir = init_repo_worker_branch_empty();
        let outcome = check_factory_branch_merge_reality(dir.path(), "test-worker", "main");
        assert!(
            matches!(outcome, MergeRealityOutcome::Refuse(_)),
            "factory branch exists with 0 commits and no remote push — must REFUSE"
        );
    }

    /// Verify the refusal message names the factory branch and parent branch.
    #[test]
    fn ac1_refusal_message_names_branches() {
        let dir = init_repo_worker_branch_empty();
        let outcome = check_factory_branch_merge_reality(dir.path(), "test-worker", "main");
        if let MergeRealityOutcome::Refuse(msg) = outcome {
            assert!(
                msg.contains("factory/test-worker"),
                "refusal message must name the factory branch; got: {msg}"
            );
            assert!(
                msg.contains("main"),
                "refusal message must name the parent branch; got: {msg}"
            );
        } else {
            panic!("expected Refuse, got Proceed");
        }
    }

    // -------------------------------------------------------------------------
    // AC3a: branch has ≥1 unmerged commit → PROCEED
    // (cas-95ce already guards this; B2 must not double-reject)
    // -------------------------------------------------------------------------

    #[test]
    fn ac3a_branch_with_unmerged_commit_proceeds() {
        let dir = init_repo_worker_branch_with_commit();
        let outcome = check_factory_branch_merge_reality(dir.path(), "test-worker", "main");
        assert!(
            matches!(outcome, MergeRealityOutcome::Proceed),
            "factory branch with ≥1 unmerged commit must PROCEED from B2 \
             (cas-95ce owns the stranded-commit rejection)"
        );
    }

    // -------------------------------------------------------------------------
    // AC3b: branch exists, 0 commits, remote tracking ref present → PROCEED
    // (push+merge path; origin/factory/<name> proves a PR existed)
    // -------------------------------------------------------------------------

    /// Simulate "was pushed to origin" by creating the remote tracking ref
    /// directly via `git update-ref`. This avoids needing a real remote.
    #[test]
    fn ac3b_remote_ref_present_proceeds() {
        let dir = init_repo_worker_branch_empty();
        // Manually create origin/factory/test-worker pointing to HEAD of main
        let head_sha = git_output(dir.path(), &["rev-parse", "main"]);
        git(
            dir.path(),
            &[
                "update-ref",
                "refs/remotes/origin/factory/test-worker",
                &head_sha,
            ],
        );
        let outcome = check_factory_branch_merge_reality(dir.path(), "test-worker", "main");
        assert!(
            matches!(outcome, MergeRealityOutcome::Proceed),
            "when remote tracking ref exists, branch was pushed — must PROCEED"
        );
    }

    // -------------------------------------------------------------------------
    // AC4: factory branch absent locally → PROCEED (push+merge+prune path)
    // -------------------------------------------------------------------------

    /// Worker pushed, PR was merged, and both local + remote refs were pruned.
    /// `factory/test-worker` does not exist. The gate must not reject the close.
    #[test]
    fn ac4_branch_absent_locally_proceeds() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        std::fs::write(p.join("seed.txt"), "seed\n").unwrap();
        git(p, &["add", "seed.txt"]);
        git(p, &["commit", "-q", "-m", "seed"]);
        // No factory/test-worker branch at all
        let outcome = check_factory_branch_merge_reality(p, "test-worker", "main");
        assert!(
            matches!(outcome, MergeRealityOutcome::Proceed),
            "absent factory branch (push+merge+prune) must PROCEED"
        );
    }
}

#[cfg(test)]
mod epic_close_owner_gate_tests {
    use super::epic_close_owner_gate;

    #[test]
    fn test_9fff_owner_match_by_id_or_name_allows_close() {
        assert!(
            epic_close_owner_gate("cas-epic", "owner-id", Some("owner-id"), None, None).is_ok()
        );
        assert!(
            epic_close_owner_gate("cas-epic", "owner-sup", None, Some("owner-sup"), None).is_ok()
        );
    }

    #[test]
    fn test_9fff_wrong_identity_rejects_close() {
        let err = epic_close_owner_gate(
            "cas-epic",
            "owner-id",
            Some("other-id"),
            Some("other-sup"),
            None,
        )
        .unwrap_err();
        assert!(
            err.contains("cannot close") && err.contains("owner-id"),
            "wrong identity must reject: {err}"
        );
    }

    /// Review P1: unknown caller must fail closed — never fall through when
    /// epic_verification_owner is set.
    #[test]
    fn test_9fff_unknown_caller_identity_fail_closed() {
        let err = epic_close_owner_gate("cas-epic", "owner-id", None, None, None).unwrap_err();
        assert!(
            err.contains("identity is unknown") && err.contains("fail closed"),
            "unknown identity must fail closed, got: {err}"
        );
    }

    /// cas-cc74: close compare trims owner + identity facets.
    #[test]
    fn test_cc74_close_owner_gate_trims_whitespace() {
        assert!(
            epic_close_owner_gate("cas-epic", "  owner-id  ", Some("owner-id"), None, None).is_ok()
        );
        assert!(
            epic_close_owner_gate("cas-epic", "owner-id", Some("  owner-id  "), None, None).is_ok()
        );
    }
}

#[cfg(test)]
mod zero_diff_spike_close_tests {
    //! cas-1932 (GH #62 symptoms 1-2 + minor): a zero-diff spike closed in a
    //! dirty shared checkout was a two-stage trap.
    //!
    //! - Symptom 1: after the supervisor recorded an APPROVED verification,
    //!   the worker's re-close re-queued to `PendingSupervisorReview` forever.
    //!   The review-queue hop now consumes a current-cycle approved verdict.
    //! - Symptom 2: `CODE_REVIEW_REQUIRED` fired because reviewable-change
    //!   detection read the shared checkout's pre-existing WIP as the task's
    //!   diff. Detection is now scoped to commits attributable to this task's
    //!   work cycle for tasks whose own spec declares no-code work.
    //! - Minor: close reported "verification skipped — assignee unknown"
    //!   although a verification row existed; the lookup now finds it.
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &std::path::Path, args: &[&str], date: &str) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    const PRIOR_CYCLE: &str = "2020-01-01T00:00:00Z";
    const THIS_CYCLE: &str = "2026-08-04T12:00:00Z";
    /// Between PRIOR_CYCLE and THIS_CYCLE (2023-11-14T22:13:20Z).
    const CYCLE_START_EPOCH: i64 = 1_700_000_000;

    fn window() -> TaskCommitReceiptWindow {
        let cycle_start = chrono::DateTime::from_timestamp(CYCLE_START_EPOCH, 0).unwrap();
        TaskCommitReceiptWindow {
            not_before: cycle_start,
            basis: "latest task lease claim/transfer",
            // cas-9596: these tests exercise cycle-scoped attribution only —
            // the task floor sits at the cycle start and no durable task
            // commit identity is recorded.
            task_floor: cycle_start,
            identity: TaskCommitIdentity::default(),
        }
    }

    /// Shared main checkout on `main` with one old commit, then N dirty
    /// (uncommitted) reviewable files — the pre-existing prior-factory WIP
    /// from the incident.
    fn init_shared_checkout_with_dirty_wip() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"], PRIOR_CYCLE);
        std::fs::write(p.join("seed.rs"), "// seed\n").unwrap();
        git(p, &["add", "seed.rs"], PRIOR_CYCLE);
        git(p, &["commit", "-q", "-m", "seed"], PRIOR_CYCLE);
        // Prior-factory WIP left dirty in the shared checkout: an uncommitted
        // edit to a tracked source file, which is what `has_reviewable_changes`
        // sees and what the incident's ~64 dirty files looked like.
        std::fs::write(p.join("seed.rs"), "// seed\n// someone else's WIP\n").unwrap();
        dir
    }

    // --- symptom 2: task-attributable reviewable detection -------------------

    #[test]
    fn dirty_shared_checkout_with_no_task_commits_is_not_task_attributable() {
        let dir = init_shared_checkout_with_dirty_wip();
        assert_eq!(
            has_task_attributable_reviewable_changes(dir.path(), "main", &window()),
            Some(false),
            "pre-existing dirty WIP in a shared checkout is not this task's diff"
        );
        // The unscoped check is what used to drive CODE_REVIEW_REQUIRED.
        assert!(
            has_reviewable_changes(dir.path()),
            "precondition: the unscoped checkout check still sees the dirty WIP"
        );
    }

    #[test]
    fn commit_made_during_this_work_cycle_is_task_attributable() {
        let dir = init_shared_checkout_with_dirty_wip();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "work"], THIS_CYCLE);
        std::fs::write(p.join("feature.rs"), "pub fn f() {}\n").unwrap();
        git(p, &["add", "feature.rs"], THIS_CYCLE);
        git(p, &["commit", "-q", "-m", "feat: f"], THIS_CYCLE);
        assert_eq!(
            has_task_attributable_reviewable_changes(p, "main", &window()),
            Some(true),
            "a reviewable commit made inside the work cycle IS the task's diff"
        );
    }

    #[test]
    fn commits_predating_the_work_cycle_are_not_task_attributable() {
        let dir = init_shared_checkout_with_dirty_wip();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "work"], PRIOR_CYCLE);
        std::fs::write(p.join("old_feature.rs"), "pub fn old() {}\n").unwrap();
        git(p, &["add", "old_feature.rs"], PRIOR_CYCLE);
        git(p, &["commit", "-q", "-m", "feat: old"], PRIOR_CYCLE);
        assert_eq!(
            has_task_attributable_reviewable_changes(p, "main", &window()),
            Some(false),
            "another task's earlier commits must not be attributed to this close"
        );
    }

    #[test]
    fn docs_only_commit_in_this_cycle_is_not_reviewable() {
        let dir = init_shared_checkout_with_dirty_wip();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "work"], THIS_CYCLE);
        std::fs::write(p.join("NOTES.md"), "# notes\n").unwrap();
        git(p, &["add", "NOTES.md"], THIS_CYCLE);
        git(p, &["commit", "-q", "-m", "docs: notes"], THIS_CYCLE);
        assert_eq!(
            has_task_attributable_reviewable_changes(p, "main", &window()),
            Some(false),
            "docs-only work is not reviewable code"
        );
    }

    #[test]
    fn attributable_detection_is_unknown_outside_a_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            has_task_attributable_reviewable_changes(dir.path(), "main", &window()),
            None,
            "unknowable git state must not be reported as 'nothing attributable'"
        );
    }

    // --- symptom 2: the shared-checkout routing decision ---------------------

    fn scope(
        task_type: TaskType,
        execution_note: Option<&'static str>,
        attributable: Option<bool>,
        dirty: bool,
    ) -> bool {
        shared_checkout_has_reviewable_changes(SharedCheckoutReviewScope {
            task_type,
            execution_note,
            attributable_reviewable_changes: attributable,
            checkout_has_reviewable_changes: dirty,
        })
    }

    #[test]
    fn zero_commit_spike_in_dirty_shared_checkout_has_no_reviewable_changes() {
        // The GH #62 incident shape: characterization-only spike, no commits,
        // shared main checkout carrying ~64 files of prior-factory WIP.
        assert!(
            !scope(TaskType::Spike, None, Some(false), true),
            "a zero-commit spike must not inherit the checkout's dirty state"
        );
        assert!(
            !scope(TaskType::Chore, None, Some(false), true),
            "chores declare no-code work the same way spikes do"
        );
        assert!(
            !scope(TaskType::Task, Some("characterization-first"), Some(false), true),
            "an execution_note is the task's own declaration that no code is expected"
        );
    }

    #[test]
    fn code_task_without_a_no_code_declaration_keeps_the_checkout_signal() {
        // Unchanged behavior: a Bug/Feature/Task with no execution_note still
        // routes on the checkout diff, so nothing silently escapes review.
        assert!(
            scope(TaskType::Bug, None, Some(false), true),
            "a code task with no no-code declaration must keep the existing signal"
        );
        assert!(
            !scope(TaskType::Bug, None, Some(false), false),
            "clean checkout stays clean"
        );
    }

    #[test]
    fn task_attributable_code_always_counts_as_reviewable() {
        assert!(
            scope(TaskType::Spike, Some("characterization-first"), Some(true), false),
            "code this task actually committed is reviewable no matter its declared shape"
        );
    }

    #[test]
    fn unknowable_attribution_falls_back_to_the_checkout_signal() {
        assert!(
            scope(TaskType::Spike, None, None, true),
            "if git state is unknowable the gate must fail closed on the old signal"
        );
    }

    // --- symptom 1: approved verification satisfies the review queue ---------

    fn approved_row(created_epoch: i64) -> Verification {
        let mut row = Verification::new("ver-fd59de6ef422".to_string(), "cas-208b".to_string());
        row.status = VerificationStatus::Approved;
        row.verification_type = VerificationType::Task;
        row.created_at = chrono::DateTime::from_timestamp(created_epoch, 0).unwrap();
        row
    }

    #[test]
    fn approved_verdict_from_this_cycle_satisfies_the_review_queue() {
        let row = approved_row(CYCLE_START_EPOCH + 600);
        assert!(
            approved_verification_satisfies_review_queue(
                &row,
                Some(&window()),
                VerificationType::Task
            ),
            "the supervisor's approval must let the worker's re-close complete"
        );
    }

    #[test]
    fn unapproved_or_stale_or_mistyped_verdicts_do_not_satisfy_the_queue() {
        let mut rejected = approved_row(CYCLE_START_EPOCH + 600);
        rejected.status = VerificationStatus::Rejected;
        assert!(
            !approved_verification_satisfies_review_queue(
                &rejected,
                Some(&window()),
                VerificationType::Task
            ),
            "a rejected verdict must never satisfy the review queue"
        );

        let stale = approved_row(CYCLE_START_EPOCH - 86_400);
        assert!(
            !approved_verification_satisfies_review_queue(
                &stale,
                Some(&window()),
                VerificationType::Task
            ),
            "an approval from a previous work cycle cannot authorize this close"
        );

        let mistyped = approved_row(CYCLE_START_EPOCH + 600);
        assert!(
            !approved_verification_satisfies_review_queue(
                &mistyped,
                Some(&window()),
                VerificationType::Epic
            ),
            "a task verdict cannot stand in for the required epic verdict"
        );
    }

    #[test]
    fn approval_within_clock_skew_of_the_cycle_start_is_accepted() {
        let row = approved_row(CYCLE_START_EPOCH - 1);
        assert!(
            approved_verification_satisfies_review_queue(
                &row,
                Some(&window()),
                VerificationType::Task
            ),
            "a verdict recorded a second before the lease timestamp is the same cycle"
        );
    }

    // --- minor: close must find an existing verification --------------------

    #[test]
    fn existing_approved_verification_replaces_a_lookup_failure_skip_reason() {
        let row = approved_row(CYCLE_START_EPOCH + 600);
        let resolved = skip_reason_with_existing_verification(
            VerificationSkipReason::AssigneeUnknown,
            Some(&row),
        );
        assert_eq!(
            resolved,
            VerificationSkipReason::ExistingApprovedVerification {
                verification_id: "ver-fd59de6ef422".to_string()
            },
            "an existing approved verdict must be cited instead of an assignee-lookup failure"
        );
        let suffix = resolved.response_suffix(true);
        assert!(
            suffix.contains("ver-fd59de6ef422"),
            "the close response must name the verification it found: {suffix}"
        );
        assert!(
            !suffix.contains("assignee unknown"),
            "the close response must stop claiming the verification was skipped: {suffix}"
        );
        assert!(
            resolved.audit_reason().contains("ver-fd59de6ef422"),
            "the audit row must record which verdict authorized the close"
        );
    }

    #[test]
    fn skip_reason_is_untouched_without_an_approved_verification() {
        assert_eq!(
            skip_reason_with_existing_verification(VerificationSkipReason::AssigneeUnknown, None),
            VerificationSkipReason::AssigneeUnknown,
            "with no verdict on record the real skip reason must survive"
        );
        assert_eq!(
            skip_reason_with_existing_verification(
                VerificationSkipReason::SupervisorBypass,
                Some(&approved_row(CYCLE_START_EPOCH + 600)),
            ),
            VerificationSkipReason::SupervisorBypass,
            "an explicit supervisor bypass is intent, not a lookup failure — keep it"
        );
        assert_eq!(
            skip_reason_with_existing_verification(VerificationSkipReason::None, None),
            VerificationSkipReason::None,
            "the non-skip path is unaffected"
        );
    }
}
