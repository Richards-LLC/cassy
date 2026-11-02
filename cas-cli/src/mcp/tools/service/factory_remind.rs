use crate::mcp::tools::service::imports::*;

const EXTERNAL_BRANCH_CONTAINED_EVENT: &str = "branch_contained_in";
const EXTERNAL_TAG_EXISTS_EVENT: &str = "tag_exists";
use crate::ui::factory::daemon::runtime::ci_watch::{
    EXTERNAL_CI_COMPLETED_EVENT, external_filter_string,
};

/// GH #1208: default lifetime of an external (`branch_contained_in`,
/// `tag_exists`, `ci_completed`) reminder when `remind_ttl_secs` is omitted.
/// Long enough for a merge queue or release; an explicit `0` still opts into
/// no expiry. Before this, the default `0` left dead sessions' wakes probing
/// origin forever.
pub(crate) const EXTERNAL_REMINDER_DEFAULT_TTL_SECS: i64 = 7 * 24 * 60 * 60;

/// Default `remind_ttl_secs` for an event (or time) reminder.
fn default_reminder_ttl_secs(remind_event: Option<&str>, filter: Option<&str>) -> i64 {
    let task_bound = matches!(remind_event, Some("task_completed" | "task_blocked"))
        && filter.and_then(|filter| serde_json::from_str::<serde_json::Value>(filter).ok())
            .and_then(|filter| filter.get("task_id").and_then(|id| id.as_str()).map(str::trim).map(str::to_owned))
            .is_some_and(|id| !id.is_empty());
    if task_bound {
        // A task wait ends with the task, rather than silently timing out
        // during a long peer lane. Explicit TTLs remain real deadlines.
        0
    } else if remind_event.is_some_and(is_external_reminder_event) {
        EXTERNAL_REMINDER_DEFAULT_TTL_SECS
    } else {
        3600
    }
}

/// GH #1208: store a `branch_contained_in` target as the bare origin branch,
/// so `origin/staging` cannot become a reminder that never fires.
fn normalize_external_filter(event_type: &str, filter: &mut serde_json::Value) {
    if event_type != EXTERNAL_BRANCH_CONTAINED_EVENT {
        return;
    }
    let Some(object) = filter.as_object_mut() else {
        return;
    };
    for key in ["target_branch", "target"] {
        if let Some(serde_json::Value::String(value)) = object.get_mut(key) {
            *value = crate::ui::factory::daemon::runtime::ci_watch::normalize_external_target_branch(value)
                .to_string();
        }
    }
}

fn is_external_reminder_event(event_type: &str) -> bool {
    matches!(
        event_type,
        EXTERNAL_BRANCH_CONTAINED_EVENT | EXTERNAL_TAG_EXISTS_EVENT | EXTERNAL_CI_COMPLETED_EVENT
    )
}

/// Recognizable status polls have event/completion signals. Keep timers for
/// actual deadlines and retries of work (for example build admission).
fn is_delay_poll(message: &str) -> bool {
    let text = message.to_ascii_lowercase();
    let words: Vec<_> = text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty()).collect();
    if words.iter().any(|word| matches!(*word, "inbox_poll" | "poll" | "polling" | "recheck")) {
        return true;
    }
    if words.contains(&"retry") && words.iter().any(|word| matches!(*word, "admission" | "capacity")) {
        return false;
    }
    words.iter().any(|word| matches!(*word,
            "check" | "inspect" | "watch" | "wait" | "waiting"))
        && words.iter().any(|word| matches!(*word,
            "ci" | "inbox" | "messages" | "merge" | "merged" | "landing"
            | "task" | "tasks" | "build" | "job" | "release" | "queue" | "completion"))
}

/// GH #1207: the `remind_filter` contract of one external event. `required`
/// names each field with the JSON keys that supply it, preferred key first.
struct ExternalFilterSpec {
    required: &'static [(&'static str, &'static [&'static str])],
    optional: &'static [&'static str],
    example: &'static str,
}

