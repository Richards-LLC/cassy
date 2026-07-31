use crate::mcp::tools::core::imports::*;

/// Heartbeat staleness window for epic_verification_owner transfer targets
/// (cas-cc74). Aligned with claim/close assignee liveness (~5 min).
const EPIC_OWNER_TARGET_STALE_SECS: i64 = 300;

/// Normalize `epic_verification_owner` at write boundaries (cas-cc74 discovery).
/// Trims whitespace; empty/whitespace-only becomes `None` (unset / invalid).
pub(crate) fn normalize_epic_verification_owner(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Normalize `assignee` on task update (cas-bf98).
///
/// - Empty / whitespace-only → `None` (explicit **clear / unassign**).
/// - Non-empty → trimmed string for storage and factory session-id normalization.
///
/// Must run **before** session-id → display-name remapping: `Some("")` is still
/// `Some`, so without this guard factory mode can treat empty as a session id
/// and rewrite it to a live worker name (observed: remapped to `hv-scope`).
pub(crate) fn normalize_assignee_update_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// True when any caller identity facet matches the stored owner (after trim).
pub(crate) fn caller_matches_epic_owner(
    owner: &str,
    caller_id: Option<&str>,
    caller_name: Option<&str>,
    caller_session: Option<&str>,
) -> bool {
    let owner = owner.trim();
    if owner.is_empty() {
        return false;
    }
    [caller_id, caller_name, caller_session]
        .into_iter()
        .flatten()
        .any(|id| id.trim() == owner)
}

/// Authorize the *caller* for an `epic_verification_owner` mutation (cas-cc74).
///
/// - Unknown / absent caller identity → fail closed.
/// - Current owner (id/name/session match) may transfer.
/// - Supervisor (`caller_is_supervisor`) may transfer or set when unset.
/// - Non-owner non-supervisor cannot rewrite or claim ownership.
pub(crate) fn authorize_epic_owner_transfer_caller(
    current_owner: Option<&str>,
    caller_id: Option<&str>,
    caller_name: Option<&str>,
    caller_session: Option<&str>,
    caller_is_supervisor: bool,
) -> Result<(), String> {
    let has_identity = [caller_id, caller_name, caller_session]
        .into_iter()
        .flatten()
        .any(|s| !s.trim().is_empty());
    if !has_identity {
        return Err(
            "epic_verification_owner transfer refused: caller identity is unknown \
             (fail closed, cas-cc74). Present CAS agent id / CAS_AGENT_NAME / \
             CAS_SESSION_ID, or act as the current owner / a supervisor."
                .to_string(),
        );
    }

    let current = current_owner.map(str::trim).filter(|s| !s.is_empty());

    if let Some(owner) = current {
        if caller_matches_epic_owner(owner, caller_id, caller_name, caller_session) {
            return Ok(());
        }
        if caller_is_supervisor {
            return Ok(());
        }
        return Err(format!(
            "epic_verification_owner transfer refused: epic is owned by '{owner}'; \
             this session is not the owner and is not a supervisor (cas-cc74). \
             Only the current owner or a supervisor may reassign ownership."
        ));
    }

    // Unset owner: only a supervisor may claim/set (no silent worker takeover).
    if caller_is_supervisor {
        return Ok(());
    }
    Err(
        "epic_verification_owner transfer refused: no current owner is set and \
         the caller is not a supervisor — refusing silent claim (cas-cc74)."
            .to_string(),
    )
}

/// Role eligible as `epic_verification_owner` transfer target (cas-cc74).
pub(crate) fn is_valid_epic_owner_role(role: cas_types::AgentRole) -> bool {
    matches!(
        role,
        cas_types::AgentRole::Supervisor | cas_types::AgentRole::Director
    )
}

/// Validate a resolved agent as a live epic-verification owner target (cas-cc74).
///
/// Fail closed: wrong role, dead/stale status, or expired heartbeat.
pub(crate) fn validate_epic_owner_target_agent(
    agent: &cas_types::Agent,
    stale_secs: i64,
) -> Result<(), String> {
    if !is_valid_epic_owner_role(agent.role) {
        return Err(format!(
            "epic_verification_owner transfer refused: target '{}' (role={}) is \
             not a supervisor/director identity (cas-cc74).",
            agent.name, agent.role
        ));
    }
    if !agent.is_alive() {
        return Err(format!(
            "epic_verification_owner transfer refused: target '{}' is not live \
             (status={}, cas-cc74).",
            agent.name, agent.status
        ));
    }
    if agent.is_heartbeat_expired(stale_secs) {
        return Err(format!(
            "epic_verification_owner transfer refused: target '{}' heartbeat is \
             stale (>{stale_secs}s, cas-cc74).",
            agent.name
        ));
    }
    Ok(())
}

/// Find a registered agent by id (exact, case-insensitive) or display name
/// (case-insensitive). Used to resolve transfer targets.
pub(crate) fn find_agent_for_epic_owner<'a>(
    agents: &'a [cas_types::Agent],
    requested: &str,
) -> Option<&'a cas_types::Agent> {
    let requested = requested.trim();
    if requested.is_empty() {
        return None;
    }
    agents
        .iter()
        .find(|a| a.id.eq_ignore_ascii_case(requested))
        .or_else(|| {
            agents
                .iter()
                .find(|a| a.name.eq_ignore_ascii_case(requested))
        })
}

