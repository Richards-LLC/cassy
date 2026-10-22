use crate::config::*;

/// Only allow secure webhook destinations, except HTTP services bound to
/// localhost/loopback for development.
pub(crate) fn validate_notification_webhook_url(
    value: &str,
) -> Result<Option<url::Url>, MemError> {
    if value.is_empty() {
        return Ok(None);
    }
    if value
        .chars()
        .any(|character| {
            character == '\\' || character.is_whitespace() || character.is_control()
        })
    {
        return Err(MemError::Parse(
            "notifications.webhook_url must not contain backslashes or whitespace".into(),
        ));
    }

    let url = url::Url::parse(value).map_err(|_| {
        MemError::Parse(
            "notifications.webhook_url must be a valid HTTPS URL or an HTTP localhost URL"
                .into(),
        )
    })?;
    let is_https = url.scheme() == "https" && url.host().is_some();
    let is_loopback_http = url.scheme() == "http"
        && match url.host() {
            Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(address)) => address.is_loopback(),
            Some(url::Host::Ipv6(address)) => address.is_loopback(),
            None => false,
        };
    if !is_https && !is_loopback_http {
        return Err(MemError::Parse(
            "notifications.webhook_url must use HTTPS; HTTP is allowed only for localhost"
                .into(),
        ));
    }
    Ok(Some(url))
}