fn external_filter_spec(event_type: &str) -> Option<ExternalFilterSpec> {
    match event_type {
        EXTERNAL_BRANCH_CONTAINED_EVENT => Some(ExternalFilterSpec {
            required: &[
                ("commit", &["commit", "branch_tip", "branch", "sha"]),
                ("target_branch", &["target_branch", "target"]),
            ],
            optional: &[],
            example: r#"{"commit":"<sha-or-ref>","target_branch":"main"}"#,
        }),
        EXTERNAL_TAG_EXISTS_EVENT => Some(ExternalFilterSpec {
            required: &[("tag", &["tag"])],
            optional: &[],
            example: r#"{"tag":"<tag>"}"#,
        }),
        EXTERNAL_CI_COMPLETED_EVENT => Some(ExternalFilterSpec {
            required: &[("sha", &["sha"])],
            optional: &["workflow"],
            example: r#"{"sha":"<full 40-character sha>","workflow":"CI"}"#,
        }),
        _ => None,
    }
}

/// Validate an external reminder and report every problem in one refusal
/// (GH #1207). Callers used to learn the contract one rejection at a time:
/// first cross_session, then a missing key, while an unknown key such as
/// `branch` on `ci_completed` was silently ignored.
fn validate_external_reminder(
    event_type: &str,
    filter: Option<&serde_json::Value>,
    cross_session: bool,
    ttl_secs: i64,
) -> std::result::Result<(), String> {
    let Some(spec) = external_filter_spec(event_type) else {
        return Ok(());
    };
    let mut problems = Vec::new();
    if !cross_session {
        problems.push(
            "cross_session=true is required, so a daemon or session restart cannot lose the wake"
                .to_string(),
        );
    }
    if ttl_secs < 0 {
        problems.push("remind_ttl_secs must be zero (no expiry) or positive".to_string());
    }
    let describe = |keys: &[&str]| match keys {
        [only] => format!("`{only}`"),
        [first, aliases @ ..] => format!(
            "`{first}` (or {})",
            aliases.iter().map(|alias| format!("`{alias}`")).collect::<Vec<_>>().join("/")
        ),
        [] => String::new(),
    };
    match filter.and_then(serde_json::Value::as_object) {
        None => problems.push(format!(
            "remind_filter must be a JSON object with {}",
            spec.required
                .iter()
                .map(|(_, keys)| describe(keys))
                .collect::<Vec<_>>()
                .join(" and ")
        )),
        Some(object) => {
            let accepted: Vec<&str> = spec
                .required
                .iter()
                .flat_map(|(_, keys)| keys.iter().copied())
                .chain(spec.optional.iter().copied())
                .collect();
            let unknown: Vec<String> = object
                .keys()
                .filter(|key| !accepted.contains(&key.as_str()))
                .map(|key| format!("`{key}`"))
                .collect();
            if !unknown.is_empty() {
                problems.push(format!(
                    "unknown remind_filter key(s) {}; {event_type} accepts {}",
                    unknown.join(", "),
                    accepted.iter().map(|key| format!("`{key}`")).collect::<Vec<_>>().join(", ")
                ));
            }
            let filter = filter.expect("object implies a filter");
            for (_, keys) in spec.required {
                if external_filter_string(filter, keys).is_none() {
                    problems.push(format!(
                        "remind_filter requires string field {}",
                        describe(keys)
                    ));
                }
            }
            problems.extend(external_filter_value_problems(event_type, filter));
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{event_type} reminder refused ({} problem{}):\n{}\nExample: cross_session=true remind_filter='{}'",
        problems.len(),
        if problems.len() == 1 { "" } else { "s" },
        problems
            .iter()
            .map(|problem| format!("- {problem}"))
            .collect::<Vec<_>>()
            .join("\n"),
        spec.example
    ))
}

/// Value checks for the fields that are present; missing fields are reported
/// by the caller.
fn external_filter_value_problems(event_type: &str, filter: &serde_json::Value) -> Vec<String> {
    let mut problems = Vec::new();
    match event_type {
        EXTERNAL_BRANCH_CONTAINED_EVENT => {
            let commit = external_filter_string(filter, &["commit", "branch_tip", "branch", "sha"]);
            if let Some(target) = external_filter_string(filter, &["target_branch", "target"]) {
                let target =
                    crate::ui::factory::daemon::runtime::ci_watch::normalize_external_target_branch(
                        &target,
                    );
                if target.is_empty() {
                    problems.push(
                        "target_branch must name a branch on origin, e.g. \"main\"".to_string(),
                    );
                } else if target.starts_with('-') {
                    problems.push("target_branch may not start with '-'".to_string());
                }
            }
            if commit.is_some_and(|value| value.starts_with('-')) {
                problems.push("commit may not start with '-'".to_string());
            }
        }
        EXTERNAL_TAG_EXISTS_EVENT => {
            if external_filter_string(filter, &["tag"])
                .is_some_and(|value| value.starts_with('-') || value.ends_with('/'))
            {
                problems.push("tag must be a valid git ref name".to_string());
            }
        }
        EXTERNAL_CI_COMPLETED_EVENT => {
            if external_filter_string(filter, &["sha"]).is_some()
                && crate::ui::factory::daemon::runtime::ci_watch::parse_ci_completed(filter)
                    .is_err()
            {
                problems.push("`sha` must be the full 40-character commit SHA".to_string());
            }
        }
        _ => {}
    }
    problems
}

