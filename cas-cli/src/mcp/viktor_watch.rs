//! Viktor's inbound half: record run-starting proxy calls and poll them from
//! the embedded daemon into the ordinary CAS prompt queue.

use std::path::{Path, PathBuf};

use cas_store::{
    EnqueueIdempotentResult, PromptQueueStore, SqlitePromptQueueStore, SqliteViktorInboundStore,
    SqliteViktorWatchStore, ViktorInboundMessage, ViktorThreadWatch,
};
use cas_types::{Agent, AgentRole, AgentStatus};
use serde_json::{Map, Value};

pub(crate) const VIKTOR_WATCH_POLL_INTERVAL_SECS: i64 = 30;
pub(crate) const VIKTOR_WATCH_MAX_PER_TICK: usize = 16;
pub(crate) const VIKTOR_WATCH_MAX_CALLS_PER_TICK: usize = VIKTOR_WATCH_MAX_PER_TICK * 2;
pub(crate) const VIKTOR_WATCH_TICK_BUDGET_SECS: u64 = 20;
pub(crate) const VIKTOR_INBOUND_DISCOVERY_SCAN_THREADS: usize = 32;
pub(crate) const VIKTOR_INBOUND_DISCOVERY_MAX_THREADS: usize = 4;
pub(crate) const VIKTOR_INBOUND_DISCOVERY_MAX_CALLS: usize =
    VIKTOR_INBOUND_DISCOVERY_MAX_THREADS + 1;
pub(crate) const VIKTOR_INBOUND_DISCOVERY_BUDGET_SECS: u64 = 4;
const VIKTOR_INBOUND_MESSAGES_PER_THREAD: usize = 8;
const VIKTOR_INBOUND_SESSION_START_LIMIT: usize = 1;
const VIKTOR_INBOUND_SESSION_START_BODY_CHARS: usize = 2_000;

const START_TOOLS: &[&str] = &["ask_viktor", "create_thread", "send_message"];
const TERMINAL_RUN_STATES: &[&str] = &[
    "completed",
    "requires_action",
    "failed",
    "cancelled",
    "timed_out",
];

pub(crate) struct ViktorWatchRecorder {
    cas_root: PathBuf,
}

/// Surface a restart that left the managed Viktor upstream disconnected before
/// a pending watch silently retries forever. The queue receipt is keyed by the
/// exact durable watch set, so another daemon restart cannot spam a supervisor
/// while a newly-recorded run still produces a fresh alert.
pub(crate) async fn alert_unpollable_watches(
    cas_root: &Path,
    proxy: &cmcp_core::ProxyEngine,
) -> Result<usize, String> {
    if proxy.upstream_connected("viktor").await {
        return Ok(0);
    }
    let store = SqliteViktorWatchStore::open(cas_root).map_err(|error| error.to_string())?;
    let watches = store.list_live().map_err(|error| error.to_string())?;
    if watches.is_empty() {
        return Ok(0);
    }

    let agents = crate::store::open_agent_store(cas_root).map_err(|error| error.to_string())?;
    let live_agents = agents.list(None).map_err(|error| error.to_string())?;
    let mut delivered = 0;
    for supervisor in live_agents.into_iter().filter(|agent| {
        matches!(agent.role, AgentRole::Supervisor | AgentRole::Director)
            && matches!(agent.status, AgentStatus::Active | AgentStatus::Idle)
    }) {
        let run_ids: Vec<&str> = watches
            .iter()
            .filter(|watch| {
                watch.factory_session.is_none()
                    || watch.factory_session == supervisor.factory_session
            })
            .map(|watch| watch.run_id.as_str())
            .collect();
        if run_ids.is_empty() {
            continue;
        }
        let run_ids_text = run_ids.join(", ");
        let prompt = format!(
            "<viktor-upstream-absent runs=\"{}\">\nViktor upstream is absent after daemon startup. {} watched run(s) cannot be polled until VIKTOR_API_KEY is available to cas serve and the daemon reconnects. Run IDs: {}.\n</viktor-upstream-absent>",
            run_ids_text,
            run_ids.len(),
            run_ids_text,
        );
        let queue = SqlitePromptQueueStore::open(cas_root).map_err(|error| error.to_string())?;
        queue.init().map_err(|error| error.to_string())?;
        let receipt = format!(
            "viktor-upstream-absent:{}:{}",
            supervisor.id,
            run_ids.join(",")
        );
        queue
            .enqueue_idempotent(
                "viktor",
                &supervisor.name,
                &prompt,
                supervisor.factory_session.as_deref(),
                Some(&format!(
                    "Viktor upstream absent — {} watched run(s) unpollable",
                    run_ids.len()
                )),
                Some(cas_store::NotificationPriority::High),
                &receipt,
                Some(&cas_store::QueueOrigin::Daemon),
            )
            .map_err(|error| error.to_string())?;
        delivered += 1;
    }
    Ok(delivered)
}

