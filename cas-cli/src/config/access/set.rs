use crate::config::*;

impl Config {
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), MemError> {
        // "(default)" is the seed sentinel used for fields whose default is
        // not a single literal value (llm.harness/model/reasoning_effort —
        // see cas-05e3/cas-fbac): resolution depends on role, so there's no
        // one concrete string to show as `default:` in the registry.
        // `cas config reset <key>` / the TUI 'd' key / the interactive
        // editor all call `set(key, meta.default)`, which for these fields
        // means `set(key, "(default)")` — meaning "clear the field back to
        // unset", NOT "set it to the literal value `(default)`". The
        // documented way to clear one of these fields directly
        // (`cas config set llm.harness ""`) means the same thing.
        //
        // Both spellings must be resolved to an empty string BEFORE running
        // `meta.validate`, not after: a `Constraint::OneOf` field (like
        // llm.harness) rejects "(default)" AND "" outright (neither is
        // "claude" or "codex"). That turned `cas config reset llm.harness`
        // into a hard error the moment llm.harness's seed default stopped
        // being the literal "claude", and left plain `set(key, "")` with no
        // way to reach the clear branch at all (cas-fbac). Scoped to fields
        // whose `default` is literally "(default)" — every other field's
        // constraints (e.g. `NotEmpty`) still apply to "" unchanged. Every
        // match arm below already treats an empty string as "clear this
        // field to None", so routing both spellings to "" here reuses that
        // existing clear-path instead of adding a second one.
        let value = if let Some(meta) = meta::registry().get(key) {
            let is_default_shaped = meta.default == "(default)";
            if is_default_shaped && (value == "(default)" || value.is_empty()) {
                ""
            } else {
                meta.validate(value).map_err(MemError::Parse)?;
                value
            }
        } else {
            value
        };

        match key {
            // Sync section
            "sync.enabled" => {
                self.sync.enabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "sync.target" => {
                self.sync.target = value.to_string();
            }
            "sync.min_helpful" => {
                self.sync.min_helpful = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "sync.promotion_threshold" => {
                self.sync.promotion_threshold = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "sync.demotion_threshold" => {
                self.sync.demotion_threshold = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "sync.promotion_evidence" => {
                self.sync.promotion_evidence =
                    parse_promotion_evidence(value).map_err(MemError::Parse)?;
            }
            "skill_validation.require_sandbox" => {
                let skill_validation = self
                    .skill_validation
                    .get_or_insert_with(SkillValidationConfig::default);
                skill_validation.require_sandbox = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "skills.optional" => {
                let skills = self.skills.get_or_insert_with(SkillsConfig::default);
                skills.optional = value
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();
            }
            // Cloud section
            "cloud.auto_sync" => {
                let cloud = self.cloud.get_or_insert_with(CloudSyncConfig::default);
                cloud.auto_sync = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "cloud.interval_secs" => {
                let cloud = self.cloud.get_or_insert_with(CloudSyncConfig::default);
                cloud.interval_secs = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "cloud.pull_on_start" => {
                let cloud = self.cloud.get_or_insert_with(CloudSyncConfig::default);
                cloud.pull_on_start = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "cloud.max_retries" => {
                let cloud = self.cloud.get_or_insert_with(CloudSyncConfig::default);
                cloud.max_retries = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "cloud.queue_pending_warning" => {
                let cloud = self.cloud.get_or_insert_with(CloudSyncConfig::default);
                cloud.queue_pending_warning = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "cloud.queue_oldest_warning_secs" => {
                let cloud = self.cloud.get_or_insert_with(CloudSyncConfig::default);
                cloud.queue_oldest_warning_secs = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            // Hooks section
            "hooks.capture_enabled" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.capture_enabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "hooks.capture_tools" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.capture_tools = value.split(',').map(|s| s.trim().to_string()).collect();
            }
            "hooks.inject_context" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.inject_context = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "hooks.context_limit" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.context_limit = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "hooks.generate_summaries" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.generate_summaries = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "hooks.token_budget" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.token_budget = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "hooks.ai_context" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.ai_context = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "hooks.ai_model" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.ai_model = value.to_string();
            }
            "hooks.ai_fallback" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.ai_fallback = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "hooks.minimal_start" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.minimal_start = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            // Hooks.plan_mode section
            "hooks.plan_mode.enabled" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.plan_mode.enabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "hooks.plan_mode.token_budget" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.plan_mode.token_budget = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "hooks.plan_mode.task_limit" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.plan_mode.task_limit = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "hooks.plan_mode.show_dependencies" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.plan_mode.show_dependencies = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "hooks.plan_mode.include_closed" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.plan_mode.include_closed = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "hooks.plan_mode.semantic_search" => {
                let hooks = self.hooks.get_or_insert_with(HookConfig::default);
                hooks.plan_mode.semantic_search = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            // Tasks section
            "tasks.commit_nudge_on_close" => {
                let tasks = self.tasks.get_or_insert_with(TasksConfig::default);
                tasks.commit_nudge_on_close = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "tasks.block_exit_on_open" => {
                let tasks = self.tasks.get_or_insert_with(TasksConfig::default);
                tasks.block_exit_on_open = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            // Dev section
            "dev.dev_mode" => {
                let dev = self.dev.get_or_insert_with(DevConfig::default);
                dev.dev_mode = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "dev.trace_commands" => {
                let dev = self.dev.get_or_insert_with(DevConfig::default);
                dev.trace_commands = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "dev.trace_store_ops" => {
                let dev = self.dev.get_or_insert_with(DevConfig::default);
                dev.trace_store_ops = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "dev.trace_claude_api" => {
                let dev = self.dev.get_or_insert_with(DevConfig::default);
                dev.trace_claude_api = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "dev.trace_hooks" => {
                let dev = self.dev.get_or_insert_with(DevConfig::default);
                dev.trace_hooks = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "dev.trace_retention_days" => {
                let dev = self.dev.get_or_insert_with(DevConfig::default);
                dev.trace_retention_days = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            // Daemon section
            "daemon.archive_max_bytes" => {
                let daemon = self.daemon.get_or_insert_with(DaemonSettings::default);
                daemon.archive_max_bytes = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "daemon.archive_retention_days" => {
                let daemon = self.daemon.get_or_insert_with(DaemonSettings::default);
                daemon.archive_retention_days = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "daemon.relevance_sampling_enabled" => {
                let daemon = self.daemon.get_or_insert_with(DaemonSettings::default);
                daemon.relevance_sampling_enabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "daemon.relevance_sampling_interval_secs" => {
                let daemon = self.daemon.get_or_insert_with(DaemonSettings::default);
                daemon.relevance_sampling_interval_secs = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "daemon.relevance_sampling_sample_size" => {
                let daemon = self.daemon.get_or_insert_with(DaemonSettings::default);
                daemon.relevance_sampling_sample_size = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            // Memory section
            "memory.session_learn_auto" => {
                let memory = self.memory.get_or_insert_with(MemoryConfig::default);
                memory.session_learn_auto = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "memory.decay.curated_importance_floor" => {
                let floor: f32 = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid number value: {value}")))?;
                if !floor.is_finite() || !(0.0..=1.0).contains(&floor) {
                    return Err(MemError::Parse(format!(
                        "Importance floor must be between 0.0 and 1.0, got: {value}"
                    )));
                }
                self.memory
                    .get_or_insert_with(MemoryConfig::default)
                    .decay
                    .curated_importance_floor = floor;
            }
            "memory.decay.promote_on_access" => {
                let memory = self.memory.get_or_insert_with(MemoryConfig::default);
                memory.decay.promote_on_access = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            // Code section
            "code.enabled" => {
                let code = self.code.get_or_insert_with(CodeConfig::default);
                code.enabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "code.watch_paths" => {
                let code = self.code.get_or_insert_with(CodeConfig::default);
                code.watch_paths = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "code.exclude_patterns" => {
                let code = self.code.get_or_insert_with(CodeConfig::default);
                code.exclude_patterns = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "code.extensions" => {
                let code = self.code.get_or_insert_with(CodeConfig::default);
                code.extensions = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "code.index_interval_secs" => {
                let code = self.code.get_or_insert_with(CodeConfig::default);
                code.index_interval_secs = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "code.debounce_ms" => {
                let code = self.code.get_or_insert_with(CodeConfig::default);
                code.debounce_ms = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            // Staging section
            "staging.staging_dir" | "staging.large_artifact_dir" => {
                let staging = self.staging.get_or_insert_with(StagingConfig::default);
                staging.staging_dir = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "staging.scratch_root" => {
                let staging = self.staging.get_or_insert_with(StagingConfig::default);
                staging.scratch_root = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "staging.tmpfs_warning_threshold_bytes" => {
                let staging = self.staging.get_or_insert_with(StagingConfig::default);
                staging.tmpfs_warning_threshold_bytes = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            // Issues section
            "issues.repo" => {
                let issues = self.issues.get_or_insert_with(IssuesConfig::default);
                issues.repo = if value.trim().is_empty() {
                    None
                } else {
                    Some(value.trim().to_string())
                };
            }
            "issues.components.cassy" => {
                let issues = self.issues.get_or_insert_with(IssuesConfig::default);
                let components = issues
                    .components
                    .get_or_insert_with(IssueComponentsConfig::default);
                components.cassy = if value.trim().is_empty() {
                    None
                } else {
                    Some(value.trim().to_string())
                };
            }
            "issues.components.mecha_cassy" => {
                let issues = self.issues.get_or_insert_with(IssuesConfig::default);
                let components = issues
                    .components
                    .get_or_insert_with(IssueComponentsConfig::default);
                components.mecha_cassy = if value.trim().is_empty() {
                    None
                } else {
                    Some(value.trim().to_string())
                };
            }
            "issues.components.cloud" => {
                let issues = self.issues.get_or_insert_with(IssuesConfig::default);
                let components = issues
                    .components
                    .get_or_insert_with(IssueComponentsConfig::default);
                components.cloud = if value.trim().is_empty() {
                    None
                } else {
                    Some(value.trim().to_string())
                };
            }
            // Release section
            "release.claude_account_allowlist" => {
                let release = self.release.get_or_insert_with(ReleaseConfig::default);
                release.claude_account_allowlist = value
                    .split(',')
                    .map(|entry| entry.trim().to_ascii_lowercase())
                    .filter(|entry| !entry.is_empty())
                    .collect();
            }
            // Notifications section
            "notifications.enabled" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.enabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "notifications.sound_enabled" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.sound_enabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "notifications.display_duration_secs" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.display_duration_secs = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "notifications.max_visible" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.max_visible = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            // Notifications.tasks section
            "notifications.tasks.on_created" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.tasks.on_created = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "notifications.tasks.on_started" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.tasks.on_started = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "notifications.tasks.on_closed" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.tasks.on_closed = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "notifications.tasks.on_updated" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.tasks.on_updated = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            // Notifications.entries section
            "notifications.entries.on_added" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.entries.on_added = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "notifications.entries.on_updated" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.entries.on_updated = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "notifications.entries.on_deleted" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.entries.on_deleted = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            // Notifications.rules section
            "notifications.rules.on_created" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.rules.on_created = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "notifications.rules.on_promoted" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.rules.on_promoted = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "notifications.rules.on_demoted" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.rules.on_demoted = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            // Notifications.skills section
            "notifications.skills.on_created" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.skills.on_created = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "notifications.skills.on_enabled" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.skills.on_enabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "notifications.skills.on_disabled" => {
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.skills.on_disabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            // Agent section
            "agent.name" => {
                let agent = self.agent.get_or_insert_with(AgentConfig::default);
                agent.name = value.to_string();
            }
            "agent.max_concurrent_tasks" => {
                let agent = self.agent.get_or_insert_with(AgentConfig::default);
                agent.max_concurrent_tasks = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "agent.capabilities" => {
                let agent = self.agent.get_or_insert_with(AgentConfig::default);
                agent.capabilities = value.split(',').map(|s| s.trim().to_string()).collect();
            }
            // Coordination section
            "coordination.mode" => {
                let coordination = self
                    .coordination
                    .get_or_insert_with(CoordinationConfig::default);
                coordination.mode = match value.to_lowercase().as_str() {
                    "local" => CoordinationMode::Local,
                    "cloud" => CoordinationMode::Cloud,
                    _ => {
                        return Err(MemError::Parse(format!(
                            "Invalid coordination mode: {value} (expected 'local' or 'cloud')"
                        )));
                    }
                };
            }
            "coordination.cloud_url" => {
                let coordination = self
                    .coordination
                    .get_or_insert_with(CoordinationConfig::default);
                coordination.cloud_url = Some(value.to_string());
            }
            // Lease section
            "lease.default_duration_mins" => {
                let lease = self.lease.get_or_insert_with(LeaseConfig::default);
                lease.default_duration_mins = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "lease.max_duration_mins" => {
                let lease = self.lease.get_or_insert_with(LeaseConfig::default);
                lease.max_duration_mins = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "lease.heartbeat_interval_secs" => {
                let lease = self.lease.get_or_insert_with(LeaseConfig::default);
                lease.heartbeat_interval_secs = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "lease.expiry_grace_secs" => {
                let lease = self.lease.get_or_insert_with(LeaseConfig::default);
                lease.expiry_grace_secs = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            // Verification section
            "verification.enabled" => {
                let verification = self
                    .verification
                    .get_or_insert_with(VerificationConfig::default);
                verification.enabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "verification.model" => {
                let verification = self
                    .verification
                    .get_or_insert_with(VerificationConfig::default);
                verification.model = value.to_string();
            }
            "verification.force_bypass_allowed" => {
                let verification = self
                    .verification
                    .get_or_insert_with(VerificationConfig::default);
                verification.force_bypass_allowed = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "verification.timeout_secs" => {
                let verification = self
                    .verification
                    .get_or_insert_with(VerificationConfig::default);
                verification.timeout_secs = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
            }
            "verification.patterns.todo_comments" => {
                let verification = self
                    .verification
                    .get_or_insert_with(VerificationConfig::default);
                verification.patterns.todo_comments = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "verification.patterns.temporal_shortcuts" => {
                let verification = self
                    .verification
                    .get_or_insert_with(VerificationConfig::default);
                verification.patterns.temporal_shortcuts = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "verification.patterns.stub_implementations" => {
                let verification = self
                    .verification
                    .get_or_insert_with(VerificationConfig::default);
                verification.patterns.stub_implementations = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "verification.patterns.empty_error_handlers" => {
                let verification = self
                    .verification
                    .get_or_insert_with(VerificationConfig::default);
                verification.patterns.empty_error_handlers = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "verification.patterns.custom" => {
                let verification = self
                    .verification
                    .get_or_insert_with(VerificationConfig::default);
                verification.patterns.custom = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            // Worktrees section
            "worktrees.enabled" => {
                let worktrees = self.worktrees.get_or_insert_with(WorktreesConfig::default);
                worktrees.enabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "worktrees.base_path" => {
                let worktrees = self.worktrees.get_or_insert_with(WorktreesConfig::default);
                worktrees.base_path = value.to_string();
            }
            "worktrees.branch_prefix" => {
                let worktrees = self.worktrees.get_or_insert_with(WorktreesConfig::default);
                worktrees.branch_prefix = value.to_string();
            }
            "worktrees.auto_merge" => {
                let worktrees = self.worktrees.get_or_insert_with(WorktreesConfig::default);
                worktrees.auto_merge = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "worktrees.cleanup_on_close" => {
                let worktrees = self.worktrees.get_or_insert_with(WorktreesConfig::default);
                worktrees.cleanup_on_close = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "worktrees.promote_entries_on_merge" => {
                let worktrees = self.worktrees.get_or_insert_with(WorktreesConfig::default);
                worktrees.promote_entries_on_merge = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "worktrees.abandon_ttl_hours" => {
                let hours: u32 = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
                let worktrees = self.worktrees.get_or_insert_with(WorktreesConfig::default);
                worktrees.abandon_ttl_hours = hours;
            }
            "worktrees.global_sweep_debounce_secs" => {
                let secs: u64 = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid integer value: {value}")))?;
                let worktrees = self.worktrees.get_or_insert_with(WorktreesConfig::default);
                worktrees.global_sweep_debounce_secs = secs;
            }
            "worktrees.sweep_claude_agent_dirs" => {
                let worktrees = self.worktrees.get_or_insert_with(WorktreesConfig::default);
                worktrees.sweep_claude_agent_dirs = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            // Telemetry section
            "telemetry.enabled" => {
                let telemetry = self.telemetry.get_or_insert_with(TelemetryConfig::default);
                telemetry.enabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "telemetry.consent_given" => {
                let telemetry = self.telemetry.get_or_insert_with(TelemetryConfig::default);
                telemetry.consent_given = Some(
                    value
                        .parse()
                        .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?,
                );
            }
            // LLM section
            "llm.harness" => {
                let llm = self.llm.get_or_insert_with(LlmConfig::default);
                llm.harness = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "llm.model" => {
                let llm = self.llm.get_or_insert_with(LlmConfig::default);
                llm.model = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "llm.reasoning_effort" => {
                let llm = self.llm.get_or_insert_with(LlmConfig::default);
                llm.reasoning_effort = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "llm.supervisor.harness" => {
                let llm = self.llm.get_or_insert_with(LlmConfig::default);
                let sup = llm.supervisor.get_or_insert(LlmRoleConfig {
                    harness: None,
                    model: None,
                    reasoning_effort: None,
                });
                sup.harness = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "llm.supervisor.model" => {
                let llm = self.llm.get_or_insert_with(LlmConfig::default);
                let sup = llm.supervisor.get_or_insert(LlmRoleConfig {
                    harness: None,
                    model: None,
                    reasoning_effort: None,
                });
                sup.model = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "llm.supervisor.reasoning_effort" => {
                let llm = self.llm.get_or_insert_with(LlmConfig::default);
                let sup = llm.supervisor.get_or_insert(LlmRoleConfig {
                    harness: None,
                    model: None,
                    reasoning_effort: None,
                });
                sup.reasoning_effort = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "llm.worker.harness" => {
                let llm = self.llm.get_or_insert_with(LlmConfig::default);
                let worker = llm.worker.get_or_insert(LlmRoleConfig {
                    harness: None,
                    model: None,
                    reasoning_effort: None,
                });
                worker.harness = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "llm.worker.model" => {
                let llm = self.llm.get_or_insert_with(LlmConfig::default);
                let worker = llm.worker.get_or_insert(LlmRoleConfig {
                    harness: None,
                    model: None,
                    reasoning_effort: None,
                });
                worker.model = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "llm.worker.reasoning_effort" => {
                let llm = self.llm.get_or_insert_with(LlmConfig::default);
                let worker = llm.worker.get_or_insert(LlmRoleConfig {
                    harness: None,
                    model: None,
                    reasoning_effort: None,
                });
                worker.reasoning_effort = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            _ => {
                return Err(MemError::Other(format!("Unknown config key: {key}")));
            }
        }
        Ok(())
    }
}