/// Build the DECISION audit note for a successful ownership transfer.
pub(crate) fn epic_owner_transfer_audit_note(
    previous: Option<&str>,
    new_owner: &str,
    by_caller: &str,
) -> String {
    let prev = previous
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("<unset>");
    format!(
        "[{}] DECISION: epic_verification_owner transferred {} → {} by {} (cas-cc74)",
        chrono::Utc::now().format("%Y-%m-%d %H:%M"),
        prev,
        new_owner,
        by_caller
    )
}

/// Non-empty branch string from a task, if present.
fn task_branch_if_set(task: &Task) -> Option<String> {
    task.branch
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Session focus epic id: `pinned_epic_id` first, then session default `epic_id`.
/// Used only as a secondary fallback when the assigned task has no parent epic branch
/// (cas-44e9). Does not invent a branch from concurrent `epic/*` listings.
fn session_focus_epic_id() -> Option<String> {
    let session = std::env::var("CAS_FACTORY_SESSION").ok()?;
    if session.trim().is_empty() {
        return None;
    }
    let data = std::fs::read_to_string(crate::ui::factory::metadata_path(&session)).ok()?;
    let meta: crate::ui::factory::SessionMetadata = serde_json::from_str(&data).ok()?;
    meta.pinned_epic_id
        .filter(|s| !s.trim().is_empty())
        .or_else(|| meta.epic_id.filter(|s| !s.trim().is_empty()))
}

/// Resolve the branch the assignment freshness gate should compare against (cas-44e9).
///
/// Order:
/// 1. If the task itself is an epic with `branch` set → that branch
/// 2. Parent epic (ParentChild) → `epic.branch`
/// 3. Session `focus_epic` pin / session default epic → its `branch`
/// 4. `None` → caller falls through to base/main inside `check_worktree_staleness`
///
/// Never returns a branch derived from "most recent / last listed `epic/*`".
pub(crate) fn resolve_assignment_freshness_branch(
    task_store: &dyn cas_store::TaskStore,
    task: &Task,
) -> Option<String> {
    if task.task_type == TaskType::Epic {
        if let Some(branch) = task_branch_if_set(task) {
            return Some(branch);
        }
    }

    match task_store.get_parent_epic(&task.id) {
        Ok(Some(epic)) => {
            if let Some(branch) = task_branch_if_set(&epic) {
                return Some(branch);
            }
        }
        Ok(None) => {}
        Err(_) => {}
    }

    let focus_id = session_focus_epic_id()?;
    let epic = task_store.get(&focus_id).ok()?;
    if epic.task_type != TaskType::Epic {
        return None;
    }
    task_branch_if_set(&epic)
}

impl CasCore {
    pub async fn cas_task_update(
        &self,
        Parameters(req): Parameters<TaskUpdateRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.cas_task_update_with_target(req, None, None).await
    }

    pub(crate) async fn cas_task_update_with_target(
        &self,
        req: TaskUpdateRequest,
        target_repo: Option<&str>,
        target_branch: Option<&str>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let mut task = task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;
        if let Err(message) = super::lifecycle::proof_scope::guard_task_proof_scope(
            &self.cas_root,
            &task,
            super::lifecycle::proof_scope::ProofScopeOperation::TaskUpdate {
                request: &req,
                target_repo_supplied: target_repo.is_some(),
                target_branch_supplied: target_branch.is_some(),
            },
        ) {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(message),
                data: None,
            });
        }
        let existing_repo_context = if target_repo.is_none() {
            match task.deliverables.work_target.as_ref() {
                Some(target) => Some(
                    super::repo_context::resolve_repo_context(&self.cas_root, target).map_err(
                        |message| McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(message),
                            data: None,
                        },
                    )?,
                ),
                None => None,
            }
        } else {
            None
        };
        let prior_assignee = task.assignee.clone();

        let mut changes = Vec::new();

        if let Some(title) = req.title {
            task.title = title;
            changes.push("title");
        }

        if let Some(notes) = req.notes {
            let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
            let note_with_timestamp = format!("[{timestamp}] {notes}");
            if task.notes.is_empty() {
                task.notes = note_with_timestamp;
            } else {
                task.notes = format!("{}\n\n{}", task.notes, note_with_timestamp);
            }
            changes.push("notes");
        }

        if let Some(priority) = req.priority {
            task.priority = Priority(priority.min(4) as i32);
            changes.push("priority");
        }

        if let Some(labels) = req.labels {
            for label in labels
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                if !task.labels.contains(&label.to_string()) {
                    task.labels.push(label.to_string());
                }
            }
            changes.push("labels");
        }

        if let Some(description) = req.description {
            task.description = description;
            changes.push("description");
        }

        if let Some(design) = req.design {
            task.design = design;
            changes.push("design");
        }

        if let Some(acceptance_criteria) = req.acceptance_criteria {
            task.acceptance_criteria = acceptance_criteria;
            changes.push("acceptance_criteria");
        }

        if let Some(demo_statement) = req.demo_statement {
            task.demo_statement = demo_statement;
            changes.push("demo_statement");
        }

        if let Some(raw) = req.execution_note.as_deref() {
            let validated =
                crate::mcp::tools::types::validate_execution_note(Some(raw)).map_err(|msg| {
                    McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(msg),
                        data: None,
                    }
                })?;
            task.execution_note = validated;
            changes.push("execution_note");
        }

        if let Some(external_ref) = req.external_ref {
            task.external_ref = Some(external_ref);
            changes.push("external_ref");
        }

        // Empty/absent depth is a no-op; an invalid value is rejected.
        if let Some(depth) = crate::mcp::tools::types::validate_depth(req.depth.as_deref())
            .map_err(|msg| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(msg),
                data: None,
            })?
        {
            task.depth = depth;
            changes.push("depth");
        }

        if target_repo.is_some() || target_branch.is_some() {
            if let Some(repo) = target_repo {
                task.deliverables.work_target = super::repo_context::declare_work_target(
                    &self.cas_root,
                    Some(repo),
                    target_branch,
                )
                .map_err(|message| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(message),
                    data: None,
                })?;
            } else if let Some(branch) = target_branch {
                let context = existing_repo_context.as_ref().ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "WORK TARGET REJECTED: target_branch requires an existing target_repo binding",
                    ),
                    data: None,
                })?;
                let branch =
                    super::repo_context::validate_target_branch(&context.repo_root, branch)
                        .map_err(|message| McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(message),
                            data: None,
                        })?;
                let target = task.deliverables.work_target.as_mut().ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "WORK TARGET REJECTED: target_branch requires an existing target_repo binding",
                    ),
                    data: None,
                })?;
                target.target_branch = branch;
            }
            changes.push("work_target");
        }

        // Track warnings to include in response
        let mut warnings: Vec<String> = Vec::new();

        if let Some(ref assignee_raw) = req.assignee {
            // cas-bf98: empty/whitespace = explicit clear (unassign). Must run
            // before factory session-id normalization — `Some("")` is still Some
            // and was previously stored as empty or remapped to a live worker.
            match normalize_assignee_update_value(assignee_raw) {
                None => {
                    task.assignee = None;
                    changes.push("assignee");
                }
                Some(assignee) => {
                    // In factory mode: normalize and validate the assignee value.
                    //
                    // cas-dbbb: `task mine` matches assignees against agent_name (the
                    // display name) and CAS_AGENT_NAME env var. Assigning by session UUID /
                    // agent ID is silently accepted but the task never appears in the target
                    // worker's `task mine` list — observed in the 2026-06-30 smoke test
                    // where director-provided session IDs left tasks in Ready/Open with no
                    // visible assignee, while worker names dispatched correctly.
                    //
                    // Resolution order:
                    //   1. Assignee matches agent.name → canonical, store as-is.
                    //   2. Assignee matches agent.id  → normalize to agent.name, warn.
                    //   3. No match                   → warn; store as-is (agent may not yet
                    //                                   be registered, e.g. pre-spawn).
                    let mut canonical_assignee = assignee.clone();
                    if std::env::var("CAS_FACTORY_MODE").is_ok() {
                        let config = self.load_config();
                        let factory_config = config.factory();
                        // cas-44e9: scope freshness to this task's parent epic (or focus pin),
                        // never an unrelated concurrent epic branch.
                        let preferred_sync =
                            resolve_assignment_freshness_branch(task_store.as_ref(), &task);

                        if let Ok(agent_store) = self.open_agent_store() {
                            if let Ok(agents) = agent_store.list(None) {
                                // Find by name first (canonical path).
                                // cas-dbbb P2: use case-insensitive compare to match
                                // `task mine`'s own identity matching logic.
                                let by_name = agents
                                    .iter()
                                    .find(|a| a.name.eq_ignore_ascii_case(&assignee));
                                // Fall back to ID lookup (supervisor may have copy-pasted
                                // session UUID).
                                let by_id = if by_name.is_none() {
                                    agents.iter().find(|a| a.id.eq_ignore_ascii_case(&assignee))
                                } else {
                                    None
                                };

                                if let Some(worker) = by_name {
                                    // Canonical — no normalization needed.
                                    // Worktree staleness check.
                                    if factory_config.warn_stale_assignment
                                        || factory_config.block_stale_assignment
                                    {
                                        if let Some(clone_path) = worker.metadata.get("clone_path")
                                        {
                                            if let Some((behind_count, branch)) =
                                                check_worktree_staleness(
                                                    clone_path,
                                                    preferred_sync.as_deref(),
                                                )
                                            {
                                                if behind_count > 0 {
                                                    let warning_msg = format!(
                                                        "⚠️ Worker '{assignee}' is {behind_count} \
                                                         commit(s) behind {branch}. Consider \
                                                         syncing first."
                                                    );
                                                    if factory_config.block_stale_assignment
                                                        && behind_count
                                                            >= factory_config
                                                                .stale_threshold_commits
                                                    {
                                                        return Err(McpError {
                                                            code: ErrorCode::INVALID_PARAMS,
                                                            message: Cow::from(format!(
                                                                "Cannot assign to worker '{}': {} \
                                                                 commits behind {} (threshold: \
                                                                 {}). Ask the worker to rebase: \
                                                                 `git rebase {}`",
                                                                assignee,
                                                                behind_count,
                                                                branch,
                                                                factory_config
                                                                    .stale_threshold_commits,
                                                                branch
                                                            )),
                                                            data: None,
                                                        });
                                                    }
                                                    warnings.push(warning_msg);
                                                }
                                            }
                                        }
                                    }
                                } else if let Some(worker) = by_id {
                                    // Assignee is a session UUID — normalize to display name
                                    // so `task mine` can find it.
                                    let worker_name = worker.name.clone();
                                    warnings.push(format!(
                                        "⚠️ Assignee '{assignee}' is a session ID. \
                                         Normalized to display name '{worker_name}' so \
                                         `task mine` can find this task. \
                                         Use '{worker_name}' directly to avoid this warning."
                                    ));
                                    canonical_assignee = worker_name.clone();
                                    // cas-dbbb P1: run the same staleness check as the by_name
                                    // branch. The original code skipped this after
                                    // normalization, leaving stale-worktree blocking disabled
                                    // for UUID assignees.
                                    if factory_config.warn_stale_assignment
                                        || factory_config.block_stale_assignment
                                    {
                                        if let Some(clone_path) = worker.metadata.get("clone_path")
                                        {
                                            if let Some((behind_count, branch)) =
                                                check_worktree_staleness(
                                                    clone_path,
                                                    preferred_sync.as_deref(),
                                                )
                                            {
                                                if behind_count > 0 {
                                                    let staleness_msg = format!(
                                                        "⚠️ Worker '{worker_name}' is \
                                                         {behind_count} commit(s) behind \
                                                         {branch}. Consider syncing first."
                                                    );
                                                    if factory_config.block_stale_assignment
                                                        && behind_count
                                                            >= factory_config
                                                                .stale_threshold_commits
                                                    {
                                                        return Err(McpError {
                                                            code: ErrorCode::INVALID_PARAMS,
                                                            message: Cow::from(format!(
                                                                "Cannot assign to worker \
                                                                 '{worker_name}': {behind_count} \
                                                                 commits behind {branch} \
                                                                 (threshold: {}). Ask the worker \
                                                                 to rebase: `git rebase {branch}`",
                                                                factory_config
                                                                    .stale_threshold_commits,
                                                            )),
                                                            data: None,
                                                        });
                                                    }
                                                    warnings.push(staleness_msg);
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    // No agent found by name or ID — warn but allow (agent
                                    // may not yet be registered, e.g. spawning in progress).
                                    warnings.push(format!(
                                        "⚠️ No registered factory agent found for assignee \
                                         '{assignee}'. Verify the worker name with \
                                         `mcp__cas__system action=worker_status`. Use the \
                                         worker's display name (e.g. 'codex-jester'), not \
                                         their session UUID, for reliable `task mine` dispatch."
                                    ));
                                }
                            }
                        }
                    }

                    task.assignee = Some(canonical_assignee);
                    changes.push("assignee");
                }
            }
        }

        // cas-cc74: epic_verification_owner is an authorized transfer, not a
        // free-form field. Unauthorized callers must not silently take over
        // owner-routed notifications / epic-close authorization.
        if let Some(owner_raw) = req.epic_verification_owner {
            let new_owner = match normalize_epic_verification_owner(&owner_raw) {
                Some(o) => o,
                None => {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(
                            "epic_verification_owner transfer refused: target identity \
                             is empty/unknown after normalize (fail closed, cas-cc74). \
                             Pass a registered supervisor agent id or display name.",
                        ),
                        data: None,
                    });
                }
            };

            let current_norm = task
                .epic_verification_owner
                .as_deref()
                .and_then(normalize_epic_verification_owner);

            // Idempotent re-set of the same owner (after trim) is a no-op.
            if current_norm.as_deref() != Some(new_owner.as_str()) {
                let caller_id = self.get_agent_id().ok();
                let caller_name = std::env::var("CAS_AGENT_NAME").ok();
                let caller_session = std::env::var("CAS_SESSION_ID").ok();
                let caller_is_supervisor = crate::harness_policy::is_supervisor_from_env();

                if let Err(msg) = authorize_epic_owner_transfer_caller(
                    current_norm.as_deref(),
                    caller_id.as_deref(),
                    caller_name.as_deref(),
                    caller_session.as_deref(),
                    caller_is_supervisor,
                ) {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(msg),
                        data: None,
                    });
                }

                // Target must resolve to a live supervisor/director identity.
                let agent_store = self.open_agent_store().map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "epic_verification_owner transfer refused: cannot open agent \
                         store to validate target (fail closed, cas-cc74): {e}"
                    )),
                    data: None,
                })?;
                let agents = agent_store.list(None).map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "epic_verification_owner transfer refused: cannot list agents \
                         to validate target (fail closed, cas-cc74): {e}"
                    )),
                    data: None,
                })?;
                let target = match find_agent_for_epic_owner(&agents, &new_owner) {
                    Some(a) => a,
                    None => {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(format!(
                                "epic_verification_owner transfer refused: target \
                                 '{new_owner}' is not a registered agent identity \
                                 (fail closed, cas-cc74)."
                            )),
                            data: None,
                        });
                    }
                };
                if let Err(msg) =
                    validate_epic_owner_target_agent(target, EPIC_OWNER_TARGET_STALE_SECS)
                {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(msg),
                        data: None,
                    });
                }

                // Prefer agent id (stable) — matches factory create preference.
                let canonical_owner = target.id.clone();
                let by_caller = caller_id
                    .as_deref()
                    .or(caller_name.as_deref())
                    .or(caller_session.as_deref())
                    .unwrap_or("<unknown>");
                let audit = epic_owner_transfer_audit_note(
                    current_norm.as_deref(),
                    &canonical_owner,
                    by_caller,
                );
                if task.notes.is_empty() {
                    task.notes = audit;
                } else {
                    task.notes = format!("{}\n\n{}", task.notes, audit);
                }

                task.epic_verification_owner = Some(canonical_owner);
                changes.push("epic_verification_owner");
            }
        }

        // cas-062d: track status transition for supervisor lifecycle push.
        let mut lifecycle_status_change: Option<(TaskStatus, TaskStatus)> = None;
        if let Some(status_str) = req.status {
            use std::str::FromStr;
            let new_status =
                cas_types::TaskStatus::from_str(&status_str).map_err(|_| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Invalid status: {status_str}. Valid: open, in_progress, closed, blocked, pending_supervisor_review, awaiting_merge"
                    )),
                    data: None,
                })?;
            if new_status != task.status {
                lifecycle_status_change = Some((task.status, new_status));
            }
            if new_status == TaskStatus::Closed
                && task.status != TaskStatus::Closed
                && self.load_config().verification_enabled()
                && task.depth != cas_types::TaskDepth::Light
                && crate::harness_policy::verification_required_for_task_type(task.task_type)
            {
                let dispatch =
                    cas_store::get_latest_verification_dispatch(&self.cas_root, &task.id).map_err(
                        |error| McpError {
                            code: ErrorCode::INTERNAL_ERROR,
                            message: Cow::from(format!(
                                "Failed to read exact verification dispatch: {error}"
                            )),
                            data: None,
                        },
                    )?;
                let verification = match dispatch.as_ref() {
                    Some(dispatch)
                        if dispatch.state == cas_types::VerificationDispatchState::Resolved =>
                    {
                        cas_store::get_verification_for_dispatch(&self.cas_root, &dispatch.id)
                            .map_err(|error| McpError {
                                code: ErrorCode::INTERNAL_ERROR,
                                message: Cow::from(format!(
                                    "Failed to read exact dispatch verdict: {error}"
                                )),
                                data: None,
                            })?
                    }
                    None => self
                        .open_verification_store()?
                        .get_latest_for_task(&task.id)
                        .map_err(|error| McpError {
                            code: ErrorCode::INTERNAL_ERROR,
                            message: Cow::from(format!(
                                "Failed to read legacy task verification: {error}"
                            )),
                            data: None,
                        })?
                        .filter(|row| row.dispatch_id.is_none()),
                    _ => None,
                };
                let authorized = verification.as_ref().is_some_and(|row| {
                    matches!(
                        row.status,
                        cas_types::VerificationStatus::Approved
                            | cas_types::VerificationStatus::Skipped
                    ) && match dispatch.as_ref() {
                        Some(dispatch) => {
                            !matches!(row.provenance, cas_types::VerificationProvenance::Legacy)
                                && row.dispatch_id.as_deref() == Some(dispatch.id.as_str())
                        }
                        None => row.dispatch_id.is_none(),
                    }
                });
                if !authorized {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!(
                            "VERIFICATION REQUIRED: update status=closed is gated only for task {} \
                             and requires a legitimate capability-bound verifier or registered \
                             supervisor verdict. Use task action=close to create or recover its \
                             explicit verification dispatch. Other tasks and mutations remain available.",
                            task.id
                        )),
                        data: None,
                    });
                }
            }
            if task.status == TaskStatus::Closed && new_status != TaskStatus::Closed {
                cas_store::invalidate_verification_dispatch_for_new_cycle(&self.cas_root, &task.id)
                    .map_err(|error| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Failed to invalidate reopened task proof cycle: {error}"
                        )),
                        data: None,
                    })?;
            }
            task.status = new_status;
            changes.push("status");
        }

        // Handle epic association change
        if let Some(epic_id) = req.epic {
            let epic_id = epic_id.trim();
            let existing_parent_deps: Vec<Dependency> = task_store
                .get_dependencies(&req.id)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to read dependencies: {e}")),
                    data: None,
                })?
                .into_iter()
                .filter(|dep| dep.dep_type == DependencyType::ParentChild)
                .collect();

            // Validate requested epic first so we don't drop existing relationships on failure.
            if !epic_id.is_empty() {
                match task_store.get(epic_id) {
                    Ok(epic_task) => {
                        if epic_task.task_type != TaskType::Epic {
                            return Err(McpError {
                                code: ErrorCode::INVALID_PARAMS,
                                message: Cow::from(format!(
                                    "Task {} is not an epic (type: {})",
                                    epic_id, epic_task.task_type
                                )),
                                data: None,
                            });
                        }
                    }
                    Err(_) => {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(format!("Epic not found: {epic_id}")),
                            data: None,
                        });
                    }
                }
            }

            // Remove existing ParentChild dependency only after validation succeeded.
            for dep in existing_parent_deps {
                task_store
                    .remove_dependency(&req.id, &dep.to_id)
                    .map_err(|e| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!(
                            "Failed to remove existing epic dependency: {e}"
                        )),
                        data: None,
                    })?;
            }

            // Add new ParentChild dependency if epic_id is not empty.
            if !epic_id.is_empty() {
                let dep = Dependency {
                    from_id: req.id.clone(),
                    to_id: epic_id.to_string(),
                    dep_type: DependencyType::ParentChild,
                    created_at: chrono::Utc::now(),
                    created_by: Some("mcp".to_string()),
                };
                task_store.add_dependency(&dep).map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to add epic dependency: {e}")),
                    data: None,
                })?;
            }
            changes.push("epic");
        }

        if changes.is_empty() {
            return Ok(Self::success("No changes specified"));
        }

        if matches!(
            lifecycle_status_change,
            Some((old, TaskStatus::Closed)) if old != TaskStatus::Closed
        ) && let Some(target) = task.deliverables.work_target.as_ref()
        {
            let context = super::repo_context::resolve_repo_context(&self.cas_root, target)
                .map_err(|message| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(message),
                    data: None,
                })?;
            let worker_worktree = self
                .resolve_worker_worktree_path(&task, Some(&context))
                .map_err(|message| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(message),
                    data: None,
                })?;
            let evidence = super::lifecycle::close_ops::run_declared_pre_close_hook(
                &task,
                &context,
                worker_worktree.as_deref(),
                None,
            )
            .map_err(|message| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "PRE-CLOSE HOOK FAILED for task update status=closed: {message}"
                )),
                data: None,
            })?;
            task.deliverables.pre_close_hook = Some(evidence);
        }

        task.updated_at = chrono::Utc::now();

        task_store.update(&task).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        // cas-062d: push blocked / ready-reopened transitions.
        if let Some((old_st, new_st)) = lifecycle_status_change {
            use crate::mcp::tools::core::task::lifecycle::supervisor_push::LifecycleTransition;
            let kind = match new_st {
                TaskStatus::Blocked => Some(LifecycleTransition::Blocked),
                TaskStatus::Open if matches!(old_st, TaskStatus::Blocked | TaskStatus::Closed) => {
                    Some(LifecycleTransition::ReadyReopened)
                }
                TaskStatus::Closed => Some(LifecycleTransition::Closed),
                _ => None,
            };
            if let Some(kind) = kind {
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
                let occurrence =
                    crate::mcp::tools::core::task::lifecycle::supervisor_push::occurrence_from_updated_at(
                        task.updated_at,
                    );
                if let Err(e) = self.push_task_lifecycle(
                    &req.id,
                    &task.title,
                    old_st,
                    new_st,
                    &actor,
                    None,
                    kind,
                    &occurrence,
                ) {
                    use crate::mcp::tools::core::task::lifecycle::supervisor_push::{
                        lifecycle_push_failure_message, transition_key,
                    };
                    let key = transition_key(
                        &req.id,
                        old_st,
                        new_st,
                        std::env::var("CAS_FACTORY_SESSION").ok().as_deref(),
                        kind,
                        &occurrence,
                    );
                    return Err(Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        lifecycle_push_failure_message(&req.id, new_st, kind, &key, &e),
                    ));
                }
            }
        }

        if changes.contains(&"assignee") {
            let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
                &self.cas_root,
                "task_assigned",
                &[
                    ("task_id", &req.id),
                    ("title", &task.title),
                    ("prior_assignee", prior_assignee.as_deref().unwrap_or("")),
                    ("assignee", task.assignee.as_deref().unwrap_or("")),
                ],
            );
        }

        // Build response message with warnings if any
        let mut response = format!("Updated task {}: {}", req.id, changes.join(", "));
        if !warnings.is_empty() {
            response = format!("{}\n\n{}", response, warnings.join("\n"));
        }

        Ok(Self::success(response))
    }
}