/// A bounded session-start warning derived from the same durable records that
/// drive daemon polling. This remains visible even when no supervisor was live
/// at daemon startup to receive the queue notification.
pub(crate) fn surface_inbound_at_session_start(
    cas_root: &Path,
    factory_session: Option<&str>,
) -> Option<String> {
    let store = SqliteViktorInboundStore::open(cas_root).ok()?;
    let messages = store
        .surface_pending(factory_session, VIKTOR_INBOUND_SESSION_START_LIMIT)
        .ok()?;
    if messages.is_empty() {
        return None;
    }
    let mut rendered = String::from(
        "⚠ Viktor-originated message arrived while no live Cassy supervisor could receive it. It is surfaced once here; answer on its existing thread through Viktor `send_message`.",
    );
    for message in messages {
        let body = message
            .body
            .chars()
            .take(VIKTOR_INBOUND_SESSION_START_BODY_CHARS)
            .collect::<String>();
        let truncated = (message.body.chars().count() > VIKTOR_INBOUND_SESSION_START_BODY_CHARS)
            .then_some("\n[message truncated to the SessionStart safety bound]")
            .unwrap_or_default();
        rendered.push_str(&format!(
            "\n\n<viktor-inbound thread_id=\"{}\" message_id=\"{}\">\n{}{}\n</viktor-inbound>",
            message.thread_id, message.message_id, body, truncated
        ));
    }
    Some(rendered)
}

