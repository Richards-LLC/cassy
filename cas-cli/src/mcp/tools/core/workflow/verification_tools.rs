use crate::mcp::tools::core::imports::*;

/// Structured task label marking an Open task that was reopened by a rejected
/// supervisor review and therefore needs a supervisor recovery decision.
pub const VERIFICATION_REJECTED_REOPEN_LABEL: &str = "verification-rejected-reopen";

impl CasCore {
    pub async fn cas_verification_add(
        &self,
        Parameters(req): Parameters<VerificationAddRequest>,
    ) -> Result<CallToolResult, McpError> {
        // Validate and sanitize caller-authored content before opening stores,
        // inspecting one-time authority, or applying expiry transitions.
        // This makes malformed issues failure-atomic even when the named
        // capability or dispatch is expired.
        let issues: Vec<VerificationIssue> = if let Some(issues_json) = &req.issues {
            if issues_json.len() > 512 * 1024 {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Invalid verification issues: expected a bounded JSON array of issue objects; input omitted.",
                    ),
                    data: None,
                });
            }
            serde_json::from_str(issues_json).map_err(|_| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Invalid verification issues: expected a bounded JSON array of issue objects; input omitted.",
                ),
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
        let caller = agent_store.get(&caller_id).map_err(|_| McpError {
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
        let requested_dispatch_id = if let Some(capability_token) =
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
        } else {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Verification requires dispatch_id naming an exact active proof boundary.",
                ),
                data: None,
            });
        };

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
        let halt_exempt =
            crate::mcp::tools::core::task::lifecycle::stale_close_guard::halt_exempt_for_owned_task(
                task.status,
                task.assignee.as_deref(),
                Some(caller.name.as_str()),
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
        if let Some(repository) = proof_dispatch.repository.as_ref()
            && let Err(error) =
                crate::mcp::tools::core::task::lifecycle::repository_proof::verify_repository_proof(
                    repository,
                )
        {
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

        let id = verification_store.generate_id().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate ID: {e}")),
            data: None,
        })?;

        let mut verification = Verification::new(id.clone(), req.task_id.clone());
        verification.status = status;
        verification.summary = authored_content.summary;
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
            if vtype == "epic" {
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
                    return Ok(Self::success(format!(
                        "{} Verification {} for task {} - {}: {} (idempotent retry)",
                        verification_status_emoji(existing.status),
                        existing.id,
                        req.task_id,
                        task.title,
                        existing.summary
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
            conn.busy_timeout(cas_store::SQLITE_BUSY_TIMEOUT)
                .map_err(|e| McpError {
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

        Ok(Self::success(format!(
            "{} Verification {} for task {} - {}: {}",
            status_emoji, id, req.task_id, task.title, verification.summary
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
                    status_icon, req.task_id, v.id, v.status, v.summary, issues_info
                )))
            }
            None => Ok(Self::success(format!(
                "No verifications found for task {}",
                req.task_id
            ))),
        }
    }

    // ========================================================================
    // Worktree Operations
    // ========================================================================
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
