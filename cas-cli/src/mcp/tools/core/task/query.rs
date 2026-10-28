use crate::mcp::tools::core::imports::*;

fn retain_list_tasks(
    tasks: &mut Vec<Task>,
    filter: &str,
    predicate: impl FnMut(&Task) -> bool,
    exclusions: &mut Vec<String>,
) {
    let before = tasks.len();
    tasks.retain(predicate);
    let removed = before - tasks.len();
    if removed > 0 {
        exclusions.push(format!("{filter}: {removed}"));
    }
}

fn append_list_filter_report(
    output: &mut String,
    exclusions: &[String],
    quarantined: usize,
    foreign: usize,
) {
    if !exclusions.is_empty() {
        output.push_str(&format!(
            "\nFilters excluded (applied in order): {}",
            exclusions.join("; ")
        ));
    }
    if quarantined > 0 {
        output.push_str(&format!("\n{quarantined} quarantined tasks hidden (local quarantine ledger; task show by id remains available)"));
    }
    if let Some(footer) = super::foreign_tasks_hidden_footer(foreign) {
        output.push('\n');
        output.push_str(&footer);
    }
}

/// Keep short history intact; for a large timeline retain the latest decision,
/// proof and checkpoint, each with a byte cursor into the original notes.
/// Full reads remain the source of truth, including attached artifact receipts.
pub(super) fn task_show_notes(notes: &str, task_id: &str) -> String {
    if notes.len() <= 700 {
        return notes.to_string();
    }
    let mut starts = vec![0];
    starts.extend(notes.match_indices("\n\n[").map(|(i, _)| i + 2));
    let entries: Vec<_> = starts
        .iter()
        .enumerate()
        .map(|(i, start)| {
            (
                *start,
                notes[*start..starts.get(i + 1).copied().unwrap_or(notes.len())].trim_end(),
            )
        })
        .collect();
    let mut selected = Vec::new();
    for marker in ["DECISION", "PROOF"] {
        if let Some((offset, text)) = entries
            .iter()
            .rev()
            .find(|(_, text)| text.lines().next().unwrap_or("").contains(marker))
        {
            selected.push((*offset, *text));
        }
    }
    if let Some(last) = entries.last() {
        selected.push(*last);
    }
    selected.sort_by_key(|(offset, _)| *offset);
    selected.dedup_by_key(|(offset, _)| *offset);
    let mut result = String::new();
    let header_bytes = format!(
        "Notes for task {}\n{}\n\n",
        task_id,
        "=".repeat(task_id.len() + 15)
    )
    .len();
    for (offset, text) in selected {
        result.push_str(&format!(
            "[notes offset={}] {}\n",
            header_bytes + offset,
            excerpt(text, 100)
        ));
    }
    if let Some(id) = notes
        .lines()
        .rev()
        .filter_map(|line| line.strip_prefix("ARTIFACT_PROOF: "))
        .find_map(|json| {
            serde_json::from_str::<serde_json::Value>(json)
                .ok()
                .and_then(|value| {
                    value
                        .get("id")
                        .and_then(|id| id.as_str())
                        .map(str::to_string)
                })
        })
    {
        result.push_str(&format!("Latest artifact: {}\n", excerpt(&id, 80)));
    }
    result.push_str(&format!(
        "History: {} bytes; task notes id={task_id} full=true",
        notes.len()
    ));
    result
}

