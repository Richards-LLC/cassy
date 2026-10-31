use crate::mcp::tools::core::imports::*;

/// Structured task label marking an Open task that was reopened by a rejected
/// supervisor review and therefore needs a supervisor recovery decision.
pub const VERIFICATION_REJECTED_REOPEN_LABEL: &str = "verification-rejected-reopen";

const MAX_VERIFICATION_ISSUES_BYTES: usize = 512 * 1024;
const VERIFICATION_ISSUE_FIELDS: &str = "each issue requires string fields `file`, `category`, and `problem` (`description` or `message` may replace `problem`); optional fields are `line`, `severity`, `code`, and `suggestion` (`fix` may replace `suggestion`)";

fn invalid_verification_issues(reason: impl std::fmt::Display) -> String {
    format!("Invalid verification issues: {reason}; input omitted; {VERIFICATION_ISSUE_FIELDS}.")
}

fn parse_verification_issues(issues_json: &str) -> Result<Vec<VerificationIssue>, String> {
    if issues_json.len() > MAX_VERIFICATION_ISSUES_BYTES {
        return Err(invalid_verification_issues("JSON exceeds 512 KiB"));
    }

    let mut value: serde_json::Value = serde_json::from_str(issues_json)
        .map_err(|_| invalid_verification_issues("expected valid JSON array of issue objects"))?;
    let Some(items) = value.as_array_mut() else {
        return Err(invalid_verification_issues(
            "expected a JSON array of issue objects",
        ));
    };

    for (index, item) in items.iter_mut().enumerate() {
        let Some(object) = item.as_object_mut() else {
            return Err(invalid_verification_issues(format!(
                "item {index} must be an object"
            )));
        };

        if !object.contains_key("problem") {
            for alias in ["description", "message"] {
                if let Some(value) = object.get(alias).cloned() {
                    if !value.is_string() {
                        return Err(invalid_verification_issues(format!(
                            "item {index} field `{alias}` must be a string"
                        )));
                    }
                    object.insert("problem".to_string(), value);
                    break;
                }
            }
        }
        if !object.contains_key("suggestion") {
            if let Some(value) = object.get("fix").cloned() {
                if !value.is_string() && !value.is_null() {
                    return Err(invalid_verification_issues(format!(
                        "item {index} field `fix` must be a string or null"
                    )));
                }
                object.insert("suggestion".to_string(), value);
            }
        }
        if object.get("severity").and_then(serde_json::Value::as_str) == Some("high") {
            object.insert(
                "severity".to_string(),
                serde_json::Value::String("blocking".to_string()),
            );
        }

        let mut missing = Vec::new();
        for field in ["file", "category", "problem"] {
            match object.get(field) {
                Some(serde_json::Value::String(_)) => {}
                Some(_) => {
                    return Err(invalid_verification_issues(format!(
                        "item {index} field `{field}` must be a string"
                    )));
                }
                None => missing.push(field),
            }
        }
        if !missing.is_empty() {
            let missing = missing
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(invalid_verification_issues(format!(
                "item {index} is missing required field(s) {missing}"
            )));
        }

        for field in ["code", "suggestion"] {
            if let Some(value) = object.get(field) {
                if !value.is_string() && !(field == "suggestion" && value.is_null()) {
                    return Err(invalid_verification_issues(format!(
                        "item {index} field `{field}` must be a string{}",
                        if field == "suggestion" {
                            " or null"
                        } else {
                            ""
                        }
                    )));
                }
            }
        }
        if let Some(value) = object.get("line") {
            if !value.is_null()
                && !value
                    .as_u64()
                    .is_some_and(|line| line <= u64::from(u32::MAX))
            {
                return Err(invalid_verification_issues(format!(
                    "item {index} field `line` must be a non-negative 32-bit integer or null"
                )));
            }
        }
        if let Some(value) = object.get("severity") {
            if !matches!(value.as_str(), Some("blocking" | "warning")) {
                return Err(invalid_verification_issues(format!(
                    "item {index} field `severity` must be `blocking`, `warning`, or `high`"
                )));
            }
        }
    }

    serde_json::from_value(value)
        .map_err(|_| invalid_verification_issues("could not decode issue objects"))
}

