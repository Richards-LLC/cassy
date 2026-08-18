//! Viktor's inbound half: record run-starting proxy calls and poll them from
//! the embedded daemon into the ordinary CAS prompt queue.

use std::path::{Path, PathBuf};

use cas_store::{
    EnqueueIdempotentResult, PromptQueueStore, SqlitePromptQueueStore, SqliteViktorWatchStore,
    ViktorThreadWatch,
};
use cas_types::{AgentRole, AgentStatus};
use serde_json::{Map, Value};

pub(crate) const VIKTOR_WATCH_POLL_INTERVAL_SECS: i64 = 30;
pub(crate) const VIKTOR_WATCH_MAX_PER_TICK: usize = 16;
pub(crate) const VIKTOR_WATCH_MAX_CALLS_PER_TICK: usize = VIKTOR_WATCH_MAX_PER_TICK * 2;
pub(crate) const VIKTOR_WATCH_TICK_BUDGET_SECS: u64 = 20;

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

        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
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

}