#[cfg(test)]
mod status_transition_tests {
    use super::*;
    use crate::mcp::tools::core::task::lifecycle::close_ops::{
        EpicCloseGateOutcome, run_epic_close_merge_gate,
    };
    use cas_types::{Task, TaskStatus, TaskType};
    use tempfile::TempDir;

    fn git(repo: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn task_update_closed_to_in_progress_clears_factory_branch_anchor() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "cas@example.com"]);
        git(repo, &["config", "user.name", "CAS Test"]);
        std::fs::write(repo.join("base.rs"), "// base\n").unwrap();
        git(repo, &["add", "base.rs"]);
        git(repo, &["commit", "-q", "-m", "base"]);
        git(repo, &["checkout", "-q", "-b", "factory/worker"]);
        std::fs::write(repo.join("first.rs"), "// first close cycle\n").unwrap();
        git(repo, &["add", "first.rs"]);
        git(repo, &["commit", "-q", "-m", "first close cycle"]);
        let already_merged_anchor = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["merge", "-q", "--no-ff", "factory/worker"]);

        let cas_dir = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();
        let core = CasCore::with_daemon(cas_dir, None, None);
        let store = core.open_task_store().unwrap();
        store.init().unwrap();

        let mut task = Task::new("cas-anchor".into(), "Anchored closed task".into());
        task.status = TaskStatus::Closed;
        task.assignee = Some("worker".into());
        task.deliverables.factory_branch_anchor = Some(already_merged_anchor);
        task.deliverables.parked_branch = Some("factory/worker".into());
        store.add(&task).unwrap();

        let req: TaskUpdateRequest = serde_json::from_value(serde_json::json!({
            "id": task.id,
            "status": "in_progress"
        }))
        .unwrap();
        core.cas_task_update(Parameters(req)).await.unwrap();

        let updated = store.get("cas-anchor").unwrap();
        assert_eq!(updated.status, TaskStatus::InProgress);
        assert!(
            updated.deliverables.factory_branch_anchor.is_none(),
            "a Closed -> non-Closed transition must invalidate the prior close cycle's anchor"
        );

        git(repo, &["checkout", "-q", "factory/worker"]);
        std::fs::write(repo.join("rework.rs"), "// new unmerged work\n").unwrap();
        git(repo, &["add", "rework.rs"]);
        git(repo, &["commit", "-q", "-m", "new unmerged rework"]);

        let mut epic = Task::new("cas-epic".into(), "Parent epic".into());
        epic.task_type = TaskType::Epic;
        let close_req: TaskCloseRequest = serde_json::from_value(serde_json::json!({
            "id": epic.id
        }))
        .unwrap();
        let gate = run_epic_close_merge_gate(&epic, &close_req, "main", repo, &[updated]);
        assert!(
            matches!(gate, EpicCloseGateOutcome::Reject(_)),
            "after status-update rework, the epic gate must inspect the live branch and block; \
             got {gate:?}"
        );
    }
}

