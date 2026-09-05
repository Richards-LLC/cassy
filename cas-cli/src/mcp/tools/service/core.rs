use crate::mcp::tools::service::imports::*;

impl CasService {
    // Memory implementations
    pub(super) async fn memory_remember(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::RememberRequest;
        let inner_req = RememberRequest {
            content: req.content.ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, "content required for remember")
            })?,
            entry_type: req.entry_type.unwrap_or_else(|| "learning".to_string()),
            tags: req.tags,
            title: req.title,
            importance: req.importance.unwrap_or(0.5),
            scope: req.scope.unwrap_or_else(|| "project".to_string()),
            valid_from: req.valid_from,
            valid_until: req.valid_until,
            team_id: req.team_id,
            bypass_overlap: req.bypass_overlap,
            mode: req.mode.clone(),
            expected_updated_at: req.expected_updated_at,
            personal: req.personal,
        };
        self.inner.cas_remember(Parameters(inner_req)).await
    }

    pub(super) async fn memory_get(&self, req: MemoryRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for get — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=get id=cas-abc1"))?,
        };
        self.inner.cas_get(Parameters(inner_req)).await
    }

    pub(super) async fn memory_list(&self, req: MemoryRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::MemoryListRequest;
        let inner_req = MemoryListRequest {
            limit: req.limit,
            scope: req.scope.unwrap_or_else(|| "all".to_string()),
            tags: req.tags,
            tier: req.tier,
            sort: req.sort,
            sort_order: req.sort_order,
            team_id: req.team_id,
        };
        self.inner.cas_list(Parameters(inner_req)).await
    }

    pub(super) async fn memory_update(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::EntryUpdateRequest;
        let inner_req = EntryUpdateRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for update — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=update id=cas-abc1"))?,
            content: req.content,
            tags: req.tags,
            importance: req.importance,
        };
        self.inner.cas_update(Parameters(inner_req)).await
    }

    pub(super) async fn memory_delete(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for delete — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=delete id=cas-abc1"))?,
        };
        self.inner.cas_delete(Parameters(inner_req)).await
    }

    pub(super) async fn memory_archive(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for archive — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=archive id=cas-abc1"))?,
        };
        self.inner.cas_archive(Parameters(inner_req)).await
    }

    pub(super) async fn memory_unarchive(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req.id.ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, "id required for unarchive — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=unarchive id=cas-abc1")
            })?,
        };
        self.inner.cas_unarchive(Parameters(inner_req)).await
    }

    pub(super) async fn memory_helpful(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for helpful — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=helpful id=cas-abc1"))?,
        };
        self.inner.cas_helpful(Parameters(inner_req)).await
    }

    pub(super) async fn memory_harmful(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for harmful — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=harmful id=cas-abc1"))?,
        };
        self.inner.cas_harmful(Parameters(inner_req)).await
    }

    pub(super) async fn memory_mark_reviewed(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req.id.ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, "id required for mark_reviewed — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=mark_reviewed id=cas-abc1")
            })?,
        };
        self.inner.cas_mark_reviewed(Parameters(inner_req)).await
    }

    pub(super) async fn memory_recent(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::RecentRequest;
        let inner_req = RecentRequest {
            n: req.limit.unwrap_or(10),
        };
        self.inner.cas_recent(Parameters(inner_req)).await
    }

    pub(super) async fn memory_set_tier(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::MemoryTierRequest;
        let inner_req = MemoryTierRequest {
            id: req.id.ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, "id required for set_tier — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=set_tier id=cas-abc1")
            })?,
            tier: req.tier.ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, "tier required for set_tier")
            })?,
        };
        self.inner.cas_set_tier(Parameters(inner_req)).await
    }

    pub(super) async fn memory_opinion_reinforce(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::OpinionReinforceRequest;
        let inner_req = OpinionReinforceRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
            evidence: req.content.ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, "content (evidence) required")
            })?,
        };
        self.inner
            .cas_opinion_reinforce(Parameters(inner_req))
            .await
    }

    pub(super) async fn memory_opinion_weaken(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::OpinionWeakenRequest;
        let inner_req = OpinionWeakenRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
            evidence: req.content.ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, "content (evidence) required")
            })?,
        };
        self.inner.cas_opinion_weaken(Parameters(inner_req)).await
    }

    pub(super) async fn memory_opinion_contradict(
        &self,
        req: MemoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::OpinionContradictRequest;
        let inner_req = OpinionContradictRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
            evidence: req.content.ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, "content (evidence) required")
            })?,
        };
        self.inner
            .cas_opinion_contradict(Parameters(inner_req))
            .await
    }

    // Task implementations
    pub(super) async fn task_create(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskCreateRequest;
        let target_repo = req.target_repo.clone();
        let target_branch = req.target_branch.clone();
        let target_project = req.project.clone();
        let blocks_origin_task_id = req.blocks_origin_task_id.clone();
        let proposal_attempt_id = req.proposal_attempt_id.clone();
        let inner_req = TaskCreateRequest {
            title: req.title.ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "title required for create — pass a short descriptive title. \
                     Example: mcp__cas__task action=create title=\"Fix login bug\" priority=1",
                )
            })?,
            description: req.description,
            priority: req.priority.unwrap_or(2),
            task_type: req.task_type.unwrap_or_else(|| "task".to_string()),
            labels: req.labels,
            notes: req.notes,
            blocked_by: req.blocked_by,
            design: req.design,
            acceptance_criteria: req.acceptance_criteria,
            demo_statement: req.demo_statement,
            execution_note: req.execution_note,
            external_ref: req.external_ref,
            assignee: req.assignee,
            epic: req.epic,
            depth: req.depth,
        };
        if let Some(target_project) = target_project {
            return self
                .inner
                .cas_task_proposal_create_with_delivery_mode(
                    inner_req,
                    &target_project,
                    blocks_origin_task_id.as_deref(),
                    proposal_attempt_id.as_deref(),
                    req.delivery_mode.as_deref(),
                )
                .await;
        }
        self.inner
            .cas_task_create_with_delivery_mode(
                inner_req,
                target_repo.as_deref(),
                target_branch.as_deref(),
                req.confirm_warning.unwrap_or(false),
                req.delivery_mode.as_deref(),
            )
            .await
    }

    pub(super) async fn task_proposal_inbox(
        &self,
        req: TaskRequest,
    ) -> Result<CallToolResult, McpError> {
        self.inner
            .cas_task_proposal_inbox(req.project.as_deref().ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "project required for proposal_inbox",
                )
            })?)
            .await
    }

    pub(super) async fn task_proposal_accept(
        &self,
        req: TaskRequest,
    ) -> Result<CallToolResult, McpError> {
        self.inner
            .cas_task_proposal_accept(
                req.proposal_id.as_deref().ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "proposal_id required for proposal_accept",
                    )
                })?,
                req.project.as_deref().ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "project required for proposal_accept",
                    )
                })?,
            )
            .await
    }

    pub(super) async fn task_proposal_reject(
        &self,
        req: TaskRequest,
    ) -> Result<CallToolResult, McpError> {
        self.inner
            .cas_task_proposal_reject(
                req.proposal_id.as_deref().ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "proposal_id required for proposal_reject",
                    )
                })?,
                req.project.as_deref().ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "project required for proposal_reject",
                    )
                })?,
                req.reason.as_deref(),
            )
            .await
    }

    pub(super) async fn task_proposal_reconcile(
        &self,
        req: TaskRequest,
    ) -> Result<CallToolResult, McpError> {
        self.inner
            .cas_task_proposal_reconcile(req.project.as_deref().ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "project required for proposal_reconcile",
                )
            })?)
            .await
    }

    pub(super) async fn task_show(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskShowRequest;
        let inner_req = TaskShowRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for show — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=show id=cas-abc1"))?,
            with_deps: req.with_deps.unwrap_or(true),
        };
        self.inner.cas_task_show(Parameters(inner_req)).await
    }

    pub(super) async fn task_update(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskUpdateRequest;
        if req.task_type.is_some() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "TASK UPDATE REJECTED: task_type is create-only and cannot be changed after task creation. No requested fields were applied.",
            ));
        }
        let target_repo = req.target_repo.clone();
        let target_branch = req.target_branch.clone();
        let state_patch = req.state_patch.clone();
        let inner_req = TaskUpdateRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for update — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=update id=cas-abc1"))?,
            title: req.title,
            notes: req.notes,
            priority: req.priority,
            labels: req.labels,
            // cas-d45f (GH #98): blocked_by used to be dropped on the floor here.
            blocked_by: req.blocked_by,
            description: req.description,
            design: req.design,
            acceptance_criteria: req.acceptance_criteria,
            demo_statement: req.demo_statement,
            execution_note: req.execution_note,
            external_ref: req.external_ref,
            assignee: req.assignee,
            origin_project: req.origin_project,
            status: req.status,
            epic: req.epic,
            epic_verification_owner: req.epic_verification_owner,
            depth: req.depth,
        };
        self.inner
            .cas_task_update_with_delivery_mode(
                inner_req,
                target_repo.as_deref(),
                target_branch.as_deref(),
                req.proof_scope_fix.unwrap_or(false),
                req.reason.as_deref(),
                state_patch,
                req.delivery_mode.as_deref(),
            )
            .await
    }

    pub(super) async fn task_start(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskStartRequest;
        let inner_req = TaskStartRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for start — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=start id=cas-abc1"))?,
            brief: req.brief,
        };
        self.inner
            .cas_task_start_with_options(Parameters(inner_req))
            .await
    }

    pub(super) async fn task_close(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskCloseRequest;
        let inline_external_ref = req.external_ref.clone();
        // cas-099d, recurring after GH #272/#294/#304/#333: the unified
        // request schema accepts execution_note on close, so dropping it here
        // made the documented no-code escape discoverable only after exact
        // verification had locked the ordinary update path.
        let inline_execution_note = req.execution_note.clone();
        let effective_supervisor_override = req.effective_supervisor_override();
        let negative_result = req.negative_result.unwrap_or(false).then(|| {
            crate::mcp::tools::NegativeResultCloseRequest {
                artifact_path: req.negative_result_artifact_path,
                reference: req.negative_result_reference,
            }
        });
        let inner_req = TaskCloseRequest {
            // cas-b192: forwarded, not dropped — this is the only path by which
            // the unified `task` tool can reach the epic close override.
            stranded_branch_override: req.stranded_branch_override,
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for close — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=close id=cas-abc1"))?,
            reason: req.reason,
            supervisor_override: effective_supervisor_override,
            legacy_bypass_code_review: req.legacy_bypass_code_review,
            search_manifest: req.search_manifest,
            commit_receipt: req.commit_receipt,
        };
        self.inner
            .cas_task_close_with_completion(
                Parameters(inner_req),
                req.completion_receipt,
                req.external_verification_receipt,
                negative_result,
                inline_external_ref,
                inline_execution_note,
            )
            .await
    }

    pub(super) async fn task_reopen(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskReopenRequest;
        let inner_req = TaskReopenRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for reopen — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=reopen id=cas-abc1"))?,
            reason: req.reason,
        };
        self.inner.cas_task_reopen(Parameters(inner_req)).await
    }

    pub(super) async fn task_cancel(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskCancelRequest;
        let inner_req = TaskCancelRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for cancel"))?,
            reason: req.reason.ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "reason required for cancel — explain why no delivery will be produced",
                )
            })?,
            superseded_by: req.superseded_by,
        };
        self.inner.cas_task_cancel(Parameters(inner_req)).await
    }

    pub(super) async fn task_request_changes(
        &self,
        req: TaskRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskRequestChangesRequest;
        let inner_req = TaskRequestChangesRequest {
            id: req.id.ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "id required for request_changes — pass task ID as `id`",
                )
            })?,
            reason: req.reason.ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "reason required for request_changes — explain what must change and whether prior commits should stand or be reverted",
                )
            })?,
        };
        self.inner
            .cas_task_request_changes(Parameters(inner_req))
            .await
    }

    pub(super) async fn task_delete(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for delete — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=delete id=cas-abc1"))?,
        };
        self.inner.cas_task_delete(Parameters(inner_req)).await
    }

    pub(super) async fn task_list(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskListRequest;
        let inner_req = TaskListRequest {
            limit: req.limit,
            scope: req.scope.unwrap_or_else(|| "all".to_string()),
            status: req.status,
            task_type: req.task_type.clone(),
            label: req.labels, // Use labels field for filtering
            assignee: req.assignee,
            epic: req.epic,
            sort: req.sort.clone(),
            sort_order: req.sort_order.clone(),
            include_foreign: req.include_foreign.unwrap_or(false),
        };
        self.inner.cas_task_list(Parameters(inner_req)).await
    }

    pub(super) async fn task_ready(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskReadyBlockedRequest;
        let inner_req = TaskReadyBlockedRequest {
            limit: req.limit,
            scope: req.scope.unwrap_or_else(|| "all".to_string()),
            sort: req.sort.clone(),
            sort_order: req.sort_order.clone(),
            epic: req.epic,
            include_foreign: req.include_foreign.unwrap_or(false),
        };
        self.inner.cas_task_ready(Parameters(inner_req)).await
    }

    pub(super) async fn task_blocked(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskReadyBlockedRequest;
        let inner_req = TaskReadyBlockedRequest {
            limit: req.limit,
            scope: req.scope.unwrap_or_else(|| "all".to_string()),
            sort: req.sort.clone(),
            sort_order: req.sort_order.clone(),
            epic: req.epic,
            include_foreign: req.include_foreign.unwrap_or(false),
        };
        self.inner.cas_task_blocked(Parameters(inner_req)).await
    }

    pub(super) async fn task_notes(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::{IdRequest, TaskNotesRequest};
        let id = req
            .id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for notes — pass task ID as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=notes id=cas-abc1"))?;

        match req.notes {
            Some(note) => {
                let inner_req = TaskNotesRequest {
                    id,
                    note,
                    note_type: req.note_type.unwrap_or_else(|| "progress".to_string()),
                };
                self.inner.cas_task_notes(Parameters(inner_req)).await
            }
            None => {
                self.inner
                    .cas_task_notes_read(Parameters(IdRequest { id }))
                    .await
            }
        }
    }

    pub(super) async fn task_dep_add(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::DependencyRequest;
        let inner_req = DependencyRequest {
            from_id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
            to_id: req
                .to_id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "to_id required"))?,
            dep_type: req.dep_type.unwrap_or_else(|| "blocks".to_string()),
        };
        self.inner.cas_task_dep_add(Parameters(inner_req)).await
    }

    pub(super) async fn task_dep_remove(
        &self,
        req: TaskRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::DependencyRequest;
        let inner_req = DependencyRequest {
            from_id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
            to_id: req
                .to_id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "to_id required"))?,
            dep_type: req.dep_type.unwrap_or_else(|| "blocks".to_string()),
        };
        self.inner.cas_task_dep_remove(Parameters(inner_req)).await
    }

    pub(super) async fn task_dep_list(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
        };
        self.inner.cas_task_dep_list(Parameters(inner_req)).await
    }

    pub(super) async fn task_claim(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskClaimRequest;
        let inner_req = TaskClaimRequest {
            task_id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
            duration_secs: req.duration_secs.unwrap_or(600),
            reason: req.reason,
        };
        self.inner.cas_task_claim(Parameters(inner_req)).await
    }

    pub(super) async fn task_release(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskReleaseRequest;
        let inner_req = TaskReleaseRequest {
            task_id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
            force: None,
        };
        self.inner.cas_task_release(Parameters(inner_req)).await
    }

    pub(super) async fn task_reset(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskReleaseRequest;
        let inner_req = TaskReleaseRequest {
            task_id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=reset id=cas-abc1"))?,
            force: req.force,
        };
        self.inner.cas_task_reset(Parameters(inner_req)).await
    }

    pub(super) async fn task_transfer(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskTransferRequest;
        let effective_supervisor_override = req.effective_supervisor_override();
        let inner_req = TaskTransferRequest {
            task_id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
            to_agent: req
                .to_agent
                .ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "to_agent required — for transfer, the target field is `to_agent` \
                         (not `assignee`). NOTE: `transfer` is for reassigning an ALREADY-CLAIMED \
                         task between agents. For initial assignment use \
                         `action=update id=<task> assignee=<worker-name>` instead. \
                         Example: mcp__cas__task action=transfer id=cas-abc1 to_agent=worker-2",
                    )
                })?,
            note: req.notes,
            // Keep the one-release close-gate alias usable for transfer while
            // exposing the surviving name consistently at the unified boundary.
            supervisor_override: effective_supervisor_override,
        };
        self.inner.cas_task_transfer(Parameters(inner_req)).await
    }

    pub(super) async fn task_available(
        &self,
        req: TaskRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::TaskAvailableRequest;
        let inner_req = TaskAvailableRequest {
            limit: req.limit,
            scope: req.scope.unwrap_or_else(|| "all".to_string()),
            sort: req.sort.clone(),
            sort_order: req.sort_order.clone(),
            include_foreign: req.include_foreign.unwrap_or(false),
        };
        self.inner.cas_tasks_available(Parameters(inner_req)).await
    }

    pub(super) async fn task_mine(&self, req: TaskRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::LimitRequest;
        let inner_req = LimitRequest {
            limit: req.limit,
            scope: req.scope.unwrap_or_else(|| "all".to_string()),
            sort: req.sort.clone(),
            sort_order: req.sort_order.clone(),
            team_id: None, // TaskRequest team_id support added in separate task
        };
        self.inner.cas_tasks_mine(Parameters(inner_req)).await
    }

    // Rule implementations
    pub(super) async fn rule_create(&self, req: RuleRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::RuleCreateRequest;
        let inner_req = RuleCreateRequest {
            content: req
                .content
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "content required"))?,
            paths: req.paths,
            tags: req.tags,
            source_ids: req.source_ids,
            scope: req.scope.unwrap_or_else(|| "project".to_string()),
            auto_approve_tools: req.auto_approve_tools,
            auto_approve_paths: req.auto_approve_paths,
        };
        self.inner.cas_rule_create(Parameters(inner_req)).await
    }

    pub(super) async fn rule_show(&self, req: RuleRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
        };
        self.inner.cas_rule_show(Parameters(inner_req)).await
    }

    pub(super) async fn rule_update(&self, req: RuleRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::RuleUpdateRequest;
        let inner_req = RuleUpdateRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
            content: req.content,
            paths: req.paths,
            tags: req.tags,
            auto_approve_tools: req.auto_approve_tools,
            auto_approve_paths: req.auto_approve_paths,
            changed_by: req.changed_by,
            change_note: req.change_note,
        };
        self.inner.cas_rule_update(Parameters(inner_req)).await
    }

    pub(super) async fn rule_delete(&self, req: RuleRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
        };
        self.inner.cas_rule_delete(Parameters(inner_req)).await
    }

    pub(super) async fn rule_history(&self, req: RuleRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::VersionRequest;
        let inner_req = VersionRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for history"))?,
            version: req.version,
            version_id: req.version_id,
            changed_by: req.changed_by,
            change_note: req.change_note,
        };
        self.inner.cas_rule_history(Parameters(inner_req)).await
    }

    pub(super) async fn rule_restore(&self, req: RuleRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::VersionRequest;
        let inner_req = VersionRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for restore"))?,
            version: req.version,
            version_id: req.version_id,
            changed_by: req.changed_by,
            change_note: req.change_note,
        };
        self.inner.cas_rule_restore(Parameters(inner_req)).await
    }

    pub(super) async fn rule_list(&self, _req: RuleRequest) -> Result<CallToolResult, McpError> {
        self.inner.cas_rules_list().await
    }

    pub(super) async fn rule_list_all(&self, req: RuleRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::LimitRequest;
        let inner_req = LimitRequest {
            limit: req.limit,
            scope: req.scope.unwrap_or_else(|| "all".to_string()),
            sort: None,
            sort_order: None,
            team_id: None, // RuleRequest team_id support added in separate task
        };
        self.inner.cas_rule_list_all(Parameters(inner_req)).await
    }

    pub(super) async fn rule_helpful(&self, req: RuleRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
        };
        self.inner.cas_rule_helpful(Parameters(inner_req)).await
    }

    pub(super) async fn rule_harmful(&self, req: RuleRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
        };
        self.inner.cas_rule_harmful(Parameters(inner_req)).await
    }

    pub(super) async fn rule_sync(&self, _req: RuleRequest) -> Result<CallToolResult, McpError> {
        self.inner.cas_rule_sync().await
    }

    pub(super) async fn rule_check_similar(
        &self,
        req: RuleRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::hybrid_search::{DocType, SearchOptions};
        use crate::store::open_rule_store;

        let content = req.content.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "content required for check_similar",
            )
        })?;
        let threshold = req.threshold.unwrap_or(0.75);

        // Open search index and search for similar rules
        let search = self.inner.open_search_index()?;

        let opts = SearchOptions {
            query: content.clone(),
            limit: 10,
            doc_types: vec![DocType::Rule],
            ..Default::default()
        };

        let results = search
            .search_unified(&opts)
            .map_err(|e| Self::error(ErrorCode::INTERNAL_ERROR, format!("Search failed: {e}")))?;

        // Filter by threshold
        let similar: Vec<_> = results
            .into_iter()
            .filter(|r| r.score >= threshold as f64)
            .collect();

        if similar.is_empty() {
            return Ok(Self::success(format!(
                "No similar rules found (threshold: {:.0}%)\n\nYou can create a new rule with this content.",
                threshold * 100.0
            )));
        }

        // Open rule store to fetch actual rule content
        let rule_store = open_rule_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open rule store: {e}"),
            )
        })?;

        let mut output = format!(
            "Found {} similar rule(s) (threshold: {:.0}%):\n\n",
            similar.len(),
            threshold * 100.0
        );

        for (i, result) in similar.iter().enumerate() {
            output.push_str(&format!(
                "{}. **{}** (similarity: {:.0}%)\n",
                i + 1,
                result.id,
                result.score * 100.0
            ));

            // Fetch actual rule content
            if let Ok(rule) = rule_store.get(&result.id) {
                let preview: String = rule.content.chars().take(200).collect();
                let ellipsis = if rule.content.len() > 200 { "..." } else { "" };
                output.push_str(&format!("   Content: {preview}{ellipsis}\n\n"));
            } else {
                output.push_str("   (Could not fetch rule content)\n\n");
            }
        }

        output.push_str(
            "Consider marking an existing rule as `helpful` instead of creating a duplicate.",
        );

        Ok(Self::success(output))
    }

    // Skill implementations
    pub(super) async fn skill_create(&self, req: SkillRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::SkillCreateRequest;
        let inner_req = SkillCreateRequest {
            name: req
                .name
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "name required"))?,
            description: req
                .description
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "description required"))?,
            invocation: req
                .invocation
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "invocation required"))?,
            skill_type: req.skill_type.unwrap_or_else(|| "command".to_string()),
            tags: req.tags,
            source_ids: req.source_ids,
            scope: req.scope.unwrap_or_else(|| "global".to_string()),
            summary: req.summary,
            example: req.example,
            preconditions: req.preconditions,
            postconditions: req.postconditions,
            validation_script: req.validation_script,
            invokable: req.invokable.unwrap_or(false),
            argument_hint: req.argument_hint,
            context_mode: req.context_mode,
            agent_type: req.agent_type,
            allowed_tools: req.allowed_tools,
            disallowed_tools: req.disallowed_tools,
            draft: req.draft.unwrap_or(false),
            disable_model_invocation: req.disable_model_invocation.unwrap_or(false),
        };
        self.inner.cas_skill_create(Parameters(inner_req)).await
    }

    pub(super) async fn skill_show(&self, req: SkillRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
        };
        self.inner.cas_skill_show(Parameters(inner_req)).await
    }

    pub(super) async fn skill_update(&self, req: SkillRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::SkillUpdateRequest;
        let inner_req = SkillUpdateRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
            name: req.name,
            description: req.description,
            invocation: req.invocation,
            tags: req.tags,
            preconditions: req.preconditions,
            postconditions: req.postconditions,
            validation_script: req.validation_script,
            summary: req.summary,
            disable_model_invocation: req.disable_model_invocation,
            changed_by: req.changed_by,
            change_note: req.change_note,
        };
        self.inner.cas_skill_update(Parameters(inner_req)).await
    }

    pub(super) async fn skill_delete(&self, req: SkillRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
        };
        self.inner.cas_skill_delete(Parameters(inner_req)).await
    }

    pub(super) async fn skill_history(
        &self,
        req: SkillRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::VersionRequest;
        let inner_req = VersionRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for history"))?,
            version: req.version,
            version_id: req.version_id,
            changed_by: req.changed_by,
            change_note: req.change_note,
        };
        self.inner.cas_skill_history(Parameters(inner_req)).await
    }

    pub(super) async fn skill_restore(
        &self,
        req: SkillRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::VersionRequest;
        let inner_req = VersionRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for restore"))?,
            version: req.version,
            version_id: req.version_id,
            changed_by: req.changed_by,
            change_note: req.change_note,
        };
        self.inner.cas_skill_restore(Parameters(inner_req)).await
    }

    pub(super) async fn skill_list(&self, _req: SkillRequest) -> Result<CallToolResult, McpError> {
        self.inner.cas_skill_list().await
    }

    pub(super) async fn skill_list_all(
        &self,
        req: SkillRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::LimitRequest;
        let inner_req = LimitRequest {
            limit: req.limit,
            scope: req.scope.unwrap_or_else(|| "all".to_string()),
            sort: None,
            sort_order: None,
            team_id: None, // SkillRequest team_id support added in separate task
        };
        self.inner.cas_skill_list_all(Parameters(inner_req)).await
    }

    pub(super) async fn skill_enable(&self, req: SkillRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
        };
        self.inner.cas_skill_enable(Parameters(inner_req)).await
    }

    pub(super) async fn skill_disable(
        &self,
        req: SkillRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
        };
        self.inner.cas_skill_disable(Parameters(inner_req)).await
    }

    pub(super) async fn skill_sync(&self, _req: SkillRequest) -> Result<CallToolResult, McpError> {
        self.inner.cas_skill_sync().await
    }

    pub(super) async fn skill_use(&self, req: SkillRequest) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::IdRequest;
        let inner_req = IdRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required — pass as `id` (not `task_id`, `taskId`, or `_id`). Example: mcp__cas__task action=<verb> id=cas-abc1"))?,
        };
        self.inner.cas_skill_use(Parameters(inner_req)).await
    }

    // Agent implementations
}