impl Config {
    /// GH #1182: replace `[factory] write_roots`. Only the operator CLI path
    /// (`cas config set factory.write_roots`) calls this, after resolving
    /// every entry with `operator_policy::parse_project_write_roots`;
    /// [`Config::set`] refuses the key.
    pub(crate) fn set_factory_write_roots(&mut self, roots: Vec<String>) {
        self.factory.get_or_insert_with(FactoryConfig::default).write_roots = roots;
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), MemError> {
        // "(default)" is the seed sentinel used for fields whose default is
        // not a single literal value (llm.harness/model/reasoning_effort —
        // see cas-05e3/cas-fbac): resolution depends on role, so there's no
        // one concrete string to show as `default:` in the registry.
        // The TUI 'd' key and the interactive editor call
        // `set(key, meta.default)`, which for these fields
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
                if meta.value_type == ConfigType::Bool {
                    match value.trim().to_ascii_lowercase().as_str() {
                        "true" | "yes" | "1" => "true",
                        "false" | "no" | "0" => "false",
                        _ => {
                            return Err(MemError::Parse(format!(
                                "Invalid boolean value for {key}: {value}"
                            )));
                        }
                    }
                } else {
                    value
                }
            }
        } else {
            value
        };

        match key {
            "slack.stakeholder_contacts" => {
                let policies: Vec<StakeholderContactPolicy> = serde_json::from_str(value)
                    .map_err(|error| MemError::Parse(format!("Invalid stakeholder_contacts JSON: {error}")))?;
                let mut seen = std::collections::BTreeSet::new();
                for policy in &policies {
                    policy.validate().map_err(MemError::Parse)?;
                    if !seen.insert((&policy.workspace_id, &policy.user_id)) {
                        return Err(MemError::Parse("Duplicate stakeholder workspace/user policy".into()));
                    }
                }
                self.slack.get_or_insert_with(Default::default).stakeholder_contacts = policies;
            }
            "factory.commander_mirror_enabled" => {
                self.factory.get_or_insert_with(Default::default).commander_mirror_enabled =
                    value.parse().map_err(|_| MemError::Parse(
                        "Invalid boolean for factory.commander_mirror_enabled".into()
                    ))?;
            }
            "hub.transport_mode" => {
                let mode = match value {
                    "auto" => HubTransportMode::Auto,
                    "required" => HubTransportMode::Required,
                    "loopback" => HubTransportMode::Loopback,
                    _ => return Err(MemError::Parse("hub.transport_mode must be auto, required or loopback".into())),
                };
                self.hub.get_or_insert_with(Default::default).transport_mode = Some(mode);
            }
            "hub.tailscale_serve" => {
                self.hub
                    .get_or_insert_with(Default::default)
                    .tailscale_serve = Some(value.parse().map_err(|_| {
                    MemError::Parse("Invalid boolean for hub.tailscale_serve".into())
                })?);
            }
            "viktor.enabled" => {
                self.viktor
                    .get_or_insert_with(ViktorConfig::default)
                    .enabled = Some(value.parse().map_err(|_| {
                    MemError::Parse("Invalid boolean for viktor.enabled".into())
                })?);
            }
            "slack.transport" => {
                self.slack
                    .get_or_insert_with(SlackConfig::default)
                    .transport = match value {
                    "violet" => SlackTransport::Violet,
                    "any" => SlackTransport::Any,
                    _ => {
                        return Err(MemError::Parse(format!(
                            "Invalid Slack transport: {value}; use violet or any"
                        )));
                    }
                };
            }
            "peers.same_repo_scope" => {
                self.peers
                    .get_or_insert_with(PeersConfig::default)
                    .same_repo_scope = match value.trim() {
                    "team" => SameRepoScope::Team,
                    "own" => SameRepoScope::Own,
                    _ => {
                        return Err(MemError::Parse(format!(
                            "Invalid peers.same_repo_scope: {value}; use team or own"
                        )));
                    }
                };
            }
            "slack.wake_enabled" => {
                self.slack.get_or_insert_with(SlackConfig::default).wake_enabled =
                    Some(value.parse().map_err(|_| {
                        MemError::Parse("Invalid boolean for slack.wake_enabled".into())
                    })?)
            }
            "slack.violet_bot_user_ids" => {
                let ids: Vec<String> = value
                    .split(',')
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .collect();
                self.slack
                    .get_or_insert_with(SlackConfig::default)
                    .violet_bot_user_ids = (!ids.is_empty()).then_some(ids);
            }
            // Factory build contention
            "factory.slot_id" => {
                self.factory.get_or_insert_with(FactoryConfig::default).slot_id =
                    (!value.trim().is_empty()).then(|| value.trim().to_string());
            }
            "factory.warn_stale_assignment" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.warn_stale_assignment = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid boolean value for factory.warn_stale_assignment: {value}"))
                })?;
            }
            "factory.block_stale_assignment" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.block_stale_assignment = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid boolean value for factory.block_stale_assignment: {value}"))
                })?;
            }
            "factory.stale_threshold_commits" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.stale_threshold_commits = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid integer value for factory.stale_threshold_commits: {value}"))
                })?;
            }
            "factory.stall_threshold_secs" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.stall_threshold_secs = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid integer value for factory.stall_threshold_secs: {value}"))
                })?;
            }
            "factory.context_recycle_threshold_percent" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.context_recycle_threshold_percent = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid integer value for factory.context_recycle_threshold_percent: {value}"))
                })?;
            }
            "factory.stall_after_secs" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.stall_after_secs = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid integer value for factory.stall_after_secs: {value}"))
                })?;
            }
            "factory.delivery_stalled_priority_secs" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.delivery_stalled_priority_secs = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid integer value for factory.delivery_stalled_priority_secs: {value}"))
                })?;
            }
            "factory.delivery_stalled_normal_secs" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.delivery_stalled_normal_secs = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid integer value for factory.delivery_stalled_normal_secs: {value}"))
                })?;
            }
            "factory.strict_cli" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.strict_cli = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid boolean value for factory.strict_cli: {value}"))
                })?;
            }
            "factory.target_cache_high_watermark_percent" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.target_cache_high_watermark_percent = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid integer value for factory.target_cache_high_watermark_percent: {value}"))
                })?;
            }
            "factory.target_cache_low_watermark_percent" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.target_cache_low_watermark_percent = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid integer value for factory.target_cache_low_watermark_percent: {value}"))
                })?;
            }
            "factory.target_cache_min_idle_secs" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.target_cache_min_idle_secs = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid integer value for factory.target_cache_min_idle_secs: {value}"))
                })?;
            }
            "factory.target_cache_retention_count" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.target_cache_retention_count = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid integer value for factory.target_cache_retention_count: {value}"))
                })?;
            }
            "factory.claude.worker_mcp" => {
                let mode = value.parse().map_err(MemError::Parse)?;
                self.factory.get_or_insert_with(FactoryConfig::default).claude.worker_mcp = mode;
            }
            "factory.claude.remote_control" => {
                let enabled = value.parse().map_err(|_| MemError::Other(format!("Invalid boolean value for {key}: {value}")))?;
                self.factory.get_or_insert_with(FactoryConfig::default).claude.remote_control = enabled;
            }
            "factory.claude.fallback_model" => {
                let model = cas_types::factory_claude::FactoryClaudeConfig::parse_fallback(value).map_err(MemError::Other)?;
                self.factory.get_or_insert_with(FactoryConfig::default).claude.fallback_model = model;
            }
            "factory.nice_cargo" => {
                self.factory.get_or_insert_with(FactoryConfig::default).nice_cargo =
                    value.parse().map_err(|_| MemError::Parse(format!(
                        "Invalid boolean value for factory.nice_cargo: {value}"
                    )))?;
            }
            "factory.max_concurrent_builders" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.max_concurrent_builders = value.parse().map_err(|_| {
                    MemError::Parse(format!(
                        "Invalid integer value for factory.max_concurrent_builders: {value}"
                    ))
                })?;
            }
            "factory.build_guard.max_load_ratio"
            | "factory.build_guard.max_cpu_pressure_percent"
            | "factory.build_guard.max_memory_pressure_percent" => {
                let number: f64 = value.parse().map_err(|_| MemError::Parse(format!("Invalid number for {key}: {value}")))?;
                let load_ratio = key.ends_with("max_load_ratio");
                if !number.is_finite() || if load_ratio { number <= 0.0 } else { !(0.0..=100.0).contains(&number) } {
                    return Err(MemError::Parse(format!("Invalid value for {key}: expected {}", if load_ratio { "a positive finite ratio" } else { "a percentage from 0 to 100" })));
                }
                let limits = &mut self.factory.get_or_insert_with(FactoryConfig::default).build_guard;
                match key {
                    "factory.build_guard.max_load_ratio" => limits.max_load_ratio = number,
                    "factory.build_guard.max_cpu_pressure_percent" => limits.max_cpu_pressure_percent = number,
                    _ => limits.max_memory_pressure_percent = number,
                }
            }
            "factory.build_guard.min_available_memory_gib" => {
                let gib: u32 = value.parse().map_err(|_| MemError::Parse(format!("Invalid GiB value for {key}: {value}")))?;
                self.factory.get_or_insert_with(FactoryConfig::default).build_guard.min_available_memory_gib = gib;
            }
            "factory.worker_memory_high_gib" => {
                self.factory.get_or_insert_with(FactoryConfig::default).worker_memory_high_gib =
                    value.parse().map_err(|_| MemError::Parse(format!(
                        "Invalid integer value for factory.worker_memory_high_gib: {value}"
                    )))?;
            }
            "factory.render_max_fps" | "factory.render_unfocused_fps" => {
                let fps: u32 = value
                    .parse()
                    .ok()
                    .filter(|fps| (1..=120).contains(fps))
                    .ok_or_else(|| {
                        MemError::Parse(format!(
                            "Invalid value for {key}: {value} (expected 1..=120 frames per second)"
                        ))
                    })?;
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                if key == "factory.render_max_fps" {
                    factory.render_max_fps = fps;
                } else {
                    factory.render_unfocused_fps = fps;
                }
            }
            "factory.spawn_min_free_gib" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.spawn_min_free_gib = value.parse().map_err(|_| {
                    MemError::Parse(format!(
                        "Invalid integer value for factory.spawn_min_free_gib: {value}"
                    ))
                })?;
            }
            "factory.prompt_retention_days" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.prompt_retention_days = value.parse().map_err(|_| {
                    MemError::Parse(format!(
                        "Invalid integer value for factory.prompt_retention_days: {value}"
                    ))
                })?;
            }
            "factory.event_telemetry_retention_days" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.event_telemetry_retention_days = value.parse().map_err(|_| {
                    MemError::Parse(format!(
                        "Invalid integer value for factory.event_telemetry_retention_days: {value}"
                    ))
                })?;
            }
            "factory.prompt_transcript_retention_days" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.prompt_transcript_retention_days = value.parse().map_err(|_| {
                    MemError::Parse(format!(
                        "Invalid integer value for factory.prompt_transcript_retention_days: {value}"
                    ))
                })?;
            }
            "factory.supervisor_queue_retention_days" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.supervisor_queue_retention_days = value.parse().map_err(|_| {
                    MemError::Parse(format!(
                        "Invalid integer value for factory.supervisor_queue_retention_days: {value}"
                    ))
                })?;
            }
            "factory.supervisor_message_debounce_secs" => {
                let number: u64 = value.parse().map_err(|_| MemError::Parse(format!("Invalid integer value for factory.supervisor_message_debounce_secs: {value}")))?;
                self.factory.get_or_insert_with(FactoryConfig::default).supervisor_message_debounce_secs = number;
            }
            "factory.supervisor_message_wakes_per_hour" => {
                let number: u64 = value.parse().map_err(|_| MemError::Parse(format!("Invalid integer value for factory.supervisor_message_wakes_per_hour: {value}")))?;
                self.factory.get_or_insert_with(FactoryConfig::default).supervisor_message_wakes_per_hour = number;
            }
            "factory.message_max_chars" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.message_max_chars = value.parse().map_err(|_| {
                    MemError::Parse(format!(
                        "Invalid integer value for factory.message_max_chars: {value}"
                    ))
                })?;
            }
            "factory.message_max_chars_escalation" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.message_max_chars_escalation = value.parse().map_err(|_| {
                    MemError::Parse(format!(
                        "Invalid integer value for factory.message_max_chars_escalation: {value}"
                    ))
                })?;
            }
            "factory.note_max_chars" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.note_max_chars = value.parse().map_err(|_| {
                    MemError::Parse(format!(
                        "Invalid integer value for factory.note_max_chars: {value}"
                    ))
                })?;
            }
            "factory.worker_build_jobs" | "factory.cargo_build_jobs" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.cargo_build_jobs = value.to_string();
            }
            // cas-8d54 (GH #1011): the runtime reads this key
            // (Config::configured_epic_base_branch); empty clears it.
            "factory.epic_base_branch" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                let branch = value.trim();
                factory.epic_base_branch = (!branch.is_empty()).then(|| branch.to_string());
            }
            // cas-1a05: registered factory keys that `set` did not accept.
            "factory.artifacts_root" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                let root = value.trim();
                factory.artifacts_root = (!root.is_empty()).then(|| root.to_string());
            }
            "factory.scratch_root" => {
                let root = value.trim();
                if !root.is_empty() && !std::path::Path::new(root).is_absolute() {
                    return Err(MemError::Parse(format!(
                        "factory.scratch_root must be an absolute path, got {root:?}"
                    )));
                }
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.scratch_root = (!root.is_empty()).then(|| root.to_string());
            }
            "factory.supervisor_only_mcp"
            | "factory.supervisor_only_env"
            | "factory.worker_credential_env"
            | "factory.worker_denied_clis"
            | "factory.worker_denied_mcp_tools" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                let names = value.split(',').map(str::trim).filter(|name| !name.is_empty())
                    .map(str::to_string).collect();
                if key == "factory.supervisor_only_mcp" {
                    factory.worker_policy.supervisor_only_mcp = names;
                } else if key == "factory.supervisor_only_env" {
                    factory.worker_policy.supervisor_only_env = names;
                } else if key == "factory.worker_denied_clis" {
                    factory.worker_denied_clis = names;
                } else if key == "factory.worker_denied_mcp_tools" {
                    factory.worker_denied_mcp_tools = names;
                } else {
                    factory.worker_policy.worker_credential_env = names;
                }
            }
            "factory.ai_enrichment.enabled" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.ai_enrichment.enabled = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "factory.ai_enrichment.endpoint" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.ai_enrichment.endpoint = value.trim().to_string();
            }
            "factory.ai_enrichment.provider" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.ai_enrichment.provider = value.trim().to_string();
            }
            "factory.ai_enrichment.api_key_env" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.ai_enrichment.api_key_env = value.trim().to_string();
            }
            "factory.ai_enrichment.model" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.ai_enrichment.model = value.trim().to_string();
            }
            "factory.ai_enrichment.effort" => {
                let factory = self.factory.get_or_insert_with(FactoryConfig::default);
                factory.ai_enrichment.effort = value.trim().to_string();
            }
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
            "cloud.team_only" => {
                let cloud = self.cloud.get_or_insert_with(CloudSyncConfig::default);
                cloud.team_only = value
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
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
            "hooks.stop.duplicate_detection_enabled"
            | "hooks.stop.learning_review_enabled"
            | "hooks.stop.rule_review_enabled" => {
                let enabled = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid boolean value for {key}: {value}"))
                })?;
                let stop = &mut self.hooks.get_or_insert_with(HookConfig::default).stop;
                match key {
                    "hooks.stop.duplicate_detection_enabled" => {
                        stop.duplicate_detection_enabled = enabled;
                    }
                    "hooks.stop.learning_review_enabled" => stop.learning_review_enabled = enabled,
                    _ => stop.rule_review_enabled = enabled,
                }
            }
            "hooks.stop.duplicate_detection_threshold"
            | "hooks.stop.learning_review_threshold"
            | "hooks.stop.rule_review_threshold" => {
                let threshold: usize = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid integer value for {key}: {value}"))
                })?;
                let stop = &mut self.hooks.get_or_insert_with(HookConfig::default).stop;
                match key {
                    "hooks.stop.duplicate_detection_threshold" => {
                        stop.duplicate_detection_threshold = threshold;
                    }
                    "hooks.stop.learning_review_threshold" => stop.learning_review_threshold = threshold,
                    _ => stop.rule_review_threshold = threshold,
                }
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
            "qa.user_facing_labels" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                qa.user_facing_labels = value
                    .split(',')
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();
            }
            "qa.telemetry_sweep" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                qa.telemetry_sweep = if value.trim().is_empty() {
                    None
                } else {
                    Some(value.trim().to_string())
                };
            }
            "qa.deployed_origins" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                let mut origins = Vec::new();
                for raw in value
                    .split(',')
                    .map(str::trim)
                    .filter(|raw| !raw.is_empty())
                {
                    let origin = crate::qa_evidence::url_origin(raw).ok_or_else(|| {
                        MemError::Parse(format!(
                            "qa.deployed_origins: {raw:?} is not an http(s) origin"
                        ))
                    })?;
                    if crate::qa_evidence::is_local_origin(&origin) {
                        return Err(MemError::Parse(format!(
                            "qa.deployed_origins: {origin} is local; a local build needs no deployed origin"
                        )));
                    }
                    origins.push(origin);
                }
                qa.deployed_origins = origins;
            }
            "qa.preflight_gh_token" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                qa.preflight_gh_token = value
                    .trim()
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "qa.preflight_env_files" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                qa.preflight_env_files = value
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();
            }
            "qa.post_release.command" => {
                let post = self.qa.get_or_insert_with(QaConfig::default).post_release.get_or_insert_with(Default::default);
                post.command = value.trim().to_string();
            }
            "qa.post_release.window_mins" => {
                let post = self.qa.get_or_insert_with(QaConfig::default).post_release.get_or_insert_with(Default::default);
                post.window_mins = value.parse().map_err(|_| MemError::Parse("Expected a positive integer".into()))?;
            }
            "qa.post_release.interval_mins" => {
                let post = self.qa.get_or_insert_with(QaConfig::default).post_release.get_or_insert_with(Default::default);
                post.interval_mins = value.parse().map_err(|_| MemError::Parse("Expected a positive integer".into()))?;
            }
            "qa.post_release.timeout_secs" => {
                let post = self.qa.get_or_insert_with(QaConfig::default).post_release.get_or_insert_with(Default::default);
                post.timeout_secs = value.parse().map_err(|_| MemError::Parse("Expected a positive integer".into()))?;
            }
            "qa.post_release.release_branches" => {
                let post = self.qa.get_or_insert_with(QaConfig::default).post_release.get_or_insert_with(Default::default);
                post.release_branches = value.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
            }
            "qa.post_release.env" => {
                let post = self.qa.get_or_insert_with(QaConfig::default).post_release.get_or_insert_with(Default::default);
                post.env = value.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
            }
            "qa.preflight_hook" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                qa.preflight_hook = if value.trim().is_empty() {
                    None
                } else {
                    Some(value.trim().to_string())
                };
            }
            "qa.preflight_hook_timeout_secs" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                let secs: u32 = value
                    .trim()
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid seconds value: {value}")))?;
                if secs == 0 {
                    return Err(MemError::Parse(
                        "qa.preflight_hook_timeout_secs must be at least 1".to_string(),
                    ));
                }
                qa.preflight_hook_timeout_secs = secs;
            }
            "qa.github_status" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                qa.github_status = value
                    .trim()
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "upkeep.codemap" | "upkeep.product_overview" => {
                let stage = value.trim().parse().map_err(|error: &str| MemError::Parse(error.into()))?;
                let upkeep = self.upkeep.get_or_insert_with(UpkeepConfig::default);
                let gate = key.strip_prefix("upkeep.").unwrap();
                if gate == "codemap" { upkeep.codemap = Some(stage); }
                else { upkeep.product_overview = Some(stage); }
                upkeep.observe_since.remove(gate);
            }
            "qa.registry.enforce" => {
                let stage = value.trim().parse().map_err(|error: &str| MemError::Parse(error.into()))?;
                self.qa.get_or_insert_with(QaConfig::default).registry.enforce = stage;
                if let Some(upkeep) = self.upkeep.as_mut() {
                    upkeep.observe_since.remove("registry");
                }
            }
            "qa.registry.path" => {
                let path = std::path::Path::new(value.trim());
                if path.as_os_str().is_empty() || path.is_absolute()
                    || path.components().any(|part| matches!(part, std::path::Component::ParentDir))
                {
                    return Err(MemError::Parse("qa.registry.path must be a non-empty project-relative path without ..".into()));
                }
                self.qa.get_or_insert_with(QaConfig::default).registry.path = value.trim().to_owned();
            }
            "qa.mode" => {
                let mode = match value.trim() {
                    "per_task" => QaMode::PerTask,
                    "batch" => QaMode::Batch,
                    _ => return Err(MemError::Parse("qa.mode must be per_task or batch".into())),
                };
                self.qa.get_or_insert_with(QaConfig::default).mode = mode;
            }
            "qa.independent_pass" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                qa.independent_pass = value
                    .trim()
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "qa.evidence_gate" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                qa.evidence_gate = value
                    .trim()
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid boolean value: {value}")))?;
            }
            "qa.terminal_render_paths" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                qa.terminal_render_paths = value
                    .split(',')
                    .map(str::trim)
                    .filter(|glob| !glob.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();
            }
            "qa.terminal_interaction_paths" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                qa.terminal_interaction_paths = value
                    .split(',')
                    .map(str::trim)
                    .filter(|glob| !glob.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();
            }
            "qa.user_facing_paths" | "qa.api_paths" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                let paths = value
                    .split(',')
                    .map(str::trim)
                    .filter(|glob| !glob.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();
                if key == "qa.api_paths" {
                    qa.api_paths = paths;
                } else {
                    qa.user_facing_paths = paths;
                }
            }
            "qa.pass_timeout_mins" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                let minutes: u32 = value
                    .trim()
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid minutes value: {value}")))?;
                if minutes == 0 {
                    return Err(MemError::Parse(
                        "qa.pass_timeout_mins must be at least 1".to_string(),
                    ));
                }
                qa.pass_timeout_mins = minutes;
            }
            "qa.max_rounds" => {
                let qa = self.qa.get_or_insert_with(QaConfig::default);
                let rounds: u32 = value
                    .trim()
                    .parse()
                    .map_err(|_| MemError::Parse(format!("Invalid round count: {value}")))?;
                if rounds == 0 {
                    return Err(MemError::Parse(
                        "qa.max_rounds must be at least 1".to_string(),
                    ));
                }
                qa.max_rounds = rounds;
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
            "memory.session_learn_min_turns" => {
                self.memory
                    .get_or_insert_with(MemoryConfig::default)
                    .session_learn_min_turns = value.parse::<usize>().map_err(|_| {
                    MemError::Parse(format!("Invalid nonnegative integer value: {value}"))
                })?;
            }
            "memory.session_learn_min_minutes" => {
                self.memory
                    .get_or_insert_with(MemoryConfig::default)
                    .session_learn_min_minutes = value.parse::<u64>().map_err(|_| {
                    MemError::Parse(format!("Invalid nonnegative integer value: {value}"))
                })?;
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
            "issues.components.violet" => {
                let issues = self.issues.get_or_insert_with(IssuesConfig::default);
                let components = issues
                    .components
                    .get_or_insert_with(IssueComponentsConfig::default);
                components.violet = if value.trim().is_empty() {
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
            // History section
            "history.github_repo" => {
                let history = self.history.get_or_insert_with(HistoryConfig::default);
                history.github_repo = if value.trim().is_empty() {
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
            "notifications.on_permission_prompt"
            | "notifications.on_idle_prompt"
            | "notifications.on_auth_success" => {
                let enabled = value.parse().map_err(|_| {
                    MemError::Parse(format!("Invalid boolean value for {key}: {value}"))
                })?;
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                match key {
                    "notifications.on_permission_prompt" => {
                        notifications.on_permission_prompt = enabled;
                    }
                    "notifications.on_idle_prompt" => notifications.on_idle_prompt = enabled,
                    _ => notifications.on_auth_success = enabled,
                }
            }
            "notifications.webhook_url" => {
                let url = validate_notification_webhook_url(value)?;
                let notifications = self
                    .notifications
                    .get_or_insert_with(NotificationConfig::default);
                notifications.webhook_url = url.map(|url| url.to_string());
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
            // GH #1182: operator-only. Stored in config.toml, but written only
            // by `cas config set factory.write_roots` through
            // `set_factory_write_roots`, which validates every path. This
            // generic setter (MCP, the config editor, reset) never widens it.
            "factory.write_roots" => {
                return Err(MemError::Other(
                    "factory.write_roots is operator-only: agents, the config editor and MCP\n\
                     cannot change it. The operator runs\n\
                     `cas config set factory.write_roots \"<dir>,<dir>\"` (empty clears it)."
                        .to_string(),
                ));
            }
            _ => {
                return Err(MemError::Other(format!("Unknown config key: {key}")));
            }
        }
        Ok(())
    }
}