impl CasCore {
    pub async fn cas_verification_add(
        &self,
        Parameters(req): Parameters<VerificationAddRequest>,
    ) -> Result<CallToolResult, McpError> {
        // cas-619f: an unknown verification_type used to fall through to a
        // task verdict silently. Only task and epic verdicts live here; the
        // independent QA verdict has its own action and record.
        if let Some(vtype) = req.verification_type.as_deref() {
            match vtype.trim().to_ascii_lowercase().as_str() {
                "" | "task" | "epic" => {}
                "qa" => {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(
                            "verification_type=qa is not recorded with action=add. The independent QA reviewer uses action=qa_record (task_id, status, summary, ledger_path); a supervisor waiver uses action=qa_waive.",
                        ),
                        data: None,
                    });
                }
                other => {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!(
                            "Unknown verification_type {other:?}: expected 'task' or 'epic'."
                        )),
                        data: None,
                    });
                }
            }
        }

        // Validate and sanitize caller-authored content before opening stores,
        // inspecting one-time authority, or applying expiry transitions.
        // This makes malformed issues failure-atomic even when the named
        // capability or dispatch is expired.
        let issues: Vec<VerificationIssue> = if let Some(issues_json) = &req.issues {
            parse_verification_issues(issues_json).map_err(|message| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(message),
                data: None,
            })?
        } else {
            Vec::new()
        };
        let files_reviewed: Vec<String> = req
            .files_reviewed
            .as_deref()
            .map(|files| {
                files
                    .split(',')
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let mut authored_content = Verification::new(String::new(), req.task_id.clone());
        authored_content.summary = req.summary.clone();
        authored_content.issues = issues;
        authored_content.files_reviewed = files_reviewed;
        authored_content.sanitize_verifier_authored_content();

        let verification_store = self.open_verification_store()?;
        let task_store = self.open_task_store()?;
        let agent_store = self.open_agent_store()?;

        // Verify task exists
        let task = task_store.get(&req.task_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        if let Some(target) = task.deliverables.work_target.as_ref() {
            crate::mcp::tools::core::task::repo_context::resolve_repo_context(
                &self.cas_root,
                target,
            )
            .map_err(|message| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(message),
                data: None,
            })?;
        }

        // cas-b269: refuse verification against an already-closed task so
        // stale post-close guidance cannot restart the verify/re-close loop.
        if crate::mcp::tools::core::task::lifecycle::stale_close_guard::is_terminal_closed(
            task.status,
        ) {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    crate::mcp::tools::core::task::lifecycle::stale_close_guard::verification_on_closed_message(
                        &req.task_id,
                    ),
                ),
                data: None,
            });
        }

        // Parse all caller-selected verdict state before a missing-dispatch
        // recovery can persist a new proof boundary. An invalid status must
        // remain failure-atomic rather than leaving a fresh pending dispatch.
        let status: VerificationStatus = req.status.parse().map_err(|_| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(
                "Invalid verification status. Expected: approved, rejected, error, or skipped.",
            ),
            data: None,
        })?;

        // Resolve authority only from the server's registered caller identity.
        // Caller-supplied names, models, verifier types, task ownership, harness
        // labels, and orphan state never grant verification authority.
        let caller_id = self.get_agent_id()?;
        let mut caller = agent_store.get(&caller_id).map_err(|_| McpError {
            code: ErrorCode::INVALID_REQUEST,
            message: Cow::from(
                "Verification requires an authenticated registered Cassy session. Anonymous or orphan callers cannot add verification records.",
            ),
            data: None,
        })?;
        if !caller.is_alive() {
            return Err(McpError {
                code: ErrorCode::INVALID_REQUEST,
                message: Cow::from(
                    "Verification caller is not an active registered Cassy session. Re-register before retrying.",
                ),
                data: None,
            });
        }

        let uses_verifier_capability = req.verifier_capability.is_some();
        let supervisor_direct = caller.role == cas_types::AgentRole::Supervisor
            && self.has_server_internal_identity(&caller_id)
            && !uses_verifier_capability;
        let uses_server_handoff = !uses_verifier_capability
            && caller.role == cas_types::AgentRole::Standard
            && caller.agent_type == cas_types::AgentType::SubAgent
            && caller.name == "task-verifier"
            && caller
                .parent_id
                .as_deref()
                .is_some_and(|parent_id| parent_id != caller_id);
        if !supervisor_direct && !uses_verifier_capability && !uses_server_handoff {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Verification authority rejected for task {}: workers and standard sessions cannot attest their own work.\n\n\
                     Legitimate paths:\n\
                       - Spawn a distinct registered task-verifier child; Cassy binds one sealed server-side handoff to its official child identity.\n\
                       - Ask a registered supervisor to verify directly.\n\n\
                     Task ownership, assignee/orphan state, harness/model labels, verification_type, and confidence do not grant authority.",
                    req.task_id
                )),
                data: None,
            });
        }
        let mut bound_server_handoff = None;
        let mut requested_dispatch_id = if let Some(capability_token) =
            req.verifier_capability.as_deref()
        {
            let capability = cas_store::inspect_verifier_capability(
                &self.cas_root,
                capability_token,
            )
            .map_err(|_| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Verifier capability rejected: it is malformed or unknown."),
                data: None,
            })?;
            let dispatch_id = capability.dispatch_id.clone().ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Verifier capability rejected: legacy authority has no exact proof boundary.",
                ),
                data: None,
            })?;
            if req
                .dispatch_id
                .as_deref()
                .is_some_and(|id| id != dispatch_id)
            {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verifier capability rejected: request names a different dispatch.",
                    ),
                    data: None,
                });
            }
            let dispatch = cas_store::get_verification_dispatch(&self.cas_root, &dispatch_id)
                .map_err(|_| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verifier capability rejected: its exact dispatch is unavailable.",
                    ),
                    data: None,
                })?;
            if capability.task_id != req.task_id || dispatch.task_id != req.task_id {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verifier capability rejected: exact dispatch belongs to another task.",
                    ),
                    data: None,
                });
            }
            if dispatch.deadline_at <= chrono::Utc::now() {
                let timed_out = cas_store::timeout_verification_dispatch(
                    &self.cas_root,
                    &req.task_id,
                    chrono::Utc::now(),
                )
                .map_err(|_| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verifier capability rejected: exact timeout transition could not be persisted; retry with current dispatch state.",
                    ),
                    data: None,
                })?
                .ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verifier capability rejected: dispatch changed before exact timeout persistence; retry with current dispatch state.",
                    ),
                    data: None,
                })?;
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Verifier capability rejected: exact dispatch {} expired and was marked timed_out; registered-supervisor recovery must name this dispatch.",
                        timed_out.id
                    )),
                    data: None,
                });
            }
            dispatch_id
        } else if uses_server_handoff {
            let capability = cas_store::inspect_bound_server_verifier_handoff(
                &self.cas_root,
                &req.task_id,
                &caller_id,
                req.dispatch_id.as_deref(),
            )
            .map_err(|_| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Verifier handoff rejected: no unique unconsumed server-side authority is bound to this registered child and task.",
                ),
                data: None,
            })?;
            if caller.parent_id.as_deref() != Some(capability.issuer_agent_id.as_str()) {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verifier handoff rejected: registered child parent does not match the server-bound issuer.",
                    ),
                    data: None,
                });
            }
            let dispatch_id = capability.dispatch_id.clone().ok_or_else(|| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Verifier handoff rejected: authority has no exact proof boundary.",
                ),
                data: None,
            })?;
            let dispatch = cas_store::get_verification_dispatch(&self.cas_root, &dispatch_id)
                .map_err(|_| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verifier handoff rejected: its exact dispatch is unavailable.",
                    ),
                    data: None,
                })?;
            if capability.task_id != req.task_id || dispatch.task_id != req.task_id {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verifier handoff rejected: exact dispatch belongs to another task.",
                    ),
                    data: None,
                });
            }
            if capability.expires_at <= chrono::Utc::now()
                || dispatch.deadline_at <= chrono::Utc::now()
            {
                let timed_out = cas_store::timeout_verification_dispatch(
                    &self.cas_root,
                    &req.task_id,
                    chrono::Utc::now(),
                )
                .map_err(|_| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verifier handoff rejected: exact timeout transition could not be persisted; retry with current dispatch state.",
                    ),
                    data: None,
                })?
                .ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verifier handoff rejected: dispatch changed before exact timeout persistence; retry with current dispatch state.",
                    ),
                    data: None,
                })?;
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Verifier handoff rejected: exact dispatch {} expired and was marked timed_out; registered-supervisor recovery must name this dispatch.",
                        timed_out.id
                    )),
                    data: None,
                });
            }
            bound_server_handoff = Some(capability);
            dispatch_id
        } else if let Some(dispatch_id) = req.dispatch_id.clone() {
            dispatch_id
        } else if supervisor_direct {
            // The active-task unique index guarantees one live boundary. Infer
            // only for this authenticated supervisor; child handoffs above stay
            // sealed, and expired/terminal cycles require explicit recovery.
            let live = cas_store::get_latest_verification_dispatch(&self.cas_root, &req.task_id)
                .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
                .filter(|dispatch| {
                    matches!(
                        dispatch.state,
                        cas_types::VerificationDispatchState::Pending
                            | cas_types::VerificationDispatchState::Claimed
                    ) && dispatch.deadline_at > chrono::Utc::now()
                });
            match live {
                Some(dispatch) => dispatch.id,
                // A restart can leave an immutable parked delivery without an
                // active review cycle; recover the boundary without a close attempt.
                None if task.status == TaskStatus::AwaitingMerge => {
                    self.recover_parked_verification_dispatch(&task, &caller)?
                        .id
                }
                None => {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "No active verification dispatch can be inferred. Retry task close to prepare the delivery; expired dispatch recovery requires its explicit dispatch_id.",
                    ));
                }
            }
        } else {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Verification requires dispatch_id naming an exact active proof boundary.",
                ),
                data: None,
            });
        };

        // A supervisor may submit the dispatch id from the handoff it read
        // after a close retry has already retired that row and minted the
        // next task-only repository cycle. Preserve exact capability and
        // child-handoff authority, but let a direct supervisor verdict follow
        // an unchanged proof boundary to the current pending dispatch.
        if supervisor_direct {
            requested_dispatch_id = cas_store::resolve_verification_dispatch_for_add(
                &self.cas_root,
                &requested_dispatch_id,
            )
            .map_err(|_| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Supervisor-direct verification rejected: named dispatch is unavailable.",
                ),
                data: None,
            })?
            .id;
        }

        // cas-b269: urgent stop halt blocks verification MCP.
        //
        // cas-3894: same owned-task exemption as `task action=close`
        // (`halt_exempt_for_owned_task`). Recorded incident: a worker's own
        // InProgress task is halted by an unrelated informational urgent,
        // and the documented escape (start a new task) is itself refused by
        // the verification jail until THIS task is verified — spawning the
        // task-verifier (which calls this endpoint) was as stuck as closing.
        // The exemption only skips the halt flag; it does not fabricate a
        // verification verdict.
        //
        // cas-1145: if an older assignment halt names a task that was never
        // created, clear it durably before applying the normal halt gate.
        let mut cleared_metadata = caller.metadata.clone();
        match crate::mcp::tools::core::task::lifecycle::stale_close_guard::clear_missing_task_halt(
            &mut cleared_metadata,
            task_store.as_ref(),
        ) {
            Ok(true) => {
                let previous_metadata = caller.metadata.clone();
                caller.metadata = cleared_metadata;
                if let Err(error) = agent_store.update(&caller) {
                    caller.metadata = previous_metadata;
                    tracing::warn!(
                        agent_id = %caller_id,
                        error = %error,
                        "cas-1145: failed to persist phantom urgent-halt cleanup; keeping halt"
                    );
                }
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(
                agent_id = %caller_id,
                error = %error,
                "cas-1145: could not verify urgent-halt task; keeping halt"
            ),
        }
        let halt_exempt =
            crate::mcp::tools::core::task::lifecycle::stale_close_guard::halt_exempt_for_owned_task(
                task.status,
                crate::mcp::tools::core::task::task_assignee_matches_agent(
                    agent_store.as_ref(),
                    task.assignee.as_deref(),
                    &caller,
                ),
            );
        if crate::mcp::tools::core::task::lifecycle::stale_close_guard::agent_task_work_halted(
            &caller.metadata,
        ) && !halt_exempt
        {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    crate::mcp::tools::core::task::lifecycle::stale_close_guard::halt_blocks_task_work_message(
                        "verification action=add",
                        crate::mcp::tools::core::task::lifecycle::stale_close_guard::halt_prompt_id(
                            &caller.metadata,
                        ),
                    ),
                ),
                data: None,
            });
        }

        // Legacy task-only verification is authorized against the exact Git
        // worktree snapshot captured when close created this dispatch. A
        // mutation while the verifier is reviewing invalidates only this task's
        // proof cycle; unrelated MCP and other-task work remains available.
        let proof_dispatch =
            cas_store::get_verification_dispatch(&self.cas_root, &requested_dispatch_id).map_err(
                |_| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("Verification rejected: exact dispatch is unavailable."),
                    data: None,
                },
            )?;
        // cas-5c33: the boundary that matters is the delivered commit
        // identity. A branch that moved on to the next task keeps the verdict
        // valid while every delivered commit is still reachable; the moved
        // tree is recorded on the verdict instead of refusing it.
        let mut repository_drift_note: Option<String> = None;
        if let Some(repository) = proof_dispatch.repository.as_ref() {
            match crate::mcp::tools::core::task::lifecycle::repository_proof::evaluate_repository_proof(
                repository,
            ) {
                Ok(status) => repository_drift_note = status.drift_note(),
                Err(error) => {
                    let superseded = cas_store::get_latest_verification_dispatch(
                        &self.cas_root,
                        &req.task_id,
                    )
                    .ok()
                    .flatten()
                    .filter(|current| current.id != proof_dispatch.id);
                    if proof_dispatch.state == cas_types::VerificationDispatchState::Invalidated
                        || superseded.is_some()
                    {
                        let bound_head = proof_dispatch
                            .repository
                            .as_ref()
                            .map(|proof| proof.head_commit.as_str())
                            .unwrap_or("unbound");
                        let current = superseded
                            .as_ref()
                            .map(|dispatch| {
                                let head = dispatch
                                    .repository
                                    .as_ref()
                                    .map(|proof| proof.head_commit.as_str())
                                    .unwrap_or("unbound");
                                format!(
                                    " Current dispatch {} is bound to head {}.",
                                    dispatch.id, head
                                )
                            })
                            .unwrap_or_default();
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(format!(
                                "Verification rejected: dispatch {} was superseded or invalidated (bound head {}).{} Re-run task close and submit the current dispatch instead of retrying this stale id.",
                                proof_dispatch.id, bound_head, current
                            )),
                            data: None,
                        });
                    }
                    cas_store::invalidate_verification_dispatch_for_repository_drift(
                        &self.cas_root,
                        &requested_dispatch_id,
                    )
                    .map_err(|store_error| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Repository proof changed, but exact task-scoped invalidation failed: {store_error}"
                        )),
                        data: None,
                    })?;
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!(
                            "Verification rejected: {error}. Retry task close to create a fresh dispatch."
                        )),
                        data: None,
                    });
                }
            }
        }

        let id = verification_store.generate_id().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate ID: {e}")),
            data: None,
        })?;

        let mut verification = Verification::new(id.clone(), req.task_id.clone());
        verification.status = status;
        verification.summary = match repository_drift_note.as_deref() {
            // The verdict record carries both digests so a reviewer can see
            // that the tree moved between dispatch and verdict (cas-5c33).
            Some(note) => format!("{}\n\n{note}", authored_content.summary),
            None => authored_content.summary,
        };
        verification.issues = authored_content.issues;
        verification.files_reviewed = authored_content.files_reviewed;
        if let Some(confidence) = req.confidence {
            verification.set_confidence(confidence);
        }
        if let Some(duration_ms) = req.duration_ms {
            verification.set_duration(duration_ms);
        }

        // Set verification type if specified (default is Task)
        if let Some(vtype) = &req.verification_type {
            if vtype.trim().eq_ignore_ascii_case("epic") {
                verification.verification_type = VerificationType::Epic;
            }
        }

        // Supervisor-direct calls can be retried after a lost response or a
        // process restart. The exact dispatch remains the authorization
        // boundary: only an identical verdict from the same registered
        // supervisor is idempotent. A conflicting retry fails closed.
        if supervisor_direct {
            verification.set_agent(caller_id.clone());
            verification.provenance = cas_types::VerificationProvenance::SupervisorDirect;
            verification.dispatch_id = Some(requested_dispatch_id.clone());
            verification.issuer_agent_id = Some(caller_id.clone());
            if let Some(existing) =
                cas_store::get_verification_for_dispatch(&self.cas_root, &requested_dispatch_id)
                    .map_err(|e| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Failed to inspect exact verification dispatch result: {e}"
                        )),
                        data: None,
                    })?
            {
                if supervisor_verification_retry_matches(&existing, &verification) {
                    // The verdict may have committed while the best-effort
                    // post-commit wake failed. An identical supervisor retry
                    // repairs that wake without recording a second verdict.
                    let retry_note = if existing.status == VerificationStatus::Approved
                        && existing.verification_type == VerificationType::Task
                        && proof_dispatch.delivery_transaction_id.is_none()
                    {
                        match crate::mcp::tools::core::task::lifecycle::supervisor_push::queue_approved_verification_close_retry(
                            &self.cas_root,
                            &task,
                            &requested_dispatch_id,
                        ) {
                            Ok(crate::mcp::tools::core::task::lifecycle::supervisor_push::ApprovedCloseRetry::Queued(prompt_id)) => format!("; close retry prompt {prompt_id} queued"),
                            Ok(crate::mcp::tools::core::task::lifecycle::supervisor_push::ApprovedCloseRetry::NotQueued(reason)) => format!("; no close retry queued: {reason}"),
                            Err(error) => format!("; close retry wake failed: {error}"),
                        }
                    } else {
                        String::new()
                    };
                    return Ok(Self::success(format!(
                        "{} Verification {} for task {} - {}: {} (idempotent retry{})",
                        verification_status_emoji(existing.status),
                        existing.id,
                        req.task_id,
                        task.title,
                        existing.summary,
                        retry_note
                    )));
                }
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(concat!(
                        "Supervisor-direct verification rejected: the exact dispatch was ",
                        "already resolved with a different verdict."
                    )),
                    data: None,
                });
            }
        }

        // Atomically persist the verdict, resolve any active exact-task
        // dispatch, and clear that task's pending transition. If any step
        // fails, all authority and lifecycle writes roll back.
        {
            let db_path = self.cas_root.join("cas.db");
            let conn = rusqlite::Connection::open(&db_path).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to open database: {e}")),
                data: None,
            })?;
            // GH #1165: the pool's budget-aware busy handler.
            cas_store::shared_db::install_busy_handler(&conn).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to set busy timeout: {e}")),
                data: None,
            })?;

            // BEGIN IMMEDIATE, not the DEFERRED default. The block below reads
            // (the dispatch, the capability) before it writes, so a deferred
            // transaction would hold a read snapshot and have to UPGRADE to a
            // writer — and SQLite refuses that upgrade with SQLITE_BUSY without
            // ever calling the busy handler, because waiting there could
            // deadlock. That is why four consecutive verification writes failed
            // in milliseconds with a 5s busy_timeout configured, while
            // single-statement writes in the same seconds succeeded (cas-759f).
            // Taking the write lock up front puts the wait where the busy
            // handler applies; the retry inside covers a holder that outlives
            // one timeout window, and stops before the body runs, so nothing
            // here is executed twice.
            let tx =
                cas_store::shared_db::begin_immediate_with_retry(&conn).map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to begin transaction: {e}")),
                    data: None,
                })?;

            let dispatch = if let Some(capability_token) = req.verifier_capability.as_deref() {
                let capability = cas_store::consume_verifier_capability_with_conn(
                    &tx,
                    capability_token,
                    &req.task_id,
                    &caller_id,
                )
                .map_err(|_| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verifier capability rejected: it is invalid, expired, consumed, bound to another task/session, or was not issued to a distinct registered task-verifier child.",
                    ),
                    data: None,
                })?;
                if caller.agent_type != cas_types::AgentType::SubAgent
                    || caller.parent_id.as_deref() != Some(capability.issuer_agent_id.as_str())
                    || capability.verifier_agent_id.as_deref() != Some(caller_id.as_str())
                {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(
                            "Verifier capability rejected: caller is not the distinct registered task-verifier child bound by Cassy.",
                        ),
                        data: None,
                    });
                }
                verification.set_agent(caller_id.clone());
                verification.provenance = cas_types::VerificationProvenance::TaskVerifier;
                verification.capability_id = Some(capability.id);
                verification.dispatch_id = Some(requested_dispatch_id.clone());
                verification.issuer_agent_id = Some(capability.issuer_agent_id);
                cas_store::get_verification_dispatch_with_conn(&tx, &requested_dispatch_id)
                    .map_err(|_| McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(
                            "Verifier capability rejected: exact dispatch is unavailable.",
                        ),
                        data: None,
                    })?
            } else if let Some(bound_handoff) = bound_server_handoff.as_ref() {
                let capability = cas_store::consume_server_verifier_handoff_with_conn(
                    &tx,
                    &bound_handoff.id,
                    &req.task_id,
                    &caller_id,
                )
                .map_err(|_| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verifier handoff rejected: it is invalid, expired, consumed, bound to another task/session, or no longer claims the exact dispatch.",
                    ),
                    data: None,
                })?;
                if caller.agent_type != cas_types::AgentType::SubAgent
                    || caller.role != cas_types::AgentRole::Standard
                    || caller.name != "task-verifier"
                    || caller.parent_id.as_deref() != Some(capability.issuer_agent_id.as_str())
                    || capability.verifier_agent_id.as_deref() != Some(caller_id.as_str())
                {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(
                            "Verifier handoff rejected: caller is not the distinct registered task-verifier child bound by Cassy.",
                        ),
                        data: None,
                    });
                }
                verification.set_agent(caller_id.clone());
                verification.provenance = cas_types::VerificationProvenance::TaskVerifier;
                verification.capability_id = Some(capability.id);
                verification.dispatch_id = Some(requested_dispatch_id.clone());
                verification.issuer_agent_id = Some(capability.issuer_agent_id);
                cas_store::get_verification_dispatch_with_conn(&tx, &requested_dispatch_id)
                    .map_err(|_| McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(
                            "Verifier handoff rejected: exact dispatch is unavailable.",
                        ),
                        data: None,
                    })?
            } else {
                // Supervisor provenance is populated before the transaction
                // so an identical retry can be compared to the durable row.
                cas_store::get_verification_dispatch_with_conn(&tx, &requested_dispatch_id)
                    .map_err(|_| McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(
                            "Supervisor-direct verification rejected: named dispatch is unavailable.",
                        ),
                        data: None,
                    })?
            };
            if dispatch.task_id != req.task_id {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Verification rejected: named dispatch belongs to another task.",
                    ),
                    data: None,
                });
            }

            cas_store::add_verification_with_conn(&tx, &verification).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to add verification: {e}")),
                data: None,
            })?;

            cas_store::resolve_verification_dispatch_with_conn(
                &tx,
                &dispatch.id,
                &caller_id,
                verification.capability_id.as_deref(),
                supervisor_direct,
            )
            .map_err(|_| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Verification dispatch resolution rejected: caller is not the bound verifier or a registered supervisor-direct authority.",
                ),
                data: None,
            })?;

            let approved_delivery = matches!(
                verification.status,
                VerificationStatus::Approved | VerificationStatus::Skipped
            );
            let delivery_transitioned = if let Some(delivery_transaction_id) =
                dispatch.delivery_transaction_id.as_deref()
                && cas_store::transition_worker_delivery_verification_with_conn(
                    &tx,
                    delivery_transaction_id,
                    &verification.id,
                    approved_delivery,
                    &caller_id,
                )
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "Failed to advance transactional worker delivery: {e}"
                    )),
                    data: None,
                })?
                .is_some()
            {
                true
            } else {
                false
            };

            if delivery_transitioned {
                tx.execute(
                    "UPDATE tasks
                     SET status = ?2, pending_verification = 0, updated_at = ?3
                     WHERE id = ?1 AND status IN ('in_progress', 'awaiting_merge')",
                    rusqlite::params![
                        req.task_id,
                        if approved_delivery {
                            "awaiting_merge"
                        } else {
                            "blocked"
                        },
                        chrono::Utc::now().to_rfc3339(),
                    ],
                )
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "Failed to project delivery verification state: {e}"
                    )),
                    data: None,
                })?;
            }

            if task.pending_verification {
                cas_store::clear_pending_verification_with_conn(&tx, &req.task_id).map_err(
                    |e| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!("Failed to clear pending_verification: {e}")),
                        data: None,
                    },
                )?;
            }

            tx.commit().map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to commit verification transaction: {e}")),
                data: None,
            })?;
        }

        crate::factory_review_scratch::reclaim_task_targets(&self.cas_root, &req.task_id).await;

        // Emit VerificationAdded event for task lifecycle tracking
        if let Ok(event_store) = cas_store::SqliteEventStore::open(&self.cas_root) {
            use cas_store::EventStore;
            use cas_types::{Event, EventEntityType, EventType};
            let status_str = match verification.status {
                VerificationStatus::Approved => "approved",
                VerificationStatus::Rejected => "rejected",
                VerificationStatus::Error => "error",
                VerificationStatus::Skipped => "skipped",
            };
            let event = Event::new(
                EventType::VerificationAdded,
                EventEntityType::Verification,
                &id,
                format!(
                    "Verification {}: {} - {}",
                    status_str, req.task_id, verification.summary
                ),
            )
            .with_metadata(serde_json::json!({
                "task_id": req.task_id,
                "status": status_str,
                "verification_type": verification.verification_type.to_string(),
                "provenance": verification.provenance.to_string(),
                "capability_id": verification.capability_id,
                "dispatch_id": verification.dispatch_id,
            }));
            // Add session ID if available for linking to the verifying agent
            let event = if let Some(Some(agent_id)) = self.agent_id.get() {
                event.with_session(agent_id)
            } else {
                event
            };
            let _ = event_store.record(&event);
        }

        let status_emoji = verification_status_emoji(verification.status);

        // GH #1036: the first worker retry may race the supervisor verdict.
        // Queue only after the verdict transaction commits, so a woken worker
        // can always observe the approval. A transactional delivery still
        // needs its merge transition before close and is excluded here.
        // cas-46ea: a merged delivery is closed or returned by its verdict;
        // the merge fact plus this dispatch-bound verdict are the authority.
        let cassy_merged = task.deliverables.green_merge_commit.is_some()
            && task.deliverables.effective_merge_ci_admission()
                != cas_types::MergeCiAdmission::Override;
        let close_retry = if verification.status == VerificationStatus::Approved
            && verification.verification_type == VerificationType::Task
            && (task.pending_verification || cassy_merged)
            && proof_dispatch.delivery_transaction_id.is_none()
        {
            // cas-eda0: a delivery Cassy merged green is closed by Cassy now;
            // the worker retry remains the fallback when a gate still holds.
            use crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose;
            let cassy_close = self
                .close_green_merged_delivery_after_verdict(&req.task_id, Some(&verification.id))
                .await;
            let worker_retry = match &cassy_close {
                CassyMergeClose::Closed { .. } | CassyMergeClose::AlreadyClosed => String::new(),
                // cas-e9d6: Cassy's close of a merged delivery runs the
                // worker close's gates on the same evidence, so a worker
                // retry cannot pass where it held. The gate is the
                // supervisor's to resolve; never relay it to the worker.
                CassyMergeClose::Held(_) if cassy_merged => {
                    "\nNo worker close retry was queued: the gate above is the supervisor's to resolve before this merged delivery can close.".to_string()
                }
                CassyMergeClose::NotEligible | CassyMergeClose::Held(_) => match task_store.get(&req.task_id) {
                    Ok(current) => match crate::mcp::tools::core::task::lifecycle::supervisor_push::queue_approved_verification_close_retry(
                        &self.cas_root,
                        &current,
                        &requested_dispatch_id,
                    ) {
                        Ok(crate::mcp::tools::core::task::lifecycle::supervisor_push::ApprovedCloseRetry::Queued(prompt_id)) => format!("\nClose retry queued for the assigned worker (prompt {prompt_id})."),
                        // cas-7fc3 (GH #1388): name the actual reason instead
                        // of claiming no live worker exists.
                        Ok(crate::mcp::tools::core::task::lifecycle::supervisor_push::ApprovedCloseRetry::NotQueued(reason)) => format!("\nNo close retry was queued: {reason}. A supervisor can retry task close."),
                        Err(error) => format!("\nClose retry could not be queued after the verdict was recorded: {error}. Retry task close or notify the worker."),
                    },
                    Err(error) => format!("\nClose retry could not read the task after the verdict was recorded: {error}. Retry task close or notify the worker."),
                },
            };
            format!("{}{worker_retry}", cassy_close.receipt(&req.task_id))
        } else if verification.status == VerificationStatus::Rejected
            && verification.verification_type == VerificationType::Task
            && cassy_merged
            && proof_dispatch.delivery_transaction_id.is_none()
        {
            self.return_rejected_merged_delivery(&req.task_id, &verification)
        } else {
            String::new()
        };

        if let Some(outcome) = crate::quality_ledger::verifier_outcome(verification.status) {
            crate::quality_ledger::check_run(&self.cas_root, crate::quality_ledger::Check {
                natural_key: format!("verifier|{id}"),
                check_id: "task_verifier",
                stage: cas_store::CheckStage::Verifier,
                outcome,
                task_id: Some(&req.task_id),
                sha: None,
                minutes: req.duration_ms.map(|ms| ms as f64 / 60_000.0),
                link: None,
                actor: Some(&caller_id),
            });
        }

        Ok(Self::success(format!(
            "{} Verification {} for task {} - {}: {}{}",
            status_emoji,
            id,
            req.task_id,
            truncate_str(&task.title, 120),
            verification_receipt_summary(&verification),
            close_retry
        )))
    }

    /// Show verification details
    pub async fn cas_verification_show(
        &self,
        Parameters(req): Parameters<VerificationShowRequest>,
    ) -> Result<CallToolResult, McpError> {
        let verification_store = self.open_verification_store()?;

        let mut verification = verification_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Verification not found: {e}")),
            data: None,
        })?;
        verification.sanitize_verifier_authored_content();

        let mut output = format!(
            "Verification: {}\n{}\n\nTask: {}\nStatus: {}\nSummary: {}\n",
            verification.id,
            "=".repeat(verification.id.len() + 14),
            verification.task_id,
            verification.status,
            verification.summary
        );

        if let Some(confidence) = verification.confidence {
            output.push_str(&format!("Confidence: {:.0}%\n", confidence * 100.0));
        }

        if let Some(agent_id) = &verification.agent_id {
            output.push_str(&format!("Verified by: {agent_id}\n"));
        }
        output.push_str(&format!("Provenance: {}\n", verification.provenance));
        if let Some(capability_id) = &verification.capability_id {
            output.push_str(&format!("Verifier capability: {capability_id}\n"));
        }

        if let Some(duration) = verification.duration_ms {
            output.push_str(&format!("Duration: {duration}ms\n"));
        }

        if !verification.files_reviewed.is_empty() {
            output.push_str(&format!(
                "\nFiles Reviewed ({}):\n",
                verification.files_reviewed.len()
            ));
            for file in &verification.files_reviewed {
                output.push_str(&format!("  - {file}\n"));
            }
        }

        if !verification.issues.is_empty() {
            let blocking = verification.blocking_count();
            let warnings = verification.warning_count();
            output.push_str(&format!(
                "\nIssues ({blocking} blocking, {warnings} warnings):\n"
            ));
            for issue in &verification.issues {
                let severity_icon = if issue.is_blocking() {
                    "🚫"
                } else {
                    "⚠️"
                };
                let location = if let Some(line) = issue.line {
                    format!("{}:{}", issue.file, line)
                } else {
                    issue.file.clone()
                };
                output.push_str(&format!(
                    "\n{} [{}] {}\n",
                    severity_icon, issue.category, location
                ));
                output.push_str(&format!("   Problem: {}\n", issue.problem));
                if !issue.code.is_empty() {
                    output.push_str(&format!("   Code: {}\n", issue.code));
                }
                if let Some(suggestion) = &issue.suggestion {
                    output.push_str(&format!("   Suggestion: {suggestion}\n"));
                }
            }
        }

        output.push_str(&format!(
            "\nCreated: {}\n",
            verification.created_at.format("%Y-%m-%d %H:%M:%S")
        ));

        Ok(Self::success(output))
    }

    /// List verifications for a task
    pub async fn cas_verification_list(
        &self,
        Parameters(req): Parameters<VerificationListRequest>,
    ) -> Result<CallToolResult, McpError> {
        let verification_store = self.open_verification_store()?;

        let mut verifications =
            verification_store
                .get_for_task(&req.task_id)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to list verifications: {e}")),
                    data: None,
                })?;
        for verification in &mut verifications {
            verification.sanitize_verifier_authored_content();
        }

        if verifications.is_empty() {
            return Ok(Self::success(format!(
                "No verifications for task {}",
                req.task_id
            )));
        }

        let limit = req.limit.unwrap_or(10);
        let mut output = format!(
            "Verifications for {} ({} total):\n\n",
            req.task_id,
            verifications.len()
        );

        for v in verifications.iter().take(limit) {
            let status_icon = match v.status {
                VerificationStatus::Approved => "✅",
                VerificationStatus::Rejected => "❌",
                VerificationStatus::Error => "⚠️",
                VerificationStatus::Skipped => "⏭️",
            };
            let issues_info = if v.issues.is_empty() {
                String::new()
            } else {
                format!(
                    " ({} blocking, {} warnings)",
                    v.blocking_count(),
                    v.warning_count()
                )
            };
            output.push_str(&format!(
                "{} {} - {}{}\n   {}\n\n",
                status_icon,
                v.id,
                v.status,
                issues_info,
                truncate_str(&v.summary, 80)
            ));
        }

        Ok(Self::success(output))
    }

    /// Get latest verification for a task
    pub async fn cas_verification_latest(
        &self,
        Parameters(req): Parameters<VerificationListRequest>,
    ) -> Result<CallToolResult, McpError> {
        let verification_store = self.open_verification_store()?;

        match verification_store
            .get_latest_for_task(&req.task_id)
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to get verification: {e}")),
                data: None,
            })? {
            Some(mut v) => {
                v.sanitize_verifier_authored_content();
                let status_icon = match v.status {
                    VerificationStatus::Approved => "✅",
                    VerificationStatus::Rejected => "❌",
                    VerificationStatus::Error => "⚠️",
                    VerificationStatus::Skipped => "⏭️",
                };
                let issues_info = if v.issues.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\nIssues: {} blocking, {} warnings",
                        v.blocking_count(),
                        v.warning_count()
                    )
                };
                Ok(Self::success(format!(
                    "{} Latest verification for {}:\n\nID: {}\nStatus: {}\nSummary: {}{}",
                    status_icon,
                    req.task_id,
                    v.id,
                    v.status,
                    verification_receipt_summary(&v),
                    issues_info
                )))
            }
            None => Ok(Self::success(format!(
                "No verifications found for task {}",
                req.task_id
            ))),
        }
    }

    fn recover_parked_verification_dispatch(
        &self,
        task: &Task,
        caller: &cas_types::Agent,
    ) -> Result<cas_types::VerificationDispatch, McpError> {
        let refuse = |message: String| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Parked verification recovery refused: {message}")),
            data: None,
        };
        if crate::mcp::tools::core::task::lifecycle::stale_close_guard::agent_task_work_halted(
            &caller.metadata,
        ) {
            return Err(refuse("the supervisor's task work is halted".into()));
        }
        let context = task
            .deliverables
            .work_target
            .as_ref()
            .map(|target| {
                crate::mcp::tools::core::task::repo_context::resolve_repo_context(
                    &self.cas_root,
                    target,
                )
            })
            .transpose()
            .map_err(&refuse)?;
        let root = context
            .as_ref()
            .map(|context| context.repo_root.as_path())
            .or_else(|| self.cas_root.parent())
            .ok_or_else(|| refuse("repository root unavailable".into()))?;
        let proof = crate::mcp::tools::core::task::lifecycle::repository_proof::capture_parked_delivery_proof(
            root, task, context.as_ref().map(|context| context.target_branch.as_str()),
        ).map_err(&refuse)?;
        let boundary = match cas_store::get_latest_worker_delivery(&self.cas_root, &task.id)
            .map_err(|error| refuse(error.to_string()))?
        {
            Some((receipt, transaction)) => {
                if proof.anchor_commits != vec![receipt.commit_sha.clone()] {
                    return Err(refuse(
                        "delivery receipt no longer matches the parked anchor".into(),
                    ));
                }
                cas_types::VerificationProofBoundary::delivery(receipt.id, transaction.id)
            }
            None => cas_types::VerificationProofBoundary::task_at(proof),
        };
        let now = chrono::Utc::now();
        cas_store::timeout_verification_dispatch(&self.cas_root, &task.id, now)
            .map_err(|error| refuse(error.to_string()))?;
        cas_store::create_verification_dispatch_bound(
            &self.cas_root,
            &task.id,
            &caller.id,
            &caller.id,
            &boundary,
            now + chrono::Duration::minutes(10),
            true,
        )
        .map_err(|error| refuse(error.to_string()))
    }

    // ========================================================================
    // Worktree Operations
    // ========================================================================
}