#[cfg(test)]
mod epic_owner_transfer_auth_tests {
    use super::*;
    use cas_types::{Agent, AgentRole, AgentStatus};

    fn agent(id: &str, name: &str, role: AgentRole, status: AgentStatus) -> Agent {
        let mut a = Agent::new(id.to_string(), name.to_string());
        a.role = role;
        a.status = status;
        a.last_heartbeat = chrono::Utc::now();
        a
    }

    #[test]
    fn test_bf98_normalize_assignee_empty_clears() {
        assert_eq!(normalize_assignee_update_value(""), None);
        assert_eq!(normalize_assignee_update_value("   \t  "), None);
        assert_eq!(
            normalize_assignee_update_value("  hv-scope  ").as_deref(),
            Some("hv-scope")
        );
        // Must never treat empty as a value that could session-id match.
        assert!(normalize_assignee_update_value("").is_none());
    }

    #[test]
    fn test_cc74_normalize_trims_and_rejects_empty() {
        assert_eq!(
            normalize_epic_verification_owner("  owner-id  ").as_deref(),
            Some("owner-id")
        );
        assert_eq!(normalize_epic_verification_owner(""), None);
        assert_eq!(normalize_epic_verification_owner("   \t  "), None);
    }

    #[test]
    fn test_cc74_unauthorized_caller_cannot_rewrite_owner() {
        let err = authorize_epic_owner_transfer_caller(
            Some("owner-id"),
            Some("other-id"),
            Some("other-name"),
            None,
            false,
        )
        .unwrap_err();
        assert!(
            err.contains("not the owner") && err.contains("cas-cc74"),
            "unauthorized rewrite must fail: {err}"
        );
    }