pub(crate) fn session_start_warning(cas_root: &Path) -> Option<String> {
    let health = crate::mcp::read_proxy_health_cache(cas_root).ok()?;
    let snapshot: cmcp_core::ProxyHealthSnapshot = serde_json::from_slice(&health).ok()?;
    let absent = snapshot
        .servers
        .iter()
        .find(|server| server.name.eq_ignore_ascii_case("viktor"))
        .filter(|server| server.state != cmcp_core::UpstreamState::Healthy)?;
    let watches = SqliteViktorWatchStore::open(cas_root)
        .ok()
        .and_then(|store| store.list_live().ok())
        .unwrap_or_default();
    let run_ids = watches
        .iter()
        .take(8)
        .map(|watch| watch.run_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let watch_detail = if watches.is_empty() {
        "No watched runs are pending.".to_string()
    } else {
        format!(
            "{} watched run(s) are unpollable: {}{}.",
            watches.len(),
            run_ids,
            if watches.len() > 8 { ", …" } else { "" }
        )
    };
    Some(format!(
        "⚠ Viktor upstream absent ({:?}; {}). {} Restore VIKTOR_API_KEY to the cas serve process, then restart or wait for proxy reconnect; run `cas viktor` for durable watch status.",
        absent.state,
        absent.last_error_code.as_deref().unwrap_or("unknown"),
        watch_detail
    ))
}

/// Discover provider-originated threads on the same cadence as run watches.
/// One tick costs at most one `list_threads` plus four `list_messages` calls,
/// and the whole discovery pass is cancelled after four seconds.
pub(crate) async fn discover_originated_messages(
    cas_root: &Path,
    proxy: &cmcp_core::ProxyEngine,
) -> Result<usize, String> {
    if !proxy.upstream_connected("viktor").await {
        return Ok(0);
    }
    tracing::debug!(
        max_threads = VIKTOR_INBOUND_DISCOVERY_MAX_THREADS,
        scan_threads = VIKTOR_INBOUND_DISCOVERY_SCAN_THREADS,
        max_calls = VIKTOR_INBOUND_DISCOVERY_MAX_CALLS,
        budget_secs = VIKTOR_INBOUND_DISCOVERY_BUDGET_SECS,
        "Viktor originated-thread discovery tick"
    );
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(VIKTOR_INBOUND_DISCOVERY_BUDGET_SECS);
    let caller = discovery_caller();
    let list_args = Map::from_iter([(
        "limit".to_string(),
        Value::from(VIKTOR_INBOUND_DISCOVERY_SCAN_THREADS as u64),
    )]);
    let threads = tokio::time::timeout_at(
        deadline,
        proxy.call_tool(&caller, "viktor", "list_threads", Some(list_args)),
    )
    .await
    .map_err(|_| "Viktor inbound discovery exhausted its 4s wall-clock budget".to_string())?
    .map_err(|error| bounded_error(&error.to_string()))?;

    let watch_store = SqliteViktorWatchStore::open(cas_root).map_err(|error| error.to_string())?;
    let inbound_store =
        SqliteViktorInboundStore::open(cas_root).map_err(|error| error.to_string())?;
    let mut thread_ids = provider_items(&threads, "threads")
        .into_iter()
        .filter_map(|thread| item_id(&thread, "thread_id"))
        .collect::<Vec<_>>();
    thread_ids.dedup();
    thread_ids.truncate(VIKTOR_INBOUND_DISCOVERY_SCAN_THREADS);
    let mut candidates = Vec::new();
    for thread_id in thread_ids {
        if watch_store
            .contains_thread(&thread_id)
            .map_err(|error| error.to_string())?
        {
            continue;
        }
        let known = inbound_store
            .contains_thread(&thread_id)
            .map_err(|error| error.to_string())?;
        candidates.push((known, thread_id));
    }
    // Drain newly observed threads before refreshing known inbound threads so
    // a busy unresolved conversation cannot permanently starve the next one.
    candidates.sort_by_key(|(known, _)| *known);

    for (_, thread_id) in candidates
        .into_iter()
        .take(VIKTOR_INBOUND_DISCOVERY_MAX_THREADS)
    {
        let args = Map::from_iter([
            ("thread_id".to_string(), Value::String(thread_id.clone())),
            (
                "limit".to_string(),
                Value::from(VIKTOR_INBOUND_MESSAGES_PER_THREAD as u64),
            ),
        ]);
        let messages = tokio::time::timeout_at(
            deadline,
            proxy.call_tool(&caller, "viktor", "list_messages", Some(args)),
        )
        .await
        .map_err(|_| "Viktor inbound discovery exhausted its 4s wall-clock budget".to_string())?
        .map_err(|error| bounded_error(&error.to_string()))?;
        for message in provider_items(&messages, "messages") {
            let Some(message_id) = item_id(&message, "message_id") else {
                tracing::error!(thread_id, "Viktor inbound message had no stable message id");
                continue;
            };
            let body = render_message_body(&message);
            if body.trim().is_empty() {
                tracing::error!(
                    thread_id,
                    message_id,
                    "Viktor inbound message had no content"
                );
                continue;
            }
            inbound_store
                .record(&thread_id, &message_id, &body)
                .map_err(|error| error.to_string())?;
        }
    }

    deliver_pending_inbound(cas_root, &inbound_store)
}

fn discovery_caller() -> cmcp_core::ProxyCaller {
    cmcp_core::ProxyCaller {
        agent_id: "cas-daemon-viktor-inbound".to_string(),
        role: AgentRole::Supervisor,
        session_id: "cas-daemon-viktor-inbound".to_string(),
        factory_session: None,
        active_task_ids: Vec::new(),
    }
}

fn live_supervisor(cas_root: &Path) -> Option<Agent> {
    let store = crate::store::open_agent_store(cas_root).ok()?;
    store
        .list(None)
        .ok()?
        .into_iter()
        .filter(|agent| {
            matches!(agent.role, AgentRole::Supervisor | AgentRole::Director)
                && matches!(agent.status, AgentStatus::Active | AgentStatus::Idle)
                && agent.factory_session.is_some()
        })
        .max_by_key(|agent| agent.last_heartbeat)
}

fn deliver_pending_inbound(
    cas_root: &Path,
    store: &SqliteViktorInboundStore,
) -> Result<usize, String> {
    let pending = store
        .list_pending(VIKTOR_INBOUND_DISCOVERY_MAX_THREADS * VIKTOR_INBOUND_MESSAGES_PER_THREAD)
        .map_err(|error| error.to_string())?;
    if pending.is_empty() {
        return Ok(0);
    }
    let Some(supervisor) = live_supervisor(cas_root) else {
        for message in pending {
            store
                .mark_delivery_error(
                    &message.message_id,
                    "no live factory supervisor was registered at discovery time",
                )
                .map_err(|error| error.to_string())?;
        }
        return Ok(0);
    };
    let queue = SqlitePromptQueueStore::open(cas_root).map_err(|error| error.to_string())?;
    queue.init().map_err(|error| error.to_string())?;
    let mut delivered = 0;
    for message in pending {
        let prompt = render_originated_message(&message);
        let enqueued = queue
            .enqueue_idempotent(
                "viktor",
                &supervisor.name,
                &prompt,
                supervisor.factory_session.as_deref(),
                Some(&format!("Viktor question on {}", message.thread_id)),
                Some(cas_store::NotificationPriority::High),
                &format!("viktor-inbound:{}", message.message_id),
                Some(&cas_store::QueueOrigin::Daemon),
            )
            .map_err(|error| error.to_string())?;
        let notification_id = match enqueued {
            EnqueueIdempotentResult::Created(id) => {
                delivered += 1;
                id
            }
            EnqueueIdempotentResult::AlreadyExists(id) => id,
        };
        store
            .mark_delivered(
                &message.message_id,
                supervisor.factory_session.as_deref(),
                Some(notification_id),
            )
            .map_err(|error| error.to_string())?;
    }
    let _ = cas_factory::notify_daemon(cas_root);
    Ok(delivered)
}

fn render_originated_message(message: &ViktorInboundMessage) -> String {
    format!(
        "<viktor-inbound thread_id=\"{}\" message_id=\"{}\">\n{}\n\nReply on this thread through the existing Viktor `send_message` tool with `thread_id=\"{}\"`; do not start a replacement thread.\n</viktor-inbound>",
        message.thread_id, message.message_id, message.body, message.thread_id
    )
}

fn render_message_body(value: &Value) -> String {
    for key in ["content", "text", "message", "markdown"] {
        if let Some(text) = value
            .as_object()
            .and_then(|object| object.get(key))
            .and_then(Value::as_str)
        {
            return text.chars().take(16_000).collect();
        }
    }
    render_provider_payload(value)
}

impl ViktorWatchRecorder {
    pub(crate) fn new(cas_root: PathBuf) -> Self {
        Self { cas_root }
    }
}

impl cmcp_core::ProxyCallObserver for ViktorWatchRecorder {
    fn call_succeeded(&self, event: cmcp_core::ProxyCallEvent<'_>) {
        if !event.server.eq_ignore_ascii_case("viktor") || !START_TOOLS.contains(&event.tool) {
            return;
        }

        let Some(thread_id) = find_identifier(event.result, "thread_id", "thread") else {
            tracing::error!(
                server = event.server,
                tool = event.tool,
                "Viktor run-starting call succeeded without a thread id; inbound watch not recorded"
            );
            return;
        };
        let Some(run_id) = find_identifier(event.result, "run_id", "run") else {
            tracing::error!(
                server = event.server,
                tool = event.tool,
                thread_id,
                "Viktor run-starting call succeeded without a run id; inbound watch not recorded"
            );
            return;
        };
        let watermark = find_identifier(event.result, "message_id", "message").or_else(|| {
            event
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.get("after"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });

        let agent_name = crate::store::open_agent_store(&self.cas_root)
            .ok()
            .and_then(|store| store.get(&event.caller.agent_id).ok())
            .map(|agent| agent.name)
            .unwrap_or_else(|| event.caller.agent_id.clone());
        let task_id = event.caller.active_task_ids.first().map(String::as_str);

        match SqliteViktorWatchStore::open(&self.cas_root).and_then(|store| {
            store.record(
                &thread_id,
                &run_id,
                &event.caller.agent_id,
                &agent_name,
                &event.caller.role.to_string(),
                event.caller.factory_session.as_deref(),
                task_id,
                watermark.as_deref(),
                cas_store::DEFAULT_VIKTOR_WATCH_TTL_SECS,
            )
        }) {
            Ok(watch_id) => tracing::info!(
                watch_id,
                thread_id,
                run_id,
                agent_id = event.caller.agent_id,
                task_id,
                "recorded Viktor inbound watch"
            ),
            Err(error) => tracing::error!(
                thread_id,
                run_id,
                error = %error,
                "Viktor call succeeded but inbound watch persistence failed"
            ),
        }
    }
}

/// Poll one bounded batch. A non-terminal watch costs one `get_run`; a
/// terminal successful watch costs one additional `get_run_result`.
pub(crate) async fn poll_due_watches(
    cas_root: &Path,
    proxy: &cmcp_core::ProxyEngine,
) -> Result<usize, String> {
    let store = SqliteViktorWatchStore::open(cas_root).map_err(|error| error.to_string())?;
    store.expire_stale().map_err(|error| error.to_string())?;
    let watches = store
        .list_due(VIKTOR_WATCH_MAX_PER_TICK)
        .map_err(|error| error.to_string())?;
    tracing::debug!(
        due = watches.len(),
        max_watches = VIKTOR_WATCH_MAX_PER_TICK,
        max_calls = VIKTOR_WATCH_MAX_CALLS_PER_TICK,
        budget_secs = VIKTOR_WATCH_TICK_BUDGET_SECS,
        "Viktor inbound watch tick"
    );
    let mut delivered = 0;
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_secs(VIKTOR_WATCH_TICK_BUDGET_SECS);
    for watch in watches {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, poll_one(cas_root, proxy, &store, &watch)).await {
            Ok(Ok(true)) => delivered += 1,
            Ok(Ok(false)) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                store
                    .record_poll(
                        watch.id,
                        VIKTOR_WATCH_POLL_INTERVAL_SECS,
                        None,
                        Some("Viktor watch tick exhausted its 20s wall-clock budget"),
                    )
                    .map_err(|error| error.to_string())?;
                break;
            }
        }
    }
    Ok(delivered)
}