/// Successful receipts link to the full stored record; findings stay complete.
fn verification_receipt_summary(verification: &Verification) -> String {
    if verification.status == VerificationStatus::Approved && verification.issues.is_empty() {
        format!(
            "{}\nFull evidence: verification action=show id={}",
            truncate_str(&verification.summary, 240),
            verification.id
        )
    } else {
        verification.summary.clone()
    }
}

fn verification_status_emoji(status: VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::Approved => "✅",
        VerificationStatus::Rejected => "❌",
        VerificationStatus::Error => "⚠️",
        VerificationStatus::Skipped => "⏭️",
    }
}

fn supervisor_verification_retry_matches(
    existing: &Verification,
    candidate: &Verification,
) -> bool {
    existing.task_id == candidate.task_id
        && existing.agent_id == candidate.agent_id
        && existing.verification_type == candidate.verification_type
        && existing.provenance == candidate.provenance
        && existing.capability_id == candidate.capability_id
        && existing.dispatch_id == candidate.dispatch_id
        && existing.issuer_agent_id == candidate.issuer_agent_id
        && existing.status == candidate.status
        && existing.confidence == candidate.confidence
        && existing.summary == candidate.summary
        && existing.files_reviewed == candidate.files_reviewed
        && existing.duration_ms == candidate.duration_ms
        && serde_json::to_value(&existing.issues).ok()
            == serde_json::to_value(&candidate.issues).ok()
}

