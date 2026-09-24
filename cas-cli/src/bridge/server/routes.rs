use anyhow::{Context, Result};
use cas_store::{EventStore, SqliteEventStore};
use tiny_http::{Method, Response, StatusCode};

use crate::bridge::server::http::{error_response, json_response, json_response_bytes, url_decode};
use crate::bridge::server::session::{
    allowed_agent_names, cas_root_for_session_with_fallback, filter_events_for_session_agents,
    resolve_session_by_name, wait_for_supervisor_ack,
};
use crate::bridge::server::types::{
    ActivityJson, AgentLatestActivityJson, AgentSummaryJson, InboxAckJson, InboxAckRequest,
    InboxCountJson, InboxPeekJson, InboxPollJson, MessageRequest, MessageResponse, PaneTailJson,
    PingJson, StatusJson, TargetsJson, TaskSummaryJson, session_json,
};
use crate::store::{open_prompt_queue_store, open_supervisor_queue_store};

pub(crate) fn handle_session_routes(
    req: &mut tiny_http::Request,
    path: &str,
    query: &str,
    cors_allow_origin: Option<&str>,
    fallback_cas_root: Option<&std::path::Path>,
    status_cache: &std::sync::Mutex<
        std::collections::HashMap<String, (std::time::Instant, Vec<u8>)>,
    >,
    status_cache_ttl_ms: u64,
) -> Result<Response<std::io::Cursor<Vec<u8>>>> {
    // Expected:
    // /v1/sessions/<name>/targets
    // /v1/sessions/<name>/status
    // /v1/sessions/<name>/message
    // /v1/sessions/<name>/ping
    // /v1/sessions/<name>/activity?since_id=&limit=
    // /v1/sessions/<name>/inbox/<inbox_id>/peek?limit=
    // /v1/sessions/<name>/inbox/<inbox_id>/poll?limit=
    // /v1/sessions/<name>/inbox/<inbox_id>/ack  JSON body: {notification_id}
    // /v1/sessions/<name>/inbox/<inbox_id>/pending_count
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 5 {
        return Ok(error_response(
            StatusCode(404),
            "not_found",
            "Invalid session route",
            cors_allow_origin,
        ));
    }
    let session_name = parts[3];
    let action = parts[4];

    let session = resolve_session_by_name(session_name)?;
    let sj = session_json(&session);

    // Pane tail routes: /v1/sessions/<name>/panes/<pane_id>/tail
    if action == "panes" && parts.len() >= 7 && parts[6] == "tail" {
        let pane_id = parts[5];
        let mut lines_limit: usize = 50;
        for pair in query.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = url_decode(v);
            if k == "lines" {
                if let Ok(n) = v.parse::<usize>() {
                    lines_limit = n.clamp(1, 1000);
                }
            }
        }

        let tail_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".cas")
            .join("sessions")
            .join(session_name)
            .join("pane-tail");
        let snapshot_path = tail_dir.join(format!("{pane_id}.txt"));

        let lines = if snapshot_path.exists() {
            match std::fs::read_to_string(&snapshot_path) {
                Ok(content) => {
                    let all_lines: Vec<String> =
                        content.lines().map(|l| l.to_string()).collect();
                    let start = all_lines.len().saturating_sub(lines_limit);
                    all_lines[start..].to_vec()
                }
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        return Ok(json_response(
            StatusCode(200),
            &PaneTailJson {
                schema_version: 1,
                session: session_name.to_string(),
                pane_id: pane_id.to_string(),
                lines,
            },
            cors_allow_origin,
        ));
    }

    // Inbox routes are nested: /v1/sessions/<name>/inbox/<inbox_id>/<op>
    if action == "inbox" {
        let cas_root = cas_root_for_session_with_fallback(&session, fallback_cas_root)?;

        if parts.len() < 7 {
            return Ok(error_response(
                StatusCode(404),
                "not_found",
                "Invalid inbox route",
                cors_allow_origin,
            ));
        }
        let inbox_id = parts[5].to_string();
        let op = parts[6];

        let queue = open_supervisor_queue_store(&cas_root)?;

        let mut limit: usize = 25;
        for pair in query.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = url_decode(v);
            if k == "limit" && !v.is_empty() {
                if let Ok(n) = v.parse::<usize>() {
                    limit = n.clamp(1, 200);
                }
            }
        }

        return match (req.method(), op) {
            (&Method::Get, "pending_count") => {
                let pending = queue.pending_count(&inbox_id)?;
                Ok(json_response(
                    StatusCode(200),
                    &InboxCountJson {
                        schema_version: 1,
                        session: sj,
                        inbox_id,
                        pending_count: pending,
                    },
                    cors_allow_origin,
                ))
            }

            (&Method::Get, "peek") => {
                let notifications = queue.peek(&inbox_id, limit)?;
                let pending = queue.pending_count(&inbox_id)?;
                Ok(json_response(
                    StatusCode(200),
                    &InboxPeekJson {
                        schema_version: 1,
                        session: sj,
                        inbox_id,
                        pending_count: pending,
                        notifications,
                    },
                    cors_allow_origin,
                ))
            }

            (&Method::Post, "poll") => {
                let notifications = queue.poll(&inbox_id, limit)?;
                Ok(json_response(
                    StatusCode(200),
                    &InboxPollJson {
                        schema_version: 1,
                        session: sj,
                        inbox_id,
                        polled: notifications.len(),
                        notifications,
                    },
                    cors_allow_origin,
                ))
            }

            (&Method::Post, "ack") => {
                let mut body = String::new();
                req.as_reader().read_to_string(&mut body)?;
                let ack: InboxAckRequest = serde_json::from_str(&body)
                    .with_context(|| "Invalid JSON body for inbox ack request")?;
                queue.ack(ack.notification_id)?;
                Ok(json_response(
                    StatusCode(200),
                    &InboxAckJson {
                        schema_version: 1,
                        session: sj,
                        inbox_id,
                        acked: true,
                        notification_id: ack.notification_id,
                    },
                    cors_allow_origin,
                ))
            }

            _ => Ok(error_response(
                StatusCode(404),
                "not_found",
                "Unknown inbox route",
                cors_allow_origin,
            )),
        };
    }

    match (req.method(), action) {
        (&Method::Get, "ping") | (&Method::Post, "ping") => {
            let cas_root = cas_root_for_session_with_fallback(&session, fallback_cas_root)
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            Ok(json_response(
                StatusCode(200),
                &PingJson {
                    schema_version: 1,
                    ok: true,
                    session: sj,
                    cas_root,
                },
                cors_allow_origin,
            ))
        }

        (&Method::Get, "targets") => {
            let supervisor_actual = session.metadata.supervisor.name.clone();
            let workers: Vec<String> = session
                .metadata
                .workers
                .iter()
                .map(|w| w.name.clone())
                .collect();
            let mut aliases = std::collections::HashMap::new();
            aliases.insert("supervisor".to_string(), supervisor_actual.clone());
            aliases.insert("all_workers".to_string(), "all_workers".to_string());

            Ok(json_response(
                StatusCode(200),
                &TargetsJson {
                    schema_version: 1,
                    session: sj,
                    supervisor: supervisor_actual,
                    workers,
                    aliases,
                },
                cors_allow_origin,
            ))
        }

        (&Method::Get, "activity") => {
            let cas_root = cas_root_for_session_with_fallback(&session, fallback_cas_root)?;
            let allowed = allowed_agent_names(&session);

            let mut since_id: Option<i64> = None;
            let mut limit: usize = 50;
            for pair in query.split('&').filter(|s| !s.is_empty()) {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                let v = url_decode(v);
                match k {
                    "since_id" if !v.is_empty() => since_id = v.parse::<i64>().ok(),
                    "limit" if !v.is_empty() => {
                        if let Ok(n) = v.parse::<usize>() {
                            limit = n.clamp(1, 200);
                        }
                    }
                    _ => {}
                }
            }

            let store = SqliteEventStore::open(&cas_root)?;
            let mut activity = store.list_recent(200)?;
            filter_events_for_session_agents(&mut activity, &allowed);
            if let Some(since) = since_id {
                activity.retain(|e| e.id > since);
            }
            if activity.len() > limit {
                activity.truncate(limit);
            }
            let latest_id = activity.iter().map(|e| e.id).max();

            Ok(json_response(
                StatusCode(200),
                &ActivityJson {
                    schema_version: 1,
                    session: sj,
                    activity,
                    latest_id,
                },
                cors_allow_origin,
            ))
        }

        (&Method::Get, "status") => {
            let cas_root = cas_root_for_session_with_fallback(&session, fallback_cas_root)?;
            let cache_key = session.name.clone();
            if let Ok(c) = status_cache.lock() {
                if let Some((ts, bytes)) = c.get(&cache_key) {
                    if ts.elapsed() < std::time::Duration::from_millis(status_cache_ttl_ms) {
                        return Ok(json_response_bytes(
                            StatusCode(200),
                            bytes.clone(),
                            cors_allow_origin,
                        ));
                    }
                }
            }
            let allowed = allowed_agent_names(&session);

            let data = cas_factory::DirectorData::load_for_project(
                &cas_root,
                None,
                false,
                crate::cloud::resolve_canonical_id(&cas_root).as_deref(),
            )?;

            let mut activity = data.activity;
            filter_events_for_session_agents(&mut activity, &allowed);
            if activity.len() > 20 {
                activity.truncate(20);
            }

            let agents: Vec<AgentSummaryJson> = data
                .agents
                .into_iter()
                .filter(|a| allowed.contains(&a.name))
                .map(|a| AgentSummaryJson {
                    id: a.id,
                    name: a.name,
                    status: format!("{:?}", a.status).to_lowercase(),
                    current_task: a.current_task,
                    latest_activity: a.latest_activity.map(|(s, ts)| AgentLatestActivityJson {
                        summary: s,
                        created_at_rfc3339: ts.to_rfc3339(),
                    }),
                    last_heartbeat_rfc3339: a.last_heartbeat.map(|ts| ts.to_rfc3339()),
                })
                .collect();

            let to_task = |t: cas_factory::TaskSummary| TaskSummaryJson {
                id: t.id,
                title: t.title,
                status: format!("{:?}", t.status).to_lowercase(),
                priority: t.priority.0,
                assignee: t.assignee,
                task_type: format!("{:?}", t.task_type).to_lowercase(),
                epic: t.epic,
                branch: t.branch,
            };

            let queue = open_prompt_queue_store(&cas_root)?;
            let pending = queue.pending_count()?;

            let body = StatusJson {
                schema_version: 1,
                session: sj,
                prompt_queue_pending: pending,
                activity,
                agents,
                tasks_ready: data.ready_tasks.into_iter().map(to_task).collect(),
                tasks_in_progress: data.in_progress_tasks.into_iter().map(to_task).collect(),
                epics: data.epic_tasks.into_iter().map(to_task).collect(),
            };

            let bytes = serde_json::to_vec_pretty(&body).unwrap_or_else(|_| b"{}".to_vec());
            if let Ok(mut c) = status_cache.lock() {
                c.insert(cache_key, (std::time::Instant::now(), bytes.clone()));
            }
            Ok(json_response_bytes(
                StatusCode(200),
                bytes,
                cors_allow_origin,
            ))
        }

        (&Method::Post, "message") => {
            let cas_root = cas_root_for_session_with_fallback(&session, fallback_cas_root)?;

            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            let msg: MessageRequest = serde_json::from_str(&body)
                .with_context(|| "Invalid JSON body for message request")?;

            let from = msg.from.unwrap_or_else(|| "openclaw".to_string());
            let resolved_target = if msg.target == "supervisor" {
                session.metadata.supervisor.name.clone()
            } else {
                msg.target.clone()
            };

            let payload = if msg.no_wrap {
                msg.message
            } else {
                let response_hint = format!(
                    "To respond, use: coordination action=message target={} message=\"...\"\n\nDO NOT USE SENDMESSAGE.",
                    from.trim()
                );
                format!("{}\n\n{}", msg.message.trim_end(), response_hint)
            };

            let queue = open_prompt_queue_store(&cas_root)?;
            let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
            let message_id = if let Some(ref session) = factory_session {
                queue.enqueue_with_session(&from, &resolved_target, &payload, session)?
            } else {
                queue.enqueue(&from, &resolved_target, &payload)?
            };

            let ack_event_id = if msg.wait_ack {
                wait_for_supervisor_ack(&cas_root, message_id, msg.timeout_ms.unwrap_or(5000))?
            } else {
                None
            };

            Ok(json_response(
                StatusCode(200),
                &MessageResponse {
                    schema_version: 1,
                    session: session.name,
                    target: resolved_target,
                    enqueued: true,
                    message_id,
                    ack_event_id,
                },
                cors_allow_origin,
            ))
        }

        _ => Ok(error_response(
            StatusCode(404),
            "not_found",
            "Unknown session route",
            cors_allow_origin,
        )),
    }
}