    #[test]
    fn test_cc74_unknown_caller_identity_fail_closed() {
        let err = authorize_epic_owner_transfer_caller(Some("owner-id"), None, None, None, false)
            .unwrap_err();
        assert!(
            err.contains("identity is unknown") && err.contains("fail closed"),
            "unknown caller must fail closed: {err}"
        );
        // Whitespace-only identity facets also count as unknown.
        let err_ws = authorize_epic_owner_transfer_caller(
            Some("owner-id"),
            Some("  "),
            Some(""),
            None,
            true, // even supervisor role without real identity fails
        )
        .unwrap_err();
        assert!(
            err_ws.contains("identity is unknown"),
            "whitespace identity must fail closed: {err_ws}"
        );
    }

    #[test]
    fn test_cc74_current_owner_may_transfer() {
        assert!(
            authorize_epic_owner_transfer_caller(
                Some("owner-id"),
                Some("owner-id"),
                None,
                None,
                false,
            )
            .is_ok()
        );
        // Match by display name
        assert!(
            authorize_epic_owner_transfer_caller(
                Some("owner-sup"),
                None,
                Some("owner-sup"),
                None,
                false,
            )
            .is_ok()
        );
        // Match ignores surrounding whitespace on facets
        assert!(
            authorize_epic_owner_transfer_caller(
                Some("owner-id"),
                Some("  owner-id  "),
                None,
                None,
                false,
            )
            .is_ok()
        );
    }

