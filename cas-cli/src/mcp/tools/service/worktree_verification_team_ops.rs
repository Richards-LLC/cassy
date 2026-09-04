use crate::mcp::tools::service::imports::*;

impl CasService {
    pub(super) async fn worktree_create(
        &self,
        req: WorktreeRequest,
    ) -> Result<CallToolResult, McpError> {
        let task_id = req.task_id.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "task_id is required for create action",
            )
        })?;
        self.inner.worktree_create(&task_id).await
    }

    pub(super) async fn worktree_list(
        &self,
        req: WorktreeRequest,
    ) -> Result<CallToolResult, McpError> {
        self.inner
            .worktree_list(
                req.all.unwrap_or(false),
                req.status.as_deref(),
                req.orphans.unwrap_or(false),
            )
            .await
    }

    pub(super) async fn worktree_show(
        &self,
        req: WorktreeRequest,
    ) -> Result<CallToolResult, McpError> {
        let id = req.id.ok_or_else(|| {
            Self::error(ErrorCode::INVALID_PARAMS, "id is required for show action")
        })?;
        self.inner.worktree_show(&id).await
    }

    pub(super) async fn worktree_cleanup(
        &self,
        req: WorktreeRequest,
    ) -> Result<CallToolResult, McpError> {
        // cas-f102 (GH #140): `id` is now honoured. Absent, this stays the
        // System-A orphan sweep it always was; present, it targets one
        // worktree and resolves it System A first, System B second — the same
        // order `worktree_merge` uses.
        self.inner
            .worktree_cleanup(
                req.id.as_deref(),
                req.dry_run.unwrap_or(false),
                req.force.unwrap_or(false),
            )
            .await
    }

    pub(super) async fn worktree_merge(
        &self,
        req: WorktreeRequest,
    ) -> Result<CallToolResult, McpError> {
        let id = req.id.ok_or_else(|| {
            Self::error(ErrorCode::INVALID_PARAMS, "id is required for merge action")
        })?;
        self.inner
            .worktree_merge(
                &id,
                req.force.unwrap_or(false),
                req.task_id.as_deref(),
                req.allow_trunk.unwrap_or(false),
                req.cleanup,
            )
            .await
    }

    pub(super) async fn worktree_status(
        &self,
        _req: WorktreeRequest,
    ) -> Result<CallToolResult, McpError> {
        self.inner.worktree_status().await
    }

    // Verification implementations
    pub(super) async fn verification_add(
        &self,
        req: VerificationRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::VerificationAddRequest;
        let inner_req = VerificationAddRequest {
            task_id: req.task_id.ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, "task_id required for add")
            })?,
            status: req.status.unwrap_or_else(|| "approved".to_string()),
            summary: req.summary.ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, "summary required for add")
            })?,
            confidence: req.confidence,
            issues: req.issues,
            files_reviewed: req.files,
            duration_ms: req.duration_ms,
            verification_type: req.verification_type,
            verifier_capability: req.verifier_capability,
            dispatch_id: req.dispatch_id,
        };
        self.inner.cas_verification_add(Parameters(inner_req)).await
    }

    pub(super) async fn verification_show(
        &self,
        req: VerificationRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::VerificationShowRequest;
        let inner_req = VerificationShowRequest {
            id: req
                .id
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for show"))?,
        };
        self.inner
            .cas_verification_show(Parameters(inner_req))
            .await
    }

    pub(super) async fn verification_list(
        &self,
        req: VerificationRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::VerificationListRequest;
        let inner_req = VerificationListRequest {
            task_id: req.task_id.ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, "task_id required for list")
            })?,
            limit: req.limit,
        };
        self.inner
            .cas_verification_list(Parameters(inner_req))
            .await
    }

    pub(super) async fn verification_latest(
        &self,
        req: VerificationRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::VerificationListRequest;
        let inner_req = VerificationListRequest {
            task_id: req.task_id.ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, "task_id required for latest")
            })?,
            limit: Some(1),
        };
        self.inner
            .cas_verification_latest(Parameters(inner_req))
            .await
    }

    // Team implementations
    pub(super) async fn team_list(&self, _req: TeamRequest) -> Result<CallToolResult, McpError> {
        {
            use crate::cloud::CloudConfig;

            let cloud_config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(
                &self.inner.cas_root,
            )
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to load cloud config: {e}"),
                )
            })?;

            if !cloud_config.is_logged_in() {
                return Ok(Self::success(
                    "Not logged in to Cassy Cloud.\n\n\
                 Use `cas cloud login` to connect to your account and access team features.",
                ));
            }

            // Shows the currently configured team from local config
            let mut output = String::from("Configured Team:\n\n");

            if let (Some(team_id), Some(team_slug)) =
                (&cloud_config.team_id, &cloud_config.team_slug)
            {
                output.push_str(&format!("• {team_slug} ({team_id})\n"));
                if let Some(ts) = cloud_config.get_team_sync_timestamp(team_id) {
                    output.push_str(&format!(
                        "  Last sync: {}\n",
                        ts.format("%Y-%m-%d %H:%M:%S UTC")
                    ));
                }
                output.push_str("\nUse `team show` for detailed team statistics.");
            } else {
                output.push_str("No team configured.\n\n");
                output.push_str(
                    "To set a user-wide team default, run `cas cloud team default <slug>`.\n",
                );
                output.push_str("For a per-project override, use `cas cloud team set <uuid>`.\n");
                output.push_str("To view all your teams, visit the Cassy Cloud web dashboard.");
            }

            Ok(Self::success(output))
        }
    }

    pub(super) async fn team_show(&self, req: TeamRequest) -> Result<CallToolResult, McpError> {
        {
            use crate::cloud::CloudConfig;

            let cloud_config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(
                &self.inner.cas_root,
            )
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to load cloud config: {e}"),
                )
            })?;

            if !cloud_config.is_logged_in() {
                return Ok(Self::success("Not logged in to Cassy Cloud."));
            }

            let team_id = req
                .team_id
                .or(cloud_config.team_id.clone())
                .ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "team_id required (or set user-wide default with `cas cloud team default <slug>`)",
                    )
                })?;

            let token = cloud_config
                .token
                .as_ref()
                .ok_or_else(|| Self::error(ErrorCode::INTERNAL_ERROR, "Missing auth token"))?;

            // Call team status API to get real counts
            let status_url = format!(
                "{}/api/teams/{}/sync/status",
                cloud_config.endpoint, team_id
            );

            let response = ureq::get(&status_url)
                .timeout(std::time::Duration::from_secs(30))
                .set("Authorization", &format!("Bearer {token}"))
                .call();

            match response {
                Ok(resp) => {
                    if let Ok(body) = resp.into_json::<serde_json::Value>() {
                        let team_name = body
                            .get("team_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Unknown");
                        let sync_state = body.get("sync_state");

                        let entries = sync_state
                            .and_then(|s| s.get("entries"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let tasks = sync_state
                            .and_then(|s| s.get("tasks"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let rules = sync_state
                            .and_then(|s| s.get("rules"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let skills = sync_state
                            .and_then(|s| s.get("skills"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);

                        let mut output = format!("Team: {team_name} ({team_id})\n\n");
                        output.push_str("Shared Resources:\n");
                        output.push_str(&format!("  Entries: {entries}\n"));
                        output.push_str(&format!("  Tasks:   {tasks}\n"));
                        output.push_str(&format!("  Rules:   {rules}\n"));
                        output.push_str(&format!("  Skills:  {skills}\n"));

                        if let Some(ts) = cloud_config.get_team_sync_timestamp(&team_id) {
                            output.push_str(&format!(
                                "\nLast local sync: {}",
                                ts.format("%Y-%m-%d %H:%M:%S UTC")
                            ));
                        }

                        output.push_str("\n\nUse `team sync` to push/pull team data.");
                        Ok(Self::success(output))
                    } else {
                        Ok(Self::success(format!(
                            "Team {team_id} - failed to parse response"
                        )))
                    }
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    if code == 403 || code == 401 {
                        Ok(Self::success(format!(
                            "Access denied to team {team_id}. Check your team membership."
                        )))
                    } else if code == 404 {
                        Ok(Self::success(format!("Team {team_id} not found.")))
                    } else {
                        Err(Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!("API error {code}: {body}"),
                        ))
                    }
                }
                Err(ureq::Error::Transport(e)) => Err(Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Network error: {e}"),
                )),
            }
        }
    }

    pub(super) async fn team_members(&self, req: TeamRequest) -> Result<CallToolResult, McpError> {
        {
            use crate::cloud::CloudConfig;

            let cloud_config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(
                &self.inner.cas_root,
            )
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to load cloud config: {e}"),
                )
            })?;

            if !cloud_config.is_logged_in() {
                return Ok(Self::success("Not logged in to Cassy Cloud."));
            }

            let team_id = req
                .team_id
                .or(cloud_config.team_id.clone())
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "team_id required"))?;

            // Team member management is available through the Cassy Cloud web interface.
            // The CLI API focuses on sync operations. Use the web dashboard for:
            // - Viewing team members and their roles
            // - Inviting new members
            // - Managing permissions
            let output = format!(
                "Team Members ({})\n\n\
             Team member management is available through the Cassy Cloud web dashboard.\n\n\
             Visit: {}/org/*/team/{}\n\n\
             The CLI focuses on sync operations. Use `team show` for team statistics\n\
             or `team sync` to synchronize shared resources.",
                team_id,
                cloud_config.endpoint,
                cloud_config.team_slug.as_deref().unwrap_or(&team_id)
            );

            Ok(Self::success(output))
        }
    }

    pub(super) async fn team_sync(&self, req: TeamRequest) -> Result<CallToolResult, McpError> {
        {
            use crate::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, SyncQueue};
            use crate::store::{open_rule_store, open_skill_store, open_store, open_task_store};
            use std::sync::Arc;

            let cloud_config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(
                &self.inner.cas_root,
            )
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to load cloud config: {e}"),
                )
            })?;

            if !cloud_config.is_logged_in() {
                return Ok(Self::success(
                    "Not logged in to Cassy Cloud. Use `cas cloud login` first.",
                ));
            }

            let team_id = req
                .team_id
                .or(cloud_config.team_id.clone())
                .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "team_id required"))?;

            // Open stores
            let store = open_store(&self.inner.cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open store: {e}"),
                )
            })?;
            let task_store = open_task_store(&self.inner.cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open task store: {e}"),
                )
            })?;
            let rule_store = open_rule_store(&self.inner.cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open rule store: {e}"),
                )
            })?;
            let skill_store = open_skill_store(&self.inner.cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open skill store: {e}"),
                )
            })?;

            // Create sync queue and syncer
            let queue = Arc::new(SyncQueue::open(&self.inner.cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to create sync queue: {e}"),
                )
            })?);

            let project_id = crate::cloud::resolve_canonical_id_for_sync(&self.inner.cas_root)
                .map_err(|e| Self::error(ErrorCode::INTERNAL_ERROR, e.to_string()))?;
            let syncer = CloudSyncer::new_for_project(
                queue,
                cloud_config,
                CloudSyncerConfig::default(),
                project_id.clone(),
                &self.inner.cas_root,
            );

            // Push first, then pull
            let mut output = format!("Team sync for {team_id}\n\n");

            output.push_str("Pushing local changes...\n");
            let push_result = syncer
                .push_team(&team_id)
                .map_err(|e| Self::error(ErrorCode::INTERNAL_ERROR, format!("Push failed: {e}")))?;
            output.push_str(&format!(
                "  Pushed: {} entries, {} tasks, {} rules, {} skills\n",
                push_result.pushed_entries,
                push_result.pushed_tasks,
                push_result.pushed_rules,
                push_result.pushed_skills
            ));

            output.push_str("\nPulling team data...\n");
            // cas-53d5: pull_team now takes project_id explicitly for
            // per-(team, project) watermark scoping. Resolve at the
            // caller; bail if we're not inside a Cassy project (same
            // contract as the syncer's prior internal resolve).
            let pull_result = syncer
                .pull_team(
                    &team_id,
                    &project_id,
                    store.as_ref(),
                    task_store.as_ref(),
                    rule_store.as_ref(),
                    skill_store.as_ref(),
                )
                .map_err(|e| Self::error(ErrorCode::INTERNAL_ERROR, format!("Pull failed: {e}")))?;
            output.push_str(&format!(
                "  Pulled: {} entries, {} tasks, {} rules, {} skills\n",
                pull_result.pulled_entries,
                pull_result.pulled_tasks,
                pull_result.pulled_rules,
                pull_result.pulled_skills
            ));

            if pull_result.conflicts_resolved > 0 {
                output.push_str(&format!(
                    "  Conflicts resolved: {}\n",
                    pull_result.conflicts_resolved
                ));
            }

            output.push_str(&format!(
                "\nSync completed in {}ms",
                push_result.duration_ms + pull_result.duration_ms
            ));

            Ok(Self::success(output))
        }
    }
}