#[cfg(test)]
mod lifecycle_friction_tests {
    use super::*;
    use crate::mcp::CasCore;
    use crate::test_support::TestEnvGuard;
    use cas_types::{Agent, AgentRole, Task, TaskStatus, VerificationProofBoundary};

    fn fixture(
        env: &mut TestEnvGuard,
    ) -> (
        tempfile::TempDir,
        CasCore,
        Task,
        cas_types::VerificationDispatch,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        env.set("CAS_ROOT", &root);
        env.set("CAS_AGENT_ROLE", "supervisor");
        env.set("XDG_CONFIG_HOME", env.home().join(".config"));
        let core = CasCore::with_daemon(root.clone(), None, None);
        let agents = core.open_agent_store().unwrap();
        agents
            .register(&Agent::new_with_role(
                "review-supervisor".into(),
                "supervisor".into(),
                AgentRole::Supervisor,
            ))
            .unwrap();
        core.set_agent_id_for_testing("review-supervisor".into());
        let mut task = Task::new("cas-5dc0-fixture".into(), "Review lifecycle fixture".into());
        task.status = TaskStatus::AwaitingMerge;
        task.assignee = Some("implementer".into());
        task.deliverables.factory_branch_anchor = Some("a".repeat(40));
        task.deliverables.parked_branch = Some("factory/implementer".into());
        core.open_task_store().unwrap().add(&task).unwrap();
        let dispatch = cas_store::create_verification_dispatch_bound(
            &root,
            &task.id,
            "implementer",
            "review-supervisor",
            &VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10),
            false,
        )
        .unwrap();
        (dir, core, task, dispatch)
    }

    #[tokio::test]
    async fn cas_5dc0_first_add_binds_the_unique_active_dispatch_without_retry() {
        let mut env = TestEnvGuard::temp_home();
        let (_dir, core, task, dispatch) = fixture(&mut env);
        let summary = "Independent review completed. ".repeat(100);
        let result = core
            .cas_verification_add(Parameters(
                serde_json::from_value(serde_json::json!({
                    "task_id":task.id,"status":"approved","summary":summary
                }))
                .unwrap(),
            ))
            .await
            .expect("first add must bind the one active dispatch");
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let recorded = cas_store::get_verification_for_dispatch(&core.cas_root, &dispatch.id)
            .unwrap()
            .unwrap();
        assert_eq!(recorded.dispatch_id.as_deref(), Some(dispatch.id.as_str()));
        let text = |result: CallToolResult| {
            result
                .content
                .into_iter()
                .filter_map(|content| {
                    if let rmcp::model::RawContent::Text(text) = content.raw {
                        Some(text.text)
                    } else {
                        None
                    }
                })
                .collect::<String>()
        };
        let receipt = text(result);
        let latest = text(
            core.cas_verification_latest(Parameters(VerificationListRequest {
                task_id: task.id.clone(),
                limit: None,
            }))
            .await
            .unwrap(),
        );
        let full = text(
            core.cas_verification_show(Parameters(VerificationShowRequest {
                id: recorded.id.clone(),
            }))
            .await
            .unwrap(),
        );
        eprintln!(
            "VERIFICATION_REPLAY summary_bytes={} receipt_bytes={} latest_bytes={} full_bytes={}",
            summary.len(),
            receipt.len(),
            latest.len(),
            full.len()
        );
        for compact in [&receipt, &latest] {
            assert!(
                compact.len() <= 2000,
                "successful verification repeats proof: {compact}"
            );
            assert!(compact.contains(&recorded.id));
            assert!(
                compact.contains("action=show"),
                "full proof needs a retrieval command"
            );
        }
        assert!(full.contains(&summary), "full proof must remain lossless");
        // A terminal or missing dispatch is never silently inferred as new authority.
        assert!(
            core.cas_verification_add(Parameters(
                serde_json::from_value(serde_json::json!({
                    "task_id":task.id,"status":"approved","summary":"Unbound duplicate"
                }))
                .unwrap()
            ))
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn cas_5dc0_request_changes_recovers_a_blocked_parked_verifier_failure() {
        let mut env = TestEnvGuard::temp_home();
        let (_dir, core, task, dispatch) = fixture(&mut env);
        let findings = "Correction required with detailed findings. ".repeat(75);
        let failure = core.cas_verification_add(Parameters(serde_json::from_value(serde_json::json!({
            "task_id":task.id,"dispatch_id":dispatch.id,"status":"rejected","summary":findings
        })).unwrap())).await.unwrap();
        assert!(
            format!("{failure:?}").contains(&findings),
            "failure findings must stay complete"
        );
        let tasks = core.open_task_store().unwrap();
        let mut blocked = tasks.get(&task.id).unwrap();
        blocked.status = TaskStatus::Blocked;
        tasks.update(&blocked).unwrap();
        let result = core
            .cas_task_request_changes(Parameters(crate::mcp::tools::TaskRequestChangesRequest {
                id: task.id.clone(),
                reason: "Fix the rejected delivery while keeping its ownership".into(),
            }))
            .await
            .expect("blocked parked deliveries need the same audited correction exit");
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let reopened = tasks.get(&task.id).unwrap();
        assert_eq!(reopened.status, TaskStatus::Open);
        assert_eq!(reopened.assignee, task.assignee);
        assert!(reopened.deliverables.factory_branch_anchor.is_none());
        assert!(
            reopened
                .deliverables
                .historical_factory_branch_anchors
                .contains(&"a".repeat(40))
        );
        assert!(!reopened.pending_verification);
        assert_eq!(
            cas_store::get_verification_dispatch(&core.cas_root, &dispatch.id)
                .unwrap()
                .state,
            cas_types::VerificationDispatchState::Invalidated
        );
    }
}
