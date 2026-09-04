use crate::harness_policy::is_supervisor_from_env;
use crate::mcp::tools::core::imports::*;

impl CasCore {
    pub async fn cas_task_claim(
        &self,
        Parameters(req): Parameters<TaskClaimRequest>,
    ) -> Result<CallToolResult, McpError> {
        let agent_store = self.open_agent_store()?;
        let task_store = self.open_task_store()?;

        // Verify task exists and is claimable
        let task = task_store.get(&req.task_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        // cas-a0d2: adopt an unattributed row into this project. Unlike
        // `start`, claim has no later write of its own, so persist the
        // adoption here or the next ownership check would ask again.
        let mut task = task;
        if let Some(adopted) = super::super::task::ensure_task_origin(&task, &self.cas_root, "claim")?
        {
            task.origin_project = Some(adopted);
            task.updated_at = task_store.update(&task).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Failed to adopt unattributed task {} into this project: {e}",
                    req.task_id
                )),
                data: None,
            })?;
        }
        let task = task;

        if task.is_terminal() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from("Cannot claim a terminal task (closed or cancelled)"),
                data: None,
            });
        }

        super::super::task::ensure_no_open_blockers(task_store.as_ref(), &req.task_id, "claim")?;
        super::super::task::ensure_no_external_blockers(&self.cas_root, &req.task_id, "claim")?;

        // Get or register the agent - this ensures the agent exists in the database
        // before we try to claim a task (required due to foreign key constraint)
        let agent_id = self.get_agent_id()?;

        // Check assignee restriction - task MUST be assigned to this agent
        // Exception: supervisors can claim unassigned tasks (auto-assigns to them)
        let agent = agent_store.get(&agent_id).ok();
        let agent_name = agent
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| agent_id.clone());
        let is_supervisor = agent
            .as_ref()
            .map(|a| a.role == cas_types::AgentRole::Supervisor)
            .unwrap_or(false);
        let is_worker = agent
            .as_ref()
            .map(|a| a.role == cas_types::AgentRole::Worker)
            .unwrap_or(false);
        let prefer_start_hint = if is_worker && task.status == TaskStatus::Open {
            Some(format!(
                "\n\n⚠️ Workflow hint: prefer `cas_task action=start id={}` for normal execution.\n\
                 Use `claim` when you need manual lease control (custom duration/reason/recovery).",
                req.task_id
            ))
        } else {
            None
        };

        // Supervisors can only claim epics, not regular tasks —
        // UNLESS the task is orphaned (assignee is inactive/dead worker).
        if is_supervisor && task.task_type != crate::types::TaskType::Epic {
            let assignee_inactive = if let Some(assignee_id) = task.assignee.as_deref() {
                agent_store
                    .get(assignee_id)
                    .map(|a| !a.is_alive() || a.is_heartbeat_expired(300))
                    .unwrap_or(true) // assignee not found → treat as inactive
            } else {
                // No assignee at all → treat as orphaned
                true
            };

            if !assignee_inactive {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Supervisors cannot claim non-epic tasks. To delegate work:\n\n\
                        1. Assign to existing worker:\n\
                           mcp__cas__task action=update id=<task_id> assignee=<worker_name>\n\
                           mcp__cas__coordination action=message target=<worker_name> message=\"Task <task_id> assigned\"\n\n\
                        2. Or spawn a new worker:\n\
                           mcp__cas__coordination action=spawn_workers count=1\n\n\
                        Supervisors coordinate and review; workers execute tasks.",
                    ),
                    data: None,
                });
            }
        }

        // Auto-assign to supervisor if task is unassigned (epics or orphaned tasks)
        let mut task = task;
        if task.assignee.is_none() && is_supervisor {
            task.assignee = Some(agent_name.clone());
            let _ = task_store.update(&task);
        }

        match &task.assignee {
            None => {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Cannot claim task: no assignee set. Ask supervisor to assign this task to you first.",
                    ),
                    data: None,
                });
            }
            Some(assignee) if assignee != &agent_id && assignee != &agent_name => {
                // Allow supervisors to reclaim orphaned tasks from dead workers
                let prev_assignee = assignee.clone();
                let can_reclaim = is_supervisor
                    && agent_store
                        .get(&prev_assignee)
                        .map(|a| !a.is_alive() || a.is_heartbeat_expired(300))
                        .unwrap_or(true);

                if can_reclaim {
                    // Re-assign orphaned task to supervisor
                    task.assignee = Some(agent_name.clone());
                    task.updated_at = chrono::Utc::now();
                    let timestamp = task.updated_at.format("%Y-%m-%d %H:%M");
                    let reclaim_note = format!(
                        "[{timestamp}] Reclaimed from inactive worker '{prev_assignee}' by supervisor '{agent_name}'"
                    );
                    if task.notes.is_empty() {
                        task.notes = reclaim_note;
                    } else {
                        task.notes = format!("{}\n\n{}", task.notes, reclaim_note);
                    }
                    let _ = task_store.update(&task);

                    // Release stale lease held by dead worker
                    let _ = agent_store.release_lease(&req.task_id, &prev_assignee);
                } else {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!(
                            "Cannot claim task: assigned to '{prev_assignee}', not you ({agent_name})"
                        )),
                        data: None,
                    });
                }
            }
            _ => {} // Assigned to this agent - allow claim
        }

        let result = agent_store
            .try_claim(
                &req.task_id,
                &agent_id,
                req.duration_secs,
                req.reason.as_deref(),
            )
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to claim task: {e}")),
                data: None,
            })?;

        match result {
            ClaimResult::Success(lease) => {
                // Update task to in_progress if it's open
                if task.status == TaskStatus::Open {
                    let mut updated = task.clone();
                    updated.status = TaskStatus::InProgress;
                    let _ = task_store.update(&updated);
                }

                // Record working epic if this task belongs to one
                // Also look up the parent epic's worktree and sibling notes
                let mut worktree_info: Option<String> = None;
                let mut sibling_notes_info: Option<String> = None;
                if let Ok(deps) = task_store.get_dependencies(&req.task_id) {
                    for dep in deps {
                        if dep.dep_type == crate::types::DependencyType::ParentChild {
                            if let Ok(parent) = task_store.get(&dep.to_id) {
                                if parent.task_type == crate::types::TaskType::Epic {
                                    let _ = agent_store.add_working_epic(&agent_id, &parent.id);

                                    // Fetch sibling task notes for epic context
                                    if let Ok(sibling_notes) =
                                        task_store.get_sibling_notes(&parent.id, &req.task_id)
                                    {
                                        if !sibling_notes.is_empty() {
                                            let mut notes_output = String::from(
                                                "\n\n📋 SIBLING TASK NOTES (from other workers on this epic):",
                                            );
                                            for (task_id, title, notes) in sibling_notes {
                                                notes_output.push_str(&format!(
                                                    "\n\n**[{task_id}] {title}**\n{notes}"
                                                ));
                                            }
                                            sibling_notes_info = Some(notes_output);
                                        }
                                    }

                                    // Look up the parent epic's worktree and claim exclusive lock
                                    if let Some(ref worktree_id) = parent.worktree_id {
                                        if let Ok(wt_store) = self.open_worktree_store() {
                                            if let Ok(worktree) = wt_store.get(worktree_id) {
                                                if worktree.path.exists() {
                                                    // Try to claim the worktree lease for exclusive access
                                                    let wt_claim_result = agent_store
                                                        .try_claim_worktree(
                                                            worktree_id,
                                                            &agent_id,
                                                            req.duration_secs,
                                                        );

                                                    let wt_lock_info = match wt_claim_result {
                                                        Ok(crate::types::WorktreeClaimResult::Success(_)) => {
                                                            "🔒 Worktree locked".to_string()
                                                        }
                                                        Ok(crate::types::WorktreeClaimResult::AlreadyClaimed { held_by, expires_at, .. }) => {
                                                            if held_by == agent_id {
                                                                "🔒 Worktree locked (renewed)".to_string()
                                                            } else {
                                                                format!("⚠️ Worktree locked by {} (expires {})", held_by, expires_at.format("%H:%M:%S"))
                                                            }
                                                        }
                                                        _ => "".to_string()
                                                    };

                                                    worktree_info = Some(format!(
                                                        "\n\n🌳 This task belongs to epic [{}] {}\n   Work in directory: {}\n   Branch: {}\n   {}",
                                                        parent.id,
                                                        parent.title,
                                                        worktree.path.display(),
                                                        worktree.branch,
                                                        wt_lock_info
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                Ok(Self::success(format!(
                    "Task claimed: {}\n\
                     Agent: {}\n\
                     Expires in: {}s\n\
                     Reason: {}{}{}{}{}",
                    lease.task_id,
                    lease.agent_id,
                    lease.remaining_secs(),
                    lease.claim_reason.as_deref().unwrap_or("none"),
                    worktree_info.unwrap_or_default(),
                    sibling_notes_info.unwrap_or_default(),
                    prefer_start_hint.clone().unwrap_or_default(),
                    Self::workflow_guidance()
                )))
            }
            ClaimResult::AlreadyClaimed {
                task_id,
                held_by,
                expires_at,
            } => Ok(Self::success(format!(
                "Task {} is already claimed by {}\nExpires at: {}{}",
                task_id,
                held_by,
                expires_at.format("%Y-%m-%d %H:%M:%S"),
                prefer_start_hint.unwrap_or_default()
            ))),
            ClaimResult::TaskNotFound(id) => Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Task not found: {id}")),
                data: None,
            }),
            ClaimResult::NotClaimable { task_id, reason } => Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Task {task_id} not claimable: {reason}")),
                data: None,
            }),
            ClaimResult::Unauthorized(msg) => Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Unauthorized: {msg}")),
                data: None,
            }),
        }
    }

    /// Release a claimed task
    /// Release a task lease, with auto-recovery for dead-session orphans.
    ///
    /// ### Dead-session auto-recovery (EPIC cas-9508 / cas-1a7c)
    ///
    /// Previously, `release` returned `"No active lease found"` when the task
    /// row was `InProgress` but the lease had been garbage-collected (e.g. the
    /// worker session died without cleanly releasing). The status stayed
    /// `InProgress` forever, and callers had to manually `update status=open`
    /// to recover. This divergence between lease state and task status was a
    /// persistent source of factory-session friction.
    ///
    /// Release now detects the "no active lease + status != Closed/Open"
    /// case, transitions the task to `Open`, clears the assignee, appends an
    /// audit note to `task.notes`, and returns success. The message reflects
    /// whether a real lease was released or the task was auto-recovered so
    /// callers can act accordingly.
    pub async fn cas_task_release(
        &self,
        Parameters(req): Parameters<TaskReleaseRequest>,
    ) -> Result<CallToolResult, McpError> {
        let agent_store = self.open_agent_store()?;
        let task_store = self.open_task_store()?;

        // Get the registered agent ID (matches how claim works)
        let agent_id = self.get_agent_id()?;

        match agent_store.release_lease(&req.task_id, &agent_id) {
            Ok(()) => {
                let mut task = task_store.get(&req.task_id).map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "Released task lease, but failed to read task state: {e}"
                    )),
                    data: None,
                })?;

                // cas-8cff: a successful release used to delete only the
                // lease, leaving a normally-started task InProgress with no
                // active worker. That phantom state is excluded from the
                // ready pool and makes a later start report the nonsensical
                // InProgress → InProgress transition. Returning a started
                // task to the pool is part of release semantics, so persist
                // the matching lifecycle state and clear its assignee.
                if task.status == TaskStatus::InProgress {
                    let prior_assignee = task.assignee.clone();
                    task.status = TaskStatus::Open;
                    task.assignee = None;
                    task.updated_at = chrono::Utc::now();
                    let ts = task.updated_at.format("%Y-%m-%d %H:%M");
                    let audit = format!(
                        "[{ts}] release: active lease released; status reset from InProgress to Open (prior assignee: {})",
                        prior_assignee.as_deref().unwrap_or("<none>")
                    );
                    task.notes = if task.notes.is_empty() {
                        audit
                    } else {
                        format!("{}\n\n{}", task.notes, audit)
                    };
                    task_store.update(&task).map_err(|e| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Released task lease, but failed to reset task status to Open: {e}"
                        )),
                        data: None,
                    })?;
                    Ok(Self::success(format!(
                        "Released task: {} (status reset from InProgress to Open)",
                        req.task_id
                    )))
                } else {
                    Ok(Self::success(format!(
                        "Released task lease: {} (task status remains {})",
                        req.task_id, task.status
                    )))
                }
            }
            Err(e) => {
                // Auto-recovery path: no active lease but the task row may
                // still be in a non-open state from a dead session. Flip it
                // back to Open with an audit note rather than surfacing the
                // raw "no lease" error to the caller.
                let err_str = e.to_string();
                let is_not_found = err_str.contains("No active lease found");

                if is_not_found {
                    if let Ok(mut task) = task_store.get(&req.task_id) {
                        // cas-6e4c: PSR tasks must be excluded from orphan-recovery.
                        // The work is already complete — only supervisor review is
                        // pending. Resetting a PSR task to Open would silently erase
                        // the worker's completed close and remove it from the
                        // parked merge queue.
                        if !task.is_terminal()
                            && task.status != TaskStatus::Open
                            && task.status != TaskStatus::AwaitingMerge
                        {
                            let prior_status = task.status;
                            let prior_assignee = task.assignee.clone();
                            task.status = TaskStatus::Open;
                            task.assignee = None;
                            let ts = chrono::Utc::now().format("%Y-%m-%d %H:%M");
                            let audit = format!(
                                "[{ts}] release: lease absent; auto-recovered from {prior_status:?} (prior assignee: {}) to Open — assumed orphaned by dead session",
                                prior_assignee.as_deref().unwrap_or("<none>")
                            );
                            task.notes = if task.notes.is_empty() {
                                audit
                            } else {
                                format!("{}\n\n{}", task.notes, audit)
                            };
                            task.updated_at = chrono::Utc::now();
                            task_store.update(&task).map_err(|upd_err| McpError {
                                code: ErrorCode::INTERNAL_ERROR,
                                message: Cow::from(format!(
                                    "Release auto-recovery: failed to update task: {upd_err}"
                                )),
                                data: None,
                            })?;
                            return Ok(Self::success(format!(
                                "Released task: {} (auto-recovered from {:?} — lease was absent; status reset to Open)",
                                req.task_id, prior_status
                            )));
                        }
                    }
                }

                Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("Failed to release task: {e}")),
                    data: None,
                })
            }
        }
    }

    /// Reset a task to a clean Open state, clearing any lease + assignee.
    ///
    /// ### Dead-session recovery AND live-worker reassign verb (EPIC cas-9508 / cas-1a7c, cas-3ed5)
    ///
    /// `reset` is the atomic "return this task to a clean Open state" verb.
    /// Unlike `release`, it does not require the caller to own the lease —
    /// it force-releases any active lease on the task (dead OR live),
    /// clears the assignee, transitions the status to `Open`, and records
    /// an audit note.
    ///
    /// Two legitimate use cases:
    ///
    /// 1. **Dead-session recovery** — a worker died mid-task and left an
    ///    orphaned lease + InProgress row. `reset` unblocks the task so a
    ///    new worker can pick it up.
    ///
    /// 2. **Supervisor reassign without worker cooperation** — the target
    ///    task is claimed by a live worker, but the supervisor wants to
    ///    reassign it without shutting the worker down. Call `reset` to
    ///    drop the live lease, then `update assignee=<new-worker>` to
    ///    route it to the new worker, then message the new worker. The
    ///    prior worker's lease is released silently; alert the prior
    ///    worker separately if needed.
    ///
    ///    Prefer `transfer to_agent=<new-worker> supervisor_override=true`
    ///    when you want a single atomic step that also sets the assignee
    ///    and attempts to pre-claim for the target — `reset` is the
    ///    two-step fallback if `transfer` is unavailable.
    ///
    /// Safety: refuses to reset a `Closed` task (use `reopen` instead).
    pub async fn cas_task_reset(
        &self,
        Parameters(req): Parameters<TaskReleaseRequest>,
    ) -> Result<CallToolResult, McpError> {
        let agent_store = self.open_agent_store()?;
        let task_store = self.open_task_store()?;

        let mut task = task_store.get(&req.task_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        if task.is_terminal() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Cannot reset terminal task {} — use `action=reopen` instead",
                    req.task_id
                )),
                data: None,
            });
        }

        // cas-86c5: Alive-worker safety guard.
        //
        // If the task has an assignee whose heartbeat is within WORKER_STALE_SECS
        // (30s), the worker is likely active mid-turn — resetting would orphan
        // their in-flight work (observed incident: cas-2fb6/happy-fox-41 reset
        // during an xhigh investigation phase). Warn and abort unless the caller
        // explicitly passes `force=true`.
        //
        // An orphaned task (no assignee, or assignee with a stale/dead heartbeat)
        // is unaffected — this is the intended dead-session recovery path.
        if !req.force.unwrap_or(false) {
            if let Some(ref assignee) = task.assignee {
                // Look up the agent by name/id.  `list(None)` includes all statuses
                // so we can check the raw heartbeat even for agents marked stale.
                let all_agents = agent_store.list(None).unwrap_or_default();
                let live_agent = all_agents
                    .iter()
                    .find(|a| a.name == *assignee || a.id == *assignee);
                if let Some(agent) = live_agent {
                    use crate::mcp::tools::service::factory_ops::WORKER_STALE_SECS;
                    let elapsed = (chrono::Utc::now() - agent.last_heartbeat).num_seconds();
                    if elapsed <= WORKER_STALE_SECS {
                        return Ok(Self::success(format!(
                            "SAFETY GUARD: task {} is assigned to '{}' whose heartbeat is {}s ago \
                             (threshold: {}s) — the worker appears to be ALIVE and may be actively \
                             working this task. Resetting now would orphan their in-flight work.\n\n\
                             • To confirm the worker is safe to reset, check `worker_status` for \
                               '{}' (look for fresh heartbeat + recent activity events).\n\
                             • To reset anyway (e.g. the worker is stuck in a loop), pass \
                               `force=true`:\n\
                               mcp__cas__task action=reset id={} force=true",
                            req.task_id,
                            assignee,
                            elapsed,
                            WORKER_STALE_SECS,
                            assignee,
                            req.task_id,
                        )));
                    }
                }
            }
        }

        // Force-release any active lease on the task (no ownership check).
        // `release_lease_for_task` returns Ok(true) if a lease was released,
        // Ok(false) if none existed — either is fine here.
        let lease_released = agent_store
            .release_lease_for_task(&req.task_id, "Task reset")
            .unwrap_or(false);

        let prior_status = task.status;
        let prior_assignee = task.assignee.clone();
        task.status = TaskStatus::Open;
        task.assignee = None;
        let ts = chrono::Utc::now().format("%Y-%m-%d %H:%M");
        // cas-86c5: note whether the reset was forced (live worker override).
        let recovery_kind = if req.force.unwrap_or(false) {
            "forced reset (alive-worker guard bypassed)"
        } else {
            "dead-session recovery"
        };
        let audit = format!(
            "[{ts}] reset: force-transitioned {prior_status:?}→Open (prior assignee: {}, lease released: {lease_released}) — {recovery_kind}",
            prior_assignee.as_deref().unwrap_or("<none>")
        );
        task.notes = if task.notes.is_empty() {
            audit
        } else {
            format!("{}\n\n{}", task.notes, audit)
        };
        task.updated_at = chrono::Utc::now();

        task_store.update(&task).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("reset: failed to update task: {e}")),
            data: None,
        })?;

        let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
            &self.cas_root,
            "task_reset",
            &[
                ("task_id", &req.task_id),
                ("prior_status", &format!("{prior_status:?}")),
                ("prior_assignee", prior_assignee.as_deref().unwrap_or("")),
                (
                    "forced",
                    if req.force.unwrap_or(false) {
                        "true"
                    } else {
                        "false"
                    },
                ),
            ],
        );

        Ok(Self::success(format!(
            "Reset task: {} (was {:?}, assignee cleared, lease released: {})",
            req.task_id, prior_status, lease_released
        )))
    }

    /// List tasks available for claiming
    pub async fn cas_tasks_available(
        &self,
        Parameters(req): Parameters<TaskAvailableRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;
        let agent_store = self.open_agent_store()?;

        // Get ready tasks (open, not blocked)
        let ready_tasks = task_store.list_ready().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list tasks: {e}")),
            data: None,
        })?;

        // Get active leases to filter out claimed tasks
        let active_leases = agent_store.list_active_leases().unwrap_or_default();
        let claimed_ids: std::collections::HashSet<_> =
            active_leases.iter().map(|l| l.task_id.as_str()).collect();

        // Filter to unclaimed tasks. Legacy rows with no origin remain visible;
        // explicitly foreign rows require the opt-in flag.
        let project_id = super::super::task::current_project_id(&self.cas_root);
        let hidden_foreign = if req.include_foreign {
            0
        } else {
            ready_tasks
                .iter()
                .filter(|task| {
                    !super::super::task::task_visible_in_project(task, project_id.as_deref())
                })
                .count()
        };
        let mut available: Vec<_> = ready_tasks
            .iter()
            .filter(|t| {
                req.include_foreign
                    || super::super::task::task_visible_in_project(t, project_id.as_deref())
            })
            .filter(|t| !claimed_ids.contains(t.id.as_str()))
            .collect();

        // cas-61d3 (GH #111): `sort`/`sort_order` are accepted by this action's
        // schema and were routed all the way into the handler, which then
        // ignored them — an agent asking for a different order got the default
        // one back with no error and no way to tell. Honour them with the same
        // semantics `ready`/`blocked` use (cas-06f9 / GH #104): unspecified
        // means priority, and an unrecognised field means unspecified rather
        // than silently falling back to creation order.
        let sort_opts = crate::mcp::tools::ready_blocked_sort_options(
            req.sort.as_deref(),
            req.sort_order.as_deref(),
        );
        crate::mcp::tools::sort_task_refs(&mut available, &sort_opts);

        if available.is_empty() {
            let mut output = "No available tasks (all claimed or none ready)".to_string();
            if let Some(footer) = super::super::task::foreign_tasks_hidden_footer(hidden_foreign) {
                output.push('\n');
                output.push_str(&footer);
            }
            return Ok(Self::success(output));
        }

        // cas-e163 (GH #109): the total here was already honest, but nothing
        // said the list had been cut — so a worker scanning for claimable work
        // could read 20 rows, see a larger total it had no reason to connect to
        // its own view, and never learn which call shows the rest. Same footer
        // as `ready`/`blocked` (cas-06f9 / GH #104), same surface behaviour.
        // The tail withheld here is the tail of whatever ordering was applied
        // just above — lower-priority work by default, but the alphabetical
        // tail under sort=title. The footer makes no ordering claim; the
        // header names the order so the two cannot disagree.
        let limit = req.limit.unwrap_or(20);
        let total = available.len();
        let shown = total.min(limit);
        // State the ordering, so honouring `sort` is observable to the caller
        // rather than something they have to infer from the rows.
        let order = crate::mcp::tools::sort_order_label(&sort_opts);
        let mut output = format!("Available Tasks ({total} total, {order}):\n\n");

        for task in available.iter().take(limit) {
            output.push_str(&format!(
                "[P{}] {} - {}\n",
                task.priority.0, task.id, task.title
            ));
        }
        output.push_str(&crate::mcp::tools::truncated_list_footer(total, shown));
        if let Some(footer) = super::super::task::foreign_tasks_hidden_footer(hidden_foreign) {
            output.push('\n');
            output.push_str(&footer);
        }

        Ok(Self::success(output))
    }

    /// Transfer a task to another agent.
    ///
    /// ### Supervisor force-transfer (cas-3ed5)
    ///
    /// Normally the caller must own the task's active lease to transfer it.
    /// When `supervisor_override=true` and the caller is a supervisor
    /// (`CAS_AGENT_ROLE=supervisor`), the handler force-releases the current
    /// lease (regardless of who holds it) and reassigns the task to the target
    /// agent. An audit-log entry is appended to `task.notes` identifying the
    /// supervisor session ID and the prior lease holder so the override is
    /// always traceable.
    ///
    /// Non-supervisor callers that set `supervisor_override=true` receive an
    /// explicit rejection; the flag is not silently ignored.
    pub async fn cas_task_transfer(
        &self,
        Parameters(req): Parameters<TaskTransferRequest>,
    ) -> Result<CallToolResult, McpError> {
        let agent_store = self.open_agent_store()?;
        let task_store = self.open_task_store()?;

        // Use registered agent ID (fail if not registered)
        let agent_id = self
            .agent_id
            .get()
            .and_then(|o| o.clone())
            .ok_or_else(|| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from("No agent registered - SessionStart hook may not have run"),
                data: None,
            })?;

        let supervisor_override_requested = req.supervisor_override.unwrap_or(false);

        // Verify current agent owns the lease — with supervisor force-transfer escape hatch.
        let lease = agent_store.get_lease(&req.task_id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to get lease: {e}")),
            data: None,
        })?;

        // prior_lease_holder is Some(<agent_id>) when we force-released a live lease.
        // Used below for the audit note.
        let prior_lease_holder: Option<String> = match &lease {
            Some(l) if l.agent_id == agent_id && l.status == LeaseStatus::Active => {
                // Caller owns the lease — normal transfer path.
                None
            }
            Some(l) if l.agent_id != agent_id => {
                // Lease is held by a different agent.
                // Resolve UUID → friendly name so the supervisor can identify
                // the holding worker without cross-referencing worker_status.
                let holder_display = agent_store
                    .get(&l.agent_id)
                    .map(|a| format!("{} ({})", a.name, l.agent_id))
                    .unwrap_or_else(|_| l.agent_id.clone());
                if supervisor_override_requested {
                    if !is_supervisor_from_env() {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(
                                "supervisor_override=true is only honored when the caller is a \
                                 supervisor (CAS_AGENT_ROLE=supervisor). Non-supervisor callers \
                                 cannot force-transfer a task owned by another agent.",
                            ),
                            data: None,
                        });
                    }
                    // Supervisor force-transfer: release the live worker's lease.
                    let holder = l.agent_id.clone();
                    agent_store
                        .release_lease_for_task(&req.task_id, "Supervisor force-transfer")
                        .map_err(|e| McpError {
                            code: ErrorCode::INTERNAL_ERROR,
                            message: Cow::from(format!(
                                "Supervisor force-transfer: failed to release live lease: {e}"
                            )),
                            data: None,
                        })?;
                    Some(holder)
                } else {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!(
                            "Task {} is owned by {}, not {}. \
                             Supervisors can force-transfer with supervisor_override=true \
                             (bypasses the lease check and logs an audit entry).",
                            req.task_id, holder_display, agent_id
                        )),
                        data: None,
                    });
                }
            }
            _ => {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("No active lease found for task {}", req.task_id)),
                    data: None,
                });
            }
        };

        // Verify target agent exists and is active
        let target_agent = agent_store.get(&req.to_agent).map_err(|_| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Target agent not found: {}", req.to_agent)),
            data: None,
        })?;

        if !target_agent.is_alive() {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "Target agent {} is not active (status: {})",
                    req.to_agent, target_agent.status
                )),
                data: None,
            });
        }

        // Add handoff note (plus supervisor-override audit entry when applicable) to task
        let mut task = task_store.get(&req.task_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
        let handoff_note = if let Some(prior_holder) = &prior_lease_holder {
            // Supervisor force-transfer: include audit information.
            let note_suffix = req
                .note
                .as_deref()
                .map(|n| format!(": {n}"))
                .unwrap_or_default();
            format!(
                "[{timestamp}] SUPERVISOR FORCE-TRANSFER by {agent_id}: \
                 released live lease from '{prior_holder}', reassigned to '{}'{}",
                req.to_agent, note_suffix
            )
        } else if let Some(note) = &req.note {
            format!(
                "[{timestamp}] Handoff from {agent_id} to {}: {note}",
                req.to_agent
            )
        } else {
            format!("[{timestamp}] Handoff from {agent_id} to {}", req.to_agent)
        };

        if task.notes.is_empty() {
            task.notes = handoff_note;
        } else {
            task.notes = format!("{}\n\n{}", task.notes, handoff_note);
        }
        // Update assignee to the target agent
        task.assignee = Some(req.to_agent.clone());
        task.updated_at = chrono::Utc::now();

        task_store.update(&task).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update task: {e}")),
            data: None,
        })?;

        // Release our lease (only needed for the normal transfer path; the
        // supervisor force-transfer path already released the live lease above).
        if prior_lease_holder.is_none() {
            agent_store
                .release_lease(&req.task_id, &agent_id)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to release lease: {e}")),
                    data: None,
                })?;
        }

        // Try to claim for target agent (best effort - they may need to claim themselves)
        let claim_result = agent_store.try_claim(
            &req.task_id,
            &req.to_agent,
            DEFAULT_LEASE_DURATION_SECS,
            Some(&format!("Transferred from {agent_id}")),
        );

        let claim_msg = match claim_result {
            Ok(ClaimResult::Success(_)) => {
                format!(
                    "Task claimed for {} - they can start immediately",
                    req.to_agent
                )
            }
            _ => format!(
                "Task released - {} will need to claim it manually",
                req.to_agent
            ),
        };

        let override_note = if prior_lease_holder.is_some() {
            "\n⚠️ Supervisor force-transfer used — audit entry appended to task notes."
        } else {
            ""
        };

        Ok(Self::success(format!(
            "Transferred task {} from {} to {}\n{}\nNote: {}{}",
            req.task_id,
            agent_id,
            req.to_agent,
            claim_msg,
            req.note.as_deref().unwrap_or("(none)"),
            override_note
        )))
    }

    /// List tasks claimed by this agent
    ///
    /// ### Spawn-time race resilience (EPIC cas-9508 / cas-5572)
    ///
    /// Workers are assigned by supervisors via `task update assignee=<worker-name>`.
    /// At the moment a fresh worker makes its first `action=mine` call, the agent
    /// store row for that worker may not yet reflect the final worker name
    /// (agent_name could still be a stale/default value, or the row lookup
    /// could race eager-registration) — and the task's `assignee` column will
    /// hold the worker-name string set by the supervisor.
    ///
    /// To make the first poll reliably return freshly-assigned tasks without
    /// requiring a coordination-message kick, this handler matches assignees
    /// against a **set** of known identifiers for the current agent:
    ///
    /// - the canonical `agent_id` (session UUID),
    /// - the `agent_name` from the agent_store row, if any,
    /// - the `CAS_AGENT_NAME` env var (set by the factory PTY spawner), and
    /// - the `CAS_SESSION_ID` env var (belt-and-suspenders for the UUID).
    ///
    /// Matching is case-insensitive on trimmed values to tolerate minor
    /// spelling drift between supervisor and worker views.
    pub async fn cas_tasks_mine(
        &self,
        Parameters(req): Parameters<LimitRequest>,
    ) -> Result<CallToolResult, McpError> {
        let agent_store = self.open_agent_store()?;
        let task_store = self.open_task_store()?;

        // Use the current Cassy agent identity (session-based in factory/Codex mode).
        let agent_id = self.get_agent_id()?;
        let agent_name = agent_store
            .get(&agent_id)
            .map(|a| a.name)
            .unwrap_or_else(|_| agent_id.clone());

        // Collect all identifiers that may have been used as the `assignee`
        // value for this agent. Factory workers are frequently assigned by
        // their friendly worker-name before the agent_store row is fully
        // populated, so we include the env-provided name/session as well.
        // See doc-comment above for the spawn-time race this guards against.
        let mut identities: Vec<String> = Vec::with_capacity(4);
        let mut push_identity = |s: &str| {
            let t = s.trim();
            if !t.is_empty() && !identities.iter().any(|i| i.eq_ignore_ascii_case(t)) {
                identities.push(t.to_string());
            }
        };
        push_identity(&agent_id);
        push_identity(&agent_name);
        if let Ok(env_name) = std::env::var("CAS_AGENT_NAME") {
            push_identity(&env_name);
        }
        if let Ok(env_session) = std::env::var("CAS_SESSION_ID") {
            push_identity(&env_session);
        }

        let leases = agent_store.list_agent_leases(&agent_id).unwrap_or_default();
        let project_id = super::super::task::current_project_id(&self.cas_root);
        let mut assigned: Vec<Task> = task_store
            .list(None)
            .unwrap_or_default()
            .into_iter()
            // cas-a0d2: match the board (`ready`/`list`/`blocked`) — an
            // unattributed row is this project's work until some other project
            // claims it, and hiding it from `mine` left an assigned worker
            // with nothing to act on.
            .filter(|t| super::super::task::task_visible_in_project(t, project_id.as_deref()))
            .filter(|t| {
                if t.is_terminal() {
                    return false;
                }
                match t.assignee.as_deref() {
                    Some(a) => {
                        let a_trim = a.trim();
                        identities.iter().any(|id| id.eq_ignore_ascii_case(a_trim))
                    }
                    None => false,
                }
            })
            .collect();

        // Show highest-priority tasks first.
        assigned.sort_by(|a, b| a.priority.0.cmp(&b.priority.0));
        let limit = req.limit.unwrap_or(20);

        if assigned.is_empty() {
            return Ok(Self::success(format!(
                "No open tasks assigned to this agent ({agent_name})"
            )));
        }

        let mut output = format!(
            "My Tasks ({} assigned, {} claimed):\n\n",
            assigned.len(),
            leases.len()
        );
        for task in assigned.iter().take(limit) {
            let lease_info = leases
                .iter()
                .find(|l| l.task_id == task.id)
                .map(|l| {
                    format!(
                        "\n  Lease: active (expires in {}s, renewals: {})",
                        l.remaining_secs(),
                        l.renewal_count
                    )
                })
                .unwrap_or_else(|| "\n  Lease: not claimed yet".to_string());
            output.push_str(&format!(
                "- [{}] {:?} P{} {} ({}) - {}{}{}\n",
                task.id,
                task.status,
                task.priority.0,
                task.task_type,
                task.depth,
                task.title,
                lease_info,
                if task.assignee.as_deref() == Some(agent_id.as_str()) {
                    " (assignee=agent_id)"
                } else if task
                    .assignee
                    .as_deref()
                    .map(|a| !a.eq_ignore_ascii_case(agent_name.as_str()))
                    .unwrap_or(false)
                {
                    // Matched via a fallback identity (e.g. CAS_AGENT_NAME
                    // env) — useful signal during the spawn-time race window.
                    " (assignee=alias)"
                } else {
                    ""
                }
            ));
        }

        if assigned.len() > limit {
            output.push_str(&format!(
                "\n... and {} more assigned task(s)",
                assigned.len() - limit
            ));
        }

        Ok(Self::success(output))
    }
}
