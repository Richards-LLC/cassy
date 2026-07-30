use crate::harness_policy::worker_harness_from_env;
use crate::mcp::tools::core::imports::*;

impl CasCore {
    pub async fn cas_verification_add(
        &self,
        Parameters(req): Parameters<VerificationAddRequest>,
    ) -> Result<CallToolResult, McpError> {
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
        if let Ok(agent_id) = self.get_agent_id() {
            if let Ok(agent) = agent_store.get(&agent_id) {
                let halt_exempt =
                    crate::mcp::tools::core::task::lifecycle::stale_close_guard::halt_exempt_for_owned_task(
                        task.status,
                        task.assignee.as_deref(),
                        Some(agent.name.as_str()),
                    );
                if crate::mcp::tools::core::task::lifecycle::stale_close_guard::agent_task_work_halted(
                    &agent.metadata,
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
            }
        }

        // Supervisors can usually only verify epics.
        // Exceptions:
        // 1. Factory sessions with Codex workers (no subagent support)
        // 2. Task-verifier subagent running within supervisor context
        // 3. Supervisor is the task assignee (self-implemented task)
        // 4. Task assignee is inactive (orphaned task)
        if let Ok(agent_id) = self.get_agent_id() {
            if let Ok(agent) = agent_store.get(&agent_id) {
                let worker_supports_subagents =
                    worker_harness_from_env().capabilities().supports_subagents;
                if agent.role == cas_types::AgentRole::Supervisor
                    && task.task_type != crate::types::TaskType::Epic
                    && worker_supports_subagents
                {
                    // Check if this is a task-verifier subagent context
                    let is_verifier_subagent = self
                        .cas_root
                        .join(".verifier_unjail_marker")
                        .exists();

                    // Check if supervisor is the task assignee
                    let supervisor_is_assignee =
                        task.assignee.as_deref() == Some(agent_id.as_str());

                    // Check if task assignee is inactive (orphaned).
                    // `unwrap_or(true)` semantics: missing assignee, missing
                    // agent record, dead agent, or stale heartbeat all count
                    // as "inactive_or_absent" — the rejection only fires
                    // when there is a *currently live* assignee.
                    let assignee_inactive_or_absent = task
                        .assignee
                        .as_deref()
                        .map(|aid| {
                            agent_store
                                .get(aid)
                                .map(|a| !a.is_alive() || a.is_heartbeat_expired(300))
                                .unwrap_or(true)
                        })
                        .unwrap_or(true); // no assignee → treat as orphaned

                    if !is_verifier_subagent
                        && !supervisor_is_assignee
                        && !assignee_inactive_or_absent
                    {
                        // Safe: assignee_inactive_or_absent is false here,
                        // which requires task.assignee to be Some(_).
                        let assignee_id = task.assignee.as_deref().unwrap_or("<unknown>");
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(format!(
                                "Cannot verify task {task_id}: it has an active assignee ({assignee_id}).\n\n\
                                Supervisors may verify individual tasks only when:\n\
                                  - the task has no assignee (orphaned), OR\n\
                                  - the assignee is inactive (dead session or heartbeat expired >5min ago), OR\n\
                                  - the supervisor IS the assignee (self-implemented task).\n\n\
                                Epics may always be verified by supervisors.\n\n\
                                Remediation:\n\
                                  - Ask {assignee_id} to verify and close the task themselves:\n\
                                    mcp__cas__coordination action=message target={assignee_id} message=\"Please verify and close task {task_id}\"\n\
                                  - Or release their lease and take over:\n\
                                    mcp__cas__task action=release id={task_id}\n\
                                  - Or wait for the assignee to disconnect (heartbeat expires after 5min).",
                                task_id = req.task_id,
                                assignee_id = assignee_id,
                            )),
                            data: None,
                        });
                    }
                }
            }
        }

        let id = verification_store.generate_id().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate ID: {e}")),
            data: None,
        })?;

        let status: VerificationStatus = req.status.parse().unwrap_or(VerificationStatus::Approved);

        // Parse issues from JSON if provided
        let issues: Vec<VerificationIssue> = if let Some(issues_json) = &req.issues {
            match serde_json::from_str(issues_json) {
                Ok(parsed) => parsed,
                Err(e) => {
                    eprintln!(
                        "[CAS] Warning: Failed to parse issues JSON: {e}. Input was: {issues_json}"
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        // Parse files reviewed
        let files_reviewed: Vec<String> = req
            .files_reviewed
            .map(|f| {
                f.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let mut verification = Verification::new(id.clone(), req.task_id.clone());
        verification.status = status;
        verification.summary = req.summary;
        verification.issues = issues;
        verification.files_reviewed = files_reviewed;
        if let Some(confidence) = req.confidence {
            verification.set_confidence(confidence);
        }
        if let Some(duration_ms) = req.duration_ms {
            verification.set_duration(duration_ms);
        }

        // Set agent ID if available
        if let Some(Some(agent_id)) = self.agent_id.get() {
            verification.set_agent(agent_id.clone());
        }

        // Set verification type if specified (default is Task)
        if let Some(vtype) = &req.verification_type {
            if vtype == "epic" {
                verification.verification_type = VerificationType::Epic;
            }
        }

        // Atomic unjail: persist verification record and clear pending_verification
        // in the same SQLite transaction. If any step fails, both roll back.
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

            let tx = conn.unchecked_transaction().map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to begin transaction: {e}")),
                data: None,
            })?;

            cas_store::add_verification_with_conn(&tx, &verification).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to add verification: {e}")),
                data: None,
            })?;

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
            }));
            // Add session ID if available for linking to the verifying agent
            let event = if let Some(Some(agent_id)) = self.agent_id.get() {
                event.with_session(agent_id)
            } else {
                event
            };
            let _ = event_store.record(&event);
        }

        let status_emoji = match verification.status {
            VerificationStatus::Approved => "✅",
            VerificationStatus::Rejected => "❌",
            VerificationStatus::Error => "⚠️",
            VerificationStatus::Skipped => "⏭️",
        };

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

        let verification = verification_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Verification not found: {e}")),
            data: None,
        })?;

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

        let verifications =
            verification_store
                .get_for_task(&req.task_id)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to list verifications: {e}")),
                    data: None,
                })?;

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
            Some(v) => {
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
