use crate::config::*;

impl Config {
    pub fn get(&self, key: &str) -> Option<String> {
        let cloud = self.cloud.clone().unwrap_or_default();
        let hooks = self.hooks.clone().unwrap_or_default();
        let tasks = self.tasks.clone().unwrap_or_default();
        let dev = self.dev.clone().unwrap_or_default();
        let daemon = self.daemon.clone().unwrap_or_default();
        let staging = self.staging.clone().unwrap_or_default();
        let issues = self.issues.clone().unwrap_or_default();
        let issue_repos = issues.resolved_registry();
        let release = self.release.clone().unwrap_or_default();
        let notifications = self.notifications.clone().unwrap_or_default();
        let skill_validation = self.skill_validation.clone().unwrap_or_default();
        let skills = self.skills.clone().unwrap_or_default();
        let memory = self.memory.clone().unwrap_or_default();
        match key {
            // Sync section
            "sync.enabled" => Some(self.sync.enabled.to_string()),
            "sync.target" => Some(self.sync.target.clone()),
            "sync.min_helpful" => Some(self.sync.min_helpful.to_string()),
            "sync.promotion_threshold" => Some(self.sync.promotion_threshold.to_string()),
            "sync.demotion_threshold" => Some(self.sync.demotion_threshold.to_string()),
            "sync.promotion_evidence" => Some(self.sync.promotion_evidence.join(",")),
            "skill_validation.require_sandbox" => {
                Some(skill_validation.require_sandbox.to_string())
            }
            "skills.optional" => Some(skills.optional.join(",")),
            // Cloud section
            "cloud.auto_sync" => Some(cloud.auto_sync.to_string()),
            "cloud.interval_secs" => Some(cloud.interval_secs.to_string()),
            "cloud.pull_on_start" => Some(cloud.pull_on_start.to_string()),
            "cloud.max_retries" => Some(cloud.max_retries.to_string()),
            "cloud.queue_pending_warning" => Some(cloud.queue_pending_warning.to_string()),
            "cloud.queue_oldest_warning_secs" => Some(cloud.queue_oldest_warning_secs.to_string()),
            // Hooks section
            "hooks.capture_enabled" => Some(hooks.capture_enabled.to_string()),
            "hooks.capture_tools" => Some(hooks.capture_tools.join(",")),
            "hooks.inject_context" => Some(hooks.inject_context.to_string()),
            "hooks.context_limit" => Some(hooks.context_limit.to_string()),
            "hooks.generate_summaries" => Some(hooks.generate_summaries.to_string()),
            "hooks.token_budget" => Some(hooks.token_budget.to_string()),
            "hooks.ai_context" => Some(hooks.ai_context.to_string()),
            "hooks.ai_model" => Some(hooks.ai_model.clone()),
            "hooks.ai_fallback" => Some(hooks.ai_fallback.to_string()),
            "hooks.minimal_start" => Some(hooks.minimal_start.to_string()),
            // Hooks.plan_mode section
            "hooks.plan_mode.enabled" => Some(hooks.plan_mode.enabled.to_string()),
            "hooks.plan_mode.token_budget" => Some(hooks.plan_mode.token_budget.to_string()),
            "hooks.plan_mode.task_limit" => Some(hooks.plan_mode.task_limit.to_string()),
            "hooks.plan_mode.show_dependencies" => {
                Some(hooks.plan_mode.show_dependencies.to_string())
            }
            "hooks.plan_mode.include_closed" => Some(hooks.plan_mode.include_closed.to_string()),
            "hooks.plan_mode.semantic_search" => Some(hooks.plan_mode.semantic_search.to_string()),
            // Tasks section
            "tasks.commit_nudge_on_close" => Some(tasks.commit_nudge_on_close.to_string()),
            "tasks.block_exit_on_open" => Some(tasks.block_exit_on_open.to_string()),
            // Dev section
            "dev.dev_mode" => Some(dev.dev_mode.to_string()),
            "dev.trace_commands" => Some(dev.trace_commands.to_string()),
            "dev.trace_store_ops" => Some(dev.trace_store_ops.to_string()),
            "dev.trace_claude_api" => Some(dev.trace_claude_api.to_string()),
            "dev.trace_hooks" => Some(dev.trace_hooks.to_string()),
            "dev.trace_retention_days" => Some(dev.trace_retention_days.to_string()),
            "daemon.archive_max_bytes" => Some(daemon.archive_max_bytes.to_string()),
            "daemon.archive_retention_days" => Some(daemon.archive_retention_days.to_string()),
            "daemon.relevance_sampling_enabled" => {
                Some(daemon.relevance_sampling_enabled.to_string())
            }
            "daemon.relevance_sampling_interval_secs" => {
                Some(daemon.relevance_sampling_interval_secs.to_string())
            }
            "daemon.relevance_sampling_sample_size" => {
                Some(daemon.relevance_sampling_sample_size.to_string())
            }
            "memory.session_learn_auto" => Some(memory.session_learn_auto.to_string()),
            "memory.decay.curated_importance_floor" => {
                Some(memory.decay.curated_importance_floor.to_string())
            }
            "memory.decay.promote_on_access" => Some(memory.decay.promote_on_access.to_string()),
            // Code section
            "code.enabled" => Some(self.code.clone().unwrap_or_default().enabled.to_string()),
            "code.watch_paths" => Some(self.code.clone().unwrap_or_default().watch_paths.join(",")),
            "code.exclude_patterns" => Some(
                self.code
                    .clone()
                    .unwrap_or_default()
                    .exclude_patterns
                    .join(","),
            ),
            "code.extensions" => Some(self.code.clone().unwrap_or_default().extensions.join(",")),
            "code.index_interval_secs" => Some(
                self.code
                    .clone()
                    .unwrap_or_default()
                    .index_interval_secs
                    .to_string(),
            ),
            "code.debounce_ms" => Some(
                self.code
                    .clone()
                    .unwrap_or_default()
                    .debounce_ms
                    .to_string(),
            ),
            // Staging section
            "staging.staging_dir" | "staging.large_artifact_dir" => {
                Some(staging.staging_dir.unwrap_or_default())
            }
            "staging.scratch_root" => Some(staging.scratch_root.unwrap_or_default()),
            "staging.tmpfs_warning_threshold_bytes" => {
                Some(staging.tmpfs_warning_threshold_bytes.to_string())
            }
            // Issues section
            "issues.repo" => Some(issues.repo.unwrap_or_default()),
            "issues.components.cassy" => Some(issue_repos.cassy),
            "issues.components.mecha_cassy" => Some(issue_repos.mecha_cassy),
            "issues.components.cloud" => Some(issue_repos.cloud),
            // Release section
            "release.claude_account_allowlist" => Some(release.claude_account_allowlist.join(",")),
            // Notifications section
            "notifications.enabled" => Some(notifications.enabled.to_string()),
            "notifications.sound_enabled" => Some(notifications.sound_enabled.to_string()),
            "notifications.display_duration_secs" => {
                Some(notifications.display_duration_secs.to_string())
            }
            "notifications.max_visible" => Some(notifications.max_visible.to_string()),
            // Notifications.tasks section
            "notifications.tasks.on_created" => Some(notifications.tasks.on_created.to_string()),
            "notifications.tasks.on_started" => Some(notifications.tasks.on_started.to_string()),
            "notifications.tasks.on_closed" => Some(notifications.tasks.on_closed.to_string()),
            "notifications.tasks.on_updated" => Some(notifications.tasks.on_updated.to_string()),
            // Notifications.entries section
            "notifications.entries.on_added" => Some(notifications.entries.on_added.to_string()),
            "notifications.entries.on_updated" => {
                Some(notifications.entries.on_updated.to_string())
            }
            "notifications.entries.on_deleted" => {
                Some(notifications.entries.on_deleted.to_string())
            }
            // Notifications.rules section
            "notifications.rules.on_created" => Some(notifications.rules.on_created.to_string()),
            "notifications.rules.on_promoted" => Some(notifications.rules.on_promoted.to_string()),
            "notifications.rules.on_demoted" => Some(notifications.rules.on_demoted.to_string()),
            // Notifications.skills section
            "notifications.skills.on_created" => Some(notifications.skills.on_created.to_string()),
            "notifications.skills.on_enabled" => Some(notifications.skills.on_enabled.to_string()),
            "notifications.skills.on_disabled" => {
                Some(notifications.skills.on_disabled.to_string())
            }
            // Agent section
            "agent.name" => Some(self.agent.clone().unwrap_or_default().name),
            "agent.max_concurrent_tasks" => Some(
                self.agent
                    .clone()
                    .unwrap_or_default()
                    .max_concurrent_tasks
                    .to_string(),
            ),
            "agent.capabilities" => Some(
                self.agent
                    .clone()
                    .unwrap_or_default()
                    .capabilities
                    .join(","),
            ),
            // Coordination section
            "coordination.mode" => {
                let mode = self.coordination.clone().unwrap_or_default().mode;
                Some(match mode {
                    CoordinationMode::Local => "local".to_string(),
                    CoordinationMode::Cloud => "cloud".to_string(),
                })
            }
            "coordination.cloud_url" => self.coordination.clone().unwrap_or_default().cloud_url,
            // Lease section
            "lease.default_duration_mins" => Some(
                self.lease
                    .clone()
                    .unwrap_or_default()
                    .default_duration_mins
                    .to_string(),
            ),
            "lease.max_duration_mins" => Some(
                self.lease
                    .clone()
                    .unwrap_or_default()
                    .max_duration_mins
                    .to_string(),
            ),
            "lease.heartbeat_interval_secs" => Some(
                self.lease
                    .clone()
                    .unwrap_or_default()
                    .heartbeat_interval_secs
                    .to_string(),
            ),
            "lease.expiry_grace_secs" => Some(
                self.lease
                    .clone()
                    .unwrap_or_default()
                    .expiry_grace_secs
                    .to_string(),
            ),
            // Verification section
            "verification.enabled" => Some(
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .enabled
                    .to_string(),
            ),
            "verification.model" => Some(self.verification.clone().unwrap_or_default().model),
            "verification.force_bypass_allowed" => Some(
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .force_bypass_allowed
                    .to_string(),
            ),
            "verification.timeout_secs" => Some(
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .timeout_secs
                    .to_string(),
            ),
            "verification.patterns.todo_comments" => Some(
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .patterns
                    .todo_comments
                    .to_string(),
            ),
            "verification.patterns.temporal_shortcuts" => Some(
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .patterns
                    .temporal_shortcuts
                    .to_string(),
            ),
            "verification.patterns.stub_implementations" => Some(
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .patterns
                    .stub_implementations
                    .to_string(),
            ),
            "verification.patterns.empty_error_handlers" => Some(
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .patterns
                    .empty_error_handlers
                    .to_string(),
            ),
            "verification.patterns.custom" => Some(
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .patterns
                    .custom
                    .join(","),
            ),
            // Worktrees section
            "worktrees.enabled" => Some(
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .enabled
                    .to_string(),
            ),
            "worktrees.base_path" => Some(self.worktrees.clone().unwrap_or_default().base_path),
            "worktrees.branch_prefix" => {
                Some(self.worktrees.clone().unwrap_or_default().branch_prefix)
            }
            "worktrees.auto_merge" => Some(
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .auto_merge
                    .to_string(),
            ),
            "worktrees.cleanup_on_close" => Some(
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .cleanup_on_close
                    .to_string(),
            ),
            "worktrees.promote_entries_on_merge" => Some(
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .promote_entries_on_merge
                    .to_string(),
            ),
            "worktrees.abandon_ttl_hours" => Some(
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .abandon_ttl_hours
                    .to_string(),
            ),
            "worktrees.global_sweep_debounce_secs" => Some(
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .global_sweep_debounce_secs
                    .to_string(),
            ),
            "worktrees.sweep_claude_agent_dirs" => Some(
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .sweep_claude_agent_dirs
                    .to_string(),
            ),
            // Telemetry section
            "telemetry.enabled" => Some(
                self.telemetry
                    .clone()
                    .unwrap_or_default()
                    .enabled
                    .to_string(),
            ),
            "telemetry.consent_given" => Some(
                self.telemetry
                    .clone()
                    .unwrap_or_default()
                    .consent_given
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "not set".to_string()),
            ),
            // LLM section
            "llm.harness" => Some(
                self.llm()
                    .harness
                    .clone()
                    .unwrap_or_else(|| "(default)".to_string()),
            ),
            "llm.model" => Some(
                self.llm()
                    .model
                    .clone()
                    .unwrap_or_else(|| "(default)".to_string()),
            ),
            "llm.reasoning_effort" => Some(
                self.llm()
                    .reasoning_effort
                    .clone()
                    .unwrap_or_else(|| "(default)".to_string()),
            ),
            "llm.supervisor.harness" => Some(
                self.llm()
                    .supervisor
                    .as_ref()
                    .and_then(|s| s.harness.clone())
                    .unwrap_or_else(|| "(inherit)".to_string()),
            ),
            "llm.supervisor.model" => Some(
                self.llm()
                    .supervisor
                    .as_ref()
                    .and_then(|s| s.model.clone())
                    .unwrap_or_else(|| "(inherit)".to_string()),
            ),
            "llm.supervisor.reasoning_effort" => Some(
                self.llm()
                    .supervisor
                    .as_ref()
                    .and_then(|s| s.reasoning_effort.clone())
                    .unwrap_or_else(|| "(inherit)".to_string()),
            ),
            "llm.worker.harness" => Some(
                self.llm()
                    .worker
                    .as_ref()
                    .and_then(|s| s.harness.clone())
                    .unwrap_or_else(|| "(inherit)".to_string()),
            ),
            "llm.worker.model" => Some(
                self.llm()
                    .worker
                    .as_ref()
                    .and_then(|s| s.model.clone())
                    .unwrap_or_else(|| "(inherit)".to_string()),
            ),
            "llm.worker.reasoning_effort" => Some(
                self.llm()
                    .worker
                    .as_ref()
                    .and_then(|s| s.reasoning_effort.clone())
                    .unwrap_or_else(|| "(inherit)".to_string()),
            ),
            _ => None,
        }
    }
}
