use crate::mcp::tools::service::imports::*;

impl CasService {
    /// Resolve the `target` parameter to an agent ID.
    /// If `target` is a name (e.g. "swift-fox"), look it up in the agent store.
    /// If `target` is already a UUID, return it directly.
    /// Returns `(agent_id, display_name)`.
    fn resolve_reminder_target(
        &self,
        target: &str,
    ) -> std::result::Result<(String, String), McpError> {
        use crate::store::open_agent_store;
        use cas_types::AgentStatus;

        let store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;

        // Try exact ID match first
        if let Ok(agent) = store.get(target) {
            return Ok((agent.id, agent.name));
        }

        // Look up by name among active/idle agents
        let agents = store.list(None).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list agents: {e}"),
            )
        })?;

        agents
            .into_iter()
            .find(|a| {
                a.name == target
                    && (a.status == AgentStatus::Active || a.status == AgentStatus::Idle)
            })
            .map(|a| (a.id, a.name))
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!("No active agent found with name or ID '{target}'"),
                )
            })
    }

    /// Create a time-based or event-based reminder
    pub(super) async fn factory_remind(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_reminder_store;
        use cas_store::ReminderTriggerType;

        let message = req.remind_message.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "remind_message is required for remind action",
            )
        })?;

        let has_delay = req.remind_delay_secs.is_some();
        let has_event = req.remind_event.is_some();

        if has_delay == has_event {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Provide exactly one of remind_delay_secs (time-based) or remind_event (event-based), not both or neither",
            ));
        }

        let store = open_reminder_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open reminder store: {e}"),
            )
        })?;

        let owner_id = self.inner.get_agent_id().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to get agent ID: {e}"),
            )
        })?;

        // Resolve target: if provided, look up agent by name/ID; otherwise self-reminder
        let (target_id, target_display) = if let Some(ref target_name) = req.target {
            let (id, name) = self.resolve_reminder_target(target_name)?;
            (Some(id), Some(name))
        } else {
            (None, None)
        };

        let ttl_secs = req.remind_ttl_secs.unwrap_or(3600);
        let cross_session = req.cross_session.unwrap_or(false);

        // cas-fcd4: bind event-based (and all) reminds to the registering factory
        // session so concurrent factories sharing cas.db do not cross-fire.
        let session_id = std::env::var("CAS_FACTORY_SESSION")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // `session_id` above is the factory daemon scope. The creator session
        // is deliberately separate: SessionEnd uses this value to cancel the
        // default one-shot reminder before a later session can receive it.
        let origin_session_id = std::env::var("CAS_SESSION_ID")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| owner_id.clone());

        if has_delay {
            let delay_secs = req.remind_delay_secs.unwrap();
            if delay_secs <= 0 {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "remind_delay_secs must be positive",
                ));
            }

            let trigger_at = chrono::Utc::now() + chrono::Duration::seconds(delay_secs);

            let id = store
                .create_with_scope(
                    &owner_id,
                    target_id.as_deref(),
                    &message,
                    ReminderTriggerType::Time,
                    Some(trigger_at),
                    None,
                    None,
                    ttl_secs,
                    session_id.as_deref(),
                    Some(&origin_session_id),
                    cross_session,
                )
                .map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to create reminder: {e}"),
                    )
                })?;

            let minutes = delay_secs / 60;
            let seconds = delay_secs % 60;
            let time_desc = if minutes > 0 && seconds > 0 {
                format!("{minutes}m {seconds}s")
            } else if minutes > 0 {
                format!("{minutes}m")
            } else {
                format!("{seconds}s")
            };

            let target_desc = match &target_display {
                Some(name) => format!(", target: {name}"),
                None => String::new(),
            };

            let scope_desc = if cross_session {
                format!(", cross-session opt-in; origin session: {origin_session_id}")
            } else {
                ", session-scoped; cancelled when this session ends".to_string()
            };
            Ok(Self::success(format!(
                "Reminder #{id} set (time-based, fires in {time_desc}{target_desc}{scope_desc})\nMessage: {message}"
            )))
        } else {
            let event_type = req.remind_event.unwrap();

            // Validate event type (must match DirectorEvent variants)
            let valid_events = [
                "task_assigned",
                "task_completed",
                "task_blocked",
                "worker_idle",
                "agent_registered",
                "epic_started",
                "epic_completed",
            ];
            if !valid_events.contains(&event_type.as_str()) {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Unknown event type: {}. Valid: {}",
                        event_type,
                        valid_events.join(", ")
                    ),
                ));
            }

            let filter = if let Some(filter_str) = &req.remind_filter {
                let parsed: serde_json::Value = serde_json::from_str(filter_str).map_err(|e| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!("Invalid JSON in remind_filter: {e}"),
                    )
                })?;
                if !parsed.is_object() {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        "remind_filter must be a JSON object",
                    ));
                }
                Some(parsed)
            } else {
                None
            };

            let id = store
                .create_with_scope(
                    &owner_id,
                    target_id.as_deref(),
                    &message,
                    ReminderTriggerType::Event,
                    None,
                    Some(&event_type),
                    filter.as_ref(),
                    ttl_secs,
                    session_id.as_deref(),
                    Some(&origin_session_id),
                    cross_session,
                )
                .map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to create reminder: {e}"),
                    )
                })?;

            let filter_desc = if let Some(f) = &filter {
                format!(" (filter: {f})")
            } else {
                String::new()
            };

            let target_desc = match &target_display {
                Some(name) => format!(", target: {name}"),
                None => String::new(),
            };

            let session_desc = match (&session_id, cross_session) {
                (Some(s), true) => format!(
                    ", factory session: {s}, cross-session opt-in; origin session: {origin_session_id}"
                ),
                (Some(s), false) => format!(
                    ", factory session: {s}, session-scoped; cancelled when this session ends"
                ),
                (None, true) => {
                    format!(", cross-session opt-in; origin session: {origin_session_id}")
                }
                (None, false) => ", session-scoped; cancelled when this session ends".to_string(),
            };

            Ok(Self::success(format!(
                "Reminder #{id} set (event-based, fires on {event_type}{filter_desc}{target_desc}{session_desc})\nMessage: {message}"
            )))
        }
    }

    /// List pending reminders relevant to the calling agent.
    /// Shows reminders the agent owns AND reminders targeting this agent.
    pub(super) async fn factory_remind_list(
        &self,
        _req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_reminder_store};
        use cas_store::ReminderTriggerType;

        let store = open_reminder_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open reminder store: {e}"),
            )
        })?;

        let my_id = self.inner.get_agent_id().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to get agent ID: {e}"),
            )
        })?;

        // Get reminders I own
        let owned = store.list_pending(&my_id).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list reminders: {e}"),
            )
        })?;

        // Also get reminders targeting me (set by other agents)
        let targeting_me = store.list_pending_for_target(&my_id).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list targeted reminders: {e}"),
            )
        })?;

        // Merge, dedup by ID (a self-reminder appears in both)
        let mut seen = std::collections::HashSet::new();
        let mut reminders = Vec::new();
        for r in owned.into_iter().chain(targeting_me) {
            if seen.insert(r.id) {
                reminders.push(r);
            }
        }

        if reminders.is_empty() {
            return Ok(Self::success("No pending reminders.".to_string()));
        }

        // Build agent ID → name map plus a session-liveness set. A reminder's
        // creator session is the authoritative lifecycle owner; an absent or
        // shut-down origin is displayed as orphaned rather than silently live.
        let (id_to_name, live_sessions): (
            std::collections::HashMap<String, String>,
            std::collections::HashSet<String>,
        ) = open_agent_store(&self.inner.cas_root)
            .ok()
            .and_then(|s| s.list(None).ok())
            .map(|agents| {
                agents.into_iter().fold(
                    (
                        std::collections::HashMap::new(),
                        std::collections::HashSet::new(),
                    ),
                    |(mut names, mut live), agent| {
                        if matches!(
                            agent.status,
                            cas_types::AgentStatus::Active | cas_types::AgentStatus::Idle
                        ) {
                            live.insert(agent.id.clone());
                        }
                        names.insert(agent.id, agent.name);
                        (names, live)
                    },
                )
            })
            .unwrap_or_default();

        let resolve = |id: &str| -> String {
            id_to_name
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.to_string())
        };

        let mut lines = vec![format!("Pending reminders ({}):", reminders.len())];

        for r in &reminders {
            let trigger_desc = match r.trigger_type {
                ReminderTriggerType::Time => {
                    if let Some(at) = r.trigger_at {
                        let now = chrono::Utc::now();
                        if at > now {
                            let remaining = (at - now).num_seconds();
                            let mins = remaining / 60;
                            let secs = remaining % 60;
                            if mins > 0 {
                                format!("time (fires in {mins}m {secs}s)")
                            } else {
                                format!("time (fires in {secs}s)")
                            }
                        } else {
                            "time (overdue)".to_string()
                        }
                    } else {
                        "time".to_string()
                    }
                }
                ReminderTriggerType::Event => {
                    let event = r.trigger_event.as_deref().unwrap_or("?");
                    if let Some(filter) = &r.trigger_filter {
                        format!("event: {event} (filter: {filter})")
                    } else {
                        format!("event: {event}")
                    }
                }
            };

            let target_desc = if r.target_id != r.owner_id {
                if r.owner_id == my_id {
                    format!(" → {}", resolve(&r.target_id))
                } else {
                    format!(" (from {})", resolve(&r.owner_id))
                }
            } else {
                String::new()
            };

            let origin = r
                .origin_session_id
                .as_deref()
                .unwrap_or("unknown legacy session");
            let origin_is_live = r
                .origin_session_id
                .as_ref()
                .is_some_and(|session| live_sessions.contains(session));
            let scope_desc = if r.cross_session {
                format!(
                    " [cross-session; origin {}: {}; created: {}]",
                    if origin_is_live { "live" } else { "orphaned" },
                    origin,
                    r.created_at.to_rfc3339()
                )
            } else if origin_is_live {
                format!(" [live-session: {origin}]")
            } else {
                format!(" [orphaned session: {origin}]")
            };

            lines.push(format!(
                "  #{}: [{}] {}{}{}",
                r.id, trigger_desc, r.message, target_desc, scope_desc
            ));
        }

        Ok(Self::success(lines.join("\n")))
    }

    /// Cancel a pending reminder
    pub(super) async fn factory_remind_cancel(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_reminder_store;

        let remind_id = req.remind_id.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "remind_id is required for remind_cancel action",
            )
        })?;

        let store = open_reminder_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open reminder store: {e}"),
            )
        })?;

        let owner_id = self.inner.get_agent_id().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to get agent ID: {e}"),
            )
        })?;

        store.cancel(remind_id, &owner_id).map_err(|e| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("Failed to cancel reminder: {e}"),
            )
        })?;

        Ok(Self::success(format!("Reminder #{remind_id} cancelled.")))
    }
}
