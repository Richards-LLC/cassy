use crate::mcp::tools::core::imports::*;

impl CasCore {
    pub async fn cas_task_show(
        &self,
        Parameters(req): Parameters<TaskShowRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let task = task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        let target = task
            .deliverables
            .work_target
            .as_ref()
            .map(|target| format!("{} @ {}", target.repo_selector, target.target_branch))
            .unwrap_or_else(|| "(none — trunk fallback)".to_string());

        let project_id = super::current_project_id(&self.cas_root);
        let origin = task
            .origin_project
            .as_deref()
            .unwrap_or("unassigned legacy row");
        let origin = if project_id
            .as_deref()
            .is_some_and(|project_id| task.origin_project.as_deref() != Some(project_id))
            && task.origin_project.is_some()
        {
            format!("{origin} — this task is owned elsewhere")
        } else {
            origin.to_string()
        };

        let mut output = format!(
            "Task: {}\n{}\n\nTitle: {}\nStatus: {:?}\nPriority: P{}\nType: {}\nDepth: {}\nDelivery mode: {}\nOrigin project: {}\nTarget: {}\n",
            task.id,
            "=".repeat(task.id.len() + 6),
            task.title,
            task.status,
            task.priority.0,
            task.task_type,
            task.depth,
            task.delivery_mode,
            origin,
            target
        );

        // Structured execution state is the compact machine resume surface.
        // Prose notes remain below for human/audit history, but are not needed
        // to reconstruct the current worker position.
        if let Some(state) = task_store
            .get_execution_state(&task.id)
            .map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Task {} has unreadable structured execution state: {error}",
                    task.id
                )),
                data: None,
            })?
        {
            let encoded = serde_json::to_string(&state).map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Task {} structured execution state could not be encoded: {error}",
                    task.id
                )),
                data: None,
            })?;
            output.push_str("\nStructured execution state:\n");
            output.push_str(&encoded);
            output.push('\n');
        }

        match cas_store::get_latest_worker_delivery(&self.cas_root, &task.id) {
            Ok(Some((receipt, transaction))) => {
                let next_action = match transaction.state {
                    cas_types::WorkerDeliveryState::AwaitingVerification => {
                        "capability-bound verifier or registered supervisor verdict"
                    }
                    cas_types::WorkerDeliveryState::AwaitingMerge => {
                        "registered supervisor worktree_merge with this task_id"
                    }
                    cas_types::WorkerDeliveryState::MergeAuthorized => {
                        "registered supervisor retry; Cassy reconciles exact target ancestry"
                    }
                    cas_types::WorkerDeliveryState::Merged
                    | cas_types::WorkerDeliveryState::CloseReady => {
                        "registered supervisor retry to resume the existing close"
                    }
                    cas_types::WorkerDeliveryState::Delivered => "none (delivery complete)",
                    cas_types::WorkerDeliveryState::VerificationFailed => {
                        "worker fixes findings, obtains fresh proof, and submits a new receipt"
                    }
                    cas_types::WorkerDeliveryState::ChangesRequested => {
                        "assigned worker starts a fresh cycle and adds the requested corrective or revert commits"
                    }
                    cas_types::WorkerDeliveryState::Conflict => {
                        "worker resolves conflict explicitly, then submits a new receipt"
                    }
                    cas_types::WorkerDeliveryState::Stale
                    | cas_types::WorkerDeliveryState::RepoMismatch
                    | cas_types::WorkerDeliveryState::TipChanged => {
                        "worker revalidates repository/tips and submits a new receipt"
                    }
                };
                output.push_str(&format!(
                    "\nTransactional delivery: {}\n  Receipt: {}\n  Commit: {}\n  Next action: {}",
                    transaction.state, receipt.id, receipt.commit_sha, next_action
                ));
                if let Some(artifact_path) = receipt.artifact_path {
                    output.push_str(&format!("\n  Durable artifact: {artifact_path}"));
                }
                if let Some(detail) = transaction.last_error_detail {
                    output.push_str(&format!("\n  Recovery detail: {detail}"));
                }
                output.push('\n');
            }
            Ok(None) => {}
            Err(error) => {
                return Ok(Self::tool_error(format!(
                    "DELIVERY STATE INVALID: task {} has unreadable durable worker-delivery state: {error}. Cassy refuses to infer a plausible state or next action.",
                    task.id
                )));
            }
        }

        // cas-a844: `awaiting_merge` alone reads as "done, just a formality" —
        // whether that's true depends entirely on whether the parked branch
        // can actually merge cleanly. Surface the distinction explicitly so
        // a conflicted parked task never looks indistinguishable from a
        // clean one queued for the supervisor.
        if task.status == TaskStatus::AwaitingMerge {
            if task.deliverables.merge_conflicted {
                output.push_str(
                    "⚠️  MERGE CONFLICT / REWORK REQUIRED — this task is NOT complete. \
                     The parked branch is conflicted or its conflict preflight could \
                     not be evaluated; the assigned worker can `task start` it to \
                     inspect and resolve the branch directly.\n",
                );
            } else {
                output.push_str(
                    "(mergeable — queued for the supervisor to merge the factory branch)\n",
                );
            }
            if let Some(ref branch) = task.deliverables.parked_branch {
                output.push_str(&format!("Parked branch: {branch}\n"));
            }
        }

        if !task.description.is_empty() {
            output.push_str(&format!("\nDescription:\n{}\n", task.description));
        }

        if !task.notes.is_empty() {
            output.push_str(&format!("\nNotes:\n{}\n", task.notes));
        }

        // cas-7d54: surface web-authored comments (read-only mirror, contract
        // §2). Best-effort fetch off the async runtime via spawn_blocking;
        // degrades to nothing when not logged in / no team / offline so it
        // never blocks or fails `task show`.
        {
            let cas_root = self.cas_root.clone();
            let comment_task_id = req.id.clone();
            let comments = tokio::task::spawn_blocking(move || {
                crate::cloud::comments::fetch_task_comments(&cas_root, &comment_task_id)
            })
            .await
            .unwrap_or_default();
            output.push_str(&crate::cloud::comments::render_comments_section(&comments));
        }

        if !task.design.is_empty() {
            output.push_str(&format!("\nDesign:\n{}\n", task.design));
        }

        if !task.acceptance_criteria.is_empty() {
            output.push_str(&format!(
                "\nAcceptance Criteria:\n{}\n",
                task.acceptance_criteria
            ));
        }

        if !task.demo_statement.is_empty() {
            output.push_str(&format!("\nDemo: {}\n", task.demo_statement));
        }

        if let Some(ref execution_note) = task.execution_note {
            output.push_str(&format!("\nExecution Note: {execution_note}\n"));
        }

        if !task.labels.is_empty() {
            output.push_str(&format!("\nLabels: {}\n", task.labels.join(", ")));
        }

        output.push_str(&format!(
            "\nCreated: {}\nUpdated: {}",
            task.created_at.format("%Y-%m-%d %H:%M"),
            task.updated_at.format("%Y-%m-%d %H:%M")
        ));

        if let Some(closed) = task.closed_at {
            let label = if task.status == TaskStatus::Cancelled {
                "Cancelled"
            } else {
                "Closed"
            };
            output.push_str(&format!("\n{label}: {}", closed.format("%Y-%m-%d %H:%M")));
        }

        if let Some(outcome) = task.effective_terminal_outcome() {
            match outcome {
                cas_types::TaskTerminalOutcome::Delivered => {
                    output.push_str("\nOutcome: delivered");
                }
                cas_types::TaskTerminalOutcome::NegativeResult => {
                    output.push_str("\nOutcome: measured negative result (no delivery)");
                }
                cas_types::TaskTerminalOutcome::Decision => {
                    output.push_str("\nOutcome: recorded decision (no delivery)");
                }
                cas_types::TaskTerminalOutcome::Cancelled { superseded_by } => {
                    output.push_str("\nOutcome: cancelled without delivery");
                    if let Some(pointer) = superseded_by {
                        output.push_str(&format!("\nSuperseded by: {pointer}"));
                    }
                }
            }
            if let Some(reason) = task.close_reason.as_deref() {
                output.push_str(&format!("\nTerminal reason: {reason}"));
            }
        }

        // Show deliverables for closed tasks
        if !task.deliverables.is_empty() {
            output.push_str("\n\nDeliverables:");
            if !task.deliverables.files_changed.is_empty() {
                output.push_str(&format!(
                    "\n  Files changed ({}):",
                    task.deliverables.files_changed.len()
                ));
                for file in &task.deliverables.files_changed {
                    output.push_str(&format!("\n    - {file}"));
                }
            }
            if let Some(ref commit) = task.deliverables.commit_hash {
                output.push_str(&format!("\n  Commit: {commit}"));
            }
            if let Some(ref merge) = task.deliverables.merge_commit {
                output.push_str(&format!("\n  Merge commit: {merge}"));
            }
        }

        if req.with_deps {
            if let Ok(deps) = task_store.get_dependencies(&req.id) {
                let blocked_by: Vec<String> = deps
                    .iter()
                    .filter(|dep| dep.dep_type == DependencyType::Blocks)
                    .map(|dep| dep.to_id.clone())
                    .collect();
                let parent_epics: Vec<String> = deps
                    .iter()
                    .filter(|dep| dep.dep_type == DependencyType::ParentChild)
                    .map(|dep| dep.to_id.clone())
                    .collect();
                let other_outgoing: Vec<String> = deps
                    .iter()
                    .filter(|dep| {
                        dep.dep_type != DependencyType::Blocks
                            && dep.dep_type != DependencyType::ParentChild
                    })
                    .map(|dep| format!("{:?}: {}", dep.dep_type, dep.to_id))
                    .collect();

                let blocking: Vec<String> = task_store
                    .get_dependents(&req.id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|dep| dep.dep_type == DependencyType::Blocks)
                    .map(|dep| dep.from_id)
                    .collect();

                if !blocked_by.is_empty()
                    || !blocking.is_empty()
                    || !parent_epics.is_empty()
                    || !other_outgoing.is_empty()
                {
                    output.push_str("\n\nDependencies:\n");
                }
                if !blocked_by.is_empty() {
                    output.push_str(&format!("  - Blocked by: {}\n", blocked_by.join(", ")));
                }
                if !blocking.is_empty() {
                    output.push_str(&format!("  - Blocks: {}\n", blocking.join(", ")));
                }
                if !other_outgoing.is_empty() {
                    for dep in &other_outgoing {
                        output.push_str(&format!("  - {dep}\n"));
                    }
                }

                // Show parent epic (for non-epic tasks)
                if task.task_type != TaskType::Epic {
                    for epic_id in &parent_epics {
                        if let Ok(epic) = task_store.get(epic_id) {
                            output.push_str(&format!("\nEpic: {} - {}\n", epic.id, epic.title));
                        }
                    }
                }
            }

            // Show subtasks (for epics)
            if task.task_type == TaskType::Epic {
                if let Ok(subtasks) = task_store.get_subtasks(&req.id) {
                    if !subtasks.is_empty() {
                        let terminal_count = subtasks.iter().filter(|t| t.is_terminal()).count();
                        let delivered_count =
                            subtasks.iter().filter(|t| t.counts_as_delivered()).count();
                        let cancelled_count = subtasks
                            .iter()
                            .filter(|t| t.status == TaskStatus::Cancelled)
                            .count();
                        output.push_str(&format!(
                            "\n\nSubtasks ({}/{} terminal; {} delivered; {} cancelled):\n",
                            terminal_count,
                            subtasks.len(),
                            delivered_count,
                            cancelled_count
                        ));
                        for subtask in &subtasks {
                            let status_icon = match subtask.status {
                                TaskStatus::Open => "○",
                                TaskStatus::InProgress => "●",
                                TaskStatus::Blocked => "◉",
                                TaskStatus::Closed => "✓",
                                TaskStatus::Cancelled => "⊘",
                                TaskStatus::AwaitingMerge => "⇄",
                            };
                            output.push_str(&format!(
                                "  {} {} [P{}] {}\n",
                                status_icon,
                                subtask.id,
                                subtask.priority.0,
                                if subtask.title.len() > 40 {
                                    truncate_str(&subtask.title, 37)
                                } else {
                                    subtask.title.clone()
                                }
                            ));
                        }
                    }
                }
            }
        }

        // Show worktree info if this task has one or belongs to an epic with one
        if let Some(ref worktree_id) = task.worktree_id {
            // This task (epic) has its own worktree
            if let Ok(wt_store) = self.open_worktree_store() {
                if let Ok(worktree) = wt_store.get(worktree_id) {
                    let status = if worktree.path.exists() {
                        ""
                    } else {
                        " (missing)"
                    };
                    output.push_str(&format!(
                        "\n\n🌳 Worktree:\n   Path: {}{}\n   Branch: {}",
                        worktree.path.display(),
                        status,
                        worktree.branch
                    ));
                }
            }
        } else {
            // Check if this task belongs to a parent epic with a worktree
            if let Ok(deps) = task_store.get_dependencies(&req.id) {
                for dep in deps {
                    if dep.dep_type == crate::types::DependencyType::ParentChild {
                        if let Ok(parent) = task_store.get(&dep.to_id) {
                            if parent.task_type == crate::types::TaskType::Epic {
                                if let Some(ref worktree_id) = parent.worktree_id {
                                    if let Ok(wt_store) = self.open_worktree_store() {
                                        if let Ok(worktree) = wt_store.get(worktree_id) {
                                            let status = if worktree.path.exists() {
                                                ""
                                            } else {
                                                " (missing)"
                                            };
                                            output.push_str(&format!(
                                                "\n\n🌳 Parent Epic Worktree:\n   Epic: [{}] {}\n   Path: {}{}\n   Branch: {}",
                                                parent.id,
                                                parent.title,
                                                worktree.path.display(),
                                                status,
                                                worktree.branch
                                            ));
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(Self::success(output))
    }

    /// List blocked tasks
    pub async fn cas_task_blocked(
        &self,
        Parameters(req): Parameters<TaskReadyBlockedRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        // If epic filter specified, get subtasks and filter to blocked ones
        let mut blocked: Vec<(cas_types::Task, Vec<cas_types::Task>)> = if let Some(ref epic_id) =
            req.epic
        {
            let subtask_ids = task_store
                .get_subtasks(epic_id)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to get subtasks for epic {epic_id}: {e}")),
                    data: None,
                })?
                .into_iter()
                .map(|task| task.id)
                .collect::<std::collections::HashSet<_>>();
            // list_blocked is the canonical externally-aware view.
            task_store
                .list_blocked()
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to list blocked: {e}")),
                    data: None,
                })?
                .into_iter()
                .filter(|(task, _)| subtask_ids.contains(&task.id))
                .collect()
        } else {
            task_store.list_blocked().map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to list blocked: {e}")),
                data: None,
            })?
        };

        let project_id = super::current_project_id(&self.cas_root);
        let hidden_foreign = if req.include_foreign {
            0
        } else {
            blocked
                .iter()
                .filter(|(task, _)| !super::task_visible_in_project(task, project_id.as_deref()))
                .count()
        };
        if !req.include_foreign {
            blocked.retain(|(task, _)| super::task_visible_in_project(task, project_id.as_deref()));
        }

        // Apply sorting to the task field of each tuple. cas-06f9 (GH #104):
        // same defaulting as `ready` — this is the same triage surface, and it
        // carried the identical silent cap.
        let sort_opts = crate::mcp::tools::ready_blocked_sort_options(
            req.sort.as_deref(),
            req.sort_order.as_deref(),
        );
        sort_blocked_tasks(&mut blocked, &sort_opts);

        if blocked.is_empty() {
            let msg = if req.epic.is_some() {
                "No blocked tasks in this epic"
            } else {
                "No blocked tasks"
            };
            let mut output = msg.to_string();
            if let Some(footer) = super::foreign_tasks_hidden_footer(hidden_foreign) {
                output.push_str("\n");
                output.push_str(&footer);
            }
            return Ok(Self::success(output));
        }

        let limit = req.limit.unwrap_or(10);
        let total = blocked.len();
        let shown = total.min(limit);
        let mut output =
            crate::mcp::tools::truncated_list_header("Blocked tasks", total, shown, &sort_opts);
        for (task, blockers) in blocked.iter().take(limit) {
            let blocker_ids: Vec<_> = blockers.iter().map(|t| t.id.as_str()).collect();
            let external = cas_store::ExternalTaskDependencyStore::open(&self.cas_root)
                .and_then(|store| store.list_blocking_for_task(&task.id))
                .unwrap_or_default();
            let mut rendered_blockers = blocker_ids
                .into_iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            rendered_blockers.extend(external.iter().map(|dependency| {
                format!(
                    "{} [external: {}]",
                    dependency.target_task_id, dependency.resolution_state
                )
            }));
            output.push_str(&format!(
                "- [{}] P{} {} - {}\n  Blocked by: {}\n",
                task.id,
                task.priority.0,
                task.task_type,
                task.title,
                rendered_blockers.join(", ")
            ));
        }
        output.push_str(&crate::mcp::tools::truncated_list_footer(total, shown));
        if let Some(footer) = super::foreign_tasks_hidden_footer(hidden_foreign) {
            output.push_str("\n");
            output.push_str(&footer);
        }

        Ok(Self::success(output))
    }

    /// Update a task
    pub async fn cas_task_list(
        &self,
        Parameters(req): Parameters<TaskListRequest>,
    ) -> Result<CallToolResult, McpError> {
        use cas_types::TaskSortOptions;

        let scope = req.scope.trim().to_ascii_lowercase();
        if scope == "global" {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Global task scope is unsupported. Tasks belong to the current Cassy project database.",
            ));
        }
        if !matches!(scope.as_str(), "project" | "all") {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Task scope must be `project` or `all`; global tasks are unsupported.",
            ));
        }
        let project_id = super::current_project_id(&self.cas_root);

        let task_store = self.open_task_store()?;

        // If epic filter is specified, get subtasks of that epic instead of all tasks
        let tasks = if let Some(ref epic_id) = req.epic {
            task_store.get_subtasks(epic_id).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to get subtasks for epic {epic_id}: {e}")),
                data: None,
            })?
        } else {
            task_store.list(None).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to list: {e}")),
                data: None,
            })?
        };

        // Apply filters. Count foreign rows after the user-supplied filters so
        // the footer describes rows hidden by this exact query.
        let mut filtered: Vec<_> = tasks
            .into_iter()
            .filter(|task| {
                // Status filter — use Display (snake_case) for matching so
                // "awaiting_merge", "in_progress", etc. all round-trip
                // correctly. Previously used Debug (PascalCase) which would not
                // match snake_case filter strings for multi-word status values.
                if let Some(ref status_filter) = req.status {
                    let task_status = task.status.to_string(); // snake_case via Display
                    if !task_status.contains(&status_filter.to_lowercase()) {
                        return false;
                    }
                }
                // Label filter
                if let Some(ref label_filter) = req.label {
                    if !task
                        .labels
                        .iter()
                        .any(|l| l.to_lowercase().contains(&label_filter.to_lowercase()))
                    {
                        return false;
                    }
                }
                // Assignee filter
                if let Some(ref assignee_filter) = req.assignee {
                    match &task.assignee {
                        Some(a) if a.to_lowercase().contains(&assignee_filter.to_lowercase()) => {}
                        _ => return false,
                    }
                }
                // Task type filter
                if let Some(ref type_filter) = req.task_type {
                    let task_type_str = task.task_type.to_string().to_lowercase();
                    if task_type_str != type_filter.to_lowercase() {
                        return false;
                    }
                }
                true
            })
            .collect();
        let hidden_foreign = if req.include_foreign {
            0
        } else {
            filtered
                .iter()
                .filter(|task| !super::task_visible_in_project(task, project_id.as_deref()))
                .count()
        };
        if !req.include_foreign {
            filtered.retain(|task| super::task_visible_in_project(task, project_id.as_deref()));
        }

        // Apply sorting
        let sort_opts =
            TaskSortOptions::from_params(req.sort.as_deref(), req.sort_order.as_deref());
        sort_tasks(&mut filtered, &sort_opts);

        let scope_note = if scope == "all" {
            format!(
                "Scope: all (currently equivalent to the current project database `{}`; no multi-project aggregator exists)",
                project_id
                    .as_deref()
                    .unwrap_or("unresolved current project")
            )
        } else {
            format!(
                "Scope: project `{}` (current Cassy database)",
                project_id
                    .as_deref()
                    .unwrap_or("unresolved current project")
            )
        };
        if filtered.is_empty() {
            let mut output = format!("No tasks found matching filters.\n{scope_note}.");
            if let Some(footer) = super::foreign_tasks_hidden_footer(hidden_foreign) {
                output.push_str("\n");
                output.push_str(&footer);
            }
            return Ok(Self::success(output));
        }

        let limit = req.limit.unwrap_or(20);
        let mut output = format!(
            "Tasks ({} total, showing {})\n{}:\n\n",
            filtered.len(),
            filtered.len().min(limit),
            scope_note,
        );
        for task in filtered.iter().take(limit) {
            // cas-a844: flag a conflicted awaiting_merge distinctly from a
            // clean one right in the list view, not just on `show`.
            let conflict_marker =
                if task.status == TaskStatus::AwaitingMerge && task.deliverables.merge_conflicted {
                    " [MERGE CONFLICT/REWORK]"
                } else {
                    ""
                };
            let outcome_marker = if task.status == TaskStatus::Cancelled {
                " [NO DELIVERY]"
            } else {
                ""
            };
            output.push_str(&format!(
                "- [{}] {:?}{}{} P{} {} - {}\n",
                task.id,
                task.status,
                conflict_marker,
                outcome_marker,
                task.priority.0,
                task.task_type,
                task.title
            ));
        }

        if filtered.len() > limit {
            output.push_str(&format!("\n... and {} more", filtered.len() - limit));
        }
        if let Some(footer) = super::foreign_tasks_hidden_footer(hidden_foreign) {
            output.push_str("\n");
            output.push_str(&footer);
        }

        Ok(Self::success(output))
    }
}
