use crate::config::*;

impl Config {
    pub fn list(&self) -> Vec<(String, String)> {
        let cloud = self.cloud.clone().unwrap_or_default();
        let hooks = self.hooks.clone().unwrap_or_default();
        let tasks = self.tasks.clone().unwrap_or_default();
        let dev = self.dev.clone().unwrap_or_default();
        let daemon = self.daemon.clone().unwrap_or_default();
        let staging = self.staging.clone().unwrap_or_default();
        let issues = self.issues.clone().unwrap_or_default();
        let issue_repos = issues.resolved_registry();
        let notifications = self.notifications.clone().unwrap_or_default();
        let skill_validation = self.skill_validation.clone().unwrap_or_default();
        let skills = self.skills.clone().unwrap_or_default();
        let memory = self.memory.clone().unwrap_or_default();
        vec![
            // Sync section
            ("sync.enabled".to_string(), self.sync.enabled.to_string()),
            ("sync.target".to_string(), self.sync.target.clone()),
            (
                "sync.min_helpful".to_string(),
                self.sync.min_helpful.to_string(),
            ),
            (
                "sync.promotion_threshold".to_string(),
                self.sync.promotion_threshold.to_string(),
            ),
            (
                "sync.demotion_threshold".to_string(),
                self.sync.demotion_threshold.to_string(),
            ),
            (
                "sync.promotion_evidence".to_string(),
                self.sync.promotion_evidence.join(","),
            ),
            (
                "skill_validation.require_sandbox".to_string(),
                skill_validation.require_sandbox.to_string(),
            ),
            ("skills.optional".to_string(), skills.optional.join(",")),
            // Cloud section
            ("cloud.auto_sync".to_string(), cloud.auto_sync.to_string()),
            (
                "cloud.interval_secs".to_string(),
                cloud.interval_secs.to_string(),
            ),
            (
                "cloud.pull_on_start".to_string(),
                cloud.pull_on_start.to_string(),
            ),
            (
                "cloud.max_retries".to_string(),
                cloud.max_retries.to_string(),
            ),
            (
                "cloud.queue_pending_warning".to_string(),
                cloud.queue_pending_warning.to_string(),
            ),
            (
                "cloud.queue_oldest_warning_secs".to_string(),
                cloud.queue_oldest_warning_secs.to_string(),
            ),
            // Hooks section
            (
                "hooks.capture_enabled".to_string(),
                hooks.capture_enabled.to_string(),
            ),
            (
                "hooks.capture_tools".to_string(),
                hooks.capture_tools.join(","),
            ),
            (
                "hooks.inject_context".to_string(),
                hooks.inject_context.to_string(),
            ),
            (
                "hooks.context_limit".to_string(),
                hooks.context_limit.to_string(),
            ),
            (
                "hooks.generate_summaries".to_string(),
                hooks.generate_summaries.to_string(),
            ),
            (
                "hooks.token_budget".to_string(),
                hooks.token_budget.to_string(),
            ),
            ("hooks.ai_context".to_string(), hooks.ai_context.to_string()),
            ("hooks.ai_model".to_string(), hooks.ai_model.clone()),
            (
                "hooks.ai_fallback".to_string(),
                hooks.ai_fallback.to_string(),
            ),
            (
                "hooks.minimal_start".to_string(),
                hooks.minimal_start.to_string(),
            ),
            // Hooks.plan_mode section
            (
                "hooks.plan_mode.enabled".to_string(),
                hooks.plan_mode.enabled.to_string(),
            ),
            (
                "hooks.plan_mode.token_budget".to_string(),
                hooks.plan_mode.token_budget.to_string(),
            ),
            (
                "hooks.plan_mode.task_limit".to_string(),
                hooks.plan_mode.task_limit.to_string(),
            ),
            (
                "hooks.plan_mode.show_dependencies".to_string(),
                hooks.plan_mode.show_dependencies.to_string(),
            ),
            (
                "hooks.plan_mode.include_closed".to_string(),
                hooks.plan_mode.include_closed.to_string(),
            ),
            (
                "hooks.plan_mode.semantic_search".to_string(),
                hooks.plan_mode.semantic_search.to_string(),
            ),
            // Tasks section
            (
                "tasks.commit_nudge_on_close".to_string(),
                tasks.commit_nudge_on_close.to_string(),
            ),
            (
                "tasks.block_exit_on_open".to_string(),
                tasks.block_exit_on_open.to_string(),
            ),
            // Dev section
            ("dev.dev_mode".to_string(), dev.dev_mode.to_string()),
            (
                "dev.trace_commands".to_string(),
                dev.trace_commands.to_string(),
            ),
            (
                "dev.trace_store_ops".to_string(),
                dev.trace_store_ops.to_string(),
            ),
            (
                "dev.trace_claude_api".to_string(),
                dev.trace_claude_api.to_string(),
            ),
            ("dev.trace_hooks".to_string(), dev.trace_hooks.to_string()),
            (
                "dev.trace_retention_days".to_string(),
                dev.trace_retention_days.to_string(),
            ),
            (
                "daemon.archive_max_bytes".to_string(),
                daemon.archive_max_bytes.to_string(),
            ),
            (
                "daemon.archive_retention_days".to_string(),
                daemon.archive_retention_days.to_string(),
            ),
            (
                "daemon.relevance_sampling_enabled".to_string(),
                daemon.relevance_sampling_enabled.to_string(),
            ),
            (
                "daemon.relevance_sampling_interval_secs".to_string(),
                daemon.relevance_sampling_interval_secs.to_string(),
            ),
            (
                "daemon.relevance_sampling_sample_size".to_string(),
                daemon.relevance_sampling_sample_size.to_string(),
            ),
            (
                "memory.session_learn_auto".to_string(),
                memory.session_learn_auto.to_string(),
            ),
            (
                "memory.decay.curated_importance_floor".to_string(),
                memory.decay.curated_importance_floor.to_string(),
            ),
            (
                "memory.decay.promote_on_access".to_string(),
                memory.decay.promote_on_access.to_string(),
            ),
            // Code section
            (
                "code.enabled".to_string(),
                self.code.clone().unwrap_or_default().enabled.to_string(),
            ),
            (
                "code.watch_paths".to_string(),
                self.code.clone().unwrap_or_default().watch_paths.join(","),
            ),
            (
                "code.exclude_patterns".to_string(),
                self.code
                    .clone()
                    .unwrap_or_default()
                    .exclude_patterns
                    .join(","),
            ),
            (
                "code.extensions".to_string(),
                self.code.clone().unwrap_or_default().extensions.join(","),
            ),
            (
                "code.index_interval_secs".to_string(),
                self.code
                    .clone()
                    .unwrap_or_default()
                    .index_interval_secs
                    .to_string(),
            ),
            (
                "code.debounce_ms".to_string(),
                self.code
                    .clone()
                    .unwrap_or_default()
                    .debounce_ms
                    .to_string(),
            ),
            // Staging section
            (
                "staging.staging_dir".to_string(),
                staging.staging_dir.clone().unwrap_or_default(),
            ),
            (
                "staging.scratch_root".to_string(),
                staging.scratch_root.clone().unwrap_or_default(),
            ),
            (
                "staging.tmpfs_warning_threshold_bytes".to_string(),
                staging.tmpfs_warning_threshold_bytes.to_string(),
            ),
            // Issues section
            ("issues.repo".to_string(), issues.repo.unwrap_or_default()),
            (
                "issues.components.cassy".to_string(),
                issue_repos.cassy,
            ),
            (
                "issues.components.mecha_cassy".to_string(),
                issue_repos.mecha_cassy,
            ),
            (
                "issues.components.cloud".to_string(),
                issue_repos.cloud,
            ),
            // Notifications section
            (
                "notifications.enabled".to_string(),
                notifications.enabled.to_string(),
            ),
            (
                "notifications.sound_enabled".to_string(),
                notifications.sound_enabled.to_string(),
            ),
            (
                "notifications.display_duration_secs".to_string(),
                notifications.display_duration_secs.to_string(),
            ),
            (
                "notifications.max_visible".to_string(),
                notifications.max_visible.to_string(),
            ),
            // Notifications.tasks section
            (
                "notifications.tasks.on_created".to_string(),
                notifications.tasks.on_created.to_string(),
            ),
            (
                "notifications.tasks.on_started".to_string(),
                notifications.tasks.on_started.to_string(),
            ),
            (
                "notifications.tasks.on_closed".to_string(),
                notifications.tasks.on_closed.to_string(),
            ),
            (
                "notifications.tasks.on_updated".to_string(),
                notifications.tasks.on_updated.to_string(),
            ),
            // Notifications.entries section
            (
                "notifications.entries.on_added".to_string(),
                notifications.entries.on_added.to_string(),
            ),
            (
                "notifications.entries.on_updated".to_string(),
                notifications.entries.on_updated.to_string(),
            ),
            (
                "notifications.entries.on_deleted".to_string(),
                notifications.entries.on_deleted.to_string(),
            ),
            // Notifications.rules section
            (
                "notifications.rules.on_created".to_string(),
                notifications.rules.on_created.to_string(),
            ),
            (
                "notifications.rules.on_promoted".to_string(),
                notifications.rules.on_promoted.to_string(),
            ),
            (
                "notifications.rules.on_demoted".to_string(),
                notifications.rules.on_demoted.to_string(),
            ),
            // Notifications.skills section
            (
                "notifications.skills.on_created".to_string(),
                notifications.skills.on_created.to_string(),
            ),
            (
                "notifications.skills.on_enabled".to_string(),
                notifications.skills.on_enabled.to_string(),
            ),
            (
                "notifications.skills.on_disabled".to_string(),
                notifications.skills.on_disabled.to_string(),
            ),
            // Agent section
            (
                "agent.name".to_string(),
                self.agent.clone().unwrap_or_default().name,
            ),
            (
                "agent.max_concurrent_tasks".to_string(),
                self.agent
                    .clone()
                    .unwrap_or_default()
                    .max_concurrent_tasks
                    .to_string(),
            ),
            (
                "agent.capabilities".to_string(),
                self.agent
                    .clone()
                    .unwrap_or_default()
                    .capabilities
                    .join(","),
            ),
            // Coordination section
            (
                "coordination.mode".to_string(),
                match self.coordination.clone().unwrap_or_default().mode {
                    CoordinationMode::Local => "local".to_string(),
                    CoordinationMode::Cloud => "cloud".to_string(),
                },
            ),
            (
                "coordination.cloud_url".to_string(),
                self.coordination
                    .clone()
                    .unwrap_or_default()
                    .cloud_url
                    .unwrap_or_default(),
            ),
            // Lease section
            (
                "lease.default_duration_mins".to_string(),
                self.lease
                    .clone()
                    .unwrap_or_default()
                    .default_duration_mins
                    .to_string(),
            ),
            (
                "lease.max_duration_mins".to_string(),
                self.lease
                    .clone()
                    .unwrap_or_default()
                    .max_duration_mins
                    .to_string(),
            ),
            (
                "lease.heartbeat_interval_secs".to_string(),
                self.lease
                    .clone()
                    .unwrap_or_default()
                    .heartbeat_interval_secs
                    .to_string(),
            ),
            (
                "lease.expiry_grace_secs".to_string(),
                self.lease
                    .clone()
                    .unwrap_or_default()
                    .expiry_grace_secs
                    .to_string(),
            ),
            // Verification section
            (
                "verification.enabled".to_string(),
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .enabled
                    .to_string(),
            ),
            (
                "verification.model".to_string(),
                self.verification.clone().unwrap_or_default().model,
            ),
            (
                "verification.force_bypass_allowed".to_string(),
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .force_bypass_allowed
                    .to_string(),
            ),
            (
                "verification.timeout_secs".to_string(),
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .timeout_secs
                    .to_string(),
            ),
            (
                "verification.patterns.todo_comments".to_string(),
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .patterns
                    .todo_comments
                    .to_string(),
            ),
            (
                "verification.patterns.temporal_shortcuts".to_string(),
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .patterns
                    .temporal_shortcuts
                    .to_string(),
            ),
            (
                "verification.patterns.stub_implementations".to_string(),
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .patterns
                    .stub_implementations
                    .to_string(),
            ),
            (
                "verification.patterns.empty_error_handlers".to_string(),
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .patterns
                    .empty_error_handlers
                    .to_string(),
            ),
            (
                "verification.patterns.custom".to_string(),
                self.verification
                    .clone()
                    .unwrap_or_default()
                    .patterns
                    .custom
                    .join(","),
            ),
            // Worktrees section
            (
                "worktrees.enabled".to_string(),
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .enabled
                    .to_string(),
            ),
            (
                "worktrees.base_path".to_string(),
                self.worktrees.clone().unwrap_or_default().base_path,
            ),
            (
                "worktrees.branch_prefix".to_string(),
                self.worktrees.clone().unwrap_or_default().branch_prefix,
            ),
            (
                "worktrees.auto_merge".to_string(),
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .auto_merge
                    .to_string(),
            ),
            (
                "worktrees.cleanup_on_close".to_string(),
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .cleanup_on_close
                    .to_string(),
            ),
            (
                "worktrees.promote_entries_on_merge".to_string(),
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .promote_entries_on_merge
                    .to_string(),
            ),
            (
                "worktrees.abandon_ttl_hours".to_string(),
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .abandon_ttl_hours
                    .to_string(),
            ),
            (
                "worktrees.global_sweep_debounce_secs".to_string(),
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .global_sweep_debounce_secs
                    .to_string(),
            ),
            (
                "worktrees.sweep_claude_agent_dirs".to_string(),
                self.worktrees
                    .clone()
                    .unwrap_or_default()
                    .sweep_claude_agent_dirs
                    .to_string(),
            ),
            // Telemetry section
            (
                "telemetry.enabled".to_string(),
                self.telemetry
                    .clone()
                    .unwrap_or_default()
                    .enabled
                    .to_string(),
            ),
            (
                "telemetry.consent_given".to_string(),
                self.telemetry
                    .clone()
                    .unwrap_or_default()
                    .consent_given
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "not set".to_string()),
            ),
            // LLM section
            (
                "llm.harness".to_string(),
                self.llm()
                    .harness
                    .clone()
                    .unwrap_or_else(|| "(default)".to_string()),
            ),
            (
                "llm.model".to_string(),
                self.llm()
                    .model
                    .clone()
                    .unwrap_or_else(|| "(default)".to_string()),
            ),
            (
                "llm.reasoning_effort".to_string(),
                self.llm()
                    .reasoning_effort
                    .clone()
                    .unwrap_or_else(|| "(default)".to_string()),
            ),
            (
                "llm.supervisor.harness".to_string(),
                self.llm()
                    .supervisor
                    .as_ref()
                    .and_then(|s| s.harness.clone())
                    .unwrap_or_else(|| "(inherit)".to_string()),
            ),
            (
                "llm.supervisor.model".to_string(),
                self.llm()
                    .supervisor
                    .as_ref()
                    .and_then(|s| s.model.clone())
                    .unwrap_or_else(|| "(inherit)".to_string()),
            ),
            (
                "llm.worker.harness".to_string(),
                self.llm()
                    .worker
                    .as_ref()
                    .and_then(|s| s.harness.clone())
                    .unwrap_or_else(|| "(inherit)".to_string()),
            ),
            (
                "llm.worker.model".to_string(),
                self.llm()
                    .worker
                    .as_ref()
                    .and_then(|s| s.model.clone())
                    .unwrap_or_else(|| "(inherit)".to_string()),
            ),
        ]
    }

    /// Get notification config with defaults
    pub fn notifications(&self) -> NotificationConfig {
        self.notifications.clone().unwrap_or_default()
    }

    /// Get skill validation sandbox policy with defaults.
    pub fn skill_validation(&self) -> SkillValidationConfig {
        self.skill_validation.clone().unwrap_or_default()
    }

    /// Check if notifications are enabled
    pub fn notifications_enabled(&self) -> bool {
        self.notifications
            .as_ref()
            .map(|n| n.enabled)
            .unwrap_or(true)
    }

    /// Get agent config with defaults
    pub fn agent(&self) -> AgentConfig {
        self.agent.clone().unwrap_or_default()
    }

    /// Get coordination config with defaults
    pub fn coordination(&self) -> CoordinationConfig {
        self.coordination.clone().unwrap_or_default()
    }

    /// Get lease config with defaults
    pub fn lease(&self) -> LeaseConfig {
        self.lease.clone().unwrap_or_default()
    }

    /// Get verification config with defaults
    pub fn verification(&self) -> VerificationConfig {
        self.verification.clone().unwrap_or_default()
    }

    /// Get tasks config with defaults
    pub fn tasks(&self) -> TasksConfig {
        self.tasks.clone().unwrap_or_default()
    }

    /// Get worktrees config with defaults
    pub fn worktrees(&self) -> WorktreesConfig {
        self.worktrees.clone().unwrap_or_default()
    }

    /// Get telemetry config with defaults
    pub fn telemetry(&self) -> TelemetryConfig {
        self.telemetry.clone().unwrap_or_default()
    }

    /// Check if worktrees are enabled
    pub fn worktrees_enabled(&self) -> bool {
        self.worktrees.as_ref().map(|w| w.enabled).unwrap_or(false)
    }

    /// Check if exit blocking is enabled
    pub fn block_exit_on_open(&self) -> bool {
        self.tasks
            .as_ref()
            .map(|t| t.block_exit_on_open)
            .unwrap_or(true)
    }

    /// Check if verification is enabled
    pub fn verification_enabled(&self) -> bool {
        self.verification
            .as_ref()
            .map(|v| v.enabled)
            .unwrap_or(true)
    }

    /// Get theme config with defaults
    pub fn theme(&self) -> crate::ui::theme::ThemeConfig {
        self.theme.clone().unwrap_or_default()
    }

    /// Get hooks config with defaults
    pub fn hooks(&self) -> HookConfig {
        self.hooks.clone().unwrap_or_default()
    }

    /// Get code indexing config with defaults
    pub fn code(&self) -> CodeConfig {
        self.code.clone().unwrap_or_default()
    }

    /// Get orchestration config with defaults
    pub fn orchestration(&self) -> OrchestrationConfig {
        self.orchestration.clone().unwrap_or_default()
    }

    /// Get factory config with defaults
    pub fn factory(&self) -> FactoryConfig {
        self.factory.clone().unwrap_or_default()
    }

    /// Get LLM config with defaults
    pub fn llm(&self) -> LlmConfig {
        self.llm.clone().unwrap_or_default()
    }

    /// Get memory lifecycle configuration with defaults.
    pub fn memory(&self) -> MemoryConfig {
        self.memory.clone().unwrap_or_default()
    }
}
