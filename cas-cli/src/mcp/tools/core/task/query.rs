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

        let mut output = format!(
            "Task: {}\n{}\n\nTitle: {}\nStatus: {:?}\nPriority: P{}\nType: {}\nDepth: {}\n",
            task.id,
            "=".repeat(task.id.len() + 6),
            task.title,
            task.status,
            task.priority.0,
            task.task_type,
            task.depth
        );

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
            let comment_task_id = req.id.clone();
            let comments = tokio::task::spawn_blocking(move || {
                crate::cloud::comments::fetch_task_comments(&comment_task_id)
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
            output.push_str(&format!("\nClosed: {}", closed.format("%Y-%m-%d %H:%M")));
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
                        let open_count = subtasks
                            .iter()
                            .filter(|t| t.status != TaskStatus::Closed)
                            .count();
                        let closed_count = subtasks.len() - open_count;
                        output.push_str(&format!(
                            "\n\nSubtasks ({}/{} complete):\n",
                            closed_count,
                            subtasks.len()
                        ));
                        for subtask in &subtasks {
                            let status_icon = match subtask.status {
                                TaskStatus::Open => "○",
                                TaskStatus::InProgress => "●",
                                TaskStatus::Blocked => "◉",
                                TaskStatus::Closed => "✓",
                                // cas-b51a: awaiting supervisor code-review
                                TaskStatus::PendingSupervisorReview => "⏳",
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
        use cas_types::TaskSortOptions;

        let task_store = self.open_task_store()?;

        // If epic filter specified, get subtasks and filter to blocked ones
        let mut blocked: Vec<(cas_types::Task, Vec<cas_types::Task>)> =
            if let Some(ref epic_id) = req.epic {
                let subtasks = task_store.get_subtasks(epic_id).map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to get subtasks for epic {epic_id}: {e}")),
                    data: None,
                })?;
                // Filter to blocked tasks and get their blockers
                subtasks
                    .into_iter()
                    .filter_map(|t| {
                        if t.status == cas_types::TaskStatus::Blocked {
                            let blockers = task_store.get_blockers(&t.id).unwrap_or_default();
                            Some((t, blockers))
                        } else {
                            None
                        }
                    })
                    .collect()
            } else {
                task_store.list_blocked().map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to list blocked: {e}")),
                    data: None,
                })?
            };

        // Apply sorting to the task field of each tuple
        let sort_opts =
            TaskSortOptions::from_params(req.sort.as_deref(), req.sort_order.as_deref());
        sort_blocked_tasks(&mut blocked, &sort_opts);

        if blocked.is_empty() {
            let msg = if req.epic.is_some() {
                "No blocked tasks in this epic"
            } else {
                "No blocked tasks"
            };
            return Ok(Self::success(msg));
        }

        let limit = req.limit.unwrap_or(10);
        let mut output = format!("Blocked tasks ({}):\n\n", blocked.len().min(limit));
        for (task, blockers) in blocked.iter().take(limit) {
            let blocker_ids: Vec<_> = blockers.iter().map(|t| t.id.as_str()).collect();
            output.push_str(&format!(
                "- [{}] P{} {} - {}\n  Blocked by: {}\n",
                task.id,
                task.priority.0,
                task.task_type,
                task.title,
                blocker_ids.join(", ")
            ));
        }

        Ok(Self::success(output))
    }

    /// Update a task
    pub async fn cas_task_list(
        &self,
        Parameters(req): Parameters<TaskListRequest>,
    ) -> Result<CallToolResult, McpError> {
        use cas_types::TaskSortOptions;

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

        // Apply filters
        let mut filtered: Vec<_> = tasks
            .into_iter()
            .filter(|task| {
                // Status filter — use Display (snake_case) for matching so
                // "pending_supervisor_review", "in_progress", etc. all round-trip
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

        // Apply sorting
        let sort_opts =
            TaskSortOptions::from_params(req.sort.as_deref(), req.sort_order.as_deref());
        sort_tasks(&mut filtered, &sort_opts);

        if filtered.is_empty() {
            return Ok(Self::success("No tasks found matching filters"));
        }

        let limit = req.limit.unwrap_or(20);
        let mut output = format!(
            "Tasks ({} total, showing {}):\n\n",
            filtered.len(),
            filtered.len().min(limit)
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
            output.push_str(&format!(
                "- [{}] {:?}{} P{} {} - {}\n",
                task.id, task.status, conflict_marker, task.priority.0, task.task_type, task.title
            ));
        }

        if filtered.len() > limit {
            output.push_str(&format!("\n... and {} more", filtered.len() - limit));
        }

        Ok(Self::success(output))
    }
}