    #[test]
    fn test_cc74_supervisor_may_transfer_or_claim_unset() {
        assert!(
            authorize_epic_owner_transfer_caller(
                Some("owner-id"),
                Some("other-sup"),
                None,
                None,
                true,
            )
            .is_ok()
        );
        assert!(
            authorize_epic_owner_transfer_caller(None, Some("sup-id"), None, None, true,).is_ok()
        );
    }

    #[test]
    fn test_cc74_non_supervisor_cannot_claim_unset_owner() {
        let err = authorize_epic_owner_transfer_caller(
            None,
            Some("worker-id"),
            Some("worker-1"),
            None,
            false,
        )
        .unwrap_err();
        assert!(
            err.contains("no current owner") && err.contains("cas-cc74"),
            "worker claim of unset owner must fail: {err}"
        );
    }

    #[test]
    fn test_cc74_target_must_be_live_supervisor_identity() {
        let sup = agent(
            "sup-1",
            "bold-merlin",
            AgentRole::Supervisor,
            AgentStatus::Active,
        );
        assert!(validate_epic_owner_target_agent(&sup, EPIC_OWNER_TARGET_STALE_SECS).is_ok());

        let director = agent(
            "dir-1",
            "director",
            AgentRole::Director,
            AgentStatus::Active,
        );
        assert!(validate_epic_owner_target_agent(&director, EPIC_OWNER_TARGET_STALE_SECS).is_ok());

        let worker = agent("w-1", "worker-1", AgentRole::Worker, AgentStatus::Active);
        let err =
            validate_epic_owner_target_agent(&worker, EPIC_OWNER_TARGET_STALE_SECS).unwrap_err();
        assert!(
            err.contains("not a supervisor") && err.contains("cas-cc74"),
            "worker target must fail: {err}"
        );

        let mut dead = agent(
            "sup-dead",
            "dead-sup",
            AgentRole::Supervisor,
            AgentStatus::Shutdown,
        );
        let err =
            validate_epic_owner_target_agent(&dead, EPIC_OWNER_TARGET_STALE_SECS).unwrap_err();
        assert!(err.contains("not live"), "dead supervisor must fail: {err}");

        dead.status = AgentStatus::Active;
        dead.last_heartbeat = chrono::Utc::now() - chrono::Duration::seconds(10_000);
        let err =
            validate_epic_owner_target_agent(&dead, EPIC_OWNER_TARGET_STALE_SECS).unwrap_err();
        assert!(err.contains("stale"), "stale heartbeat must fail: {err}");
    }