async fn poll_one(
    cas_root: &Path,
    proxy: &cmcp_core::ProxyEngine,
    store: &SqliteViktorWatchStore,
    watch: &ViktorThreadWatch,
) -> Result<bool, String> {
    let role = watch
        .requesting_agent_role
        .parse::<AgentRole>()
        .unwrap_or(AgentRole::Standard);
    let caller = cmcp_core::ProxyCaller {
        agent_id: watch.requesting_agent_id.clone(),
        role,
        session_id: watch.requesting_agent_id.clone(),
        factory_session: watch.factory_session.clone(),
        active_task_ids: watch.task_id.iter().cloned().collect(),
    };
    let args = Map::from_iter([("run_id".to_string(), Value::String(watch.run_id.clone()))]);
    let run = match proxy
        .call_tool(&caller, "viktor", "get_run", Some(args.clone()))
        .await
    {
        Ok(value) => value,
        Err(error) => {
            store
                .record_poll(
                    watch.id,
                    VIKTOR_WATCH_POLL_INTERVAL_SECS,
                    None,
                    Some(&bounded_error(&error.to_string())),
                )
                .map_err(|error| error.to_string())?;
            return Ok(false);
        }
    };
    let status = find_string_by_key(&run, "status")
        .unwrap_or_else(|| "unknown".to_string())
        .to_ascii_lowercase();
    if !TERMINAL_RUN_STATES.contains(&status.as_str()) {
        store
            .record_poll(watch.id, VIKTOR_WATCH_POLL_INTERVAL_SECS, None, None)
            .map_err(|error| error.to_string())?;
        return Ok(false);
    }

    let payload = if matches!(status.as_str(), "completed" | "requires_action") {
        match proxy
            .call_tool(&caller, "viktor", "get_run_result", Some(args))
            .await
        {
            Ok(result) => result,
            Err(error) => {
                store
                    .record_poll(
                        watch.id,
                        VIKTOR_WATCH_POLL_INTERVAL_SECS,
                        None,
                        Some(&bounded_error(&error.to_string())),
                    )
                    .map_err(|error| error.to_string())?;
                return Ok(false);
            }
        }
    } else {
        run
    };

    let Some((target, fallback)) = delivery_target(cas_root, watch) else {
        store
            .mark_undeliverable(
                watch.id,
                "requesting agent ended and no live session supervisor was registered",
            )
            .map_err(|error| error.to_string())?;
        return Ok(false);
    };
    let body = render_provider_payload(&payload);
    let fallback_line = if fallback {
        format!(
            "Requesting agent '{}' is no longer live; routed to its factory supervisor.\n",
            watch.requesting_agent_name
        )
    } else {
        String::new()
    };
    let prompt = format!(
        "<viktor-reply thread_id=\"{}\" run_id=\"{}\" status=\"{}\">\n{}{}\n</viktor-reply>",
        watch.thread_id, watch.run_id, status, fallback_line, body
    );
    let summary = format!("Viktor reply: {} / {}", watch.thread_id, watch.run_id);
    let queue = SqlitePromptQueueStore::open(cas_root).map_err(|error| error.to_string())?;
    queue.init().map_err(|error| error.to_string())?;
    let enqueued = queue
        .enqueue_idempotent(
            "viktor",
            &target,
            &prompt,
            watch.factory_session.as_deref(),
            Some(&summary),
            Some(cas_store::NotificationPriority::High),
            &format!("viktor-watch:{}", watch.id),
            Some(&cas_store::QueueOrigin::Daemon),
        )
        .map_err(|error| error.to_string())?;
    let notification_id = match enqueued {
        EnqueueIdempotentResult::Created(id) | EnqueueIdempotentResult::AlreadyExists(id) => id,
    };
    store
        .mark_delivered(watch.id, notification_id)
        .map_err(|error| error.to_string())?;
    let _ = cas_factory::notify_daemon(cas_root);
    Ok(true)
}

