use crate::mcp::tools::core::imports::*;

pub(crate) mod close_ops;
pub(crate) mod proof_scope;
pub(crate) mod repository_proof;
pub(crate) mod stale_close_guard;
pub(crate) mod supervisor_push;

/// Resolve `epic_verification_owner` for a factory-mode epic create (cas-9fff).
///
/// Preference: agent id → display name → session id. Returns `Err` when none
/// resolve so factory epic creation cannot silently leave the owner unset
/// (which would disable both director routing and the close ownership gate).
pub(crate) fn resolve_factory_epic_owner(
    agent_id: Option<String>,
    agent_name: Option<String>,
    session_id: Option<String>,
) -> Result<String, String> {
    // cas-cc74: normalize/trim at the create write boundary so close gating
    // and owner-routed compares never see padded owner strings.
    let trim_nonempty = |s: Option<String>| {
        s.and_then(|v| {
            let t = v.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        })
    };
    trim_nonempty(agent_id)
        .or_else(|| trim_nonempty(agent_name))
        .or_else(|| trim_nonempty(session_id))
        .ok_or_else(|| {
            "Factory epic create requires a resolvable agent identity for \
             epic_verification_owner (CAS agent id / CAS_AGENT_NAME / CAS_SESSION_ID). \
             Refusing ownerless factory epic (cas-9fff)."
                .to_string()
        })
}

impl CasCore {
    pub async fn cas_task_create(
        &self,
        Parameters(req): Parameters<TaskCreateRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.cas_task_create_with_target(req, None, None).await
    }