    #[test]
    fn test_cc74_find_agent_by_id_or_name_unknown_fail_closed() {
        let agents = vec![
            agent(
                "sup-uuid",
                "OwnerSup",
                AgentRole::Supervisor,
                AgentStatus::Active,
            ),
            agent("w-uuid", "worker-1", AgentRole::Worker, AgentStatus::Active),
        ];
        assert_eq!(
            find_agent_for_epic_owner(&agents, "sup-uuid").map(|a| a.id.as_str()),
            Some("sup-uuid")
        );
        assert_eq!(
            find_agent_for_epic_owner(&agents, "ownersup").map(|a| a.id.as_str()),
            Some("sup-uuid")
        );
        assert!(find_agent_for_epic_owner(&agents, "missing").is_none());
        assert!(find_agent_for_epic_owner(&agents, "  ").is_none());
    }

    #[test]
    fn test_cc74_audit_note_records_transfer() {
        let note = epic_owner_transfer_audit_note(Some("old-owner"), "new-owner", "caller-1");
        assert!(
            note.contains("DECISION")
                && note.contains("old-owner")
                && note.contains("new-owner")
                && note.contains("caller-1")
                && note.contains("cas-cc74"),
            "audit note shape wrong: {note}"
        );
        let note_unset = epic_owner_transfer_audit_note(None, "new-owner", "sup");
        assert!(
            note_unset.contains("<unset>") && note_unset.contains("new-owner"),
            "unset previous must be explicit: {note_unset}"
        );
    }
}