fn delivery_target(cas_root: &Path, watch: &ViktorThreadWatch) -> Option<(String, bool)> {
    let store = crate::store::open_agent_store(cas_root).ok()?;
    if let Ok(agent) = store.get(&watch.requesting_agent_id)
        && matches!(agent.status, AgentStatus::Active | AgentStatus::Idle)
    {
        return Some((agent.name, false));
    }
    let session = watch.factory_session.as_deref()?;
    store.list(None).ok()?.into_iter().find_map(|agent| {
        (agent.factory_session.as_deref() == Some(session)
            && matches!(agent.role, AgentRole::Supervisor | AgentRole::Director)
            && matches!(agent.status, AgentStatus::Active | AgentStatus::Idle))
        .then_some((agent.name, true))
    })
}

fn bounded_error(message: &str) -> String {
    message.chars().take(512).collect()
}

fn render_provider_payload(value: &Value) -> String {
    let mut texts = Vec::new();
    collect_content_text(value, &mut texts);
    let rendered = if texts.is_empty() {
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    } else {
        texts.join("\n\n")
    };
    rendered.chars().take(16_000).collect()
}

fn provider_items(value: &Value, collection_key: &str) -> Vec<Value> {
    let decoded = decode_provider_payload(value);
    match decoded {
        Value::Array(items) => items,
        Value::Object(mut object) => object
            .remove(collection_key)
            .and_then(|items| items.as_array().cloned())
            .or_else(|| {
                object
                    .values()
                    .find_map(|child| find_array_by_key(child, collection_key))
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn decode_provider_payload(value: &Value) -> Value {
    if let Value::Object(object) = value
        && let Some(Value::Array(content)) = object.get("content")
        && let Some(text) = content.iter().find_map(|item| {
            item.as_object()
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str)
        })
        && let Ok(decoded) = serde_json::from_str(text)
    {
        return decoded;
    }
    value.clone()
}

fn find_array_by_key(value: &Value, key: &str) -> Option<Vec<Value>> {
    match value {
        Value::Object(object) => object
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .or_else(|| {
                object
                    .values()
                    .find_map(|child| find_array_by_key(child, key))
            }),
        Value::Array(items) => items.iter().find_map(|child| find_array_by_key(child, key)),
        _ => None,
    }
}

fn item_id(value: &Value, flat_key: &str) -> Option<String> {
    value
        .as_object()
        .and_then(|object| object.get(flat_key).or_else(|| object.get("id")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| find_string_by_key(value, flat_key))
}

fn collect_content_text(value: &Value, texts: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(text)) = object.get("text") {
                texts.push(text.clone());
            } else {
                for child in object.values() {
                    collect_content_text(child, texts);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_content_text(item, texts);
            }
        }
        _ => {}
    }
}

fn find_identifier(value: &Value, flat_key: &str, object_key: &str) -> Option<String> {
    find_string_by_key(value, flat_key).or_else(|| find_nested_id(value, object_key))
}

fn find_nested_id(value: &Value, object_key: &str) -> Option<String> {
    match value {
        Value::Object(object) => {
            if let Some(Value::Object(nested)) = object.get(object_key)
                && let Some(id) = nested.get("id").and_then(Value::as_str)
            {
                return Some(id.to_string());
            }
            object
                .values()
                .find_map(|child| find_nested_id(child, object_key))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_nested_id(child, object_key)),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|parsed| find_nested_id(&parsed, object_key)),
        _ => None,
    }
}

fn find_string_by_key(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(object) => {
            if let Some(text) = object.get(key).and_then(Value::as_str) {
                return Some(text.to_string());
            }
            object
                .values()
                .find_map(|child| find_string_by_key(child, key))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_string_by_key(child, key)),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|parsed| find_string_by_key(&parsed, key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::PromptQueueStore;
    use cas_types::{Agent, AgentRole, AgentStatus};
    use std::collections::HashMap;

    #[test]
    fn extracts_viktor_ids_from_mcp_text_content() {
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "{\"thread\":{\"id\":\"th-1\"},\"run\":{\"id\":\"run-1\"},\"message\":{\"id\":\"msg-1\"}}"}]
        });
        assert_eq!(
            find_identifier(&result, "thread_id", "thread").as_deref(),
            Some("th-1")
        );
        assert_eq!(
            find_identifier(&result, "run_id", "run").as_deref(),
            Some("run-1")
        );
        assert_eq!(
            find_identifier(&result, "message_id", "message").as_deref(),
            Some("msg-1")
        );
    }

    #[test]
    fn terminal_states_match_viktor_contract() {
        for state in [
            "completed",
            "requires_action",
            "failed",
            "cancelled",
            "timed_out",
        ] {
            assert!(TERMINAL_RUN_STATES.contains(&state));
        }
        assert!(!TERMINAL_RUN_STATES.contains(&"running"));
        assert_eq!(VIKTOR_WATCH_MAX_CALLS_PER_TICK, 32);
        assert_eq!(VIKTOR_WATCH_TICK_BUDGET_SECS, 20);
        assert_eq!(VIKTOR_INBOUND_DISCOVERY_SCAN_THREADS, 32);
        assert_eq!(VIKTOR_INBOUND_DISCOVERY_MAX_THREADS, 4);
        assert_eq!(VIKTOR_INBOUND_DISCOVERY_MAX_CALLS, 5);
        assert_eq!(VIKTOR_INBOUND_DISCOVERY_BUDGET_SECS, 4);
    }

    #[tokio::test]
    async fn daemon_poll_delivers_and_falls_back_to_the_session_supervisor() {
        let temp = tempfile::tempdir().unwrap();
        let agents = crate::store::open_agent_store(temp.path()).unwrap();
        let mut worker = Agent::new_with_role(
            "worker-session".to_string(),
            "worker-1".to_string(),
            AgentRole::Worker,
        );
        worker.factory_session = Some("factory-1".to_string());
        agents.register(&worker).unwrap();
        let mut supervisor = Agent::new_with_role(
            "supervisor-session".to_string(),
            "supervisor".to_string(),
            AgentRole::Supervisor,
        );
        supervisor.factory_session = Some("factory-1".to_string());
        agents.register(&supervisor).unwrap();

        let fixture = crate::test_paths::crate_root()
            .join("tests/fixtures/mock_mcp_viktor_thread_server.py");
        let config = cmcp_core::config::ServerConfig::Stdio {
            command: "python3".to_string(),
            args: vec![fixture.display().to_string()],
            env: HashMap::new(),
        };
        let engine =
            cmcp_core::ProxyEngine::from_configs(HashMap::from([("viktor".to_string(), config)]))
                .await
                .unwrap();
        engine
            .set_call_observer(std::sync::Arc::new(ViktorWatchRecorder::new(
                temp.path().to_path_buf(),
            )))
            .await;
        let caller = cmcp_core::ProxyCaller {
            agent_id: worker.id.clone(),
            role: AgentRole::Worker,
            session_id: worker.id.clone(),
            factory_session: worker.factory_session.clone(),
            active_task_ids: vec!["cas-fixture".to_string()],
        };
        engine
            .call_tool(
                &caller,
                "viktor",
                "create_thread",
                Some(Map::from_iter([(
                    "message".to_string(),
                    Value::String("reply asynchronously".to_string()),
                )])),
            )
            .await
            .unwrap();

        let watches = SqliteViktorWatchStore::open(temp.path())
            .unwrap()
            .list_live()
            .unwrap();
        assert_eq!(watches.len(), 1, "proxy completion must arm one watch");
        assert_eq!(poll_due_watches(temp.path(), &engine).await.unwrap(), 1);
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let delivered = queue.poll_for_target("worker-1", 10).unwrap();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].source, "viktor");
        assert!(delivered[0].prompt.contains("thread-fixture-1"));
        assert!(delivered[0].prompt.contains("run-fixture-1"));
        assert!(delivered[0].prompt.contains("fixture Viktor reply"));

        let store = SqliteViktorWatchStore::open(temp.path()).unwrap();
        store
            .record(
                "thread-fixture-2",
                "run-fixture-2",
                &worker.id,
                &worker.name,
                "worker",
                worker.factory_session.as_deref(),
                Some("cas-fixture"),
                None,
                cas_store::DEFAULT_VIKTOR_WATCH_TTL_SECS,
            )
            .unwrap();
        worker.status = AgentStatus::Shutdown;
        agents.update(&worker).unwrap();
        assert_eq!(poll_due_watches(temp.path(), &engine).await.unwrap(), 1);
        let fallback = queue.poll_for_target("supervisor", 10).unwrap();
        assert_eq!(fallback.len(), 1);
        assert!(
            fallback[0]
                .prompt
                .contains("routed to its factory supervisor")
        );
        assert!(fallback[0].prompt.contains("run-fixture-2"));

        engine.shutdown().await;
    }

    async fn inbound_fixture_engine(cas_root: &Path) -> cmcp_core::ProxyEngine {
        let fixture = crate::test_paths::crate_root()
            .join("tests/fixtures/mock_mcp_viktor_inbound_server.py");
        let config = cmcp_core::config::ServerConfig::Stdio {
            command: "python3".to_string(),
            args: vec![fixture.display().to_string()],
            env: HashMap::new(),
        };
        let engine =
            cmcp_core::ProxyEngine::from_configs(HashMap::from([("viktor".to_string(), config)]))
                .await
                .unwrap();
        engine
            .set_call_observer(std::sync::Arc::new(ViktorWatchRecorder::new(
                cas_root.to_path_buf(),
            )))
            .await;
        engine
    }

    #[tokio::test]
    async fn provider_originated_thread_reaches_live_supervisor_once_and_can_reply() {
        let temp = tempfile::tempdir().unwrap();
        let agents = crate::store::open_agent_store(temp.path()).unwrap();
        let mut other_supervisor = Agent::new_with_role(
            "other-supervisor-session".to_string(),
            "other-supervisor".to_string(),
            AgentRole::Supervisor,
        );
        other_supervisor.factory_session = Some("factory-other".to_string());
        agents.register(&other_supervisor).unwrap();
        let mut supervisor = Agent::new_with_role(
            "supervisor-session".to_string(),
            "live-supervisor".to_string(),
            AgentRole::Supervisor,
        );
        supervisor.factory_session = Some("factory-inbound".to_string());
        supervisor.last_heartbeat = other_supervisor.last_heartbeat + chrono::Duration::seconds(1);
        agents.register(&supervisor).unwrap();

        // The second fixture thread was opened by CAS earlier. Discovery must
        // leave that existing watch path alone, including after its run ends.
        let watch_store = SqliteViktorWatchStore::open(temp.path()).unwrap();
        let prior_watch = watch_store
            .record(
                "thread-cas-opened",
                "run-cas-opened",
                &supervisor.id,
                &supervisor.name,
                "supervisor",
                supervisor.factory_session.as_deref(),
                None,
                None,
                cas_store::DEFAULT_VIKTOR_WATCH_TTL_SECS,
            )
            .unwrap();
        watch_store.mark_delivered(prior_watch, 404).unwrap();

        let engine = inbound_fixture_engine(temp.path()).await;
        assert_eq!(
            discover_originated_messages(temp.path(), &engine)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            discover_originated_messages(temp.path(), &engine)
                .await
                .unwrap(),
            0,
            "the same provider message id must not enqueue twice"
        );

        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let messages = queue
            .poll_for_target_with_session(
                &supervisor.name,
                supervisor.factory_session.as_deref(),
                10,
            )
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].source, "viktor");
        assert_eq!(
            messages[0].factory_session.as_deref(),
            Some("factory-inbound")
        );
        assert!(messages[0].prompt.contains("thread-viktor-originated"));
        assert!(messages[0].prompt.contains("message-viktor-question"));
        assert!(
            messages[0]
                .prompt
                .contains("Can Cassy answer this question on-thread?")
        );
        assert!(messages[0].prompt.contains("send_message"));
        assert!(!messages[0].prompt.contains("thread-cas-opened"));
        assert!(
            queue
                .poll_for_target_with_session(
                    &other_supervisor.name,
                    other_supervisor.factory_session.as_deref(),
                    10,
                )
                .unwrap()
                .is_empty(),
            "an originated question must route to one supervisor session, not broadcast"
        );

        // No new write surface is needed: the existing send_message route is
        // still observed and arms the ordinary watched-run reply path.
        let caller = cmcp_core::ProxyCaller {
            agent_id: supervisor.id.clone(),
            role: AgentRole::Supervisor,
            session_id: supervisor.id.clone(),
            factory_session: supervisor.factory_session.clone(),
            active_task_ids: vec![],
        };
        engine
            .call_tool(
                &caller,
                "viktor",
                "send_message",
                Some(Map::from_iter([
                    (
                        "thread_id".to_string(),
                        Value::String("thread-viktor-originated".to_string()),
                    ),
                    (
                        "message".to_string(),
                        Value::String("Cassy's on-thread answer".to_string()),
                    ),
                ])),
            )
            .await
            .unwrap();
        assert!(
            SqliteViktorWatchStore::open(temp.path())
                .unwrap()
                .list_live()
                .unwrap()
                .iter()
                .any(|watch| watch.run_id == "run-cassy-reply")
        );
        engine.shutdown().await;
    }

    #[tokio::test]
    async fn no_live_supervisor_is_durable_and_surfaces_once_at_session_start() {
        let temp = tempfile::tempdir().unwrap();
        SqliteViktorWatchStore::open(temp.path())
            .unwrap()
            .record(
                "thread-cas-opened",
                "run-cas-opened",
                "ended-agent",
                "ended-agent",
                "standard",
                None,
                None,
                None,
                cas_store::DEFAULT_VIKTOR_WATCH_TTL_SECS,
            )
            .unwrap();
        let engine = inbound_fixture_engine(temp.path()).await;
        assert_eq!(
            discover_originated_messages(temp.path(), &engine)
                .await
                .unwrap(),
            0,
            "discovery cannot claim live delivery when no supervisor exists"
        );
        let pending = SqliteViktorInboundStore::open(temp.path())
            .unwrap()
            .list_pending(8)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].last_error.as_deref(),
            Some("no live factory supervisor was registered at discovery time")
        );

        let warning = surface_inbound_at_session_start(temp.path(), Some("factory-next"))
            .expect("the next supervisor SessionStart must surface the durable question");
        assert!(warning.contains("while no live Cassy supervisor could receive"));
        assert!(warning.contains("thread-viktor-originated"));
        assert!(warning.contains("Can Cassy answer this question on-thread?"));
        assert!(warning.contains("send_message"));
        assert!(
            surface_inbound_at_session_start(temp.path(), Some("factory-next")).is_none(),
            "SessionStart surfacing must be exactly once"
        );
        assert!(
            SqliteViktorInboundStore::open(temp.path())
                .unwrap()
                .list_pending(8)
                .unwrap()
                .is_empty()
        );
        engine.shutdown().await;
    }

    #[tokio::test]
    async fn restart_with_absent_viktor_notifies_supervisor_with_durable_run_ids() {
        let temp = tempfile::tempdir().unwrap();
        let agents = crate::store::open_agent_store(temp.path()).unwrap();
        let mut supervisor = Agent::new_with_role(
            "supervisor-session".to_string(),
            "supervisor".to_string(),
            AgentRole::Supervisor,
        );
        supervisor.factory_session = Some("factory-1".to_string());
        agents.register(&supervisor).unwrap();

        // This record stands in for the previous daemon's successful
        // run-starting call. It must survive the replacement daemon even when
        // that daemon has no credential with which to reconnect Viktor.
        let watch_store = SqliteViktorWatchStore::open(temp.path()).unwrap();
        watch_store
            .record(
                "thread-before-restart",
                "run-before-restart",
                "worker-session",
                "worker-1",
                "worker",
                Some("factory-1"),
                Some("cas-fixture"),
                None,
                cas_store::DEFAULT_VIKTOR_WATCH_TTL_SECS,
            )
            .unwrap();

        let replacement = cmcp_core::ProxyEngine::from_configs(HashMap::from([(
            "viktor".to_string(),
            cmcp_core::config::ServerConfig::Http {
                url: "https://example.invalid/mcp".to_string(),
                auth: Some("env:CAS_TEST_MISSING_VIKTOR_KEY_8563".to_string()),
                headers: HashMap::new(),
                oauth: false,
            },
        )]))
        .await
        .unwrap();
        assert!(!replacement.upstream_connected("viktor").await);

        assert_eq!(
            alert_unpollable_watches(temp.path(), &replacement)
                .await
                .unwrap(),
            1
        );
        // The receipt is restart-idempotent: the same durable run does not
        // create a second high-priority alert every time the daemon starts.
        assert_eq!(
            alert_unpollable_watches(temp.path(), &replacement)
                .await
                .unwrap(),
            1
        );
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let messages = queue.poll_for_target("supervisor", 10).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].prompt.contains("Viktor upstream is absent"));
        assert!(messages[0].prompt.contains("run-before-restart"));
        assert_eq!(watch_store.list_live().unwrap().len(), 1);

        replacement.shutdown().await;
    }
}