impl CasService {
    /// Resolve the `target` parameter to an agent ID.
    /// If `target` is a name (e.g. "swift-fox"), look it up in the agent store.
    /// If `target` is already a UUID, return it directly.
    /// Returns `(agent_id, display_name)`.
    fn resolve_reminder_target(
        &self,
        target: &str,
        owner_id: &str,
    ) -> std::result::Result<(String, String), McpError> {
        use crate::mcp::tools::service::agent_search_system::message::
            resolve_worker_supervisor_target;
        use crate::store::open_agent_store;
        use cas_types::AgentStatus;

        let store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;

        if target.eq_ignore_ascii_case("supervisor")
            && let Ok(caller) = store.get(owner_id)
            && caller.role == cas_types::AgentRole::Worker
        {
            // Match message's authenticated worker resolution: the caller's
            // registered factory session wins, with the environment retained
            // only as the same compatibility fallback used by message_send.
            let factory_session = caller
                .factory_session
                .as_deref()
                .filter(|session| !session.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    std::env::var("CAS_FACTORY_SESSION")
                        .ok()
                        .filter(|session| !session.trim().is_empty())
                });
            let supervisor = resolve_worker_supervisor_target(
                store.as_ref(),
                factory_session.as_deref(),
            )
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Cannot resolve 'supervisor' for factory session {}: no live supervisor belongs to this worker's session",
                        factory_session.as_deref().unwrap_or("none")
                    ),
                )
            })?;
            return Ok((supervisor.agent_id, supervisor.name));
        }

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

        if has_delay && is_delay_poll(&message) {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Delay reminders cannot poll inbox, CI, task, merge or job status. \
                 Use remind_event=ci_completed with remind_filter={\"sha\":\"<40-hex-sha>\"}, \
                 remind_event=task_completed with remind_filter={\"task_id\":\"<task-id>\"}, \
                 or remind_event=branch_contained_in with \
                 remind_filter={\"commit\":\"<sha>\",\"target_branch\":\"main\"}. \
                 External events require cross_session=true. For detached commands, \
                 use the harness background-task completion notice. Operator messages \
                 already wake the supervisor; report a missing wake instead of polling.",
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
            let (id, name) = self.resolve_reminder_target(target_name, &owner_id)?;
            (Some(id), Some(name))
        } else {
            (None, None)
        };

        let ttl_secs = req
            .remind_ttl_secs
            .unwrap_or_else(|| default_reminder_ttl_secs(req.remind_event.as_deref(), req.remind_filter.as_deref()));
        if ttl_secs < 0 {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "remind_ttl_secs must be zero (no expiry) or positive",
            ));
        }
        let cross_session = req.cross_session.unwrap_or(false);
        let explicit_task_id = req
            .task_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty());

        let task_id = if let Some(task_id) = explicit_task_id {
            let task_store = self.inner.open_task_store().map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Cannot open task store for reminder context: {e}"),
                )
            })?;
            task_store.get(task_id).map_err(|e| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!("Cannot bind reminder to task {task_id}: {e}"),
                )
            })?;
            Some(task_id.to_string())
        } else {
            // GH #276: callers historically had no task_id parameter. Bind a
            // reminder to the issuer's one unambiguous active task so closing
            // that task quarantines stale follow-up context by default.
            let task_store = self.inner.open_task_store().map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Cannot open task store for reminder context: {e}"),
                )
            })?;
            let agent_name = crate::store::open_agent_store(&self.inner.cas_root)
                .ok()
                .and_then(|store| store.get(&owner_id).ok())
                .map(|agent| agent.name);
            let active: Vec<_> = task_store
                .list(None)
                .map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Cannot list tasks for reminder context: {e}"),
                    )
                })?
                .into_iter()
                .filter(|task| {
                    task.status == cas_types::TaskStatus::InProgress
                        && task.assignee.as_deref().is_some_and(|assignee| {
                            assignee.eq_ignore_ascii_case(&owner_id)
                                || agent_name
                                    .as_deref()
                                    .is_some_and(|name| assignee.eq_ignore_ascii_case(name))
                        })
                })
                .collect();
            (active.len() == 1).then(|| active[0].id.clone())
        };

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
                    task_id.as_deref(),
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
                format!(
                    ", explicit keep across session/task close; origin session: {origin_session_id}"
                )
            } else {
                ", session-scoped; cancelled when this session ends".to_string()
            };
            let task_desc = task_id
                .as_deref()
                .map(|id| format!(", task: {id}"))
                .unwrap_or_default();
            Ok(Self::success(format!(
                "Reminder #{id} set (time-based, fires in {time_desc}{target_desc}{task_desc}{scope_desc})\nMessage: {message}"
            )))
        } else {
            let event_type = req.remind_event.unwrap();

            // Director events plus persisted split decisions and external conditions.
            let valid_events = [
                "task_assigned",
                "task_completed",
                "task_blocked",
                "worker_idle",
                "agent_registered",
                "epic_started",
                "epic_completed",
                "epic_split_decided",
                EXTERNAL_BRANCH_CONTAINED_EVENT,
                EXTERNAL_TAG_EXISTS_EVENT,
                EXTERNAL_CI_COMPLETED_EVENT,
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

            if let Err(error) = validate_external_reminder(
                &event_type,
                filter.as_ref(),
                cross_session,
                ttl_secs,
            ) {
                return Err(Self::error(ErrorCode::INVALID_PARAMS, error));
            }
            let mut filter = filter;
            if event_type == "epic_split_decided"
                && filter.as_ref().and_then(|value| value.get("split_id"))
                    .and_then(serde_json::Value::as_str).is_none_or(|id| id.trim().is_empty())
            {
                return Err(Self::error(ErrorCode::INVALID_PARAMS,
                    "epic_split_decided requires remind_filter={\"split_id\":\"<split>\"}"));
            }
            if let Some(filter) = filter.as_mut() {
                normalize_external_filter(&event_type, filter);
            }

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
                    task_id.as_deref(),
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
                    ", factory session: {s}, explicit keep across session/task close; origin session: {origin_session_id}"
                ),
                (Some(s), false) => format!(
                    ", factory session: {s}, session-scoped; cancelled when this session ends"
                ),
                (None, true) => {
                    format!(
                        ", explicit keep across session/task close; origin session: {origin_session_id}"
                    )
                }
                (None, false) => ", session-scoped; cancelled when this session ends".to_string(),
            };
            let task_desc = task_id
                .as_deref()
                .map(|id| format!(", task: {id}"))
                .unwrap_or_default();

            Ok(Self::success(format!(
                "Reminder #{id} set (event-based, fires on {event_type}{filter_desc}{target_desc}{task_desc}{session_desc})\nMessage: {message}"
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

        // cas-c5ab: a held worker's suspended reminders still wait.
        let held = store.list_suspended(Some(&my_id)).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list held reminders: {e}"),
            )
        })?;

        // Merge, dedup by ID (a self-reminder appears in both)
        let mut seen = std::collections::HashSet::new();
        let mut reminders = Vec::new();
        for r in owned.into_iter().chain(targeting_me).chain(held) {
            if seen.insert(r.id) {
                reminders.push(r);
            }
        }

        // cas-57c1 (GH #984): reminders that will no longer fire are shown too,
        // so an expired or cancelled wake is visible instead of silently gone.
        let ended = store
            .list_recently_ended(&my_id, RECENTLY_ENDED_WINDOW_SECS)
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to list expired or cancelled reminders: {e}"),
                )
            })?;

        if reminders.is_empty() && ended.is_empty() {
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

        let mut lines = vec![if reminders.is_empty() {
            "No pending reminders.".to_string()
        } else {
            format!("Pending reminders ({}):", reminders.len())
        }];

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

            let held_desc = if r.status == cas_store::ReminderStatus::Suspended {
                " [held: re-armed when the worker is released]"
            } else {
                ""
            };
            lines.push(format!(
                "  #{}: [{}] {}{}{}{}",
                r.id, trigger_desc, r.message, target_desc, scope_desc, held_desc
            ));
        }

        if !ended.is_empty() {
            lines.push(format!(
                "Expired or cancelled in the last 24h ({}); these will not fire:",
                ended.len()
            ));
            for r in &ended {
                lines.push(format!(
                    "  #{}: [{}] {}",
                    r.id,
                    ended_reminder_desc(r),
                    r.message
                ));
            }
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

/// How far back `remind_list` shows reminders that expired or were cancelled.
const RECENTLY_ENDED_WINDOW_SECS: i64 = 24 * 60 * 60;

/// Why an ended reminder will not fire, in the operator's terms.
fn ended_reminder_desc(reminder: &cas_store::Reminder) -> String {
    use cas_store::{ReminderStatus, ReminderTriggerType};
    match reminder.status {
        ReminderStatus::Cancelled => match reminder.cancelled_at {
            Some(at) => format!("cancelled {}", at.to_rfc3339()),
            None => "cancelled".to_string(),
        },
        ReminderStatus::Expired => match (reminder.trigger_type, reminder.trigger_at) {
            (ReminderTriggerType::Time, Some(due)) => format!(
                "expired: due {} but not delivered within its {}s TTL",
                due.to_rfc3339(),
                reminder.ttl_secs
            ),
            _ => format!(
                "expired: {}s TTL from creation {} ran out",
                reminder.ttl_secs,
                reminder.created_at.to_rfc3339()
            ),
        },
        ReminderStatus::Pending | ReminderStatus::Fired | ReminderStatus::Suspended => {
            reminder.status.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::validate_external_reminder;

    #[tokio::test(flavor = "current_thread")]
    async fn worker_remind_supervisor_target_uses_owning_factory_session() {
        use crate::test_support::TestEnvGuard;
        use cas_types::{Agent, AgentRole};

        let env = TestEnvGuard::temp_home();
        let sessions = env.home().join(".cas").join("sessions");
        std::fs::create_dir_all(&sessions).expect("sessions directory");
        std::fs::write(
            sessions.join("factory-a.json"),
            r#"{"name":"factory-a","supervisor":{"name":"supervisor-a"},"workers":[]}"#,
        )
        .expect("session metadata");

        let temp = tempfile::tempdir().expect("temporary Cassy root");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("Cassy root");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let agents = core.open_agent_store().expect("agent store");

        let mut worker = Agent::new("worker-id".into(), "worker-a".into());
        worker.role = AgentRole::Worker;
        worker.factory_session = Some("factory-a".into());
        agents.register(&worker).expect("register worker");

        let mut supervisor_a = Agent::new("supervisor-a-id".into(), "supervisor-a".into());
        supervisor_a.role = AgentRole::Supervisor;
        agents
            .register(&supervisor_a)
            .expect("register sessionless owning supervisor");

        let mut supervisor_b = Agent::new("supervisor-b-id".into(), "supervisor-b".into());
        supervisor_b.role = AgentRole::Supervisor;
        supervisor_b.factory_session = Some("factory-b".into());
        agents
            .register(&supervisor_b)
            .expect("register sibling supervisor");

        core.set_agent_id_for_testing(worker.id.clone());
        #[cfg(feature = "mcp-proxy")]
        let service = crate::mcp::tools::service::CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = crate::mcp::tools::service::CasService::new(core);
        let request: crate::mcp::tools::service::FactoryRequest =
            serde_json::from_value(serde_json::json!({
                "action": "remind",
                "target": "supervisor",
                "remind_message": "factory checkpoint",
                "remind_delay_secs": 60
            }))
            .expect("reminder request");

        service
            .factory_remind(request)
            .await
            .expect("worker's supervisor alias resolves");

        let reminders = crate::store::open_reminder_store(&cas_root).expect("reminder store");
        let targeted = reminders
            .list_pending_for_target(&supervisor_a.id)
            .expect("reminders for owning supervisor");
        assert_eq!(targeted.len(), 1);
        assert_eq!(targeted[0].owner_id, worker.id);
        assert!(
            reminders
                .list_pending_for_target(&supervisor_b.id)
                .expect("reminders for sibling supervisor")
                .is_empty(),
            "must not target a supervisor from another factory session"
        );

        let explicit_request: crate::mcp::tools::service::FactoryRequest =
            serde_json::from_value(serde_json::json!({
                "action": "remind",
                "target": supervisor_b.id.clone(),
                "remind_message": "explicit recipient",
                "remind_delay_secs": 60
            }))
            .expect("explicit target reminder request");
        service
            .factory_remind(explicit_request)
            .await
            .expect("registered agent IDs remain valid targets");
        assert_eq!(
            reminders
                .list_pending_for_target(&supervisor_b.id)
                .expect("explicit registered-ID reminder")
                .len(),
            1
        );

        let self_request: crate::mcp::tools::service::FactoryRequest =
            serde_json::from_value(serde_json::json!({
                "action": "remind",
                "remind_message": "Retry capped cargo check after build admission",
                "remind_delay_secs": 60
            }))
            .expect("self reminder request");
        service
            .factory_remind(self_request)
            .await
            .expect("omitted target remains a self-reminder");
        let self_reminders = reminders
            .list_pending_for_target(&worker.id)
            .expect("self-targeted reminder");
        assert_eq!(self_reminders.len(), 1);
        assert_eq!(self_reminders[0].target_id, worker.id);

        for message in [
            "Poll inbox_poll for operator messages in three minutes",
            "Check CI on the pushed SHA",
            "Recheck whether the delivery has merged",
            "Poll detached build log for completion",
        ] {
            let request = serde_json::from_value(serde_json::json!({
                "action": "remind", "remind_message": message, "remind_delay_secs": 180
            })).unwrap();
            let error = service.factory_remind(request).await
                .expect_err("delay-based polling must not create a wake");
            let error = error.to_string();
            for pointer in ["ci_completed", "task", "branch_contained_in", "background-task"] {
                assert!(error.contains(pointer), "missing {pointer}: {error}");
            }
        }
        assert_eq!(reminders.list_pending_for_target(&worker.id).unwrap().len(), 1,
            "refused polls leave no pending timer");
        let event_request = serde_json::from_value(serde_json::json!({
            "action": "remind", "remind_message": "Check CI on the pushed SHA",
            "remind_event": "ci_completed", "cross_session": true,
            "remind_filter": serde_json::json!({"sha": "a".repeat(40)}).to_string()
        })).unwrap();
        service.factory_remind(event_request).await.expect("the suggested event form is accepted");
        let pending = reminders.list_pending_for_target(&worker.id).unwrap();
        assert_eq!(pending.len(), 2);
        assert!(pending.iter().any(|row| row.trigger_type == cas_store::ReminderTriggerType::Event));


    }

    #[test]
    fn cas_3349_validated_aliases_are_parseable() {
        use crate::ui::factory::daemon::runtime::ci_watch::{
            ExternalWakeCondition, parse_external_wake_condition,
        };

        // A malformed preferred key must not hide a usable legacy alias.
        for invalid in [serde_json::json!(""), serde_json::json!(" \t"),
            serde_json::json!(null), serde_json::json!(17), serde_json::json!("two refs")]
        {
            for commit_key in ["branch_tip", "branch", "sha"] {
                let filter = serde_json::json!({
                    "commit": invalid,
                    commit_key: "abc123",
                    "target_branch": invalid,
                    "target": "origin/main",
                });
                validate_external_reminder("branch_contained_in", Some(&filter), true, 0)
                    .expect("valid aliases are accepted at creation");
                assert_eq!(
                    parse_external_wake_condition("branch_contained_in", &filter),
                    Ok(ExternalWakeCondition::BranchContained {
                        commit: "abc123".into(),
                        target_branch: "main".into(),
                    }),
                    "accepted reminder must be executable: {filter}"
                );
            }
            let filter = serde_json::json!({"commit": invalid, "target_branch": "main"});
            assert!(validate_external_reminder("branch_contained_in", Some(&filter), true, 0).is_err());
            assert!(parse_external_wake_condition("branch_contained_in", &filter).is_err());
        }
        let filter = serde_json::json!({"commit": "first", "branch_tip": "second", "target_branch": "main", "target": "staging"});
        assert_eq!(
            parse_external_wake_condition("branch_contained_in", &filter),
            Ok(ExternalWakeCondition::BranchContained { commit: "first".into(), target_branch: "main".into() })
        );
    }

    #[test]
    fn ended_reminders_say_why_they_will_not_fire() {
        use cas_store::{Reminder, ReminderStatus, ReminderTriggerType};
        let due = chrono::Utc::now();
        let mut reminder = Reminder {
            id: 7,
            owner_id: "sup".into(),
            target_id: "sup".into(),
            message: "overnight checkpoint".into(),
            trigger_type: ReminderTriggerType::Time,
            trigger_at: Some(due),
            trigger_event: None,
            trigger_filter: None,
            status: ReminderStatus::Expired,
            ttl_secs: 3600,
            created_at: due,
            fired_at: None,
            cancelled_at: None,
            fired_event: None,
            session_id: None,
            origin_session_id: None,
            cross_session: false,
            task_id: None,
        };
        let desc = super::ended_reminder_desc(&reminder);
        assert!(
            desc.starts_with("expired: due ") && desc.ends_with("within its 3600s TTL"),
            "{desc}"
        );
        reminder.status = ReminderStatus::Cancelled;
        reminder.cancelled_at = Some(due);
        assert!(super::ended_reminder_desc(&reminder).starts_with("cancelled "));
    }

    #[test]
    fn external_reminder_validation_requires_restart_safe_scope_and_filter() {
        let filter = serde_json::json!({
            "commit": "abc123",
            "target_branch": "main"
        });
        assert!(
            validate_external_reminder("branch_contained_in", Some(&filter), true, 0).is_ok()
        );
        assert!(
            validate_external_reminder("branch_contained_in", Some(&filter), false, 0).is_err()
        );
        assert!(validate_external_reminder("branch_contained_in", None, true, 0).is_err());
        // GH #1207: `sha` names the commit for branch_contained_in too.
        let sha = serde_json::json!({"sha": "abc123", "target_branch": "main"});
        assert!(validate_external_reminder("branch_contained_in", Some(&sha), true, 0).is_ok());
        // Director events keep free-form subset filters.
        let task = serde_json::json!({"task_id": "cas-a1b2", "anything": 1});
        assert!(validate_external_reminder("task_completed", Some(&task), false, 0).is_ok());
    }

    #[test]
    fn external_reminder_validation_accepts_tag_and_rejects_invalid_refs() {
        let tag = serde_json::json!({"tag": "v9.99.0"});
        assert!(validate_external_reminder("tag_exists", Some(&tag), true, 0).is_ok());
        assert!(validate_external_reminder(
            "tag_exists",
            Some(&serde_json::json!({"tag": "-bad"})),
            true,
            0
        )
        .is_err());
        assert!(validate_external_reminder("tag_exists", Some(&tag), true, -1).is_err());
    }

    /// GH #1207 reproduction: `ci_completed` with `{"run_id":…,"branch":…}`
    /// and no cross_session was refused for cross_session alone; the retry
    /// was refused for the missing `sha`, and the unknown keys were never
    /// named. One refusal now lists all of it, with a working example.
    #[test]
    fn one_refusal_lists_every_missing_requirement() {
        let filter = serde_json::json!({"run_id": 18234, "branch": "main"});
        let error =
            validate_external_reminder("ci_completed", Some(&filter), false, 0).unwrap_err();
        assert!(error.starts_with("ci_completed reminder refused (3 problems):"), "{error}");
        assert!(error.contains("cross_session=true is required"), "{error}");
        assert!(error.contains("unknown remind_filter key(s) `branch`, `run_id`"), "{error}");
        assert!(error.contains("accepts `sha`, `workflow`"), "{error}");
        assert!(error.contains("requires string field `sha`"), "{error}");
        assert!(error.contains(r#"Example: cross_session=true remind_filter='{"sha":"#), "{error}");

        // branch_contained_in names both missing fields and their aliases.
        let error = validate_external_reminder(
            "branch_contained_in",
            Some(&serde_json::json!({})),
            true,
            0,
        )
        .unwrap_err();
        assert!(error.contains("(2 problems)"), "{error}");
        assert!(
            error.contains("`commit` (or `branch_tip`/`branch`/`sha`)")
                && error.contains("`target_branch` (or `target`)"),
            "{error}"
        );

        // A malformed value is reported with the other problems.
        let error = validate_external_reminder(
            "ci_completed",
            Some(&serde_json::json!({"sha": "abc"})),
            false,
            0,
        )
        .unwrap_err();
        assert!(error.contains("(2 problems)"), "{error}");
        assert!(error.contains("`sha` must be the full 40-character commit SHA"), "{error}");
    }
}