#[cfg(test)]
mod assignment_freshness_branch_tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use cas_types::{Dependency, DependencyType, Task, TaskType};
    use tempfile::TempDir;

    fn open_store() -> (TempDir, std::sync::Arc<dyn cas_store::TaskStore>) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();
        let store = crate::store::open_task_store(&cas_dir).expect("open task store");
        store.init().expect("init task store");
        (temp, store)
    }

    #[test]
    fn prefers_parent_epic_branch_over_unrelated_epic() {
        let (_tmp, store) = open_store();

        let mut epic_a = Task::new("cas-epica".into(), "Epic A".into());
        epic_a.task_type = TaskType::Epic;
        epic_a.branch = Some("epic/a".into());
        store.add(&epic_a).unwrap();

        let mut epic_b = Task::new("cas-epicb".into(), "Epic B".into());
        epic_b.task_type = TaskType::Epic;
        epic_b.branch = Some("epic/z-last".into());
        store.add(&epic_b).unwrap();

        let child = Task::new("cas-childa".into(), "Child of A".into());
        store.add(&child).unwrap();
        store
            .add_dependency(&Dependency::new(
                child.id.clone(),
                epic_a.id.clone(),
                DependencyType::ParentChild,
            ))
            .unwrap();

        let branch = resolve_assignment_freshness_branch(store.as_ref(), &child);
        assert_eq!(
            branch.as_deref(),
            Some("epic/a"),
            "must use parent epic A branch, not concurrent epic B"
        );
        assert_ne!(branch.as_deref(), Some("epic/z-last"));
    }

    #[test]
    fn epic_task_uses_own_branch() {
        let (_tmp, store) = open_store();
        let mut epic = Task::new("cas-epic1".into(), "Epic".into());
        epic.task_type = TaskType::Epic;
        epic.branch = Some("epic/self".into());
        store.add(&epic).unwrap();

        let branch = resolve_assignment_freshness_branch(store.as_ref(), &epic);
        assert_eq!(branch.as_deref(), Some("epic/self"));
    }

    #[test]
    fn standalone_task_without_focus_returns_none() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_SESSION", None)]);
        let (_tmp, store) = open_store();
        let task = Task::new("cas-solo".into(), "Standalone".into());
        store.add(&task).unwrap();

        let branch = resolve_assignment_freshness_branch(store.as_ref(), &task);
        assert_eq!(
            branch, None,
            "no parent epic and no focus pin → None (caller uses base/main)"
        );
    }

    #[test]
    fn falls_back_to_session_focus_pin_branch() {
        let session = format!("test-focus-{}", std::process::id());
        let _env =
            TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_SESSION", Some(session.as_str()))]);
        let (_tmp, store) = open_store();

        let mut epic_a = Task::new("cas-epinf".into(), "Focused Epic".into());
        epic_a.task_type = TaskType::Epic;
        epic_a.branch = Some("epic/focused".into());
        store.add(&epic_a).unwrap();

        let mut epic_b = Task::new("cas-epother".into(), "Other Epic".into());
        epic_b.task_type = TaskType::Epic;
        epic_b.branch = Some("epic/other".into());
        store.add(&epic_b).unwrap();

        let solo = Task::new("cas-solof".into(), "No parent".into());
        store.add(&solo).unwrap();

        // Write session metadata with pinned focus on epic A.
        let meta_path = crate::ui::factory::metadata_path(&session);
        if let Some(parent) = meta_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let meta = serde_json::json!({
            "name": session,
            "created_at": "2026-07-21T00:00:00Z",
            "daemon_pid": 1,
            "socket_path": "/tmp/test.sock",
            "supervisor": { "name": "sup", "pid": null },
            "workers": [],
            "epic_id": null,
            "pinned_epic_id": "cas-epinf",
            "project_dir": null
        });
        std::fs::write(&meta_path, meta.to_string()).unwrap();
        let branch = resolve_assignment_freshness_branch(store.as_ref(), &solo);
        let _ = std::fs::remove_file(&meta_path);

        assert_eq!(
            branch.as_deref(),
            Some("epic/focused"),
            "standalone task should fall back to focus_epic pin branch"
        );
    }
}