    pub(crate) async fn cas_task_create_with_target(
        &self,
        req: TaskCreateRequest,
        target_repo: Option<&str>,
        target_branch: Option<&str>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let id = task_store.generate_id().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate ID: {e}")),
            data: None,
        })?;

        let task_type: TaskType = req.task_type.parse().unwrap_or(TaskType::Task);
        let labels: Vec<String> = req
            .labels
            .map(|l| {
                l.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let status = TaskStatus::Open;
        let blocked_by_ids: Vec<String> = req
            .blocked_by
            .as_deref()
            .map(|blocked_by| {
                blocked_by
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let epic_id = req
            .epic
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        // Reject the case where the epic and a blocker are the same task (cas-6009).
        // A task cannot be blocked by its own parent epic — the ParentChild dep and
        // a Blocks dep would share the same (from_id, to_id) pair, and `dep_remove`
        // used to delete both silently when given just an ID with no dep_type.
        if let Some(ref epic) = epic_id {
            if blocked_by_ids.contains(epic) {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!(
                        "Task cannot be both a child of and blocked by the same task ({epic}). \
                        Use `blocked_by` for peer tasks only."
                    )),
                    data: None,
                });
            }
        }

        let execution_note =
            crate::mcp::tools::types::validate_execution_note(req.execution_note.as_deref())
                .map_err(|msg| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(msg),
                    data: None,
                })?;

        // Absent/empty depth defaults to Deep (full rigor); an invalid value
        // is rejected with a clear error.
        let depth = crate::mcp::tools::types::validate_depth(req.depth.as_deref())
            .map_err(|msg| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(msg),
                data: None,
            })?
            .unwrap_or_default();
        let work_target =
            super::repo_context::declare_work_target(&self.cas_root, target_repo, target_branch)
                .map_err(|message| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(message),
                    data: None,
                })?;

        let now = chrono::Utc::now();
        // cas-9fff: in factory mode, stamp the creating agent as
        // epic_verification_owner on new epics so director completion
        // notifications route to the owning session (not every concurrent
        // supervisor). Fail closed when identity cannot be resolved —
        // silent None would disable both routing and the close guard.
        // Outside factory mode leave unset (legacy solo flow).
        let in_factory = std::env::var("CAS_FACTORY_MODE").is_ok()
            || std::env::var("CAS_FACTORY_SESSION").is_ok();
        let epic_verification_owner = if task_type == TaskType::Epic && in_factory {
            match resolve_factory_epic_owner(
                self.get_agent_id().ok(),
                std::env::var("CAS_AGENT_NAME").ok(),
                std::env::var("CAS_SESSION_ID").ok(),
            ) {
                Ok(owner) => Some(owner),
                Err(msg) => {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(msg),
                        data: None,
                    });
                }
            }
        } else {
            None
        };
        let task = Task {
            id: id.clone(),
            scope: crate::types::Scope::Project, // MCP tasks are project-scoped
            title: req.title,
            description: req.description.unwrap_or_default(),
            design: req.design.unwrap_or_default(),
            acceptance_criteria: req.acceptance_criteria.unwrap_or_default(),
            demo_statement: req.demo_statement.unwrap_or_default(),
            execution_note,
            notes: req.notes.unwrap_or_default(),
            status,
            priority: Priority(req.priority.min(4) as i32),
            task_type,
            assignee: req.assignee,
            labels,
            created_at: now,
            updated_at: now,
            closed_at: None,
            close_reason: None,
            external_ref: req.external_ref,
            content_hash: None,
            branch: None,
            deliverables: crate::types::TaskDeliverables {
                work_target,
                ..Default::default()
            },
            team_id: None,
            worktree_id: None,
            pending_verification: false,
            pending_worktree_merge: false,
            epic_verification_owner,
            share: None,
            depth,
        };

        // Associate the creation event and dependency metadata with the
        // current session. This is used by ambient recall to avoid feeding a
        // task back to the agent that just authored it.
        let created_by = self.get_agent_id().ok();
        task_store
            .create_atomic(
                &task,
                &blocked_by_ids,
                epic_id.as_deref(),
                created_by.as_deref().or(Some("mcp")),
            )
            .map_err(|e| {
                let is_invalid_epic = match &e {
                    cas_store::StoreError::TaskNotFound(missing_id) => {
                        epic_id.as_deref() == Some(missing_id.as_str())
                    }
                    cas_store::StoreError::Parse(msg) => {
                        msg.starts_with("Task ") && msg.contains(" is not an epic")
                    }
                    _ => false,
                };
                let (code, message) = if is_invalid_epic {
                    let msg = match &e {
                        cas_store::StoreError::TaskNotFound(missing_id) => {
                            format!("Epic not found: {missing_id}")
                        }
                        _ => e.to_string(),
                    };
                    (ErrorCode::INVALID_PARAMS, msg)
                } else {
                    (
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to create task: {e}"),
                    )
                };
                McpError {
                    code,
                    message: Cow::from(message),
                    data: None,
                }
            })?;

        if let Ok(search) = self.open_search_index() {
            let _ = search.index_task(&task);
        }

        // Auto-create epic branch for all epic creates (regardless of start flag)
        // Epics get a branch (not a worktree) - workers get worktrees when spawned
        let branch_info = if task.task_type == crate::types::TaskType::Epic
            && task.branch.as_deref().unwrap_or("").is_empty()
        {
            use crate::worktree::GitOperations;

            let declared_context = match task.deliverables.work_target.as_ref() {
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
            };
            let project_root = declared_context
                .as_ref()
                .map(|context| context.repo_root.as_path())
                .unwrap_or_else(|| self.cas_root.parent().unwrap_or(&self.cas_root));

            // Try to create epic branch using git operations directly
            // This works regardless of whether worktrees are enabled
            if GitOperations::is_git_available() {
                if let Ok(git_ops) =
                    GitOperations::detect_repo_root(project_root).map(GitOperations::new)
                {
                    let branch_name = format!("epic/{}-{}", slugify_for_branch(&task.title), id);
                    // Base epic branches on the configured trunk, never on
                    // the caller's incidental HEAD (cas-dc28).
                    let trunk = declared_context
                        .as_ref()
                        .map(|context| context.target_branch.clone())
                        .unwrap_or_else(|| git_ops.detect_default_branch());
                    // cas-a85e (GH #99): trunk stays the default anchor, but a
                    // checkout sitting on the PREVIOUS epic branch must not
                    // silently strand that epic's commits — base from it, or
                    // say plainly what was left out.
                    let base_choice = git_ops.resolve_epic_base(&trunk);
                    let base_ref = base_choice.base_ref.clone();
                    let base_sha = git_ops.ref_sha(&base_ref).unwrap_or_default();
                    let sha_preview = &base_sha[..base_sha.len().min(8)];
                    match git_ops.create_branch_from(&branch_name, &base_ref) {
                        Ok(created) => {
                            // Update epic with branch info (no worktree)
                            let task_store = self.open_task_store()?;
                            if let Ok(mut updated_task) = task_store.get(&id) {
                                if updated_task.branch.as_deref().unwrap_or("").is_empty() {
                                    updated_task.branch = Some(branch_name.clone());
                                }
                                let _ = task_store.update(&updated_task);
                            }

                            // Push to origin only when explicitly enabled
                            let push_enabled = std::env::var("CAS_PUSH_EPIC_BRANCH")
                                .map(|v| {
                                    let v = v.to_ascii_lowercase();
                                    v == "1" || v == "true" || v == "on"
                                })
                                .unwrap_or(false);
                            if created && push_enabled {
                                if let Err(e) = git_ops.push_branch(&branch_name) {
                                    eprintln!(
                                        "[CAS] Warning: Failed to push epic branch to origin: {e}"
                                    );
                                }
                            }

                            let divergence = base_choice
                                .notice
                                .as_deref()
                                .map(|notice| format!("\n   {notice}"))
                                .unwrap_or_default();
                            Some(format!(
                                "\n\n🌿 Epic branch created: {branch_name}\n   Base: '{base_ref}' @ {sha_preview}. Workers will branch from this when spawned.{divergence}"
                            ))
                        }
                        Err(e) => {
                            // Log but continue - branch creation is optional enhancement
                            eprintln!("Warning: Failed to create epic branch: {e}");
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self::success(format!(
            "Created task: {} - {} (P{}){}",
            id,
            task.title,
            task.priority.0,
            branch_info.unwrap_or_default()
        )))
    }

    /// List ready tasks
    pub async fn cas_task_ready(
        &self,
        Parameters(req): Parameters<TaskReadyBlockedRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        // If epic filter specified, get subtasks and filter to ready ones
        let mut tasks = if let Some(ref epic_id) = req.epic {
            let subtasks = task_store.get_subtasks(epic_id).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to get subtasks for epic {epic_id}: {e}")),
                data: None,
            })?;
            // Filter to only ready tasks (open, not blocked)
            subtasks
                .into_iter()
                .filter(|t| {
                    t.status == cas_types::TaskStatus::Open
                        && task_store
                            .get_blockers(&t.id)
                            .map_or(true, |b| b.is_empty())
                })
                .collect()
        } else {
            task_store.list_ready().map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to list: {e}")),
                data: None,
            })?
        };

        // Apply sorting. cas-06f9 (GH #104): unspecified means priority order
        // here, not creation order — this is the "what should I pick up next"
        // surface, and incidental ordering combined with the cap below is what
        // buried thirteen ready P0s behind P2/P3 follow-ups.
        let sort_opts = crate::mcp::tools::ready_blocked_sort_options(
            req.sort.as_deref(),
            req.sort_order.as_deref(),
        );
        sort_tasks(&mut tasks, &sort_opts);

        if tasks.is_empty() {
            let msg = if req.epic.is_some() {
                "No ready tasks in this epic"
            } else {
                "No ready tasks"
            };
            return Ok(Self::success(msg));
        }

        // cas-06f9 (GH #104): the cap stays (an unbounded dump is its own
        // problem) but it is no longer silent — the header carries the true
        // total and the footer says how to see the rest. "Ready tasks (10)"
        // read as a drained queue when 30 were waiting.
        let limit = req.limit.unwrap_or(10);
        let total = tasks.len();
        let shown = total.min(limit);
        let mut output =
            crate::mcp::tools::truncated_list_header("Ready tasks", total, shown, &sort_opts);
        for task in tasks.iter().take(limit) {
            output.push_str(&format!(
                "- [{}] P{} {} - {}\n",
                task.id, task.priority.0, task.task_type, task.title
            ));
        }
        output.push_str(&crate::mcp::tools::truncated_list_footer(total, shown));

        Ok(Self::success(output))
    }

    /// Start a task
    pub async fn cas_task_start(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.cas_task_start_with_options(Parameters(TaskStartRequest {
            id: req.id,
            brief: None,
        }))
        .await
    }

    /// Start a task with optional context-affordable output.
    pub async fn cas_task_start_with_options(
        &self,
        Parameters(req): Parameters<TaskStartRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let mut task = task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        if task.status == TaskStatus::Closed {
            // cas-3c23: this message used to tell EVERY caller "Use reopen
            // first" — a factory worker follows that verbatim, reopens an
            // already-merged task, re-verifies already-shipped code, and
            // re-closes it (the cas-a7c8 thrash loop). Reopen is now a
            // supervisor-only action (see `cas_task_reopen`), so the
            // guidance must differ by role: a supervisor may still reopen,
            // a worker must not.
            // cas-b269 review: do NOT clear halt_task_work here — failed
            // starts must preserve the urgent-stop halt.
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                if crate::harness_policy::is_supervisor_from_env() {
                    "Cannot start a closed task. Use reopen first if this task \
                     genuinely needs rework."
                        .to_string()
                } else {
                    format!(
                        "Cannot start a closed task. Task {} is closed — do not \
                         reopen it; report to your supervisor if you believe \
                         this task needs rework.",
                        req.id
                    )
                },
            ));
        }

        // cas-9684: PSR tasks are "work complete, awaiting supervisor review".
        // Allowing `start` would silently clobber the status back to InProgress,
        // dropping the task from `list status=pending_supervisor_review`.
        if task.status == TaskStatus::PendingSupervisorReview {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Cannot start a task that is pending supervisor review. \
                The work is already complete — wait for the supervisor to \
                review and close it (or reopen it if rework is needed).",
            ));
        }

        super::ensure_no_open_blockers(task_store.as_ref(), &req.id, "start")?;

        if let Some(target) = task.deliverables.work_target.as_ref() {
            super::repo_context::resolve_repo_context(&self.cas_root, target).map_err(
                |message| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(message),
                    data: None,
                },
            )?;
        }

        // cas-a844/cas-5054: a genuinely conflicted parked branch is unfinished
        // work, so its assigned worker may resume it. A clean AwaitingMerge
        // task remains worker-complete and must stay parked for the supervisor.
        let resuming_awaiting_merge = task.status == TaskStatus::AwaitingMerge;
        if resuming_awaiting_merge {
            if !task.deliverables.merge_conflicted {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Cannot start a task that is awaiting merge. The worker work is \
                         already complete; wait for the supervisor to merge the factory \
                         branch, then retry task close. If review fails — the supervisor \
                         declines the delivery, or requires an amendment after merging — \
                         they must first run \
                         `mcp__cas__task action=request_changes id={} reason=\"state what remains and what must be corrected or reverted\"`; \
                         a worker cannot self-reject or start a clean parked delivery. If \
                         that merge fails with a genuine \
                         git conflict, CAS marks the parked task conflicted and its assigned \
                         worker can then start task {} to resolve it.",
                        req.id,
                        req.id
                    ),
                ));
            }

            let now = chrono::Utc::now();
            let timestamp = now.format("%Y-%m-%d %H:%M");
            let parked_branch = task
                .deliverables
                .parked_branch
                .as_deref()
                .unwrap_or("the parked factory branch");
            let audit = format!(
                "[{timestamp}] Decision: resume from awaiting_merge for merge recovery. \
                 {parked_branch} was flagged with a merge conflict or its conflict \
                 preflight could not be evaluated, so the task is back in_progress for \
                 the assigned worker to inspect and resolve directly."
            );
            task.notes = if task.notes.is_empty() {
                audit
            } else {
                format!("{}\n\n{}", task.notes, audit)
            };
        }

        // Auto-claim the task with a lease
        let agent_id = self.get_agent_id()?;

        let agent_store = self.open_agent_store()?;

        // Check agent role for supervisor/worker-specific logic
        let is_worker = agent_store
            .get(&agent_id)
            .map(|a| a.role == cas_types::AgentRole::Worker)
            .unwrap_or(false);

        // cas-3558: reject a worker starting a task explicitly assigned to
        // someone else. This is the code-level half of the self-dispatch
        // guard — the skill-level half tells an idle worker to wait for an
        // explicit assignment instead of grabbing from `action=ready`. Only
        // fires for factory workers with a *pre-existing, different*
        // assignee: `assignee.is_none()` (the normal first-start case) and
        // Standard/interactive sessions are both exempt, so this can't
        // regress the auto-assign-on-start behavior above.
        if is_worker {
            if let (Ok(agent), Some(assignee)) =
                (agent_store.get(&agent_id), task.assignee.as_deref())
            {
                if assignee != agent.name {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "Task {} is assigned to {}, not you ({}). Do not self-dispatch — \
                            wait for an explicit assignment, or ask the supervisor to \
                            reassign it: mcp__cas__task action=transfer id={} to_agent=<your-agent-id>",
                            req.id, assignee, agent.name, req.id
                        ),
                    ));
                }
            }
        }

        // Check if supervisor is trying to start a non-epic task
        if let Ok(agent) = agent_store.get(&agent_id) {
            if agent.role == cas_types::AgentRole::Supervisor
                && task.task_type != crate::types::TaskType::Epic
            {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(
                        "Supervisors cannot start non-epic tasks. To delegate work:\n\n\
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

        // cas-156b (GH #135): a task with no work target used to be leased with
        // no nativity check at all, so replicated foreign rows were claimed,
        // worked and closed from the wrong repository. Gather the four local
        // signals and warn (never block) when none of them anchors this task
        // here. Evaluated before the assignee defaults to the starting agent
        // further down — otherwise `start` would manufacture its own anchor and
        // the check could never fire. Every lookup is best-effort: a failed read
        // must not turn an advisory check into a failed start, and each
        // "unknown" resolves toward silence, preserving the fail-silent contract.
        let unanchored_warning = {
            let evidence = super::repo_context::TaskAnchorEvidence {
                has_work_target: task.deliverables.work_target.is_some(),
                dependency_edge_count: task_store
                    .get_dependencies(&req.id)
                    .map(|deps| deps.len())
                    .unwrap_or(usize::MAX),
                assignee_is_local_agent: match task.assignee.as_deref() {
                    // `None` status filter on purpose: a registered agent that
                    // is idle or stopped is still local provenance.
                    Some(assignee) => agent_store
                        .list(None)
                        .map(|agents| agents.iter().any(|agent| agent.name == assignee))
                        // An unreadable agent registry must not manufacture a
                        // warning about an assignee we simply could not check.
                        .unwrap_or(true),
                    None => false,
                },
                cloud_sync_configured: crate::cloud::CloudConfig::load()
                    .map(|config| config.is_logged_in())
                    .unwrap_or(false),
            };
            if super::repo_context::task_has_no_local_anchor(&evidence) {
                Some(super::repo_context::unanchored_task_start_warning(
                    &req.id,
                    &self.cas_root,
                    crate::cloud::get_project_canonical_id().as_deref(),
                ))
            } else {
                None
            }
        };

        let config = self.load_config();
        let lease_duration = (config.lease().default_duration_mins as i64) * 60;

        let claim_info =
            match agent_store.try_claim(&req.id, &agent_id, lease_duration, Some("Task started")) {
                Ok(ClaimResult::Success(lease)) => Some(format!(
                    " (claimed until {})",
                    lease.expires_at.format("%H:%M")
                )),
                Ok(ClaimResult::AlreadyClaimed {
                    held_by,
                    expires_at,
                    ..
                }) => {
                    if held_by == agent_id {
                        Some(format!(
                            " (already claimed by you until {})",
                            expires_at.format("%H:%M")
                        ))
                    } else {
                        // Resolve UUID → friendly name so the supervisor can
                        // identify the worker without cross-referencing worker_status.
                        let holder_display = agent_store
                            .get(&held_by)
                            .map(|a| format!("{} ({})", a.name, held_by))
                            .unwrap_or_else(|_| held_by.clone());
                        return Err(Self::error(
                            ErrorCode::INVALID_PARAMS,
                            format!(
                                "Task is locked by {} until {}",
                                holder_display,
                                expires_at.format("%H:%M")
                            ),
                        ));
                    }
                }
                Ok(_) => None, // TaskNotFound, NotClaimable, Unauthorized - log but continue
                Err(e) => {
                    // Log but continue - claim is optional enhancement
                    eprintln!("Warning: Failed to claim task: {e}");
                    None
                }
            };

        // Record working epic if this task belongs to one
        // This is used by the exit blocker to ensure all epic subtasks are completed
        // Also look up the parent epic's worktree and sibling notes
        let brief = req.brief.unwrap_or(false);
        let mut parent_worktree_info: Option<String> = None;
        let mut epic_ownership_info: Option<String> = None;
        let mut sibling_notes_info: Option<String> = None;
        if let Ok(deps) = task_store.get_dependencies(&req.id) {
            for dep in deps {
                if dep.dep_type == crate::types::DependencyType::ParentChild {
                    // This task is a subtask - dep.to_id is the parent epic
                    if let Ok(parent) = task_store.get(&dep.to_id) {
                        if parent.task_type == crate::types::TaskType::Epic {
                            let _ = agent_store.add_working_epic(&agent_id, &parent.id);

                            // Brief start is deliberately own-task-only. It
                            // still records the working epic, but avoids even
                            // fetching sibling notes so large sibling payloads
                            // cannot consume worker context (cas-0447).
                            if !brief {
                                if let Ok(sibling_notes) =
                                    task_store.get_sibling_notes(&parent.id, &req.id)
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
                            }

                            // Epic ownership logic - only for supervisors, not workers
                            // Workers just execute their assigned tasks; supervisors own the epic
                            if !is_worker {
                                // Auto-start epic if not already in progress
                                let epic_was_started = if parent.status != TaskStatus::InProgress {
                                    let mut parent_mut = parent.clone();
                                    parent_mut.status = TaskStatus::InProgress;
                                    parent_mut.updated_at = chrono::Utc::now();
                                    task_store.update(&parent_mut).is_ok()
                                } else {
                                    false
                                };

                                // Claim epic for this agent (they now own the entire epic)
                                let epic_claim_status = match agent_store.try_claim(
                                    &parent.id,
                                    &agent_id,
                                    lease_duration,
                                    Some("Epic auto-claimed from subtask start"),
                                ) {
                                    Ok(ClaimResult::Success(lease)) => {
                                        format!(
                                            "claimed until {}",
                                            lease.expires_at.format("%H:%M")
                                        )
                                    }
                                    Ok(ClaimResult::AlreadyClaimed {
                                        held_by,
                                        expires_at,
                                        ..
                                    }) => {
                                        if held_by == agent_id {
                                            format!(
                                                "already yours until {}",
                                                expires_at.format("%H:%M")
                                            )
                                        } else {
                                            format!("held by {held_by}")
                                        }
                                    }
                                    _ => "unclaimed".to_string(),
                                };

                                // Get subtask count
                                let subtask_count = task_store
                                    .get_subtasks(&parent.id)
                                    .map(|s| s.len())
                                    .unwrap_or(0);

                                // Build epic ownership message
                                let started_note = if epic_was_started {
                                    " (auto-started)"
                                } else {
                                    ""
                                };
                                if !brief {
                                    epic_ownership_info = Some(format!(
                                        "\n\n📋 EPIC OWNERSHIP: You are now responsible for epic [{}] {}{}\n   Subtasks: {} total | Status: {}",
                                        parent.id,
                                        parent.title,
                                        started_note,
                                        subtask_count,
                                        epic_claim_status
                                    ));
                                }
                            }

                            // Look up the parent epic's worktree
                            if !brief && let Some(ref worktree_id) = parent.worktree_id {
                                if let Ok(wt_store) = self.open_worktree_store() {
                                    if let Ok(worktree) = wt_store.get(worktree_id) {
                                        if worktree.path.exists() {
                                            parent_worktree_info = Some(format!(
                                                "\n\n🌳 This task belongs to epic [{}] {}\n   Work in directory: {}\n   Branch: {}",
                                                parent.id,
                                                parent.title,
                                                worktree.path.display(),
                                                worktree.branch
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

        // Try to create worktree if enabled AND task is an epic
        // Worktrees are scoped to epics, not individual tasks
        let worktree_info = if task.task_type == crate::types::TaskType::Epic {
            // Track this epic in working_epics for exit blocker
            // This ensures we can't stop while epic subtasks remain incomplete
            let _ = agent_store.add_working_epic(&agent_id, &req.id);

            if let Some(manager) = self.worktree_manager() {
                match manager.create_for_epic(&req.id, Some(&agent_id)) {
                    Ok(worktree) => {
                        // Store the worktree record
                        if let Ok(wt_store) = self.open_worktree_store() {
                            let _ = wt_store.add(&worktree);
                        }

                        // Update epic with worktree info
                        task.branch = Some(worktree.branch.clone());
                        task.worktree_id = Some(worktree.id.clone());

                        Some(format!(
                            "\n\n🌳 Worktree created for isolated development:\n   Branch: {}\n   Path: {}\n\n⚠️  Work in this directory for all changes.",
                            worktree.branch,
                            worktree.path.display()
                        ))
                    }
                    Err(e) => {
                        // Log but continue - worktree is optional enhancement
                        eprintln!("Warning: Failed to create worktree: {e}");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        // cas-6945: `start` previously left `task.assignee` untouched, so the TUI's
        // epic-focus inference gate (task_assigned_to_session_agent, tasks.rs) could
        // never pass without the supervisor manually running
        // `task action=update assignee=<worker>`. Default the assignee to the
        // starting agent's display name — assignees are matched as display names,
        // not session IDs (cas-dbbb, see task/update.rs) — whenever it's unset.
        // Never clobber an existing assignee (e.g. the supervisor already assigned
        // it to someone else, or this is a resume after reassignment).
        if task.assignee.is_none() {
            if let Ok(agent) = agent_store.get(&agent_id) {
                task.assignee = Some(agent.name.clone());
            }
        }

        // Capture old status before mutation for lifecycle push (cas-062d).
        let old_status = task.status;
        task.status = TaskStatus::InProgress;
        task.updated_at = chrono::Utc::now();

        // cas-ec74: `updated_at` is store-owned. Adopt the stamp the store
        // actually persisted so the lifecycle occurrence below is derived from
        // the same clock read as the stored row, not from a second one.
        task.updated_at = task_store.update(&task).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        // cas-b269 review 2: clear urgent-stop halt ONLY after start fully
        // succeeds, and only if halt gen is not newer than this start's
        // ceiling (concurrent urgent stop must win).
        if let Ok(agent_store) = self.open_agent_store() {
            if let Ok(mut agent) = agent_store.get(&agent_id) {
                if stale_close_guard::agent_task_work_halted(&agent.metadata) {
                    let clear_ceiling = chrono::Utc::now().timestamp_millis().max(0) as u64;
                    let stored_gen = stale_close_guard::halt_generation(&agent.metadata);
                    if stale_close_guard::should_clear_halt_at_generation(stored_gen, clear_ceiling)
                    {
                        stale_close_guard::clear_halt_metadata(&mut agent.metadata);
                        let _ = agent_store.update(&agent);
                    }
                }
            }
        }

        // cas-062d / cas-17e4: durable outbox push after successful start.
        let actor = agent_store
            .get(&agent_id)
            .map(|a| a.name)
            .unwrap_or_else(|_| agent_id.clone());
        let occurrence = supervisor_push::occurrence_from_updated_at(task.updated_at);
        let push_note = match self.push_task_lifecycle(
            &req.id,
            &task.title,
            old_status,
            TaskStatus::InProgress,
            &actor,
            None,
            supervisor_push::LifecycleTransition::Started,
            &occurrence,
        ) {
            Ok(supervisor_push::LifecyclePushResult::Enqueued { notification_id })
            | Ok(supervisor_push::LifecyclePushResult::Recovered { notification_id }) => {
                format!("\n\n📡 Supervisor notified (lifecycle event id={notification_id})")
            }
            Ok(supervisor_push::LifecyclePushResult::AlreadyComplete { .. }) => {
                "\n\n📡 Supervisor lifecycle event already complete (outbox)".to_string()
            }
            Ok(supervisor_push::LifecyclePushResult::NoSupervisor) => String::new(),
            Err(e) => {
                let key = supervisor_push::transition_key(
                    &req.id,
                    old_status,
                    TaskStatus::InProgress,
                    std::env::var("CAS_FACTORY_SESSION").ok().as_deref(),
                    supervisor_push::LifecycleTransition::Started,
                    &occurrence,
                );
                return Err(Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    supervisor_push::lifecycle_push_failure_message(
                        &req.id,
                        TaskStatus::InProgress,
                        supervisor_push::LifecycleTransition::Started,
                        &key,
                        &e,
                    ),
                ));
            }
        };

        // For subtasks, show parent epic's worktree; for epics, show newly created worktree
        let wt_info = parent_worktree_info.or(worktree_info).unwrap_or_default();

        let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
            &self.cas_root,
            "task_started",
            &[
                ("task_id", &req.id),
                ("title", &task.title),
                ("actor", &actor),
                ("assignee", task.assignee.as_deref().unwrap_or("")),
            ],
        );

        if brief {
            // Bound the complete variable portion of the brief response. The
            // fixed header/claim/warning/push text remains small, while own
            // notes are capped to 4 KiB and sibling/epic/workflow payloads are
            // omitted entirely.
            let own_notes = if task.notes.is_empty() {
                String::new()
            } else {
                format!(
                    "\n\n📋 TASK NOTES:\n{}",
                    crate::mcp::tools::truncate_str(&task.notes, 4_093)
                )
            };
            let response = format!(
                "Started task: {} - {}{}{}{}{}",
                req.id,
                crate::mcp::tools::truncate_str(&task.title, 509),
                claim_info.unwrap_or_default(),
                crate::mcp::tools::truncate_str(
                    &unanchored_warning.unwrap_or_default(),
                    765,
                ),
                own_notes,
                push_note,
            );
            // `truncate_str` appends three bytes when it truncates, hence the
            // 6_141 payload bound for a hard 6 KiB response ceiling.
            return Ok(Self::success(crate::mcp::tools::truncate_str(
                &response, 6_141,
            )));
        }

        Ok(Self::success(format!(
            "Started task: {} - {}{}{}{}{}{}{}{}",
            req.id,
            task.title,
            claim_info.unwrap_or_default(),
            // cas-156b: placed directly after the claim line so the nativity
            // warning cannot be pushed out of view by long sibling-note or
            // worktree blocks.
            unanchored_warning.unwrap_or_default(),
            epic_ownership_info.unwrap_or_default(),
            wt_info,
            sibling_notes_info.unwrap_or_default(),
            Self::workflow_guidance(),
            push_note,
        )))
    }
}

#[cfg(test)]
mod factory_epic_owner_tests {
    use super::resolve_factory_epic_owner;

    #[test]
    fn test_9fff_factory_epic_owner_prefers_agent_id() {
        let owner = resolve_factory_epic_owner(
            Some("agent-uuid".into()),
            Some("display-name".into()),
            Some("session".into()),
        )
        .unwrap();
        assert_eq!(owner, "agent-uuid");
    }

    #[test]
    fn test_9fff_factory_epic_owner_falls_back_to_name_then_session() {
        assert_eq!(
            resolve_factory_epic_owner(None, Some("owner-sup".into()), Some("sess".into()))
                .unwrap(),
            "owner-sup"
        );
        assert_eq!(
            resolve_factory_epic_owner(None, None, Some("sess-only".into())).unwrap(),
            "sess-only"
        );
    }

    #[test]
    fn test_9fff_factory_epic_create_rejects_when_identity_unresolvable() {
        let err = resolve_factory_epic_owner(None, None, None).unwrap_err();
        assert!(
            err.contains("cas-9fff") && err.contains("Refusing ownerless factory epic"),
            "expected fail-closed ownerless create, got: {err}"
        );
        // Empty strings must not count as identity either.
        let err_empty =
            resolve_factory_epic_owner(Some("  ".into()), Some("".into()), None).unwrap_err();
        assert!(err_empty.contains("Refusing ownerless factory epic"));
    }

    /// cas-cc74: create write boundary trims owner identity before store.
    #[test]
    fn test_cc74_factory_epic_owner_trims_whitespace() {
        let owner =
            resolve_factory_epic_owner(Some("  agent-uuid  ".into()), Some("display".into()), None)
                .unwrap();
        assert_eq!(owner, "agent-uuid");
    }
}