pub(super) fn excerpt(text: &str, budget: usize) -> String {
    if text.len() <= budget {
        return text.to_string();
    }
    let mut end = budget.saturating_sub(3).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

pub(super) fn read_hint(id: &str, action: &str, clipped: bool) -> String {
    let warning = if clipped {
        "\nRead clipped instructions before acting."
    } else {
        ""
    };
    format!(
        "{warning}\nFull: task {action} id={id} full=true\nPage: task {action} id={id} offset=0 limit=1600"
    )
}

/// The summary and navigation together fit the response budget. A page is a
/// slice of the complete renderer, so following next= is lossless even in UTF-8.
pub(super) fn render_task_read(
    full: &str,
    summary: &str,
    id: &str,
    action: &str,
    read: &TaskReadOptions,
) -> Result<String, McpError> {
    if read.full {
        return Ok(full.to_string());
    }
    if read.offset.is_some() || read.limit.is_some() {
        let start = read.offset.unwrap_or(0);
        if start > full.len() || !full.is_char_boundary(start) {
            return Err(CasCore::error(
                ErrorCode::INVALID_PARAMS,
                "offset must be a UTF-8 boundary within the full text; follow next= from the previous page",
            ));
        }
        let budget = read.limit.unwrap_or(1600).clamp(4, 1600);
        let mut end = start.saturating_add(budget).min(full.len());
        while !full.is_char_boundary(end) {
            end -= 1;
        }
        let next = if end == full.len() {
            "end".into()
        } else {
            end.to_string()
        };
        return Ok(format!(
            "Task {action} bytes {start}..{end}/{} next={next}\n\n{}",
            full.len(),
            &full[start..end]
        ));
    }
    // Reserve the longer warning before deciding whether the final budget clips.
    let reserve = read_hint(id, action, true).len();
    let hint = read_hint(
        id,
        action,
        summary.contains('…') || summary.len() > 2000 - reserve,
    );
    Ok(format!(
        "{}{}",
        excerpt(summary, 2000usize.saturating_sub(hint.len())),
        hint
    ))
}

pub(super) fn task_scope(task: &Task) -> String {
    let risk = task
        .risk
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Execution Note: {}\nDescription: {}\nAcceptance Criteria: {}\nProof Targets: {}\nRisk: {}\nDoor: {} (recorded only)\n",
        excerpt(task.execution_note.as_deref().unwrap_or("(none)"), 40),
        excerpt(&task.description, 100),
        excerpt(&task.acceptance_criteria, 80),
        excerpt(&task.proof_targets.join(", "), 40),
        excerpt(&risk, 40),
        task.door
            .map(|door| door.to_string())
            .unwrap_or_else(|| "undeclared".into())
    )
}

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

        let mut read_status = String::new();
        let mut batch_summary = String::new();

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
        let foreign_origin = project_id.is_some()
            && task.origin_project.is_some()
            && !super::task_owned_by_project(&task, &self.cas_root, project_id.as_deref());
        let origin = if foreign_origin {
            format!("{origin} — this task is owned elsewhere")
        } else {
            origin.to_string()
        };

        let mut output = format!(
            "Task: {}\n{}\n\nTitle: {}\nStatus: {:?}\nAssignee: {}\nPriority: P{}\nType: {}\nDepth: {}\nDelivery mode: {}\nOrigin project: {}\nTarget: {}\n",
            task.id,
            "=".repeat(task.id.len() + 6),
            task.title,
            task.status,
            task.assignee.as_deref().unwrap_or("(unassigned)"),
            task.priority.0,
            task.task_type,
            task.depth,
            task.delivery_mode,
            origin,
            target
        );
        // cas-ea9c (GH #1005): cited issues attached at assignment.
        if let Some(section) =
            crate::github_issue_attach::cited_issue_section(&self.cas_root, &task)
        {
            output.push_str(&format!("\n{section}\n"));
        }

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
            read_status.push_str(&format!(
                "Structured execution state: {}\n",
                excerpt(&encoded, 180)
            ));
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
                read_status.insert_str(
                    0,
                    &format!(
                        "Transactional delivery: {}\nReceipt: {}\nCommit: {}\n",
                        transaction.state, receipt.id, receipt.commit_sha
                    ),
                );
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

        // cas-f146: a supervisor-spawned task-verifier records its verdict on
        // the supervisor's identity, which must name the exact dispatch. Its
        // instructions read the id from this line.
        if let Ok(Some(dispatch)) =
            cas_store::get_latest_verification_dispatch(&self.cas_root, &task.id)
            && matches!(
                dispatch.state,
                cas_types::VerificationDispatchState::Pending
                    | cas_types::VerificationDispatchState::Claimed
            )
        {
            read_status.insert_str(0, &format!("Verification dispatch: {}\n", dispatch.id));
            output.push_str(&format!(
                "\nVerification dispatch: {} (deadline {})\n",
                dispatch.id,
                dispatch.deadline_at.to_rfc3339()
            ));
        }

        // cas-a844: `awaiting_merge` alone reads as "done, just a formality" —
        // whether that's true depends entirely on whether the parked branch
        // can actually merge cleanly. Surface the distinction explicitly so
        // a conflicted parked task never looks indistinguishable from a
        // clean one queued for the supervisor.
        if task.status == TaskStatus::AwaitingMerge {
            if task.deliverables.merge_conflicted {
                read_status.insert_str(0, "MERGE CONFLICT / REWORK REQUIRED\n");
                output.push_str(
                    "⚠️  MERGE CONFLICT / REWORK REQUIRED — this task is NOT complete. \
                     The parked branch is conflicted or its conflict preflight could \
                     not be evaluated; the assigned worker can `task start` it to \
                     inspect from inside the worker worktree, rebase the parked \
                     factory branch onto the current integration target tip, push, \
                     and re-park with merge_request=true.\n",
                );
            } else {
                output.push_str(
                    "(mergeable — queued for the supervisor to merge the factory branch)\n",
                );
            }
            if let Some(ref branch) = task.deliverables.parked_branch {
                read_status.insert_str(0, &format!("Parked branch: {}\n", excerpt(branch, 100)));
                output.push_str(&format!("Parked branch: {branch}\n"));
            }
        }

        if let Some(batch) = task.deliverables.integration_batch.as_ref() {
            batch_summary = format!(
                "{}: {}@{}\n",
                if task.status == TaskStatus::AwaitingMerge {
                    "Staged in integration batch"
                } else {
                    "Integration batch receipt"
                },
                excerpt(&batch.branch, 60),
                batch.tip
            );

            output.push_str(&format!(
                "{}: {}@{} (base {}, delivery {}, supervisor {})\n",
                if task.status == TaskStatus::AwaitingMerge {
                    "Staged in integration batch"
                } else {
                    "Integration batch receipt"
                },
                batch.branch,
                batch.tip,
                batch.base,
                batch.delivered_head,
                batch.supervisor_id
            ));
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
        if req.read.full || req.read.offset.is_some() || req.read.limit.is_some() {
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

        if !task.risk.is_empty() {
            let risks = task
                .risk
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            output.push_str(&format!("\nRisk: {}\n", risks.join(", ")));
        } else {
            output.push_str("\nRisk: undeclared\n");
        }
        output.push_str(&format!(
            "Door: {} (recorded only)\n",
            task.door
                .map(|door| door.to_string())
                .unwrap_or_else(|| "undeclared".into())
        ));
        if !task.proof_targets.is_empty() {
            output.push_str(&format!(
                "Proof Targets: {}\n",
                task.proof_targets.join(", ")
            ));
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
                cas_types::TaskTerminalOutcome::EvidenceOnly => {
                    output.push_str(
                        "\nOutcome: successful evidence-only delivery (not for integration)",
                    );
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
            if let Some(merge) = task
                .deliverables
                .merge_commit
                .as_ref()
                .or(task.deliverables.green_merge_commit.as_ref())
            {
                output.push_str(&format!("\n  Merge commit: {merge}"));
            }
            if task.deliverables.green_merge_commit.is_some()
                || task.deliverables.merge_ci_admission.is_some()
            {
                read_status.insert_str(
                    0,
                    &format!(
                        "CI admission: {}\n",
                        task.deliverables.effective_merge_ci_admission().label()
                    ),
                );
                output.push_str(&format!(
                    "\n  CI admission: {}",
                    task.deliverables.effective_merge_ci_admission().label()
                ));
            }
        }

        if req.with_deps {
            if let Ok(deps) = task_store.get_dependencies(&req.id) {
                let blocked_by: Vec<String> = deps
                    .iter()
                    .filter(|dep| dep.dep_type == DependencyType::Blocks)
                    .map(|dep| dep.to_id.clone())
                    .collect();
                let requires_start: Vec<String> = deps
                    .iter()
                    .filter(|dep| dep.dep_type == DependencyType::RequiresStart)
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
                            && dep.dep_type != DependencyType::RequiresStart
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
                    || !requires_start.is_empty()
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
                if !requires_start.is_empty() {
                    output.push_str(&format!(
                        "  - Requires start before this task: {}\n",
                        requires_start.join(", ")
                    ));
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

        let origin_summary = if foreign_origin {
            format!(
                "{} — this task is owned elsewhere",
                excerpt(task.origin_project.as_deref().unwrap_or(""), 40)
            )
        } else {
            excerpt(&origin, 40)
        };
        let header = format!(
            "Task: {} - {}\nStatus: {:?}\nAssignee: {}\nOrigin project: {}\nPriority: P{}; Depth: {}; Delivery mode: {}\nTarget: {}\n",
            task.id,
            excerpt(&task.title, 60),
            task.status,
            excerpt(task.assignee.as_deref().unwrap_or("(unassigned)"), 40),
            origin_summary,
            task.priority.0,
            task.depth,
            task.delivery_mode,
            excerpt(&target, 80),
        );
        // Builtin verifier/epic flows consume Type and Demo from bare show.
        // Name full-only sections so callers cannot silently infer absence.
        let summary = format!(
            "Type: {}\nDemo: {}\nOmitted from summary (full=true): Design, Deliverables (files/commit/merge), Dependencies/Epic/Subtasks, Outcome/Terminal reason/Superseded by, worktree.\n{}\n{}{}{}\nNotes:\n{}",
            task.task_type,
            excerpt(&task.demo_statement, 160),
            excerpt(&header, 400),
            task_scope(&task),
            excerpt(&read_status, 200),
            batch_summary,
            task_show_notes(&task.notes, &task.id)
        );
        Ok(Self::success(render_task_read(
            &output, &summary, &task.id, "show", &req.read,
        )?))
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

        let project_scope = super::ProjectScope::current(&self.cas_root);
        let hidden_foreign = if req.include_foreign {
            0
        } else {
            blocked
                .iter()
                .filter(|(task, _)| !project_scope.shows(task))
                .count()
        };
        if !req.include_foreign {
            blocked.retain(|(task, _)| project_scope.shows(task));
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
        let project_scope = super::ProjectScope::current(&self.cas_root);
        let project_id = project_scope.project_id();

        let task_store = self.open_task_store()?;

        // Preserve suppression metadata through store wrappers. User filters
        // run before quarantine/origin so each count describes this query.
        let (mut filtered, suppressed) =
            task_store
                .list_with_suppressed(None)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to list: {e}")),
                    data: None,
                })?;
        let suppressed_ids: std::collections::BTreeSet<_> =
            suppressed.iter().map(|task| task.id.clone()).collect();
        filtered.extend(suppressed);
        let mut exclusions = Vec::new();
        if let Some(ref epic_id) = req.epic {
            let subtasks = task_store.get_subtasks(epic_id).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to get subtasks for epic {epic_id}: {e}")),
                data: None,
            })?;
            let ids: std::collections::BTreeSet<_> = subtasks.iter().map(|task| &task.id).collect();
            retain_list_tasks(
                &mut filtered,
                &format!("epic={epic_id}"),
                |task| ids.contains(&task.id),
                &mut exclusions,
            );
        }
        if let Some(ref status) = req.status {
            retain_list_tasks(
                &mut filtered,
                &format!("status={status}"),
                |task| task.status.to_string().contains(&status.to_lowercase()),
                &mut exclusions,
            );
        }
        if let Some(ref label) = req.label {
            retain_list_tasks(
                &mut filtered,
                &format!("label={label}"),
                |task| {
                    task.labels
                        .iter()
                        .any(|value| value.to_lowercase().contains(&label.to_lowercase()))
                },
                &mut exclusions,
            );
        }
        if let Some(ref assignee) = req.assignee {
            retain_list_tasks(
                &mut filtered,
                &format!("assignee={assignee}"),
                |task| {
                    task.assignee.as_ref().is_some_and(|value| {
                        value.to_lowercase().contains(&assignee.to_lowercase())
                    })
                },
                &mut exclusions,
            );
        }
        if let Some(ref task_type) = req.task_type {
            retain_list_tasks(
                &mut filtered,
                &format!("type={task_type}"),
                |task| task.task_type.to_string().eq_ignore_ascii_case(task_type),
                &mut exclusions,
            );
        }
        let before_quarantine = filtered.len();
        filtered.retain(|task| !suppressed_ids.contains(&task.id));
        let hidden_quarantined = before_quarantine - filtered.len();
        let hidden_foreign = if req.include_foreign {
            0
        } else {
            filtered
                .iter()
                .filter(|task| !project_scope.shows(task))
                .count()
        };
        if !req.include_foreign {
            filtered.retain(|task| project_scope.shows(task));
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
            append_list_filter_report(&mut output, &exclusions, hidden_quarantined, hidden_foreign);
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
        append_list_filter_report(&mut output, &exclusions, hidden_quarantined, hidden_foreign);

        Ok(Self::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list_request() -> TaskListRequest {
        TaskListRequest {
            limit: Some(500),
            scope: "project".into(),
            status: Some("open".into()),
            task_type: None,
            label: None,
            assignee: None,
            epic: None,
            sort: None,
            sort_order: None,
            include_foreign: false,
        }
    }

    fn list_text(result: CallToolResult) -> String {
        let rmcp::model::RawContent::Text(text) = &result.content[0].raw else {
            panic!("task list must return text")
        };
        text.text.clone()
    }

    fn listed_ids(text: &str) -> std::collections::BTreeSet<String> {
        text.lines()
            .filter_map(|line| line.strip_prefix("- ["))
            .map(|line| line.split(']').next().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn cas_4bd8_open_list_accounts_for_quarantined_mixed_rows_and_release() {
        use cas_types::TaskType;
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        let store = core.open_task_store().unwrap();
        let queue = crate::cloud::SyncQueue::open(&root).unwrap();
        queue.init().unwrap();
        for index in 0..225 {
            let mut task =
                cas_types::Task::new(format!("cas-row{index:03}"), "Mixed fixture".into());
            task.task_type = [
                TaskType::Epic,
                TaskType::Chore,
                TaskType::Task,
                TaskType::Bug,
                TaskType::Feature,
            ][index % 5];
            store
                .create_atomic(&task, &[], (index > 0).then_some("cas-row000"), None)
                .unwrap();
            if index < 30 {
                queue
                    .quarantine_row(
                        crate::cloud::QUARANTINE_TASK,
                        &task.id,
                        "fixture suppression",
                    )
                    .unwrap();
            }
        }
        let conn = rusqlite::Connection::open(&root.join("cas.db")).unwrap();
        let expected = conn
            .prepare("SELECT id FROM tasks WHERE status='open'")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .unwrap();
        assert_eq!(expected.len(), 225);
        let hidden = queue
            .quarantined_ids(crate::cloud::QUARANTINE_TASK)
            .unwrap();
        let text = list_text(
            core.cas_task_list(Parameters(list_request()))
                .await
                .unwrap(),
        );
        assert_eq!(
            listed_ids(&text),
            expected.difference(&hidden).cloned().collect()
        );
        assert!(text.contains("30 quarantined tasks hidden"), "{text}");
        let mut limited = list_request();
        limited.limit = Some(5);
        let limited_text = list_text(core.cas_task_list(Parameters(limited)).await.unwrap());
        assert_eq!(listed_ids(&limited_text).len(), 5);
        assert!(limited_text.contains("195 total, showing 5"));
        assert!(limited_text.contains("... and 190 more"));
        assert!(limited_text.contains("30 quarantined tasks hidden"));
        let mut epic_request = list_request();
        epic_request.epic = Some("cas-row000".into());
        let epic_text = list_text(core.cas_task_list(Parameters(epic_request)).await.unwrap());
        assert_eq!(listed_ids(&epic_text), listed_ids(&text));
        assert!(epic_text.contains("epic=cas-row000: 1"), "{epic_text}");
        assert!(
            epic_text.contains("29 quarantined tasks hidden"),
            "{epic_text}"
        );
        assert_eq!(store.get("cas-row007").unwrap().status, TaskStatus::Open);
        assert!(
            queue
                .release_quarantined_row(crate::cloud::QUARANTINE_TASK, "cas-row007")
                .unwrap()
        );
        let text = list_text(
            core.cas_task_list(Parameters(list_request()))
                .await
                .unwrap(),
        );
        assert_eq!(listed_ids(&text).len(), 196);
        assert!(listed_ids(&text).contains("cas-row007"));
        assert!(text.contains("29 quarantined tasks hidden"), "{text}");
        for id in hidden {
            queue
                .release_quarantined_row(crate::cloud::QUARANTINE_TASK, &id)
                .unwrap();
        }
        let text = list_text(
            core.cas_task_list(Parameters(list_request()))
                .await
                .unwrap(),
        );
        assert_eq!(listed_ids(&text), expected);
        assert!(!text.contains("quarantined tasks hidden"));
    }

    #[tokio::test]
    async fn cas_4bd8_filters_count_only_matching_quarantine_and_empty_results() {
        use cas_types::TaskType;
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        let store = core.open_task_store().unwrap();
        let queue = crate::cloud::SyncQueue::open(&root).unwrap();
        queue.init().unwrap();
        for id in [
            "keep",
            "hidden",
            "hidden-closed",
            "foreign",
            "closed",
            "label",
            "assignee",
            "type",
        ] {
            let mut task = Task::new(format!("cas-{id}"), "Filter fixture".into());
            task.task_type = if id == "type" {
                TaskType::Chore
            } else {
                TaskType::Bug
            };
            task.status = if id.ends_with("closed") {
                TaskStatus::Closed
            } else {
                TaskStatus::Open
            };
            task.labels = vec![
                if id == "label" || id == "closed" {
                    "other"
                } else {
                    "keep"
                }
                .into(),
            ];
            task.assignee = Some(if id == "assignee" { "other" } else { "owner" }.into());
            if id == "foreign" {
                task.origin_project = Some("another-project".into());
            }
            store.add(&task).unwrap();
            if id.starts_with("hidden") {
                queue
                    .quarantine_row(crate::cloud::QUARANTINE_TASK, &task.id, "fixture")
                    .unwrap();
            }
        }
        let mut req = list_request();
        req.label = Some("keep".into());
        req.assignee = Some("owner".into());
        req.task_type = Some("bug".into());
        let text = list_text(core.cas_task_list(Parameters(req)).await.unwrap());
        assert_eq!(
            listed_ids(&text),
            ["cas-keep".to_string()].into_iter().collect()
        );
        assert!(
            text.contains("status=open: 2; label=keep: 1; assignee=owner: 1; type=bug: 1"),
            "{text}"
        );
        assert!(text.contains("1 quarantined tasks hidden"), "{text}");
        assert!(text.contains("1 foreign-origin tasks hidden"), "{text}");

        let mut req = list_request();
        req.include_foreign = true;
        req.scope = "all".into();
        let text = list_text(core.cas_task_list(Parameters(req)).await.unwrap());
        assert!(listed_ids(&text).contains("cas-foreign"));
        assert!(!text.contains("foreign-origin tasks hidden"));
        assert!(text.contains("1 quarantined tasks hidden"));
        assert!(text.contains("Scope: all (currently equivalent"));

        let mut req = list_request();
        req.status = Some("closed".into());
        req.label = Some("keep".into());
        let text = list_text(core.cas_task_list(Parameters(req)).await.unwrap());
        assert!(listed_ids(&text).is_empty());
        assert!(text.contains("No tasks found matching filters."));
        assert!(text.contains("1 quarantined tasks hidden"), "{text}");
    }

    #[test]
    fn task_show_notes_leaves_short_history_without_paging_line() {
        let rendered = task_show_notes("first\n\nsecond", "cas-9568");
        assert_eq!(rendered, "first\n\nsecond");
    }

    #[test]
    fn task_show_notes_preserves_blank_lines_inside_a_note() {
        let notes = "[2026-09-10 12:00] 📝 PROGRESS first paragraph\n\nsecond paragraph\n\n\
[2026-09-10 12:01] 📝 PROGRESS latest";
        let rendered = task_show_notes(notes, "cas-9568");
        assert!(rendered.contains("first paragraph\n\nsecond paragraph"));
        assert!(rendered.contains("latest"));
    }
}
